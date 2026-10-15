//! G1 카드 점검 · G2 나누기 제안. 규칙만 쓴다 (모델 없이, 기기 안에서 즉시).
//! 이 모듈은 카드를 몰래 바꾸지 않는다. 바꾸는 것은 사용자가 승인한 split_apply 뿐이고, 한 번의 변경으로 기록해 되돌릴 수 있다

use crate::commands::{self, AppState, NewCard};
use crate::db::{now_ms, DAY_MS};
use crate::{queue, render};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 검사 기준 (G1 화면 아래 한 줄과 같다)
const LONG_LINES: i64 = 8;
const LONG_ITEMS: i64 = 6;
const MANY_QUESTIONS: i64 = 2;
const AGAIN_DAYS: i64 = 14;
const AGAIN_MIN: i64 = 2;
/// [그대로 두기] 뒤 다시 올리지 않는 기간
const HIDE_DAYS: i64 = 30;

/// 문제 하나. kind: "lines"(답 N줄) · "items"(답 N항목) · "questions"(질문 N개) · "again"(최근 14일 Again N번)
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Flag {
    pub kind: &'static str,
    pub n: i64,
}

impl Flag {
    /// 카드 모양의 문제 (Again 은 학습 결과라 따로 센다). G3 학습 화면이 쓴다
    pub fn is_shape(&self) -> bool {
        self.kind != "again"
    }
}

// ------------------------------------------------------------------ 글자 규칙 (순수 함수)

/// HTML 을 줄 단위 글자로. 블록 태그와 <br> 에서 줄을 나누고, 빈 줄은 버린다
pub(crate) fn html_lines(html: &str) -> Vec<String> {
    const BLOCK: [&str; 14] = ["br", "div", "p", "li", "tr", "td", "th", "hr", "h1", "h2", "h3", "h4", "h5", "h6"];
    let mut out = String::with_capacity(html.len());
    let mut tag: Option<String> = None;
    for ch in html.chars() {
        match (&mut tag, ch) {
            (None, '<') => tag = Some(String::new()),
            (Some(t), '>') => {
                let name: String = t.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if BLOCK.contains(&name.to_ascii_lowercase().as_str()) {
                    out.push('\n');
                }
                tag = None;
            }
            (Some(t), c) => t.push(c),
            (None, c) => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect()
}

/// 목록 항목 표시로 시작하는가 (①, 1., 1), -, •, · 등)
fn item_marker(line: &str) -> Option<usize> {
    let mut chars = line.char_indices();
    let (_, first) = chars.next()?;
    if ('\u{2460}'..='\u{2473}').contains(&first) || ['-', '•', '·', '*', '▪', '○', '●', '◦', '■', '□'].contains(&first) {
        return Some(first.len_utf8());
    }
    // 1. 1) (1) 가. 가)
    let digits: String = line.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &line[digits.len()..];
    if !digits.is_empty() && (rest.starts_with('.') || rest.starts_with(')')) {
        return Some(digits.len() + 1);
    }
    if let Some(after) = line.strip_prefix('(') {
        let inner: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !inner.is_empty() && after[inner.len()..].starts_with(')') {
            return Some(inner.len() + 2);
        }
    }
    None
}

/// 답의 항목들. 줄마다 항목 표시가 있으면 그 줄들, 한 줄에 ①②… 가 이어 쓰여 있으면 그것으로 나눈다
pub(crate) fn answer_items(lines: &[String]) -> Vec<String> {
    let marked: Vec<String> = lines.iter().filter(|l| item_marker(l).is_some()).cloned().collect();
    if marked.len() >= 2 {
        return marked;
    }
    // "① A ② B ③ C" 한 줄
    let joined = lines.join(" ");
    let mut items = vec![];
    let mut cur = String::new();
    for ch in joined.chars() {
        if ('\u{2460}'..='\u{2473}').contains(&ch) && !cur.trim().is_empty() {
            items.push(cur.trim().to_string());
            cur.clear();
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        items.push(cur.trim().to_string());
    }
    if items.len() >= 2 && items.iter().skip(1).all(|i| i.starts_with(|c: char| ('\u{2460}'..='\u{2473}').contains(&c))) {
        return items;
    }
    vec![]
}

/// 질문 안의 물음 수. 물음표 수에, "…와/과 …은?" 처럼 두 가지를 한 번에 묻는 마디를 하나씩 더한다
pub(crate) fn question_count(question: &str) -> i64 {
    let q = question.replace('？', "?");
    let marks = q.matches('?').count() as i64;
    if marks == 0 {
        return 0;
    }
    let joined = q.split_inclusive('?').filter(|seg| joined_pair(seg).is_some()).count() as i64;
    marks + joined
}

/// 따옴표 안은 이름이라 물음으로 보지 않는다 ('AUC·F1과 해석성'에 해당하는 …은?). 같은 길이의 빈칸으로 바꾼다
fn unquoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut close: Option<char> = None;
    for c in s.chars() {
        match close {
            Some(end) if c == end => close = None,
            Some(_) => {}
            None => match c {
                '\'' => close = Some('\''),
                '"' => close = Some('"'),
                '‘' => close = Some('’'),
                '“' => close = Some('”'),
                '「' => close = Some('」'),
                '『' => close = Some('』'),
                _ => {}
            },
        }
        if close.is_some() || matches!(c, '\'' | '"' | '’' | '”' | '」' | '』') {
            out.extend(std::iter::repeat_n(' ', c.len_utf8()));
        } else {
            out.push(c);
        }
    }
    out
}

/// "A와 B은?" 마디면 (A, B) 를 돌려준다. 따옴표 안의 와/과는 보지 않는다
fn joined_pair(seg: &str) -> Option<(&str, &str)> {
    let seg = seg.trim();
    if !seg.ends_with("은?") {
        return None;
    }
    // 바이트 위치가 같게 바꿨으므로 찾은 위치로 원문을 자른다
    let plain = unquoted(seg);
    let (a, b) = ["와 ", "과 "].iter().filter_map(|j| plain.find(j).map(|i| (i, j.len()))).min().map(|(i, len)| (&seg[..i], &seg[i + len..]))?;
    // "R과 Python의 차이점은?" 처럼 뒤 낱말이 '의'로 끝나면 둘을 묶은 이름 하나다
    if b.split_whitespace().next().is_some_and(|w| w.ends_with('의')) {
        return None;
    }
    // "A와 B의 차이점은?"처럼 둘을 한꺼번에 견주는 물음도 하나다
    const PAIR_NOUNS: [&str; 5] = ["항목은?", "차이점은?", "공통점은?", "구분은?", "연관성은?"];
    if b.split_whitespace().last().is_some_and(|w| PAIR_NOUNS.contains(&w)) {
        return None;
    }
    Some((a, b))
}

/// 질문 · 답 글자만으로 보는 문제 (Again 은 DB 가 더한다)
pub(crate) fn text_flags(question: &str, answer_lines: &[String]) -> Vec<Flag> {
    let mut flags = vec![];
    let items = answer_items(answer_lines).len() as i64;
    let lines = answer_lines.len() as i64;
    if items >= LONG_ITEMS {
        flags.push(Flag { kind: "items", n: items });
    } else if lines >= LONG_LINES {
        flags.push(Flag { kind: "lines", n: lines });
    }
    let q = question_count(question);
    if q >= MANY_QUESTIONS {
        flags.push(Flag { kind: "questions", n: q });
    }
    flags
}

// ------------------------------------------------------------------ 카드 읽기

/// 카드의 질문 글자와 답 줄들 (화면에 보이는 그대로, 템플릿을 적용해서)
struct CardText {
    deck_id: i64,
    question: String,
    answer: Vec<String>,
}

fn card_texts(con: &Connection, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Res<Vec<(i64, CardText)>> {
    let names = commands::note_field_names(con)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.deck_id, n.fields, nt.name, t.qfmt, t.afmt, c.template_ord
             FROM card c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id
             LEFT JOIN card_template t ON t.note_type_id = n.note_type_id
                  AND t.ord = (CASE WHEN nt.name = 'Cloze' OR t.qfmt LIKE '%{{{{cloze:%' THEN 0 ELSE c.template_ord END)
             WHERE {filter} ORDER BY c.id"
        ))
        .map_err(err)?;
    let rows = st
        .query_map(args, |r| {
            let fields: String = r.get(2)?;
            let nt: String = r.get(3)?;
            let q: Option<String> = r.get(4)?;
            let a: Option<String> = r.get(5)?;
            let rendered = commands::render_fields(&names, &nt, &fields, q.as_deref(), a.as_deref(), r.get(6)?);
            // Q 목록 · 선택형은 항목이 많아도 나누기 제안을 하지 않는다 (나누면 일반 카드가 된다. Q4 는 10개 넘을 때 경고만)
            let special = names.get(&nt).is_some_and(|n| crate::recall::is_special(&nt, n));
            Ok((
                r.get::<_, i64>(0)?,
                CardText {
                    deck_id: r.get(1)?,
                    question: render::strip_tags(&rendered.front),
                    answer: if special { vec![] } else { html_lines(commands::answer_part(&rendered.back)) },
                },
            ))
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

/// 카드별 최근 14일 Again 횟수
fn again_counts(con: &Connection, card_filter: &str, args: &[&dyn rusqlite::ToSql]) -> Res<HashMap<i64, i64>> {
    let since = now_ms() - AGAIN_DAYS * DAY_MS;
    let mut st = con
        .prepare(&format!(
            "SELECT r.card_id, count(*) FROM review_log r JOIN card c ON c.id = r.card_id
             WHERE r.rating = 1 AND r.reviewed_at >= {since} AND r.kind IS NULL AND {card_filter} GROUP BY r.card_id"
        ))
        .map_err(err)?;
    let rows = st.query_map(args, |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

fn with_again(mut flags: Vec<Flag>, again: i64) -> Vec<Flag> {
    if again >= AGAIN_MIN {
        flags.push(Flag { kind: "again", n: again });
    }
    flags
}

/// G3(학습 화면)도 쓰는 카드 한 장 검사. 싸다: 카드 하나만 렌더링한다
pub(crate) fn card_flags(con: &Connection, card_id: i64) -> Res<Vec<Flag>> {
    let Some((_, t)) = card_texts(con, "c.id = ?1", &[&card_id])?.into_iter().next() else {
        return Ok(vec![]);
    };
    let again = again_counts(con, "c.id = ?1", &[&card_id])?.get(&card_id).copied().unwrap_or(0);
    Ok(with_again(text_flags(&t.question, &t.answer), again))
}

// ------------------------------------------------------------------ G1 점검 목록

#[derive(Serialize)]
pub struct QualityCard {
    id: i64,
    question: String,
    /// 최상위부터 카드가 든 덱까지
    path: Vec<String>,
    flags: Vec<Flag>,
    /// [그대로 두기]로 숨긴 카드 (목록 맨 아래 "숨긴 카드 N")
    hidden: bool,
}

/// 덱(하위 덱 포함)에서 카드 모양에 문제가 있는 카드. Again 은 문제와 겹칠 때만 붙는다 (보기는 화면이 거른다)
#[tauri::command]
pub fn quality_check(state: State<AppState>, deck_id: i64) -> Res<Vec<QualityCard>> {
    let con = state.db.lock().map_err(err)?;
    let decks = commands::load_decks(&con)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let ids = commands::descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let scope = format!("c.deck_id IN ({ids}) AND c.state <> 'suspended'");
    let again = again_counts(&con, &scope, &[])?;
    let hidden: HashMap<i64, bool> = {
        let mut st = con
            .prepare(&format!("SELECT c.id FROM card c WHERE {scope} AND c.quality_hidden_until > ?1"))
            .map_err(err)?;
        let rows = st.query_map([now_ms()], |r| r.get::<_, i64>(0)).map_err(err)?;
        rows.map(|r| r.map(|id| (id, true))).collect::<Result<_, _>>().map_err(err)?
    };
    let mut out = vec![];
    for (id, t) in card_texts(&con, &scope, &[])? {
        let flags = text_flags(&t.question, &t.answer);
        if flags.is_empty() {
            continue;
        }
        out.push(QualityCard {
            id,
            question: t.question,
            path: commands::path_of(t.deck_id, &by_id),
            flags: with_again(flags, again.get(&id).copied().unwrap_or(0)),
            hidden: hidden.contains_key(&id),
        });
    }
    // 자주 틀리는 카드부터
    out.sort_by_key(|c| std::cmp::Reverse(c.flags.iter().find(|f| f.kind == "again").map_or(0, |f| f.n)));
    Ok(out)
}

/// [그대로 두기] 30일 동안 목록에서 뺀다. hide = false 면 다시 올린다. 카드 자체는 그대로다
#[tauri::command]
pub fn hide_quality(state: State<AppState>, card_id: i64, hide: bool) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    let until = hide.then(|| now_ms() + HIDE_DAYS * DAY_MS);
    con.execute("UPDATE card SET quality_hidden_until = ?2 WHERE id = ?1", params![card_id, until]).map_err(err)?;
    Ok(())
}

// ------------------------------------------------------------------ G2 나누기 제안

/// 제안 카드 한 장. 글자 그대로 (답은 줄바꿈으로 항목을 나눈다). 만들 때 HTML 로 바꾼다
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SplitPart {
    pub(crate) front: String,
    pub(crate) back: String,
}

/// 글자를 카드 필드 HTML 로 (줄바꿈 → <br>)
pub(crate) fn to_html(text: &str) -> String {
    text.trim().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>")
}

/// 필드는 ai.rs(성격별로 묶기)가 같은 모양의 제안을 만들 때 쓴다
#[derive(Serialize)]
pub struct SplitProposal {
    pub(crate) card_id: i64,
    pub(crate) question: String,
    /// 지금 카드의 답 줄 (왼쪽 열)
    pub(crate) answer: Vec<String>,
    /// "왜?" 한 줄
    pub(crate) why: String,
    pub(crate) parts: Vec<SplitPart>,
    /// 한 장에 담은 항목 수 (다른 제안 받기가 바꾼다)
    pub(crate) per_card: i64,
    pub(crate) flags: Vec<Flag>,
    /// items | lines | questions | nature(ChatGPT 성격별로 묶기). questions 면 한 장 항목 수를 바꿔도 같은 제안이다
    pub(crate) kind: &'static str,
    /// R3: ChatGPT 에 함께 보낸 자료 구간 · 그중 실제로 인용한 구간 (근거 칩). 규칙 제안은 비어 있다
    pub(crate) sent: Vec<crate::rag::Sent>,
    pub(crate) evidence: Vec<crate::rag::Sent>,
}

/// 받침이 있는가 (조사 고르기)
fn has_batchim(s: &str) -> bool {
    s.chars().last().is_some_and(|c| ('가'..='힣').contains(&c) && !(c as u32 - '가' as u32).is_multiple_of(28))
}

/// 질문에서 "…을 모두 쓰시오." 같은 지시를 떼고 물을 대상만 남긴다 ("조직 문화의 10가지 특성")
/// 이름꼴로 끝나면 true ("… 중 1–3번째는?"로 묻는다). 아니면 원래 물음 뒤에 "(1–3번째)"를 붙인다.
/// 마지막 물음표 뒤의 안내 문장("판단한 뒤 정답을 확인하세요.")은 뗀다
fn question_base(question: &str) -> (String, bool) {
    let q = question.trim();
    let q = q.rfind(['?', '？']).map_or(q, |i| &q[..i]);
    let base = q.trim().trim_end_matches(['?', '？', '.', '!', ' ']);
    for tail in [
        "을 모두 쓰시오", "를 모두 쓰시오", "을 쓰시오", "를 쓰시오", "을 설명하시오", "를 설명하시오", "을 나열하시오", "를 나열하시오",
        "은 무엇인가", "는 무엇인가", "은", "는", "을", "를",
    ] {
        if let Some(b) = base.strip_suffix(tail) {
            return (b.trim().to_string(), true);
        }
    }
    (base.to_string(), false)
}

/// 항목 앞 표시(①, 1. 등)를 뗀 글자
pub(crate) fn strip_marker(item: &str) -> String {
    match item_marker(item) {
        Some(n) => item[n..].trim().to_string(),
        None => item.trim().to_string(),
    }
}

/// 규칙 분할. 항목이 있으면 번호 순서대로 per_card 개씩 ("…중 1–3번째는?"), 물음이 여럿이면 물음마다,
/// 둘 다 아니면 답 줄을 항목처럼 나눈다. 나머지는 마지막 카드에 붙인다 (10개 → 3·3·4)
pub(crate) fn split_text(question: &str, answer: &[String], per_card: i64) -> Vec<SplitPart> {
    let items = answer_items(answer);
    if items.len() < 2 && question_count(question) >= MANY_QUESTIONS {
        return split_questions(question, answer);
    }
    let items = if items.len() >= 2 { items } else { answer.to_vec() };
    if items.len() < 2 {
        return vec![];
    }
    let per = per_card.clamp(1, items.len() as i64) as usize;
    let cards = (items.len() / per).max(1);
    let (base, noun) = question_base(question);
    let mut parts = vec![];
    let mut start = 0;
    for k in 0..cards {
        // 앞 카드들은 per 개씩, 남는 것은 마지막 카드로
        let end = if k + 1 == cards { items.len() } else { start + per };
        let range = if end - start == 1 { format!("{}번째", start + 1) } else { format!("{}–{}번째", start + 1, end) };
        parts.push(SplitPart {
            front: if noun { format!("{base} 중 {range}{}?", if has_batchim(&range) { "은" } else { "는" }) } else { format!("{base}? ({range})") },
            back: items[start..end].iter().map(|i| strip_marker(i)).collect::<Vec<_>>().join("\n"),
        });
        start = end;
    }
    parts
}

/// 물음이 여럿인 질문을 물음마다. 답 줄 수가 물음 수와 같으면 짝지어 주고, 아니면 답 전체를 붙인다 (고쳐서 쓴다)
fn split_questions(question: &str, answer: &[String]) -> Vec<SplitPart> {
    let q = question.replace('？', "?");
    let mut asks: Vec<String> = vec![];
    for seg in q.split_inclusive('?').map(str::trim).filter(|s| !s.is_empty()) {
        // "A와 B은?" → "A는?" · "B은?"
        match joined_pair(seg) {
            Some((a, b)) => {
                asks.push(format!("{a}{}?", if has_batchim(a) { "은" } else { "는" }));
                asks.push(b.to_string());
            }
            None => asks.push(seg.to_string()),
        }
    }
    let whole = answer.join("\n");
    asks.iter()
        .enumerate()
        .map(|(i, a)| SplitPart {
            front: a.clone(),
            back: if answer.len() == asks.len() { answer[i].clone() } else { whole.clone() },
        })
        .collect()
}

/// 기본 한 장 항목 수: 3개 안팎으로 묶는다 (10개 → 3장)
fn default_per_card(n: usize) -> i64 {
    if n <= 4 { 1 } else { 3 }
}

#[tauri::command]
pub fn split_suggest(state: State<AppState>, card_id: i64, per_card: Option<i64>) -> Res<SplitProposal> {
    suggest(&*state.db.lock().map_err(err)?, card_id, per_card)
}

/// 규칙 제안. 질문 · 답은 화면에 보이는 글자다 (HTML · 그림 · [sound:] 를 뺐다). ai.rs 는 이 글자만 보낸다
pub(crate) fn suggest(con: &Connection, card_id: i64, per_card: Option<i64>) -> Res<SplitProposal> {
    let (_, t) = card_texts(con, "c.id = ?1", &[&card_id])?.into_iter().next().ok_or("카드를 찾을 수 없어요.")?;
    let flags = card_flags(con, card_id)?;
    let items = answer_items(&t.answer);
    let n = if items.len() >= 2 { items.len() } else { t.answer.len() };
    let per = per_card.unwrap_or_else(|| default_per_card(n));
    let parts = split_text(&t.question, &t.answer, per);
    let by_questions = items.len() < 2 && question_count(&t.question) >= MANY_QUESTIONS;
    let kind = if by_questions { "questions" } else if items.len() >= 2 { "items" } else { "lines" };
    // "번호 순서대로 3–4개씩" (Figma). 앞 카드들은 같은 수, 나머지는 마지막 카드에 붙는다
    let sizes: Vec<usize> = parts.iter().map(|p| p.back.lines().count()).collect();
    let (lo, hi) = (sizes.iter().min().copied().unwrap_or(0), sizes.iter().max().copied().unwrap_or(0));
    let each = if lo == hi { format!("{lo}") } else { format!("{lo}–{hi}") };
    let how = match kind {
        "questions" => "물음마다 나눴어요.".to_string(),
        "items" => format!("번호 순서대로 {each}개씩 나눴어요."),
        _ => format!("순서대로 {each}줄씩 나눴어요."),
    };
    let why = format!("{}. {how}", reason_of(&flags));
    Ok(SplitProposal { card_id, question: t.question, answer: t.answer, why, parts, per_card: per, flags, kind, sent: vec![], evidence: vec![] })
}

/// "왜?"의 앞부분 = 걸린 규칙 (G1 배지와 같은 말) + Again. 뒤에 어떻게 나눴는지를 붙인다 (ai.rs 도 쓴다)
pub(crate) fn reason_of(flags: &[Flag]) -> String {
    let again = flags.iter().find(|f| f.kind == "again").map(|f| format!(" (Again {}번)", f.n)).unwrap_or_default();
    let reason = flags.iter().find(|f| f.kind != "again").map(|f| match f.kind {
        "items" => format!("답에 항목이 {}개라 한 번에 떠올리기 어려워요", f.n),
        "lines" => format!("답이 {}줄이라 한 번에 떠올리기 어려워요", f.n),
        _ => format!("질문 하나에 물음이 {}개라 한 번에 답하기 어려워요", f.n),
    });
    format!("{}{again}", reason.unwrap_or_else(|| "한 장에 담긴 것이 많아요".into()))
}

#[derive(Deserialize, Serialize)]
struct SplitPayload {
    prev_state: Option<String>,
    new_notes: Vec<i64>,
}

/// 승인한 나누기. 새 카드를 만들고(같은 덱 · 같은 개별 중요도), 원하면 원래 카드를 보관(학습에서 뺌)한다.
/// 복습 기록은 지우지 않는다. 한 번의 변경으로 기록해 change id 를 돌려준다
pub(crate) fn apply_split(con: &mut Connection, card_id: i64, parts: &[SplitPart], archive_original: bool) -> Res<i64> {
    let parts: Vec<&SplitPart> = parts.iter().filter(|p| !render::is_blank(&p.front) && !render::is_blank(&p.back)).collect();
    if parts.is_empty() {
        return Err("나눌 카드가 없어요. 질문과 답을 채워주세요.".into());
    }
    let tx = con.transaction().map_err(err)?;
    let (deck_id, importance, state): (i64, Option<i64>, String) = tx
        .query_row("SELECT deck_id, importance, state FROM card WHERE id = ?1", [card_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(err)?
        .ok_or("카드를 찾을 수 없어요.")?;
    let mut new_notes = vec![];
    for p in &parts {
        let card = NewCard { deck_id, kind: "basic".into(), front: to_html(&p.front), back: to_html(&p.back), importance };
        commands::insert_note(&tx, &card, "", None)?;
        new_notes.push(tx.query_row("SELECT note_id FROM card WHERE id = last_insert_rowid()", [], |r| r.get(0)).map_err(err)?);
    }
    let prev_state = archive_original.then(|| state.clone());
    if archive_original {
        tx.execute("UPDATE card SET state = 'suspended', updated_at = ?2 WHERE id = ?1", params![card_id, now_ms()]).map_err(err)?;
    }
    let payload = serde_json::to_string(&SplitPayload { prev_state, new_notes }).map_err(err)?;
    // 변경 기록에 어느 카드였는지 남긴다 (질문 앞부분)
    let question = card_texts(&tx, "c.id = ?1", &[&card_id])?.into_iter().next().map(|(_, t)| t.question).unwrap_or_default();
    let short: String = question.chars().take(40).collect();
    let more = if question.chars().count() > 40 { "…" } else { "" };
    let summary = format!("「{short}{more}」 {}장으로 나누기", parts.len());
    tx.execute(
        "INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at) VALUES ('split', ?1, ?2, ?3, ?4, ?5)",
        params![deck_id, card_id, summary, payload, now_ms()],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(err)?;
    Ok(id)
}

/// 나누기를 되돌린다: 새 카드를 지우고 원래 카드를 원래 상태로
pub(crate) fn undo(con: &mut Connection, change_id: i64) -> Res<()> {
    let tx = con.transaction().map_err(err)?;
    let (card_id, payload, undone): (Option<i64>, String, Option<i64>) = tx
        .query_row("SELECT card_id, payload, undone_at FROM card_change WHERE id = ?1", [change_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(err)?
        .ok_or("변경 기록을 찾을 수 없어요.")?;
    if undone.is_some() {
        return Err("이미 되돌린 변경이에요.".into());
    }
    let p: SplitPayload = serde_json::from_str(&payload).map_err(err)?;
    for nid in &p.new_notes {
        // 새 카드에 복습 기록이 있어도 기록은 남는다 (review_log.card_id 가 NULL 이 된다)
        tx.execute("DELETE FROM note WHERE id = ?1", [nid]).map_err(err)?;
    }
    if let (Some(card), Some(prev)) = (card_id, p.prev_state) {
        tx.execute("UPDATE card SET state = ?2, updated_at = ?3 WHERE id = ?1", params![card, prev, now_ms()]).map_err(err)?;
    }
    tx.execute("UPDATE card_change SET undone_at = ?2 WHERE id = ?1", params![change_id, now_ms()]).map_err(err)?;
    tx.commit().map_err(err)
}

#[tauri::command]
pub fn split_apply(state: State<AppState>, card_id: i64, parts: Vec<SplitPart>, archive_original: bool) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    apply_split(&mut con, card_id, &parts, archive_original)
}

#[tauri::command]
pub fn undo_change(state: State<AppState>, change_id: i64) -> Res<()> {
    let mut con = state.db.lock().map_err(err)?;
    undo(&mut con, change_id)
}

#[derive(Serialize)]
pub struct ChangeRow {
    id: i64,
    summary: String,
    card_id: Option<i64>,
    created_at: i64,
    undone: bool,
}

/// G1 "최근 나눈 카드" 30개 (R1 초안은 토스트의 [되돌리기]로만)
#[tauri::command]
pub fn recent_changes(state: State<AppState>, deck_id: i64) -> Res<Vec<ChangeRow>> {
    let con = state.db.lock().map_err(err)?;
    let decks = commands::load_decks(&con)?;
    let ids = commands::descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut st = con
        .prepare(&format!(
            "SELECT id, summary, card_id, created_at, undone_at IS NOT NULL FROM card_change WHERE kind = 'split' AND deck_id IN ({ids}) ORDER BY created_at DESC LIMIT 30"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| Ok(ChangeRow { id: r.get(0)?, summary: r.get(1)?, card_id: r.get(2)?, created_at: r.get(3)?, undone: r.get(4)? }))
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn html_to_lines() {
        assert_eq!(html_lines("<div>① A</div><div>② B&nbsp;C</div><br>"), ["① A", "② B C"]);
        assert_eq!(html_lines("가<br>나<br/><br>다"), ["가", "나", "다"]);
    }

    #[test]
    fn items_and_flags() {
        let ten: Vec<String> = (0..10).map(|i| format!("{} 특성{i}", char::from_u32(0x2460 + i).unwrap())).collect();
        assert_eq!(answer_items(&ten).len(), 10);
        // 한 줄에 이어 쓴 ①②③
        assert_eq!(answer_items(&lines("① 가 ② 나 ③ 다")).len(), 3);
        assert_eq!(answer_items(&lines("1. 가\n2) 나\n(3) 다")).len(), 3);
        assert!(answer_items(&lines("그냥 문장\n또 문장")).is_empty());
        assert_eq!(text_flags("조직 문화의 10가지 특성을 모두 쓰시오.", &ten), [Flag { kind: "items", n: 10 }]);
        let nine: Vec<String> = (0..9).map(|i| format!("설명 {i}")).collect();
        assert_eq!(text_flags("절차는?", &nine), [Flag { kind: "lines", n: 9 }]);
        assert!(text_flags("짧은 질문?", &lines("짧은 답")).is_empty());
    }

    #[test]
    fn question_counting() {
        assert_eq!(question_count("데이터 품질 관리 절차와 각 단계의 산출물은?"), 2);
        assert_eq!(question_count("A는? B는?"), 2);
        assert_eq!(question_count("A와 B의 차이는?"), 1, "'…은?'으로 끝나지 않으면 한 물음");
        assert_eq!(question_count("설명하시오."), 0);
        assert_eq!(question_count("'AUC·F1과 해석성'에 해당하는 항목은? 판단한 뒤 확인하세요."), 1, "따옴표 안의 과는 물음이 아니다");
        assert_eq!(joined_pair("‘가’와 나은?"), Some(("‘가’", "나은?")));
        assert_eq!(question_count("R과 Python의 구분의 항목은?"), 1, "'A과 B의 …'는 이름 하나");
        assert_eq!(question_count("정형·반정형·비정형 데이터의 예와 저장 방식은?"), 2);
        assert_eq!(question_count("회귀 평가와 변수 선택의 항목은?"), 1, "견주는 물음은 하나");
    }

    #[test]
    fn split_by_items_3_3_4() {
        let ten: Vec<String> = (0..10).map(|i| format!("{} 특성{i}", char::from_u32(0x2460 + i).unwrap())).collect();
        let parts = split_text("조직 문화의 10가지 특성을 모두 쓰시오.", &ten, 3);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].front, "조직 문화의 10가지 특성 중 1–3번째는?");
        assert_eq!(parts[0].back, "특성0\n특성1\n특성2");
        assert_eq!(parts[2].back.lines().count(), 4, "나머지는 마지막 카드에");
        // 한 장에 하나씩
        let one = split_text("조직 문화의 10가지 특성을 모두 쓰시오.", &ten, 1);
        assert_eq!(one.len(), 10);
        assert_eq!(one[0].front, "조직 문화의 10가지 특성 중 1번째는?");
        // 이름꼴이 아니면 물음 뒤에 범위를 붙이고, 물음표 뒤 안내 문장은 뗀다
        let p = split_text("항목과 키워드를 연결하면? 판단한 뒤 정답을 확인하세요.", &ten, 5);
        assert_eq!(p[1].front, "항목과 키워드를 연결하면? (6–10번째)");
    }

    #[test]
    fn split_by_questions() {
        let parts = split_text("데이터 품질 관리 절차와 각 단계의 산출물은?", &lines("절차: 계획 → 측정\n산출물: 보고서"), 3);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].front, "데이터 품질 관리 절차는?");
        assert_eq!(parts[1].front, "각 단계의 산출물은?");
        assert_eq!(parts[1].back, "산출물: 보고서", "답 줄 수가 물음 수와 같으면 짝짓는다");
    }

    #[test]
    fn apply_and_undo_split() {
        let mut con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('d', 0, 0)", []).unwrap();
        let deck = con.last_insert_rowid();
        let card = NewCard { deck_id: deck, kind: "basic".into(), front: "Q?".into(), back: "① a<br>② b<br>③ c".into(), importance: Some(2) };
        commands::insert_note(&con, &card, "", None).unwrap();
        let orig: i64 = con.query_row("SELECT max(id) FROM card", [], |r| r.get(0)).unwrap();
        con.execute("UPDATE card SET state = 'review' WHERE id = ?1", [orig]).unwrap();
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (?1, 1, 1)", [orig]).unwrap();
        let parts = vec![SplitPart { front: "Q 1?".into(), back: "a < b\nc".into() }, SplitPart { front: "Q 2?".into(), back: "b".into() }];
        let count = |con: &Connection, sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();

        let change = apply_split(&mut con, orig, &parts, true).unwrap();
        assert_eq!(count(&con, "SELECT count(*) FROM card"), 3);
        assert_eq!(count(&con, &format!("SELECT count(*) FROM card WHERE id = {orig} AND state = 'suspended'")), 1, "원래 카드는 보관");
        assert_eq!(count(&con, "SELECT count(*) FROM card WHERE importance = 2 AND state = 'new'"), 2, "새 카드는 중요도를 따른다");
        assert_eq!(count(&con, &format!("SELECT count(*) FROM review_log WHERE card_id = {orig}")), 1, "복습 기록은 그대로");
        let back: String = con.query_row("SELECT fields FROM note ORDER BY id DESC LIMIT 1 OFFSET 1", [], |r| r.get(0)).unwrap();
        assert!(back.contains("a &lt; b<br>c"), "글자는 HTML 로 바꿔 넣는다: {back}");

        undo(&mut con, change).unwrap();
        assert_eq!(count(&con, "SELECT count(*) FROM card"), 1, "새 카드는 지워진다");
        assert_eq!(count(&con, &format!("SELECT count(*) FROM card WHERE id = {orig} AND state = 'review'")), 1, "원래 상태로");
        assert!(undo(&mut con, change).is_err(), "두 번 되돌리지 않는다");
        // 보관하지 않고 나누면 원래 카드는 그대로
        let change = apply_split(&mut con, orig, &parts, false).unwrap();
        assert_eq!(count(&con, &format!("SELECT count(*) FROM card WHERE id = {orig} AND state = 'review'")), 1);
        undo(&mut con, change).unwrap();
        assert!(apply_split(&mut con, orig, &[SplitPart { front: " ".into(), back: "".into() }], true).is_err());
    }
}
