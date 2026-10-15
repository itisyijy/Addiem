//! B2 / B2-j 가져오기. tools/import_reference.py (스파이크 1) 를 옮겼다.
//!
//! 원칙 (MVP 6.4)
//!   - 항목 하나가 깨져도 전체를 버리지 않는다. 몇 번째가 왜 실패했는지 전부 보고한다
//!   - 지원하지 않는 템플릿 기능은 조용히 바꾸지 않고 "확인이 필요한 카드" 로 보고한다
//!   - Anki 복습 기록은 옮기지 않는다. 사용자가 동의한 뒤에만 가져온다 (B2 화면)
//!   - 원본 파일은 읽기만 한다
//!
//! 중요도는 하위 덱 단위다. 태그는 카드가 아니라 덱의 제안으로 모은다 (B4 에서 확정)

use crate::commands::{insert_note, media_dir, mime_of, safe_media_name, AppState, NewCard};
use crate::db::{self, now_ms};
use crate::srs;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const SEP: char = '\x1f'; // Anki 가 덱 계층과 노트 필드를 나누는 구분자
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const DEFAULT_DECK: &str = "가져온 카드";

/// 화면 문구의 수는 천 단위 쉼표 (4,342개). 화면의 n() 과 같다
fn n(v: usize) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[derive(Serialize, Clone)]
pub struct Issue {
    /// skipped | warning | info
    severity: &'static str,
    kind: &'static str,
    detail: String,
    /// JSON 항목 번호 (1부터)
    index: Option<usize>,
}

fn issue(severity: &'static str, kind: &'static str, detail: impl Into<String>) -> Issue {
    Issue { severity, kind, detail: detail.into(), index: None }
}

#[derive(Serialize, Default)]
pub struct ImportResult {
    source: String,
    kind: String,
    /// 카드가 들어간 최상위 덱 수
    decks: usize,
    /// 이번에 새로 만든 하위 덱 수
    subdecks: usize,
    notes: usize,
    cards: usize,
    media: usize,
    skipped: usize,
    /// Anki 복습 기록 수. history 가 true 면 옮긴 수, 아니면 옮기지 않은 수
    reviews: usize,
    /// 복습 기록과 일정을 옮겼는가
    history: bool,
    filtered_decks: usize,
    filtered_cards: usize,
    /// 템플릿 기능 때문에 화면이 원본과 다를 수 있는 카드
    check_cards: usize,
    root_deck_id: Option<i64>,
    /// 카드가 들어간 최상위 덱 전부 (B4 를 차례로 연다)
    roots: Vec<i64>,
    issues: Vec<Issue>,
}

// ------------------------------------------------------------------ 중요도 규칙

/// 태그 -> 중요도. 실측 컬렉션의 `중요도::` 3단계가 우리 3단계와 같다
fn tag_level(tag: &str) -> Option<i64> {
    let t = tag.to_lowercase();
    match t.as_str() {
        "중요도::최우선" | "중요도::매우높음" | "star5" | "star4" | "star3" | "highlight" | "중요" | "핵심" | "important" => Some(2),
        "중요도::높음" | "star2" => Some(1),
        "중요도::보통" | "중요도::낮음" | "star1" => Some(0),
        _ => None,
    }
}

/// 여러 태그가 붙으면 가장 높은 것이 이긴다
fn tags_level(tags: &str) -> Option<(i64, String)> {
    tags.split_whitespace().filter_map(|t| tag_level(t).map(|l| (l, t.to_string()))).max_by_key(|(l, _)| *l)
}

/// 덱 이름 -> 중요도 (MVP 6.5.1 3순위)
fn deck_name_level(name: &str) -> Option<(i64, String)> {
    const CORE: [&str; 8] = ["요약", "핵심", "정리", "기출", "필수", "summary", "core", "essential"];
    const SUPP: [&str; 7] = ["부록", "참고", "심화", "보충", "appendix", "extra", "advanced"];
    let lower = name.to_lowercase();
    if let Some(k) = CORE.iter().find(|k| lower.contains(*k)) {
        return Some((2, format!("하위 덱 이름에 '{k}'이 있어요")));
    }
    SUPP.iter().find(|k| lower.contains(*k)).map(|k| (0, format!("하위 덱 이름에 '{k}'이 있어요")))
}

// ------------------------------------------------------------------ 덱 만들기 (두 경로 공통)

#[derive(Default)]
struct Decks {
    cache: HashMap<(Option<i64>, String), i64>,
    created: Vec<i64>,
}

impl Decks {
    /// 경로대로 찾거나 만든다. 같은 이름이 있으면 그 덱에 합친다 (Anki 와 같다)
    fn ensure(&mut self, con: &Connection, parent: Option<i64>, parts: &[String], now: i64) -> Res<i64> {
        let mut cur = parent;
        for name in parts {
            let key = (cur, name.clone());
            let id = match self.cache.get(&key) {
                Some(id) => *id,
                None => {
                    let found: Option<(i64, Option<i64>)> = con
                        .query_row("SELECT id, archived_at FROM deck WHERE parent_id IS ?1 AND name = ?2", params![cur, name], |r| {
                            Ok((r.get(0)?, r.get(1)?))
                        })
                        .optional()
                        .map_err(err)?;
                    let id = match found {
                        Some((id, archived)) => {
                            if archived.is_some() {
                                con.execute("UPDATE deck SET archived_at = NULL, updated_at = ?2 WHERE id = ?1", params![id, now]).map_err(err)?;
                            }
                            id
                        }
                        None => {
                            let (imp, src, reason) = match deck_name_level(name) {
                                Some((l, why)) => (l, "rule", Some(why)),
                                None => (1, "default", None),
                            };
                            con.execute(
                                "INSERT INTO deck (parent_id, name, created_at, updated_at, importance, importance_source, importance_reason)
                                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)",
                                params![cur, name, now, imp, src, reason],
                            )
                            .map_err(err)?;
                            let id = con.last_insert_rowid();
                            self.created.push(id);
                            id
                        }
                    };
                    self.cache.insert(key, id);
                    id
                }
            };
            cur = Some(id);
        }
        cur.ok_or_else(|| "덱 이름이 비어 있어요.".into())
    }
}

/// 카드가 들어간 덱에 태그 제안을 붙인다. 사용자가 정한 덱(user)과 태그 제안이 이미 있는 덱은 건드리지 않는다.
/// 덱 카드(이번에 넣은 것 포함 전체)의 절반 이상이 같은 중요도 태그를 달고 있으면 그 중요도를 제안한다
fn suggest_from_tags(con: &Connection, card_tags: &HashMap<i64, Vec<Option<(i64, String)>>>) -> Res<()> {
    for (id, list) in card_tags {
        let total: i64 = con.query_row("SELECT count(*) FROM card WHERE deck_id = ?1", [id], |r| r.get(0)).map_err(err)?;
        let total = (total as usize).max(list.len());
        let mut by_level: HashMap<i64, (usize, String)> = HashMap::new();
        for (l, tag) in list.iter().flatten() {
            let e = by_level.entry(*l).or_insert((0, tag.clone()));
            e.0 += 1;
        }
        if let Some((level, (n, tag))) = by_level.into_iter().max_by_key(|(l, (n, _))| (*n, *l))
            && n * 2 >= total
        {
            let pct = n * 100 / total;
            con.execute(
                "UPDATE deck SET importance = ?2, importance_source = 'tag', importance_reason = ?3 WHERE id = ?1 AND importance_source IN ('default','rule')",
                params![id, level, format!("태그 {tag} {pct}%")],
            )
            .map_err(err)?;
        }
    }
    Ok(())
}

fn finish_batch(con: &Connection, batch: i64, r: &ImportResult) -> Res<()> {
    for i in &r.issues {
        con.execute(
            "INSERT INTO import_issue (batch_id, severity, kind, detail, ref_label) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![batch, i.severity, i.kind, i.detail, i.index.map(|n| format!("#{n}"))],
        )
        .map_err(err)?;
    }
    con.execute(
        "UPDATE import_batch SET finished_at = ?2, status = ?3, deck_count = ?4, note_count = ?5, card_count = ?6, media_count = ?7, skipped_count = ?8 WHERE id = ?1",
        params![
            batch,
            now_ms(),
            if r.skipped > 0 { "partial" } else { "ok" },
            (r.decks + r.subdecks) as i64,
            r.notes as i64,
            r.cards as i64,
            r.media as i64,
            r.skipped as i64
        ],
    )
    .map_err(err)?;
    Ok(())
}

/// 카드가 들어간 덱들의 최상위 덱. 하나면 B3 에서 그 덱으로 간다
fn roots_of(con: &Connection, deck_ids: &HashSet<i64>) -> Res<BTreeSet<i64>> {
    let mut roots = BTreeSet::new();
    for id in deck_ids {
        let mut cur = *id;
        for _ in 0..64 {
            let parent: Option<i64> = con.query_row("SELECT parent_id FROM deck WHERE id = ?1", [cur], |r| r.get(0)).map_err(err)?;
            match parent {
                Some(p) => cur = p,
                None => break,
            }
        }
        roots.insert(cur);
    }
    Ok(roots)
}

// ------------------------------------------------------------------ JSON (docs/engineering/json-import-format.md)

struct JsonCard {
    index: usize,
    front: String,
    back: String,
    deck: Vec<String>,
    tags: String,
    importance: Option<i64>,
    /// basic | reversed | cloze (NewCard.kind)
    kind: &'static str,
}

fn split_path(p: &str) -> Vec<String> {
    p.split("::").map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

fn has_cloze(s: &str) -> bool {
    s.match_indices("{{c").any(|(i, _)| s[i + 3..].starts_with(|c: char| c.is_ascii_digit()) && s[i + 3..].contains("::"))
}

/// 반환: (가져올 카드, 문제 목록, 전체 실패 이유)
fn parse_json(text: &str) -> (Vec<JsonCard>, Vec<Issue>, Option<String>) {
    let doc: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (vec![], vec![], Some(format!("JSON 형식이 올바르지 않아요 ({}번째 줄 {}번째 글자)", e.line(), e.column()))),
    };
    let (items, default_deck, default_type) = match &doc {
        Value::Array(a) => (a, None, "Basic".to_string()),
        Value::Object(o) => match o.get("cards") {
            Some(Value::Array(a)) => (
                a,
                o.get("deck").and_then(Value::as_str).map(String::from),
                o.get("note_type").and_then(Value::as_str).unwrap_or("Basic").to_string(),
            ),
            _ => return (vec![], vec![], Some("cards 가 배열이 아니에요.".into())),
        },
        _ => return (vec![], vec![], Some("최상위가 배열도 객체도 아니에요.".into())),
    };
    if items.is_empty() {
        return (vec![], vec![], Some("가져올 카드가 없어요.".into()));
    }

    let mut out = vec![];
    let mut issues = vec![];
    let mut seen: HashMap<String, usize> = HashMap::new();
    let at = |i: usize, sev: &'static str, kind: &'static str, d: String| Issue { severity: sev, kind, detail: d, index: Some(i) };
    for (n, item) in items.iter().enumerate() {
        let i = n + 1;
        let Value::Object(o) = item else {
            issues.push(at(i, "skipped", "bad_item", "항목이 객체가 아니에요".into()));
            continue;
        };
        let note_type = o.get("note_type").and_then(Value::as_str).unwrap_or(&default_type);
        let front = o.get("front");
        let cloze = note_type.eq_ignore_ascii_case("cloze") || front.and_then(Value::as_str).is_some_and(has_cloze);
        // 빈칸 카드는 back 이 없어도 된다 (추가 설명)
        let mut bad = None;
        for (key, val, required) in [("front", front, true), ("back", o.get("back"), !cloze)] {
            match val {
                None | Some(Value::Null) => {
                    if required {
                        bad = Some((key, "missing_field", format!("{key} 키가 없어요")));
                    }
                }
                Some(Value::String(s)) => {
                    if required && crate::render::is_blank(s) {
                        bad = Some((key, "empty_field", format!("{key} 이 비어 있어요")));
                    }
                }
                Some(_) => bad = Some((key, "bad_field", format!("{key} 가 문자열이 아니에요"))),
            }
            if bad.is_some() {
                break;
            }
        }
        if let Some((_, kind, d)) = bad {
            issues.push(at(i, "skipped", kind, d));
            continue;
        }
        let front = front.and_then(Value::as_str).unwrap_or_default().to_string();
        let back = o.get("back").and_then(Value::as_str).unwrap_or_default().to_string();
        if cloze && !has_cloze(&front) {
            issues.push(at(i, "skipped", "no_cloze", "빈칸({{c1::...}})이 없어요".into()));
            continue;
        }
        // 앞뒤 둘 다: 노트 하나에서 카드 두 장 (앞 → 뒤, 뒤 → 앞)
        let reversed = ["reversed", "Basic (and reversed card)"].iter().any(|t| note_type.eq_ignore_ascii_case(t));
        if !cloze && !reversed && !note_type.eq_ignore_ascii_case("basic") {
            issues.push(at(i, "info", "note_type", format!("노트타입 '{note_type}' 는 질문과 답변으로 가져왔어요")));
        }
        let kind = if cloze { "cloze" } else if reversed { "reversed" } else { "basic" };

        let tags = match o.get("tags") {
            Some(Value::Array(a)) => a.iter().map(|t| t.as_str().map(String::from).unwrap_or_else(|| t.to_string())).collect::<Vec<_>>().join(" "),
            Some(Value::String(s)) => s.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => String::new(),
        };
        let importance = match o.get("importance") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let word = v.as_str().map(|s| s.trim().to_lowercase()).unwrap_or_else(|| v.to_string());
                let level = match word.as_str() {
                    "핵심" | "core" => Some(2),
                    "일반" | "normal" => Some(1),
                    "보충" | "extra" => Some(0),
                    _ => None,
                };
                if level.is_none() {
                    issues.push(at(i, "warning", "bad_importance", format!("importance 값 '{word}'를 알 수 없어 태그로 추론했어요")));
                }
                level
            }
        };
        match seen.get(&front) {
            Some(first) => issues.push(at(i, "warning", "duplicate", format!("front 가 #{first} 와 같아요 · 둘 다 가져왔어요"))),
            None => {
                seen.insert(front.clone(), i);
            }
        }
        let deck = o.get("deck").and_then(Value::as_str).map(String::from).or(default_deck.clone()).unwrap_or_else(|| DEFAULT_DECK.into());
        let deck = match split_path(&deck) {
            p if p.is_empty() => vec![DEFAULT_DECK.to_string()],
            p => p,
        };
        out.push(JsonCard { index: i, front, back, deck, tags, importance, kind });
    }
    (out, issues, None)
}

/// 대상 덱이 있으면 경로를 그 아래로 옮긴다. 경로 첫 단계가 대상 덱 이름이면 겹치지 않게 뗀다
fn relative_path(path: &[String], target_name: Option<&str>) -> Vec<String> {
    match target_name {
        Some(t) if path.first().is_some_and(|p| p == t) => path[1..].to_vec(),
        Some(_) => path.to_vec(),
        None => path.to_vec(),
    }
}

#[derive(Serialize)]
pub struct JsonPreview {
    ok: usize,
    skipped: usize,
    warnings: usize,
    issues: Vec<Issue>,
    /// 카드가 들어갈 최상위 덱 이름
    roots: Vec<String>,
    /// 새로 만들 하위 덱 수 (최상위 덱은 빼고)
    new_decks: usize,
    error: Option<String>,
}

#[tauri::command]
pub fn preview_json(state: State<AppState>, text: String, deck_id: Option<i64>) -> Res<JsonPreview> {
    let (cards, issues, error) = parse_json(&text);
    let con = state.db.lock().map_err(err)?;
    let target: Option<(i64, String)> = match deck_id {
        Some(id) => Some(con.query_row("SELECT id, name FROM deck WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?),
        None => None,
    };
    // 만들어질 덱을 센다. 읽기만 한다
    let mut new_paths: HashSet<Vec<String>> = HashSet::new();
    let mut roots: Vec<String> = vec![];
    for c in &cards {
        let path = relative_path(&c.deck, target.as_ref().map(|t| t.1.as_str()));
        let root_name = target.as_ref().map(|t| t.1.clone()).or_else(|| path.first().cloned()).unwrap_or_default();
        if !roots.contains(&root_name) {
            roots.push(root_name);
        }
        let mut parent = target.as_ref().map(|t| t.0);
        let mut missing = false;
        for d in 1..=path.len() {
            if !missing {
                let found: Option<i64> = con
                    .query_row("SELECT id FROM deck WHERE parent_id IS ?1 AND name = ?2", params![parent, path[d - 1]], |r| r.get(0))
                    .optional()
                    .map_err(err)?;
                match found {
                    Some(id) => parent = Some(id),
                    None => missing = true,
                }
            }
            // 최상위 덱은 세지 않는다. "정보처리기사 · 하위 덱 3개 새로" (대상 덱이 있으면 전부 하위 덱이다)
            if missing && (target.is_some() || d >= 2) {
                new_paths.insert(path[..d].to_vec());
            }
        }
    }
    let skipped = issues.iter().filter(|i| i.severity == "skipped").count();
    Ok(JsonPreview {
        ok: cards.len(),
        skipped,
        warnings: issues.len() - skipped,
        issues,
        roots,
        new_decks: new_paths.len(),
        error,
    })
}

#[tauri::command]
pub fn import_json(state: State<AppState>, text: String, source: String, deck_id: Option<i64>) -> Res<ImportResult> {
    let (cards, issues, error) = parse_json(&text);
    if let Some(e) = error {
        return Err(e);
    }
    if cards.is_empty() {
        return Err("가져올 수 있는 카드가 없어요.".into());
    }
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let now = now_ms();
    tx.execute("INSERT INTO import_batch (source_kind, source_name, started_at) VALUES ('json', ?1, ?2)", params![source, now])
        .map_err(err)?;
    let batch = tx.last_insert_rowid();
    let target: Option<(i64, String)> = match deck_id {
        Some(id) => Some(tx.query_row("SELECT id, name FROM deck WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?),
        None => None,
    };

    let mut decks = Decks::default();
    let mut used: HashSet<i64> = HashSet::new();
    let mut card_tags: HashMap<i64, Vec<Option<(i64, String)>>> = HashMap::new();
    let mut made = 0;
    for c in &cards {
        let path = relative_path(&c.deck, target.as_ref().map(|t| t.1.as_str()));
        let did = match (&target, path.is_empty()) {
            (Some((id, _)), true) => *id,
            _ => decks.ensure(&tx, target.as_ref().map(|t| t.0), &path, now)?,
        };
        used.insert(did);
        let card = NewCard {
            deck_id: did,
            kind: c.kind.into(),
            front: c.front.clone(),
            back: c.back.clone(),
            importance: c.importance,
        };
        let n = insert_note(&tx, &card, &c.tags, Some(batch)).map_err(|e| format!("#{}: {e}", c.index))?;
        made += n;
        for _ in 0..n {
            card_tags.entry(did).or_default().push(tags_level(&c.tags));
        }
    }
    suggest_from_tags(&tx, &card_tags)?;
    crate::exam::fill_from_tags(&tx).map_err(err)?;
    crate::exam::suggest_all(&tx)?;

    let roots = roots_of(&tx, &used)?;
    let skipped = issues.iter().filter(|i| i.severity == "skipped").count();
    let result = ImportResult {
        source,
        kind: "json".into(),
        decks: roots.len(),
        subdecks: decks.created.iter().filter(|id| !roots.contains(id)).count(),
        notes: cards.len(),
        cards: made,
        skipped,
        root_deck_id: target.as_ref().map(|t| t.0).or_else(|| (roots.len() == 1).then(|| *roots.iter().next().unwrap())),
        roots: match &target {
            Some(t) => vec![t.0],
            None => roots.iter().copied().collect(),
        },
        issues,
        ..Default::default()
    };
    finish_batch(&tx, batch, &result)?;
    tx.commit().map_err(err)?;
    Ok(result)
}

// ------------------------------------------------------------------ protobuf (Anki 설정 블롭에서 문자열 몇 개만 읽는다)

enum Pb<'a> {
    Bytes(&'a [u8]),
    Varint(u64),
    Other,
}

fn varint(buf: &[u8], i: &mut usize) -> u64 {
    let (mut v, mut shift) = (0u64, 0);
    while *i < buf.len() {
        let b = buf[*i];
        *i += 1;
        if shift < 64 {
            v |= u64::from(b & 0x7f) << shift;
        }
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
    }
    v
}

/// 최상위 필드 -> [(번호, 값)]. 알 수 없는 와이어 타입이 나오면 멈춘다
fn pb_fields(buf: &[u8]) -> Vec<(u64, Pb<'_>)> {
    let mut out = vec![];
    let mut i = 0;
    while i < buf.len() {
        let key = varint(buf, &mut i);
        let (num, wt) = (key >> 3, key & 7);
        match wt {
            0 => {
                let v = varint(buf, &mut i);
                out.push((num, Pb::Varint(v)));
            }
            2 => {
                let len = varint(buf, &mut i) as usize;
                let end = (i + len).min(buf.len());
                out.push((num, Pb::Bytes(&buf[i..end])));
                i = end;
            }
            5 => {
                i += 4;
                out.push((num, Pb::Other));
            }
            1 => {
                i += 8;
                out.push((num, Pb::Other));
            }
            _ => break,
        }
    }
    out
}

fn pb_str(buf: &[u8], field: u64) -> String {
    pb_fields(buf)
        .into_iter()
        .find_map(|(n, v)| match v {
            Pb::Bytes(b) if n == field => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ Anki 패키지

struct ADeck {
    path: Vec<String>,
    filtered: bool,
}

struct AType {
    name: String,
    fields: Vec<String>,
    css: String,
    templates: Vec<(u32, String, String, String)>, // ord, 이름, 앞면, 뒷면
    /// 원본과 다르게 보일 수 있는 기능 (스크립트, 빈칸 type-in, TTS)
    degraded: Vec<&'static str>,
    /// Anki originalStockKind (0 = 사용자 정의). 내보낼 때 다시 붙인다 (I78)
    stock: i64,
}

struct Package {
    col: Connection,
    tmp: PathBuf,
    zip: zip::ZipArchive<std::fs::File>,
    decks: HashMap<i64, ADeck>,
    types: HashMap<i64, AType>,
}

impl Drop for Package {
    fn drop(&mut self) {
        // Windows 는 열린 파일을 지우지 못한다. 연결을 먼저 닫는다
        if let Ok(mem) = Connection::open_in_memory() {
            drop(std::mem::replace(&mut self.col, mem));
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.tmp.display()));
        }
    }
}

fn unzstd(raw: Vec<u8>, limit: usize) -> Res<Vec<u8>> {
    if raw.starts_with(&ZSTD_MAGIC) {
        let mut out = vec![];
        zstd::stream::read::Decoder::new(&raw[..]).map_err(err)?.take(limit as u64).read_to_end(&mut out).map_err(err)?;
        Ok(out)
    } else {
        Ok(raw)
    }
}

fn read_entry(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Res<Vec<u8>> {
    let mut f = zip.by_name(name).map_err(err)?;
    let mut buf = vec![];
    f.read_to_end(&mut buf).map_err(err)?;
    Ok(buf)
}

fn degraded_features(body: &str) -> Vec<&'static str> {
    let lower = body.to_lowercase();
    let mut out = vec![];
    // Anki 그림 가리기(IO) 템플릿의 스크립트는 우리가 직접 그리므로 원본과 다르게 보이지 않는다 (occlusion.rs)
    if lower.contains("<script") && !crate::occlusion::is_template(body) {
        out.push("스크립트");
    }
    // type-in 은 J 서술형이 입력칸 · 정답 · 비교로 받는다 (I69). 빈칸 type-in 만 정답이 그 빈칸이 아니라 본문 전체로 잡힌다
    if body.contains("{{type:cloze:") {
        out.push("빈칸 입력칸");
    }
    if body.contains("{{tts") {
        out.push("읽어주기(TTS)");
    }
    out
}

fn open_package(path: &Path, work: &Path) -> Res<Package> {
    let file = std::fs::File::open(path).map_err(|e| format!("파일을 열 수 없어요: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "Anki 패키지(zip) 형식이 아니에요.".to_string())?;
    let names: HashSet<String> = zip.file_names().filter_map(Result::ok).map(|n| n.into_owned()).collect();
    // anki21b 가 우선이다. collection.anki2 는 구버전 경고용 스텁일 수 있다
    let entry = ["collection.anki21b", "collection.anki21", "collection.anki2"]
        .into_iter()
        .find(|e| names.contains(*e))
        .ok_or("패키지 안에 컬렉션이 없어요.")?;
    let raw = unzstd(read_entry(&mut zip, entry)?, 4 << 30)?;
    if !raw.starts_with(b"SQLite format 3") {
        return Err("컬렉션이 SQLite 파일이 아니에요.".into());
    }
    std::fs::create_dir_all(work).map_err(err)?;
    let tmp = work.join(format!("import-{}.sqlite", std::process::id()));
    std::fs::write(&tmp, &raw).map_err(err)?;
    let col = Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    // Anki 가 실행 중에 등록하는 콜레이션이라 파일에 정의가 없다. 읽기만 하므로 대소문자 무시 비교면 된다
    col.create_collation("unicase", |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase())).map_err(err)?;

    let modern: bool = col
        .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'notetypes'", [], |r| r.get::<_, i64>(0))
        .map_err(err)?
        > 0;
    let (decks, types) = if modern { read_modern(&col)? } else { read_legacy(&col)? };
    Ok(Package { col, tmp, zip, decks, types })
}

/// 스키마 18 (Anki 2.1.50+). 덱 이름은 \x1f 로 계층을 나눈다
fn read_modern(col: &Connection) -> Res<(HashMap<i64, ADeck>, HashMap<i64, AType>)> {
    let mut decks = HashMap::new();
    let mut st = col.prepare("SELECT id, name, kind FROM decks").map_err(err)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<Vec<u8>>>(2)?)))
        .map_err(err)?;
    for row in rows {
        let (id, name, kind) = row.map_err(err)?;
        // kind: 1 = 일반 덱, 2 = 필터 덱
        let filtered = kind.as_deref().is_some_and(|k| pb_fields(k).first().is_some_and(|(n, _)| *n == 2));
        decks.insert(id, ADeck { path: name.split(SEP).map(String::from).collect(), filtered });
    }

    let mut types = HashMap::new();
    let mut st = col.prepare("SELECT id, name, config FROM notetypes").map_err(err)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<Vec<u8>>>(2)?)))
        .map_err(err)?;
    for row in rows {
        let (id, name, cfg) = row.map_err(err)?;
        let cfg = cfg.unwrap_or_default();
        // NotetypeConfig.original_stock_kind = 9 (proto/anki/notetypes.proto)
        let stock = pb_fields(&cfg).into_iter().find_map(|(n, v)| match v {
            Pb::Varint(k) if n == 9 => Some(k as i64),
            _ => None,
        });
        let fields: Vec<String> = col
            .prepare("SELECT name FROM fields WHERE ntid = ?1 ORDER BY ord")
            .and_then(|mut s| s.query_map([id], |r| r.get(0))?.collect())
            .map_err(err)?;
        let raw: Vec<(u32, String, Option<Vec<u8>>)> = col
            .prepare("SELECT ord, name, config FROM templates WHERE ntid = ?1 ORDER BY ord")
            .and_then(|mut s| s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
            .map_err(err)?;
        let templates: Vec<(u32, String, String, String)> = raw
            .into_iter()
            .map(|(ord, n, c)| {
                let c = c.unwrap_or_default();
                (ord, n, pb_str(&c, 1), pb_str(&c, 2))
            })
            .collect();
        let mut degraded = vec![];
        for t in &templates {
            for f in degraded_features(&format!("{}\n{}", t.2, t.3)) {
                if !degraded.contains(&f) {
                    degraded.push(f);
                }
            }
        }
        types.insert(id, AType { name, fields, css: pb_str(&cfg, 3), templates, degraded, stock: stock.unwrap_or(0) });
    }
    Ok((decks, types))
}

/// 스키마 11 (구버전 .apkg). 덱과 노트타입이 col 테이블의 JSON 안에 있다
fn read_legacy(col: &Connection) -> Res<(HashMap<i64, ADeck>, HashMap<i64, AType>)> {
    let (decks_json, models_json): (String, String) =
        col.query_row("SELECT decks, models FROM col", [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
    let mut decks = HashMap::new();
    if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&decks_json) {
        for (id, d) in m {
            let Ok(id) = id.parse::<i64>() else { continue };
            let name = d.get("name").and_then(Value::as_str).unwrap_or_default();
            let filtered = d.get("dyn").and_then(Value::as_i64).unwrap_or(0) == 1;
            decks.insert(id, ADeck { path: name.split("::").map(String::from).collect(), filtered });
        }
    }
    let mut types = HashMap::new();
    if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&models_json) {
        for (id, t) in m {
            let Ok(id) = id.parse::<i64>() else { continue };
            let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let fields = t.get("flds").and_then(Value::as_array).map(|a| a.iter().map(|f| s(f, "name")).collect()).unwrap_or_default();
            let templates: Vec<(u32, String, String, String)> = t
                .get("tmpls")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .enumerate()
                        .map(|(i, x)| (x.get("ord").and_then(Value::as_u64).unwrap_or(i as u64) as u32, s(x, "name"), s(x, "qfmt"), s(x, "afmt")))
                        .collect()
                })
                .unwrap_or_default();
            let mut degraded = vec![];
            for tp in &templates {
                for f in degraded_features(&format!("{}\n{}", tp.2, tp.3)) {
                    if !degraded.contains(&f) {
                        degraded.push(f);
                    }
                }
            }
            let stock = t.get("originalStockKind").and_then(Value::as_i64).unwrap_or(0);
            types.insert(id, AType { name: s(&t, "name"), fields, css: s(&t, "css"), templates, degraded, stock });
        }
    }
    Ok((decks, types))
}

/// 미디어 목록. N번째 이름이 zip 안의 "N" 항목이다
fn media_names(pkg: &mut Package) -> Res<Vec<(String, String)>> {
    let names: HashSet<String> = pkg.zip.file_names().filter_map(Result::ok).map(|n| n.into_owned()).collect();
    if !names.contains("media") {
        return Ok(vec![]);
    }
    let raw = unzstd(read_entry(&mut pkg.zip, "media")?, 64 << 20)?;
    let mut out = vec![];
    if raw.first() == Some(&b'{') {
        // 구버전: {"0": "a.png", ...}
        if let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(&raw) {
            for (k, v) in m {
                out.push((k, v.as_str().unwrap_or_default().to_string()));
            }
        }
    } else {
        // 신버전: repeated field 1 { 1: 파일 이름, 2: 크기, 3: sha1 }
        for (i, (num, v)) in pb_fields(&raw).into_iter().enumerate() {
            if let (1, Pb::Bytes(b)) = (num, v) {
                out.push((i.to_string(), pb_str(b, 1)));
            }
        }
    }
    out.retain(|(k, _)| names.contains(k));
    Ok(out)
}

/// 카드의 실제 덱. 필터 덱에 올라간 카드는 odid 가 원래 덱이다
fn real_deck(did: i64, odid: i64) -> i64 {
    if odid != 0 { odid } else { did }
}

#[derive(Serialize)]
pub struct Degraded {
    feature: String,
    note_types: Vec<String>,
    cards: usize,
}

#[derive(Serialize)]
pub struct AnkiPreview {
    file_name: String,
    bytes: u64,
    decks_top: usize,
    decks_sub: usize,
    notes: usize,
    cards: usize,
    media: usize,
    note_types: usize,
    note_types_ok: usize,
    degraded: Vec<Degraded>,
    filtered_decks: usize,
    filtered_cards: usize,
    reviews: usize,
    /// 복습 기록을 옮기면 오늘 기준 밀려 있을 복습 수 (B2-r "하루 200장씩")
    overdue: usize,
    /// [latex] 태그를 쓰는 카드 (B2 "LaTeX 태그 N장"). 수식만 들었으면 KaTeX 로 그린다
    latex_cards: usize,
    /// 그중 LaTeX 문서 전체(\documentclass 등)라 원문으로 보이는 카드 ("일부는 원문으로 보여요")
    latex_raw_cards: usize,
    /// B2-s 덱 고르기. 카드가 있는 실제 덱마다 그 덱 "자기" 카드의 합계. 화면이 고른 덱만 더해 사전 검사를 다시 계산한다
    deck_tree: Vec<DeckStat>,
    /// 원본과 다르게 보일 수 있는 노트타입 (B2-s "고른 덱이 쓰는 N종 모두 지원")
    degraded_types: Vec<i64>,
    /// 지운 카드의 기록. 덱에 속하지 않아 덱을 전부 골랐을 때만 함께 옮긴다
    orphan_reviews: usize,
}

#[derive(Serialize, Default)]
pub struct DeckStat {
    /// Anki 덱 id. import_anki 의 deck_ids 로 돌려준다
    id: i64,
    path: Vec<String>,
    /// 파일에 있는 카드 · 노트 전부 (트리의 장수)
    cards: usize,
    notes: usize,
    /// 이미 앱에 있는 노트 (같은 guid). 가져와도 건너뛴다
    existing_notes: usize,
    /// 아래는 실제로 들어갈 것만: 이미 있는 노트의 카드는 빼고 센다 (사전 검사 · 버튼 문구)
    new_cards: usize,
    reviews: usize,
    overdue: usize,
    latex_cards: usize,
    latex_raw_cards: usize,
    /// 쓰는 노트타입 (Anki 모델 id)
    types: Vec<i64>,
    degraded: Vec<Degraded>,
}

/// 우리 덱 경로로 옮길 때의 이름 (앞뒤 공백을 떼고 빈 이름은 뺀다). 가져오기와 덱 고르기 트리가 같은 경로를 쓴다
fn deck_parts(d: &ADeck) -> Vec<String> {
    let parts: Vec<String> = d.path.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() { vec![DEFAULT_DECK.into()] } else { parts }
}

/// 카드가 실제로 들어갈 Anki 덱. 필터 덱에 올라간 카드는 원래 덱(odid)이고, 필터 덱 자신은 None
fn target_deck(pkg: &Package, did: i64, odid: i64) -> Option<&ADeck> {
    pkg.decks.get(&real_deck(did, odid)).or_else(|| pkg.decks.get(&did)).filter(|d| !d.filtered)
}

/// B2-s. 실제 덱마다 자기 카드 · 노트 · 기록 · 밀린 복습 · 수식 · 템플릿 기능 수.
/// existing: 이미 앱에 있는 노트 guid. overdue_before: 이 날(컬렉션 생성일 기준 일수)보다 기한이 앞선 복습 카드는 밀린 것
fn deck_stats(pkg: &Package, existing: &HashSet<String>, overdue_before: i64) -> Res<Vec<DeckStat>> {
    let revs: HashMap<i64, usize> = {
        let mut st = pkg.col.prepare("SELECT cid, count(*) FROM revlog GROUP BY cid").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as usize))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let mut st = pkg
        .col
        .prepare(
            "SELECT c.id, c.nid, c.did, c.odid, c.type, c.queue, c.due, n.mid, n.guid, n.flds LIKE '%[latex]%',
                    n.flds LIKE '%\\documentclass%' OR n.flds LIKE '%\\begin{document}%' OR n.flds LIKE '%\\usepackage%'
             FROM cards c JOIN notes n ON n.id = c.nid",
        )
        .map_err(err)?;
    let mut rows = st.query([]).map_err(err)?;
    let mut by: std::collections::BTreeMap<i64, (DeckStat, HashSet<i64>)> = Default::default();
    while let Some(r) = rows.next().map_err(err)? {
        let (cid, nid, did, odid): (i64, i64, i64, i64) = (r.get(0).map_err(err)?, r.get(1).map_err(err)?, r.get(2).map_err(err)?, r.get(3).map_err(err)?);
        let Some(d) = target_deck(pkg, did, odid) else { continue };
        let (s, notes) = by
            .entry(real_deck(did, odid))
            .or_insert_with(|| (DeckStat { id: real_deck(did, odid), path: deck_parts(d), ..Default::default() }, HashSet::new()));
        s.cards += 1;
        let old = existing.contains(&r.get::<_, String>(8).map_err(err)?);
        if notes.insert(nid) {
            s.notes += 1;
            s.existing_notes += old as usize;
        }
        if old {
            continue; // 이미 있는 노트는 다시 넣지 않으니 기록 · 밀린 복습 · 템플릿 검사에서 뺀다
        }
        s.new_cards += 1;
        s.reviews += revs.get(&cid).copied().unwrap_or(0);
        let (ctype, queue, due): (i64, i64, i64) = (r.get(4).map_err(err)?, r.get(5).map_err(err)?, r.get(6).map_err(err)?);
        s.overdue += (ctype == 2 && queue != -1 && due < overdue_before) as usize;
        if r.get::<_, bool>(9).map_err(err)? {
            s.latex_cards += 1;
            s.latex_raw_cards += r.get::<_, bool>(10).map_err(err)? as usize;
        }
        let mid: i64 = r.get(7).map_err(err)?;
        if !s.types.contains(&mid) {
            s.types.push(mid);
        }
        for (f, t) in pkg.types.get(&mid).into_iter().flat_map(|t| t.degraded.iter().map(move |f| (f, t))) {
            match s.degraded.iter_mut().find(|x| x.feature == *f) {
                Some(x) => {
                    x.cards += 1;
                    if !x.note_types.contains(&t.name) {
                        x.note_types.push(t.name.clone());
                    }
                }
                None => s.degraded.push(Degraded { feature: f.to_string(), note_types: vec![t.name.clone()], cards: 1 }),
            }
        }
    }
    Ok(by.into_values().map(|(s, _)| s).collect())
}

/// 카드가 실제로 있는 덱 경로들 (필터 덱 제외)
fn card_deck_paths(pkg: &Package) -> Res<BTreeSet<Vec<String>>> {
    let mut st = pkg.col.prepare("SELECT DISTINCT did, odid FROM cards").map_err(err)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(err)?;
    let mut out = BTreeSet::new();
    for row in rows {
        let (did, odid) = row.map_err(err)?;
        if let Some(d) = pkg.decks.get(&real_deck(did, odid)) {
            out.insert(d.path.clone());
        }
    }
    Ok(out)
}

fn count(col: &Connection, sql: &str) -> Res<usize> {
    col.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n as usize).map_err(err)
}

/// 노트타입별 카드 수
fn cards_by_type(col: &Connection) -> Res<HashMap<i64, usize>> {
    let mut st = col.prepare("SELECT n.mid, count(*) FROM cards c JOIN notes n ON n.id = c.nid GROUP BY n.mid").map_err(err)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as usize))).map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

fn degraded_summary(pkg: &Package, by_type: &HashMap<i64, usize>) -> Vec<Degraded> {
    let mut out: Vec<Degraded> = vec![];
    for (id, t) in &pkg.types {
        let cards = by_type.get(id).copied().unwrap_or(0);
        if cards == 0 {
            continue;
        }
        for f in &t.degraded {
            match out.iter_mut().find(|d| d.feature == *f) {
                Some(d) => {
                    d.note_types.push(t.name.clone());
                    d.cards += cards;
                }
                None => out.push(Degraded { feature: f.to_string(), note_types: vec![t.name.clone()], cards }),
            }
        }
    }
    out
}

#[tauri::command]
pub async fn inspect_anki(state: State<'_, AppState>, path: String) -> Res<AnkiPreview> {
    let p = Path::new(&path);
    let mut pkg = open_package(p, &state.data_dir)?;
    let media = media_names(&mut pkg)?.len();
    let paths = card_deck_paths(&pkg)?;
    let tops: BTreeSet<&String> = paths.iter().filter_map(|p| p.first()).collect();
    let prefixes: BTreeSet<&[String]> = paths.iter().flat_map(|p| (2..=p.len()).map(move |d| &p[..d])).collect();
    let by_type = cards_by_type(&pkg.col)?;
    let used_types: Vec<&i64> = pkg.types.keys().filter(|id| by_type.contains_key(id)).collect();
    // 밀린 복습: 기한(컬렉션 생성일부터의 일수)이 오늘보다 앞선 복습 카드
    let crt_day = db::study_day(pkg.col.query_row("SELECT crt FROM col", [], |r| r.get::<_, i64>(0)).map_err(err)? * 1000);
    let overdue_before = db::study_day(now_ms()) - crt_day;
    let existing: HashSet<String> = {
        let con = state.db.lock().map_err(err)?;
        let mut st = con.prepare("SELECT anki_guid FROM note WHERE anki_guid IS NOT NULL").map_err(err)?;
        let rows = st.query_map([], |r| r.get(0)).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let deck_tree = deck_stats(&pkg, &existing, overdue_before)?;
    Ok(AnkiPreview {
        file_name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
        decks_top: tops.len(),
        decks_sub: prefixes.len(),
        notes: count(&pkg.col, "SELECT count(*) FROM notes")?,
        cards: count(&pkg.col, "SELECT count(*) FROM cards")?,
        media,
        note_types: used_types.len(),
        note_types_ok: used_types.iter().filter(|id| pkg.types[*id].degraded.is_empty()).count(),
        degraded: degraded_summary(&pkg, &by_type),
        filtered_decks: pkg.decks.values().filter(|d| d.filtered).count(),
        filtered_cards: count(&pkg.col, "SELECT count(*) FROM cards WHERE odid <> 0")?,
        reviews: count(&pkg.col, "SELECT count(*) FROM revlog")?,
        latex_cards: count(&pkg.col, "SELECT count(*) FROM cards c JOIN notes n ON n.id = c.nid WHERE n.flds LIKE '%[latex]%'")?,
        latex_raw_cards: count(
            &pkg.col,
            "SELECT count(*) FROM cards c JOIN notes n ON n.id = c.nid
             WHERE n.flds LIKE '%[latex]%' AND (n.flds LIKE '%\\documentclass%' OR n.flds LIKE '%\\begin{document}%' OR n.flds LIKE '%\\usepackage%')",
        )?,
        overdue: pkg
            .col
            .query_row("SELECT count(*) FROM cards WHERE type = 2 AND queue <> -1 AND due < ?1", [overdue_before], |r| r.get::<_, i64>(0))
            .map_err(err)? as usize,
        deck_tree,
        degraded_types: pkg.types.iter().filter(|(_, t)| !t.degraded.is_empty()).map(|(id, _)| *id).collect(),
        orphan_reviews: count(&pkg.col, "SELECT count(*) FROM revlog WHERE cid NOT IN (SELECT id FROM cards)")?,
    })
}

/// B2-p 진행 표시. 단계: unzip → decks → cards → media → done
#[derive(Serialize, Clone)]
struct Progress {
    step: &'static str,
    done: usize,
    total: usize,
}

const CANCELLED: &str = "가져오기를 멈췄어요. 기존 덱과 카드는 그대로예요.";

/// B2-p "취소". 진행 중인 가져오기를 멈춘다. 트랜잭션이 되돌려지고 새로 쓴 미디어 파일도 지운다
#[tauri::command]
pub fn cancel_import(state: State<AppState>) {
    state.import_cancel.store(true, Ordering::Relaxed);
}

/// deck_ids: B2-s 에서 고른 Anki 덱 (inspect_anki 의 deck_tree id). 없으면 전부
#[tauri::command]
pub async fn import_anki(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    with_history: Option<bool>,
    deck_ids: Option<Vec<i64>>,
) -> Res<ImportResult> {
    state.import_cancel.store(false, Ordering::Relaxed);
    let emit = |step: &'static str, done: usize, total: usize| {
        let _ = app.emit("import-progress", Progress { step, done, total });
    };
    let mut written = vec![];
    let selected: Option<HashSet<i64>> = deck_ids.map(|v| v.into_iter().collect());
    let result = run_import_selected(&emit, &state, &path, with_history.unwrap_or(false), selected.as_ref(), &mut written);
    if result.is_err() {
        // 실패하거나 멈추면 기존 데이터에 영향이 없어야 한다 (MVP 6.10). DB 는 트랜잭션이 되돌린다
        for f in written {
            let _ = std::fs::remove_file(f);
        }
    }
    result
}

/// emit: 진행 표시 (단계, 끝난 수, 전체). AppHandle 없이 부를 수 있게 밖에서 받는다 (export.rs 왕복 테스트)
#[cfg(test)]
pub(crate) fn run_import_anki(
    emit: &dyn Fn(&'static str, usize, usize),
    state: &AppState,
    path: &str,
    with_history: bool,
    written: &mut Vec<PathBuf>,
) -> Res<ImportResult> {
    run_import_selected(emit, state, path, with_history, None, written)
}

/// 고른 덱의 카드 (필터 덱에 있던 카드는 원래 덱 기준). 그 카드를 가진 노트만, 그 노트가 쓰는 미디어 · 노트타입 · 기록만 들어간다
struct Chosen {
    cards: HashSet<i64>,
    notes: HashSet<i64>,
}

fn choose(pkg: &Package, selected: &HashSet<i64>) -> Res<Chosen> {
    let mut st = pkg.col.prepare("SELECT id, nid, did, odid FROM cards").map_err(err)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))).map_err(err)?;
    let mut out = Chosen { cards: HashSet::new(), notes: HashSet::new() };
    for row in rows {
        let (id, nid, did, odid) = row.map_err(err)?;
        if selected.contains(&real_deck(did, odid)) {
            out.cards.insert(id);
            out.notes.insert(nid);
        }
    }
    Ok(out)
}

fn run_import_selected(
    emit: &dyn Fn(&'static str, usize, usize),
    state: &AppState,
    path: &str,
    with_history: bool,
    selected: Option<&HashSet<i64>>,
    written: &mut Vec<PathBuf>,
) -> Res<ImportResult> {
    let cancelled = || state.import_cancel.load(Ordering::Relaxed);

    emit("unzip", 0, 1);
    let p = Path::new(path);
    let mut pkg = open_package(p, &state.data_dir)?;
    emit("unzip", 1, 1);
    let source = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = if source.to_lowercase().ends_with(".colpkg") { "colpkg" } else { "apkg" };
    let now = now_ms();
    let mut result = ImportResult { source: source.clone(), kind: kind.into(), history: with_history, ..Default::default() };

    // B2-s 고른 덱만. None 이면 전부 (지금까지와 같다)
    let chosen = selected.map(|s| choose(&pkg, s)).transpose()?;
    let picked_card = |id: i64| chosen.as_ref().is_none_or(|c| c.cards.contains(&id));
    let picked_note = |id: i64| chosen.as_ref().is_none_or(|c| c.notes.contains(&id));
    let by_type = match &chosen {
        None => cards_by_type(&pkg.col)?,
        Some(c) => {
            let mut st = pkg.col.prepare("SELECT n.mid, c.id FROM cards c JOIN notes n ON n.id = c.nid").map_err(err)?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(err)?;
            let mut out: HashMap<i64, usize> = HashMap::new();
            for row in rows {
                let (mid, id) = row.map_err(err)?;
                if c.cards.contains(&id) {
                    *out.entry(mid).or_default() += 1;
                }
            }
            out
        }
    };
    let degraded = degraded_summary(&pkg, &by_type);
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    tx.execute("INSERT INTO import_batch (source_kind, source_name, started_at) VALUES (?1, ?2, ?3)", params![kind, source, now])
        .map_err(err)?;
    let batch = tx.last_insert_rowid();

    // 덱. 카드가 실제로 있는 덱만 만든다 (필터 덱의 카드는 원래 덱으로)
    let mut decks = Decks::default();
    let mut deck_cache: HashMap<i64, i64> = HashMap::new();
    {
        let pairs: Vec<(i64, i64)> = {
            let mut st = pkg.col.prepare("SELECT DISTINCT did, odid FROM cards").map_err(err)?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
            rows.collect::<Result<_, _>>().map_err(err)?
        };
        for (did, odid) in pairs {
            let real = real_deck(did, odid);
            if deck_cache.contains_key(&real) || !selected.is_none_or(|s| s.contains(&real)) {
                continue;
            }
            let Some(d) = target_deck(&pkg, did, odid) else { continue };
            let id = decks.ensure(&tx, None, &deck_parts(d), now)?;
            tx.execute("UPDATE deck SET anki_deck_id = COALESCE(anki_deck_id, ?2) WHERE id = ?1", params![id, real]).map_err(err)?;
            deck_cache.insert(real, id);
        }
        emit("decks", decks.cache.len(), decks.cache.len());
    }

    // 노트타입. 같은 Anki 노트타입을 다시 가져오면 재사용한다
    let mut nt_map: HashMap<i64, i64> = HashMap::new();
    for (aid, t) in &pkg.types {
        if !by_type.contains_key(aid) {
            continue;
        }
        let existing: Option<i64> = tx.query_row("SELECT id FROM note_type WHERE anki_model_id = ?1", [aid], |r| r.get(0)).optional().map_err(err)?;
        let id = match existing {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO note_type (name, fields, css, created_at, updated_at, is_supported, unsupported_note, anki_model_id, anki_stock_kind)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        t.name,
                        serde_json::to_string(&t.fields).map_err(err)?,
                        t.css,
                        now,
                        t.degraded.is_empty() as i64,
                        (!t.degraded.is_empty()).then(|| t.degraded.join(", ")),
                        aid,
                        (t.stock > 0).then_some(t.stock)
                    ],
                )
                .map_err(err)?;
                let id = tx.last_insert_rowid();
                for (ord, name, q, a) in &t.templates {
                    tx.execute(
                        "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![id, ord, name, q, a],
                    )
                    .map_err(err)?;
                }
                id
            }
        };
        nt_map.insert(*aid, id);
    }

    // 노트와 카드. 진행 표시는 둘을 합쳐 "카드" 한 단계로 보여준다
    let total = match &chosen {
        Some(c) => c.notes.len() + c.cards.len(),
        None => count(&pkg.col, "SELECT count(*) FROM notes")? + count(&pkg.col, "SELECT count(*) FROM cards")?,
    };
    let mut done = 0;
    let step = |n: &mut usize| -> Res<()> {
        *n += 1;
        if n.is_multiple_of(200) {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            emit("cards", *n, total);
        }
        Ok(())
    };

    // 노트. 같은 guid 가 이미 있으면 다시 넣지 않는다 (재가져오기)
    let mut note_map: HashMap<i64, (i64, String)> = HashMap::new();
    let mut dup = 0;
    let mut refs: HashSet<String> = HashSet::new(); // 고른 노트가 쓰는 미디어 (덱을 골랐을 때만 쓴다)
    {
        let mut st = pkg.col.prepare("SELECT id, guid, mid, mod, tags, flds FROM notes").map_err(err)?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))
            })
            .map_err(err)?;
        for row in rows {
            let (anid, guid, mid, modified, tags, flds) = row.map_err(err)?;
            if !picked_note(anid) {
                continue;
            }
            step(&mut done)?;
            let Some(ntid) = nt_map.get(&mid) else { continue };
            let exists: bool = tx.query_row("SELECT count(*) FROM note WHERE anki_guid = ?1", [&guid], |r| r.get::<_, i64>(0)).map_err(err)? > 0;
            if exists {
                dup += 1;
                continue;
            }
            if chosen.is_some() {
                crate::export::media_refs(&flds, &mut refs); // 새로 넣는 노트가 쓰는 미디어만 (이미 있는 노트의 것은 이미 있다)
            }
            let fields: Vec<&str> = flds.split(SEP).collect();
            tx.execute(
                "INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, anki_note_id, anki_guid, import_batch_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?4, ?6, ?7)",
                params![ntid, serde_json::to_string(&fields).map_err(err)?, tags.trim(), anid, modified * 1000, guid, batch],
            )
            .map_err(err)?;
            note_map.insert(anid, (tx.last_insert_rowid(), tags.trim().to_string()));
        }
    }
    result.notes = note_map.len();
    if dup > 0 {
        result.issues.push(issue("info", "duplicate", format!("이미 가져온 노트 {}개는 다시 넣지 않았어요", n(dup))));
    }

    // 카드. 복습 기록을 옮기지 않으면 모두 새 카드로 시작하고 일시 정지만 지킨다.
    // 옮기면 Anki 의 상태, 간격, 기한, 기억 상태를 그대로 가져온다
    let crt_day = db::study_day(pkg.col.query_row("SELECT crt FROM col", [], |r| r.get::<_, i64>(0)).map_err(err)? * 1000);
    let last_review: HashMap<i64, i64> = if with_history {
        let mut st = pkg.col.prepare("SELECT cid, max(id) FROM revlog WHERE ease BETWEEN 1 AND 4 GROUP BY cid").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    } else {
        HashMap::new()
    };
    let mut used: HashSet<i64> = HashSet::new();
    let mut card_tags: HashMap<i64, Vec<Option<(i64, String)>>> = HashMap::new();
    {
        let mut st = pkg
            .col
            .prepare("SELECT id, nid, did, ord, queue, odid, type, due, ivl, factor, reps, lapses, coalesce(data, '') FROM cards ORDER BY due, id")
            .map_err(err)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    (r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(5)?),
                    AnkiCard {
                        queue: r.get(4)?,
                        ctype: r.get(6)?,
                        due: r.get(7)?,
                        ivl: r.get(8)?,
                        factor: r.get(9)?,
                        reps: r.get(10)?,
                        lapses: r.get(11)?,
                        data: r.get(12)?,
                    },
                ))
            })
            .map_err(err)?;
        for row in rows {
            let ((acid, anid, did, ord, odid), anki) = row.map_err(err)?;
            if !picked_card(acid) {
                continue;
            }
            step(&mut done)?;
            let queue = anki.queue;
            let Some((nid, tags)) = note_map.get(&anid) else { continue };
            let real = real_deck(did, odid);
            if odid != 0 {
                result.filtered_cards += 1;
            }
            let Some(&our) = deck_cache.get(&real) else {
                result.skipped += 1;
                result.issues.push(issue("skipped", "missing_deck", format!("카드 {acid}: 덱을 찾지 못했어요")));
                continue;
            };
            if with_history {
                let s = schedule(&anki, crt_day, &state.fsrs);
                tx.execute(
                    "INSERT OR IGNORE INTO card (note_id, deck_id, template_ord, state, due_at, interval_days, stability, difficulty, srs_step,
                                                 reps, lapses, last_reviewed_at, created_at, updated_at, anki_card_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
                    params![
                        nid,
                        our,
                        ord,
                        s.state,
                        s.due_at,
                        s.interval_days,
                        s.stability,
                        s.difficulty,
                        s.step,
                        anki.reps,
                        anki.lapses,
                        last_review.get(&acid),
                        now,
                        acid
                    ],
                )
                .map_err(err)?;
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO card (note_id, deck_id, template_ord, state, created_at, updated_at, anki_card_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                    params![nid, our, ord, if queue == -1 { "suspended" } else { "new" }, now, acid],
                )
                .map_err(err)?;
            }
            used.insert(our);
            result.cards += 1;
            card_tags.entry(our).or_default().push(tags_level(tags));
        }
    }
    emit("cards", total, total);
    suggest_from_tags(&tx, &card_tags)?;
    crate::exam::fill_from_tags(&tx).map_err(err)?;
    crate::exam::suggest_all(&tx)?;

    // 미디어. 새로 쓴 파일은 written 에 남겨, 실패하거나 멈추면 지운다
    let dir = media_dir(&state.data_dir);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let mut names = media_names(&mut pkg)?;
    if chosen.is_some() {
        // 고른 노트가 쓰는 것과, 들여온 노트타입의 템플릿 · CSS 가 쓰는 "_" 파일(글꼴 등)만
        let type_text: String = nt_map
            .keys()
            .filter_map(|id| pkg.types.get(id))
            .flat_map(|t| std::iter::once(t.css.as_str()).chain(t.templates.iter().flat_map(|x| [x.2.as_str(), x.3.as_str()])))
            .collect();
        names.retain(|(_, n)| refs.contains(n) || (n.starts_with('_') && type_text.contains(n.as_str())));
    }
    for (i, (key, name)) in names.iter().enumerate() {
        if i.is_multiple_of(20) {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            emit("media", i, names.len());
        }
        if safe_media_name(name).is_err() {
            result.issues.push(issue("skipped", "media_name", format!("미디어 '{name}' 는 이름이 올바르지 않아 건너뛰었어요")));
            continue;
        }
        let blob = match read_entry(&mut pkg.zip, key).and_then(|b| unzstd(b, 512 << 20)) {
            Ok(b) => b,
            Err(e) => {
                result.issues.push(issue("warning", "media", format!("미디어 '{name}' 를 읽지 못했어요: {e}")));
                continue;
            }
        };
        let file = dir.join(name);
        if !file.exists() {
            std::fs::write(&file, &blob).map_err(err)?;
            written.push(file);
        }
        tx.execute(
            "INSERT OR IGNORE INTO media (filename, rel_path, sha256, byte_size, mime_type, created_at, anki_media_key) VALUES (?1, ?2, '', ?3, ?4, ?5, ?6)",
            params![name, format!("media/{name}"), blob.len() as i64, mime_of(name), now, key],
        )
        .map_err(err)?;
        result.media += 1;
    }
    emit("media", names.len(), names.len());

    let roots = roots_of(&tx, &used)?;
    result.decks = roots.len();
    let sounds = {
        let mut st = pkg.col.prepare("SELECT id FROM notes WHERE flds LIKE '%[sound:%'").map_err(err)?;
        let ids = st.query_map([], |r| r.get::<_, i64>(0)).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?;
        ids.into_iter().filter(|id| picked_note(*id)).count()
    };
    if sounds > 0 {
        result.issues.push(issue("warning", "sound", format!("소리가 든 노트 {}개는 소리 없이 글자만 보여요", n(sounds))));
    }
    result.subdecks = decks.created.iter().filter(|id| !roots.contains(id)).count();
    result.root_deck_id = (roots.len() == 1).then(|| *roots.iter().next().unwrap());
    result.roots = roots.iter().copied().collect();
    // 기록도 고른 카드 것만 (덱을 고르지 않았으면 지운 카드의 기록까지 전부)
    let cards_of = chosen.as_ref().map(|c| &c.cards);
    result.reviews = if with_history {
        import_revlog(&tx, &pkg.col, cards_of)?
    } else {
        let mut st = pkg.col.prepare("SELECT cid FROM revlog").map_err(err)?;
        let cids = st.query_map([], |r| r.get::<_, i64>(0)).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?;
        cids.into_iter().filter(|cid| cards_of.is_none_or(|c| c.contains(cid))).count()
    };
    result.filtered_decks = pkg.decks.values().filter(|d| d.filtered).count();
    for d in &degraded {
        result.check_cards += d.cards;
        let what = match d.feature.as_str() {
            "빈칸 입력칸" => "써 넣은 답을 그 빈칸이 아니라 본문 전체와 비교해요. 맞게 써도 틀린 것처럼 보일 수 있어요.",
            "스크립트" => "스크립트는 실행하지 않아요. 스크립트로 바뀌는 부분은 보이지 않아요.",
            _ => "소리 없이 글자만 보여요.",
        };
        result.issues.push(issue(
            "warning",
            "degraded_template",
            format!("노트타입 '{}'이 {}을 써요. {what} ({}장)", d.note_types.join("', '"), d.feature, n(d.cards)),
        ));
    }
    if cancelled() {
        return Err(CANCELLED.into());
    }
    finish_batch(&tx, batch, &result)?;
    tx.commit().map_err(err)?;
    emit("done", 1, 1);
    Ok(result)
}

// ------------------------------------------------------------------ 복습 기록 이전 (mvp-scope 7장)

/// Anki cards 테이블의 일정 관련 열
struct AnkiCard {
    queue: i64,
    ctype: i64,
    due: i64,
    ivl: i64,
    factor: i64,
    reps: i64,
    lapses: i64,
    data: String,
}

#[derive(Debug, PartialEq)]
struct Schedule {
    state: &'static str,
    due_at: Option<i64>,
    interval_days: f64,
    stability: Option<f32>,
    difficulty: Option<f32>,
    step: Option<u32>,
}

/// Anki 카드 한 장의 일정 -> 우리 일정.
/// 기억 상태는 Anki 가 FSRS 로 남긴 값(cards.data 의 s, d)을 먼저 쓰고, 없으면 SM-2 간격과 ease 에서 바꾼다.
/// 기한: 복습과 하루 단위 학습(queue 3)은 컬렉션 생성일(crt)부터의 일수, 분 단위 학습(queue 1)은 epoch 초
fn schedule(c: &AnkiCard, crt_day: i64, fsrs: &fsrs::FSRS) -> Schedule {
    let day_start = |day: i64| db::study_day_end_ms(day - 1) + 1;
    let memory = serde_json::from_str::<Value>(&c.data)
        .ok()
        .and_then(|v| Some((v.get("s")?.as_f64()? as f32, v.get("d")?.as_f64()? as f32)))
        .or_else(|| srs::from_sm2(fsrs, c.ivl as f32, c.factor).map(|m| (m.stability, m.difficulty)));
    let (stability, difficulty) = memory.map_or((None, None), |(s, d)| (Some(s), Some(d)));
    let sched = |state, due_at, interval_days: f64, step| Schedule { state, due_at, interval_days, stability, difficulty, step };
    let state = match c.ctype {
        0 => {
            return Schedule { state: if c.queue == -1 { "suspended" } else { "new" }, due_at: None, interval_days: 0.0, stability: None, difficulty: None, step: None };
        }
        2 => sched("review", Some(day_start(crt_day + c.due)), c.ivl.max(1) as f64, None),
        t => {
            // 1 = 학습 중, 3 = 재학습. queue 3 이면 기한이 일수, 아니면 epoch 초
            let due_at = if c.queue == 3 { day_start(crt_day + c.due) } else { c.due * 1000 };
            sched(if t == 3 { "relearning" } else { "learning" }, Some(due_at), 0.0, Some(0))
        }
    };
    if c.queue == -1 { Schedule { state: "suspended", ..state } } else { state }
}

/// Anki 복습 기록을 review_log 로 옮긴다. 같은 기록은 다시 넣지 않는다: 이미 옮긴 기록(anki_revlog_id), 또는 같은 카드 · 같은 시각의 기록
/// (이 앱이 내보낸 .apkg 를 다시 가져오면 revlog id 가 원래 기록의 시각이다, I79).
/// 카드는 anki_card_id 로 찾고, 없으면 노트 guid + 카드 순서로 찾는다 (이미 있어 건너뛴 노트의 카드).
/// 끝내 못 찾은 기록(원본에서 지운 카드)은 card_id 없이 남긴다 (통계는 사실로 남아야 한다, 데이터 모델 A4)
fn import_revlog(tx: &Connection, col: &Connection, only: Option<&HashSet<i64>>) -> Res<usize> {
    let mut card_by_anki: HashMap<i64, i64> = {
        let mut st = tx.prepare("SELECT anki_card_id, id FROM card WHERE anki_card_id IS NOT NULL").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    {
        let mut ours = tx
            .prepare("SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = ?1 AND c.template_ord = ?2")
            .map_err(err)?;
        let mut st = col.prepare("SELECT c.id, c.ord, n.guid FROM cards c JOIN notes n ON n.id = c.nid").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))).map_err(err)?;
        for row in rows {
            let (cid, ord, guid) = row.map_err(err)?;
            if card_by_anki.contains_key(&cid) {
                continue;
            }
            if let Some(id) = ours.query_row(params![guid, ord], |r| r.get(0)).optional().map_err(err)? {
                card_by_anki.insert(cid, id);
            }
        }
    }
    let done: HashSet<i64> = {
        let mut st = tx.prepare("SELECT anki_revlog_id FROM review_log WHERE anki_revlog_id IS NOT NULL").map_err(err)?;
        let rows = st.query_map([], |r| r.get(0)).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let seen: HashSet<(i64, i64)> = {
        let mut st = tx.prepare("SELECT card_id, reviewed_at FROM review_log WHERE card_id IS NOT NULL").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    // 음수 간격은 초 단위다 (학습 단계)
    let days = |x: i64| if x < 0 { -x as f64 / 86_400.0 } else { x as f64 };
    let mut st = col.prepare("SELECT id, cid, ease, ivl, lastIvl, time, type FROM revlog WHERE ease BETWEEN 1 AND 4 ORDER BY id").map_err(err)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?))
        })
        .map_err(err)?;
    let mut n = 0;
    for row in rows {
        let (id, cid, ease, ivl, last_ivl, time, rtype) = row.map_err(err)?;
        let card = card_by_anki.get(&cid).copied();
        if done.contains(&id) || card.is_some_and(|c| seen.contains(&(c, id))) || !only.is_none_or(|c| c.contains(&cid)) {
            continue;
        }
        let prev_state = match rtype {
            0 => "learning",
            2 => "relearning",
            _ => "review",
        };
        tx.execute(
            "INSERT INTO review_log (card_id, reviewed_at, rating, elapsed_ms, prev_interval_days, next_interval_days, prev_state, fed_to_scheduler, anki_revlog_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?2)",
            params![card, id, ease, time, days(last_ivl), days(ivl), prev_state],
        )
        .map_err(err)?;
        n += 1;
    }
    Ok(n)
}

/// B2-j 파일 끌어놓기. 웹뷰 드롭 이벤트는 경로만 주므로 여기서 읽는다. .json 만, 20MB 까지
#[tauri::command]
pub fn read_json_file(path: String) -> Res<String> {
    let p = std::path::Path::new(&path);
    if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
        return Err("JSON 파일(.json)만 넣을 수 있어요.".into());
    }
    if std::fs::metadata(p).map_err(err)?.len() > 20 * 1024 * 1024 {
        return Err("20MB가 넘는 파일은 나눠서 가져와 주세요.".into());
    }
    std::fs::read_to_string(p).map_err(|_| "UTF-8 텍스트로 읽을 수 없는 파일이에요.".into())
}

/// B2-c CSV 끌어놓기. 인코딩(UTF-8 / EUC-KR)은 화면이 고르므로 바이트 그대로 준다. .csv .tsv .txt 만, 20MB 까지
#[tauri::command]
pub fn read_table_file(path: String) -> Res<tauri::ipc::Response> {
    let p = std::path::Path::new(&path);
    if !p.extension().is_some_and(|e| ["csv", "tsv", "txt"].iter().any(|x| e.eq_ignore_ascii_case(x))) {
        return Err("CSV 파일(.csv · .tsv · .txt)만 넣을 수 있어요.".into());
    }
    if std::fs::metadata(p).map_err(err)?.len() > 20 * 1024 * 1024 {
        return Err("20MB가 넘는 파일은 나눠서 가져와 주세요.".into());
    }
    Ok(tauri::ipc::Response::new(std::fs::read(p).map_err(err)?))
}

/// B3 "결과를 파일로 저장". 경로는 사용자가 저장 대화상자에서 고른다
#[tauri::command]
pub fn save_text(path: String, text: String) -> Res<()> {
    std::fs::write(path, text).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_rules() {
        assert_eq!(tags_level("중요도::최우선 과목::1").map(|t| t.0), Some(2));
        assert_eq!(tags_level("중요도::높음").map(|t| t.0), Some(1));
        assert_eq!(tags_level("중요도::보통").map(|t| t.0), Some(0));
        assert_eq!(tags_level("과목::1 unit_question"), None);
        assert_eq!(tags_level("star1 중요도::최우선").map(|t| t.0), Some(2), "가장 높은 것이 이긴다");
        assert_eq!(deck_name_level("3장 요약").map(|t| t.0), Some(2));
        assert_eq!(deck_name_level("부록").map(|t| t.0), Some(0));
        assert_eq!(deck_name_level("4단원"), None);
    }

    #[test]
    fn json_format() {
        let (cs, iss, err) = parse_json(r#"[{"front":"a","back":"b"}]"#);
        assert!(cs.len() == 1 && iss.is_empty() && err.is_none(), "최소 형태가 동작해야 한다");
        assert_eq!(cs[0].deck, vec![DEFAULT_DECK.to_string()]);

        let (cs, _, _) = parse_json(r#"{"deck":"D","cards":[{"front":"a","back":"b","deck":"D::2과목"}]}"#);
        assert_eq!(cs[0].deck, vec!["D".to_string(), "2과목".to_string()]);

        // 항목 하나가 깨져도 나머지는 들어온다. 실패는 전부 몇 번째인지 보고한다
        let (cs, iss, _) = parse_json(r#"[{"front":"a","back":"b"},{"front":"x"},{"back":"y"},{"front":"z","back":"  "},"문자열",{"front":1,"back":"b"}]"#);
        assert_eq!(cs.len(), 1);
        assert_eq!(iss.len(), 5);
        assert!(iss.iter().all(|i| i.index.is_some() && i.severity == "skipped"));

        // 중복 front 는 버리지 않고 경고
        let (cs, iss, _) = parse_json(r#"[{"front":"a","back":"1"},{"front":"a","back":"2"}]"#);
        assert!(cs.len() == 2 && iss.iter().any(|i| i.kind == "duplicate"));

        // importance 명시값과 오타
        let (cs, _, _) = parse_json(r#"[{"front":"a","back":"b","importance":"핵심"}]"#);
        assert_eq!(cs[0].importance, Some(2));
        let (cs, iss, _) = parse_json(r#"[{"front":"a","back":"b","importance":"몰라"}]"#);
        assert!(cs[0].importance.is_none() && iss.iter().any(|i| i.kind == "bad_importance"));

        // tags 는 배열도 문자열도 받는다
        assert_eq!(parse_json(r#"[{"front":"a","back":"b","tags":["x","y"]}]"#).0[0].tags, "x y");
        assert_eq!(parse_json(r#"[{"front":"a","back":"b","tags":"x  y"}]"#).0[0].tags, "x y");

        // 빈칸 카드는 back 이 없어도 된다
        let (cs, iss, _) = parse_json(r#"[{"front":"TCP는 {{c1::연결 지향}}"}]"#);
        assert!(cs.len() == 1 && cs[0].kind == "cloze" && iss.is_empty());

        // 앞뒤 둘 다
        let (cs, iss, _) = parse_json(r#"{"note_type":"Basic (and reversed card)","cards":[{"front":"a","back":"b"}]}"#);
        assert!(cs[0].kind == "reversed" && iss.is_empty());
        assert_eq!(parse_json(r#"[{"front":"a","back":"b","note_type":"reversed"}]"#).0[0].kind, "reversed");

        // 최상위가 엉뚱하면 전체 실패
        assert!(parse_json(r#""문자열""#).2.is_some());
        assert!(parse_json(r#"{"cards":"배열아님"}"#).2.is_some());
        assert!(parse_json("[]").2.is_some());
        assert!(parse_json("{ 깨진").2.is_some());
    }

    #[test]
    fn anki_schedule_mapping() {
        let f = fsrs::FSRS::default();
        let base = AnkiCard { queue: 2, ctype: 2, due: 100, ivl: 10, factor: 2500, reps: 5, lapses: 1, data: String::new() };
        let crt = 20_000;
        let s = schedule(&base, crt, &f);
        assert_eq!(s.state, "review");
        assert_eq!(s.interval_days, 10.0);
        assert_eq!(db::study_day(s.due_at.unwrap()), crt + 100, "복습 기한은 생성일부터의 일수");
        assert!(s.stability.is_some(), "SM-2 에서 기억 상태를 만든다");

        // Anki FSRS 값이 있으면 그대로 쓴다
        let fsrs_card = AnkiCard { data: r#"{"s":12.5,"d":4.2,"dr":0.9}"#.into(), ..base };
        let s = schedule(&fsrs_card, crt, &f);
        assert_eq!((s.stability, s.difficulty), (Some(12.5), Some(4.2)));

        // 분 단위 학습은 epoch 초
        let learning = AnkiCard { ctype: 1, queue: 1, due: 1_800_000_000, ivl: 0, ..fsrs_card };
        let s = schedule(&learning, crt, &f);
        assert_eq!((s.state, s.due_at, s.step), ("learning", Some(1_800_000_000_000), Some(0)));

        // 일시 정지는 상태만 바꾼다. 신규는 일정이 없다
        let suspended = AnkiCard { queue: -1, ctype: 2, due: 100, ivl: 10, factor: 2500, reps: 5, lapses: 1, data: String::new() };
        assert_eq!(schedule(&suspended, crt, &f).state, "suspended");
        let new = AnkiCard { queue: 0, ctype: 0, due: 7, ivl: 0, factor: 0, reps: 0, lapses: 0, data: String::new() };
        assert_eq!(schedule(&new, crt, &f), Schedule { state: "new", due_at: None, interval_days: 0.0, stability: None, difficulty: None, step: None });
    }

    #[test]
    fn protobuf_reader() {
        // field1 = "hi", field8 = varint 1
        assert_eq!(pb_str(b"\x0a\x02hi\x40\x01", 1), "hi");
        assert_eq!(pb_str(b"\x0a\x02hi", 2), "");
        assert!(pb_fields(b"").is_empty());
    }

    #[test]
    fn relative_path_strips_target_name() {
        let p = vec!["정보처리기사".to_string(), "2과목".to_string()];
        assert_eq!(relative_path(&p, Some("정보처리기사")), vec!["2과목".to_string()]);
        assert_eq!(relative_path(&p, Some("다른 덱")), p);
    }

    /// 테스트가 끝나면 (실패해도) 지운다
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn state(dir: &Path) -> AppState {
        std::fs::create_dir_all(dir.join("media")).unwrap();
        AppState {
            db: std::sync::Mutex::new(db::open(&dir.join("neoanki.sqlite")).unwrap()),
            fsrs: fsrs::FSRS::default(),
            data_dir: dir.to_path_buf(),
            undo: Default::default(),
            import_cancel: Default::default(),
            startup_error: Default::default(),
        }
    }

    /// 구버전(스키마 11) .apkg. 덱 A::1 · A::2 · B 와 필터 덱 F (A::1 의 카드 하나가 올라가 있다).
    /// 노트마다 카드 한 장 · 미디어 하나 · 기록 하나
    fn two_deck_package(path: &Path) {
        let db = path.with_extension("anki2");
        let col = Connection::open(&db).unwrap();
        col.execute_batch(
            "CREATE TABLE col (id integer primary key, crt integer, mod integer, scm integer, ver integer, dty integer, usn integer, ls integer,
                               conf text, models text, decks text, dconf text, tags text);
             CREATE TABLE notes (id integer primary key, guid text, mid integer, mod integer, usn integer, tags text, flds text, sfld text,
                                 csum integer, flags integer, data text);
             CREATE TABLE cards (id integer primary key, nid integer, did integer, ord integer, mod integer, usn integer, type integer,
                                 queue integer, due integer, ivl integer, factor integer, reps integer, lapses integer, left integer,
                                 odue integer, odid integer, flags integer, data text);
             CREATE TABLE revlog (id integer primary key, cid integer, usn integer, ease integer, ivl integer, lastIvl integer,
                                  factor integer, time integer, type integer);",
        )
        .unwrap();
        let models = r#"{"7": {"name": "Basic", "css": ".card {}", "flds": [{"name": "Front"}, {"name": "Back"}],
                         "tmpls": [{"ord": 0, "name": "Card 1", "qfmt": "{{Front}}", "afmt": "{{FrontSide}}<hr id=answer>{{Back}}"}]}}"#;
        let decks = r#"{"1": {"name": "Default", "dyn": 0}, "11": {"name": "A::1", "dyn": 0}, "12": {"name": "A::2", "dyn": 0},
                        "20": {"name": "B", "dyn": 0}, "30": {"name": "F", "dyn": 1}}"#;
        col.execute("INSERT INTO col VALUES (1, 0, 0, 0, 11, 0, 0, 0, '{}', ?1, ?2, '{}', '{}')", params![models, decks]).unwrap();
        // (노트, 그림, 카드가 있는 덱, 원래 덱)
        for (n, img, did, odid) in [(1, "a1.png", 11, 0), (2, "a2.png", 12, 0), (3, "b.png", 20, 0), (4, "f.png", 30, 11)] {
            col.execute(
                "INSERT INTO notes VALUES (?1, ?2, 7, 0, 0, ?4, ?3, '', 0, 0, '')",
                params![n, format!("guid{n}"), format!("앞 {n}\x1f<img src=\"{img}\">"), if n == 1 { "기출::2023_중간" } else { "" }],
            )
            .unwrap();
            col.execute("INSERT INTO cards VALUES (?1, ?2, ?3, 0, 0, 0, 2, 2, 5, 3, 2500, 2, 0, 0, 0, ?4, 0, '')", params![100 + n, n, did, odid])
                .unwrap();
            col.execute("INSERT INTO revlog VALUES (?1, ?2, 0, 3, 3, 1, 2500, 1000, 1)", params![1_000 + n, 100 + n]).unwrap();
        }
        drop(col);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("collection.anki2", opts).unwrap();
        std::io::Write::write_all(&mut zip, &std::fs::read(&db).unwrap()).unwrap();
        zip.start_file("media", opts).unwrap();
        std::io::Write::write_all(&mut zip, br#"{"0": "a1.png", "1": "a2.png", "2": "b.png", "3": "f.png"}"#).unwrap();
        for i in 0..4 {
            zip.start_file(i.to_string(), opts).unwrap();
            std::io::Write::write_all(&mut zip, format!("img{i}").as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn reimport_keeps_review_history() {
        // 이 앱에서 학습하고 기록을 포함해 내보낸 덱을 같은 DB 로 다시 가져온다 (I79).
        // 내보내기가 쓰는 모양 그대로: 노트 guid 는 note.anki_guid 에 남고, 카드에는 anki_card_id 가 없고, revlog id 는 기록 시각이다
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-revlog-{}-{}", std::process::id(), now_ms())));
        let s = state(&tmp.0.join("data"));
        let pkg_path = tmp.0.join("two.apkg");
        two_deck_package(&pkg_path);
        {
            let con = s.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('B', 0, 0)", []).unwrap();
            let deck = con.last_insert_rowid();
            let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic' AND anki_model_id IS NULL", [], |r| r.get(0)).unwrap();
            con.execute("INSERT INTO note (note_type_id, fields, anki_guid, created_at, updated_at) VALUES (?1, '[\"앞 3\",\"\"]', 'guid3', 0, 0)", [nt])
                .unwrap();
            con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (last_insert_rowid(), ?1, 0, 0, 0)", [deck])
                .unwrap();
            // 학습한 기록 (anki_revlog_id 없음). 패키지의 revlog 1003 이 이 기록이다
            con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (last_insert_rowid(), 1003, 3)", []).unwrap();
        }
        let count = |s: &AppState| -> (i64, i64) {
            let con = s.db.lock().unwrap();
            con.query_row("SELECT count(*), count(*) - count(card_id) FROM review_log", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        };
        let r = run_import_anki(&|_, _, _| {}, &s, pkg_path.to_str().unwrap(), true, &mut vec![]).unwrap();
        assert_eq!(r.reviews, 3, "이미 있는 노트(guid3)의 기록은 원래 카드의 같은 기록이라 넣지 않는다");
        assert_eq!(count(&s), (4, 0), "card_id 없는 기록이 생기지 않는다");
        let r = run_import_anki(&|_, _, _| {}, &s, pkg_path.to_str().unwrap(), true, &mut vec![]).unwrap();
        assert_eq!((r.notes, r.reviews), (0, 0), "다시 가져와도 기록 수가 그대로");
        assert_eq!(count(&s), (4, 0));
    }

    #[test]
    fn selective_import() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-b2s-{}-{}", std::process::id(), now_ms())));
        let s = state(&tmp.0.join("data"));
        let pkg_path = tmp.0.join("two.apkg");
        two_deck_package(&pkg_path);

        // 덱 고르기 트리: 실제 덱마다 자기 카드. 필터 덱의 카드는 원래 덱(A::1)으로 센다
        let pkg = open_package(&pkg_path, &s.data_dir).unwrap();
        let stats = deck_stats(&pkg, &HashSet::from(["guid3".to_string()]), i64::MAX).unwrap();
        let row = |id: i64| stats.iter().find(|d| d.id == id).unwrap();
        assert_eq!(stats.len(), 3, "필터 덱은 트리에 없다");
        assert_eq!((row(11).path.join("::"), row(11).cards, row(11).new_cards, row(11).reviews, row(11).overdue), ("A::1".into(), 2, 2, 2, 2));
        let b = row(20);
        assert_eq!((b.cards, b.notes, b.existing_notes), (1, 1, 1), "이미 있는 노트도 트리의 장수에는 센다");
        assert_eq!((b.new_cards, b.reviews, b.overdue, b.types.len()), (0, 0, 0, 0), "들어가지 않을 카드는 사전 검사에서 뺀다");
        assert_eq!(n(4342), "4,342");
        assert_eq!((n(0), n(999), n(1_000_000)), ("0".into(), "999".into(), "1,000,000".into()));
        drop(pkg);

        // A::1 만 가져온다: 노트 1 · 4, 그 카드 · 그림 · 기록만
        let picked = HashSet::from([11]);
        let r = run_import_selected(&|_, _, _| {}, &s, pkg_path.to_str().unwrap(), true, Some(&picked), &mut vec![]).unwrap();
        assert_eq!((r.notes, r.cards, r.media, r.reviews, r.filtered_cards), (2, 2, 2, 2, 1));
        let con = s.db.lock().unwrap();
        let strings = |sql: &str| -> Vec<String> { con.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap() };
        assert_eq!(strings("SELECT name FROM deck ORDER BY id"), ["A", "1"], "고르지 않은 덱(A::2, B)은 만들지 않는다");
        assert_eq!(strings("SELECT anki_guid FROM note ORDER BY anki_guid"), ["guid1", "guid4"]);
        assert_eq!(strings("SELECT filename FROM media ORDER BY filename"), ["a1.png", "f.png"], "고른 노트가 쓰는 미디어만");
        assert!(!s.data_dir.join("media/a2.png").exists() && !s.data_dir.join("media/b.png").exists());
        assert_eq!(strings("SELECT anki_revlog_id || '' FROM review_log ORDER BY 1"), ["1001", "1004"], "고른 카드의 기록만");
        assert_eq!(
            strings("SELECT anki_guid || ' ' || exam_hits || ' ' || exam_names FROM note WHERE exam_hits > 0"),
            [r#"guid1 1 ["2023 중간"]"#],
            "기출 태그를 기출 표시로 (8.4)"
        );
        drop(con);

        // 나머지를 나중에 같은 파일로: 이미 있는 노트는 건너뛰고 B 만 들어온다
        let r = run_import_selected(&|_, _, _| {}, &s, pkg_path.to_str().unwrap(), false, Some(&HashSet::from([11, 20])), &mut vec![]).unwrap();
        assert_eq!((r.notes, r.cards, r.media), (1, 1, 1));
        assert!(r.issues.iter().any(|i| i.kind == "duplicate"));
    }
}
