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
use crate::{recall, srs};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

pub(crate) mod deck_update;

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
    /// basic | reversed | cloze (NewCard.kind) | recall::LIST · CHOICE (목록 · 선택형 노트 유형 이름)
    kind: &'static str,
    /// 목록 · 선택형 필드 (Q10). 나머지는 비어 있다
    fields: Vec<String>,
}

fn split_path(p: &str) -> Vec<String> {
    p.split("::").map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

fn has_cloze(s: &str) -> bool {
    s.match_indices("{{c").any(|(i, _)| s[i + 3..].starts_with(|c: char| c.is_ascii_digit()) && s[i + 3..].contains("::"))
}

/// 목록 · 선택형 (Q10). 여기서는 키 모양만 보고, 내용은 앱의 저장 검증(recall::list_fields · choice_fields) 그대로
fn json_recall(nt: &str, o: &serde_json::Map<String, Value>) -> Result<Vec<String>, (&'static str, String)> {
    let missing = |key: &str| ("missing_field", format!("{key} 키가 없어요"));
    let text = |key: &str| match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(("bad_field", format!("{key} 가 문자열이 아니에요"))),
    };
    let flag = |key: &str| match o.get(key) {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(("bad_field", format!("{key} 는 true 또는 false 예요"))),
    };
    let strings = |key: &str| match o.get(key) {
        None | Some(Value::Null) => Err(missing(key)),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(String::from).ok_or(("bad_field", format!("{key} 에 문자열이 아닌 값이 있어요")))).collect(),
        Some(_) => Err(("bad_field", format!("{key} 가 배열이 아니에요"))),
    };
    let question = text("question")?.ok_or_else(|| missing("question"))?;
    let extra = text("extra")?.unwrap_or_default();
    let checked = |r: Res<Vec<String>>| r.map_err(|e| ("invalid", e));
    if nt == recall::LIST {
        let items = strings("items")?;
        return checked(recall::list_fields(&recall::ListNew { deck_id: 0, question, items, ordered: flag("ordered")?, extra, importance: None }));
    }
    let options: Vec<String> = strings("options")?;
    // 정답은 둘 중 하나 (Q26). answer = 보기 글자 (앞뒤 공백만 무시), correct = 보기 번호 (1부터, 화면 · 정답 칸과 같다).
    // 하나면 배열 없이 써도 된다. 앱 화면에서는 없는 보기를 고를 수 없어 choice_fields 가 조용히 버린다. 여기서는 건너뛰고 알린다
    let given = |key: &str| match o.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.iter().collect::<Vec<_>>()),
        Some(v) => Some(vec![v]),
    };
    let correct = match (given("answer"), given("correct")) {
        (Some(_), Some(_)) => return Err(("bad_field", "correct 와 answer 중 하나만 써 주세요".into())),
        (None, None) => return Err(missing("answer (또는 correct)")),
        (Some(answer), None) => answer
            .iter()
            .map(|v| {
                let t = v.as_str().ok_or(("bad_field", "answer 는 보기 글자예요 (번호는 correct)".to_string()))?.trim();
                let hits: Vec<usize> = options.iter().enumerate().filter(|(_, o)| !t.is_empty() && o.trim() == t).map(|(i, _)| i).collect();
                match hits[..] {
                    [i] => Ok(i),
                    [] => Err(("invalid", format!("answer '{t}' 와 같은 보기가 없어요"))),
                    _ => Err(("invalid", format!("answer '{t}' 와 같은 보기가 여러 개예요"))),
                }
            })
            .collect::<Result<Vec<_>, _>>()?,
        (None, Some(correct)) => correct
            .iter()
            .map(|v| match v.as_u64() {
                Some(n) if n >= 1 && options.get(n as usize - 1).is_some_and(|o| !o.trim().is_empty()) => Ok(n as usize - 1),
                Some(n) if n >= 1 => Err(("invalid", format!("correct 의 {n}번 보기가 없거나 비어 있어요"))),
                _ => Err(("bad_field", "correct 는 1부터 세는 보기 번호예요".into())),
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    checked(recall::choice_fields(&recall::ChoiceNew { deck_id: 0, question, options, correct, shuffle: flag("shuffle")?, extra, importance: None }))
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
        // 목록 · 선택형 (Q10): note_type 이 list · choice (또는 앱 유형 이름)이거나 items · options 키가 있으면
        let is = |short: &str, name: &str, key: &str| note_type.eq_ignore_ascii_case(short) || note_type == name || o.contains_key(key);
        let special = if is("list", recall::LIST, "items") {
            Some(recall::LIST)
        } else if is("choice", recall::CHOICE, "options") {
            Some(recall::CHOICE)
        } else {
            None
        };
        let (front, back, kind, fields) = match special {
            Some(nt) => match json_recall(nt, o) {
                // 질문은 중복 경고에만 쓴다
                Ok(f) => (f[0].clone(), String::new(), nt, f),
                Err((kind, d)) => {
                    issues.push(at(i, "skipped", kind, d));
                    continue;
                }
            },
            None => {
                let front = o.get("front");
                let cloze = note_type.eq_ignore_ascii_case("cloze") || front.and_then(Value::as_str).is_some_and(has_cloze);
                // 빈칸 카드는 back 이 없어도 된다 (추가 설명)
                let mut bad = None;
                // 고칠 곳을 찾기 쉽게 뜻과 키를 같이 (X1-04 "답(back)이 비어 있어요")
                for (key, name, val, required) in [("front", "질문(front)", front, true), ("back", "답(back)", o.get("back"), !cloze)] {
                    match val {
                        None | Some(Value::Null) => {
                            if required {
                                bad = Some((key, "missing_field", format!("{name} 항목이 없어요")));
                            }
                        }
                        Some(Value::String(s)) => {
                            if required && crate::render::is_blank(s) {
                                bad = Some((key, "empty_field", format!("{name}이 비어 있어요")));
                            }
                        }
                        Some(_) => bad = Some((key, "bad_field", format!("{name}이 문자열이 아니에요"))),
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
                    issues.push(at(i, "info", "note_type", format!("노트타입 '{note_type}'의 카드는 질문과 답변으로 가져왔어요")));
                }
                (front, back, if cloze { "cloze" } else if reversed { "reversed" } else { "basic" }, vec![])
            }
        };

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
                    issues.push(at(i, "warning", "bad_importance", format!("중요도(importance) 값 '{word}'를 읽지 못해 태그 기준으로 정했어요")));
                }
                level
            }
        };
        match seen.get(&front) {
            Some(first) => issues.push(at(i, "warning", "duplicate", format!("질문(front)이 {first}번째 항목과 같아요 · 둘 다 가져왔어요"))),
            None => {
                seen.insert(front.clone(), i);
            }
        }
        let deck = o.get("deck").and_then(Value::as_str).map(String::from).or(default_deck.clone()).unwrap_or_else(|| DEFAULT_DECK.into());
        let deck = match split_path(&deck) {
            p if p.is_empty() => vec![DEFAULT_DECK.to_string()],
            p => p,
        };
        out.push(JsonCard { index: i, front, back, deck, tags, importance, kind, fields });
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
        let n = match c.kind {
            recall::LIST | recall::CHOICE => recall::insert(&tx, c.kind, did, &c.fields, c.importance, &c.tags, Some(batch)).map(|_| 1),
            kind => {
                let card = NewCard { deck_id: did, kind: kind.into(), front: c.front.clone(), back: c.back.clone(), importance: c.importance };
                insert_note(&tx, &card, &c.tags, Some(batch))
            }
        }
        .map_err(|e| format!("#{}: {e}", c.index))?;
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

/// 이미 있던 노트 유형에 들어오는 것: (Anki 필드 i -> 우리 자리, 우리 자리 이름 (지운 자리는 ""), Anki 템플릿 ord -> 우리 ord. 없는 ord = 이 앱에서 지운 카드 종류.
/// 빈칸 유형은 None: 카드 ord 가 빈칸 번호라 그대로 쓴다)
type Remap = (Vec<Option<usize>>, Vec<String>, Option<HashMap<u32, u32>>);

struct AType {
    name: String,
    fields: Vec<String>,
    css: String,
    templates: Vec<(u32, String, String, String)>, // ord, 이름, 앞면, 뒷면
    /// 원본과 다르게 보일 수 있는 기능 (스크립트, 빈칸 type-in, TTS)
    degraded: Vec<&'static str>,
    /// Anki originalStockKind (0 = 사용자 정의). 내보낼 때 다시 붙인다 (I78)
    stock: i64,
    /// 카드 종류마다 덱 (Anki deck override, 템플릿 did · target_deck_id): (ord, Anki 덱 id). NT76
    decks: Vec<(u32, i64)>,
}

struct Package {
    // 필드는 적은 순서대로 닫힌다. Windows 는 열린 파일을 지우지 못하므로 연결(col)이 임시 파일(_tmp)보다 먼저다
    col: Connection,
    zip: zip::ZipArchive<std::fs::File>,
    decks: HashMap<i64, ADeck>,
    types: HashMap<i64, AType>,
    _tmp: TmpFile,
}

/// 임시 컬렉션 (import-<pid>.sqlite). 어디서 실패해도 지운다: Package 를 만들기 전(열기 · 첫 쿼리)에 실패해도 (T7-05)
struct TmpFile(PathBuf);

impl Drop for TmpFile {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

/// 지난 실행이 남긴 임시 컬렉션 (가져오는 중 강제 종료 · 충돌). 앱을 열 때 지운다 (T7-05)
pub fn clean_stale(data_dir: &Path) {
    for e in std::fs::read_dir(data_dir).into_iter().flatten().flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("import-") && name.contains(".sqlite") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// 압축을 푼 크기 상한 (T7-04 압축 폭탄). 컬렉션 · 미디어 목록 · 미디어 파일 하나 · 패키지 전체
const MAX_COLLECTION: u64 = 2 << 30;
const MAX_MEDIA_LIST: u64 = 64 << 20;
const MAX_MEDIA_FILE: u64 = 512 << 20;
const MAX_TOTAL: u64 = 10 << 30;
/// 이만큼 푼 뒤에도 압축 크기의 MAX_RATIO 배를 넘으면 폭탄으로 보고 멈춘다. 작은 파일은 압축률이 높아도 괜찮다
const RATIO_FROM: u64 = 64 << 20;
const MAX_RATIO: u64 = 100;

fn mb(bytes: u64) -> String {
    n((bytes >> 20) as usize)
}

/// zip 항목을 out 에 푼다. 최신 Anki 처럼 zstd 로 한 번 더 압축한 항목도 푼다. 상한이나 압축률을 넘으면 그 자리에서 멈춘다
fn unpack(zip: &mut zip::ZipArchive<std::fs::File>, entry: &str, label: &str, limit: u64, out: &mut impl Write) -> Res<u64> {
    let mut f = zip.by_name(entry).map_err(err)?;
    let packed = f.compressed_size().max(1);
    let mut head = vec![];
    (&mut f).take(4).read_to_end(&mut head).map_err(err)?;
    let mut src: Box<dyn Read + '_> = if head.starts_with(&ZSTD_MAGIC) {
        Box::new(zstd::stream::read::Decoder::new(head.as_slice().chain(f)).map_err(err)?)
    } else {
        Box::new(head.as_slice().chain(f))
    };
    let mut buf = vec![0; 1 << 16];
    let mut total = 0u64;
    loop {
        let got = src.read(&mut buf).map_err(err)?;
        if got == 0 {
            return Ok(total);
        }
        total += got as u64;
        if total > limit || (total > RATIO_FROM && total / packed > MAX_RATIO) {
            return Err(format!(
                "{label}의 압축을 풀면 너무 커져서 멈췄어요 (최대 {} MB · 압축 크기의 {MAX_RATIO}배). 손상됐거나 안전하지 않은 파일일 수 있어요.",
                mb(limit)
            ));
        }
        out.write_all(&buf[..got]).map_err(err)?;
    }
}

/// voices: 이 기기에서 읽을 수 있는 언어의 앞부분 ("en"). 화면이 내장 음성과 OS 음성을 합쳐 보낸다
fn degraded_features(body: &str, voices: &[String]) -> Vec<&'static str> {
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
    // AU23 읽어주기는 그 언어 음성이 이 기기에 없을 때만 알린다 (경고가 아니라 참고 한 줄)
    for lang in crate::tts::template_langs(body) {
        let f = crate::tts::no_voice(lang);
        if !voices.contains(&crate::tts::prefix(lang)) && !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

fn open_package(path: &Path, work: &Path, voices: &[String]) -> Res<Package> {
    let file = std::fs::File::open(path).map_err(|e| format!("파일을 열 수 없어요: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "Anki 패키지(zip) 형식이 아니에요.".to_string())?;
    let names: HashSet<String> = zip.file_names().filter_map(Result::ok).map(|n| n.into_owned()).collect();
    // anki21b 가 우선이다. collection.anki2 는 구버전 경고용 스텁일 수 있다
    let entry = ["collection.anki21b", "collection.anki21", "collection.anki2"]
        .into_iter()
        .find(|e| names.contains(*e))
        .ok_or("패키지 안에 컬렉션이 없어요.")?;
    std::fs::create_dir_all(work).map_err(err)?;
    // 메모리에 올리지 않고 임시 파일로 바로 푼다. 이 뒤 어디서 실패해도 tmp 가 지운다
    let tmp = TmpFile(work.join(format!("import-{}.sqlite", std::process::id())));
    unpack(&mut zip, entry, "컬렉션", MAX_COLLECTION, &mut std::fs::File::create(&tmp.0).map_err(err)?)?;
    let mut magic = vec![];
    std::fs::File::open(&tmp.0).map_err(err)?.take(15).read_to_end(&mut magic).map_err(err)?;
    if magic != b"SQLite format 3" {
        return Err("컬렉션이 SQLite 파일이 아니에요.".into());
    }
    let col = Connection::open_with_flags(&tmp.0, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    // Anki 가 실행 중에 등록하는 콜레이션이라 파일에 정의가 없다. 읽기만 하므로 대소문자 무시 비교면 된다
    col.create_collation("unicase", |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase())).map_err(err)?;

    let modern: bool = col
        .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'notetypes'", [], |r| r.get::<_, i64>(0))
        .map_err(err)?
        > 0;
    let (decks, types) = if modern { read_modern(&col, voices)? } else { read_legacy(&col, voices)? };
    Ok(Package { col, zip, decks, types, _tmp: tmp })
}

/// 스키마 18 (Anki 2.1.50+). 덱 이름은 \x1f 로 계층을 나눈다
fn read_modern(col: &Connection, voices: &[String]) -> Res<(HashMap<i64, ADeck>, HashMap<i64, AType>)> {
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
        // CardTemplate.Config: 1 앞면 · 2 뒷면 · 5 target_deck_id (deck override, 0 = 없음)
        let tdecks: Vec<(u32, i64)> = raw
            .iter()
            .filter_map(|(ord, _, c)| {
                pb_fields(c.as_deref().unwrap_or_default()).into_iter().find_map(|(n, v)| match v {
                    Pb::Varint(d) if n == 5 && d > 0 => Some((*ord, d as i64)),
                    _ => None,
                })
            })
            .collect();
        let templates: Vec<(u32, String, String, String)> = raw
            .into_iter()
            .map(|(ord, n, c)| {
                let c = c.unwrap_or_default();
                (ord, n, pb_str(&c, 1), pb_str(&c, 2))
            })
            .collect();
        let mut degraded = vec![];
        for t in &templates {
            for f in degraded_features(&format!("{}\n{}", t.2, t.3), voices) {
                if !degraded.contains(&f) {
                    degraded.push(f);
                }
            }
        }
        types.insert(id, AType { name, fields, css: pb_str(&cfg, 3), templates, degraded, stock: stock.unwrap_or(0), decks: tdecks });
    }
    Ok((decks, types))
}

/// 스키마 11 (구버전 .apkg). 덱과 노트타입이 col 테이블의 JSON 안에 있다
fn read_legacy(col: &Connection, voices: &[String]) -> Res<(HashMap<i64, ADeck>, HashMap<i64, AType>)> {
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
                for f in degraded_features(&format!("{}\n{}", tp.2, tp.3), voices) {
                    if !degraded.contains(&f) {
                        degraded.push(f);
                    }
                }
            }
            let stock = t.get("originalStockKind").and_then(Value::as_i64).unwrap_or(0);
            let tdecks: Vec<(u32, i64)> = t
                .get("tmpls")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .enumerate()
                        .filter_map(|(i, x)| Some((x.get("ord").and_then(Value::as_u64).unwrap_or(i as u64) as u32, x.get("did").and_then(Value::as_i64).filter(|d| *d > 0)?)))
                        .collect()
                })
                .unwrap_or_default();
            types.insert(id, AType { name: s(&t, "name"), fields, css: s(&t, "css"), templates, degraded, stock, decks: tdecks });
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
    let mut raw = vec![];
    unpack(&mut pkg.zip, "media", "미디어 목록", MAX_MEDIA_LIST, &mut raw)?;
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

/// 미디어 이름의 상한 (UTF-16 글자 수. Windows 이름 한 칸은 255)
const MAX_NAME: usize = 200;

/// 이름의 확장자 앞에 suffix 를 붙인다 (a.png -> a-suffix.png). 전체가 MAX_NAME 을 넘으면 확장자 앞을 줄인다
fn fit_name(name: &str, suffix: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() && e.len() <= 10 => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let tail = format!("{suffix}{ext}");
    let mut room = MAX_NAME.saturating_sub(tail.encode_utf16().count());
    let mut out = String::new();
    for c in stem.chars() {
        if c.len_utf16() > room {
            break;
        }
        room -= c.len_utf16();
        out.push(c);
    }
    out + &tail
}

/// Windows 에 그대로 저장할 수 없는 미디어 이름을 저장할 수 있는 이름으로 (T7-06 · T7-09). safe_media_name 을 지난 이름만 온다.
/// 금지 글자 `<>:"/\|?*` · 제어 문자 · 방향 제어 문자는 `_`, 끝의 점 · 공백은 떼고, 장치 이름(CON · NUL · COM1 …)은 앞에 `_`, 길면 줄인다
fn storable_name(name: &str) -> Cow<'_, str> {
    let bad = |c: char| c.is_control() || "<>:\"/\\|?*\u{61c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}".contains(c);
    let mut out: String = name.chars().map(|c| if bad(c) { '_' } else { c }).collect();
    if out.encode_utf16().count() > MAX_NAME {
        out = fit_name(&out, "");
    }
    out.truncate(out.trim_end_matches(['.', ' ']).len());
    let base = out.split('.').next().unwrap_or_default().trim_end_matches(' ').to_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str())
        || ["COM", "LPT"].iter().any(|p| base.strip_prefix(p).is_some_and(|d| d.chars().count() == 1 && d.chars().all(|c| c.is_ascii_digit() || "¹²³".contains(c))));
    if device || out.is_empty() {
        out.insert(0, '_');
    }
    if out == name { Cow::Borrowed(name) } else { Cow::Owned(out) }
}

/// 그 이름의 파일이 없거나 내용이 같은가 (같으면 그대로 쓴다). Windows 는 이름의 대소문자를 가리지 않아 A.png 도 a.png 로 보인다
fn same_or_absent(path: &Path, blob: &[u8]) -> bool {
    match std::fs::metadata(path) {
        Err(_) => true,
        Ok(m) => m.len() == blob.len() as u64 && std::fs::read(path).is_ok_and(|b| b == blob),
    }
}

/// 필드의 미디어 참조(<img src> · [sound:])를 바꾼 이름으로 고친다. 이름은 HTML 엔티티를 푼 값으로 찾고,
/// 원문이 엔티티로 적혀 있었으면 새 이름도 엔티티로 쓴다 (Anki 의 replace_media_refs 와 같다)
fn rename_refs(html: &str, renamed: &HashMap<String, String>) -> String {
    let unescape = |s: &str| s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        // 다음 참조의 값 범위 [a, b)
        let src = rest.find("src=").map(|i| {
            let v = i + 4;
            match rest[v..].chars().next() {
                Some(q @ ('"' | '\'')) => (v + 1, rest[v + 1..].find(q).map_or(rest.len(), |e| v + 1 + e)),
                _ => (v, rest[v..].find([' ', '>']).map_or(rest.len(), |e| v + e)),
            }
        });
        let sound = rest.find("[sound:").and_then(|i| rest[i + 7..].find(']').map(|e| (i + 7, i + 7 + e)));
        let Some((a, b)) = [src, sound].into_iter().flatten().min() else { break };
        let old = &rest[a..b];
        let plain = unescape(old);
        out.push_str(&rest[..a]);
        match renamed.get(&plain) {
            Some(new) if plain != old => out.push_str(&crate::occlusion::escape(new)),
            Some(new) => out.push_str(new),
            None => out.push_str(old),
        }
        rest = &rest[b..];
    }
    out.push_str(rest);
    out
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
    /// B2-u0 이미 가져온 덱의 새 버전 같으면 그 근거 (이미 있는 노트가 하나도 없으면 없다). 기본 모드는 suggested (DU15)
    update: Option<deck_update::UpdateHint>,
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
pub async fn inspect_anki(state: State<'_, AppState>, path: String, voices: Option<Vec<String>>) -> Res<AnkiPreview> {
    let p = Path::new(&path);
    let mut pkg = open_package(p, &state.data_dir, &voices.unwrap_or_default())?;
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
    let update = deck_update::file_hint(&*state.db.lock().map_err(err)?, &pkg)?;
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
        update,
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
    voices: Option<Vec<String>>,
) -> Res<ImportResult> {
    state.import_cancel.store(false, Ordering::Relaxed);
    let emit = |step: &'static str, done: usize, total: usize| {
        let _ = app.emit("import-progress", Progress { step, done, total });
    };
    let mut written = vec![];
    let selected: Option<HashSet<i64>> = deck_ids.map(|v| v.into_iter().collect());
    let result = run_import_selected(&emit, &state, &path, with_history.unwrap_or(false), selected.as_ref(), &voices.unwrap_or_default(), &mut written);
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
    run_import_selected(emit, state, path, with_history, None, &[], written)
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
    voices: &[String],
    written: &mut Vec<PathBuf>,
) -> Res<ImportResult> {
    let cancelled = || state.import_cancel.load(Ordering::Relaxed);

    emit("unzip", 0, 1);
    let p = Path::new(path);
    let mut pkg = open_package(p, &state.data_dir, voices)?;
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
    // 8.1 이미 있던 유형(같은 모델 id)은 사용자가 필드 순서 · 이름 · 카드 종류를 고쳤을 수 있다: 들어오는 필드는 자리로, 카드 ord 는 우리 ord 로
    let mut remap: HashMap<i64, Remap> = HashMap::new();
    for (aid, t) in &pkg.types {
        if !by_type.contains_key(aid) {
            continue;
        }
        // 내보낸 내 덱의 앱 기본 유형은 내보낸 모델 id 로 찾는다 (DU55, 새 "Basic" 이 생기지 않게)
        let existing = deck_update::type_of_model(&tx, *aid)?.map(|(id, _)| id);
        let id = match existing {
            Some(id) => {
                let ours = crate::notetype::load(&tx, id)?;
                let tmpls: Vec<(u32, String, String)> = t.templates.iter().map(|(o, n, q, _)| (*o, n.clone(), q.clone())).collect();
                let ords = (!ours.cloze()).then(|| crate::notetype::incoming_ords(&ours, &tmpls));
                remap.insert(*aid, (crate::notetype::incoming_slots(&ours, &t.fields), ours.slots.clone(), ords));
                id
            }
            None => insert_type(&tx, *aid, t, now, &|did| Ok((deck_cache.get(&did).copied(), pkg.decks.get(&did).map(|d| d.path.join(" › ")))))?,
        };
        nt_map.insert(*aid, id);
        // Q10: 이름은 목록 · 선택형인데 필드가 바뀐 노트는 알아보지 못하고 템플릿대로 보인다
        if crate::recall::unrecognized(&t.name, &t.fields) {
            result.issues.push(issue("info", "recall_type", format!("노트타입 '{}'은 필드가 바뀌어 목록 · 선택형으로 알아보지 못했어요. 질문과 답변처럼 한 번에 보여요", t.name)));
        }
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
    let mut note_mid: HashMap<i64, i64> = HashMap::new(); // 다시 맞출 유형의 노트만 (Anki 노트 id -> 모델 id)
    let mut dup = 0;
    let mut refs: HashSet<String> = HashSet::new(); // 고른 노트가 쓰는 미디어 (덱을 골랐을 때만 쓴다)
    // 다시 맞춘 유형에서 값이 숨긴 자리로 간 · 넣을 자리가 없는 Anki 필드 -> 노트 수 (Q2-13)
    let (mut hidden, mut dropped): (BTreeMap<String, usize>, BTreeMap<String, usize>) = Default::default();
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
            let mut fields: Vec<&str> = flds.split(SEP).collect();
            if let Some((slots, names, _)) = remap.get(&mid) {
                let mut out = vec![""; names.len()];
                for (i, (v, s)) in fields.iter().zip(slots).enumerate() {
                    let field = || pkg.types.get(&mid).and_then(|t| t.fields.get(i)).cloned().unwrap_or_default();
                    match s {
                        Some(s) => {
                            out[*s] = v;
                            if names[*s].is_empty() && !v.is_empty() {
                                *hidden.entry(field()).or_default() += 1;
                            }
                        }
                        None if !v.is_empty() => *dropped.entry(field()).or_default() += 1,
                        None => {}
                    }
                }
                fields = out;
                note_mid.insert(anid, mid);
            }
            // .apkg 로 내보낼 때만 다는 출처 태그(Neo-Anki::출처::)는 들이지 않는다 (export::source_tags, 쌓이지 않게). 사용자의 출처:: 태그는 그대로
            let tags = crate::export::without_source_tags(&tags);
            tx.execute(
                "INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, anki_note_id, anki_guid, import_batch_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?4, ?6, ?7)",
                params![ntid, serde_json::to_string(&fields).map_err(err)?, tags, anid, modified * 1000, guid, batch],
            )
            .map_err(err)?;
            note_map.insert(anid, (tx.last_insert_rowid(), tags));
        }
    }
    result.notes = note_map.len();
    if dup > 0 {
        result.issues.push(issue("info", "duplicate", format!("이미 가져온 노트 {}개는 다시 넣지 않았어요", n(dup))));
        // 같은 guid 가 이 기기에 두 벌 (두 기기가 받지 않은 채 같은 파일을 업데이트했다, DU-06)
        let twins: i64 = tx
            .query_row("SELECT count(*) FROM (SELECT anki_guid FROM note WHERE anki_guid IS NOT NULL GROUP BY anki_guid HAVING count(*) > 1)", [], |r| r.get(0))
            .map_err(err)?;
        if twins > 0 {
            result.issues.push(issue("warning", "duplicate_notes", format!("두 벌 있는 노트 {}개가 있어요. 카드 목록에서 하나를 지워 주세요", n(twins as usize))));
        }
    }
    for (f, c) in &hidden {
        result.issues.push(issue("info", "hidden_field", format!("이 앱에서 지운 필드 '{f}'의 값이 있는 노트 {}개는 그 값을 숨긴 자리에 넣었어요. 카드에는 나오지 않아요", n(*c))));
    }
    for (f, c) in &dropped {
        result.issues.push(issue("warning", "dropped_field", format!("이 노트 유형에 없는 필드 '{f}'의 값이 있는 노트 {}개는 그 값을 넣지 않았어요", n(*c))));
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
    // 이 앱에서 지운 카드 종류의 Anki 카드: (유형 이름, 카드 종류 이름) -> Anki 카드 id (Q2-08)
    let mut gone: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
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
            let ((acid, anid, did, aord, odid), anki) = row.map_err(err)?;
            if !picked_card(acid) {
                continue;
            }
            step(&mut done)?;
            let queue = anki.queue;
            let Some((nid, tags)) = note_map.get(&anid) else { continue };
            // 다시 맞춘 유형: 우리 ord 로. 없으면 이 앱에서 지운 카드 종류다. 같은 순번의 다른 카드 종류에 붙이지 않고 건너뛴다
            let ord = match note_mid.get(&anid) {
                Some(m) => match remap[m].2.as_ref().map(|o| o.get(&(aord as u32))) {
                    None => aord,
                    Some(Some(o)) => *o as i64,
                    Some(None) => {
                        let t = &pkg.types[m];
                        let tpl = t.templates.iter().find(|x| x.0 as i64 == aord).map_or_else(|| format!("{}", aord + 1), |x| x.1.clone());
                        gone.entry((t.name.clone(), tpl)).or_default().push(acid);
                        continue;
                    }
                },
                None => aord,
            };
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
    if !gone.is_empty() {
        let logs: HashMap<i64, i64> = {
            let mut st = pkg.col.prepare("SELECT cid, count(*) FROM revlog WHERE ease BETWEEN 1 AND 4 GROUP BY cid").map_err(err)?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
            rows.collect::<Result<_, _>>().map_err(err)?
        };
        for ((ty, tpl), cids) in &gone {
            let reviews = cids.iter().filter_map(|c| logs.get(c)).sum::<i64>() as usize;
            // 기록은 지운 카드의 기록처럼 카드 없이 통계에만 남긴다 (A4). 옮기지 않으면 넣지 않는다
            let logs = match (with_history, reviews) {
                (true, r) if r > 0 => format!(" 그 카드의 복습 기록 {}건은 통계에만 남겼어요.", n(r)),
                _ => String::new(),
            };
            result.skipped += cids.len();
            result.issues.push(issue("skipped", "deleted_card_type", format!("이 앱에서 지운 카드 종류 '{tpl}'({ty})의 카드 {}장은 넣지 않았어요.{logs}", n(cids.len()))));
        }
        // 카드가 하나도 남지 않은 새 노트는 넣지 않는다
        let empty = tx
            .execute("DELETE FROM note WHERE import_batch_id = ?1 AND NOT EXISTS (SELECT 1 FROM card WHERE card.note_id = note.id)", [batch])
            .map_err(err)?;
        result.notes -= empty.min(result.notes);
    }
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
    // 패키지의 이름 -> 저장한 이름. 이번에 넣은 노트의 참조를 고친다 (Anki 와 같다)
    let mut renamed: HashMap<String, String> = HashMap::new();
    let (mut clashes, mut fixed, mut unpacked) = (0, 0, 0u64);
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
        let mut blob = vec![];
        if let Err(e) = unpack(&mut pkg.zip, key, &format!("미디어 '{name}'"), MAX_MEDIA_FILE, &mut blob) {
            result.issues.push(issue("warning", "media", format!("미디어 '{name}' 를 읽지 못했어요: {e}")));
            continue;
        }
        unpacked += blob.len() as u64;
        if unpacked > MAX_TOTAL {
            return Err(format!("패키지의 미디어를 모두 풀면 {} MB를 넘어서 멈췄어요. 손상됐거나 안전하지 않은 파일일 수 있어요.", mb(MAX_TOTAL)));
        }
        // Windows 에 저장할 수 없는 이름은 바꾼다 (T7-06 · T7-09). 같은 이름에 내용이 다른 파일이 있으면 그 파일은 두고
        // 내용 해시를 붙인 새 이름으로 저장한다 (T2-01 · T7-02). 내용이 같으면 그대로 쓴다
        let storable = storable_name(name);
        let mut target = storable.to_string();
        if !same_or_absent(&dir.join(&target), &blob) {
            use sha2::{Digest, Sha256};
            target = fit_name(&storable, &format!("-{}", &format!("{:x}", Sha256::digest(&blob))[..8]));
            if !same_or_absent(&dir.join(&target), &blob) {
                result.issues.push(issue("warning", "media", format!("미디어 '{name}' 를 저장할 이름을 찾지 못해 건너뛰었어요")));
                continue;
            }
        }
        if target != *name {
            renamed.insert(name.clone(), target.clone());
        }
        let file = dir.join(&target);
        if !file.exists() {
            written.push(file.clone()); // 쓰다가 실패해도 지우게 먼저 남긴다
            std::fs::write(&file, &blob).map_err(err)?;
            result.media += 1; // 이번에 새로 저장한 파일만 센다 (이미 있던 같은 파일은 세지 않는다)
            if target != storable {
                clashes += 1;
            } else if target != *name {
                fixed += 1;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO media (filename, rel_path, sha256, byte_size, mime_type, created_at, anki_media_key) VALUES (?1, ?2, '', ?3, ?4, ?5, ?6)",
            params![target, format!("media/{target}"), blob.len() as i64, mime_of(&target), now, key],
        )
        .map_err(err)?;
    }
    emit("media", names.len(), names.len());
    // 넣은 노트의 미디어 참조를 바꾼 이름으로 고치고, 넣은 값(자리 맞추기 · 미디어 이름 바꾸기 뒤)을 원본(B)으로 남긴다 (DU1)
    {
        let notes: Vec<(i64, String, String)> = {
            let mut st = tx.prepare("SELECT id, fields, tags FROM note WHERE import_batch_id = ?1").map_err(err)?;
            let rows = st.query_map([batch], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(err)?;
            rows.collect::<Result<_, _>>().map_err(err)?
        };
        for (id, fields, tags) in notes {
            let old: Vec<String> = serde_json::from_str(&fields).map_err(err)?;
            let new: Vec<String> = old.iter().map(|f| rename_refs(f, &renamed)).collect();
            tx.execute(
                "UPDATE note SET fields = ?2, origin = ?3 WHERE id = ?1",
                params![id, serde_json::to_string(&new).map_err(err)?, deck_update::note_origin(&new, &tags)],
            )
            .map_err(err)?;
        }
    }
    if clashes > 0 {
        result.issues.push(issue("info", "media_renamed", format!("이름은 같은데 내용이 다른 미디어 {}개는 기존 파일을 그대로 두고 새 이름으로 저장했어요", n(clashes))));
    }
    if fixed > 0 {
        result.issues.push(issue("info", "media_renamed", format!("Windows 에서 쓸 수 없는 이름의 미디어 {}개는 이름을 바꿔 저장했어요", n(fixed))));
    }

    let roots = roots_of(&tx, &used)?;
    result.decks = roots.len();
    // AU5 소리는 재생한다 (I28 경고는 없앴다). 쓰는 소리 파일이 미디어 폴더에 없는 노트만 알린다
    let missing_sound = {
        let mut st = pkg.col.prepare("SELECT id, flds FROM notes WHERE flds LIKE '%[sound:%'").map_err(err)?;
        let notes = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?;
        let gone = |s: &str| safe_media_name(s).is_err() || !dir.join(renamed.get(s).map_or(s, String::as_str)).exists();
        notes.iter().filter(|(id, flds)| picked_note(*id) && crate::render::sound_names(flds).any(gone)).count()
    };
    if missing_sound > 0 {
        result.issues.push(issue("warning", "missing_sound", format!("소리 파일이 빠진 노트 {}개 · 학습할 때 그 자리에 \"소리 파일을 찾을 수 없어요\"로 보여요", n(missing_sound))));
    }
    result.subdecks = decks.created.iter().filter(|id| !roots.contains(id)).count();
    result.root_deck_id = (roots.len() == 1).then(|| *roots.iter().next().unwrap());
    result.roots = roots.iter().copied().collect();
    // 기록도 고른 카드 것만 (덱을 고르지 않았으면 지운 카드의 기록까지 전부)
    let cards_of = chosen.as_ref().map(|c| &c.cards);
    let ords_of: HashMap<i64, HashMap<u32, u32>> = remap.iter().filter_map(|(m, r)| Some((*m, r.2.clone()?))).collect();
    result.reviews = if with_history {
        import_revlog(&tx, &pkg.col, cards_of, &ords_of)?
    } else {
        let mut st = pkg.col.prepare("SELECT cid FROM revlog").map_err(err)?;
        let cids = st.query_map([], |r| r.get::<_, i64>(0)).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?;
        cids.into_iter().filter(|cid| cards_of.is_none_or(|c| c.contains(cid))).count()
    };
    result.filtered_decks = pkg.decks.values().filter(|d| d.filtered).count();
    for d in &degraded {
        // AU23 "이 기기에 프랑스어 음성이 없어 읽어주기 N장은 소리 없이 보여요" (참고. 확인이 필요한 카드에 세지 않는다)
        if let Some(lang) = d.feature.strip_prefix("읽어주기(").and_then(|l| l.strip_suffix(')')) {
            result.issues.push(issue("info", "tts_no_voice", format!("이 기기에 {lang} 음성이 없어 읽어주기 {}장은 소리 없이 보여요.", n(d.cards))));
            continue;
        }
        result.check_cards += d.cards;
        let what = match d.feature.as_str() {
            "빈칸 입력칸" => "써 넣은 답을 그 빈칸이 아니라 본문 전체와 비교해요. 맞게 써도 틀린 것처럼 보일 수 있어요.",
            _ => "스크립트는 실행하지 않아요. 스크립트로 바뀌는 부분은 보이지 않아요.",
        };
        result.issues.push(issue(
            "warning",
            "degraded_template",
            format!(
                "노트타입 '{}'이 {}{} 써요. {what} ({}장)",
                d.note_types.join("', '"),
                d.feature,
                if crate::quality::has_batchim(&d.feature) { "을" } else { "를" },
                n(d.cards)
            ),
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

/// 처음 보는 Anki 노트타입을 그대로 넣는다. 원본(B)도 함께 남긴다 (DU1, 덱 업데이트가 다음 버전과 비교한다).
/// deck_of: Anki 덱 id → (이번에 가져온 우리 덱, 패키지의 덱 경로). 카드 종류의 덱(deck override)은 그 덱이 있을 때만 (없으면 비워 두고
/// 요약에는 알리지 않는다. 경로를 알면 편집기가 "지운 덱"으로 보인다, NT76 · NT11)
pub(super) type DeckOf<'a> = &'a dyn Fn(i64) -> Res<(Option<i64>, Option<String>)>;
fn insert_type(tx: &Connection, aid: i64, t: &AType, now: i64, deck_of: DeckOf) -> Res<i64> {
    tx.execute(
        "INSERT INTO note_type (name, fields, css, created_at, updated_at, is_supported, unsupported_note, anki_model_id, anki_stock_kind, origin)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            t.name,
            serde_json::to_string(&t.fields).map_err(err)?,
            t.css,
            now,
            t.degraded.is_empty() as i64,
            (!t.degraded.is_empty()).then(|| t.degraded.join(", ")),
            aid,
            (t.stock > 0).then_some(t.stock),
            deck_update::new_type_origin(t)
        ],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    for (ord, name, q, a) in &t.templates {
        let (deck, deck_name) = match t.decks.iter().find(|(o, _)| o == ord) {
            Some((_, did)) => deck_of(*did)?,
            None => (None, None),
        };
        let deck_name = match deck {
            Some(d) => crate::notetype::deck_path(tx, d)?.or(deck_name),
            None => deck_name,
        };
        tx.execute(
            "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt, deck_id, deck_name) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, ord, name, q, a, deck, deck_name],
        )
        .map_err(err)?;
    }
    Ok(id)
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
/// ords_of: 다시 맞춘 유형(모델 id)의 Anki ord -> 우리 ord. 없는 ord 는 이 앱에서 지운 카드 종류라 카드를 찾지 않는다 (Q2-08)
fn import_revlog(tx: &Connection, col: &Connection, only: Option<&HashSet<i64>>, ords_of: &HashMap<i64, HashMap<u32, u32>>) -> Res<usize> {
    let mut card_by_anki: HashMap<i64, i64> = {
        let mut st = tx.prepare("SELECT anki_card_id, id FROM card WHERE anki_card_id IS NOT NULL").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    {
        let mut ours = tx
            .prepare("SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = ?1 AND c.template_ord = ?2")
            .map_err(err)?;
        let mut st = col.prepare("SELECT c.id, c.ord, n.guid, n.mid FROM cards c JOIN notes n ON n.id = c.nid").map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))).map_err(err)?;
        for row in rows {
            let (cid, ord, guid, mid) = row.map_err(err)?;
            if card_by_anki.contains_key(&cid) {
                continue;
            }
            let ord = match ords_of.get(&mid) {
                Some(m) => match m.get(&(ord as u32)) {
                    Some(o) => *o as i64,
                    None => continue,
                },
                None => ord,
            };
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
    let text = std::fs::read_to_string(p).map_err(|_| "UTF-8 텍스트로 읽을 수 없는 파일이에요.".to_string())?;
    // 메모장 · PowerShell 이 붙이는 UTF-8 BOM 을 뗀다 (T2-09). 화면도 이 글자를 JSON.parse 한다 (건너뛴 항목 복사)
    Ok(text.strip_prefix('\u{feff}').map(String::from).unwrap_or(text))
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
    fn tts_note_only_for_languages_without_voice() {
        let t = "{{tts en_US:Front}}{{tts zh_CN:Back}}<br>{{tts zh_TW speed=0.8:Back}}";
        assert_eq!(degraded_features(t, &["en".into(), "ko".into()]), ["읽어주기(중국어)"], "언어마다 한 번");
        assert!(degraded_features(t, &["en".into(), "zh".into()]).is_empty(), "OS 음성이 있으면 알리지 않는다");
    }

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
    fn json_list_and_choice() {
        let names = |nt: &str| serde_json::from_str::<Vec<String>>(if nt == recall::LIST { recall::LIST_FIELDS_JSON } else { recall::CHOICE_FIELDS_JSON }).unwrap();
        // 목록: items 키로 알아본다. 항목은 글자라 이스케이프한다. 저장한 필드를 학습 화면이 그대로 읽는다
        let (cs, iss, _) = parse_json(r#"[{"question":"OSI 하위 계층은?","items":["물리"," a<b ","  "],"ordered":false,"extra":"설명","tags":["x"]}]"#);
        assert!(iss.is_empty());
        assert_eq!((cs[0].kind, cs[0].front.as_str(), cs[0].tags.as_str()), (recall::LIST, "OSI 하위 계층은?", "x"));
        assert_eq!(cs[0].fields, ["OSI 하위 계층은?", "물리<br>a&lt;b", "순서 없이", "설명"]);
        let read = recall::special(recall::LIST, &names(recall::LIST), &serde_json::to_string(&cs[0].fields).unwrap());
        assert!(matches!(read, Some(recall::Special::List { ordered: false, ref items, .. }) if items == &["물리", "a<b"]));
        assert_eq!(parse_json(r#"{"note_type":"list","cards":[{"question":"q","items":["a","b"]}]}"#).0[0].fields[2], "순서대로", "ordered 기본은 순서대로");

        // 선택형: 정답은 1부터. 하나면 숫자만. 섞기 기본 켬
        let (cs, iss, _) = parse_json(r#"[{"question":"3-way 두 번째는?","options":["SYN","SYN-ACK","ACK"],"correct":2}]"#);
        assert!(iss.is_empty());
        assert_eq!(cs[0].kind, recall::CHOICE);
        assert_eq!((&cs[0].fields[1..4], cs[0].fields[7].as_str(), cs[0].fields[8].as_str()), (&["SYN".to_string(), "SYN-ACK".into(), "ACK".into()][..], "2", "섞기"));
        let f = &parse_json(r#"[{"note_type":"choice","question":"q","options":["a","","b","c"],"correct":[1,3],"shuffle":false}]"#).0[0].fields;
        assert_eq!((f[7].as_str(), f[8].as_str()), ("1,2", "섞지 않음"), "빈 보기는 당기고 정답 번호도 따라간다 (앱과 같다)");
        let read = recall::special(recall::CHOICE, &names(recall::CHOICE), &serde_json::to_string(f).unwrap());
        assert!(matches!(read, Some(recall::Special::Choice { shuffle: false, ref correct, .. }) if correct == &[1, 2]));
        // answer (Q26): 보기 글자로. 앞뒤 공백만 무시하고 같은 1부터 정답 칸에 저장한다
        let f = &parse_json(r#"[{"question":"q","options":["SYN"," SYN-ACK ","ACK"],"answer":"SYN-ACK "}]"#).0[0].fields;
        assert_eq!(f[7], "2");
        let f = &parse_json(r#"[{"question":"q","options":["a","","b","c"],"answer":["a","b"]}]"#).0[0].fields;
        assert_eq!(f[7], "1,2", "빈 보기를 당겨도 correct 와 같다");
        assert!(parse_json(r#"[{"question":"q","options":["a","b"],"answer":"c"}]"#).1[0].detail.contains("'c'"), "어느 글자인지 알린다");

        // 잘못된 항목은 그것만 건너뛰고 번호와 이유를 알린다
        for (card, kind) in [
            (r#"{"items":["a","b"]}"#, "missing_field"),
            (r#"{"question":5,"items":["a","b"]}"#, "bad_field"),
            (r#"{"question":"<br>","items":["a","b"]}"#, "invalid"),
            (r#"{"note_type":"list","question":"q"}"#, "missing_field"),
            (r#"{"question":"q","items":"a b"}"#, "bad_field"),
            (r#"{"question":"q","items":["a",2]}"#, "bad_field"),
            (r#"{"question":"q","items":["a"," "]}"#, "invalid"),
            (r#"{"question":"q","items":["a","b"],"ordered":"yes"}"#, "bad_field"),
            (r#"{"question":"q","items":["a","b"],"extra":1}"#, "bad_field"),
            (r#"{"question":"q","options":["a"],"correct":1}"#, "invalid"),
            (r#"{"question":"q","options":["1","2","3","4","5","6","7"],"correct":1}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"]}"#, "missing_field"),
            (r#"{"question":"q","options":["a","b"],"correct":[]}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"],"correct":0}"#, "bad_field"),
            (r#"{"question":"q","options":["a","b"],"correct":"1"}"#, "bad_field"),
            (r#"{"question":"q","options":["a","b"],"correct":[1,3]}"#, "invalid"),
            (r#"{"question":"q","options":["a","","b"],"correct":2}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"],"correct":1,"shuffle":0}"#, "bad_field"),
            (r#"{"question":"q","options":["a","b"],"correct":1,"answer":"a"}"#, "bad_field"),
            (r#"{"question":"q","options":["a","b"],"answer":"c"}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"],"answer":["a","A"]}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"],"answer":""}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"," a"],"answer":"a"}"#, "invalid"),
            (r#"{"question":"q","options":["a","b"],"answer":1}"#, "bad_field"),
        ] {
            let (cs, iss, err) = parse_json(&format!(r#"[{{"front":"a","back":"b"}},{card}]"#));
            assert_eq!((cs.len(), err), (1, None), "{card}");
            assert_eq!((iss.len(), iss[0].index, iss[0].severity, iss[0].kind), (1, Some(2), "skipped", kind), "{card}: {}", iss[0].detail);
        }
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

    const LEGACY_TABLES: &str = "CREATE TABLE col (id integer primary key, crt integer, mod integer, scm integer, ver integer, dty integer, usn integer, ls integer,
                           conf text, models text, decks text, dconf text, tags text);
         CREATE TABLE notes (id integer primary key, guid text, mid integer, mod integer, usn integer, tags text, flds text, sfld text,
                             csum integer, flags integer, data text);
         CREATE TABLE cards (id integer primary key, nid integer, did integer, ord integer, mod integer, usn integer, type integer,
                             queue integer, due integer, ivl integer, factor integer, reps integer, lapses integer, left integer,
                             odue integer, odid integer, flags integer, data text);
         CREATE TABLE revlog (id integer primary key, cid integer, usn integer, ease integer, ivl integer, lastIvl integer,
                              factor integer, time integer, type integer);";

    fn write_zip(path: &Path, entries: &[(String, Vec<u8>)]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            zip.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    /// 구버전 .apkg: 덱 D 에 Basic 노트 (guid, 앞면) 마다 카드 한 장 (노트 · 카드 id 는 first_id 부터). 미디어는 (이름, 내용)
    fn media_package(path: &Path, first_id: i64, notes: &[(&str, &str)], media: &[(&str, &[u8])]) {
        let db = path.with_extension("anki2");
        let _ = std::fs::remove_file(&db);
        let col = Connection::open(&db).unwrap();
        col.execute_batch(LEGACY_TABLES).unwrap();
        let models = r#"{"7": {"name": "Basic", "css": "", "flds": [{"name": "Front"}, {"name": "Back"}],
                         "tmpls": [{"ord": 0, "name": "Card 1", "qfmt": "{{Front}}", "afmt": "{{Back}}"}]}}"#;
        let decks = r#"{"1": {"name": "D", "dyn": 0}}"#;
        col.execute("INSERT INTO col VALUES (1, 0, 0, 0, 11, 0, 0, 0, '{}', ?1, ?2, '{}', '{}')", [models, decks]).unwrap();
        col.execute_batch("BEGIN").unwrap();
        for (i, (guid, front)) in notes.iter().enumerate() {
            let id = first_id + i as i64;
            col.execute("INSERT INTO notes VALUES (?1, ?2, 7, 0, 0, '', ?3, '', 0, 0, '')", params![id, guid, format!("{front}\x1f")]).unwrap();
            col.execute("INSERT INTO cards VALUES (?1, ?1, 1, 0, 0, 0, 0, 0, ?1, 0, 0, 0, 0, 0, 0, 0, 0, '')", [id]).unwrap();
        }
        col.execute_batch("COMMIT").unwrap();
        drop(col);
        let list: serde_json::Map<String, Value> = media.iter().enumerate().map(|(i, (n, _))| (i.to_string(), Value::from(*n))).collect();
        let mut entries = vec![("collection.anki2".to_string(), std::fs::read(&db).unwrap()), ("media".into(), serde_json::to_vec(&list).unwrap())];
        entries.extend(media.iter().enumerate().map(|(i, (_, b))| (i.to_string(), b.to_vec())));
        write_zip(path, &entries);
        let _ = std::fs::remove_file(db);
    }

    /// 구버전(스키마 11) .apkg. 덱 A::1 · A::2 · B 와 필터 덱 F (A::1 의 카드 하나가 올라가 있다).
    /// 노트마다 카드 한 장 · 미디어 하나 · 기록 하나
    fn two_deck_package(path: &Path) {
        let db = path.with_extension("anki2");
        let col = Connection::open(&db).unwrap();
        col.execute_batch(LEGACY_TABLES).unwrap();
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
    fn reimport_into_existing_cloze_type_keeps_new_cards() {
        // 같은 Anki 모델 id 의 빈칸 유형에 새 노트를 다시 가져온다 (같은 컬렉션에서 내보낸 두 번째 덱). 빈칸 카드 ord 는 빈칸 번호라 카드 종류로 맞추지 않는다
        use deck_update::testpkg::{self, Model, note};
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-czre-{}-{}", std::process::id(), now_ms())));
        let s = state(&tmp.0.join("data"));
        let model = || Model { id: 8, name: "Cloze", fields: vec!["Text", "Back Extra"], templates: vec![("Cloze", "{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}")], css: "" };
        let (p1, p2) = (tmp.0.join("a.apkg"), tmp.0.join("b.apkg"));
        testpkg::write(&p1, &[model()], &[(1, "A")], &[note(1, "g1", 8, &["{{c1::a}} {{c2::b}}", ""], &[(1, 0), (1, 1)])], &[]);
        testpkg::write(&p2, &[model()], &[(2, "B")], &[note(2, "g2", 8, &["{{c1::x}} {{c2::y}}", ""], &[(2, 0), (2, 1)])], &[]);
        run_import_anki(&|_, _, _| {}, &s, p1.to_str().unwrap(), false, &mut vec![]).unwrap();
        let r = run_import_anki(&|_, _, _| {}, &s, p2.to_str().unwrap(), true, &mut vec![]).unwrap();
        assert_eq!((r.notes, r.cards, r.skipped, r.reviews), (1, 2, 0, 2), "{:?}", r.issues.iter().map(|i| &i.detail).collect::<Vec<_>>());
        let con = s.db.lock().unwrap();
        let linked: i64 = con.query_row("SELECT count(*) FROM review_log WHERE card_id IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(linked, 2, "기록도 그 빈칸 카드에 붙는다");
    }

    #[test]
    fn selective_import() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-b2s-{}-{}", std::process::id(), now_ms())));
        let s = state(&tmp.0.join("data"));
        let pkg_path = tmp.0.join("two.apkg");
        two_deck_package(&pkg_path);

        // 덱 고르기 트리: 실제 덱마다 자기 카드. 필터 덱의 카드는 원래 덱(A::1)으로 센다
        let pkg = open_package(&pkg_path, &s.data_dir, &[]).unwrap();
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
        let r = run_import_selected(&|_, _, _| {}, &s, pkg_path.to_str().unwrap(), true, Some(&picked), &[], &mut vec![]).unwrap();
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
        let r = run_import_selected(&|_, _, _| {}, &s, pkg_path.to_str().unwrap(), false, Some(&HashSet::from([11, 20])), &[], &mut vec![]).unwrap();
        assert_eq!((r.notes, r.cards, r.media), (1, 1, 1));
        assert!(r.issues.iter().any(|i| i.kind == "duplicate"));
    }

    fn files(dir: &Path) -> BTreeSet<String> {
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect()
    }

    #[test]
    fn media_name_clash_keeps_both_files() {
        // T2-01 · T7-02: 같은 이름 다른 내용이면 기존 파일은 두고 내용 해시를 붙인 새 이름으로 저장, 새 노트의 참조를 고친다 (Anki 와 같다)
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-clash-{}-{}", std::process::id(), now_ms())));
        let s = state(&tmp.0.join("data"));
        let import = |file: &str, id: i64, notes: &[(&str, &str)], media: &[(&str, &[u8])]| {
            let p = tmp.0.join(file);
            media_package(&p, id, notes, media);
            run_import_anki(&|_, _, _| {}, &s, p.to_str().unwrap(), false, &mut vec![]).unwrap()
        };
        let fields = |guid: &str| -> String { s.db.lock().unwrap().query_row("SELECT fields FROM note WHERE anki_guid = ?1", [guid], |r| r.get(0)).unwrap() };
        let renamed = |r: &ImportResult| r.issues.iter().filter(|i| i.kind == "media_renamed").map(|i| i.detail.clone()).collect::<Vec<_>>();
        let media = s.data_dir.join("media");

        let r = import("a.apkg", 1, &[("ga", r#"<img src="heart.png">"#)], &[("heart.png", b"AAAA")]);
        assert_eq!(r.media, 1);
        let b = [("gb", r#"<img src="heart.png"> [sound:heart.png] <img src='heart.png'>"#)];
        let r = import("b.apkg", 100, &b, &[("heart.png", b"BBBBBB")]);
        let new = files(&media).into_iter().find(|n| n != "heart.png").unwrap();
        assert!(new.starts_with("heart-") && new.ends_with(".png") && new.len() == "heart-12345678.png".len(), "{new}");
        assert_eq!(std::fs::read(media.join("heart.png")).unwrap(), b"AAAA", "다른 덱의 그림을 덮어 보이지 않는다");
        assert_eq!(std::fs::read(media.join(&new)).unwrap(), b"BBBBBB", "가져온 그림이 저장된다");
        assert_eq!(fields("ga"), r#"["<img src=\"heart.png\">",""]"#, "이미 있던 노트는 그대로");
        assert_eq!(fields("gb"), format!(r#"["<img src=\"{new}\"> [sound:{new}] <img src='{new}'>",""]"#));
        assert_eq!(r.media, 1);
        assert_eq!(renamed(&r), ["이름은 같은데 내용이 다른 미디어 1개는 기존 파일을 그대로 두고 새 이름으로 저장했어요"]);
        assert!(!r.issues.iter().any(|i| i.kind == "missing_sound"), "바꾼 이름으로 소리 파일을 찾는다");
        let rows: Vec<String> = {
            let con = s.db.lock().unwrap();
            con.prepare("SELECT filename FROM media ORDER BY filename").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(rows, [new.clone(), "heart.png".into()]);

        // 같은 파일을 다시: 새로 쓰는 것도, 세는 것도 없다 (T2-17)
        let r = import("b.apkg", 100, &b, &[("heart.png", b"BBBBBB")]);
        assert_eq!((r.notes, r.media, renamed(&r).len(), files(&media).len()), (0, 0, 0, 2));
        // 내용이 같으면 그 파일을 그대로 쓴다
        let r = import("c.apkg", 200, &[("gc", r#"<img src="heart.png">"#)], &[("heart.png", b"AAAA")]);
        assert_eq!((r.notes, r.media, renamed(&r).len(), files(&media).len()), (1, 0, 0, 2));
        assert_eq!(fields("gc"), r#"["<img src=\"heart.png\">",""]"#);
    }

    #[test]
    fn windows_unsafe_media_names_are_renamed() {
        // T7-06 · T7-09: Windows 가 저장하지 못하는 이름 하나로 덱 전체가 실패하지 않는다. 안전한 이름으로 바꾸고 참조를 고친다
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-winname-{}-{}", std::process::id(), now_ms())));
        let s = state(&tmp.0.join("data"));
        let long = format!("{}.png", "가".repeat(300));
        let media: [(&str, &[u8]); 10] = [
            ("q?.png", b"1"),
            ("a<b>.png", b"2"),
            ("bell\u{7}.mp3", b"3"),
            ("con.png", b"4"),
            ("COM1", b"5"),
            ("dot. ", b"6"),
            (&long, b"7"),
            ("nul\0.png", b"8"),
            ("x:ads.png", b"9"),
            ("ok.png", b"10"),
        ];
        let front = format!(
            r#"<img src="q?.png"><img src="a&lt;b&gt;.png">[sound:bell{}.mp3]<img src="con.png">[sound:COM1]<img src="{long}"><img src="ok.png">"#,
            '\u{7}'
        );
        let p = tmp.0.join("w.apkg");
        media_package(&p, 1, &[("g", &front)], &media);
        let r = run_import_anki(&|_, _, _| {}, &s, p.to_str().unwrap(), false, &mut vec![]).unwrap();
        let short = format!("{}.png", "가".repeat(196));
        let want: BTreeSet<String> =
            ["_COM1", "_con.png", "a_b_.png", "bell_.mp3", "dot", "nul_.png", "ok.png", "q_.png", short.as_str()].into_iter().map(String::from).collect();
        assert_eq!(files(&s.data_dir.join("media")), want);
        assert_eq!(r.media, 9, "x:ads.png 만 건너뛴다 (경로 · ADS 이름은 지금처럼)");
        assert!(r.issues.iter().any(|i| i.kind == "media_name" && i.detail.contains("x:ads.png")));
        assert!(
            r.issues.iter().any(|i| i.detail == "Windows 에서 쓸 수 없는 이름의 미디어 8개는 이름을 바꿔 저장했어요"),
            "{:?}",
            r.issues.iter().map(|i| &i.detail).collect::<Vec<_>>()
        );
        let fields: String = s.db.lock().unwrap().query_row("SELECT fields FROM note", [], |r| r.get(0)).unwrap();
        let fields: Vec<String> = serde_json::from_str(&fields).unwrap();
        assert_eq!(
            fields[0],
            format!(r#"<img src="q_.png"><img src="a_b_.png">[sound:bell_.mp3]<img src="_con.png">[sound:_COM1]<img src="{short}"><img src="ok.png">"#)
        );
    }

    #[test]
    fn storable_media_names() {
        for (name, want) in [
            ("ok.png", "ok.png"),
            ("q?.png", "q_.png"),
            ("a<b>|*\".png", "a_b____.png"),
            ("bell\u{7}.mp3", "bell_.mp3"),
            ("n\0.png", "n_.png"),
            ("\u{202e}gnp.exe", "_gnp.exe"),
            ("con.png", "_con.png"),
            ("CON", "_CON"),
            ("Lpt9.txt.gz", "_Lpt9.txt.gz"),
            ("com¹.png", "_com¹.png"),
            ("conout$", "_conout$"),
            ("nul .txt", "_nul .txt"),
            ("COM10.png", "COM10.png"),
            ("console.png", "console.png"),
            ("dot.", "dot"),
            ("sp . ", "sp"),
            ("   ", "_"),
        ] {
            let got = storable_name(name);
            assert_eq!(got, want, "{name:?}");
            assert!(safe_media_name(&got).is_ok());
        }
        assert!(matches!(storable_name("ok.png"), Cow::Borrowed(_)));
        let long = format!("{}.png", "가".repeat(300));
        assert_eq!(storable_name(&long), format!("{}.png", "가".repeat(196)), "확장자는 지키고 앞을 줄인다");
        assert_eq!(storable_name(&"😀".repeat(150)).encode_utf16().count(), MAX_NAME, "Windows 는 UTF-16 으로 센다");
        assert_eq!(fit_name("a.png", "-1234abcd"), "a-1234abcd.png");
        assert_eq!(fit_name("noext", "-1234abcd"), "noext-1234abcd");
        assert_eq!(fit_name(&long, "-1234abcd"), format!("{}-1234abcd.png", "가".repeat(187)));
    }

    #[test]
    fn rename_media_refs() {
        let map = HashMap::from([("a b.png".to_string(), "x.png".to_string()), ("q&<.mp3".into(), "q__.mp3".into()), ("t&.png".into(), "t&-1.png".into())]);
        assert_eq!(
            rename_refs(r#"<img src="a b.png"><img src='a b.png'>[sound:q&amp;&lt;.mp3]<img src=other.png><img src=a b.png>"#, &map),
            r#"<img src="x.png"><img src='x.png'>[sound:q__.mp3]<img src=other.png><img src=a b.png>"#
        );
        assert_eq!(
            rename_refs(r#"<img src="t&amp;.png"><img src="t&.png">"#, &map),
            r#"<img src="t&amp;-1.png"><img src="t&-1.png">"#,
            "엔티티로 적힌 이름은 새 이름도 엔티티로"
        );
        assert_eq!(rename_refs("a b.png 는 글자 그대로 [sound:a b.png", &map), "a b.png 는 글자 그대로 [sound:a b.png");
    }

    #[test]
    fn broken_or_huge_package_leaves_no_temp_file() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-bomb-{}-{}", std::process::id(), now_ms())));
        let work = tmp.0.join("data");
        std::fs::create_dir_all(&work).unwrap();
        let temps = || files(&work).iter().filter(|n| n.starts_with("import-")).count();
        let p = tmp.0.join("x.apkg");
        // T7-05: SQLite 머리만 맞고 나머지가 깨진 컬렉션. 연결 · 첫 쿼리에서 실패해도 임시 컬렉션을 지운다
        let mut bad = b"SQLite format 3\0".to_vec();
        bad.extend([7u8; 4096]);
        write_zip(&p, &[("collection.anki2".into(), bad)]);
        assert!(open_package(&p, &work, &[]).is_err());
        assert_eq!(temps(), 0);
        // T7-04: zstd 압축 폭탄 (QA: 99KB -> 3GB). 상한까지 가지 않고 압축률에서 멈추고, 쓰던 임시 파일도 지운다
        let zeros = zstd::stream::encode_all(std::io::repeat(0).take(RATIO_FROM + (1 << 20)), 1).unwrap();
        write_zip(&p, &[("collection.anki21b".into(), zeros)]);
        let e = open_package(&p, &work, &[]).err().unwrap();
        assert!(e.starts_with("컬렉션의 압축을 풀면 너무 커져서 멈췄어요"), "{e}");
        assert_eq!(temps(), 0);
        // 항목 하나의 상한
        write_zip(&p, &[("m".into(), vec![1u8; 3 << 20])]);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&p).unwrap()).unwrap();
        assert!(unpack(&mut zip, "m", "미디어", 2 << 20, &mut vec![]).unwrap_err().contains("최대 2 MB"));
        assert_eq!(unpack(&mut zip, "m", "미디어", 3 << 20, &mut vec![]).unwrap(), 3 << 20);
        // 지난 실행이 남긴 임시 컬렉션은 앱을 열 때 지운다
        for f in ["import-1.sqlite", "import-1.sqlite-wal", "neoanki.sqlite"] {
            std::fs::write(work.join(f), b"x").unwrap();
        }
        clean_stale(&work);
        assert_eq!(files(&work).into_iter().collect::<Vec<_>>(), ["neoanki.sqlite"]);
    }

    #[test]
    fn guid_lookup_uses_index_and_bom_json_parses() {
        // T2-11 · T7-08: 노트마다 하는 guid 조회가 표 전체를 훑지 않는다
        let con = db::open_in_memory().unwrap();
        for sql in [
            "SELECT count(*) FROM note WHERE anki_guid = 'g'",
            "SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = 'g' AND c.template_ord = 0",
        ] {
            let plan: Vec<String> =
                con.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap().query_map([], |r| r.get(3)).unwrap().map(Result::unwrap).collect();
            assert!(plan.iter().any(|p| p.contains("idx_note_guid")), "{plan:?}");
        }
        // T2-09: BOM 이 붙은 JSON 파일 (메모장 · PowerShell)
        let p = std::env::temp_dir().join(format!("neoanki-bom-{}-{}.json", std::process::id(), now_ms()));
        std::fs::write(&p, "\u{feff}[{\"front\":\"a\",\"back\":\"b\"}]").unwrap();
        let text = read_json_file(p.to_string_lossy().into_owned()).unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(text.starts_with('[') && parse_json(&text).2.is_none());
    }

    /// T2-11 · T7-08 측정: 노트 2만 개를 빈 DB 에, 2만 개가 있는 DB 에 다른 2만 개, 같은 파일 다시. 색인을 지운 DB(before)와 비교한다.
    /// cargo test --lib bench_import_20k -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_import_20k() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-bench-{}-{}", std::process::id(), now_ms())));
        std::fs::create_dir_all(&tmp.0).unwrap();
        let pkg = |tag: &str, first: i64| {
            let notes: Vec<(String, String)> = (0..20_000).map(|i| (format!("{tag}{i}"), format!("{tag} 앞면 {i}"))).collect();
            let refs: Vec<(&str, &str)> = notes.iter().map(|(g, f)| (g.as_str(), f.as_str())).collect();
            let p = tmp.0.join(format!("{tag}.apkg"));
            media_package(&p, first, &refs, &[]);
            p
        };
        let (a, b) = (pkg("a", 1_000_000), pkg("b", 2_000_000));
        for index in [false, true] {
            let s = state(&tmp.0.join(format!("data-{index}")));
            if !index {
                s.db.lock().unwrap().execute_batch("DROP INDEX idx_note_guid").unwrap();
            }
            let run = |p: &Path| {
                let t = std::time::Instant::now();
                let r = run_import_anki(&|_, _, _| {}, &s, p.to_str().unwrap(), true, &mut vec![]).unwrap();
                (t.elapsed().as_secs_f64(), r.notes)
            };
            let (first, second, again) = (run(&a), run(&b), run(&a));
            eprintln!(
                "색인 {}: 빈 DB 에 2만 {:.2}초 · 2만 있는 DB 에 다른 2만 {:.2}초 · 같은 파일 다시(전부 중복) {:.2}초",
                if index { "있음(after)" } else { "없음(before)" },
                first.0,
                second.0,
                again.0
            );
            assert_eq!((first.1, second.1, again.1), (20_000, 20_000, 0));
        }
    }
}
