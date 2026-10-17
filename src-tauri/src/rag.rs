//! R · 자료 근거 AI (RAG). 찾기는 모두 기기 안(source_segment · source_link · e5 임베딩)이고 ChatGPT 는 글을 만들 때만 쓴다 (ai.rs).
//! 보내는 것: 사용자가 미리보기에서 본 구간 글(1구간 600자에서 자름)과 기능마다 정한 카드 글뿐.
//! 자료 파일 · 그림 · 다른 카드 · 복습 기록 · 덱 이름 · 자료 파일 이름과 쪽은 보내지 않는다.
//! 구간은 번호로만 보내고, 근거 칩은 응답의 번호를 기기 안에서 자료 · 쪽으로 되돌려 만든다. 결과는 모두 제안이다 (자동 적용 없음)

use crate::ai::{self, AiError};
use crate::commands::{self, AppState, NewCard};
use crate::db::now_ms;
use crate::{embed, quality, render, source, written};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 1구간 최대 글자. 넘으면 자르고 "…(잘림)" (미리보기에도 그대로)
const CLIP: usize = 600;
const CUT: &str = "…(잘림)";
/// R1 한 번에 고를 수 있는 구간
pub const MAX_SEGMENTS: usize = 10;
/// R1 초안 카드 수 (형식 검증)
const MAX_DRAFTS: usize = 30;
/// R1-c "비슷한 카드가 있어요": 이 덱 카드와의 e5 유사도 (자동 연결 I3 과 같은 기준)
const SIMILAR_CARD: f32 = source::AUTO_LINK;
/// R1-c "근거 구간에 없는 말이 있어요": 뒷면 문장과 근거 구간의 유사도가 이보다 낮으면 짚는다.
/// 실측 (2026-10-15, 손으로 쓴 초안 문장 25개 · 구간 5개): 근거 있는 문장 0.872~0.969, 없는 문장 0.809~0.912.
/// 0.86 이면 없는 문장 14개 중 10개를 짚고 있는 문장은 짚지 않는다. ponytail: 숫자 · 용어만 바뀐 문장은 유사도로 못 잡는다
const UNGROUNDED: f32 = 0.86;

fn with_db<T>(app: &AppHandle, f: impl FnOnce(&Connection) -> Res<T>) -> Result<T, AiError> {
    let st = app.state::<AppState>();
    let con = st.db.lock().map_err(|_| AiError::Other("데이터베이스를 잠그지 못했어요".into()))?;
    f(&con).map_err(AiError::Other)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ---------------------------------------------------------------- 보내는 구간 (R 공통 "OpenAI로 보낼 내용")

/// 보낼 구간 하나. text 는 보내는 글자 그대로다 (잘렸으면 끝에 "…(잘림)"). 자료 이름 · 쪽 · 신호는 화면에만 쓴다
#[derive(Serialize, Clone, Debug)]
pub struct Sent {
    pub id: i64,
    pub material_id: i64,
    pub material: String,
    /// pdf | pptx (쪽 · 장)
    pub kind: String,
    pub page: i64,
    pub signals: Vec<String>,
    pub text: String,
    /// manual: 직접 연결 · auto: 자동 연결 · similar: 비슷한 구간 · picked: 직접 고름 (R1)
    pub how: String,
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    format!("{}{CUT}", t.chars().take(max).collect::<String>().trim_end())
}

/// 구간들을 보내는 모양으로 (ids 순서대로). 없는 구간은 빠진다
pub(crate) fn sent_of(con: &Connection, ids: &[(i64, &str)]) -> Res<Vec<Sent>> {
    let mut out = vec![];
    let mut st = con
        .prepare(
            "SELECT s.material_id, m.file_name, m.kind, s.page, s.signals, s.text FROM source_segment s
             JOIN source_material m ON m.id = s.material_id WHERE s.id = ?1",
        )
        .map_err(err)?;
    for (id, how) in ids {
        let row = st.query_row([id], |r| {
            let signals: String = r.get(4)?;
            Ok(Sent {
                id: *id,
                material_id: r.get(0)?,
                material: r.get(1)?,
                kind: r.get(2)?,
                page: r.get(3)?,
                signals: signals.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                text: clip(&r.get::<_, String>(5)?, CLIP),
                how: how.to_string(),
            })
        });
        match row {
            Ok(s) if !out.iter().any(|x: &Sent| x.id == s.id) => out.push(s),
            Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(err(e)),
        }
    }
    Ok(out)
}

/// 모델에 가는 구간 글: 번호와 글만 ("[1] …")
pub(crate) fn segments_input(sent: &[Sent]) -> String {
    sent.iter().enumerate().map(|(i, s)| format!("[{}] {}\n", i + 1, s.text)).collect()
}

/// 응답의 구간 번호(1부터) → 구간 id. 보낸 구간 밖이면 None
pub(crate) fn segment_at(sent: &[Sent], n: &J) -> Option<i64> {
    let i = n.as_u64().filter(|&i| i >= 1)? as usize;
    sent.get(i - 1).map(|s| s.id)
}

/// R3 성격별로 묶기에 함께 보낼 구간: 이 카드에 연결된 것 최대 3개 (직접 > 자동). 아무것도 보내지 않는다
#[tauri::command]
pub fn rag_linked(state: State<AppState>, card_id: i64) -> Res<Vec<Sent>> {
    let con = state.db.lock().map_err(err)?;
    sent_of(&con, &linked_segments(&con, card_id)?)
}

/// R1-b 미리보기: 고른 구간을 보내는 모양으로 (R1 은 매번 이 목록을 보고 보낸다)
#[tauri::command]
pub fn rag_preview(state: State<AppState>, segment_ids: Vec<i64>) -> Res<Vec<Sent>> {
    let con = state.db.lock().map_err(err)?;
    sent_of(&con, &segment_ids.iter().map(|id| (*id, "picked")).collect::<Vec<_>>())
}

// ---------------------------------------------------------------- R1 자료에서 카드 초안

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Similar {
    card_id: i64,
    question: String,
    score: f32,
}

#[derive(Serialize, Debug)]
pub struct DraftCard {
    front: String,
    back: String,
    segment_id: i64,
    /// 중요도 제안: 2 핵심 · 1 일반. ★ · 붉은 글씨 구간의 첫 카드만 핵심 (나머지는 일반, 사용자가 칩으로 바꾼다)
    importance: i64,
    /// 이 덱에 비슷한 카드가 있다 (기본 체크를 끈다)
    similar: Option<Similar>,
    /// 근거 구간과 유사도가 낮은 문장
    ungrounded: Vec<String>,
}

#[derive(Serialize)]
pub struct Draft {
    cards: Vec<DraftCard>,
    /// 보낸 구간 (근거 칩의 자료 · 쪽)
    sent: Vec<Sent>,
    /// 초안이 안 나온 구간
    missing: Vec<i64>,
    /// 넣을 곳 기본값: 그 자료가 붙은 덱 아래에서 이웃 쪽의 카드가 가장 많이 있는 하위 덱. 없으면 자료가 붙은 덱
    deck_id: i64,
    /// 고르는 칸의 뿌리 덱
    root_deck_id: i64,
    /// 기기 안 검사(비슷한 카드 · 근거에 없는 말)를 했는가. 분석 모델이 있을 때만 한다
    checked: bool,
}

fn draft_instructions(per: &str, cloze: bool) -> String {
    let shape = if cloze {
        "front 는 핵심어를 {{c1::핵심어}} 처럼 가린 한 문장(한 문장에 빈칸은 1~2개, c1 · c2 …), back 은 빈 글자나 짧은 덧붙임."
    } else {
        "front 는 하나만 묻는 짧은 질문, back 은 짧은 답."
    };
    format!(
        "강의 자료에서 강조된 구간으로 암기 카드 초안을 만들어요. 사용자가 번호 붙인 구간 글을 보내요.
- 구간마다 {per}. 구간 글에 있는 내용만 쓰고, 구간에 없는 사실을 더하지 마세요.
- {shape}
- 카드마다 segment 에 근거 구간 번호를 쓰세요. 구간 글과 같은 언어로.
- 답은 {{\"cards\": [{{\"front\": …, \"back\": …, \"segment\": 번호}}]}} 모양의 JSON 하나만."
    )
}

fn draft_schema() -> J {
    let card = ai::object(json!({ "front": { "type": "string" }, "back": { "type": "string" }, "segment": { "type": "integer" } }));
    ai::object(json!({ "cards": { "type": "array", "items": card } }))
}

/// 모델 출력 → (앞, 뒤, 구간 id). 하나라도 어긋나면 BadOutput (일부도 쓰지 않는다):
/// 카드 1–30장, 앞이 비지 않음(빈칸이면 빈칸 표시가 있음), 질문과 답이면 뒤도 비지 않음, 근거 구간이 보낸 구간 안
fn parse_drafts(text: &str, sent: &[Sent], cloze: bool) -> Result<Vec<(String, String, i64)>, AiError> {
    let bad = |m: &str| AiError::BadOutput(m.to_string());
    let v = ai::parse_json(text)?;
    let cards = v["cards"].as_array().ok_or_else(|| bad("cards 가 없어요"))?;
    if !(1..=MAX_DRAFTS).contains(&cards.len()) {
        return Err(bad("카드 수가 1–30장이 아니에요"));
    }
    let mut out = vec![];
    for c in cards {
        let front = c["front"].as_str().unwrap_or("").trim().to_string();
        let back = c["back"].as_str().unwrap_or("").trim().to_string();
        let ok = if cloze { !render::cloze_numbers(&front).is_empty() } else { !front.is_empty() && !back.is_empty() };
        if !ok {
            return Err(bad("앞면이나 뒷면이 빈 카드가 있어요"));
        }
        let seg = segment_at(sent, &c["segment"]).ok_or_else(|| bad("보내지 않은 구간 번호예요"))?;
        out.push((front, back, seg));
    }
    Ok(out)
}

/// 가짜 초안: 구간마다 1장(보통 · 많이면 둘째 구간에 1장 더, 근거에 없는 말을 넣는다). 3구간 이상이면 마지막 구간은 비운다 (부분 성공)
fn fake_drafts(sent: &[Sent], amount: &str, cloze: bool) -> String {
    let n = if sent.len() >= 3 { sent.len() - 1 } else { sent.len() };
    let mut cards = vec![];
    for (i, s) in sent.iter().take(n).enumerate() {
        let first = sentences(&s.text).into_iter().next().unwrap_or_else(|| s.text.clone());
        let word = first.split_whitespace().next().unwrap_or("핵심").to_string();
        if cloze {
            cards.push(json!({ "front": first.replacen(&word, &format!("{{{{c1::{word}}}}}"), 1), "back": "", "segment": i + 1 }));
        } else {
            cards.push(json!({ "front": format!("{word}에 대해 자료가 말하는 것은?"), "back": first, "segment": i + 1 }));
        }
        if i == 1 && amount != "few" && !cloze {
            cards.push(json!({ "front": format!("{word}와 함께 기억할 점은?"), "back": format!("{first}. 이 방법은 1950년대에 처음 쓰였다."), "segment": i + 1 }));
        }
    }
    json!({ "cards": cards }).to_string()
}

/// 문장 나누기 (근거에 없는 말 검사). 짧은 조각은 뜻이 없어 뺀다
fn sentences(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut it = text.chars().peekable();
    while let Some(ch) = it.next() {
        cur.push(ch);
        // 마침표 뒤에 빈칸이 올 때만 나눈다 ("0.1 이하")
        if ch == '\n' || (matches!(ch, '.' | '?' | '!' | '。') && it.peek().is_none_or(|c| c.is_whitespace())) {
            out.push(std::mem::take(&mut cur));
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().trim_end_matches(['.', '。']).trim().to_string()).filter(|s| s.chars().count() >= 4).collect()
}

/// 넣을 곳 기본값: 고른 구간마다 같은 자료에서 쪽이 가장 가까운 "카드가 있는 구간"의 카드 덱을 모아 가장 많은 덱
fn default_deck(con: &Connection, sent: &[Sent]) -> Res<i64> {
    let mut votes: HashMap<i64, usize> = HashMap::new();
    for s in sent {
        if let Some(d) = source::nearest_deck(con, s.material_id, s.page)? {
            *votes.entry(d).or_default() += 1;
        }
    }
    if let Some((d, _)) = votes.into_iter().max_by_key(|(d, n)| (*n, std::cmp::Reverse(*d))) {
        return Ok(d);
    }
    let first = sent.first().ok_or("고른 구간이 없어요.")?;
    con.query_row("SELECT deck_id FROM source_material WHERE id = ?1", [first.material_id], |r| r.get(0)).map_err(err)
}

fn root_of(con: &Connection, deck_id: i64) -> Res<i64> {
    let decks = commands::load_decks(con)?;
    let parent: HashMap<i64, Option<i64>> = decks.iter().map(|d| (d.id, d.parent_id)).collect();
    let mut id = deck_id;
    while let Some(Some(p)) = parent.get(&id) {
        id = *p;
    }
    Ok(id)
}

/// R1-b [초안 만들기]. amount: few | normal | many (구간마다 1 / 1–2 / 1–3장). 아무것도 바꾸지 않는다
#[tauri::command]
pub async fn rag_draft(app: AppHandle, segment_ids: Vec<i64>, amount: String, cloze: bool) -> Result<Draft, AiError> {
    tauri::async_runtime::spawn_blocking(move || draft_blocking(&app, &segment_ids, &amount, cloze)).await.map_err(|e| AiError::Other(e.to_string()))?
}

fn draft_blocking(app: &AppHandle, ids: &[i64], amount: &str, cloze: bool) -> Result<Draft, AiError> {
    if ids.is_empty() || ids.len() > MAX_SEGMENTS {
        return Err(AiError::Other(format!("구간은 1–{MAX_SEGMENTS}개 고를 수 있어요.")));
    }
    let (sent, deck_id, root) = with_db(app, |c| {
        let sent = sent_of(c, &ids.iter().map(|id| (*id, "picked")).collect::<Vec<_>>())?;
        let deck = default_deck(c, &sent)?;
        Ok((sent, deck, root_of(c, deck)?))
    })?;
    let per = match amount {
        "few" => "1장",
        "many" => "1~3장",
        _ => "1~2장",
    };
    let instructions = draft_instructions(per, cloze);
    let input = segments_input(&sent);
    let schema = draft_schema();
    let text = ai::generate(app, &|m, f| ai::request_body(m, &instructions, &input, f.then_some(("drafts", &schema))), || fake_drafts(&sent, amount, cloze))?;
    let drafts = parse_drafts(&text, &sent, cloze)?;
    let missing = sent.iter().map(|s| s.id).filter(|id| !drafts.iter().any(|d| d.2 == *id)).collect();
    let mut cards: Vec<DraftCard> = vec![];
    for (front, back, segment_id) in drafts {
        let strong = sent.iter().find(|s| s.id == segment_id).is_some_and(|s| s.signals.iter().any(|g| g == "star" || g == "red"));
        let first = !cards.iter().any(|c| c.segment_id == segment_id);
        cards.push(DraftCard { front, back, segment_id, importance: if strong && first { 2 } else { 1 }, similar: None, ungrounded: vec![] });
    }
    // 기기 안 검사는 분석 모델이 있을 때만. 실패해도 초안은 그대로 보인다
    let checked = check_drafts(app, root, &sent, &mut cards, cloze).is_ok();
    Ok(Draft { cards, sent, missing, deck_id, root_deck_id: root, checked })
}

/// 비슷한 카드(이 덱 카드와 0.88 이상)와 근거에 없는 말(근거 구간과 유사도가 낮은 뒷면 문장). 보내지 않는다
fn check_drafts(app: &AppHandle, root: i64, sent: &[Sent], cards: &mut [DraftCard], cloze: bool) -> Res<()> {
    let state = app.state::<AppState>();
    let model = embed::load(&state.data_dir)?;
    let existing = {
        let con = state.db.lock().map_err(err)?;
        source::card_texts(&con, &source::subtree(&con, root)?)?
    };
    let vecs = source::embed_cached(app, "card", &existing.iter().map(|c| (c.id, c.text.clone())).collect::<Vec<_>>(), |_| {})?;
    // 보낸 구간은 잘리기 전 원문으로 견준다
    let seg_vecs = {
        let con = state.db.lock().map_err(err)?;
        let mut items = vec![];
        for s in sent {
            items.push((s.id, con.query_row("SELECT text FROM source_segment WHERE id = ?1", [s.id], |r| r.get::<_, String>(0)).map_err(err)?));
        }
        items
    };
    let seg_vecs = source::embed_cached(app, "segment", &seg_vecs, |_| {})?;
    let texts: Vec<String> = cards.iter().map(|c| format!("{} {}", plain_cloze(&c.front), c.back)).collect();
    let card_vecs = model.embed(&texts)?;
    for (c, v) in cards.iter_mut().zip(&card_vecs) {
        c.similar = existing
            .iter()
            .filter_map(|e| vecs.get(&e.id).map(|ev| (e, dot(v, ev))))
            .filter(|(_, s)| *s >= SIMILAR_CARD)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(e, score)| Similar { card_id: e.id, question: e.front.chars().take(60).collect(), score });
        let Some(sv) = seg_vecs.get(&c.segment_id) else { continue };
        // 빈칸 카드는 앞면 문장이 곧 내용이다
        let said = if cloze { format!("{}\n{}", plain_cloze(&c.front), c.back) } else { c.back.clone() };
        let parts = sentences(&said);
        if parts.is_empty() {
            continue;
        }
        let pv = model.embed(&parts)?;
        c.ungrounded = parts.into_iter().zip(pv).filter(|(_, v)| dot(v, sv) < UNGROUNDED).map(|(p, _)| p).collect();
    }
    Ok(())
}

/// {{c1::답::힌트}} → 답
fn plain_cloze(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{c") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let (Some(open), Some(close)) = (tail.find("::"), tail.find("}}")) else {
            out.push_str(tail);
            return out;
        };
        if open > close {
            out.push_str(&tail[..close + 2]);
        } else {
            out.push_str(tail[open + 2..close].split("::").next().unwrap_or(""));
        }
        rest = &tail[close + 2..];
    }
    out.push_str(rest);
    out
}

#[derive(Deserialize)]
pub struct ApprovedCard {
    front: String,
    back: String,
    segment_id: i64,
    /// 0 보충 · 1 일반 · 2 핵심
    importance: Option<i64>,
}

/// R1-c [N장 만들기]: 새 카드 + source_link(직접 연결, 초안에서 만듦). 한 번의 변경(card_change 'draft')으로 묶어 되돌릴 수 있다
#[tauri::command]
pub fn rag_draft_apply(state: State<AppState>, deck_id: i64, cloze: bool, cards: Vec<ApprovedCard>) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    apply_drafts(&mut con, deck_id, cloze, &cards)
}

fn apply_drafts(con: &mut Connection, deck_id: i64, cloze: bool, cards: &[ApprovedCard]) -> Res<i64> {
    let cards: Vec<&ApprovedCard> = cards.iter().filter(|c| !render::is_blank(&c.front) && (cloze || !render::is_blank(&c.back))).collect();
    if cards.is_empty() {
        return Err("만들 카드가 없어요. 앞면과 뒷면을 채워 주세요.".into());
    }
    let tx = con.transaction().map_err(err)?;
    let now = now_ms();
    let mut notes = vec![];
    for c in &cards {
        let card = NewCard {
            deck_id,
            kind: if cloze { "cloze" } else { "basic" }.into(),
            front: quality::to_html(&c.front),
            back: quality::to_html(&c.back),
            importance: c.importance.filter(|i| (0..=2).contains(i)),
        };
        commands::insert_note(&tx, &card, "", None)?;
        let note: i64 = tx.query_row("SELECT note_id FROM card WHERE id = last_insert_rowid()", [], |r| r.get(0)).map_err(err)?;
        // 빈칸 노트는 카드가 여럿일 수 있다. 모두 그 구간에 잇는다
        tx.execute(
            "INSERT INTO source_link (segment_id, card_id, method, origin, created_at)
             SELECT ?1, id, 'manual', 'draft', ?2 FROM card WHERE note_id = ?3
             ON CONFLICT (segment_id, card_id) DO UPDATE SET method = 'manual', origin = 'draft', score = NULL",
            params![c.segment_id, now, note],
        )
        .map_err(err)?;
        notes.push(note);
    }
    let file: String = tx
        .query_row("SELECT m.file_name FROM source_segment s JOIN source_material m ON m.id = s.material_id WHERE s.id = ?1", [cards[0].segment_id], |r| r.get(0))
        .map_err(err)?;
    tx.execute(
        "INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at) VALUES ('draft', ?1, NULL, ?2, ?3, ?4)",
        params![deck_id, format!("「{file}」에서 카드 {}장 만들기", cards.len()), json!({ "prev_state": null, "new_notes": notes }).to_string(), now],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(err)?;
    Ok(id)
}

// ---------------------------------------------------------------- R2 서술형 채점 보조

/// 내 답 · 모범 답안을 보내는 최대 글자 (넘으면 자르고 "…(잘림)")
const ANSWER_CLIP: usize = 1500;
/// 함께 보내는 자료 구간 (R2 · R3): 연결된 것 먼저, 최대 3개
pub const MAX_CONTEXT: usize = 3;
/// R2 연결된 구간 뒤 남은 자리에 넣을 비슷한 구간의 최소 유사도 (질문 + 모범 답안과)
const SIMILAR_SEGMENT: f32 = 0.8;

/// 이 카드에 연결된 구간: 직접 연결 > 자동 연결(유사도 순), 최대 3개
pub(crate) fn linked_segments(con: &Connection, card_id: i64) -> Res<Vec<(i64, &'static str)>> {
    let mut st = con
        .prepare("SELECT segment_id, method FROM source_link WHERE card_id = ?1 AND method != 'rejected' ORDER BY method = 'manual' DESC, score DESC, id LIMIT ?2")
        .map_err(err)?;
    let rows = st.query_map(params![card_id, MAX_CONTEXT as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(err)?;
    rows.map(|r| r.map(|(id, m)| (id, if m == "manual" { "manual" } else { "auto" })).map_err(err)).collect()
}

/// 카드 뿌리 덱에 붙은 자료의 구간과 query 의 유사도 (거르지 않음). 분석 모델이 없으면 Err (연결된 구간만 보낸다)
fn similar_segments(app: &AppHandle, card_id: i64, query: &str) -> Res<Vec<(i64, f32)>> {
    let state = app.state::<AppState>();
    let model = embed::load(&state.data_dir)?;
    let items: Vec<(i64, String)> = {
        let con = state.db.lock().map_err(err)?;
        let deck: i64 = con.query_row("SELECT deck_id FROM card WHERE id = ?1", [card_id], |r| r.get(0)).map_err(err)?;
        let ids = source::subtree(&con, root_of(&con, deck)?)?;
        let mut st = con
            .prepare(&format!(
                "SELECT s.id, s.text FROM source_segment s JOIN source_material m ON m.id = s.material_id WHERE m.deck_id IN ({ids}) AND m.status = 'ready'"
            ))
            .map_err(err)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let vecs = source::embed_cached(app, "segment", &items, |_| {})?;
    let q = model.embed(&[query.to_string()])?.remove(0);
    Ok(vecs.iter().map(|(id, v)| (*id, dot(&q, v))).collect())
}

/// 연결된 구간 먼저, 남은 자리(모두 3개까지)는 0.8 이상인 비슷한 구간을 유사도 순으로 (이미 넣은 구간은 빼고)
fn grade_picks(linked: Vec<(i64, &'static str)>, mut hits: Vec<(i64, f32)>) -> Vec<(i64, &'static str)> {
    hits.retain(|h| h.1 >= SIMILAR_SEGMENT && !linked.iter().any(|l| l.0 == h.0));
    hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut picks = linked;
    picks.extend(hits.into_iter().map(|h| (h.0, "similar")));
    picks.truncate(MAX_CONTEXT);
    picks
}

/// (질문, 모범 답안, 내 답) 보내는 글자 그대로
fn grade_texts(con: &Connection, card_id: i64, answer: &str) -> Res<(String, String, String)> {
    let question = quality::suggest(con, card_id, None)?.question;
    let model = written::model_answer(con, card_id)?;
    Ok((clip(&question, CLIP), clip(&model, ANSWER_CLIP), clip(answer, ANSWER_CLIP)))
}

#[derive(Serialize)]
pub struct GradeScope {
    /// 보내는 질문 · 내 답 · 모범 답안 (보내는 글자 그대로)
    question: String,
    mine: String,
    model: String,
    /// 연결된 구간(직접 > 자동), 남은 자리는 비슷한 구간
    segments: Vec<Sent>,
}

/// R2 보내는 범위 (버튼 아래 한 줄 · 첫 확인 목록). 아무것도 보내지 않는다
#[tauri::command]
pub async fn rag_grade_scope(app: AppHandle, card_id: i64, answer: String) -> Result<GradeScope, AiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let (question, model, mine, linked) = with_db(&app, |c| {
            let (q, model, mine) = grade_texts(c, card_id, &answer)?;
            Ok((q, model, mine, linked_segments(c, card_id)?))
        })?;
        // 모델이 없으면 비슷한 구간 없이 보낸다. 연결이 3개면 재지 않는다
        let hits = if linked.len() < MAX_CONTEXT { similar_segments(&app, card_id, &format!("{question} {model}")).unwrap_or_default() } else { vec![] };
        let picks = grade_picks(linked, hits);
        let segments = with_db(&app, |c| sent_of(c, &picks))?;
        Ok(GradeScope { question, mine, model, segments })
    })
    .await
    .map_err(|e| AiError::Other(e.to_string()))?
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Missing {
    text: String,
    segments: Vec<i64>,
}

#[derive(Serialize)]
pub struct Grade {
    right: Vec<String>,
    missing: Vec<Missing>,
    /// ChatGPT 추천: 1 Again · 2 Hard · 3 Good (핵심어 검사처럼 Easy 는 없다)
    rating: u8,
    /// 실제로 보낸 구간 (결과 머리 "보낸 것")
    sent: Vec<Sent>,
}

const GRADE_INSTRUCTIONS: &str = "서술형 답을 스스로 채점하는 학습자를 도와요. 사용자가 질문, 모범 답안, 학습자가 쓴 답, 번호 붙인 자료 구간을 보내요.
- 학습자 답에서 모범 답안과 맞는 점(right)과 빠지거나 틀린 점(missing)을 학습자에게 말하듯 짧은 문장으로 쓰세요. 각각 4개까지.
- 빠진 점마다 근거가 되는 자료 구간 번호를 segments 에 쓰세요. 없으면 빈 배열.
- 모범 답안과 자료 구간에 없는 기준으로 채점하지 마세요.
- rating 은 again(핵심을 거의 못 씀) · hard(일부만 씀) · good(필요한 내용을 다 씀) 중 하나.
- 답은 {\"right\": [문장, …], \"missing\": [{\"text\": 문장, \"segments\": [번호, …]}], \"rating\": …} 모양의 JSON 하나만.";

fn grade_schema() -> J {
    let ints = json!({ "type": "array", "items": { "type": "integer" } });
    let missing = ai::object(json!({ "text": { "type": "string" }, "segments": ints }));
    ai::object(json!({
        "right": { "type": "array", "items": { "type": "string" } },
        "missing": { "type": "array", "items": missing },
        "rating": { "type": "string", "enum": ["again", "hard", "good"] },
    }))
}

fn grade_input(question: &str, model: &str, mine: &str, sent: &[Sent]) -> String {
    let segs = if sent.is_empty() { "없음\n".to_string() } else { segments_input(sent) };
    format!("질문: {question}\n모범 답안:\n{model}\n학습자 답:\n{mine}\n자료 구간:\n{segs}")
}

/// 모델 출력 → 맞은 점 · 빠진 점 · 추천. 하나라도 어긋나면 BadOutput (일부도 쓰지 않는다)
fn parse_grade(text: &str, sent: &[Sent]) -> Result<(Vec<String>, Vec<Missing>, u8), AiError> {
    let bad = |m: &str| AiError::BadOutput(m.to_string());
    let v = ai::parse_json(text)?;
    let line = |x: &J| x.as_str().map(str::trim).filter(|t| !t.is_empty() && t.chars().count() <= 300).map(String::from);
    let right: Vec<String> =
        v["right"].as_array().ok_or_else(|| bad("right 가 없어요"))?.iter().map(line).collect::<Option<_>>().ok_or_else(|| bad("빈 문장이 있어요"))?;
    let mut missing = vec![];
    for m in v["missing"].as_array().ok_or_else(|| bad("missing 이 없어요"))? {
        let text = line(&m["text"]).ok_or_else(|| bad("빈 문장이 있어요"))?;
        let segments = m["segments"].as_array().ok_or_else(|| bad("segments 가 없어요"))?.iter().map(|n| segment_at(sent, n)).collect::<Option<Vec<_>>>();
        missing.push(Missing { text, segments: segments.ok_or_else(|| bad("보내지 않은 구간 번호예요"))? });
    }
    if (right.is_empty() && missing.is_empty()) || right.len() > 8 || missing.len() > 8 {
        return Err(bad("맞은 점 · 빠진 점 수가 맞지 않아요"));
    }
    let rating = match v["rating"].as_str() {
        Some("again") => 1,
        Some("hard") => 2,
        Some("good") => 3,
        _ => return Err(bad("추천이 again · hard · good 이 아니에요")),
    };
    Ok((right, missing, rating))
}

fn fake_grade(sent: &[Sent]) -> String {
    let cite: Vec<usize> = if sent.is_empty() { vec![] } else { vec![1] };
    json!({
        "right": ["핵심 개념을 모범 답안과 같은 뜻으로 정의했어요", "판단 기준을 정확히 썼어요"],
        "missing": [{ "text": "모범 답안의 해결 방법이 빠졌어요", "segments": cite }],
        "rating": "hard",
    })
    .to_string()
}

/// R2 [ChatGPT로 채점 보조]: 내 답 · 모범 답안 · 확인한 구간을 보내 맞은 점 · 빠진 점 · 추천을 받는다. 아무것도 바꾸지 않는다
#[tauri::command]
pub async fn rag_grade(app: AppHandle, card_id: i64, answer: String, segment_ids: Vec<i64>) -> Result<Grade, AiError> {
    tauri::async_runtime::spawn_blocking(move || {
        if answer.trim().is_empty() {
            return Err(AiError::Other("쓴 답이 없어 채점 보조를 받을 수 없어요.".into()));
        }
        if segment_ids.len() > MAX_CONTEXT {
            return Err(AiError::Other(format!("자료 구간은 {MAX_CONTEXT}개까지 보낼 수 있어요.")));
        }
        let (question, model, mine, sent) = with_db(&app, |c| {
            let (q, model, mine) = grade_texts(c, card_id, &answer)?;
            let linked = linked_segments(c, card_id)?;
            let how = |id: i64| linked.iter().find(|l| l.0 == id).map_or("similar", |l| l.1);
            Ok((q, model, mine, sent_of(c, &segment_ids.iter().map(|id| (*id, how(*id))).collect::<Vec<_>>())?))
        })?;
        let input = grade_input(&question, &model, &mine, &sent);
        let schema = grade_schema();
        let text = ai::generate(&app, &|m, f| ai::request_body(m, GRADE_INSTRUCTIONS, &input, f.then_some(("grade", &schema))), || fake_grade(&sent))?;
        let (right, missing, rating) = parse_grade(&text, &sent)?;
        Ok(Grade { right, missing, rating, sent })
    })
    .await
    .map_err(|e| AiError::Other(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(n: usize) -> Vec<Sent> {
        (1..=n as i64)
            .map(|i| Sent {
                id: 100 + i,
                material_id: 1,
                material: "a.pdf".into(),
                kind: "pdf".into(),
                page: i,
                signals: vec![if i == 1 { "star".into() } else { "highlight".into() }],
                text: format!("구간 {i}의 글. 둘째 문장."),
                how: "picked".into(),
            })
            .collect()
    }

    #[test]
    fn clipping_and_input_carry_no_names() {
        let long = "가".repeat(700);
        let c = clip(&long, CLIP);
        assert_eq!(c.chars().count(), 600 + CUT.chars().count());
        assert!(c.ends_with("…(잘림)"));
        assert_eq!(clip(" 짧은 글 ", CLIP), "짧은 글");
        let s = sent(2);
        let input = segments_input(&s);
        assert_eq!(input, "[1] 구간 1의 글. 둘째 문장.\n[2] 구간 2의 글. 둘째 문장.\n");
        assert!(!input.contains("a.pdf") && !input.contains("p."), "자료 이름 · 쪽은 보내지 않는다");
        assert_eq!(segment_at(&s, &json!(2)), Some(102));
        assert_eq!(segment_at(&s, &json!(0)), None);
        assert_eq!(segment_at(&s, &json!(3)), None);
    }

    #[test]
    fn drafts_are_validated_whole() {
        let s = sent(3);
        let ok = r#"{"cards":[{"front":"질문?","back":"답","segment":1},{"front":"둘째?","back":"답2","segment":3}]}"#;
        assert_eq!(parse_drafts(ok, &s, false).unwrap(), vec![("질문?".into(), "답".into(), 101), ("둘째?".into(), "답2".into(), 103)]);
        let bad = |t: &str, cloze: bool| matches!(parse_drafts(t, &s, cloze), Err(AiError::BadOutput(_)));
        assert!(bad(r#"{"cards":[]}"#, false), "0장");
        let many: Vec<String> = (0..31).map(|_| r#"{"front":"q","back":"a","segment":1}"#.to_string()).collect();
        assert!(bad(&format!(r#"{{"cards":[{}]}}"#, many.join(",")), false), "31장");
        assert!(bad(r#"{"cards":[{"front":"q","back":" ","segment":1}]}"#, false), "빈 뒷면");
        assert!(bad(r#"{"cards":[{"front":"q","back":"a","segment":4}]}"#, false), "보내지 않은 구간");
        assert!(bad(r#"{"cards":[{"front":"q","back":"a","segment":1},{"front":"q","back":"a"}]}"#, false), "구간 없음 하나라도");
        assert!(bad(r#"{"cards":[{"front":"빈칸 없음","back":"","segment":1}]}"#, true), "빈칸 표시 없음");
        assert!(parse_drafts(r#"{"cards":[{"front":"{{c1::잔차}} 분석","back":"","segment":2}]}"#, &s, true).is_ok(), "빈칸은 뒷면이 비어도 된다");
        assert!(bad("{\"cards\": [", false));
    }

    #[test]
    fn fake_drafts_pass_the_same_checks() {
        let s = sent(3);
        for (amount, cloze, n) in [("few", false, 2), ("normal", false, 3), ("many", true, 2)] {
            let d = parse_drafts(&fake_drafts(&s, amount, cloze), &s, cloze).unwrap();
            assert_eq!(d.len(), n, "{amount}");
            assert!(!d.iter().any(|x| x.2 == 103), "마지막 구간은 초안이 안 나온다 (부분 성공)");
        }
        // 소수점에서 문장을 자르지 않는다 (가짜 응답도 앱의 문장 나누기를 쓴다)
        let mut v = sent(1);
        v[0].text = "VIF 10 이상 또는 공차한계 0.1 이하로 판단한다. 둘째 문장.".into();
        let d = parse_drafts(&fake_drafts(&v, "few", false), &v, false).unwrap();
        assert_eq!(d[0].1, "VIF 10 이상 또는 공차한계 0.1 이하로 판단한다");
    }

    #[test]
    fn sentences_and_cloze_text() {
        assert_eq!(sentences("일부 계수를 0으로 만든다. L1 벌점을 쓴다."), vec!["일부 계수를 0으로 만든다", "L1 벌점을 쓴다"]);
        assert_eq!(sentences("예."), Vec::<String>::new(), "짧은 조각은 뺀다");
        assert_eq!(sentences("VIF 10 이상 또는 공차한계 0.1 이하"), vec!["VIF 10 이상 또는 공차한계 0.1 이하"]);
        assert_eq!(plain_cloze("{{c1::잔차}} 분석으로 {{c2::등분산성::가정}}을 본다"), "잔차 분석으로 등분산성을 본다");
        assert_eq!(plain_cloze("빈칸 없음"), "빈칸 없음");
    }

    #[test]
    fn grades_are_validated_whole() {
        let s = sent(2);
        let ok = r#"{"right":["정의를 맞게 썼어요"],"missing":[{"text":"공차한계 기준이 빠졌어요","segments":[2]},{"text":"해결 방법이 없어요","segments":[]}],"rating":"hard"}"#;
        let (right, missing, rating) = parse_grade(ok, &s).unwrap();
        assert_eq!((right.len(), rating), (1, 2));
        assert_eq!(missing[0], Missing { text: "공차한계 기준이 빠졌어요".into(), segments: vec![102] }, "구간 번호는 기기 안에서 구간 id 로");
        let bad = |t: &str| matches!(parse_grade(t, &s), Err(AiError::BadOutput(_)));
        assert!(bad(r#"{"right":[],"missing":[],"rating":"good"}"#), "둘 다 비었다");
        assert!(bad(r#"{"right":["a"],"missing":[],"rating":"easy"}"#), "Easy 는 없다");
        assert!(bad(r#"{"right":[" "],"missing":[],"rating":"good"}"#), "빈 문장");
        assert!(bad(r#"{"right":["a"],"missing":[{"text":"b","segments":[3]}],"rating":"good"}"#), "보내지 않은 구간");
        assert!(bad(r#"{"right":["a"],"rating":"good"}"#), "missing 없음");
        assert!(parse_grade(&fake_grade(&s), &s).is_ok() && parse_grade(&fake_grade(&[]), &[]).is_ok());
        let input = grade_input("다중공선성이란?", "독립변수 간 상관이 높아…", "변수끼리 상관", &[]);
        assert!(input.ends_with("자료 구간:\n없음\n"));
        assert!(!grade_input("q", "m", "a", &s).contains("a.pdf"), "자료 이름은 보내지 않는다");
    }

    #[test]
    fn linked_segments_prefer_manual_and_cap_at_three() {
        let con = setup();
        crate::commands::insert_note(&con, &NewCard { deck_id: 3, kind: "basic".into(), front: "q".into(), back: "a".into(), importance: None }, "", None).unwrap();
        let card: i64 = con.query_row("SELECT max(id) FROM card", [], |r| r.get(0)).unwrap();
        con.execute_batch(&format!(
            "INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight) VALUES (21, 1, 21, 0, 'x', 'bold', 1), (22, 1, 22, 0, 'y', 'bold', 1);
             INSERT INTO source_link (segment_id, card_id, method, score, created_at) VALUES (11, {card}, 'auto', 0.90, 0), (13, {card}, 'auto', 0.95, 0),
               (20, {card}, 'manual', NULL, 0), (21, {card}, 'rejected', 0.99, 0), (22, {card}, 'auto', 0.89, 0);"
        ))
        .unwrap();
        assert_eq!(linked_segments(&con, card).unwrap(), vec![(20, "manual"), (13, "auto"), (11, "auto")]);
    }

    #[test]
    fn grade_picks_fill_after_linked() {
        // 실계정: 연결 1개(14) + 빠진 점을 받치는 구간(15, 0.897) → 둘 다
        assert_eq!(grade_picks(vec![(14, "manual")], vec![(14, 0.95), (15, 0.897), (16, 0.79)]), vec![(14, "manual"), (15, "similar")]);
        // 연결이 3개면 더하지 않는다
        let three = vec![(1, "manual"), (2, "auto"), (3, "auto")];
        assert_eq!(grade_picks(three.clone(), vec![(9, 0.99)]), three);
        // 0.8 아래는 넣지 않는다. 연결이 없으면 비슷한 구간 위 3개
        assert_eq!(grade_picks(vec![], vec![(5, 0.79)]), vec![]);
        assert_eq!(grade_picks(vec![], vec![(5, 0.81), (6, 0.9), (7, 0.85), (8, 0.82)]), vec![(6, "similar"), (7, "similar"), (8, "similar")]);
    }

    fn setup() -> Connection {
        let con = crate::db::open_in_memory().unwrap();
        con.execute_batch(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '빅데이터', 0, 0), (2, '3과목', 0, 0), (3, '회귀', 0, 0);
             UPDATE deck SET parent_id = 1 WHERE id = 2; UPDATE deck SET parent_id = 2 WHERE id = 3;
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, status, created_at, updated_at) VALUES (1, 1, '3장.pdf', 'sources/1.pdf', 'pdf', 'ready', 0, 0);
             INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight) VALUES
               (11, 1, 12, 0, '다중공선성은 VIF 로 본다', 'highlight', 3), (13, 1, 13, 0, '잔차 분석', 'star', 2), (20, 1, 20, 0, '교차 검증', 'bold', 1);",
        )
        .unwrap();
        con
    }

    #[test]
    fn draft_apply_links_and_undoes_as_one_change() {
        let mut con = setup();
        // 이웃 쪽(12쪽) 구간에 회귀 덱 카드가 이어져 있으면 넣을 곳 기본값은 그 덱
        crate::commands::insert_note(&con, &NewCard { deck_id: 3, kind: "basic".into(), front: "VIF?".into(), back: "10".into(), importance: None }, "", None).unwrap();
        con.execute("INSERT INTO source_link (segment_id, card_id, method, created_at) VALUES (11, last_insert_rowid(), 'manual', 0)", []).unwrap();
        let s = sent_of(&con, &[(13, "picked"), (20, "picked")]).unwrap();
        assert_eq!(default_deck(&con, &s).unwrap(), 3);
        assert_eq!(root_of(&con, 3).unwrap(), 1);
        let gaps = |con: &Connection| source::material(con, 1).unwrap().gaps;
        assert_eq!(gaps(&con), 2);
        let cards = vec![
            ApprovedCard { front: "잔차 분석으로 확인하는 가정은?".into(), back: "등분산성\n정규성".into(), segment_id: 13, importance: Some(2) },
            ApprovedCard { front: " ".into(), back: "빈 앞면은 버린다".into(), segment_id: 20, importance: None },
        ];
        let change = apply_drafts(&mut con, 3, false, &cards).unwrap();
        let n = |sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(n("SELECT count(*) FROM card WHERE deck_id = 3"), 2);
        assert_eq!(n("SELECT count(*) FROM source_link WHERE segment_id = 13 AND method = 'manual' AND origin = 'draft'"), 1);
        assert_eq!(n("SELECT importance FROM card WHERE id = (SELECT max(id) FROM card)"), 2);
        assert_eq!(gaps(&con), 1, "만든 구간은 빠진 범위에서 빠진다");
        let summary: String = con.query_row("SELECT summary FROM card_change WHERE id = ?1", [change], |r| r.get(0)).unwrap();
        assert_eq!(summary, "「3장.pdf」에서 카드 1장 만들기");
        quality::undo(&mut con, change).unwrap();
        let n = |sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!((n("SELECT count(*) FROM card WHERE deck_id = 3"), n("SELECT count(*) FROM source_link WHERE origin = 'draft'")), (1, 0));
        assert_eq!(gaps(&con), 2);
        // 빈칸: 노트의 카드 둘 다 잇는다
        let cloze = vec![ApprovedCard { front: "{{c1::잔차}} 분석으로 {{c2::등분산성}}을 본다".into(), back: String::new(), segment_id: 13, importance: None }];
        apply_drafts(&mut con, 3, true, &cloze).unwrap();
        assert_eq!(con.query_row("SELECT count(*) FROM source_link WHERE segment_id = 13 AND origin = 'draft'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert!(apply_drafts(&mut con, 3, false, &[]).is_err());
    }
}

