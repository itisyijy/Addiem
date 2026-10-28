//! I · 원자료 (post-mvp B-1 · B-2). 덱에 강의 자료(PDF · PPTX)를 붙이고 강조된 구간을 카드와 잇는다.
//! PDF 는 화면(pdf.js)이 읽어 구간을 보내고, PPTX 는 여기서 XML 을 직접 읽는다.
//! 원본은 <data_dir>/sources/ 로 복사해 두고 고치지 않는다. 자료를 지워도 카드는 그대로 (연결만 끊긴다)

use crate::commands::{answer_part, descendants, load_decks, note_field_names, render_fields, AppState};
use crate::db::now_ms;
use crate::{embed, exam, render};
use quick_xml::events::Event;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 신호와 가중치 (post-mvp 5장). 하이라이트·밑줄·색은 사용자가 일부러 남긴 것이라 높고,
/// 볼드는 원본 서식일 수 있어 낮다. 배열 순서가 곧 신뢰도 순이다 (I1 배지 순서).
/// figure · table: 8.4 그림 · 표로 만든 카드의 영역 (IT9). 강조가 아니라 가중치 0, 빠진 범위 · 분석에서 뺀다.
/// heading: 8.4 목차 제목 (OT1). 강조와 따로 둔 행이고 가중치 0 (동기화로 받은 옛 앱이 이 행에 카드를 자동으로 이어도 점수가 늘지 않는다)
pub const SIGNALS: [(&str, f64); 8] =
    [("highlight", 3.0), ("underline", 3.0), ("red", 3.0), ("star", 2.0), ("bold", 1.0), ("figure", 0.0), ("table", 0.0), ("heading", 0.0)];
const SIGNAL_LABEL: [&str; 8] = ["하이라이트", "밑줄", "붉은 글씨", "★", "볼드", "그림", "표", "목차"];
const HEADING: &str = "heading";

/// 자동 연결 기준 (multilingual-e5-small 코사인 유사도). 이보다 낮으면 잇지 않고 비워 둔다 (억지 연결 금지, post-mvp 6장).
/// 실측 (2026-10-14, 한국어 카드 20장 · 강조 구간 12곳): 맞는 짝 0.89~0.99, 틀린 짝 0.85~0.875 (짧은 제목에 끌림).
/// ponytail: 고정값. 자료가 쌓이면 덱별 분포로 조정한다
pub const AUTO_LINK: f32 = 0.88;

/// 알려진 신호만 신뢰도 순으로
fn sorted_signals<S: AsRef<str>>(signals: &[S]) -> Vec<&'static str> {
    SIGNALS.iter().map(|s| s.0).filter(|s| signals.iter().any(|x| x.as_ref() == *s)).collect()
}

/// 8.4 그림 · 표 영역 구간인가
fn is_region<S: AsRef<str>>(signals: &[S]) -> bool {
    signals.iter().any(|s| matches!(s.as_ref(), "figure" | "table"))
}

/// 8.4 목차 제목 구간인가
fn is_heading<S: AsRef<str>>(signals: &[S]) -> bool {
    signals.iter().any(|s| s.as_ref() == HEADING)
}

fn weight_of(signals: &[&str]) -> f64 {
    SIGNALS.iter().filter(|s| signals.contains(&s.0)).map(|s| s.1).sum()
}

// ------------------------------------------------------------------ 목록 · 추가 · 지우기

#[derive(Serialize)]
pub struct Material {
    id: i64,
    deck_id: i64,
    file_name: String,
    kind: String,
    pages: i64,
    /// pending: PDF 를 아직 읽지 않음 / ready / unsupported: 글자를 읽을 수 없음
    status: String,
    segments: i64,
    /// 카드가 이어진 구간 수
    linked: i64,
    /// R1-a 빠진 범위: 강조됐는데 이어진 카드가 없는 구간 수 (끊은 자동 연결은 없는 것으로 본다)
    pub(crate) gaps: i64,
    /// 8.4 목차 제목 수
    headings: i64,
}

// 강조 수 · 연결 수 · 빠진 범위는 글자 강조만 센다. 그림 · 표 영역(IT13)은 [그림·표 N] 탭에서만, 목차 제목(OT1)은 [목차 N] 탭에서만
const MATERIAL_SQL: &str = "SELECT m.id, m.deck_id, m.file_name, m.kind, m.pages, m.status,
        (SELECT count(*) FROM source_segment s WHERE s.material_id = m.id AND s.signals NOT IN ('figure','table','heading')),
        (SELECT count(DISTINCT s.id) FROM source_segment s JOIN source_link l ON l.segment_id = s.id
         WHERE s.material_id = m.id AND l.method != 'rejected' AND s.signals NOT IN ('figure','table','heading')),
        (SELECT count(*) FROM source_segment s WHERE s.material_id = m.id AND s.signals NOT IN ('figure','table','heading')
         AND NOT EXISTS (SELECT 1 FROM source_link l WHERE l.segment_id = s.id AND l.method != 'rejected')),
        (SELECT count(*) FROM source_segment s WHERE s.material_id = m.id AND s.signals = 'heading')
     FROM source_material m";

fn material_row(r: &rusqlite::Row) -> rusqlite::Result<Material> {
    Ok(Material {
        id: r.get(0)?,
        deck_id: r.get(1)?,
        file_name: r.get(2)?,
        kind: r.get(3)?,
        pages: r.get(4)?,
        status: r.get(5)?,
        segments: r.get(6)?,
        linked: r.get(7)?,
        gaps: r.get(8)?,
        headings: r.get(9)?,
    })
}

pub(crate) fn subtree(con: &Connection, deck_id: i64) -> Res<String> {
    let decks = load_decks(con)?;
    Ok(descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","))
}

/// 자료 · 연결 순서는 기기마다 다른 로컬 id 대신 동기화로 같은 값으로 (만든 시각 → uid, Q7). id 는 uid 가 비었을 때만
const MATERIAL_ORDER: &str = "m.created_at, m.uid, m.id";
const LINK_ORDER: &str = "l.created_at, l.uid, l.id";

/// 이 덱과 하위 덱에 붙은 자료
#[tauri::command(async)]
pub fn source_list(state: State<AppState>, deck_id: i64) -> Res<Vec<Material>> {
    let con = state.db.lock().map_err(err)?;
    materials_in(&con, deck_id)
}

fn materials_in(con: &Connection, deck_id: i64) -> Res<Vec<Material>> {
    let ids = subtree(con, deck_id)?;
    let mut st = con.prepare(&format!("{MATERIAL_SQL} WHERE m.deck_id IN ({ids}) ORDER BY {MATERIAL_ORDER}")).map_err(err)?;
    let rows = st.query_map([], material_row).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    Ok(rows)
}

pub(crate) fn material(con: &Connection, id: i64) -> Res<Material> {
    con.query_row(&format!("{MATERIAL_SQL} WHERE m.id = ?1"), [id], material_row).map_err(|_| "자료를 찾을 수 없어요.".to_string())
}

/// [자료 추가]. 원본은 복사만 하고 고치지 않는다. PPTX 는 바로 강조를 읽고, PDF 는 화면이 읽어 source_save_pdf 로 보낸다
#[tauri::command]
pub fn source_add(state: State<AppState>, deck_id: i64, path: String) -> Res<Material> {
    let src = Path::new(&path);
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let kind = match ext.as_str() {
        "pdf" | "pptx" => ext,
        _ => return Err("PDF나 PPTX 파일만 넣을 수 있어요.".into()),
    };
    if std::fs::metadata(src).map_err(|_| "파일을 찾을 수 없어요.".to_string())?.len() > 200 * 1024 * 1024 {
        return Err("200MB가 넘는 자료는 넣을 수 없어요.".into());
    }
    let bytes = std::fs::read(src).map_err(|_| "파일을 읽을 수 없어요.".to_string())?;
    let slides = if kind == "pptx" { Some(pptx_read(&bytes)) } else { None };
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("자료.{kind}"));
    let mut con = state.db.lock().map_err(err)?;
    if let Some(old) = same_file(&con, &state.data_dir, deck_id, &kind, &bytes)? {
        return Err(format!("이미 넣은 자료와 같은 파일이에요 ({old})."));
    }
    let tx = con.transaction().map_err(err)?;
    let now = now_ms();
    tx.execute(
        "INSERT INTO source_material (deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (?1, ?2, '', ?3, ?4, ?4)",
        params![deck_id, name, kind, now],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    // 같은 이름의 자료가 여러 개여도 겹치지 않게 복사본은 id 로 부른다
    let rel = format!("sources/{id}.{kind}");
    let dest = state.data_dir.join(&rel);
    std::fs::create_dir_all(dest.parent().unwrap_or(&state.data_dir)).map_err(err)?;
    std::fs::write(&dest, &bytes).map_err(err)?;
    tx.execute("UPDATE source_material SET rel_path = ?1 WHERE id = ?2", params![rel, id]).map_err(err)?;
    if let Some(slides) = slides {
        // 열 수 없는 PPTX 도 목록에는 남겨 "글자를 읽을 수 없어요"로 알린다
        let (slides, sections) = slides.unwrap_or_default();
        let mut segs = pptx_segments(&slides);
        segs.extend(pptx_headings(&slides, &sections)); // 8.4 목차: 강조 구간 뒤에 넣어 강조 구간 순서 · id 는 전과 같다
        let readable = slides.iter().flatten().any(|p| !p.text.is_empty());
        save_segments(&tx, id, slides.len() as i64, &segs, readable)?;
    }
    tx.commit().map_err(err)?;
    material(&con, id)
}

/// 이 덱(하위 덱 포함)에 바이트까지 같은 자료가 이미 있으면 그 이름. 같은 파일이 두 줄로 들어가 구간 · 미연결 수가 두 배가 됐다 (QA T6-07).
/// 이 기기에 복사본이 없는 자료(동기화로 목록만 받음)는 견주지 않는다
fn same_file(con: &Connection, data_dir: &Path, deck_id: i64, kind: &str, bytes: &[u8]) -> Res<Option<String>> {
    let ids = subtree(con, deck_id)?;
    let mut st = con.prepare(&format!("SELECT m.file_name, m.rel_path FROM source_material m WHERE m.deck_id IN ({ids}) AND m.kind = ?1 ORDER BY {MATERIAL_ORDER}")).map_err(err)?;
    let rows: Vec<(String, String)> = st.query_map([kind], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    let same = |rel: &str| {
        let path = data_dir.join(rel);
        crate::commands::safe_rel_path(rel, "sources")
            && path.metadata().is_ok_and(|m| m.len() == bytes.len() as u64)
            && std::fs::read(&path).is_ok_and(|b| b == bytes)
    };
    Ok(rows.into_iter().find(|(_, rel)| same(rel)).map(|(name, _)| name))
}

/// 자료를 지운다. 카드는 그대로, 연결만 사라진다 (CASCADE). 복사본 파일도 지운다
#[tauri::command]
pub fn source_delete(state: State<AppState>, material_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    let rel: Option<String> =
        con.query_row("SELECT rel_path FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).optional().map_err(err)?;
    con.execute("DELETE FROM source_material WHERE id = ?1", [material_id]).map_err(err)?;
    if let Some(rel) = rel.filter(|r| crate::commands::safe_rel_path(r, "sources")) {
        let _ = std::fs::remove_file(state.data_dir.join(rel));
    }
    Ok(())
}

/// pdf.js 가 읽을 복사본 바이트. 동기화로 자료 목록만 넘어온 기기에는 파일이 없을 수 있다
#[tauri::command]
pub fn source_bytes(state: State<AppState>, material_id: i64) -> Res<tauri::ipc::Response> {
    let rel: String = {
        let con = state.db.lock().map_err(err)?;
        con.query_row("SELECT rel_path FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).map_err(err)?
    };
    let bytes = Some(rel)
        .filter(|r| crate::commands::safe_rel_path(r, "sources"))
        .and_then(|r| std::fs::read(state.data_dir.join(r)).ok())
        .ok_or_else(|| "이 기기에는 자료 파일이 없어요.".to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

// ------------------------------------------------------------------ 강조 구간

#[derive(Deserialize)]
pub struct NewSegment {
    page: i64,
    ord: i64,
    text: String,
    signals: Vec<String>,
    rects: Option<Vec<[f64; 4]>>,
    before: Option<String>,
    after: Option<String>,
}

fn save_segments(con: &Connection, material_id: i64, pages: i64, segs: &[NewSegment], readable: bool) -> Res<()> {
    con.execute("DELETE FROM source_segment WHERE material_id = ?1", [material_id]).map_err(err)?;
    insert_segments(con, material_id, segs, false)?;
    con.execute(
        "UPDATE source_material SET pages = ?1, status = ?2, updated_at = ?3 WHERE id = ?4",
        params![pages, if readable { "ready" } else { "unsupported" }, now_ms(), material_id],
    )
    .map_err(err)?;
    Ok(())
}

/// 알 수 없는 신호뿐이거나 글이 빈 구간은 버린다. headings: 목차 제목 구간만 받는다 (목차 다시 읽기)
fn insert_segments(con: &Connection, material_id: i64, segs: &[NewSegment], headings: bool) -> Res<()> {
    let mut st = con
        .prepare(
            "INSERT INTO source_segment (material_id, page, ord, text, signals, weight, rects, before_text, after_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(err)?;
    for s in segs {
        let signals = sorted_signals(&s.signals);
        if signals.is_empty() || s.text.trim().is_empty() || (headings && signals != [HEADING]) {
            continue;
        }
        let rects = s.rects.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default());
        st.execute(params![material_id, s.page, s.ord, s.text.trim(), signals.join(","), weight_of(&signals), rects, s.before, s.after])
            .map_err(err)?;
    }
    Ok(())
}

/// 화면이 pdf.js 로 읽은 PDF 의 강조 구간. 아직 읽지 않은(pending) 자료에만 받는다 (이미 만든 연결을 지우지 않게)
#[tauri::command]
pub fn source_save_pdf(state: State<AppState>, material_id: i64, pages: i64, segments: Vec<NewSegment>, readable: bool) -> Res<Material> {
    let mut con = state.db.lock().map_err(err)?;
    let status: String =
        con.query_row("SELECT status FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).map_err(err)?;
    if status == "pending" {
        let tx = con.transaction().map_err(err)?;
        save_segments(&tx, material_id, pages, &segments, readable)?;
        tx.commit().map_err(err)?;
    }
    material(&con, material_id)
}

// ------------------------------------------------------------------ 8.4 목차 (DECISIONS OT)

/// 제목 구간의 글: 단계만큼 "#" + 띄어쓰기 + 제목 ("## 3.2 요약"). 새 열 없이 단계를 담는다 (OT2). PDF 는 화면(outline.ts headingText)이 같게 쓴다
fn heading_text(level: usize, title: &str) -> String {
    format!("{} {}", "#".repeat(level.clamp(1, 6)), title.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// "## 3.2 요약" → (2, "3.2 요약"). 표시가 없으면 1단계 그대로
fn heading_of(text: &str) -> (usize, &str) {
    let level = text.bytes().take_while(|b| *b == b'#').count();
    match text[level..].strip_prefix(' ') {
        Some(title) if (1..=6).contains(&level) => (level, title),
        _ => (1, text),
    }
}

/// 핵심 범위로 보는 제목 낱말: 하위 덱 이름 규칙(import.rs deck_name_level)과 같은 낱말 (OT4)
const CORE_WORDS: [&str; 8] = ["요약", "핵심", "정리", "기출", "필수", "summary", "core", "essential"];
/// 제목에 남긴 사용자 표시. 볼드는 제목 서식이라 뺀다
const MARKS: [&str; 4] = ["highlight", "underline", "red", "star"];
/// 목차 근거로 제안하려면 핵심 범위와 이어진 카드가 이만큼 (OT5)
const OUTLINE_MIN_CARDS: usize = 3;
const OUTLINE_HEAD: &str = "목차 · ";

#[derive(Serialize, Debug, PartialEq)]
pub struct Section {
    /// 제목 구간 id
    id: i64,
    level: usize,
    title: String,
    page: i64,
    /// 끝나는 쪽 (다음 같은 · 윗 단계 제목 앞)
    end_page: i64,
    /// 구간 안(아래 단계 포함) 강조 구간 수 · 그 구간들과 이어진 카드 수
    segments: usize,
    cards: usize,
    /// 핵심 범위면 까닭 ("제목에 '요약'" · "제목이 하이라이트로 강조돼 있어요")
    core: Option<String>,
}

#[derive(Serialize, Debug, Default)]
pub struct Outline {
    sections: Vec<Section>,
    /// 핵심 범위가 강조 구간의 절반을 넘어 (문서 제목 · 머리글처럼 전체를 덮는 제목) 이 자료는 목차로 가려내지 않았다. 가장 넓게 덮은 제목
    wide: Option<String>,
    /// 읽은 곳: outline (PDF 목차) · size (목차 없는 PDF 의 큰 글자 줄, 제목 구간에 줄 위치가 있다) ·
    /// slide (PPTX 제목과 구역) · slide_title (구역 없는 PPTX 의 제목) · slide_section (제목 자리 없는 PPTX 의 구역)
    source: String,
    /// 자료를 읽은 때 (source_material.updated_at, ms)
    read_at: i64,
    /// 핵심 범위 안 강조 구간 id → 그 범위의 구간 번호 (가장 안쪽 핵심 범위)
    #[serde(skip)]
    core: HashMap<i64, usize>,
}

/// 띄어쓰기를 빼고 소문자로 (제목과 강조 구간 글 맞추기)
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// 제목이 핵심 범위인 까닭 (OT4): 핵심 낱말이 들었거나, 같은 쪽에서 제목 글과 맞는 강조 구간에 사용자 표시가 있다
fn core_reason(title: &str, page: i64, texts: &[&Segment]) -> Option<String> {
    let lower = title.to_lowercase();
    // 영어 낱말은 낱말 단위로 ("Model Score" 의 core 를 잡지 않게)
    let has = |w: &str| if w.is_ascii() { lower.split(|c: char| !c.is_alphanumeric()).any(|x| x == w) } else { lower.contains(w) };
    if let Some(w) = CORE_WORDS.iter().find(|w| has(w)) {
        return Some(format!("제목에 '{w}'"));
    }
    let t = squash(title);
    texts.iter().filter(|s| s.page == page).find_map(|s| {
        let x = squash(&s.text);
        let (short, long) = if x.chars().count() <= t.chars().count() { (&x, &t) } else { (&t, &x) };
        // 한쪽이 다른 쪽에 들고 짧은 쪽이 긴 쪽의 절반 이상 (본문 문장 속 같은 낱말은 제목 표시가 아니다)
        let n = short.chars().count();
        let same = n >= 2 && long.contains(short.as_str()) && n * 2 >= long.chars().count();
        let mark = SIGNALS.iter().position(|g| MARKS.contains(&g.0) && s.signals.iter().any(|x| x == g.0))?;
        same.then(|| format!("제목이 {}로 강조돼 있어요", SIGNAL_LABEL[mark]))
    })
}

/// 구간 = 제목부터 다음 같은 · 윗 단계 제목 앞까지 (OT3). 순서는 쪽 → 쪽 안 순서, 같은 자리면 제목이 먼저 (윗 단계부터).
/// 두 기기가 따로 다시 읽어 같은 자리 같은 제목이 둘이면 하나로 본다
fn outline(segs: &[Segment], pages: i64) -> Outline {
    let level = |s: &Segment| if is_heading(&s.signals) { heading_of(&s.text).0 } else { 0 };
    let text = |s: &Segment| !is_heading(&s.signals) && !is_region(&s.signals);
    let mut items: Vec<&Segment> = segs.iter().collect();
    items.sort_by_key(|s| (s.page, s.ord, !is_heading(&s.signals), level(s)));
    items.dedup_by(|a, b| is_heading(&a.signals) && is_heading(&b.signals) && (a.page, a.ord, &a.text) == (b.page, b.ord, &b.text));
    let texts: Vec<&Segment> = items.iter().filter(|s| text(s)).copied().collect();
    // 화면 단계는 가장 얕은 제목을 1단계로 당긴다 (구역 없는 PPTX 는 제목이 모두 2단계로 저장된다, QA L6).
    // 구간 끝은 저장된 단계끼리 견주므로 구간 · 핵심 범위는 그대로다
    let base = items.iter().filter(|s| is_heading(&s.signals)).map(|s| level(s)).min().unwrap_or(1) - 1;
    let mut out = Outline::default();
    let mut ranges: Vec<(usize, Vec<i64>)> = vec![]; // 핵심 범위 구간 번호 · 그 안 강조 구간
    for (i, h) in items.iter().enumerate() {
        if !is_heading(&h.signals) {
            continue;
        }
        let (lv, title) = heading_of(&h.text);
        let end = items[i + 1..].iter().position(|s| is_heading(&s.signals) && level(s) <= lv).map_or(items.len(), |k| i + 1 + k);
        let inside: Vec<&Segment> = items[i + 1..end].iter().filter(|s| text(s)).copied().collect();
        // 다음 제목이 그 쪽 맨 앞(앞에 제목뿐)이면 끝나는 쪽은 그 앞 쪽
        let end_page = match items.get(end) {
            Some(b) if items[..end].iter().rev().take_while(|s| s.page == b.page).all(|s| is_heading(&s.signals)) => b.page - 1,
            Some(b) => b.page,
            None => pages,
        };
        let core = core_reason(title, h.page, &texts);
        if core.is_some() {
            ranges.push((out.sections.len(), inside.iter().map(|s| s.id).collect()));
        }
        out.sections.push(Section {
            id: h.id,
            level: lv - base,
            title: title.to_string(),
            page: h.page,
            end_page: end_page.max(h.page),
            segments: inside.len(),
            cards: inside.iter().flat_map(|s| s.links.iter().map(|l| l.card_id)).collect::<std::collections::HashSet<_>>().len(),
            core,
        });
    }
    let covered: std::collections::HashSet<i64> = ranges.iter().flat_map(|r| r.1.iter().copied()).collect();
    if covered.len() * 2 > texts.len() {
        out.wide = ranges.iter().max_by_key(|r| (r.1.len(), std::cmp::Reverse(r.0))).map(|r| out.sections[r.0].title.clone());
        out.sections.iter_mut().for_each(|s| s.core = None);
        return out;
    }
    // 안쪽 핵심 범위가 뒤에 오므로 덮어쓰면 가장 안쪽 범위가 남는다
    for (k, ids) in ranges {
        for id in ids {
            out.core.insert(id, k);
        }
    }
    out
}

fn outline_of(con: &Connection, material_id: i64) -> Res<Outline> {
    let (pages, kind, read_at): (i64, String, i64) = con
        .query_row("SELECT pages, kind, updated_at FROM source_material WHERE id = ?1", [material_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(err)?;
    let segs = segments_of(con, material_id)?;
    let mut out = outline(&segs, pages);
    // 큰 글자 줄로 읽은 제목만 줄 위치(rects)를 남긴다 (pdf.ts readPdf)
    let sized = segs.iter().any(|s| is_heading(&s.signals) && s.rects.is_some());
    // PPTX 는 구역이 1단계, 슬라이드 제목이 2단계로 저장된다 (pptx_headings)
    let has = |lv: usize| segs.iter().any(|s| is_heading(&s.signals) && heading_of(&s.text).0 == lv);
    out.source = match (kind == "pptx", has(1), has(2)) {
        (true, true, false) => "slide_section",
        (true, false, true) => "slide_title",
        (true, ..) => "slide",
        _ if sized => "size",
        _ => "outline",
    }
    .into();
    out.read_at = read_at;
    Ok(out)
}

/// I1 [목차 N] 탭. 제목 구간이 없으면 빈 목록
#[tauri::command]
pub fn source_outline(state: State<AppState>, material_id: i64) -> Res<Outline> {
    let con = state.db.lock().map_err(err)?;
    outline_of(&con, material_id)
}

/// [목차 다시 읽기]: 이 버전 전에 읽은 자료에 제목 구간을 채운다 (OT6). 제목 구간만 바꾼다: 강조 구간 · 카드 연결 · 기출 표시는 그대로.
/// PDF 는 화면이 pdf.js 로 읽은 제목(headings)을 보내고, PPTX 는 비워 보내면 파일에서 다시 읽는다
#[tauri::command]
pub fn source_save_outline(state: State<AppState>, material_id: i64, headings: Option<Vec<NewSegment>>) -> Res<Outline> {
    let headings = match headings {
        Some(h) => h,
        None => {
            let mut p = open_pptx(&state, material_id)?;
            let slides = pptx_paras(&mut p);
            pptx_headings(&slides, &p.sections)
        }
    };
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    save_headings(&tx, material_id, &headings)?;
    tx.commit().map_err(err)?;
    outline_of(&con, material_id)
}

/// 읽은(ready) 자료의 제목 구간만 바꾼다. 같은 제목이면 아무것도 쓰지 않는다 (동기화 변경이 불지 않게)
fn save_headings(con: &Connection, material_id: i64, headings: &[NewSegment]) -> Res<()> {
    let ready: bool = con.query_row("SELECT status = 'ready' FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).map_err(err)?;
    let mut st = con.prepare("SELECT page, ord, text FROM source_segment WHERE material_id = ?1 AND signals = 'heading' ORDER BY page, ord, text").map_err(err)?;
    let old: Vec<(i64, i64, String)> = st.query_map([material_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    let mut new: Vec<(i64, i64, String)> =
        headings.iter().filter(|h| sorted_signals(&h.signals) == [HEADING] && !h.text.trim().is_empty()).map(|h| (h.page, h.ord, h.text.trim().to_string())).collect();
    new.sort();
    if !ready || old == new {
        return Ok(());
    }
    con.execute("DELETE FROM source_segment WHERE material_id = ?1 AND signals = 'heading'", [material_id]).map_err(err)?;
    insert_segments(con, material_id, headings, true)
}

#[derive(Serialize)]
pub struct Link {
    card_id: i64,
    /// manual: 직접 연결 / auto: 자동 연결
    method: String,
    /// draft: R1 초안에서 만듦 (method 는 manual)
    origin: Option<String>,
}

#[derive(Serialize)]
pub struct Segment {
    id: i64,
    page: i64,
    ord: i64,
    text: String,
    signals: Vec<String>,
    weight: f64,
    rects: Option<Vec<[f64; 4]>>,
    before: Option<String>,
    after: Option<String>,
    links: Vec<Link>,
    /// 8.4 기출 구간 (I1 ⋯ [기출 구간으로 표시]). R1-a 정렬에서 ★ 와 같은 등급
    exam: exam::ExamState,
}

const SEGMENT_SQL: &str = "SELECT id, page, ord, text, signals, weight, rects, before_text, after_text, exam_hits, exam_names FROM source_segment";

fn segment_row(r: &rusqlite::Row) -> rusqlite::Result<Segment> {
    let signals: String = r.get(4)?;
    let rects: Option<String> = r.get(6)?;
    Ok(Segment {
        id: r.get(0)?,
        page: r.get(1)?,
        ord: r.get(2)?,
        text: r.get(3)?,
        signals: signals.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
        weight: r.get(5)?,
        rects: rects.and_then(|r| serde_json::from_str(&r).ok()),
        before: r.get(7)?,
        after: r.get(8)?,
        links: vec![],
        exam: exam::ExamState::of(r.get(0)?, r.get(9)?, r.get(10)?),
    })
}

pub(crate) fn segments_of(con: &Connection, material_id: i64) -> Res<Vec<Segment>> {
    let mut st = con.prepare(&format!("{SEGMENT_SQL} WHERE material_id = ?1 ORDER BY page, ord")).map_err(err)?;
    let mut segs: Vec<Segment> = st.query_map([material_id], segment_row).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    let mut st = con
        .prepare(&format!(
            "SELECT l.segment_id, l.card_id, l.method, l.origin FROM source_link l JOIN source_segment s ON s.id = l.segment_id
             WHERE s.material_id = ?1 AND l.method != 'rejected' ORDER BY l.method = 'manual' DESC, l.score DESC, {LINK_ORDER}",
        ))
        .map_err(err)?;
    let mut links: HashMap<i64, Vec<Link>> = HashMap::new();
    for row in st.query_map([material_id], |r| Ok((r.get::<_, i64>(0)?, Link { card_id: r.get(1)?, method: r.get(2)?, origin: r.get(3)? }))).map_err(err)? {
        let (seg, link) = row.map_err(err)?;
        links.entry(seg).or_default().push(link);
    }
    for s in &mut segs {
        s.links = links.remove(&s.id).unwrap_or_default();
    }
    Ok(segs)
}

/// I1 가운데·오른쪽 열. 쪽 순서로 준다 (신뢰도 순 정렬은 화면이)
#[tauri::command]
pub fn source_segments(state: State<AppState>, material_id: i64) -> Res<Vec<Segment>> {
    let con = state.db.lock().map_err(err)?;
    segments_of(&con, material_id)
}

/// [카드 잇기]. 끊었던 자동 연결을 다시 이어도 직접 연결이 된다
#[tauri::command]
pub fn source_link(state: State<AppState>, segment_id: i64, card_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    link(&con, segment_id, card_id)
}

fn link(con: &Connection, segment_id: i64, card_id: i64) -> Res<()> {
    con.execute(
        "INSERT INTO source_link (segment_id, card_id, method, created_at) VALUES (?1, ?2, 'manual', ?3)
         ON CONFLICT (segment_id, card_id) DO UPDATE SET method = 'manual', score = NULL",
        params![segment_id, card_id, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// 연결 끊기. 자동 연결은 rejected 로 남겨 다시 분석해도 되살리지 않는다
#[tauri::command]
pub fn source_unlink(state: State<AppState>, segment_id: i64, card_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    unlink(&con, segment_id, card_id)
}

fn unlink(con: &Connection, segment_id: i64, card_id: i64) -> Res<()> {
    con.execute("DELETE FROM source_link WHERE segment_id = ?1 AND card_id = ?2 AND method = 'manual'", params![segment_id, card_id])
        .map_err(err)?;
    con.execute("UPDATE source_link SET method = 'rejected' WHERE segment_id = ?1 AND card_id = ?2 AND method = 'auto'", params![segment_id, card_id])
        .map_err(err)?;
    Ok(())
}

// ------------------------------------------------------------------ 8.4 그림 · 표 영역 (IT9)

/// 그림 · 표 카드가 나온 곳. kind: figure | table. text: 대체 글 · "그림 · 3쪽"(PPTX "그림 · 슬라이드 3") · 표 글자 (I2 패널 · 검색)
#[derive(Deserialize)]
pub struct Region {
    material_id: i64,
    page: i64,
    rects: Vec<[f64; 4]>,
    kind: String,
    text: String,
}

/// 영역 구간. 같은 쪽 · 같은 영역 · 같은 종류면 그 구간을 다시 쓰고 글자만 바꾼다 (같은 표에서 두 번 만들어도 구간 하나)
fn region_segment(con: &Connection, r: &Region) -> Res<i64> {
    if !matches!(r.kind.as_str(), "figure" | "table") {
        return Err("그림이나 표만 이을 수 있어요.".into());
    }
    let rects = serde_json::to_string(&r.rects).map_err(err)?;
    let text = if r.text.trim().is_empty() {
        // 위치 표기는 화면과 같게: PPTX "슬라이드 N", PDF "N쪽" (QA T4-14)
        let kind: Option<String> = con.query_row("SELECT kind FROM source_material WHERE id = ?1", [r.material_id], |x| x.get(0)).optional().map_err(err)?;
        let place = if kind.as_deref() == Some("pptx") { format!("슬라이드 {}", r.page) } else { format!("{}쪽", r.page) };
        format!("{} · {place}", if r.kind == "table" { "표" } else { "그림" })
    } else {
        r.text.trim().to_string()
    };
    let hit: Option<i64> = con
        .query_row(
            "SELECT id FROM source_segment WHERE material_id = ?1 AND page = ?2 AND signals = ?3 AND rects = ?4",
            params![r.material_id, r.page, r.kind, rects],
            |row| row.get(0),
        )
        .optional()
        .map_err(err)?;
    if let Some(id) = hit {
        con.execute("UPDATE source_segment SET text = ?2 WHERE id = ?1", params![id, text]).map_err(err)?;
        return Ok(id);
    }
    // 쪽 안 순서는 그 쪽의 강조 구간 뒤
    con.execute(
        "INSERT INTO source_segment (material_id, page, ord, text, signals, weight, rects)
         SELECT ?1, ?2, coalesce(max(ord), -1) + 1, ?3, ?4, 0, ?5 FROM source_segment WHERE material_id = ?1 AND page = ?2",
        params![r.material_id, r.page, text, r.kind, rects],
    )
    .map_err(err)?;
    Ok(con.last_insert_rowid())
}

/// 노트의 카드를 모두 그 구간에 직접 연결로 (빈칸 노트는 카드가 여럿일 수 있다)
fn link_notes(con: &Connection, segment_id: i64, note_ids: &[i64]) -> Res<()> {
    for nid in note_ids {
        con.execute(
            "INSERT INTO source_link (segment_id, card_id, method, created_at) SELECT ?1, id, 'manual', ?2 FROM card WHERE note_id = ?3
             ON CONFLICT (segment_id, card_id) DO UPDATE SET method = 'manual', score = NULL",
            params![segment_id, now_ms(), nid],
        )
        .map_err(err)?;
    }
    Ok(())
}

/// 다른 경로로 만든 노트(M 그림 가리기 · Q 목록)를 영역에 잇는다. 구간 id 를 돌려준다
#[tauri::command]
pub fn source_region_link(state: State<AppState>, material_id: i64, page: i64, rects: Vec<[f64; 4]>, kind: String, text: String, note_ids: Vec<i64>) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let seg = region_segment(&tx, &Region { material_id, page, rects, kind, text })?;
    link_notes(&tx, seg, &note_ids)?;
    tx.commit().map_err(err)?;
    Ok(seg)
}

/// IT3 [카드 N장 만들기] · 그림을 앞면에 둔 카드: 초안(table::drafts 또는 화면이 만든 HTML)을 저장하고 영역에 잇는다.
/// 한 번의 변경(card_change 'draft')이라 되돌리기(qualityApi.undo)로 노트를 지운다. 영역 구간은 남는다 (연결만 사라진다)
#[tauri::command]
pub fn source_region_cards(state: State<AppState>, deck_id: i64, region: Region, cards: Vec<crate::table::Draft>) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    region_cards(&mut con, deck_id, &region, &cards)
}

fn region_cards(con: &mut Connection, deck_id: i64, region: &Region, cards: &[crate::table::Draft]) -> Res<i64> {
    if cards.is_empty() {
        return Err("만들 카드가 없어요.".into());
    }
    let tx = con.transaction().map_err(err)?;
    let mut notes = vec![];
    for c in cards {
        if render::is_blank(&c.front) || (c.kind != "cloze" && render::is_blank(&c.back)) {
            return Err("앞면과 뒷면을 채워 주세요.".into());
        }
        let nid = if c.kind == "list" {
            crate::recall::insert(&tx, crate::recall::LIST, deck_id, &[c.front.clone(), c.back.clone(), "순서대로".into(), String::new()], None, "", None)?
        } else {
            let card = crate::commands::NewCard { deck_id, kind: c.kind.clone(), front: c.front.clone(), back: c.back.clone(), importance: None };
            crate::commands::insert_note(&tx, &card, "", None)?;
            tx.query_row("SELECT note_id FROM card WHERE id = last_insert_rowid()", [], |r| r.get(0)).map_err(err)?
        };
        notes.push(nid);
    }
    let seg = region_segment(&tx, region)?;
    link_notes(&tx, seg, &notes)?;
    let file: String = tx.query_row("SELECT file_name FROM source_material WHERE id = ?1", [region.material_id], |r| r.get(0)).map_err(err)?;
    let what = if region.kind == "table" { "표" } else { "그림" };
    tx.execute(
        "INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at) VALUES ('draft', ?1, NULL, ?2, ?3, ?4)",
        params![deck_id, format!("「{file}」 {what}에서 카드 {}장 만들기", notes.len()), serde_json::json!({ "prev_state": null, "new_notes": notes }).to_string(), now_ms()],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(err)?;
    Ok(id)
}

/// R8 넣을 곳: 같은 자료에서 쪽이 가장 가까운 "카드가 있는 구간"의 카드 덱 (rag 초안 · IT 그림 · 표)
pub(crate) fn nearest_deck(con: &Connection, material_id: i64, page: i64) -> Res<Option<i64>> {
    con.query_row(
        &format!(
            "SELECT c.deck_id FROM source_segment s JOIN source_link l ON l.segment_id = s.id AND l.method != 'rejected'
             JOIN card c ON c.id = l.card_id WHERE s.material_id = ?1 ORDER BY abs(s.page - ?2), s.page, s.ord, {LINK_ORDER} LIMIT 1"
        ),
        params![material_id, page],
        |r| r.get(0),
    )
    .optional()
    .map_err(err)
}

/// PDF 그림 · 표의 넣을 곳 기본값 (R8, PPTX 는 source_slide_objects 의 deck_id). 없으면 자료의 덱
#[tauri::command]
pub fn source_region_deck(state: State<AppState>, material_id: i64, page: i64) -> Res<i64> {
    let con = state.db.lock().map_err(err)?;
    match nearest_deck(&con, material_id, page)? {
        Some(d) => Ok(d),
        None => con.query_row("SELECT deck_id FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).map_err(err),
    }
}

#[derive(Serialize)]
pub struct CardSource {
    material: Material,
    segment: Segment,
    /// 카드와의 연결 방식. 구간으로 열면(근거 칩) 빈 글자
    method: String,
}

/// R 근거 칩: 구간 하나를 I2 패널로 (카드 연결과 상관없이)
#[tauri::command]
pub fn source_segment(state: State<AppState>, segment_id: i64) -> Res<CardSource> {
    let con = state.db.lock().map_err(err)?;
    let material_id: i64 =
        con.query_row("SELECT material_id FROM source_segment WHERE id = ?1", [segment_id], |r| r.get(0)).map_err(|_| "구간을 찾을 수 없어요.".to_string())?;
    let segment = segments_of(&con, material_id)?.into_iter().find(|s| s.id == segment_id).ok_or("구간을 찾을 수 없어요.")?;
    Ok(CardSource { material: material(&con, material_id)?, segment, method: String::new() })
}

/// I2 학습 중 원자료. 직접 연결을 먼저, 그다음 유사도가 높은 것. 없으면 null (버튼을 숨긴다)
#[tauri::command(async)]
pub fn source_for_card(state: State<AppState>, card_id: i64) -> Res<Option<CardSource>> {
    let con = state.db.lock().map_err(err)?;
    for_card(&con, card_id)
}

fn for_card(con: &Connection, card_id: i64) -> Res<Option<CardSource>> {
    let hit: Option<(i64, i64, String)> = con
        .query_row(
            &format!(
                "SELECT s.material_id, s.id, l.method FROM source_link l JOIN source_segment s ON s.id = l.segment_id
                 WHERE l.card_id = ?1 AND l.method != 'rejected' ORDER BY l.method = 'manual' DESC, l.score DESC, {LINK_ORDER} LIMIT 1"
            ),
            [card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(err)?;
    let Some((material_id, segment_id, method)) = hit else { return Ok(None) };
    let segment = segments_of(con, material_id)?.into_iter().find(|s| s.id == segment_id).ok_or("구간을 찾을 수 없어요.")?;
    Ok(Some(CardSource { material: material(con, material_id)?, segment, method }))
}

/// I1 "분석 뒤에 추가한 카드 N장". 분석 시각을 따로 두지 않고 이 기기의 임베딩 캐시로 센다
/// (분석은 기기마다 하므로 기기 안 값이 맞다). 이 덱을 분석한 적이 없으면 0
#[tauri::command]
pub fn source_unanalyzed(state: State<AppState>, deck_id: i64) -> Res<i64> {
    let con = state.db.lock().map_err(err)?;
    unanalyzed(&con, deck_id)
}

fn unanalyzed(con: &Connection, deck_id: i64) -> Res<i64> {
    let ids = subtree(con, deck_id)?;
    let (missing, done): (i64, i64) = con
        .query_row(
            &format!(
                "SELECT coalesce(sum(e.ref_id IS NULL), 0), count(e.ref_id) FROM card c
                 LEFT JOIN embedding_cache e ON e.kind = 'card' AND e.ref_id = c.id AND e.model = ?1 WHERE c.deck_id IN ({ids})"
            ),
            [embed::MODEL_KEY],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err)?;
    Ok(if done > 0 { missing } else { 0 })
}

// ------------------------------------------------------------------ PPTX

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Para {
    pub text: String,
    pub signals: Vec<&'static str>,
    /// 표 칸 안 문단 (I2 패널은 표를 따로 그려서 뺀다)
    pub cell: bool,
    /// 8.4 목차: 제목 자리(p:ph type title · ctrTitle) 안 문단인가 · 런 글자 크기(sz)의 가장 큰 값 pt (적어 두지 않았으면 0)
    #[serde(skip)]
    pub title: bool,
    #[serde(skip)]
    pub size: f64,
}

/// 열어 둔 PPTX: 슬라이드 경로 (presentation.xml 의 sldIdLst 순서, 파일 이름 순서와 다를 수 있다) · 슬라이드 크기 (EMU) ·
/// 구역 (p14:section 이름과 슬라이드 수, 순서대로)
struct Pptx<R> {
    zip: zip::ZipArchive<R>,
    slides: Vec<String>,
    size: (f64, f64),
    sections: Sections,
}

/// 구역 이름 · 그 안 슬라이드 수 (순서대로)
type Sections = Vec<(String, usize)>;

/// PPTX 안 XML 한 파일의 최대 크기. 보통 수백 KB 다. 압축 폭탄이 메모리를 다 쓰지 않게
const MAX_XML: u64 = 32 << 20;
/// 슬라이드 그림 한 장의 최대 크기 (파일 전체 상한과 같은 정도)
const MAX_IMAGE: u64 = 200 << 20;

/// max 바이트까지만 읽는다. 넘으면 None (잘린 내용을 쓰지 않는다)
fn read_capped(r: impl Read, max: u64) -> Option<Vec<u8>> {
    let mut b = vec![];
    r.take(max + 1).read_to_end(&mut b).ok()?;
    (b.len() as u64 <= max).then_some(b)
}

fn zip_text<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    String::from_utf8(read_capped(zip.by_name(name).ok()?, MAX_XML)?).ok()
}

/// rels 의 Target 을 zip 안 경로로 ("../media/a.png" 는 dir 기준, "/ppt/.." 는 그대로)
fn resolve(dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') { vec![] } else { dir.split('/').filter(|p| !p.is_empty()).collect() };
    for p in target.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// rels 의 Id → zip 안 경로. 외부 링크(TargetMode="External")는 뺀다
fn rels_map(xml: &str, dir: &str) -> HashMap<String, String> {
    xml_attrs(xml, "Relationship", &["Id", "Target", "TargetMode"])
        .into_iter()
        .filter(|v| v[2] != "External")
        .map(|v| (v[0].clone(), resolve(dir, &v[1])))
        .collect()
}

fn pptx_open<R: Read + Seek>(r: R) -> Res<Pptx<R>> {
    let bad = || "PPTX 파일을 열 수 없어요.".to_string();
    let mut zip = zip::ZipArchive::new(r).map_err(|_| bad())?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let rels = zip_text(&mut zip, "ppt/_rels/presentation.xml.rels").ok_or_else(bad)?;
    let pres = zip_text(&mut zip, "ppt/presentation.xml").ok_or_else(bad)?;
    let targets = rels_map(&rels, "ppt");
    let mut slides: Vec<String> = xml_attrs_ns(&pres, "sldId", "id").into_iter().filter_map(|rid| targets.get(&rid).cloned()).collect();
    if slides.is_empty() {
        // 목록이 없으면 slideN.xml 의 N 순서로
        let num = |n: &str| n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse::<u32>().ok();
        slides = names.into_iter().filter(|n| num(n).is_some()).collect();
        slides.sort_by_key(|n| num(n));
    }
    let size = xml_attrs(&pres, "sldSz", &["cx", "cy"])
        .first()
        .and_then(|v| Some((v[0].parse::<f64>().ok()?, v[1].parse::<f64>().ok()?)))
        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
        .unwrap_or((9144000.0, 6858000.0));
    let sections = pptx_sections(&pres);
    Ok(Pptx { zip, slides, size, sections })
}

/// 구역 (p14:sectionLst 의 p14:section): 이름과 그 안 슬라이드 수, 순서대로. 구역을 쓰지 않은 파일은 비어 있다
fn pptx_sections(pres: &str) -> Sections {
    let mut reader = quick_xml::Reader::from_str(pres);
    let mut out: Sections = vec![];
    let mut in_lst = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "sectionLst" => in_lst = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == "sectionLst" => in_lst = false,
            Ok(Event::Start(e) | Event::Empty(e)) if in_lst => match e.local_name().as_ref() {
                "section" => out.push((e.attributes().flatten().find(|a| a.key.as_ref() == "name").and_then(|a| attr_value(&a)).unwrap_or_default(), 0)),
                "sldId" => {
                    if let Some(last) = out.last_mut() {
                        last.1 += 1;
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn pptx_paras<R: Read + Seek>(p: &mut Pptx<R>) -> Vec<Vec<Para>> {
    let slides = std::mem::take(&mut p.slides);
    let out = slides.iter().map(|s| zip_text(&mut p.zip, s).map(|x| parse_slide(&x)).unwrap_or_default()).collect();
    p.slides = slides;
    out
}

/// 슬라이드별 문단과 강조 신호
pub fn pptx_slides(bytes: &[u8]) -> Res<Vec<Vec<Para>>> {
    Ok(pptx_paras(&mut pptx_open(std::io::Cursor::new(bytes))?))
}

/// 슬라이드별 문단 · 구역 (source_add)
fn pptx_read(bytes: &[u8]) -> Res<(Vec<Vec<Para>>, Sections)> {
    let mut p = pptx_open(std::io::Cursor::new(bytes))?;
    Ok((pptx_paras(&mut p), std::mem::take(&mut p.sections)))
}

fn attr_value(a: &quick_xml::events::attributes::Attribute) -> Option<String> {
    a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned())
}

/// 어떤 요소들의 속성 값 (없는 속성은 빈 글자). local name 으로만 찾는다
fn xml_attrs(xml: &str, elem: &str, keys: &[&str]) -> Vec<Vec<String>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = vec![];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == elem => {
                out.push(
                    keys.iter()
                        .map(|k| {
                            e.attributes()
                                .flatten()
                                .find(|a| a.key.as_ref() == *k)
                                .and_then(|a| attr_value(&a))
                                .unwrap_or_default()
                        })
                        .collect(),
                );
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// r:id 처럼 접두어가 붙은 속성 (접두어는 문서마다 다를 수 있어 local name 으로 찾는다)
fn xml_attrs_ns(xml: &str, elem: &str, local: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = vec![];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == elem => {
                if let Some(v) = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key.as_ref().contains(':') && a.key.local_name().as_ref() == local)
                    .and_then(|a| attr_value(&a))
                {
                    out.push(v);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn is_red(hex: &str) -> bool {
    let c = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).map(i32::from);
    matches!((c(0), c(2), c(4)), (Some(r), Some(g), Some(b)) if r >= 150 && g <= 90 && b <= 90 && r - g.max(b) >= 100)
}

/// 타이핑한 별표: ★ ☆ 또는 문단 앞뒤의 * (손으로 그린 표시는 읽지 못한다)
fn has_star(text: &str) -> bool {
    let t = text.trim();
    t.contains('★') || t.contains('☆') || t.starts_with('*') || t.ends_with('*')
}

/// 슬라이드 하나의 문단. 런(a:r)의 서식 중 하이라이트·밑줄·붉은 글씨·볼드를 문단 신호로 올린다
fn parse_slide(xml: &str) -> Vec<Para> {
    #[derive(Default, Clone, Copy)]
    struct Run {
        bold: bool,
        underline: bool,
        red: bool,
        highlight: bool,
        size: f64,
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut paras = vec![];
    let mut cur: Option<Para> = None;
    let mut run = Run::default();
    let (mut in_rpr, mut in_fill, mut in_hl, mut in_t) = (false, false, false, false);
    let mut in_tbl = 0usize;
    // 8.4 목차: 도형(p:sp) 깊이와 제목 자리인 도형의 깊이
    let (mut sp, mut title_sp) = (0usize, None::<usize>);
    let mark = |p: &mut Para, run: Run| {
        for (on, sig) in [(run.highlight, "highlight"), (run.underline, "underline"), (run.red, "red"), (run.bold, "bold")] {
            if on && !p.signals.contains(&sig) {
                p.signals.push(sig);
            }
        }
        p.size = p.size.max(run.size);
    };
    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                // <a:rPr/> 처럼 빈 요소는 End 가 오지 않으므로 "안에 있음" 표시를 켜지 않는다
                let open = matches!(ev, Event::Start(_));
                let attr = |k: &str| {
                    e.attributes().flatten().find(|a| a.key.as_ref() == k).and_then(|a| attr_value(&a))
                };
                match e.local_name().as_ref() {
                    "p" if open => cur = Some(Para { cell: in_tbl > 0, title: title_sp.is_some() && in_tbl == 0, ..Default::default() }),
                    "tbl" if open => in_tbl += 1,
                    "sp" if open => sp += 1,
                    "ph" if sp > 0 && attr("type").is_some_and(|t| t == "title" || t == "ctrTitle") => title_sp = Some(sp),
                    "r" | "fld" => run = Run::default(),
                    "rPr" => {
                        run.bold = attr("b").is_some_and(|v| v == "1" || v == "true");
                        run.underline = attr("u").is_some_and(|v| v != "none");
                        run.size = attr("sz").and_then(|v| v.parse::<f64>().ok()).map_or(0.0, |v| v / 100.0);
                        in_rpr = open;
                    }
                    "solidFill" if in_rpr && !in_hl => in_fill = open,
                    "highlight" if in_rpr => {
                        in_hl = open;
                        run.highlight = true;
                    }
                    "srgbClr" if in_fill => run.red |= attr("val").is_some_and(|v| is_red(&v)),
                    "t" => in_t = open,
                    "br" => {
                        if let Some(p) = cur.as_mut() {
                            p.text.push(' ');
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "p" => {
                    if let Some(mut p) = cur.take() {
                        p.text = p.text.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !p.text.is_empty() {
                            if has_star(&p.text) {
                                p.signals.push("star");
                            }
                            p.signals = sorted_signals(&p.signals);
                            paras.push(p);
                        }
                    }
                }
                "rPr" => in_rpr = false,
                "tbl" => in_tbl = in_tbl.saturating_sub(1),
                "sp" => {
                    if title_sp == Some(sp) {
                        title_sp = None;
                    }
                    sp = sp.saturating_sub(1);
                }
                "solidFill" => in_fill = false,
                "highlight" => in_hl = false,
                "t" => in_t = false,
                _ => {}
            },
            Event::Text(t) if in_t => {
                if let Some(p) = cur.as_mut() {
                    let s = t.xml10_content();
                    if !s.trim().is_empty() {
                        mark(p, run);
                    }
                    p.text.push_str(&s);
                }
            }
            Event::GeneralRef(r) if in_t => {
                if let Some(p) = cur.as_mut() {
                    let name = r.xml10_content();
                    match r.resolve_char_ref() {
                        Ok(Some(c)) => p.text.push(c),
                        _ => p.text.push_str(quick_xml::escape::resolve_predefined_entity(&name).unwrap_or("")),
                    }
                    mark(p, run);
                }
            }
            _ => {}
        }
    }
    paras
}

/// 강조된 문단만 구간으로. 앞뒤 문단은 I2 패널의 문맥
fn pptx_segments(slides: &[Vec<Para>]) -> Vec<NewSegment> {
    let mut out = vec![];
    for (i, paras) in slides.iter().enumerate() {
        for (j, p) in paras.iter().enumerate() {
            if p.signals.is_empty() {
                continue;
            }
            out.push(NewSegment {
                page: i as i64 + 1,
                ord: j as i64,
                text: p.text.clone(),
                signals: p.signals.iter().map(|s| s.to_string()).collect(),
                rects: None,
                before: j.checked_sub(1).map(|k| paras[k].text.clone()),
                after: paras.get(j + 1).map(|p| p.text.clone()),
            });
        }
    }
    out
}

/// 8.4 목차 (OT1): 구역 이름은 1단계, 슬라이드 제목은 2단계. 쪽 안 순서는 -1 (그 슬라이드의 강조 구간보다 앞).
/// 제목 자리가 없는 슬라이드는 글자 크기(sz)가 본문(크기를 적은 문단의 중앙값)의 1.3배 이상인 가장 큰 문단을 제목으로 본다
fn pptx_headings(slides: &[Vec<Para>], sections: &[(String, usize)]) -> Vec<NewSegment> {
    let head = |i: usize, level: usize, title: &str| NewSegment {
        page: i as i64 + 1,
        ord: -1,
        text: heading_text(level, title),
        signals: vec![HEADING.into()],
        rects: None,
        before: None,
        after: None,
    };
    // 구역은 슬라이드를 순서대로 나눈다. 수가 맞지 않으면(읽지 못한 슬라이드) 구역을 쓰지 않는다
    let mut starts = HashMap::new();
    if sections.iter().map(|s| s.1).sum::<usize>() == slides.len() {
        let mut at = 0;
        for (name, n) in sections {
            if *n > 0 && !name.trim().is_empty() {
                starts.insert(at, name.as_str());
            }
            at += n;
        }
    }
    let mut sizes: Vec<f64> = slides.iter().flatten().filter(|p| p.size > 0.0 && !p.title && !p.cell).map(|p| p.size).collect();
    sizes.sort_by(f64::total_cmp);
    let body = sizes.get(sizes.len() / 2).copied();
    let mut out = vec![];
    for (i, paras) in slides.iter().enumerate() {
        if let Some(name) = starts.get(&i) {
            out.push(head(i, 1, name));
        }
        let mut title = paras.iter().filter(|p| p.title).map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ");
        if title.is_empty()
            && let Some(b) = body
            && let Some(p) = paras.iter().filter(|p| !p.cell && p.size >= b * 1.3 && p.text.chars().count() <= 60).max_by(|x, y| x.size.total_cmp(&y.size))
        {
            title = p.text.clone();
        }
        if !title.trim().is_empty() {
            out.push(head(i, 2, &title));
        }
    }
    out
}

/// I1 미리보기 (PPTX). 슬라이드 하나의 모든 문단과 신호
#[tauri::command]
pub fn source_slide(state: State<AppState>, material_id: i64, page: i64) -> Res<Vec<Para>> {
    let rel: String = {
        let con = state.db.lock().map_err(err)?;
        con.query_row("SELECT rel_path FROM source_material WHERE id = ?1 AND kind = 'pptx'", [material_id], |r| r.get(0)).map_err(err)?
    };
    let bytes = Some(rel)
        .filter(|r| crate::commands::safe_rel_path(r, "sources"))
        .and_then(|r| std::fs::read(state.data_dir.join(r)).ok())
        .ok_or_else(|| "이 기기에는 자료 파일이 없어요.".to_string())?;
    let mut slides = pptx_slides(&bytes)?;
    let i = usize::try_from(page - 1).map_err(err)?;
    Ok(if i < slides.len() { slides.swap_remove(i) } else { vec![] })
}

// ------------------------------------------------------------------ 8.4 PPTX 그림 · 표 (IT1-p)

/// 슬라이드 그림 (p:pic). rect 는 슬라이드 대비 [x, y, w, h] 0~1 (그룹 안이면 그룹 변환을 거친 값).
/// supported: 앱이 그릴 수 있는 형식 (EMF · WMF · TIFF 는 아니다, IT11)
#[derive(Serialize, Debug, PartialEq)]
pub struct SlideImage {
    index: usize,
    /// 도형 이름 · 대체 글 (cNvPr name · descr)
    name: String,
    descr: String,
    rect: [f64; 4],
    ext: String,
    supported: bool,
    #[serde(skip)]
    path: String,
}

/// 슬라이드 표 (a:tbl). 합친 칸은 table::Cell 그대로
#[derive(Serialize, Debug, PartialEq)]
pub struct SlideTable {
    name: String,
    rect: [f64; 4],
    rows: Vec<Vec<crate::table::Cell>>,
}

#[derive(Serialize)]
pub struct SlideObjects {
    page: i64,
    /// 첫 문단 (보통 제목)
    title: String,
    /// R8 넣을 곳 기본값
    deck_id: i64,
    images: Vec<SlideImage>,
    tables: Vec<SlideTable>,
}

const NO_TF: [f64; 4] = [1.0, 0.0, 1.0, 0.0];

/// 그림 · 표와 위치. 그룹(grpSp)은 chOff · chExt 로 바꿔 슬라이드 좌표로. mc:Fallback 은 Choice 와 같은 것이라 건너뛴다
fn parse_objects(xml: &str, rels: &HashMap<String, String>, size: (f64, f64)) -> (Vec<SlideImage>, Vec<SlideTable>) {
    use crate::table::Cell;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut path: Vec<String> = vec![];
    // 그룹 변환 x' = a·x + b, y' = c·y + d (바깥 그룹부터 합성)
    let mut tf: Vec<[f64; 4]> = vec![NO_TF];
    let mut grp = [0.0f64; 8]; // off x y · ext cx cy · chOff x y · chExt cx cy
    let mut xfrm = [0.0f64; 4];
    let (mut images, mut tables) = (vec![], vec![]);
    let mut pic: Option<[String; 3]> = None; // name · descr · r:embed
    let mut frame_name = String::new();
    let mut tbl: Option<Vec<Vec<Cell>>> = None;
    let mut cell: Option<Cell> = None;
    let mut fallback = 0usize;
    let mut unplaced = 0usize; // 위치(xfrm) 없는 그림 수
    let round = |v: f64| (v * 10000.0).round() / 10000.0;
    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let open = matches!(ev, Event::Start(_));
                let name = e.local_name().as_ref().to_string();
                let attr = |k: &str| e.attributes().flatten().find(|a| a.key.as_ref() == k).and_then(|a| attr_value(&a)).unwrap_or_default();
                let num = |k: &str| attr(k).parse::<f64>().unwrap_or(0.0);
                if name == "Fallback" && open {
                    fallback += 1;
                }
                if fallback == 0 {
                    let parent = path.last().map_or("", String::as_str);
                    let grand = path.len().checked_sub(2).and_then(|i| path.get(i)).map_or("", String::as_str);
                    match name.as_str() {
                        "grpSp" if open => tf.push(*tf.last().unwrap_or(&NO_TF)),
                        "pic" if open => {
                            pic = Some(Default::default());
                            // OLE 개체의 대체 그림은 graphicFrame 의 위치를 쓴다
                            if !path.iter().any(|p| p == "graphicFrame") {
                                xfrm = [0.0; 4];
                            }
                        }
                        "graphicFrame" if open => {
                            frame_name.clear();
                            xfrm = [0.0; 4];
                        }
                        "cNvPr" => match pic.as_mut() {
                            Some(p) => (p[0], p[1]) = (attr("name"), attr("descr")),
                            None => frame_name = attr("name"),
                        },
                        "blip" => {
                            if let Some(p) = pic.as_mut() {
                                p[2] = e
                                    .attributes()
                                    .flatten()
                                    .find(|a| a.key.as_ref().contains(':') && a.key.local_name().as_ref() == "embed")
                                    .and_then(|a| attr_value(&a))
                                    .unwrap_or_default();
                            }
                        }
                        "off" | "ext" | "chOff" | "chExt" if parent == "xfrm" => {
                            let (x, y) = if name.ends_with("ff") { (num("x"), num("y")) } else { (num("cx"), num("cy")) };
                            let i = ["off", "ext", "chOff", "chExt"].iter().position(|n| *n == name).unwrap_or(0);
                            if grand == "grpSpPr" {
                                (grp[i * 2], grp[i * 2 + 1]) = (x, y);
                            } else if i < 2 && matches!(grand, "spPr" | "graphicFrame") {
                                (xfrm[i * 2], xfrm[i * 2 + 1]) = (x, y);
                            }
                        }
                        "tbl" if open => tbl = Some(vec![]),
                        "tr" if open => {
                            if let Some(t) = tbl.as_mut() {
                                t.push(vec![]);
                            }
                        }
                        "tc" => {
                            let on = |k: &str| matches!(attr(k).as_str(), "1" | "true");
                            let span = |k: &str| attr(k).parse::<usize>().unwrap_or(1).max(1);
                            let c = Cell { text: String::new(), row_span: span("rowSpan"), col_span: span("gridSpan"), merged: on("hMerge") || on("vMerge") };
                            match (open, tbl.as_mut().and_then(|t| t.last_mut())) {
                                (true, _) => cell = Some(c),
                                (false, Some(row)) => row.push(c),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                if open {
                    path.push(name);
                }
            }
            Event::End(_) => {
                let Some(name) = path.pop() else { continue };
                if name == "Fallback" {
                    fallback = fallback.saturating_sub(1);
                    continue;
                }
                if fallback > 0 {
                    continue;
                }
                let t = *tf.last().unwrap_or(&NO_TF);
                let rect = || {
                    let (x, y, w, h) = (t[0] * xfrm[0] + t[1], t[2] * xfrm[1] + t[3], t[0] * xfrm[2], t[2] * xfrm[3]);
                    [round(x / size.0), round(y / size.1), round(w / size.0), round(h / size.1)]
                };
                match name.as_str() {
                    "grpSpPr" if path.last().is_some_and(|p| p == "grpSp") => {
                        let a = if grp[6] > 0.0 { grp[2] / grp[6] } else { 1.0 };
                        let c = if grp[7] > 0.0 { grp[3] / grp[7] } else { 1.0 };
                        let (b, d) = (grp[0] - grp[4] * a, grp[1] - grp[5] * c);
                        let p = tf.len().checked_sub(2).and_then(|i| tf.get(i)).copied().unwrap_or(NO_TF);
                        if let Some(last) = tf.last_mut() {
                            *last = [p[0] * a, p[0] * b + p[1], p[2] * c, p[2] * d + p[3]];
                        }
                        grp = [0.0; 8];
                    }
                    "grpSp" if tf.len() > 1 => {
                        tf.pop();
                    }
                    "pic" => {
                        if let Some([name, descr, embed]) = pic.take()
                            && let Some(target) = rels.get(&embed)
                        {
                            let ext = target.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
                            let supported = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp");
                            let mut rect = rect();
                            // 자리표시자 그림은 xfrm 없이 레이아웃 위치를 물려받는다. 위치 없는 그림은 그 순번을 x 에 넣어 그림마다 다른 영역이 되게 한다
                            // (모두 [0,0,0,0] 이라 한 구간으로 묶여 카드 수 · I2 그림이 섞였다, QA T4-02). 첫째는 [0,0,0,0] 그대로라 전에 만든 연결은 첫째 그림에 남는다.
                            // ponytail: 레이아웃(slideLayout) 의 자리표시자 위치는 읽지 않는다. 쪽 보기에 자리를 그려야 하면 ph idx 로 따라간다
                            if rect[2] == 0.0 && rect[3] == 0.0 {
                                rect[0] = round(unplaced as f64 / 10000.0);
                                unplaced += 1;
                            }
                            images.push(SlideImage { index: images.len(), name, descr, rect, ext, supported, path: target.clone() });
                        }
                    }
                    "tc" => {
                        if let (Some(mut c), Some(row)) = (cell.take(), tbl.as_mut().and_then(|t| t.last_mut())) {
                            c.text = c.text.trim().to_string();
                            row.push(c);
                        }
                    }
                    // 칸 안 문단은 줄로 나눈다
                    "p" => {
                        if let Some(c) = cell.as_mut() {
                            c.text.push('\n');
                        }
                    }
                    "graphicFrame" => {
                        if let Some(rows) = tbl.take().filter(|r| r.iter().any(|row| !row.is_empty())) {
                            tables.push(SlideTable { name: std::mem::take(&mut frame_name), rect: rect(), rows });
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) if fallback == 0 && path.last().is_some_and(|p| p == "t") => {
                if let Some(c) = cell.as_mut() {
                    c.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) if fallback == 0 && path.last().is_some_and(|p| p == "t") => {
                if let Some(c) = cell.as_mut() {
                    let name = r.xml10_content();
                    match r.resolve_char_ref() {
                        Ok(Some(ch)) => c.text.push(ch),
                        _ => c.text.push_str(quick_xml::escape::resolve_predefined_entity(&name).unwrap_or("")),
                    }
                }
            }
            _ => {}
        }
    }
    (images, tables)
}

/// 자료 복사본 (PPTX). 전체를 읽지 않고 zip 에서 필요한 것만 꺼낸다
fn open_pptx(state: &AppState, material_id: i64) -> Res<Pptx<std::fs::File>> {
    let rel: String = {
        let con = state.db.lock().map_err(err)?;
        con.query_row("SELECT rel_path FROM source_material WHERE id = ?1 AND kind = 'pptx'", [material_id], |r| r.get(0)).map_err(err)?
    };
    let file = Some(rel)
        .filter(|r| crate::commands::safe_rel_path(r, "sources"))
        .and_then(|r| std::fs::File::open(state.data_dir.join(r)).ok())
        .ok_or_else(|| "이 기기에는 자료 파일이 없어요.".to_string())?;
    pptx_open(file)
}

/// 슬라이드 하나의 XML · 그림 · 표 (rels 로 그림 파일을 찾는다)
fn slide_objects<R: Read + Seek>(p: &mut Pptx<R>, i: usize) -> Option<(String, Vec<SlideImage>, Vec<SlideTable>)> {
    let slide = p.slides.get(i)?.clone();
    let xml = zip_text(&mut p.zip, &slide)?;
    let (dir, file) = slide.rsplit_once('/').unwrap_or(("", &slide));
    let rels = rels_map(&zip_text(&mut p.zip, &format!("{dir}/_rels/{file}.rels")).unwrap_or_default(), dir);
    let (images, tables) = parse_objects(&xml, &rels, p.size);
    Some((xml, images, tables))
}

/// IT1-p. page 를 주면 그 슬라이드만, 없으면 그림 · 표가 있는 슬라이드 전부
#[tauri::command]
pub fn source_slide_objects(state: State<AppState>, material_id: i64, page: Option<i64>) -> Res<Vec<SlideObjects>> {
    let mut p = open_pptx(&state, material_id)?;
    let mut out = vec![];
    for i in 0..p.slides.len() {
        if page.is_some_and(|pg| pg != i as i64 + 1) {
            continue;
        }
        let Some((xml, images, tables)) = slide_objects(&mut p, i) else { continue };
        if page.is_none() && images.is_empty() && tables.is_empty() {
            continue;
        }
        let title = parse_slide(&xml).into_iter().next().map(|p| p.text).unwrap_or_default();
        out.push(SlideObjects { page: i as i64 + 1, title, deck_id: 0, images, tables });
    }
    let con = state.db.lock().map_err(err)?;
    let own: i64 = con.query_row("SELECT deck_id FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).map_err(err)?;
    for o in &mut out {
        o.deck_id = nearest_deck(&con, material_id, o.page)?.unwrap_or(own);
    }
    Ok(out)
}

/// 그림 원본 바이트 (썸네일 · 가리기 편집기). 미디어 폴더에는 카드를 만들 때 화면이 save_media 로 넣는다
#[tauri::command]
pub fn source_slide_image(state: State<AppState>, material_id: i64, page: i64, index: usize) -> Res<tauri::ipc::Response> {
    let mut p = open_pptx(&state, material_id)?;
    let i = usize::try_from(page - 1).map_err(err)?;
    let (_, images, _) = slide_objects(&mut p, i).ok_or("슬라이드를 찾을 수 없어요.")?;
    let img = images.get(index).ok_or("그림을 찾을 수 없어요.")?;
    let f = p.zip.by_name(&img.path).map_err(|_| "그림을 찾을 수 없어요.".to_string())?;
    let bytes = read_capped(f, MAX_IMAGE).ok_or("그림이 너무 크거나 읽을 수 없어요.")?;
    Ok(tauri::ipc::Response::new(bytes))
}

// ------------------------------------------------------------------ I3 분석: 임베딩 → 자동 연결 → 중요도 제안

static ANALYZING: AtomicBool = AtomicBool::new(false);
static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);

/// "source-analyze" 이벤트. step/steps: 카드 → 자료 하나씩, label: 지금 읽는 것 (I1 "분석 중 2/3 · 기출.pdf")
#[derive(Serialize, Clone)]
pub struct Progress {
    step: usize,
    steps: usize,
    label: String,
    done: usize,
    total: usize,
}

/// 분석 중이면 지금 진행 (화면을 다시 열었을 때 이어서 보여준다)
pub fn analyzing() -> Option<Progress> {
    if ANALYZING.load(Ordering::SeqCst) { PROGRESS.lock().ok().and_then(|p| p.clone()) } else { None }
}

#[derive(Serialize)]
pub struct Analysis {
    /// 새로 만든 자동 연결
    linked: usize,
    /// 자료로 바꾼 중요도 제안 (하위 덱 수)
    suggested: usize,
    /// 다른 분석이 도는 중이라 줄에 섰다. 그 분석이 끝나면 이어서 하고, 끝나면 "source-analyze" null 이 온다
    queued: bool,
}

/// 분석하는 동안 들어온 분석 요청 (덱 id). 지금 분석이 끝나면 이어서 한다 (QA T6-05: 분석 중에 넣은 자료가 오류를 띄우고 분석되지 않았다)
static QUEUED: Mutex<Vec<i64>> = Mutex::new(vec![]);

/// 분석을 시작하면 true. 이미 분석 중이면 그 덱을 줄에 세우고 false
fn begin(deck_id: i64) -> bool {
    let mut q = QUEUED.lock().unwrap_or_else(|e| e.into_inner());
    if !ANALYZING.swap(true, Ordering::SeqCst) {
        return true;
    }
    if !q.contains(&deck_id) {
        q.push(deck_id);
    }
    false
}

/// 줄에 선 다음 덱. 없으면 분석을 끝낸다 (begin 과 같은 잠금 안에서 깃발을 내려 그 사이 들어온 요청을 잃지 않는다)
fn next_queued() -> Option<i64> {
    let mut q = QUEUED.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_empty() {
        if let Ok(mut p) = PROGRESS.lock() {
            *p = None;
        }
        ANALYZING.store(false, Ordering::SeqCst);
        return None;
    }
    Some(q.remove(0))
}

#[tauri::command]
pub async fn source_analyze(app: AppHandle, deck_id: i64) -> Res<Analysis> {
    if !begin(deck_id) {
        return Ok(Analysis { linked: 0, suggested: 0, queued: true });
    }
    let mut first = None;
    let mut deck = deck_id;
    loop {
        let a = app.clone();
        let res = tauri::async_runtime::spawn_blocking(move || analyze(&a, deck)).await.map_err(err).and_then(|r| r);
        first.get_or_insert(res);
        match next_queued() {
            Some(d) => deck = d,
            None => break,
        }
    }
    // 어떻게 끝나든 null: 분석을 시작하지 않은 화면(다시 들어온 자료 화면 · 중요도 배너)도 막대를 지우고 다시 읽는다 (QA T6-04)
    let _ = app.emit("source-analyze", None::<Progress>);
    first.unwrap_or_else(|| Err("분석하지 못했어요.".into()))
}

fn text_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// 캐시에 없거나 글이 바뀐 것만 계산한다. 계산하는 동안에는 DB 를 잠그지 않는다
pub(crate) fn embed_cached(app: &AppHandle, kind: &str, items: &[(i64, String)], mut tick: impl FnMut(usize)) -> Res<HashMap<i64, Vec<f32>>> {
    let state = app.state::<AppState>();
    let cached: HashMap<i64, (String, Vec<u8>)> = {
        let con = state.db.lock().map_err(err)?;
        let mut st = con.prepare("SELECT ref_id, text_hash, vec FROM embedding_cache WHERE kind = ?1 AND model = ?2").map_err(err)?;
        let rows = st.query_map(params![kind, embed::MODEL_KEY], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let mut out = HashMap::new();
    let mut todo = vec![];
    for (id, text) in items {
        let h = text_hash(text);
        match cached.get(id) {
            Some((ch, v)) if *ch == h => {
                out.insert(*id, v.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect());
            }
            _ => todo.push((*id, text.clone(), h)),
        }
    }
    tick(items.len() - todo.len());
    let model = embed::load(&state.data_dir)?;
    for chunk in todo.chunks(16) {
        let vecs = model.embed(&chunk.iter().map(|c| c.1.clone()).collect::<Vec<_>>())?;
        let con = state.db.lock().map_err(err)?;
        for ((id, _, h), v) in chunk.iter().zip(vecs) {
            let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
            con.execute(
                "INSERT OR REPLACE INTO embedding_cache (kind, ref_id, model, text_hash, vec) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind, id, embed::MODEL_KEY, h, blob],
            )
            .map_err(err)?;
            out.insert(*id, v);
        }
        drop(con);
        tick(chunk.len());
    }
    Ok(out)
}

/// 카드 글 (앞 + 뒤, 서식 뺌)과 앞면 글. 카드 목록(C2)과 같은 방식으로 렌더한다. ids: 덱 id 목록 ("1,2,3")
pub(crate) fn card_texts(con: &Connection, ids: &str) -> Res<Vec<CardText>> {
    let names = note_field_names(con)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, n.fields, c.template_ord, n.note_type_id, t.qfmt, t.afmt
             FROM card c JOIN note n ON n.id = c.note_id
             LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {}
             WHERE c.deck_id IN ({ids}) ORDER BY c.id",
            crate::notetype::TEMPLATE_OF_CARD
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| {
            let fields: String = r.get(1)?;
            let q: Option<String> = r.get(4)?;
            let a: Option<String> = r.get(5)?;
            let rendered = render_fields(&names, r.get(3)?, &fields, q.as_deref(), a.as_deref(), r.get(2)?);
            let front = render::strip_tags(&rendered.front);
            let text = format!("{front} {}", render::strip_tags(answer_part(&rendered.back)));
            Ok(CardText { id: r.get(0)?, front, text: text.chars().take(1000).collect() })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

pub(crate) struct CardText {
    pub id: i64,
    pub front: String,
    /// 임베딩에 쓰는 글 (앞 + 뒤, 1000자까지)
    pub text: String,
}

/// 카드마다 가장 비슷한 구간 하나. 기준보다 낮으면 잇지 않는다. 벡터는 L2 정규화돼 있어 내적이 코사인이다
pub fn best_links(cards: &[(i64, &[f32])], segs: &[(i64, &[f32])], threshold: f32) -> Vec<(i64, i64, f32)> {
    let mut out = vec![];
    for (card, cv) in cards {
        let best = segs
            .iter()
            .map(|(s, sv)| (*s, cv.iter().zip(sv.iter()).map(|(a, b)| a * b).sum::<f32>()))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((seg, sim)) = best.filter(|b| b.1 >= threshold) {
            out.push((seg, *card, sim));
        }
    }
    out
}

pub struct DeckScore {
    pub deck_id: i64,
    /// 자료와 이어진 카드 수
    pub linked: usize,
    /// 소속 카드 점수(이어진 구간 가중치 합)의 평균
    pub score: f64,
}

/// 상대 순위로 제안 (post-mvp 6장). 연결이 있는 하위 덱끼리 비교해 상위 20% 핵심, 하위 20% 보충.
/// 3개 미만이면 순위가 의미 없어 제안하지 않는다. (덱, 중요도, 위/아래에서 몇 % 안인지)
pub fn rank(decks: &[DeckScore]) -> Vec<(i64, i64, usize)> {
    let pct = |k: usize, n: usize| (k as f64 * 100.0 / n as f64).round() as usize;
    let mut v: Vec<&DeckScore> = decks.iter().filter(|d| d.linked > 0).collect();
    let n = v.len();
    if n < 3 {
        return vec![];
    }
    v.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.deck_id.cmp(&b.deck_id)));
    let (top, bottom) = (n.div_ceil(5), n / 5);
    let mut out = vec![];
    for (i, d) in v.iter().enumerate() {
        if i < top {
            out.push((d.deck_id, 2, pct(i + 1, n)));
        } else if i >= n - bottom {
            out.push((d.deck_id, 0, pct(n - i, n)));
        }
    }
    out
}

fn emit(app: &AppHandle, p: Progress) {
    if let Ok(mut g) = PROGRESS.lock() {
        *g = Some(p.clone());
    }
    let _ = app.emit("source-analyze", p);
}

fn analyze(app: &AppHandle, deck_id: i64) -> Res<Analysis> {
    let state = app.state::<AppState>();
    let (ids, cards, materials, segs) = {
        let con = state.db.lock().map_err(err)?;
        // 지운 카드·구간의 캐시는 여기서 치운다
        con.execute_batch(
            "DELETE FROM embedding_cache WHERE kind = 'card' AND ref_id NOT IN (SELECT id FROM card);
             DELETE FROM embedding_cache WHERE kind = 'segment' AND ref_id NOT IN (SELECT id FROM source_segment);",
        )
        .map_err(err)?;
        let ids = subtree(&con, deck_id)?;
        let cards = card_texts(&con, &ids)?;
        let mut st = con
            .prepare(&format!("SELECT m.id, m.file_name FROM source_material m WHERE m.deck_id IN ({ids}) AND m.status = 'ready' ORDER BY {MATERIAL_ORDER}"))
            .map_err(err)?;
        let materials: Vec<(i64, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        let mut segs = vec![];
        for (m, _) in &materials {
            segs.push(link_candidates(segments_of(&con, *m)?));
        }
        (ids, cards, materials, segs)
    };
    let steps = materials.len() + 1;
    let total = cards.len() + segs.iter().map(Vec::len).sum::<usize>();
    let mut done = 0;
    let mut progress = |step: usize, label: &str, n: usize| {
        done += n;
        emit(app, Progress { step, steps, label: label.into(), done, total });
    };
    progress(0, &format!("카드 {}장", cards.len()), 0);
    let card_items: Vec<(i64, String)> = cards.iter().map(|c| (c.id, c.text.clone())).collect();
    let card_vecs = embed_cached(app, "card", &card_items, |n| progress(0, &format!("카드 {}장", cards.len()), n))?;
    let mut seg_vecs = HashMap::new();
    for (i, ((_, name), list)) in materials.iter().zip(&segs).enumerate() {
        progress(i + 1, name, 0);
        let items: Vec<(i64, String)> = list.iter().map(|s| (s.id, s.text.clone())).collect();
        seg_vecs.extend(embed_cached(app, "segment", &items, |n| progress(i + 1, name, n))?);
    }

    let con = state.db.lock().map_err(err)?;
    // 사용자가 직접 이은 카드와 끊은 연결은 그대로 둔다. 자동 연결만 새로 만든다 (이번에 후보에서 빠진 구간의 옛 자동 연결도 지운다, QA T6-06)
    let mat_ids = materials.iter().map(|m| m.0.to_string()).collect::<Vec<_>>().join(",");
    let mat_ids = if mat_ids.is_empty() { "-1".to_string() } else { mat_ids };
    con.execute(
        &format!("DELETE FROM source_link WHERE method = 'auto' AND segment_id IN (SELECT id FROM source_segment WHERE material_id IN ({mat_ids}))"),
        [],
    )
    .map_err(err)?;
    let manual: std::collections::HashSet<i64> = {
        let mut st = con.prepare("SELECT DISTINCT card_id FROM source_link WHERE method = 'manual'").map_err(err)?;
        st.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    let card_refs: Vec<(i64, &[f32])> =
        cards.iter().filter(|c| !manual.contains(&c.id)).filter_map(|c| card_vecs.get(&c.id).map(|v| (c.id, v.as_slice()))).collect();
    let seg_refs: Vec<(i64, &[f32])> = seg_vecs.iter().map(|(id, v)| (*id, v.as_slice())).collect();
    let links = best_links(&card_refs, &seg_refs, AUTO_LINK);
    for (seg, card, sim) in &links {
        con.execute(
            "INSERT OR IGNORE INTO source_link (segment_id, card_id, method, score, created_at) VALUES (?1, ?2, 'auto', ?3, ?4)",
            params![seg, card, *sim as f64, now_ms()],
        )
        .map_err(err)?;
    }
    let suggested = suggest(&con, &ids)?;
    exam::suggest_all(&con)?; // 자료 제안을 다시 만들면 기출 근거 줄도 다시 붙인다 (N3)
    Ok(Analysis { linked: links.len(), suggested, queued: false })
}

/// 자동 연결 · 중요도 제안에 쓸 구간. 그림 · 표 영역은 강조가 아니고, 목차 제목은 짧아 카드가 끌린다 (AUTO_LINK).
/// 같은 쪽 제목과 글이 같은 볼드뿐인 구간도 뺀다: 슬라이드 제목의 굵은 글씨는 서식이지 강조가 아니다 (QA T6-06: 표지 제목에 카드 9장이 붙었다)
fn link_candidates(segs: Vec<Segment>) -> Vec<Segment> {
    let titles: Vec<(i64, String)> = segs.iter().filter(|s| is_heading(&s.signals)).map(|s| (s.page, heading_of(&s.text).1.trim().to_string())).collect();
    let title_bold = |s: &Segment| s.signals == ["bold"] && titles.iter().any(|(p, t)| *p == s.page && t == s.text.trim());
    segs.into_iter().filter(|s| !is_region(&s.signals) && !is_heading(&s.signals) && !title_bold(s)).collect()
}

/// 하위 덱 중요도 제안. 제안을 붙인 덱 수
fn suggest(con: &Connection, ids: &str) -> Res<usize> {
    // 하위 덱 점수: 카드마다 이어진 구간 가중치 합의 평균. 근거 문장에 쓸 자료 이름과 신호도 모은다 (그림 · 표 영역은 강조가 아니라 뺀다)
    let mut st = con
        .prepare(&format!(
            "SELECT c.deck_id, c.id, s.weight, m.file_name, s.signals, s.id FROM card c
             JOIN source_link l ON l.card_id = c.id AND l.method != 'rejected'
             JOIN source_segment s ON s.id = l.segment_id JOIN source_material m ON m.id = s.material_id
             WHERE c.deck_id IN ({ids}) AND m.deck_id IN ({ids}) AND s.signals NOT IN ('figure','table','heading')"
        ))
        .map_err(err)?;
    #[derive(Default)]
    struct Acc {
        cards: std::collections::HashSet<i64>,
        weight: f64,
        files: HashMap<String, std::collections::HashSet<i64>>,
        signals: HashMap<String, usize>,
        /// 8.4 목차: 핵심 범위 제목 → 그 범위 강조 구간과 이어진 카드
        core: HashMap<String, std::collections::HashSet<i64>>,
    }
    let core = core_ranges(con, ids)?;
    let mut acc: HashMap<i64, Acc> = HashMap::new();
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?))
        })
        .map_err(err)?;
    for row in rows {
        let (deck, card, w, file, signals, seg) = row.map_err(err)?;
        let a = acc.entry(deck).or_default();
        a.cards.insert(card);
        a.weight += w;
        a.files.entry(file).or_default().insert(card);
        *a.signals.entry(signals.split(',').next().unwrap_or_default().to_string()).or_default() += 1;
        if let Some(title) = core.get(&seg) {
            a.core.entry(title.clone()).or_default().insert(card);
        }
    }
    drop(st);
    // 8.4 기출은 여기서 더하지 않는다. 기출 비율 규칙이 따로 제안하고 근거 줄을 붙인다 (exam::suggest_all, N3)
    let mut st = con.prepare(&format!("SELECT deck_id, count(*) FROM card WHERE deck_id IN ({ids}) GROUP BY deck_id")).map_err(err)?;
    let counts: Vec<(i64, usize)> = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    drop(st);
    let scores: Vec<DeckScore> = counts
        .iter()
        .map(|(id, n)| {
            let a = acc.get(id);
            DeckScore {
                deck_id: *id,
                linked: a.map_or(0, |a| a.cards.len()),
                score: a.map_or(0.0, |a| a.weight / (*n).max(1) as f64),
            }
        })
        .collect();
    let picks = rank(&scores);

    // 확정한 값(user)은 덮어쓰지 않는다 (MVP 6.5.1). 태그·이름 규칙 제안도 그대로 두고 비어 있는(default) 덱만 채운다.
    // 이번에 빠진 지난 자료 제안은 기본으로 되돌린다 (자료 제안은 default 였던 덱에만 붙으므로 잃는 것이 없다)
    con.execute(
        &format!("UPDATE deck SET importance = 1, importance_source = 'default', importance_reason = NULL WHERE id IN ({ids}) AND importance_source = 'material'"),
        [],
    )
    .map_err(err)?;
    let mut out: Vec<(i64, i64, String)> = vec![];
    for (deck, level, pct) in picks {
        let Some(a) = acc.get(&deck) else { continue };
        let (file, _) = a.files.iter().max_by_key(|(name, c)| (c.len(), std::cmp::Reverse((*name).clone()))).map(|(n, c)| (n.clone(), c.len())).unwrap_or_default();
        let signal = a.signals.iter().max_by_key(|(_, n)| **n).map(|(s, _)| s.as_str()).unwrap_or_default();
        let label = SIGNALS.iter().position(|s| s.0 == signal).map_or("강조", |i| SIGNAL_LABEL[i]);
        // 근거는 줄마다 머리를 붙인다. 기출 줄은 exam::suggest_all 이 앞에 더한다 (N3 툴팁 "근거 2가지")
        let reason = if level == 2 {
            format!("자료 · {}장이 {file}의 {label} 구간과 연결 (덱 안 상위 {pct}%)", a.cards.len())
        } else {
            format!("자료 · {}장만 강조 구간과 연결 (덱 안 하위 {pct}%)", a.cards.len())
        };
        out.push((deck, level, reason));
    }
    // 8.4 목차 (OT5): 자료와 이어진 카드의 절반 이상(3장 이상)이 핵심 범위와 이어진 덱은 핵심. 순위 제안이 핵심이면 줄을 더하고,
    // 보충이면 목차가 이긴다 (기출과 같다, N3). 제목 구간이 없는 자료는 core 가 비어 위 결과 그대로다
    let mut decks: Vec<(&i64, &Acc)> = acc.iter().filter(|(_, a)| !a.core.is_empty()).collect();
    decks.sort_by_key(|d| *d.0);
    for (deck, a) in decks {
        let cards: std::collections::HashSet<i64> = a.core.values().flatten().copied().collect();
        if cards.len() < OUTLINE_MIN_CARDS || cards.len() * 2 < a.cards.len() {
            continue;
        }
        let Some((label, top)) = a.core.iter().max_by_key(|(t, c)| (c.len(), std::cmp::Reverse((*t).clone()))) else { continue };
        let more = if a.core.len() > 1 { format!(" · 다른 핵심 범위 {}곳과도 연결", a.core.len() - 1) } else { String::new() };
        let line = format!("{OUTLINE_HEAD}이 하위 덱 카드 {}장이 {label}{more}", top.len());
        match out.iter_mut().find(|o| o.0 == *deck) {
            Some(o) if o.1 == 2 => o.2 = format!("{}\n{line}", o.2),
            Some(o) => *o = (*deck, 2, line),
            None => out.push((*deck, 2, line)),
        }
    }
    let mut suggested = 0;
    for (deck, level, reason) in out {
        suggested += con
            .execute(
                "UPDATE deck SET importance = ?1, importance_source = 'material', importance_reason = ?2 WHERE id = ?3 AND importance_source = 'default'",
                params![level, reason, deck],
            )
            .map_err(err)?;
    }
    Ok(suggested)
}

/// 하위 덱 자료들의 핵심 범위: 강조 구간 id → 그 범위를 말하는 글 ("a.pdf의 '3장 요약' 구간과 연결됐어요. … (22–24쪽)").
/// 제목 구간이 있는 자료만 읽는다
fn core_ranges(con: &Connection, ids: &str) -> Res<HashMap<i64, String>> {
    let mut st = con
        .prepare(&format!(
            "SELECT m.id, m.file_name FROM source_material m WHERE m.deck_id IN ({ids})
             AND EXISTS (SELECT 1 FROM source_segment s WHERE s.material_id = m.id AND s.signals = 'heading') ORDER BY m.id"
        ))
        .map_err(err)?;
    let rows: Vec<(i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    let mut core = HashMap::new();
    for (m, file) in rows {
        let o = outline_of(con, m)?;
        let labels: Vec<String> = o
            .sections
            .iter()
            .map(|s| {
                let title = if s.title.chars().count() > 30 { format!("{}…", s.title.chars().take(30).collect::<String>()) } else { s.title.clone() };
                let range = if s.end_page > s.page { format!("{}–{}", s.page, s.end_page) } else { s.page.to_string() };
                let range = if o.source.starts_with("slide") { format!("슬라이드 {range}") } else { format!("{range}쪽") };
                let size = if o.source == "size" { " · 목차가 없는 PDF라 큰 글자 줄로 읽은 제목이에요" } else { "" };
                format!("{file}의 '{title}' 구간과 연결됐어요. 목차에서 핵심 범위로 본 구간이에요 ({range}){size}")
            })
            .collect();
        core.extend(o.core.into_iter().map(|(seg, k)| (seg, labels[k].clone())));
    }
    Ok(core)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn capped_read_refuses_more_than_max() {
        assert_eq!(read_capped(&b"abc"[..], 3).as_deref(), Some(&b"abc"[..]));
        assert_eq!(read_capped(&b"abcd"[..], 3), None, "넘으면 잘라 쓰지 않는다");
        // 압축 폭탄: 끝까지 풀지 않고 max + 1 에서 멈춘다
        assert_eq!(read_capped(std::io::repeat(0), 1 << 10), None);
    }

    /// 슬라이드 두 장짜리 최소 PPTX. 순서는 presentation.xml 이 정한다 (slide2 가 먼저)
    fn tiny_pptx() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(vec![]);
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        let files = [
            (
                "ppt/presentation.xml",
                r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId id="256" r:id="rId3"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst><p:sldSz cx="10000000" cy="5000000"/></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="rId2" Target="slides/slide1.xml"/><Relationship Id="rId3" Target="slides/slide2.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                r#"<p:sld xmlns:p="p" xmlns:a="a"><p:txBody>
                  <a:p><a:r><a:rPr lang="ko"/><a:t>평범한 문장</a:t></a:r></a:p>
                  <a:p><a:r><a:rPr b="1"/><a:t>로지스틱 회귀</a:t></a:r><a:r><a:t> &#8212; 종속변수가 범주형</a:t></a:r></a:p>
                  <a:p><a:r><a:rPr><a:solidFill><a:srgbClr val="C00000"/></a:solidFill></a:rPr><a:t>다중공선성 주의</a:t></a:r></a:p>
                  <a:p><a:r><a:rPr><a:highlight><a:srgbClr val="FFFF00"/></a:highlight></a:rPr><a:t>잔차 분석</a:t></a:r></a:p>
                  <a:p><a:r><a:rPr><a:solidFill><a:srgbClr val="1F4E79"/></a:solidFill></a:rPr><a:t>파란 글씨는 강조가 아님</a:t></a:r></a:p>
                </p:txBody>
                <p:pic><p:nvPicPr><p:cNvPr id="4" name="그림 3" descr="잔차 산점도"/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"><a:extLst><a:ext uri="x"><x:svgBlip r:embed="rId9"/></a:ext></a:extLst></a:blip></p:blipFill>
                  <p:spPr><a:xfrm><a:off x="1000000" y="500000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm></p:spPr></p:pic>
                <p:grpSp><p:nvGrpSpPr><p:cNvPr id="5" name="묶음"/></p:nvGrpSpPr>
                  <p:grpSpPr><a:xfrm><a:off x="5000000" y="2500000"/><a:ext cx="2000000" cy="1000000"/><a:chOff x="0" y="0"/><a:chExt cx="4000000" cy="2000000"/></a:xfrm></p:grpSpPr>
                  <p:pic><p:nvPicPr><p:cNvPr id="6" name="chart"/></p:nvPicPr><p:blipFill><a:blip r:embed="rId3"/></p:blipFill>
                    <p:spPr><a:xfrm><a:off x="2000000" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm></p:spPr></p:pic>
                </p:grpSp>
                <mc:AlternateContent><mc:Choice Requires="x"><p:sp/></mc:Choice><mc:Fallback>
                  <p:pic><p:nvPicPr><p:cNvPr id="8" name="대체"/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic>
                </mc:Fallback></mc:AlternateContent>
                <p:pic><p:nvPicPr><p:cNvPr id="9" name="링크"/></p:nvPicPr><p:blipFill><a:blip r:link="rId4"/></p:blipFill></p:pic>
                <p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="7" name="표 1"/></p:nvGraphicFramePr>
                  <p:xfrm><a:off x="0" y="2500000"/><a:ext cx="5000000" cy="2500000"/></p:xfrm>
                  <a:graphic><a:graphicData><a:tbl><a:tblGrid><a:gridCol w="1"/><a:gridCol w="1"/></a:tblGrid>
                    <a:tr><a:tc gridSpan="2"><a:txBody><a:p><a:r><a:t>구분</a:t></a:r></a:p></a:txBody></a:tc><a:tc hMerge="1"><a:txBody><a:p/></a:txBody></a:tc></a:tr>
                    <a:tr><a:tc rowSpan="2"><a:txBody><a:p><a:r><a:t>회귀</a:t></a:r></a:p></a:txBody></a:tc>
                      <a:tc><a:txBody><a:p><a:r><a:t>선형 &amp; 직선</a:t></a:r></a:p><a:p><a:r><a:t>둘째 줄</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
                    <a:tr><a:tc vMerge="1"><a:txBody><a:p/></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>로지스틱</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
                  </a:tbl></a:graphicData></a:graphic></p:graphicFrame>
                </p:sld>"#,
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                r#"<Relationships><Relationship Id="rId2" Target="../media/image1.png"/><Relationship Id="rId3" Target="../media/image2.EMF"/>
                   <Relationship Id="rId4" Target="https://example.com/a.png" TargetMode="External"/></Relationships>"#,
            ),
            ("ppt/media/image1.png", "PNG!"),
            (
                "ppt/slides/slide2.xml",
                r#"<p:sld xmlns:p="p" xmlns:a="a"><p:txBody><a:p><a:r><a:rPr u="sng"/><a:t>★ 기출 &amp; 핵심</a:t></a:r></a:p></p:txBody></p:sld>"#,
            ),
        ];
        for (name, body) in files {
            z.start_file(name, opt).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn pptx_emphasis() {
        let slides = pptx_slides(&tiny_pptx()).unwrap();
        assert_eq!(slides.len(), 2);
        // presentation.xml 순서: slide2 가 1쪽
        assert_eq!(slides[0], vec![Para { text: "★ 기출 & 핵심".into(), signals: vec!["underline", "star"], ..Default::default() }]);
        let s = &slides[1];
        assert_eq!(s[0].signals, Vec::<&str>::new());
        assert_eq!(s[1], Para { text: "로지스틱 회귀 — 종속변수가 범주형".into(), signals: vec!["bold"], ..Default::default() });
        assert_eq!(s[2].signals, vec!["red"]);
        assert_eq!(s[3].signals, vec!["highlight"]);
        assert_eq!(s[4].signals, Vec::<&str>::new());
        assert_eq!((s[5].text.as_str(), s[5].cell, s[4].cell), ("구분", true, false));
        let segs = pptx_segments(&slides);
        assert_eq!(segs.len(), 4);
        assert_eq!((segs[1].page, segs[1].before.as_deref(), segs[1].after.as_deref()), (2, Some("평범한 문장"), Some("다중공선성 주의")));
        assert_eq!(weight_of(&sorted_signals(&segs[0].signals)), 5.0);
        assert!(pptx_slides(b"not a zip").is_err());
    }

    #[test]
    fn placeholder_pictures_without_xfrm_stay_apart() {
        let pic = |id: &str| format!(r#"<p:pic><p:nvPicPr><p:cNvPr id="1" name="자리표시자 {id}"/></p:nvPicPr><p:blipFill><a:blip r:embed="{id}"/></p:blipFill><p:spPr/></p:pic>"#);
        let xml = format!("<p:sld><p:cSld><p:spTree>{}{}</p:spTree></p:cSld></p:sld>", pic("rId1"), pic("rId2"));
        let rels = HashMap::from([("rId1".to_string(), "ppt/media/a.png".to_string()), ("rId2".to_string(), "ppt/media/b.png".to_string())]);
        let (images, _) = parse_objects(&xml, &rels, (100.0, 100.0));
        assert_eq!(images.iter().map(|i| i.rect).collect::<Vec<_>>(), vec![[0.0, 0.0, 0.0, 0.0], [0.0001, 0.0, 0.0, 0.0]]);
    }

    #[test]
    fn pptx_objects() {
        let mut p = pptx_open(std::io::Cursor::new(tiny_pptx())).unwrap();
        assert_eq!(p.size, (10000000.0, 5000000.0));
        // presentation.xml 순서: slide1 이 2쪽. 1쪽(slide2)에는 그림 · 표가 없다
        let (_, images, tables) = slide_objects(&mut p, 0).unwrap();
        assert!(images.is_empty() && tables.is_empty());
        let (_, images, tables) = slide_objects(&mut p, 1).unwrap();
        // Fallback 안 그림 · 외부 링크 그림은 빼고 둘. 둘째는 그룹 변환(반으로 줄이고 옮김)을 거친다
        let img = |index, name: &str, descr: &str, rect, ext: &str, supported, path: &str| SlideImage {
            index,
            name: name.into(),
            descr: descr.into(),
            rect,
            ext: ext.into(),
            supported,
            path: path.into(),
        };
        assert_eq!(
            images,
            vec![
                img(0, "그림 3", "잔차 산점도", [0.1, 0.1, 0.2, 0.2], "png", true, "ppt/media/image1.png"),
                img(1, "chart", "", [0.6, 0.5, 0.1, 0.2], "emf", false, "ppt/media/image2.EMF"),
            ]
        );
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!((t.name.as_str(), t.rect), ("표 1", [0.0, 0.5, 0.5, 0.5]));
        let cell = |text: &str, row_span, col_span, merged| crate::table::Cell { text: text.into(), row_span, col_span, merged };
        assert_eq!(
            t.rows,
            vec![
                vec![cell("구분", 1, 2, false), cell("", 1, 1, true)],
                vec![cell("회귀", 2, 1, false), cell("선형 & 직선\n둘째 줄", 1, 1, false)],
                vec![cell("", 1, 1, true), cell("로지스틱", 1, 1, false)],
            ]
        );
        // 그림 바이트는 zip 에서 그대로
        let mut bytes = vec![];
        p.zip.by_name(&images[0].path).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"PNG!");
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt", "/ppt/slides/slide1.xml"), "ppt/slides/slide1.xml");
    }

    #[test]
    fn region_cards_link_and_undo() {
        use crate::table::{drafts, Draft, TableSpec};
        let mut con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, 'D', 0, 0), (2, '하위', 0, 0);
             UPDATE deck SET parent_id = 1 WHERE id = 2;
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (1, 1, '강의.pptx', 'sources/1.pptx', 'pptx', 0, 0);
             INSERT INTO source_segment (material_id, page, ord, text, signals, weight) VALUES (1, 3, 0, '강조', 'star', 2);",
        )
        .unwrap();
        let region = |kind: &str, rect: [f64; 4]| Region { material_id: 1, page: 3, rects: vec![rect], kind: kind.into(), text: String::new() };
        let spec = TableSpec {
            rows: vec![
                vec!["검정", "귀무가설"].into_iter().map(|t| crate::table::Cell { text: t.into(), row_span: 1, col_span: 1, merged: false }).collect(),
                vec!["t", "평균이 같다"].into_iter().map(|t| crate::table::Cell { text: t.into(), row_span: 1, col_span: 1, merged: false }).collect(),
                vec!["F", "분산이 같다"].into_iter().map(|t| crate::table::Cell { text: t.into(), row_span: 1, col_span: 1, merged: false }).collect(),
            ],
            header: true,
            method: "cloze".into(),
            cols: vec![1],
            hide_same_col: true,
        };
        let mut cards = drafts(&spec);
        cards.push(Draft { kind: "list".into(), front: "검정 2가지".into(), back: "t<br>F".into() });
        let change = region_cards(&mut con, 2, &region("table", [0.0, 0.5, 0.5, 0.5]), &cards).unwrap();
        let count = |con: &Connection, sql: &str| -> i64 { con.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count(&con, "SELECT count(*) FROM card WHERE deck_id = 2"), 3);
        assert_eq!(count(&con, "SELECT count(*) FROM note n JOIN note_type t ON t.id = n.note_type_id WHERE t.name = 'Addiem 목록 회상'"), 1);
        let segs = segments_of(&con, 1).unwrap();
        // 영역 구간은 그 쪽 강조 구간 뒤, 가중치 0, 글자가 없으면 "표 · 슬라이드 3" (PPTX)
        let t = segs.iter().find(|s| s.signals == ["table"]).unwrap();
        assert_eq!((t.ord, t.weight, t.text.as_str(), t.links.len(), t.rects.clone()), (1, 0.0, "표 · 슬라이드 3", 3, Some(vec![[0.0, 0.5, 0.5, 0.5]])));
        // 강조 수 · 연결 수 · 빠진 범위는 글자 강조만 (영역은 세지 않는다, IT13). R8 은 이 쪽에서 가장 가까운 카드의 덱
        let m = material(&con, 1).unwrap();
        assert_eq!((m.segments, m.linked, m.gaps), (1, 0, 1));
        assert_eq!(nearest_deck(&con, 1, 9).unwrap(), Some(2));
        let summary: String = con.query_row("SELECT summary FROM card_change WHERE id = ?1", [change], |r| r.get(0)).unwrap();
        assert_eq!(summary, "「강의.pptx」 표에서 카드 3장 만들기");
        // 같은 영역을 다시 이으면 구간 하나 (글자만 새로)
        let note: i64 = con.query_row("SELECT max(note_id) FROM card", [], |r| r.get(0)).unwrap();
        let mut again = region("table", [0.0, 0.5, 0.5, 0.5]);
        again.text = "검정 · 귀무가설".into();
        let seg = region_segment(&con, &again).unwrap();
        link_notes(&con, seg, &[note]).unwrap();
        assert_eq!((seg, count(&con, "SELECT count(*) FROM source_segment")), (t.id, 2));
        assert_eq!(segments_of(&con, 1).unwrap()[1].text, "검정 · 귀무가설");
        // 다른 영역 · 다른 종류는 새 구간. 모르는 종류는 받지 않는다
        assert_ne!(region_segment(&con, &region("figure", [0.0, 0.5, 0.5, 0.5])).unwrap(), t.id);
        assert!(region_segment(&con, &region("star", [0.0, 0.0, 1.0, 1.0])).is_err());
        assert!(region_cards(&mut con, 2, &region("figure", [0.1, 0.1, 0.2, 0.2]), &[]).is_err());
        assert!(region_cards(&mut con, 2, &region("figure", [0.1, 0.1, 0.2, 0.2]), &[Draft { kind: "basic".into(), front: "<img src=\"neo-a.png\">".into(), back: " ".into() }]).is_err());
        // 되돌리면 노트와 연결이 사라지고 구간은 남는다
        crate::quality::undo(&mut con, change).unwrap();
        assert_eq!(count(&con, "SELECT count(*) FROM card"), 0);
        assert_eq!(count(&con, "SELECT count(*) FROM source_link"), 0);
        assert_eq!(count(&con, "SELECT count(*) FROM source_segment WHERE signals = 'table'"), 1);
    }

    #[test]
    fn segment_links() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, 'D', 0, 0);
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (1, 1, 'a.pdf', 'sources/1.pdf', 'pdf', 0, 0);",
        )
        .unwrap();
        let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic'", [], |r| r.get(0)).unwrap();
        for id in [1, 2] {
            con.execute(
                "INSERT INTO note (id, note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, '[\"q\",\"a\"]', 0, 0)",
                params![id, nt],
            )
            .unwrap();
            con.execute("INSERT INTO card (id, note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?1, 1, 0, 0, 0)", [id])
                .unwrap();
        }
        let seg = |text: &str, signals: &[&str]| NewSegment {
            page: 1,
            ord: 0,
            text: text.into(),
            signals: signals.iter().map(|s| s.to_string()).collect(),
            rects: Some(vec![[0.1, 0.2, 0.5, 0.02]]),
            before: None,
            after: None,
        };
        // 알 수 없는 신호뿐인 구간은 버린다
        save_segments(&con, 1, 3, &[seg("회귀분석", &["bold", "highlight"]), seg("버림", &["glow"])], true).unwrap();
        let segs = segments_of(&con, 1).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].signals.clone(), segs[0].weight), (vec!["highlight".to_string(), "bold".to_string()], 4.0));
        let s = segs[0].id;
        link(&con, s, 1).unwrap();
        link(&con, s, 1).unwrap(); // 두 번 이어도 하나
        con.execute("INSERT INTO source_link (segment_id, card_id, method, score, created_at) VALUES (?1, 2, 'auto', 0.9, 0)", [s]).unwrap();
        assert_eq!(segments_of(&con, 1).unwrap()[0].links.len(), 2);
        // 분석한 적이 없으면 0, 카드 1만 분석했으면 그 뒤에 생긴 카드 2 한 장
        assert_eq!(unanalyzed(&con, 1).unwrap(), 0);
        con.execute("INSERT INTO embedding_cache (kind, ref_id, model, text_hash, vec) VALUES ('card', 1, ?1, 'h', x'00')", [embed::MODEL_KEY])
            .unwrap();
        assert_eq!(unanalyzed(&con, 1).unwrap(), 1);
        assert_eq!(material(&con, 1).unwrap().linked, 1);
        // 자동 연결을 끊으면 rejected 로 남아 목록에서 빠진다
        unlink(&con, s, 2).unwrap();
        assert_eq!(segments_of(&con, 1).unwrap()[0].links.iter().map(|l| l.card_id).collect::<Vec<_>>(), vec![1]);
        let m: String = con.query_row("SELECT method FROM source_link WHERE card_id = 2", [], |r| r.get(0)).unwrap();
        assert_eq!(m, "rejected");
        // 카드를 지우면 연결만, 자료를 지워도 카드는 그대로
        con.execute("DELETE FROM card WHERE id = 1", []).unwrap();
        assert!(segments_of(&con, 1).unwrap()[0].links.is_empty());
        con.execute("DELETE FROM source_material WHERE id = 1", []).unwrap();
        let cards: i64 = con.query_row("SELECT count(*) FROM card", [], |r| r.get(0)).unwrap();
        let left: i64 = con.query_row("SELECT count(*) FROM source_segment", [], |r| r.get(0)).unwrap();
        assert_eq!((cards, left), (1, 0));
    }

    #[test]
    fn exam_ratio_joins_material_suggestion() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '시험', 0, 0);
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (1, 1, 'a.pdf', 'sources/1.pdf', 'pdf', 0, 0);
             INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight) VALUES (1, 1, 1, 0, '강조', 'highlight', 3), (2, 1, 1, 1, '굵게', 'bold', 1);",
        )
        .unwrap();
        let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic'", [], |r| r.get(0)).unwrap();
        // (덱, 카드 수, 기출 카드 수, 첫 카드에 이을 구간). 기출 카드의 앞 두 장은 2회
        let mut id = 0;
        for (deck, n, exam, seg) in [(2, 10, 0, None), (3, 10, 7, Some(1)), (4, 10, 7, Some(2)), (5, 4, 4, None), (6, 10, 0, Some(2))] {
            con.execute("INSERT INTO deck (id, parent_id, name, created_at, updated_at) VALUES (?1, 1, ?1, 0, 0)", [deck]).unwrap();
            for i in 0..n {
                id += 1;
                let hits = (i < exam).then_some(if i < 2 { 2 } else { 1 });
                con.execute(
                    "INSERT INTO note (id, note_type_id, fields, exam_hits, created_at, updated_at) VALUES (?1, ?2, '[\"q\",\"a\"]', ?3, 0, 0)",
                    params![id, nt, hits],
                )
                .unwrap();
                con.execute("INSERT INTO card (id, note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?1, ?2, 0, 0, 0)", [id, deck]).unwrap();
                if let (0, Some(s)) = (i, seg) {
                    link(&con, s, id).unwrap();
                }
            }
        }
        let row = |deck: i64| -> (i64, String, Option<String>) {
            con.query_row("SELECT importance, importance_source, importance_reason FROM deck WHERE id = ?1", [deck], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
        };
        // 자료: 이어진 덱 3 · 4 · 6 중 덱 3 이 맨 위. 기출 가중치는 더하지 않는다
        assert_eq!(suggest(&con, "1,2,3,4,5,6").unwrap(), 1);
        let material = "자료 · 1장이 a.pdf의 하이라이트 구간과 연결 (덱 안 상위 33%)";
        assert_eq!(row(3), (2, "material".into(), Some(material.into())));
        // 기출 비율: 5개 덱의 상위 25% (2개, 같은 비율은 함께) = 덱 5(100%) · 덱 3 · 덱 4(70%). 덱 5 는 4장뿐이라 빠진다
        assert_eq!(crate::exam::suggest_all(&con).unwrap(), 2);
        let exam3 = "기출 · 이 하위 덱 카드 10장 중 7장에 기출 표시 (2회 이상 2장)";
        assert_eq!(row(3), (2, "material".into(), Some(format!("{exam3}\n{material}"))), "근거 2가지");
        assert_eq!(row(4).2.as_deref(), Some(exam3), "기본값 덱은 기출 근거만으로 핵심");
        assert_eq!(row(5), (1, "default".into(), None));
        assert_eq!(crate::exam::suggest_all(&con).unwrap(), 0, "바뀐 것이 없으면 고치지 않는다");
        // 기출 표시를 지우면 기출 줄만 빠지고, 기출로만 핵심이던 덱은 기본값으로
        con.execute("UPDATE note SET exam_hits = 0 WHERE id IN (SELECT note_id FROM card WHERE deck_id IN (3, 4))", []).unwrap();
        assert_eq!(crate::exam::suggest_all(&con).unwrap(), 2);
        assert_eq!(row(3), (2, "material".into(), Some(material.into())));
        assert_eq!(row(4), (1, "default".into(), None));
        // 확정한 덱은 덮지 않는다. 자료 제안이 핵심이 아니면 기출이 이긴다
        con.execute("UPDATE deck SET importance_source = 'user' WHERE id = 3", []).unwrap();
        con.execute("UPDATE deck SET importance = 0, importance_source = 'material', importance_reason = '자료 · 하위' WHERE id = 6", []).unwrap();
        con.execute("UPDATE note SET exam_hits = 1 WHERE id IN (SELECT note_id FROM card WHERE deck_id IN (3, 6))", []).unwrap();
        crate::exam::suggest_all(&con).unwrap();
        assert_eq!(row(3).1, "user");
        assert_eq!(row(6), (2, "material".into(), Some("기출 · 이 하위 덱 카드 10장 중 10장에 기출 표시".into())));
    }

    #[test]
    fn links_and_ranking() {
        // 고정 벡터: 카드 1 은 구간 10 과 거의 같고, 카드 2 는 어느 구간과도 멀다
        let (c1, c2) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let (s10, s11) = ([0.99, 0.141, 0.0], [0.0, 1.0, 0.0]);
        let links = best_links(&[(1, &c1[..]), (2, &c2[..])], &[(10, &s10[..]), (11, &s11[..])], AUTO_LINK);
        assert_eq!(links.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(), vec![(10, 1)]);

        let d = |deck_id, linked, score| DeckScore { deck_id, linked, score };
        // 연결 있는 덱이 3개 미만이면 제안하지 않는다
        assert!(rank(&[d(1, 3, 1.0), d(2, 1, 0.1), d(3, 0, 0.0)]).is_empty());
        let picks = rank(&[d(1, 2, 0.6), d(2, 5, 1.5), d(3, 1, 0.1), d(4, 3, 0.9), d(5, 2, 0.5), d(6, 0, 0.0)]);
        // 연결 있는 5개: 상위 1개 핵심(20%), 하위 1개 보충. 연결 없는 덱 6 은 순위에 넣지 않는다
        assert_eq!(picks, vec![(2, 2, 20), (3, 0, 20)]);
    }

    // ------------------------------------------------------------------ 8.4 목차 (OT)

    #[test]
    fn heading_text_round_trip() {
        assert_eq!(heading_text(2, " 3.2  요약 "), "## 3.2 요약");
        assert_eq!(heading_of("## 3.2 요약"), (2, "3.2 요약"));
        // 제목이 # 로 시작해도 앞 표시 한 덩어리만 단계다
        assert_eq!(heading_of(&heading_text(1, "# of samples")), (1, "# of samples"));
        assert_eq!(heading_of("표시 없음"), (1, "표시 없음"));
        assert_eq!(heading_of("####### 일곱"), (1, "####### 일곱"));
        assert_eq!(heading_text(9, "깊음"), "###### 깊음");
        // 핵심 낱말: 한국어는 글 안 어디든, 영어는 낱말 단위
        assert_eq!(core_reason("3장 요약정리", 1, &[]).as_deref(), Some("제목에 '요약'"));
        assert_eq!(core_reason("Key Summary", 1, &[]).as_deref(), Some("제목에 'summary'"));
        assert_eq!(core_reason("Model Score", 1, &[]), None);
    }

    /// 구역 둘(소개 1장 · 핵심 정리 2장), 슬라이드 셋: 제목 자리 · 제목 자리 없이 큰 글자 · 두 문단짜리 가운데 제목
    fn outline_pptx() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(vec![]);
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        let sp = |ph: &str, body: &str| format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="s"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:txBody>{body}</p:txBody></p:sp>"#);
        let para = |sz: u32, extra: &str, t: &str| format!(r#"<a:p><a:r><a:rPr lang="ko" sz="{sz}">{extra}</a:rPr><a:t>{t}</a:t></a:r></a:p>"#);
        let slide = |shapes: String| format!(r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#);
        let s1 = slide(format!(
            "{}{}",
            sp(r#"<p:ph type="title"/>"#, r#"<a:p><a:r><a:t>1장 개요</a:t></a:r></a:p>"#),
            sp("", &format!("{}{}", para(1800, "", "본문 첫 줄"), para(1800, "<a:highlight><a:srgbClr val=\"FFFF00\"/></a:highlight>", "하이라이트 본문")))
        ));
        let s2 = slide(sp("", &format!("{}{}{}", para(4000, "", "회귀 요약"), para(1800, "", "본문"), para(2000, "", "조금 큰 본문"))));
        let s3 = slide(format!(
            "{}{}",
            sp(r#"<p:ph type="ctrTitle"/>"#, r#"<a:p><a:r><a:t>3장</a:t></a:r></a:p><a:p><a:r><a:rPr u="sng"/><a:t>부록</a:t></a:r></a:p>"#),
            sp(r#"<p:ph type="body" idx="1"/>"#, &para(1800, "", "제목 아님"))
        ));
        let files = [
            (
                "ppt/presentation.xml",
                r#"<p:presentation xmlns:p="p" xmlns:r="r" xmlns:p14="p14"><p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/><p:sldId id="258" r:id="rId3"/></p:sldIdLst>
                   <p:extLst><p:ext uri="{521415D9-36F7-43E2-AB2F-B90AF26B5E84}"><p14:sectionLst>
                   <p14:section name="소개" id="{A}"><p14:sldIdLst><p14:sldId id="256"/></p14:sldIdLst></p14:section>
                   <p14:section name="핵심 정리" id="{B}"><p14:sldIdLst><p14:sldId id="257"/><p14:sldId id="258"/></p14:sldIdLst></p14:section>
                   </p14:sectionLst></p:ext></p:extLst></p:presentation>"#
                    .to_string(),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Target="slides/slide1.xml"/><Relationship Id="rId2" Target="slides/slide2.xml"/><Relationship Id="rId3" Target="slides/slide3.xml"/></Relationships>"#.to_string(),
            ),
            ("ppt/slides/slide1.xml", s1),
            ("ppt/slides/slide2.xml", s2),
            ("ppt/slides/slide3.xml", s3),
        ];
        for (name, body) in files {
            z.start_file(name, opt).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn pptx_titles_sizes_sections() {
        let (slides, sections) = pptx_read(&outline_pptx()).unwrap();
        assert_eq!(sections, vec![("소개".to_string(), 1), ("핵심 정리".to_string(), 2)]);
        assert_eq!(slides[0].iter().map(|p| (p.text.as_str(), p.title, p.size)).collect::<Vec<_>>(), vec![("1장 개요", true, 0.0), ("본문 첫 줄", false, 18.0), ("하이라이트 본문", false, 18.0)]);
        assert_eq!(slides[2].iter().map(|p| (p.title, p.cell)).collect::<Vec<_>>(), vec![(true, false), (true, false), (false, false)]);
        let heads = pptx_headings(&slides, &sections);
        let got: Vec<(i64, i64, &str)> = heads.iter().map(|h| (h.page, h.ord, h.text.as_str())).collect();
        // 본문 중앙값 18pt 의 1.3배(23.4pt) 이상만 큰 글자 제목. 20pt 는 아니다. 가운데 제목은 두 문단을 잇는다
        assert_eq!(got, vec![(1, -1, "# 소개"), (1, -1, "## 1장 개요"), (2, -1, "# 핵심 정리"), (2, -1, "## 회귀 요약"), (3, -1, "## 3장 부록")]);
        assert!(heads.iter().all(|h| h.signals == ["heading"] && h.rects.is_none()));
        // 강조 구간은 전과 같다 (제목은 따로 붙는다): 하이라이트 본문 · 밑줄 "부록"
        let segs = pptx_segments(&slides);
        assert_eq!(segs.iter().map(|s| (s.page, s.ord, s.text.as_str())).collect::<Vec<_>>(), vec![(1, 2, "하이라이트 본문"), (3, 1, "부록")]);
        // 구역 수가 슬라이드 수와 맞지 않으면 구역을 쓰지 않는다. 구역 없는 파일(tiny_pptx)은 제목 자리가 없어 제목도 없다
        assert!(pptx_headings(&slides, &[("소개".into(), 1)]).iter().all(|h| !h.text.starts_with("# ")));
        let (plain, none) = pptx_read(&tiny_pptx()).unwrap();
        assert!(none.is_empty() && pptx_headings(&plain, &none).is_empty());
    }

    /// 자료 하나 (PDF 10쪽) · 하위 덱 2..=6 의 카드. (덱, 카드 수) 대로 만들고 카드 id 를 돌려준다
    fn outline_db(decks: &[(i64, i64)]) -> (Connection, HashMap<i64, Vec<i64>>) {
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '시험', 0, 0);
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, pages, status, created_at, updated_at) VALUES (1, 1, 'a.pdf', 'sources/1.pdf', 'pdf', 10, 'ready', 0, 0);",
        )
        .unwrap();
        let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic'", [], |r| r.get(0)).unwrap();
        let mut cards = HashMap::new();
        let mut id = 0;
        for (deck, n) in decks {
            con.execute("INSERT INTO deck (id, parent_id, name, created_at, updated_at) VALUES (?1, 1, ?1, 0, 0)", [deck]).unwrap();
            for _ in 0..*n {
                id += 1;
                con.execute("INSERT INTO note (id, note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, '[\"q\",\"a\"]', 0, 0)", params![id, nt]).unwrap();
                con.execute("INSERT INTO card (id, note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?1, ?2, 0, 0, 0)", [id, *deck]).unwrap();
                cards.entry(*deck).or_insert_with(Vec::new).push(id);
            }
        }
        (con, cards)
    }

    fn add_seg(con: &Connection, page: i64, ord: i64, text: &str, signals: &str) -> i64 {
        let w = weight_of(&sorted_signals(&signals.split(',').collect::<Vec<_>>()));
        con.execute(
            "INSERT INTO source_segment (material_id, page, ord, text, signals, weight) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![page, ord, text, signals, w],
        )
        .unwrap();
        con.last_insert_rowid()
    }

    #[test]
    fn sections_and_core_ranges() {
        let (con, cards) = outline_db(&[(2, 3)]);
        // 강조 구간을 먼저 넣고 제목을 나중에 넣는다 (다시 읽기와 같은 행 순서): 같은 자리면 제목이 먼저여야 한다
        let bold = add_seg(&con, 1, 0, "개요 굵게", "bold");
        add_seg(&con, 2, 0, "빅데이터 정의", "highlight");
        add_seg(&con, 2, 1, "다른 강조", "underline");
        add_seg(&con, 2, 2, "또 다른 강조", "bold");
        let star = add_seg(&con, 3, 0, "3V ★", "star");
        let sum1 = add_seg(&con, 5, 1, "요약 1", "red");
        add_seg(&con, 6, 0, "요약 2", "highlight");
        add_seg(&con, 7, 0, "요약 3", "bold");
        let marked = add_seg(&con, 7, 2, "3장 회귀", "highlight");
        add_seg(&con, 8, 0, "회귀식", "bold");
        add_seg(&con, 9, 0, "그림 · 9쪽", "figure");
        for (page, ord, text) in [(1, 0, "# 1장 개요"), (3, 0, "## 1.1 특징"), (5, 0, "# 2장 요약"), (5, 0, "# 2장 요약"), (7, 2, "# 3장 회귀")] {
            add_seg(&con, page, ord, text, "heading");
        }
        link(&con, sum1, cards[&2][0]).unwrap();
        link(&con, sum1, cards[&2][1]).unwrap();
        link(&con, bold, cards[&2][2]).unwrap();
        link(&con, star, cards[&2][2]).unwrap();
        let o = outline_of(&con, 1).unwrap();
        let rows: Vec<_> =
            o.sections.iter().map(|s| (s.level, s.title.as_str(), s.page, s.end_page, s.segments, s.cards, s.core.as_deref())).collect();
        assert_eq!(
            rows,
            vec![
                // 볼드는 제목 서식이라 핵심 표시가 아니다. 아래 단계(1.1)의 강조 · 카드도 센다
                (1, "1장 개요", 1, 4, 5, 1, None),
                (2, "1.1 특징", 3, 4, 1, 1, None),
                // 같은 자리 같은 제목 둘은 하나로. 7쪽 3장 제목 앞에 강조가 있어 7쪽까지
                (1, "2장 요약", 5, 7, 3, 2, Some("제목에 '요약'")),
                (1, "3장 회귀", 7, 10, 2, 0, Some("제목이 하이라이트로 강조돼 있어요")),
            ]
        );
        assert_eq!((o.wide.as_deref(), o.source.as_str()), (None, "outline"));
        // 핵심 범위는 강조 구간 10곳 중 5곳 (그림 영역은 세지 않는다). 절반을 넘지 않아 쓴다
        assert_eq!(o.core.len(), 5);
        assert_eq!((o.core[&sum1], o.core[&marked]), (2, 3));
        // 개수 · 빠진 범위는 제목을 세지 않는다
        let m = material(&con, 1).unwrap();
        assert_eq!((m.segments, m.headings, m.gaps), (10, 5, 7));

        // 절반을 넘으면 (자료 전체를 덮는 "요약 노트" 같은 제목) 핵심 범위를 두지 않는다
        add_seg(&con, 1, 0, "# 핵심 요약 노트", "heading");
        let o = outline_of(&con, 1).unwrap();
        assert!(o.core.is_empty() && o.sections.iter().all(|s| s.core.is_none()));
        assert_eq!(o.wide.as_deref(), Some("핵심 요약 노트"), "가장 넓게 덮은 제목");
        // 큰 글자 줄로 읽은 제목은 줄 위치가 있다
        con.execute("UPDATE source_segment SET rects = '[[0,0,1,0.1]]' WHERE signals = 'heading'", []).unwrap();
        assert_eq!(outline_of(&con, 1).unwrap().source, "size");
    }

    /// (덱, 중요도, 출처, 근거) 전부
    fn deck_rows(con: &Connection) -> Vec<(i64, i64, String, Option<String>)> {
        let mut st = con.prepare("SELECT id, importance, importance_source, importance_reason FROM deck ORDER BY id").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn outline_suggestion_and_regression() {
        // 덱 2: 하이라이트(1쪽) 4장 · 덱 3: 볼드 2장/4 · 덱 4: 볼드 3장/4 · 덱 5: 볼드(5 · 6쪽) 3장/10 · 덱 6: 하이라이트(8쪽) 4장
        let (con, cards) = outline_db(&[(2, 4), (3, 4), (4, 4), (5, 10), (6, 4)]);
        let s1 = add_seg(&con, 1, 0, "핵심 강조", "highlight");
        let s2 = add_seg(&con, 2, 0, "볼드 둘", "bold");
        let s3 = add_seg(&con, 3, 0, "볼드 셋", "bold");
        let s4 = add_seg(&con, 5, 0, "요약 볼드", "bold");
        let s5 = add_seg(&con, 6, 0, "요약 볼드 둘", "bold");
        let s6 = add_seg(&con, 8, 0, "다른 강조", "highlight");
        let links = [(2, vec![s1; 4]), (3, vec![s2; 2]), (4, vec![s3; 3]), (5, vec![s4, s4, s5]), (6, vec![s6; 4])];
        for (deck, segs) in &links {
            for (i, s) in segs.iter().enumerate() {
                link(&con, *s, cards[deck][i]).unwrap();
            }
        }
        let ids = "1,2,3,4,5,6";
        // 오늘의 결과 (제목 구간 없음): 덱 2 핵심 · 덱 5 보충
        assert_eq!(suggest(&con, ids).unwrap(), 2);
        let before = deck_rows(&con);
        let core2 = "자료 · 4장이 a.pdf의 하이라이트 구간과 연결 (덱 안 상위 20%)";
        assert_eq!(before[1], (2, 2, "material".into(), Some(core2.into())));
        assert_eq!(before[4], (5, 0, "material".into(), Some("자료 · 3장만 강조 구간과 연결 (덱 안 하위 20%)".into())));
        // 핵심 범위가 없는 제목만 있으면 바이트 단위로 같다
        add_seg(&con, 2, 0, "# 1장", "heading");
        add_seg(&con, 7, 0, "# 3장", "heading");
        assert_eq!(suggest(&con, ids).unwrap(), 2);
        assert_eq!(deck_rows(&con), before);

        // 핵심 범위: 1쪽 "핵심 정리"(1쪽만) · 5쪽 "2장 요약"(5–6쪽)
        add_seg(&con, 1, 0, "# 핵심 정리", "heading");
        add_seg(&con, 5, 0, "# 2장 요약", "heading");
        assert_eq!(suggest(&con, ids).unwrap(), 2);
        let rows = deck_rows(&con);
        let outline2 = "목차 · 이 하위 덱 카드 4장이 a.pdf의 '핵심 정리' 구간과 연결됐어요. 목차에서 핵심 범위로 본 구간이에요 (1쪽)";
        let outline5 = "목차 · 이 하위 덱 카드 3장이 a.pdf의 '2장 요약' 구간과 연결됐어요. 목차에서 핵심 범위로 본 구간이에요 (5–6쪽)";
        // 순위 제안이 핵심이면 줄을 더하고, 보충이면 목차가 이긴다. 나머지 덱은 그대로
        assert_eq!(rows[1], (2, 2, "material".into(), Some(format!("{core2}\n{outline2}"))));
        assert_eq!(rows[4], (5, 2, "material".into(), Some(outline5.into())));
        assert_eq!([&rows[0], &rows[2], &rows[3], &rows[5]], [&before[0], &before[2], &before[3], &before[5]]);
        // 기출 근거를 다시 붙여도 목차 줄은 남는다
        assert_eq!(crate::exam::suggest_all(&con).unwrap(), 0);
        // 사용자가 정한 덱은 덮지 않는다 (6.5.1)
        con.execute("UPDATE deck SET importance = 1, importance_source = 'user', importance_reason = NULL WHERE id = 5", []).unwrap();
        suggest(&con, ids).unwrap();
        assert_eq!(deck_rows(&con)[4], (5, 1, "user".into(), None));
        // 핵심 범위와 이어진 카드가 3장보다 적거나 자료와 이어진 카드의 절반보다 적으면 목차 근거를 붙이지 않는다
        con.execute("UPDATE deck SET importance_source = 'default' WHERE id = 5", []).unwrap();
        for c in &cards[&5][3..9] {
            link(&con, s6, *c).unwrap();
        }
        suggest(&con, ids).unwrap();
        assert!(deck_rows(&con)[4].3.as_deref().is_none_or(|r| !r.contains(OUTLINE_HEAD)));
    }

    #[test]
    fn rereading_outline_keeps_segments_and_links() {
        let (con, cards) = outline_db(&[(2, 1)]);
        let s = add_seg(&con, 3, 0, "강조", "highlight");
        link(&con, s, cards[&2][0]).unwrap();
        let head = |page, ord, text: &str, signal: &str| NewSegment { page, ord, text: text.into(), signals: vec![signal.into()], rects: None, before: None, after: None };
        let ids = |con: &Connection| -> Vec<i64> {
            let mut st = con.prepare("SELECT id FROM source_segment ORDER BY id").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        // 제목만 들어간다 (강조 신호가 섞인 구간은 버린다). 강조 구간 · 연결은 그대로
        save_headings(&con, 1, &[head(1, 0, "# 1장", "heading"), head(3, 0, "# 2장 요약", "heading"), head(2, 0, "몰래 강조", "highlight")]).unwrap();
        let first = ids(&con);
        assert_eq!(first.len(), 3);
        assert_eq!(segments_of(&con, 1).unwrap().iter().find(|x| x.id == s).unwrap().links.len(), 1);
        // 같은 제목이면 쓰지 않는다 (행 id 그대로, 동기화 변경 없음). 다르면 제목만 바꾼다
        save_headings(&con, 1, &[head(3, 0, "# 2장 요약", "heading"), head(1, 0, "# 1장", "heading")]).unwrap();
        assert_eq!(ids(&con), first);
        save_headings(&con, 1, &[head(1, 0, "# 1장 개요", "heading")]).unwrap();
        let texts: Vec<String> = segments_of(&con, 1).unwrap().into_iter().map(|x| x.text).collect();
        assert_eq!(texts, vec!["# 1장 개요", "강조"]);
        // 아직 읽지 않은 자료는 받지 않는다 (처음 읽기가 모두 넣는다)
        con.execute("UPDATE source_material SET status = 'pending'", []).unwrap();
        save_headings(&con, 1, &[head(1, 0, "# 다른 제목", "heading")]).unwrap();
        assert_eq!(segments_of(&con, 1).unwrap()[0].text, "# 1장 개요");
    }

    #[test]
    fn grading_context_skips_headings() {
        // R2 채점 문맥 후보 SQL (rag.rs similar_segments) 을 모델 없이 그대로 돌린다: 목차 제목만 빠지고 그림 · 표는 지금처럼 남는다
        let sql = include_str!("rag.rs").lines().find(|l| l.contains("SELECT s.id, s.text FROM source_segment s")).unwrap().trim().trim_matches('"').replace("{ids}", "1");
        let (con, _) = outline_db(&[]);
        let a = add_seg(&con, 1, 0, "강조", "highlight");
        let f = add_seg(&con, 1, 1, "그림 · 1쪽", "figure");
        add_seg(&con, 1, -1, "## 3장 요약", "heading");
        let mut st = con.prepare(&sql).unwrap();
        let mut got: Vec<i64> = st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        got.sort();
        assert_eq!(got, vec![a, f]);
    }

    #[test]
    fn outline_levels_start_at_one() {
        // 구역 없는 PPTX: 제목이 모두 2단계("## ")로 저장된다. 화면 단계만 1부터, 구간 · 끝 쪽 · 핵심 범위는 1단계로 저장한 것과 같다
        let build = |marks: [&str; 3]| {
            let (con, _) = outline_db(&[]);
            add_seg(&con, 1, 0, "강조 하나", "highlight");
            add_seg(&con, 2, 0, "요약 강조", "highlight");
            add_seg(&con, 3, 0, "세부 강조", "bold");
            add_seg(&con, 4, 0, "다른 강조", "red");
            for (i, (page, title)) in [(1, "소개"), (2, "자료구조 요약"), (3, "세부")].into_iter().enumerate() {
                add_seg(&con, page, -1, &format!("{} {title}", marks[i]), "heading");
            }
            add_seg(&con, 4, -1, &format!("{} 트리", marks[0]), "heading");
            let o = outline_of(&con, 1).unwrap();
            let rows: Vec<_> = o.sections.iter().map(|s| (s.level, s.title.clone(), s.page, s.end_page, s.segments, s.core.clone())).collect();
            let mut core: Vec<(String, usize)> = segments_of(&con, 1).unwrap().into_iter().filter_map(|s| o.core.get(&s.id).map(|k| (s.text, *k))).collect();
            core.sort();
            (rows, core)
        };
        let deep = build(["##", "##", "###"]);
        // PPTX 출처 줄: 제목만 · 구역만 · 둘 다
        let (con, _) = outline_db(&[]);
        con.execute("UPDATE source_material SET kind = 'pptx'", []).unwrap();
        add_seg(&con, 1, -1, "## 스택", "heading");
        assert_eq!(outline_of(&con, 1).unwrap().source, "slide_title");
        add_seg(&con, 1, -1, "# 기초", "heading");
        assert_eq!(outline_of(&con, 1).unwrap().source, "slide");
        con.execute("DELETE FROM source_segment WHERE text = '## 스택'", []).unwrap();
        assert_eq!(outline_of(&con, 1).unwrap().source, "slide_section");
        assert_eq!(deep, build(["#", "#", "##"]));
        assert_eq!(deep.0.iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 1, 2, 1]);
        assert_eq!(deep.0[1], (1, "자료구조 요약".to_string(), 2, 3, 2, Some("제목에 '요약'".to_string())));
        assert_eq!(deep.1, vec![("세부 강조".to_string(), 1), ("요약 강조".to_string(), 1)]);
    }

    /// QA T6-05: 분석 중에 들어온 요청은 오류가 아니라 줄에 서고, 지금 분석이 끝나면 차례로 돈다. 줄이 비면 깃발을 내린다
    #[test]
    fn analysis_requests_during_analysis_queue_up() {
        assert!(begin(7));
        assert!(!begin(8));
        assert!(!begin(8), "같은 덱은 한 번만 선다");
        assert!(!begin(7), "지금 도는 덱도 다시 선다 (새 자료가 빠졌을 수 있다)");
        assert_eq!((next_queued(), next_queued(), next_queued()), (Some(8), Some(7), None));
        assert!(!ANALYZING.load(Ordering::SeqCst));
        assert!(begin(9), "끝난 뒤에는 바로 시작한다");
        assert_eq!(next_queued(), None);
    }

    /// QA T6-06: 슬라이드 제목의 굵은 글씨(제목과 글이 같은 볼드뿐인 구간)는 자동 연결 후보가 아니다. 다른 쪽 · 다른 신호는 남는다
    #[test]
    fn title_bold_is_not_a_link_candidate() {
        let (con, _) = outline_db(&[]);
        add_seg(&con, 1, 0, "네트워크 3강: 전송 계층", "bold");
        add_seg(&con, 1, -1, "## 네트워크 3강: 전송 계층", "heading");
        add_seg(&con, 1, 1, "TCP", "bold");
        add_seg(&con, 2, 0, "네트워크 3강: 전송 계층", "bold");
        add_seg(&con, 3, 0, "흐름 제어", "highlight,bold");
        add_seg(&con, 3, -1, "## 흐름 제어", "heading");
        add_seg(&con, 4, 0, "그림 · 4쪽", "figure");
        let got: Vec<(i64, String)> = link_candidates(segments_of(&con, 1).unwrap()).into_iter().map(|s| (s.page, s.text)).collect();
        assert_eq!(got, vec![(1, "TCP".into()), (2, "네트워크 3강: 전송 계층".into()), (3, "흐름 제어".into())]);
    }

    /// QA T6-07: 같은 덱(하위 덱 포함)에 바이트가 같은 자료가 있으면 찾는다. 내용이 다르거나 다른 덱이면 없다
    #[test]
    fn same_file_is_found_by_bytes_in_the_deck_tree() {
        let dir = std::env::temp_dir().join(format!("na-source-same-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sources")).unwrap();
        std::fs::write(dir.join("sources/1.pdf"), b"PDF-A").unwrap();
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, 'D', 0, 0), (2, '하위', 0, 0), (3, '다른', 0, 0);
             UPDATE deck SET parent_id = 1 WHERE id = 2;
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (1, 2, 'lecture.pdf', 'sources/1.pdf', 'pdf', 0, 0),
               (2, 1, '없는 파일.pdf', 'sources/9.pdf', 'pdf', 0, 0);",
        )
        .unwrap();
        assert_eq!(same_file(&con, &dir, 1, "pdf", b"PDF-A").unwrap().as_deref(), Some("lecture.pdf"));
        assert_eq!(same_file(&con, &dir, 1, "pdf", b"PDF-B").unwrap(), None);
        assert_eq!(same_file(&con, &dir, 1, "pptx", b"PDF-A").unwrap(), None);
        assert_eq!(same_file(&con, &dir, 3, "pdf", b"PDF-A").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn order_follows_shared_creation_not_local_id() {
        // 다른 기기에서 먼저 만든 자료가 동기화로 나중에 들어오면 이 기기의 id 가 더 크다 (Q7). 목록 · 연결 순서는 만든 시각 → uid
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, 'D', 0, 0);
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, status, created_at, updated_at, uid) VALUES
               (1, 1, '나중.pdf', 'sources/1.pdf', 'pdf', 'ready', 200, 0, 'b'), (2, 1, '먼저.pdf', 'sources/2.pdf', 'pdf', 'ready', 100, 0, 'a'),
               (3, 1, '같은 시각.pdf', 'sources/3.pdf', 'pdf', 'ready', 100, 0, '0');
             INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight) VALUES (1, 2, 1, 0, '가', 'highlight', 3), (2, 2, 2, 0, '나', 'highlight', 3);",
        )
        .unwrap();
        let names: Vec<String> = materials_in(&con, 1).unwrap().into_iter().map(|m| m.file_name).collect();
        assert_eq!(names, ["같은 시각.pdf", "먼저.pdf", "나중.pdf"]);
        let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic'", [], |r| r.get(0)).unwrap();
        for id in [1, 2] {
            con.execute("INSERT INTO note (id, note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, '[\"q\",\"a\"]', 0, 0)", params![id, nt]).unwrap();
            con.execute("INSERT INTO card (id, note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?1, 1, 0, 0, 0)", [id]).unwrap();
        }
        // 직접 연결 둘: id 는 카드 1 · 2쪽 구간이 먼저지만 만든 것은 카드 2 · 1쪽 구간이 먼저
        con.execute_batch(
            "INSERT INTO source_link (id, segment_id, card_id, method, created_at, uid) VALUES (1, 1, 1, 'manual', 300, 'y'), (2, 1, 2, 'manual', 100, 'x'),
               (3, 2, 1, 'manual', 50, 'w');",
        )
        .unwrap();
        let links: Vec<i64> = segments_of(&con, 2).unwrap()[0].links.iter().map(|l| l.card_id).collect();
        assert_eq!(links, [2, 1]);
        // I2 원자료 · 넣을 곳 기본값도 먼저 만든 연결
        assert_eq!(for_card(&con, 1).unwrap().unwrap().segment.id, 2);
        con.execute("INSERT INTO deck (id, parent_id, name, created_at, updated_at) VALUES (2, 1, '하위', 0, 0)", []).unwrap();
        con.execute("UPDATE card SET deck_id = 2 WHERE id = 2", []).unwrap();
        assert_eq!(nearest_deck(&con, 2, 1).unwrap(), Some(2));
    }
}
