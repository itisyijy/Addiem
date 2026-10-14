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
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 신호와 가중치 (post-mvp 5장). 하이라이트·밑줄·색은 사용자가 일부러 남긴 것이라 높고,
/// 볼드는 원본 서식일 수 있어 낮다. 배열 순서가 곧 신뢰도 순이다 (I1 배지 순서)
pub const SIGNALS: [(&str, f64); 5] = [("highlight", 3.0), ("underline", 3.0), ("red", 3.0), ("star", 2.0), ("bold", 1.0)];
const SIGNAL_LABEL: [&str; 5] = ["하이라이트", "밑줄", "붉은 글씨", "★", "볼드"];

/// 자동 연결 기준 (multilingual-e5-small 코사인 유사도). 이보다 낮으면 잇지 않고 비워 둔다 (억지 연결 금지, post-mvp 6장).
/// 실측 (2026-10-14, 한국어 카드 20장 · 강조 구간 12곳): 맞는 짝 0.89~0.99, 틀린 짝 0.85~0.875 (짧은 제목에 끌림).
/// ponytail: 고정값. 자료가 쌓이면 덱별 분포로 조정한다
pub const AUTO_LINK: f32 = 0.88;

/// 알려진 신호만 신뢰도 순으로
fn sorted_signals<S: AsRef<str>>(signals: &[S]) -> Vec<&'static str> {
    SIGNALS.iter().map(|s| s.0).filter(|s| signals.iter().any(|x| x.as_ref() == *s)).collect()
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
}

const MATERIAL_SQL: &str = "SELECT m.id, m.deck_id, m.file_name, m.kind, m.pages, m.status,
        (SELECT count(*) FROM source_segment s WHERE s.material_id = m.id),
        (SELECT count(DISTINCT s.id) FROM source_segment s JOIN source_link l ON l.segment_id = s.id
         WHERE s.material_id = m.id AND l.method != 'rejected')
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
    })
}

fn subtree(con: &Connection, deck_id: i64) -> Res<String> {
    let decks = load_decks(con)?;
    Ok(descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","))
}

/// 이 덱과 하위 덱에 붙은 자료
#[tauri::command]
pub fn source_list(state: State<AppState>, deck_id: i64) -> Res<Vec<Material>> {
    let con = state.db.lock().map_err(err)?;
    let ids = subtree(&con, deck_id)?;
    let mut st = con.prepare(&format!("{MATERIAL_SQL} WHERE m.deck_id IN ({ids}) ORDER BY m.id")).map_err(err)?;
    let rows = st.query_map([], material_row).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    Ok(rows)
}

fn material(con: &Connection, id: i64) -> Res<Material> {
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
    let slides = if kind == "pptx" { Some(pptx_slides(&bytes)) } else { None };
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("자료.{kind}"));
    let mut con = state.db.lock().map_err(err)?;
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
        let slides = slides.unwrap_or_default();
        let segs = pptx_segments(&slides);
        let readable = slides.iter().flatten().any(|p| !p.text.is_empty());
        save_segments(&tx, id, slides.len() as i64, &segs, readable)?;
    }
    tx.commit().map_err(err)?;
    material(&con, id)
}

/// 자료를 지운다. 카드는 그대로, 연결만 사라진다 (CASCADE). 복사본 파일도 지운다
#[tauri::command]
pub fn source_delete(state: State<AppState>, material_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    let rel: Option<String> =
        con.query_row("SELECT rel_path FROM source_material WHERE id = ?1", [material_id], |r| r.get(0)).optional().map_err(err)?;
    con.execute("DELETE FROM source_material WHERE id = ?1", [material_id]).map_err(err)?;
    if let Some(rel) = rel.filter(|r| r.starts_with("sources/")) {
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
    let bytes = std::fs::read(state.data_dir.join(rel)).map_err(|_| "이 기기에는 자료 파일이 없어요.".to_string())?;
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
    let mut st = con
        .prepare(
            "INSERT INTO source_segment (material_id, page, ord, text, signals, weight, rects, before_text, after_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(err)?;
    for s in segs {
        let signals = sorted_signals(&s.signals);
        if signals.is_empty() || s.text.trim().is_empty() {
            continue;
        }
        let rects = s.rects.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default());
        st.execute(params![material_id, s.page, s.ord, s.text.trim(), signals.join(","), weight_of(&signals), rects, s.before, s.after])
            .map_err(err)?;
    }
    con.execute(
        "UPDATE source_material SET pages = ?1, status = ?2, updated_at = ?3 WHERE id = ?4",
        params![pages, if readable { "ready" } else { "unsupported" }, now_ms(), material_id],
    )
    .map_err(err)?;
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

#[derive(Serialize)]
pub struct Link {
    card_id: i64,
    /// manual: 직접 연결 / auto: 자동 연결
    method: String,
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
}

const SEGMENT_SQL: &str = "SELECT id, page, ord, text, signals, weight, rects, before_text, after_text FROM source_segment";

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
    })
}

fn segments_of(con: &Connection, material_id: i64) -> Res<Vec<Segment>> {
    let mut st = con.prepare(&format!("{SEGMENT_SQL} WHERE material_id = ?1 ORDER BY page, ord")).map_err(err)?;
    let mut segs: Vec<Segment> = st.query_map([material_id], segment_row).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    let mut st = con
        .prepare(
            "SELECT l.segment_id, l.card_id, l.method FROM source_link l JOIN source_segment s ON s.id = l.segment_id
             WHERE s.material_id = ?1 AND l.method != 'rejected' ORDER BY l.method = 'manual' DESC, l.score DESC, l.id",
        )
        .map_err(err)?;
    let mut links: HashMap<i64, Vec<Link>> = HashMap::new();
    for row in st.query_map([material_id], |r| Ok((r.get::<_, i64>(0)?, Link { card_id: r.get(1)?, method: r.get(2)? }))).map_err(err)? {
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

#[derive(Serialize)]
pub struct CardSource {
    material: Material,
    segment: Segment,
    method: String,
}

/// I2 학습 중 원자료. 직접 연결을 먼저, 그다음 유사도가 높은 것. 없으면 null (버튼을 숨긴다)
#[tauri::command]
pub fn source_for_card(state: State<AppState>, card_id: i64) -> Res<Option<CardSource>> {
    let con = state.db.lock().map_err(err)?;
    let hit: Option<(i64, i64, String)> = con
        .query_row(
            "SELECT s.material_id, s.id, l.method FROM source_link l JOIN source_segment s ON s.id = l.segment_id
             WHERE l.card_id = ?1 AND l.method != 'rejected' ORDER BY l.method = 'manual' DESC, l.score DESC, l.id LIMIT 1",
            [card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(err)?;
    let Some((material_id, segment_id, method)) = hit else { return Ok(None) };
    let segment = segments_of(&con, material_id)?.into_iter().find(|s| s.id == segment_id).ok_or("구간을 찾을 수 없어요.")?;
    Ok(Some(CardSource { material: material(&con, material_id)?, segment, method }))
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
}

/// 슬라이드별 문단과 강조 신호. 순서는 presentation.xml 의 sldIdLst (파일 이름 순서와 다를 수 있다)
pub fn pptx_slides(bytes: &[u8]) -> Res<Vec<Vec<Para>>> {
    let bad = || "PPTX 파일을 열 수 없어요.".to_string();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| bad())?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let mut read = |name: &str| -> Option<String> {
        let mut s = String::new();
        zip.by_name(name).ok()?.read_to_string(&mut s).ok()?;
        Some(s)
    };
    let rels = read("ppt/_rels/presentation.xml.rels").ok_or_else(bad)?;
    let pres = read("ppt/presentation.xml").ok_or_else(bad)?;
    let mut targets = HashMap::new();
    for (id, target) in xml_attrs(&rels, "Relationship", &["Id", "Target"]).into_iter().map(|mut v| (v.remove(0), v.remove(0))) {
        let path = match target.strip_prefix('/') {
            Some(abs) => abs.to_string(),
            None => format!("ppt/{target}"),
        };
        targets.insert(id, path);
    }
    let mut order: Vec<String> = xml_attrs_ns(&pres, "sldId", "id").into_iter().filter_map(|rid| targets.get(&rid).cloned()).collect();
    if order.is_empty() {
        // 목록이 없으면 slideN.xml 의 N 순서로
        let num = |n: &str| n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse::<u32>().ok();
        order = names.into_iter().filter(|n| num(n).is_some()).collect();
        order.sort_by_key(|n| num(n));
    }
    Ok(order.iter().map(|p| read(p).map(|x| parse_slide(&x)).unwrap_or_default()).collect())
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
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut paras = vec![];
    let mut cur: Option<Para> = None;
    let mut run = Run::default();
    let (mut in_rpr, mut in_fill, mut in_hl, mut in_t) = (false, false, false, false);
    let mark = |p: &mut Para, run: Run| {
        for (on, sig) in [(run.highlight, "highlight"), (run.underline, "underline"), (run.red, "red"), (run.bold, "bold")] {
            if on && !p.signals.contains(&sig) {
                p.signals.push(sig);
            }
        }
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
                    "p" if open => cur = Some(Para::default()),
                    "r" | "fld" => run = Run::default(),
                    "rPr" => {
                        run.bold = attr("b").is_some_and(|v| v == "1" || v == "true");
                        run.underline = attr("u").is_some_and(|v| v != "none");
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

/// I1 미리보기 (PPTX). 슬라이드 하나의 모든 문단과 신호
#[tauri::command]
pub fn source_slide(state: State<AppState>, material_id: i64, page: i64) -> Res<Vec<Para>> {
    let rel: String = {
        let con = state.db.lock().map_err(err)?;
        con.query_row("SELECT rel_path FROM source_material WHERE id = ?1 AND kind = 'pptx'", [material_id], |r| r.get(0)).map_err(err)?
    };
    let bytes = std::fs::read(state.data_dir.join(rel)).map_err(|_| "이 기기에는 자료 파일이 없어요.".to_string())?;
    let mut slides = pptx_slides(&bytes)?;
    let i = usize::try_from(page - 1).map_err(err)?;
    Ok(if i < slides.len() { slides.swap_remove(i) } else { vec![] })
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
}

#[tauri::command]
pub async fn source_analyze(app: AppHandle, deck_id: i64) -> Res<Analysis> {
    if ANALYZING.swap(true, Ordering::SeqCst) {
        return Err("이미 자료를 분석하고 있어요.".into());
    }
    let res = tauri::async_runtime::spawn_blocking(move || analyze(&app, deck_id)).await.map_err(err);
    ANALYZING.store(false, Ordering::SeqCst);
    *PROGRESS.lock().map_err(err)? = None;
    res?
}

fn text_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// 캐시에 없거나 글이 바뀐 것만 계산한다. 계산하는 동안에는 DB 를 잠그지 않는다
fn embed_cached(app: &AppHandle, kind: &str, items: &[(i64, String)], mut tick: impl FnMut(usize)) -> Res<HashMap<i64, Vec<f32>>> {
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

/// 카드 글 (앞 + 뒤, 서식 뺌). 카드 목록(C2)과 같은 방식으로 렌더한다
fn card_texts(con: &Connection, ids: &str) -> Res<Vec<(i64, i64, String)>> {
    let names = note_field_names(con)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.deck_id, n.fields, c.template_ord, nt.name, t.qfmt, t.afmt
             FROM card c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id
             LEFT JOIN card_template t ON t.note_type_id = n.note_type_id
                  AND t.ord = (CASE WHEN nt.name = 'Cloze' OR t.qfmt LIKE '%{{{{cloze:%' THEN 0 ELSE c.template_ord END)
             WHERE c.deck_id IN ({ids}) ORDER BY c.id"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| {
            let fields: String = r.get(2)?;
            let nt: String = r.get(4)?;
            let q: Option<String> = r.get(5)?;
            let a: Option<String> = r.get(6)?;
            let rendered = render_fields(&names, &nt, &fields, q.as_deref(), a.as_deref(), r.get(3)?);
            let text = format!("{} {}", render::strip_tags(&rendered.front), render::strip_tags(answer_part(&rendered.back)));
            Ok((r.get(0)?, r.get(1)?, text.chars().take(1000).collect()))
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
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
            .prepare(&format!("SELECT id, file_name FROM source_material WHERE deck_id IN ({ids}) AND status = 'ready' ORDER BY id"))
            .map_err(err)?;
        let materials: Vec<(i64, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        let mut segs = vec![];
        for (m, _) in &materials {
            segs.push(segments_of(&con, *m)?);
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
    let card_items: Vec<(i64, String)> = cards.iter().map(|c| (c.0, c.2.clone())).collect();
    let card_vecs = embed_cached(app, "card", &card_items, |n| progress(0, &format!("카드 {}장", cards.len()), n))?;
    let mut seg_vecs = HashMap::new();
    for (i, ((_, name), list)) in materials.iter().zip(&segs).enumerate() {
        progress(i + 1, name, 0);
        let items: Vec<(i64, String)> = list.iter().map(|s| (s.id, s.text.clone())).collect();
        seg_vecs.extend(embed_cached(app, "segment", &items, |n| progress(i + 1, name, n))?);
    }

    let con = state.db.lock().map_err(err)?;
    // 사용자가 직접 이은 카드와 끊은 연결은 그대로 둔다. 자동 연결만 새로 만든다
    let seg_ids = segs.iter().flatten().map(|s| s.id.to_string()).collect::<Vec<_>>().join(",");
    let seg_ids = if seg_ids.is_empty() { "-1".to_string() } else { seg_ids };
    con.execute(&format!("DELETE FROM source_link WHERE method = 'auto' AND segment_id IN ({seg_ids})"), []).map_err(err)?;
    let manual: std::collections::HashSet<i64> = {
        let mut st = con.prepare("SELECT DISTINCT card_id FROM source_link WHERE method = 'manual'").map_err(err)?;
        st.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    let card_refs: Vec<(i64, &[f32])> =
        cards.iter().filter(|c| !manual.contains(&c.0)).filter_map(|c| card_vecs.get(&c.0).map(|v| (c.0, v.as_slice()))).collect();
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
    Ok(Analysis { linked: links.len(), suggested })
}

/// 하위 덱 중요도 제안. 제안을 붙인 덱 수
fn suggest(con: &Connection, ids: &str) -> Res<usize> {
    // 하위 덱 점수: 카드마다 이어진 구간 가중치 합의 평균. 근거 문장에 쓸 자료 이름과 신호도 모은다
    let mut st = con
        .prepare(&format!(
            "SELECT c.deck_id, c.id, s.weight, m.file_name, s.signals FROM card c
             JOIN source_link l ON l.card_id = c.id AND l.method != 'rejected'
             JOIN source_segment s ON s.id = l.segment_id JOIN source_material m ON m.id = s.material_id
             WHERE c.deck_id IN ({ids}) AND m.deck_id IN ({ids})"
        ))
        .map_err(err)?;
    #[derive(Default)]
    struct Acc {
        cards: std::collections::HashSet<i64>,
        weight: f64,
        files: HashMap<String, std::collections::HashSet<i64>>,
        signals: HashMap<String, usize>,
        /// 8.4 기출 카드 (자료와 이어지지 않았어도 센다)
        exam: std::collections::HashSet<i64>,
    }
    let mut acc: HashMap<i64, Acc> = HashMap::new();
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))
        .map_err(err)?;
    for row in rows {
        let (deck, card, w, file, signals) = row.map_err(err)?;
        let a = acc.entry(deck).or_default();
        a.cards.insert(card);
        a.weight += w;
        a.files.entry(file).or_default().insert(card);
        *a.signals.entry(signals.split(',').next().unwrap_or_default().to_string()).or_default() += 1;
    }
    drop(st);
    // 8.4 기출 카드는 강조 구간 하나만큼 더한다 (카드마다 한 번)
    let mut st = con
        .prepare(&format!("SELECT c.deck_id, c.id FROM card c JOIN note n ON n.id = c.note_id WHERE c.deck_id IN ({ids}) AND {}", exam::IS_EXAM))
        .map_err(err)?;
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(err)? {
        let (deck, card) = row.map_err(err)?;
        let a = acc.entry(deck).or_default();
        a.exam.insert(card);
        a.weight += exam::EXAM_WEIGHT;
    }
    drop(st);
    let mut st = con.prepare(&format!("SELECT deck_id, count(*) FROM card WHERE deck_id IN ({ids}) GROUP BY deck_id")).map_err(err)?;
    let counts: Vec<(i64, usize)> = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    drop(st);
    let scores: Vec<DeckScore> = counts
        .iter()
        .map(|(id, n)| {
            let a = acc.get(id);
            DeckScore {
                deck_id: *id,
                linked: a.map_or(0, |a| a.cards.union(&a.exam).count()),
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
    let mut suggested = 0;
    for (deck, level, pct) in picks {
        let Some(a) = acc.get(&deck) else { continue };
        let (file, _) = a.files.iter().max_by_key(|(name, c)| (c.len(), std::cmp::Reverse((*name).clone()))).map(|(n, c)| (n.clone(), c.len())).unwrap_or_default();
        let signal = a.signals.iter().max_by_key(|(_, n)| **n).map(|(s, _)| s.as_str()).unwrap_or_default();
        let label = SIGNALS.iter().position(|s| s.0 == signal).map_or("강조", |i| SIGNAL_LABEL[i]);
        let reason = if level == 2 && !a.exam.is_empty() && a.cards.is_empty() {
            format!("이 덱 카드 {}장이 기출로 표시됐어요 (덱 안 상위 {pct}%)", a.exam.len())
        } else if level == 2 && !a.exam.is_empty() {
            format!("이 덱 카드 {}장이 기출이고 {}장이 {file}의 {label} 구간과 연결됐어요 (덱 안 상위 {pct}%)", a.exam.len(), a.cards.len())
        } else if level == 2 {
            format!("이 덱 카드 {}장이 {file}의 {label} 구간과 연결됐어요 (덱 안 상위 {pct}%)", a.cards.len())
        } else {
            format!("이 덱 카드 {}장만 자료의 강조 구간과 연결됐어요 (덱 안 하위 {pct}%)", a.cards.len())
        };
        suggested += con
            .execute(
                "UPDATE deck SET importance = ?1, importance_source = 'material', importance_reason = ?2 WHERE id = ?3 AND importance_source = 'default'",
                params![level, reason, deck],
            )
            .map_err(err)?;
    }
    Ok(suggested)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 슬라이드 두 장짜리 최소 PPTX. 순서는 presentation.xml 이 정한다 (slide2 가 먼저)
    fn tiny_pptx() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(vec![]);
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        let files = [
            (
                "ppt/presentation.xml",
                r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId id="256" r:id="rId3"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst></p:presentation>"#,
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
                </p:txBody></p:sld>"#,
            ),
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
        assert_eq!(slides[0], vec![Para { text: "★ 기출 & 핵심".into(), signals: vec!["underline", "star"] }]);
        let s = &slides[1];
        assert_eq!(s[0].signals, Vec::<&str>::new());
        assert_eq!(s[1], Para { text: "로지스틱 회귀 — 종속변수가 범주형".into(), signals: vec!["bold"] });
        assert_eq!(s[2].signals, vec!["red"]);
        assert_eq!(s[3].signals, vec!["highlight"]);
        assert_eq!(s[4].signals, Vec::<&str>::new());
        let segs = pptx_segments(&slides);
        assert_eq!(segs.len(), 4);
        assert_eq!((segs[1].page, segs[1].before.as_deref(), segs[1].after.as_deref()), (2, Some("평범한 문장"), Some("다중공선성 주의")));
        assert_eq!(weight_of(&sorted_signals(&segs[0].signals)), 5.0);
        assert!(pptx_slides(b"not a zip").is_err());
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
    fn exam_marks_join_material_suggestion() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '시험', 0, 0);
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (1, 1, 'a.pdf', 'sources/1.pdf', 'pdf', 0, 0);
             INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight)
               VALUES (1, 1, 1, 0, '강조', 'highlight', 3), (2, 1, 1, 1, '굵게', 'bold', 1), (3, 1, 2, 0, '기출 구간', 'bold', 1);",
        )
        .unwrap();
        let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic'", [], |r| r.get(0)).unwrap();
        // (덱, 카드 수, 첫 카드에 이을 구간, 노트에 기출 표시)
        let mut id = 0;
        for (deck, n, seg, exam) in [(2, 2, None, true), (3, 2, Some(1), false), (4, 1, Some(2), false), (5, 1, Some(3), false), (6, 4, Some(2), false)] {
            con.execute("INSERT INTO deck (id, parent_id, name, created_at, updated_at) VALUES (?1, 1, ?1, 0, 0)", [deck]).unwrap();
            for i in 0..n {
                id += 1;
                con.execute(
                    "INSERT INTO note (id, note_type_id, fields, exam_hits, created_at, updated_at) VALUES (?1, ?2, '[\"q\",\"a\"]', ?3, 0, 0)",
                    params![id, nt, exam.then_some(1)],
                )
                .unwrap();
                con.execute("INSERT INTO card (id, note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?1, ?2, 0, 0, 0)", [id, deck]).unwrap();
                if let (0, Some(s)) = (i, seg) {
                    link(&con, s, id).unwrap();
                }
            }
        }
        let row = |deck: i64| -> (i64, Option<String>) {
            con.query_row("SELECT importance, importance_reason FROM deck WHERE id = ?1", [deck], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        };
        // 기출 구간과 이은 카드: 볼드(1) + 기출(3) = 4 로 맨 위. 노트 기출 덱 2 는 6 / 2 = 3
        con.execute("UPDATE source_segment SET exam_hits = 1 WHERE id = 3", []).unwrap();
        assert_eq!(suggest(&con, "1,2,3,4,5,6").unwrap(), 2);
        assert_eq!(row(5), (2, Some("이 덱 카드 1장이 기출이고 1장이 a.pdf의 볼드 구간과 연결됐어요 (덱 안 상위 20%)".into())));
        assert_eq!(row(6).0, 0);
        // 구간의 기출 표시를 지우면 덱 5 는 1, 자료와 잇지 않은 기출 카드만 있는 덱 2 가 맨 위
        con.execute("UPDATE source_segment SET exam_hits = 0 WHERE id = 3", []).unwrap();
        assert_eq!(suggest(&con, "1,2,3,4,5,6").unwrap(), 2);
        assert_eq!(row(2), (2, Some("이 덱 카드 2장이 기출로 표시됐어요 (덱 안 상위 20%)".into())));
        assert_eq!(row(5), (1, None));
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
}
