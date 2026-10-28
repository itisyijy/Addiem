//! G1 카드 점검 · G2 나누기 제안 · 8.2 질문과 답의 대응 · 서식 제안. 규칙만 쓴다 (모델 없이, 기기 안에서 즉시).
//! 이 모듈은 카드를 몰래 바꾸지 않는다. 바꾸는 것은 사용자가 승인한 split_apply · rewrite_apply 뿐이고, 한 번의 변경으로 기록해 되돌릴 수 있다

use crate::commands::{self, AppState, NewCard};
use crate::db::{now_ms, DAY_MS};
use crate::{notetype, queue, render};
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
/// · "mismatch"(답이 안 맞아 보임) · "format"(서식 제안, n = 고친 뒤 줄 수). 뒤의 둘은 근거 · 제안 한 줄(reason)이 있다
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Flag {
    pub kind: &'static str,
    pub n: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 서식 제안의 센 사실 ("한 줄에 번호 7개"). 배지로 보인다
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
}

impl Flag {
    fn new(kind: &'static str, n: i64) -> Self {
        Flag { kind, n, reason: None, badge: None }
    }
}

impl Flag {
    /// 카드 모양의 문제 (나누기로 고치는 것) (Again 은 학습 결과라 따로 센다). G3 학습 화면이 쓴다
    pub fn is_shape(&self) -> bool {
        matches!(self.kind, "lines" | "items" | "questions")
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
pub(crate) fn item_marker(line: &str) -> Option<usize> {
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
        flags.push(Flag::new("items", items));
    } else if lines >= LONG_LINES {
        flags.push(Flag::new("lines", lines));
    }
    let q = question_count(question);
    if q >= MANY_QUESTIONS {
        flags.push(Flag::new("questions", q));
    }
    flags
}

// ------------------------------------------------------------------ 8.2 질문과 답의 대응 · 서식 (규칙. 오탐을 줄이려고 확실한 모양만 본다)

/// 답 자리를 채우기만 한 글자 (소문자로 견준다)
const PLACEHOLDERS: [&str; 17] = ["todo", "tbd", "tba", "n/a", "x", "미정", "모름", "몰라", "작성 중", "작성중", "추가 예정", "나중에", "답", "정답", "내용", "빈칸", "없음"];
/// 개수를 묻는 단위 ("세 가지", "4단계", "5개")
const COUNTERS: [&str; 3] = ["가지", "단계", "개"];
/// 고유어 수. 긴 것부터 (다섯 · 여섯 … 을 두 · 세 보다 먼저)
const NATIVE: [(&str, i64); 9] = [("다섯", 5), ("여섯", 6), ("일곱", 7), ("여덟", 8), ("아홉", 9), ("두", 2), ("세", 3), ("네", 4), ("열", 10)];
/// 수 · 단위 뒤에 붙어도 같은 낱말인 조사. 다른 한글이 붙으면 "두 개념"처럼 다른 낱말이다
const PARTICLES: &str = "를을은는의가이로와과에도만";
/// 이 말이 뒤따르면 묻는 개수가 아니다 ("세 가지 중", "3가지 이상")
const NOT_ASKED: [&str; 8] = ["중", "이상", "이하", "미만", "초과", "정도", "내외", "남짓"];
/// 수를 말하는 낱말 (답에 수가 있는지 볼 때). 넓게 잡을수록 덜 걸린다
const NUMERALS: [&str; 33] = [
    "하나", "둘", "셋", "넷", "다섯", "여섯", "일곱", "여덟", "아홉", "열", "스물", "스무", "서른", "마흔", "쉰", "한", "두", "세", "네", "일", "이", "삼", "사", "오", "육", "칠", "팔", "구", "십", "백", "천", "만", "억",
];
const UNITS: [&str; 24] = ["가지", "개", "명", "번", "회", "단계", "계층", "층", "년", "살", "배", "차", "장", "권", "곳", "시간", "분", "초", "일", "주", "달", "종", "마리", "%"];
/// 쉼표 목록으로 볼 줄 길이 · 항목 길이 (글자)
const LIST_MIN_CHARS: usize = 40;
const LIST_ITEM_CHARS: usize = 25;
/// 목록을 나누는 쉼표
const COMMAS: [char; 3] = [',', '，', '、'];

fn is_hangul(c: char) -> bool {
    ('가'..='힣').contains(&c)
}

fn is_circled(c: char) -> bool {
    ('\u{2460}'..='\u{2473}').contains(&c)
}

/// 띄어쓰기 · 문장 부호를 빼고 소문자로 (같은 말인지 견줄 때)
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// "광합성이란?" → "광합성". 정의 · 대상을 묻는 끝말을 뗀다
fn asked_subject(q: &str) -> &str {
    let base = q.trim().trim_end_matches(|c: char| !c.is_alphanumeric());
    const TAILS: [&str; 13] = ["이란 무엇인가", "란 무엇인가", "은 무엇인가", "는 무엇인가", "의 정의는", "의 뜻은", "의 의미는", "이란", "란", "은", "는", "이", "가"];
    TAILS.iter().find_map(|t| base.strip_suffix(t)).unwrap_or(base)
}

/// 항목 표시의 모양 ("1." · "1)" · "(1)" · "①" · "-"). 번호는 숫자 하나로 본다 (하위 항목 "-" 를 같이 세지 않으려고)
fn marker_kind(line: &str) -> Option<String> {
    let n = item_marker(line)?;
    let mut k = String::new();
    for c in line[..n].chars() {
        let m = if c.is_ascii_digit() {
            '0'
        } else if is_circled(c) {
            '①'
        } else {
            c
        };
        if !(m == '0' && k.ends_with('0')) {
            k.push(m);
        }
    }
    Some(k)
}

/// 질문이 묻는 개수와 단위 ("세 가지" → (3, "가지")). 한 번만 나올 때만.
/// "N번째", "… 중", "N가지 이상", "두 개념"(단위 뒤에 조사 아닌 한글)은 묻는 개수가 아니다
fn asked_count(q: &str) -> Option<(i64, &'static str)> {
    if q.contains("번째") {
        return None;
    }
    let mut found = vec![];
    for (i, c) in q.char_indices() {
        // 낱말 첫머리만 ("10" 의 "0", "가세" 의 "세" 는 보지 않는다)
        if q[..i].chars().next_back().is_some_and(char::is_alphanumeric) {
            continue;
        }
        let rest = &q[i..];
        let (n, after) = if c.is_ascii_digit() {
            let d: String = rest.chars().take_while(char::is_ascii_digit).collect();
            match d.parse::<i64>() {
                Ok(n) => (n, &rest[d.len()..]),
                Err(_) => continue,
            }
        } else if let Some((w, n)) = NATIVE.iter().find(|(w, _)| rest.starts_with(w)) {
            (*n, &rest[w.len()..])
        } else {
            continue;
        };
        let after = after.trim_start();
        let Some(unit) = COUNTERS.iter().find(|u| after.starts_with(*u)) else { continue };
        let tail = &after[unit.len()..];
        if tail.chars().next().is_some_and(|c| is_hangul(c) && !PARTICLES.contains(c)) {
            continue;
        }
        let tail = tail.trim_start_matches(|c: char| PARTICLES.contains(c)).trim_start();
        if NOT_ASKED.iter().any(|w| tail.starts_with(w)) {
            continue;
        }
        found.push((n, *unit));
    }
    match found[..] {
        [(n, unit)] if n >= 2 => Some((n, unit)),
        _ => None,
    }
}

/// 답 항목 수. 같은 모양의 표시가 붙은 항목이 2개 이상이면 그 수, 낱말 하나뿐이면 1, 아니면 모른다 (풀어 쓴 답은 세지 않는다)
fn answer_count(answer: &[String]) -> Option<usize> {
    let items = answer_items(answer);
    if items.len() >= 2 {
        let first = marker_kind(&items[0]);
        return Some(items.iter().filter(|i| marker_kind(i) == first).count());
    }
    match answer {
        [one] if !one.contains(char::is_whitespace) && !one.contains([',', ';', '·', '/', '、', '，']) => Some(1),
        _ => None,
    }
}

/// 수를 묻는 질문이면 질문에 실제로 있는 그 말 ("몇" · "연도" · "how many" …, 근거 줄에 그대로 인용한다). "몇몇"은 수를 묻지 않는다
fn asks_number(q: &str) -> Option<&'static str> {
    let l = q.to_lowercase();
    if ["연도는", "연도를", "년도는", "년도를"].iter().any(|w| l.contains(w)) {
        return Some("연도");
    }
    if let Some(w) = ["what year", "which year", "how many", "how much"].into_iter().find(|w| l.contains(w)) {
        return Some(w);
    }
    l.match_indices('몇').any(|(i, _)| !l[i..].starts_with("몇몇") && !l[..i].ends_with('몇')).then_some("몇")
}

/// 영어 수 낱말 ("seven", "twenty-one", "a dozen")
const EN_NUMBERS: [&str; 33] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen",
    "seventeen", "eighteen", "nineteen", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion", "dozen",
];

/// 풀어 써야 하는 물음 (CQ1 "질문이 묻는 꼴"). 앞의 것부터 본다. 방법 · 뜻은 낱말 하나로도 답이 되어 넣지 않는다
const FORMS: [(&str, &str); 8] = [("차이", "차이"), ("이유", "이유"), ("왜 ", "왜"), ("순서", "순서"), ("차례", "차례"), ("과정", "과정"), ("절차", "절차"), ("장단점", "장단점")];

/// 답에 수가 있는가 (숫자 · 한자 수 · "일곱", "세 가지" · "seven" 같은 수 낱말)
fn has_number(s: &str) -> bool {
    let lower = s.to_lowercase();
    s.chars().any(|c| c.is_ascii_digit() || is_circled(c) || "〇一二三四五六七八九十百千万".contains(c))
        || lower.split(|c: char| !c.is_ascii_alphabetic()).any(|w| EN_NUMBERS.contains(&w))
        || s.split_whitespace().any(|w| {
            NUMERALS.iter().any(|n| {
                w.strip_prefix(n).is_some_and(|r| r.is_empty() || r.starts_with(|c: char| PARTICLES.contains(c)) || UNITS.iter().any(|u| r.starts_with(u)))
            })
        })
}

/// 목적격 조사 ("차이"를 · "몇"을)
fn obj(word: &str) -> &'static str {
    if has_batchim(word) { "을" } else { "를" }
}

/// 답이 낱말 하나뿐이면 그 모양 ("숫자 하나(\"80\")" · "낱말 하나(\"TCP\")")
fn single_word(answer: &[String]) -> Option<String> {
    match answer {
        [one] if !one.contains(char::is_whitespace) && !one.contains([',', ';', '·', '/', '、', '，']) => {
            let what = if one.chars().all(|c| c.is_ascii_digit() || ".,%".contains(c)) { "숫자" } else { "낱말" };
            Some(format!("{what} 하나(\"{one}\")"))
        }
        _ => None,
    }
}

/// 질문과 답이 어긋나 보이는 근거 한 줄 (CQ1 "근거 · …"). 처음 걸린 규칙 하나. media: 답 칸에 그림 · 소리가 있다 (글자가 없어도 빈 답이 아니다)
pub(crate) fn mismatch(question: &str, answer: &[String], media: bool) -> Option<String> {
    let whole = answer.join(" ");
    let a = whole.trim();
    if a.is_empty() {
        return (!media).then(|| "답이 비어 있어요".to_string());
    }
    if PLACEHOLDERS.contains(&a.to_lowercase().as_str()) || squash(a).is_empty() {
        return Some(format!("답이 \"{a}\"뿐이에요"));
    }
    let sa = squash(a);
    if sa == squash(question) || sa == squash(asked_subject(question)) {
        return Some("답이 질문을 그대로 되풀이해요".into());
    }
    if question_count(question) == 0 && answer.len() == 1 && a.ends_with(['?', '？']) {
        return Some("답이 물음으로 끝나요. 앞면과 뒷면이 바뀌었을 수 있어요".into());
    }
    if let (Some((n, unit)), Some(got)) = (asked_count(question), answer_count(answer))
        && got as i64 != n
    {
        let asked = format!("{n}{unit}");
        let got = if got == 1 { format!("답이 {}예요", single_word(answer).unwrap_or_else(|| "하나".into())) } else { format!("답은 {got}개예요") };
        return Some(format!("질문은 \"{asked}\"{} 묻는데 {got}", obj(&asked)));
    }
    let q = format!("{question} ");
    if let Some((_, form)) = FORMS.iter().find(|(w, _)| q.contains(w))
        && let Some(one) = single_word(answer)
    {
        return Some(format!("질문은 \"{form}\"{} 묻는데 답이 {one}예요", obj(form)));
    }
    if let Some(what) = asks_number(question)
        && answer.len() == 1
        && answer_items(answer).len() < 2
        && !a.contains([',', ';', '、', '，'])
        && !has_number(a)
    {
        return Some(format!("질문은 \"{what}\"{} 묻는데 답에 수가 없어요", obj(what)));
    }
    None
}

/// 괄호 밖의 나눔 글자에서 자른다. 숫자 사이 쉼표(1,000)는 자르지 않는다
fn split_outside(s: &str, seps: &[char]) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let (mut out, mut cur, mut depth) = (vec![], String::new(), 0i32);
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' | '[' | '{' | '「' | '（' => depth += 1,
            ')' | ']' | '}' | '」' | '）' => depth -= 1,
            _ => {}
        }
        let digits = i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if depth == 0 && seps.contains(&c) && !digits {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out.into_iter().map(|p| p.trim().to_string()).collect()
}

/// 한 줄에 이어 쓴 번호 목록을 항목마다 한 줄로 ("특징: 1. 가 2. 나" → "특징:", "1. 가", "2. 나"). (줄들, 항목 수).
/// 번호가 1부터 같은 모양으로 3개 이상 이어질 때만 (CQ1). ①②는 붙여 써도, 1. · 1) · (1) 은 줄 첫머리나 띄어쓰기 뒤에서 띄어 쓸 때만 본다
fn inline_list(line: &str) -> Option<(Vec<String>, usize)> {
    let mut marks: Vec<(usize, u32, &str)> = vec![];
    for (i, c) in line.char_indices() {
        if is_circled(c) {
            marks.push((i, c as u32 - 0x2460 + 1, "①"));
            continue;
        }
        if i > 0 && !line[..i].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &line[i..];
        let (paren, body) = match rest.strip_prefix('(') {
            Some(b) => (true, b),
            None => (false, rest),
        };
        let d: String = body.chars().take_while(char::is_ascii_digit).collect();
        let after = &body[d.len()..];
        let style = match (paren, after.chars().next()) {
            (true, Some(')')) => "(1)",
            (false, Some('.')) => "1.",
            (false, Some(')')) => "1)",
            _ => continue,
        };
        let Ok(n) = d.parse::<u32>() else { continue };
        if !after[1..].starts_with(char::is_whitespace) {
            continue;
        }
        marks.push((i, n, style));
    }
    let style = marks.first()?.2;
    let starts: Vec<usize> = marks.iter().filter(|m| m.2 == style).enumerate().map_while(|(k, m)| (m.1 == k as u32 + 1).then_some(m.0)).collect();
    if starts.len() < 3 {
        return None;
    }
    let mut out = vec![];
    let lead = line[..starts[0]].trim();
    if !lead.is_empty() {
        out.push(lead.to_string());
    }
    for (k, start) in starts.iter().enumerate() {
        let item = line[*start..starts.get(k + 1).copied().unwrap_or(line.len())].trim();
        // 번호만 있는 항목 ("1. 2. 3.")은 목록이 아니다
        if strip_marker(item).is_empty() {
            return None;
        }
        out.push(item.to_string());
    }
    Some((out, starts.len()))
}

/// 쉼표 · 세미콜론으로 이은 짧은 항목 3개 이상인 긴 줄 → 항목마다 한 줄 ("구성: 가, 나, 다" → "구성:", "가", "나", "다").
/// 괄호 안 쉼표는 자르지 않고, 문장(…다 · …요로 끝나는 항목)은 건드리지 않는다
fn comma_list(line: &str) -> Option<(Vec<String>, usize)> {
    if line.chars().count() < LIST_MIN_CHARS {
        return None;
    }
    let (lead, body) = match split_outside(line, &[':', '：']).as_slice() {
        [lead, body] if !lead.is_empty() && !lead.contains(COMMAS) && !lead.contains(';') => (Some(format!("{lead}:")), body.clone()),
        _ => (None, line.to_string()),
    };
    let semi = split_outside(&body, &[';', '；']);
    let parts = if semi.len() >= 3 { semi } else { split_outside(&body, &COMMAS) };
    let sentence = |p: &str| p.trim_end_matches(['.', '。']).ends_with(['다', '요']);
    if parts.len() < 3 || parts.iter().any(|p| p.is_empty() || p.chars().count() > LIST_ITEM_CHARS || sentence(p)) {
        return None;
    }
    let n = parts.len();
    Some((lead.into_iter().chain(parts).collect(), n))
}

/// "이름:" 머리말이 한 줄에 2번 이상 ("1NF: 원자값 2NF: 부분 종속 없음") → 머리말마다 한 줄, 이름은 굵게 (**1NF:**). (줄들, 이름들).
/// 이름은 줄 첫머리나 띄어쓰기 뒤의 띄어쓰기 없는 1–12자 + 콜론 + 띄어쓰기. 숫자만인 것(10:30)은 아니다
fn label_list(line: &str) -> Option<(Vec<String>, Vec<String>)> {
    let mut labels: Vec<(usize, &str)> = vec![];
    let mut at = 0;
    for word in line.split_inclusive(char::is_whitespace) {
        let w = word.trim_end();
        if let Some(name) = w.strip_suffix([':', '：'])
            && (1..=12).contains(&name.chars().count())
            && !name.chars().all(|c| c.is_ascii_digit())
            && !name.contains([':', '：'])
            && word.len() > w.len()
        {
            labels.push((at, w));
        }
        at += word.len();
    }
    if labels.len() < 2 {
        return None;
    }
    let mut out = vec![];
    let lead = line[..labels[0].0].trim();
    if !lead.is_empty() {
        out.push(lead.to_string());
    }
    for (k, (start, label)) in labels.iter().enumerate() {
        let end = labels.get(k + 1).map_or(line.len(), |l| l.0);
        let body = line[start + label.len()..end].trim();
        if body.is_empty() {
            return None;
        }
        out.push(format!("**{label}** {body}"));
    }
    Some((out, labels.iter().map(|l| l.1.to_string()).collect()))
}

/// 답 줄의 서식 제안 (CQ1): 한 줄에 이은 번호 목록 · "이름:" 머리말 · 쉼표 목록을 항목마다 한 줄로. 글자는 그대로다.
/// 굵게는 "이름:" 머리말에만 (규칙으로 짐작할 수 있는 것은 그것뿐). (고친 줄들, 배지에 쓸 센 사실, "제안 · …" 한 줄). 고칠 것이 없으면 None
pub(crate) fn format_lines(lines: &[String]) -> Option<(Vec<String>, String, String)> {
    let mut out = vec![];
    let mut found: Vec<(String, String)> = vec![];
    for l in lines {
        if let Some((items, n)) = inline_list(l) {
            found.push((format!("한 줄에 번호 {n}개"), "번호마다 줄을 나눠요 (내용은 그대로)".into()));
            out.extend(items);
        } else if let Some((items, names)) = label_list(l) {
            let quoted = |sep: &str, n: usize| names.iter().take(n).map(|x| format!("{sep}{x}{sep}")).collect::<Vec<_>>().join(" ");
            let more = if names.len() > 4 { " …" } else { "" };
            found.push((format!("{}가 한 줄에", quoted("'", 2)), format!("{}{more} 앞에서 줄을 나누고 이름을 굵게", quoted("\"", 4))));
            out.extend(items);
        } else if let Some((items, n)) = comma_list(l) {
            found.push((format!("쉼표로 이은 항목 {n}개"), "쉼표마다 줄을 나눠요 (내용은 그대로)".into()));
            out.extend(items);
        } else {
            out.push(l.clone());
        }
    }
    let (badge, _) = found.first()?.clone();
    let mut why: Vec<String> = found.into_iter().map(|f| f.1).collect();
    why.dedup();
    Some((out, badge, why.join(" · ")))
}

/// 글자로 덮어도 잃는 것이 없는 칸: 속성 없는 줄 태그(br · div · p)와 html_lines 가 푸는 문자 참조뿐.
/// 그림 · 소리 · 굵게 · 색 · 정렬이 있는 칸은 아니다 (고쳐 쓰기가 막는다)
pub(crate) fn plain_field(html: &str) -> bool {
    if html.contains("[sound:") {
        return false;
    }
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        let Some(j) = rest[i..].find('>') else { return false };
        let inner = rest[i + 1..i + j].trim_start_matches('/').trim_end_matches('/').trim();
        if !["br", "div", "p"].contains(&inner.to_ascii_lowercase().as_str()) {
            return false;
        }
        rest = &rest[i + j + 1..];
    }
    const KNOWN: [&str; 6] = ["&nbsp;", "&lt;", "&gt;", "&quot;", "&#39;", "&amp;"];
    html.match_indices('&').all(|(i, _)| {
        let t = &html[i + 1..];
        let name: String = t.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '#').collect();
        name.is_empty() || !t[name.len()..].starts_with(';') || KNOWN.iter().any(|k| html[i..].starts_with(k))
    })
}

/// 칸에 그림 · 소리 · 영상이 있다 (글자가 없어도 빈 답이 아니다)
fn has_media(html: &str) -> bool {
    let l = html.to_ascii_lowercase();
    ["<img", "[sound:", "<audio", "<video", "<svg", "<object", "<iframe", "<canvas"].iter().any(|m| l.contains(m))
}

/// 고쳐 쓰기 화면의 글자: 줄마다 띄어쓰기를 하나로, 빈 줄은 뺀다 (html_lines 와 같은 모양)
pub(crate) fn tidy(text: &str) -> String {
    text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 필드 → 고쳐 쓰기 화면의 글자 (태그 · [sound:] 를 뺀 줄들)
fn field_text(html: &str) -> String {
    html_lines(&render::strip_sound(html)).join("\n")
}

/// 질문 · 답 대응과 서식 문제 (G1 목록 · 고쳐 쓰기 제안). 카드 모양(text_flags) · G3 신호(card_flags)와 따로 센다
fn fix_flags(t: &CardText) -> Vec<Flag> {
    let Some((_, a)) = t.qa.filter(|_| t.checked) else { return vec![] };
    let field = &t.fields[a];
    let mut out = vec![];
    if let Some(reason) = mismatch(&t.question, &t.answer, has_media(field)) {
        out.push(Flag { reason: Some(reason), ..Flag::new("mismatch", 0) });
    }
    if plain_field(field)
        && let Some((lines, badge, why)) = format_lines(&html_lines(field))
    {
        out.push(Flag { reason: Some(why), badge: Some(badge), ..Flag::new("format", lines.len() as i64) });
    }
    out
}

// ------------------------------------------------------------------ 카드 읽기

/// 카드의 질문 글자와 답 줄들 (화면에 보이는 그대로, 템플릿을 적용해서)
struct CardText {
    deck_id: i64,
    question: String,
    answer: Vec<String>,
    note_id: i64,
    note_type_id: i64,
    /// 노트 필드 (자리 순서, 유형의 자리 수만큼)
    fields: Vec<String>,
    /// 이 카드의 질문 · 답이 각각 필드 하나에서 오면 그 자리 (고쳐 쓰기는 이때만)
    qa: Option<(usize, usize)>,
    /// 질문과 답의 대응 · 서식 규칙을 볼 카드: 질문 · 답이 필드 하나씩인 일반 카드 (빈칸 · 그림 가리기 · 목록 · 선택형은 뺀다)
    checked: bool,
}

/// 템플릿에서 질문 · 답 필드 자리. 앞면이 쓰는 필드가 하나, 뒷면이 더 쓰는 필드가 하나일 때만 (그 밖에는 어느 칸을 고칠지 모른다)
fn qa_slots(slot_names: &[String], qfmt: &str, afmt: &str) -> Option<(usize, usize)> {
    let q = notetype::refs(qfmt);
    let a: Vec<String> = notetype::refs(afmt).into_iter().filter(|n| !q.contains(n)).collect();
    let slot = |n: &String| slot_names.iter().position(|s| s == n);
    match (&q[..], &a[..]) {
        ([q], [a]) => Some((slot(q)?, slot(a)?)),
        _ => None,
    }
}

fn card_texts(con: &Connection, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Res<Vec<(i64, CardText)>> {
    let names = commands::note_field_names(con)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.deck_id, n.fields, n.note_type_id, t.qfmt, t.afmt, c.template_ord, n.id
             FROM card c JOIN note n ON n.id = c.note_id
             LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {}
             WHERE {filter} ORDER BY c.id",
            crate::notetype::TEMPLATE_OF_CARD
        ))
        .map_err(err)?;
    let rows = st
        .query_map(args, |r| {
            let fields: String = r.get(2)?;
            let nt: i64 = r.get(3)?;
            let q: Option<String> = r.get(4)?;
            let a: Option<String> = r.get(5)?;
            let rendered = commands::render_fields(&names, nt, &fields, q.as_deref(), a.as_deref(), r.get(6)?);
            // Q 목록 · 선택형은 항목이 많아도 나누기 제안을 하지 않는다 (나누면 일반 카드가 된다. Q4 는 10개 넘을 때 경고만)
            let special = names.get(&nt).is_some_and(|(name, n)| crate::recall::is_special(name, n));
            let slot_names = names.get(&nt).map_or(&[][..], |(_, n)| &n[..]);
            let mut values: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
            values.resize(values.len().max(slot_names.len()), String::new());
            let qa = q.as_deref().zip(a.as_deref()).and_then(|(q, a)| qa_slots(slot_names, q, a));
            let plain = q.as_deref().is_some_and(|q| !notetype::cloze_like(q) && !crate::occlusion::is_template(q));
            Ok((
                r.get::<_, i64>(0)?,
                CardText {
                    deck_id: r.get(1)?,
                    question: render::strip_tags(&rendered.front),
                    answer: if special { vec![] } else { html_lines(commands::answer_part(&rendered.back)) },
                    note_id: r.get(7)?,
                    note_type_id: nt,
                    fields: values,
                    checked: qa.is_some() && plain && !special,
                    qa,
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
        flags.push(Flag::new("again", again));
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

/// 덱(하위 덱 포함)에서 카드 모양에 문제가 있는 카드. Again 은 모양 문제와 겹칠 때만 붙는다 (보기는 화면이 거른다).
/// 질문 · 답 대응(mismatch) · 서식(format)은 그 뒤에 붙는다 (Again 과 상관없이)
#[tauri::command(async)]
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
        let shape = text_flags(&t.question, &t.answer);
        let fix = fix_flags(&t);
        if shape.is_empty() && fix.is_empty() {
            continue;
        }
        let mut flags = if shape.is_empty() { shape } else { with_again(shape, again.get(&id).copied().unwrap_or(0)) };
        flags.extend(fix);
        out.push(QualityCard { id, question: t.question, path: commands::path_of(t.deck_id, &by_id), flags, hidden: hidden.contains_key(&id) });
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

/// 고쳐 쓰기 글자 → 필드 HTML. to_html 에 더해 짝이 맞는 **…** 를 굵게(<b>)로 (서식 제안의 "이름:" 머리말)
fn to_rich_html(text: &str) -> String {
    let html = to_html(text);
    let parts: Vec<&str> = html.split("**").collect();
    // 짝이 맞는 수만큼만 굵게. 마지막 짝 없는 ** 는 글자 그대로
    let pairs = (parts.len() - 1) / 2;
    let mut out = String::from(parts[0]);
    for (i, p) in parts[1..].iter().enumerate() {
        out.push_str(if i >= pairs * 2 { "**" } else if i % 2 == 0 { "<b>" } else { "</b>" });
        out.push_str(p);
    }
    out
}

/// 필드 HTML → 고쳐 쓰기 글자, 굵게는 **…** 로 (전후 보기)
fn rich_text(html: &str) -> String {
    let b = html.replace("<b>", "**").replace("</b>", "**").replace("<strong>", "**").replace("</strong>", "**");
    field_text(&b)
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
pub(crate) fn has_batchim(s: &str) -> bool {
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
        return Err("나눌 카드가 없어요. 질문과 답을 채워 주세요.".into());
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
    let summary = format!("「{}」 {}장으로 나누기", short(&question), parts.len());
    tx.execute(
        "INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at) VALUES ('split', ?1, ?2, ?3, ?4, ?5)",
        params![deck_id, card_id, summary, payload, now_ms()],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(err)?;
    Ok(id)
}

/// 변경 기록의 카드 이름 (질문 앞 40자)
fn short(question: &str) -> String {
    let head: String = question.chars().take(40).collect();
    if question.chars().count() > 40 { format!("{head}…") } else { head }
}

/// 변경 하나를 되돌린다 (그 뒤에 고친 고쳐 쓰기는 되돌리지 않는다). 테스트용 (R1 초안 · 자료 카드 테스트도 쓴다)
#[cfg(test)]
pub(crate) fn undo(con: &mut Connection, change_id: i64) -> Res<()> {
    if undo_at(con, change_id, false)? { Ok(()) } else { Err(CHANGED_SINCE.into()) }
}

/// 변경 하나를 되돌린다. 나누기 · 초안: 새 카드를 지우고 원래 카드를 원래 상태로. 고쳐 쓰기: undo_rewrite.
/// Ok(false) = 고쳐 쓰기 뒤에 그 칸을 또 고쳐서 되돌리지 않았다 (아무것도 바꾸지 않는다). force 는 화면이
/// "그 뒤에 고친 내용이 있어요 · 되돌리면 그 내용도 사라져요" 확인을 받은 뒤에만 준다 (CQ2)
pub(crate) fn undo_at(con: &mut Connection, change_id: i64, force: bool) -> Res<bool> {
    let tx = con.transaction().map_err(err)?;
    let (kind, card_id, payload, undone): (String, Option<i64>, String, Option<i64>) = tx
        .query_row("SELECT kind, card_id, payload, undone_at FROM card_change WHERE id = ?1", [change_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()
        .map_err(err)?
        .ok_or("변경 기록을 찾을 수 없어요.")?;
    if undone.is_some() {
        return Err("이미 되돌린 변경이에요.".into());
    }
    if kind == "rewrite" {
        if !undo_rewrite(&tx, &payload, force)? {
            return Ok(false);
        }
    } else {
        let p: SplitPayload = serde_json::from_str(&payload).map_err(err)?;
        for nid in &p.new_notes {
            // 새 카드에 복습 기록이 있어도 기록은 남는다 (review_log.card_id 가 NULL 이 된다)
            tx.execute("DELETE FROM note WHERE id = ?1", [nid]).map_err(err)?;
        }
        if let (Some(card), Some(prev)) = (card_id, p.prev_state) {
            tx.execute("UPDATE card SET state = ?2, updated_at = ?3 WHERE id = ?1", params![card, prev, now_ms()]).map_err(err)?;
        }
    }
    tx.execute("UPDATE card_change SET undone_at = ?2 WHERE id = ?1", params![change_id, now_ms()]).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(true)
}

#[tauri::command]
pub fn split_apply(state: State<AppState>, card_id: i64, parts: Vec<SplitPart>, archive_original: bool) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    apply_split(&mut con, card_id, &parts, archive_original)
}

#[tauri::command]
/// false: 고쳐 쓰기 뒤에 또 고쳐서 되돌리지 않았다 → 화면이 확인을 받고 force 로 다시 부른다
pub fn undo_change(state: State<AppState>, change_id: i64, force: Option<bool>) -> Res<bool> {
    let mut con = state.db.lock().map_err(err)?;
    undo_at(&mut con, change_id, force.unwrap_or(false))
}

#[derive(Serialize)]
pub struct ChangeRow {
    id: i64,
    /// split | rewrite
    kind: String,
    /// rewrite 만: format | ai | edit
    how: Option<String>,
    summary: String,
    card_id: Option<i64>,
    created_at: i64,
    undone: bool,
}

/// G1 "최근 나눈 카드" · "최근 고친 카드"(7일) 30개 (R1 초안은 토스트의 [되돌리기]로만)
#[tauri::command]
pub fn recent_changes(state: State<AppState>, deck_id: i64) -> Res<Vec<ChangeRow>> {
    let con = state.db.lock().map_err(err)?;
    let decks = commands::load_decks(&con)?;
    let ids = commands::descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut st = con
        .prepare(&format!(
            "SELECT id, kind, json_extract(payload, '$.how'), summary, card_id, created_at, undone_at IS NOT NULL FROM card_change
             WHERE (kind = 'split' OR (kind = 'rewrite' AND created_at >= ?1)) AND deck_id IN ({ids}) ORDER BY created_at DESC LIMIT 30"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([now_ms() - REWRITE_DAYS * DAY_MS], |r| {
            Ok(ChangeRow { id: r.get(0)?, kind: r.get(1)?, how: r.get(2)?, summary: r.get(3)?, card_id: r.get(4)?, created_at: r.get(5)?, undone: r.get(6)? })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

// ------------------------------------------------------------------ 8.2 고쳐 쓰기 (서식 제안 · 질문과 답 맞추기)

const NOT_ONE_FIELD: &str = "이 카드는 질문과 답이 필드 하나씩이 아니라 여기서 고칠 수 없어요. 카드 편집에서 고쳐 주세요.";
const LOCKED: &str = "그림 · 소리 · 서식이 있는 칸은 여기서 고칠 수 없어요. 카드 편집에서 고쳐 주세요.";
#[cfg(test)]
const CHANGED_SINCE: &str = "그 뒤에 고친 내용이 있어 되돌리지 않았어요.";

/// 고쳐 쓰기 전후. 글자 그대로 (줄바꿈으로 줄을 나눈다). 적용할 때 HTML 로 바꾼다
#[derive(Serialize)]
pub struct Rewrite {
    card_id: i64,
    /// 지금 질문 · 답 (필드 글자). ai.rs 는 이 글자만 보낸다
    pub(crate) question: String,
    pub(crate) answer: String,
    /// 규칙 제안 (서식). 고칠 것이 없으면 지금과 같다
    new_question: String,
    new_answer: String,
    /// 걸린 규칙의 이유들 (" · " 로 잇는다). 없으면 빈 글자
    pub(crate) why: String,
    /// 그림 · 소리 · 서식이 있어 글자로 덮을 수 없는 칸 [질문, 답]
    pub(crate) locked: [bool; 2],
    /// [전후 보기]: 고친 뒤 그 칸을 또 고쳤다 (new_* 는 지금 값)
    edited_since: bool,
}

/// 고쳐 쓰기 제안. 질문 · 답이 필드 하나씩인 일반 카드만 (빈칸 · 그림 가리기 · 목록 · 선택형은 카드 편집에서)
pub(crate) fn rewrite_of(con: &Connection, card_id: i64) -> Res<Rewrite> {
    let (_, t) = card_texts(con, "c.id = ?1", &[&card_id])?.into_iter().next().ok_or("카드를 찾을 수 없어요.")?;
    let (q, a) = t.qa.filter(|_| t.checked).ok_or(NOT_ONE_FIELD)?;
    let (question, answer) = (field_text(&t.fields[q]), field_text(&t.fields[a]));
    let locked = [!plain_field(&t.fields[q]), !plain_field(&t.fields[a])];
    let format = if locked[1] { None } else { format_lines(&html_lines(&t.fields[a])) };
    let new_answer = format.map_or_else(|| answer.clone(), |(l, ..)| l.join("\n"));
    let why = fix_flags(&t).iter().filter_map(|f| f.reason.clone()).collect::<Vec<_>>().join(" · ");
    Ok(Rewrite { card_id, new_question: question.clone(), question, answer, new_answer, why, locked, edited_since: false })
}

#[tauri::command]
pub fn rewrite_suggest(state: State<AppState>, card_id: i64) -> Res<Rewrite> {
    rewrite_of(&*state.db.lock().map_err(err)?, card_id)
}

/// 고쳐 쓰기를 어떻게 했나 ("최근 고친 카드"의 둘째 줄): format(서식 고치기) · ai(AI로 확인한 뒤) · edit(직접)
const HOWS: [&str; 3] = ["format", "ai", "edit"];
/// "최근 고친 카드"에 두는 기간 (CQ1)
const REWRITE_DAYS: i64 = 7;

/// card_change 'rewrite'. 바꾼 자리와 그 자리의 이전 · 이후 필드 HTML 그대로. qa: 질문 · 답 자리 (전후 보기)
#[derive(Deserialize, Serialize)]
struct RewritePayload {
    note_id: i64,
    qa: [usize; 2],
    slots: Vec<usize>,
    prev: Vec<String>,
    next: Vec<String>,
    how: String,
}

/// 승인한 고쳐 쓰기. 바뀐 칸만 보통 편집처럼 쓴다 (save_note_fields, 동기화 그대로). 학습 기록 · 일정은 그대로다.
/// 빈칸 번호가 줄어 카드가 지워질 고쳐 쓰기(I14)는 막는다. 한 번의 변경으로 기록해 change id 를 돌려준다.
/// 글자의 짝 맞는 **…** 는 굵게가 된다 (서식 제안의 "이름:" 머리말)
pub(crate) fn apply_rewrite(con: &mut Connection, card_id: i64, question: &str, answer: &str, how: &str) -> Res<i64> {
    let tx = con.transaction().map_err(err)?;
    let (_, t) = card_texts(&tx, "c.id = ?1", &[&card_id])?.into_iter().next().ok_or("카드를 찾을 수 없어요.")?;
    let (q, a) = t.qa.ok_or(NOT_ONE_FIELD)?;
    let mut next = t.fields.clone();
    let mut slots = vec![];
    for (slot, text) in [(q, question), (a, answer)] {
        let text = tidy(text);
        if text == field_text(&t.fields[slot]) {
            continue;
        }
        if !plain_field(&t.fields[slot]) {
            return Err(LOCKED.into());
        }
        if text.is_empty() {
            return Err("질문과 답을 비울 수 없어요.".into());
        }
        next[slot] = to_rich_html(&text);
        slots.push(slot);
    }
    if slots.is_empty() {
        return Err("바뀐 것이 없어요.".into());
    }
    let nt = notetype::load(&tx, t.note_type_id)?;
    if nt.cloze() {
        let keep = notetype::card_ords(&nt, &next);
        let ords: Vec<u32> = {
            let mut st = tx.prepare("SELECT template_ord FROM card WHERE note_id = ?1").map_err(err)?;
            st.query_map([t.note_id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
        };
        let lost = ords.iter().filter(|o| !keep.contains(o)).count();
        if lost > 0 {
            return Err(format!("빈칸 번호가 줄어 카드 {lost}장이 지워져요. 카드 편집에서 직접 고쳐 주세요."));
        }
    }
    commands::save_note_fields(&tx, t.note_id, &next)?;
    let payload = RewritePayload {
        note_id: t.note_id,
        qa: [q, a],
        prev: slots.iter().map(|s| t.fields[*s].clone()).collect(),
        next: slots.iter().map(|s| next[*s].clone()).collect(),
        slots,
        how: (if HOWS.contains(&how) { how } else { "edit" }).to_string(),
    };
    // "최근 고친 카드"는 줄마다 질문을 그대로 보인다 (CQ1)
    tx.execute(
        "INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at) VALUES ('rewrite', ?1, ?2, ?3, ?4, ?5)",
        params![t.deck_id, card_id, short(&t.question), serde_json::to_string(&payload).map_err(err)?, now_ms()],
    )
    .map_err(err)?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(err)?;
    Ok(id)
}

/// 고쳐 쓰기를 되돌린다: 지금 값이 적용한 값과 같을 때만 이전 값으로 (보통 편집으로 써서 동기화된다).
/// 그 뒤에 이 기기나 다른 기기에서 고쳤으면 false (그대로 둔다). force 면 그 칸도 이전 값으로 덮는다
fn undo_rewrite(tx: &Connection, payload: &str, force: bool) -> Res<bool> {
    let p: RewritePayload = serde_json::from_str(payload).map_err(err)?;
    let stored: String = tx
        .query_row("SELECT fields FROM note WHERE id = ?1", [p.note_id], |r| r.get(0))
        .optional()
        .map_err(err)?
        .ok_or("고친 카드가 지워져서 되돌릴 수 없어요.")?;
    let mut fields: Vec<String> = serde_json::from_str(&stored).map_err(err)?;
    if !force && p.slots.iter().zip(&p.next).any(|(s, v)| fields.get(*s) != Some(v)) {
        return Ok(false);
    }
    fields.resize(fields.len().max(p.slots.iter().max().map_or(0, |m| m + 1)), String::new());
    for (s, v) in p.slots.iter().zip(p.prev) {
        fields[*s] = v;
    }
    commands::save_note_fields(tx, p.note_id, &fields)?;
    Ok(true)
}

#[tauri::command]
pub fn rewrite_apply(state: State<AppState>, card_id: i64, question: String, answer: String, how: String) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    apply_rewrite(&mut con, card_id, &question, &answer, &how)
}

/// [전후 보기]: 고친 변경의 전후 (Rewrite 모양). 오른쪽은 지금 카드: 고친 뒤 그 칸을 또 고쳤으면 지금 값과 edited_since. 굵게는 **…**
pub(crate) fn change_view(con: &Connection, change_id: i64) -> Res<Rewrite> {
    let (card_id, payload): (Option<i64>, String) = con
        .query_row("SELECT card_id, payload FROM card_change WHERE id = ?1 AND kind = 'rewrite'", [change_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(err)?
        .ok_or("변경 기록을 찾을 수 없어요.")?;
    let p: RewritePayload = serde_json::from_str(&payload).map_err(err)?;
    let stored: String = con
        .query_row("SELECT fields FROM note WHERE id = ?1", [p.note_id], |r| r.get(0))
        .optional()
        .map_err(err)?
        .ok_or("고친 카드가 지워졌어요.")?;
    let fields: Vec<String> = serde_json::from_str(&stored).map_err(err)?;
    let now = |slot: usize| fields.get(slot).map_or("", String::as_str);
    let side = |slot: usize| match p.slots.iter().position(|s| *s == slot) {
        Some(i) => (rich_text(&p.prev[i]), rich_text(now(slot))),
        None => (rich_text(now(slot)), rich_text(now(slot))),
    };
    let edited_since = p.slots.iter().zip(&p.next).any(|(s, v)| now(*s) != v);
    let ((question, new_question), (answer, new_answer)) = (side(p.qa[0]), side(p.qa[1]));
    Ok(Rewrite { card_id: card_id.unwrap_or(0), question, answer, new_question, new_answer, why: String::new(), locked: [true, true], edited_since })
}

#[tauri::command]
pub fn rewrite_change(state: State<AppState>, change_id: i64) -> Res<Rewrite> {
    change_view(&*state.db.lock().map_err(err)?, change_id)
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
        assert_eq!(text_flags("조직 문화의 10가지 특성을 모두 쓰시오.", &ten), [Flag::new("items", 10)]);
        let nine: Vec<String> = (0..9).map(|i| format!("설명 {i}")).collect();
        assert_eq!(text_flags("절차는?", &nine), [Flag::new("lines", 9)]);
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

    // ------------------------------------------------------------------ 8.2 질문과 답의 대응 · 서식 · 고쳐 쓰기

    fn mm(q: &str, a: &str) -> Option<String> {
        mismatch(q, &lines(a), false)
    }

    #[test]
    fn mismatch_empty_and_placeholder() {
        assert_eq!(mm("광합성이란?", ""), Some("답이 비어 있어요".into()));
        assert_eq!(mismatch("이 그림의 이름은?", &[], true), None, "그림 · 소리만 있는 답은 빈 답이 아니다");
        assert_eq!(mm("광합성이란?", "?"), Some("답이 \"?\"뿐이에요".into()));
        assert_eq!(mm("광합성이란?", "TODO"), Some("답이 \"TODO\"뿐이에요".into()));
        assert_eq!(mm("광합성이란?", "모름"), Some("답이 \"모름\"뿐이에요".into()));
        assert_eq!(mm("광합성이란?", "..."), Some("답이 \"...\"뿐이에요".into()), "문장 부호뿐");
        assert_eq!(mm("빛의 한 종류는?", "X선"), None);
    }

    #[test]
    fn mismatch_repeat_and_swapped() {
        assert_eq!(mm("광합성이란?", "광합성"), Some("답이 질문을 그대로 되풀이해요".into()));
        assert_eq!(mm("Paris", "paris"), Some("답이 질문을 그대로 되풀이해요".into()), "대소문자 · 문장 부호는 보지 않는다");
        assert_eq!(mm("광합성이란?", "빛 에너지로 포도당을 만드는 과정"), None);
        assert_eq!(mm("광합성", "빛으로 포도당을 만드는 과정은?"), Some("답이 물음으로 끝나요. 앞면과 뒷면이 바뀌었을 수 있어요".into()));
        assert_eq!(mm("광합성은?", "빛으로 무엇을 만들까?"), None, "질문도 물음이면 바뀐 것이 아니다");
        assert_eq!(mm("광합성", "왜 중요할까?\n산소를 만든다"), None, "여러 줄 답");
    }

    #[test]
    fn mismatch_count() {
        let four = "① 물리\n② 데이터 링크\n③ 네트워크\n④ 전송";
        assert_eq!(mm("OSI 하위 계층 세 가지를 쓰시오.", four), Some("질문은 \"3가지\"를 묻는데 답은 4개예요".into()));
        assert_eq!(mm("데이터 분석 4단계는?", "1. 수집\n2. 정제\n3. 분석"), Some("질문은 \"4단계\"를 묻는데 답은 3개예요".into()));
        assert_eq!(mm("정보 보안 3원칙 세 가지는?", "기밀성"), Some("질문은 \"3가지\"를 묻는데 답이 낱말 하나(\"기밀성\")예요".into()));
        assert_eq!(mm("OSI 하위 계층 네 가지를 쓰시오.", four), None, "수가 같다");
        assert_eq!(mm("조직 문화의 10가지 특성 중 1–3번째는?", "가\n나\n다"), None, "나눈 카드의 'N번째'");
        assert_eq!(mm("세 가지 중 하나를 고르면?", "가"), None, "'… 중'");
        assert_eq!(mm("장점을 3가지 이상 쓰시오.", "① 가\n② 나\n③ 다\n④ 라\n⑤ 마"), None, "'이상'");
        assert_eq!(mm("두 개념을 비교하면?", "가 · 나"), None, "'두 개념'은 개수를 묻지 않는다");
        assert_eq!(mm("3개월 뒤 결과는?", "가\n나"), None, "'3개월'");
        assert_eq!(mm("세 가지 장점은?", "1. 가\n- 가의 예\n- 가의 다른 예\n2. 나\n3. 다"), None, "하위 항목은 세지 않는다");
        assert_eq!(mm("세 가지 장점은?", "비용이 싸고 빠르며 안전하다"), None, "풀어 쓴 답은 세지 않는다");
        assert_eq!(mm("2가지 장점과 3가지 단점은?", "가\n나"), None, "묻는 수가 둘이면 보지 않는다");
    }

    #[test]
    fn mismatch_form() {
        assert_eq!(mm("TCP와 UDP의 차이를 설명하시오.", "80"), Some("질문은 \"차이\"를 묻는데 답이 숫자 하나(\"80\")예요".into()));
        assert_eq!(mm("하늘이 파란 이유는?", "산란"), Some("질문은 \"이유\"를 묻는데 답이 낱말 하나(\"산란\")예요".into()));
        assert_eq!(mm("하늘은 왜 파란가?", "산란"), Some("질문은 \"왜\"를 묻는데 답이 낱말 하나(\"산란\")예요".into()));
        assert_eq!(mm("정규화 과정은?", "분해"), Some("질문은 \"과정\"을 묻는데 답이 낱말 하나(\"분해\")예요".into()), "받침 뒤는 '을'");
        assert_eq!(mm("TCP와 UDP의 차이는?", "TCP는 연결형, UDP는 비연결형"), None);
        assert_eq!(mm("과대적합을 막는 방법은?", "드롭아웃"), None, "'방법'은 낱말 하나로도 답이 된다");
        assert_eq!(mm("영상 왜곡의 원인은?", "렌즈"), None, "'왜곡'은 '왜'가 아니다");
    }

    #[test]
    fn mismatch_number() {
        assert_eq!(mm("OSI 모형은 몇 계층인가?", "물리 계층부터 응용 계층까지"), Some("질문은 \"몇\"을 묻는데 답에 수가 없어요".into()));
        assert_eq!(mm("조선의 건국 연도는?", "고려 말기"), Some("질문은 \"연도\"를 묻는데 답에 수가 없어요".into()));
        assert_eq!(mm("OSI 모형은 몇 계층인가?", "7계층"), None);
        assert_eq!(mm("OSI 모형은 몇 계층인가?", "일곱 계층"), None, "수 낱말");
        assert_eq!(mm("조선의 건국 연도는?", "1392년"), None);
        assert_eq!(mm("몇몇 학자가 주장한 이론은?", "진화론"), None, "'몇몇'은 수를 묻지 않는다");
        assert_eq!(mm("OSI 모형은 몇 계층인가?", "물리, 데이터 링크, 네트워크"), None, "목록으로 답했다");
        assert_eq!(mm("How many layers does the OSI model have?", "seven"), None, "영어 수 낱말 (QA L2)");
        assert_eq!(mm("How many layers does the OSI model have?", "twenty-one"), None);
        assert_eq!(mm("How many layers does the OSI model have?", "from physical to application"), Some("질문은 \"how many\"를 묻는데 답에 수가 없어요".into()), "질문에 있는 말을 인용한다");
    }

    #[test]
    fn format_inline_numbers() {
        let one = lines("1. 응용 계층 2. 표현 계층 3. 세션 계층 4. 전송 계층 5. 네트워크 계층 6. 데이터 링크 계층 7. 물리 계층");
        let (out, badge, why) = format_lines(&one).unwrap();
        assert_eq!(out.len(), 7);
        assert_eq!((out[0].as_str(), out[6].as_str()), ("1. 응용 계층", "7. 물리 계층"));
        assert_eq!((badge.as_str(), why.as_str()), ("한 줄에 번호 7개", "번호마다 줄을 나눠요 (내용은 그대로)"));
        assert_eq!(format_lines(&lines("특징: 1. 가 2. 나 3. 다")).unwrap().0, ["특징:", "1. 가", "2. 나", "3. 다"], "앞말은 제 줄에");
        assert_eq!(format_lines(&lines("①가②나③다")).unwrap().0, ["①가", "②나", "③다"], "①②는 붙여 써도");
        assert_eq!(format_lines(&lines("(1) 가 (2) 나 (3) 다")).unwrap().0.len(), 3);
        assert!(format_lines(&lines("① 가 ② 나")).is_none(), "번호 2개는 그대로 (3개부터)");
        assert!(format_lines(&lines("2. 가 3. 나 4. 다")).is_none(), "1부터가 아니다");
        assert!(format_lines(&lines("원주율은 3.14 이고 1.5 배")).is_none(), "소수");
        assert!(format_lines(&lines("1. 가\n2. 나\n3. 다")).is_none(), "이미 줄마다");
        assert!(format_lines(&lines("1. 2. 3.")).is_none(), "번호뿐");
    }

    #[test]
    fn format_labels_and_commas() {
        let (out, badge, why) = format_lines(&lines("1NF: 원자값 2NF: 부분 함수 종속 제거 3NF: 이행 종속 제거")).unwrap();
        assert_eq!(out, ["**1NF:** 원자값", "**2NF:** 부분 함수 종속 제거", "**3NF:** 이행 종속 제거"]);
        assert_eq!(badge, "'1NF:' '2NF:'가 한 줄에");
        assert_eq!(why, "\"1NF:\" \"2NF:\" \"3NF:\" 앞에서 줄을 나누고 이름을 굵게");
        assert!(format_lines(&lines("예: 사과")).is_none(), "머리말 하나");
        assert!(format_lines(&lines("회의는 10:30 에서 11:00 까지")).is_none(), "시각");
        let list = "정보 시스템의 구성 요소: 하드웨어, 소프트웨어, 데이터, 절차, 사람(사용자, 관리자)";
        let (out, badge, _) = format_lines(&lines(list)).unwrap();
        assert_eq!(out, ["정보 시스템의 구성 요소:", "하드웨어", "소프트웨어", "데이터", "절차", "사람(사용자, 관리자)"], "괄호 안 쉼표는 그대로");
        assert_eq!(badge, "쉼표로 이은 항목 5개");
        assert!(format_lines(&lines("이 방법은 처리 속도가 빠르고, 비용이 저렴하며, 결과가 안정적이라 많이 쓴다.")).is_none(), "문장");
        assert!(format_lines(&lines("가, 나, 다")).is_none(), "짧은 줄");
        let money = "가격표 정리: 1,000원 기본형, 2,000원 고급형, 3,000원 전문가형";
        assert_eq!(format_lines(&lines(money)).unwrap().0[1], "1,000원 기본형", "숫자 사이 쉼표는 자르지 않는다");
    }

    #[test]
    fn plain_fields_and_rich_html() {
        assert!(plain_field("<div>가</div><br>&nbsp;나 &amp; 다 &lt;b&gt; A & B &#39;"));
        assert!(plain_field("<p>가</p><br/><br />"));
        for not in ["<b>가</b>", "<img src=\"a.png\">", "가 [sound:a.mp3]", "<div style=\"color:red\">가</div>", "가&hellip;", "<span>가</span>"] {
            assert!(!plain_field(not), "{not}");
        }
        assert_eq!(to_rich_html("**1NF:** 원자값\n2NF: a<b"), "<b>1NF:</b> 원자값<br>2NF: a&lt;b");
        assert_eq!(to_rich_html("별표 ** 하나"), "별표 ** 하나", "짝 없는 ** 는 글자 그대로");
        assert_eq!(rich_text("<b>1NF:</b> 원자값<br>2"), "**1NF:** 원자값\n2");
    }

    fn note(con: &Connection, kind: &str, front: &str, back: &str) -> i64 {
        con.execute("INSERT OR IGNORE INTO deck (id, name, created_at, updated_at) VALUES (1, 'd', 0, 0)", []).unwrap();
        commands::insert_note(con, &NewCard { deck_id: 1, kind: kind.into(), front: front.into(), back: back.into(), importance: None }, "", None).unwrap();
        con.query_row("SELECT min(id) FROM card WHERE note_id = (SELECT max(id) FROM note)", [], |r| r.get(0)).unwrap()
    }

    fn fields_of(con: &Connection, card: i64) -> Vec<String> {
        let f: String = con.query_row("SELECT n.fields FROM note n JOIN card c ON c.note_id = n.id WHERE c.id = ?1", [card], |r| r.get(0)).unwrap();
        serde_json::from_str(&f).unwrap()
    }

    #[test]
    fn fix_flags_are_separate_from_split_flags() {
        let con = crate::db::open_in_memory().unwrap();
        let card = note(&con, "basic", "OSI 7계층을 위에서부터 쓰시오.", "<div>1. 응용 2. 표현 3. 세션 4. 전송 5. 네트워크 6. 데이터 링크 7. 물리</div>");
        let (_, t) = card_texts(&con, "c.id = ?1", &[&card]).unwrap().remove(0);
        let f = fix_flags(&t);
        assert_eq!(f.iter().map(|f| f.kind).collect::<Vec<_>>(), ["format"]);
        assert_eq!(f[0].badge.as_deref(), Some("한 줄에 번호 7개"));
        assert!(card_flags(&con, card).unwrap().is_empty(), "G3 학습 신호(card_flags)는 그대로 모양 문제만 본다");
        let bad = note(&con, "basic", "TCP와 UDP의 차이를 설명하시오.", "80");
        let (_, t) = card_texts(&con, "c.id = ?1", &[&bad]).unwrap().remove(0);
        assert_eq!(fix_flags(&t)[0].reason.as_deref(), Some("질문은 \"차이\"를 묻는데 답이 숫자 하나(\"80\")예요"));
        // 빈칸 카드는 대응 · 서식 규칙을 보지 않는다
        let cloze = note(&con, "cloze", "{{c1::A}} 와 {{c2::B}}", "");
        let (_, t) = card_texts(&con, "c.id = ?1", &[&cloze]).unwrap().remove(0);
        assert!(fix_flags(&t).is_empty() && t.qa == Some((0, 1)));
        assert!(rewrite_of(&con, cloze).is_err(), "빈칸 카드는 제안을 만들지 않는다");
    }

    #[test]
    fn rewrite_apply_and_undo() {
        let mut con = crate::db::open_in_memory().unwrap();
        let card = note(&con, "basic", "OSI 7계층을 위에서부터 쓰시오.", "<div>1. 응용 2. 표현 3. 세션</div>");
        con.execute("UPDATE card SET state = 'review', due_at = 5 WHERE id = ?1", [card]).unwrap();
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (?1, 1, 3)", [card]).unwrap();
        let before = fields_of(&con, card);
        let r = rewrite_of(&con, card).unwrap();
        assert_eq!((r.answer.as_str(), r.new_answer.as_str()), ("1. 응용 2. 표현 3. 세션", "1. 응용\n2. 표현\n3. 세션"));
        assert_eq!(r.locked, [false, false]);
        assert!(r.why.contains("번호마다 줄을 나눠요"));
        let count = |con: &Connection, sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count(&con, "SELECT count(*) FROM card_change"), 0, "제안은 아무것도 바꾸지 않는다");

        let change = apply_rewrite(&mut con, card, &r.new_question, &r.new_answer, "format").unwrap();
        assert_eq!(fields_of(&con, card), [before[0].clone(), "1. 응용<br>2. 표현<br>3. 세션".to_string()], "바뀐 칸만 쓴다");
        assert_eq!(count(&con, &format!("SELECT count(*) FROM card WHERE id = {card} AND state = 'review' AND due_at = 5")), 1, "일정은 그대로");
        assert_eq!(count(&con, &format!("SELECT count(*) FROM review_log WHERE card_id = {card}")), 1, "기록은 그대로");
        let (kind, summary): (String, String) = con.query_row("SELECT kind, summary FROM card_change WHERE id = ?1", [change], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((kind.as_str(), summary.as_str()), ("rewrite", "OSI 7계층을 위에서부터 쓰시오."));
        let v = change_view(&con, change).unwrap();
        assert_eq!((v.answer.as_str(), v.new_answer.as_str(), v.question.as_str()), ("1. 응용 2. 표현 3. 세션", "1. 응용\n2. 표현\n3. 세션", "OSI 7계층을 위에서부터 쓰시오."));
        assert!(!v.edited_since);
        assert!(rewrite_of(&con, card).unwrap().why.is_empty(), "고친 뒤에는 서식 제안이 없다");

        assert_eq!(undo_at(&mut con, change, false), Ok(true));
        assert_eq!(fields_of(&con, card), before, "이전 값 그대로");
        assert!(undo_at(&mut con, change, false).is_err(), "두 번 되돌리지 않는다");

        // 바뀐 것이 없거나 비우면 쓰지 않는다
        assert!(apply_rewrite(&mut con, card, &r.question, "  1. 응용 2. 표현  3. 세션 ", "format").is_err(), "띄어쓰기만 다르면 바뀐 것이 없다");
        assert!(apply_rewrite(&mut con, card, &r.question, " \n ", "edit").is_err());
        assert_eq!(count(&con, "SELECT count(*) FROM card_change WHERE undone_at IS NULL"), 0);
    }

    #[test]
    fn rewrite_undo_after_later_edit_needs_force() {
        let mut con = crate::db::open_in_memory().unwrap();
        let card = note(&con, "basic", "TCP와 UDP의 차이를 설명하시오.", "80");
        let note_id: i64 = con.query_row("SELECT note_id FROM card WHERE id = ?1", [card], |r| r.get(0)).unwrap();
        let change = apply_rewrite(&mut con, card, "HTTP 기본 포트는?", "80", "ai").unwrap();
        assert_eq!(fields_of(&con, card)[0], "HTTP 기본 포트는?");
        // 그 뒤에 카드 편집(다른 기기 동기화도 같다)에서 또 고쳤다
        commands::save_note_fields(&con, note_id, &["HTTP 의 기본 포트 번호는?".into(), "80".into()]).unwrap();
        assert_eq!(undo_at(&mut con, change, false), Ok(false), "확인 없이 덮지 않는다");
        let v = change_view(&con, change).unwrap();
        assert_eq!((v.new_question.as_str(), v.edited_since), ("HTTP 의 기본 포트 번호는?", true), "전후 보기 오른쪽은 지금 값 (QA L1)");
        assert_eq!(fields_of(&con, card)[0], "HTTP 의 기본 포트 번호는?", "그대로 둔다");
        assert!(undo(&mut con, change).is_err());
        let undone: Option<i64> = con.query_row("SELECT undone_at FROM card_change WHERE id = ?1", [change], |r| r.get(0)).unwrap();
        assert_eq!(undone, None, "되돌린 것으로 적지 않는다");
        // 확인을 받으면 (force) 이전 값으로
        assert_eq!(undo_at(&mut con, change, true), Ok(true));
        assert_eq!(fields_of(&con, card), ["TCP와 UDP의 차이를 설명하시오.", "80"]);
        // 다른 칸만 고쳤으면 확인 없이 되돌린다
        let change = apply_rewrite(&mut con, card, "TCP와 UDP의 차이를 설명하시오.", "TCP 연결형\nUDP 비연결형", "edit").unwrap();
        let mut f = fields_of(&con, card);
        assert_eq!(f[1], "TCP 연결형<br>UDP 비연결형");
        f[0] = "TCP · UDP 차이는?".into();
        commands::save_note_fields(&con, note_id, &f).unwrap();
        assert_eq!(undo_at(&mut con, change, false), Ok(true));
        assert_eq!(fields_of(&con, card), ["TCP · UDP 차이는?", "80"], "고친 칸만 되돌리고 그 뒤에 고친 다른 칸은 남는다");
    }

    #[test]
    fn rewrite_keeps_media_and_cloze_cards() {
        let mut con = crate::db::open_in_memory().unwrap();
        // 그림 · 서식이 있는 칸은 글자로 덮지 않는다. 다른 칸은 고칠 수 있다
        let card = note(&con, "basic", "이 구조의 이름은?", "<b>세포막</b><img src=\"cell.png\">");
        let r = rewrite_of(&con, card).unwrap();
        assert_eq!((r.answer.as_str(), r.locked), ("세포막", [false, true]));
        assert!(apply_rewrite(&mut con, card, &r.question, "세포벽", "edit").is_err());
        apply_rewrite(&mut con, card, "그림 속 구조의 이름은?", &r.answer, "edit").unwrap();
        assert_eq!(fields_of(&con, card)[1], "<b>세포막</b><img src=\"cell.png\">", "답 칸은 그대로");
        // 굵게: 서식 제안의 **이름:** 은 <b> 로
        let card = note(&con, "basic", "정규화 단계별 조건은?", "1NF: 원자값 2NF: 부분 종속 제거 3NF: 이행 종속 제거");
        let r = rewrite_of(&con, card).unwrap();
        let change = apply_rewrite(&mut con, card, &r.new_question, &r.new_answer, "format").unwrap();
        assert_eq!(fields_of(&con, card)[1], "<b>1NF:</b> 원자값<br><b>2NF:</b> 부분 종속 제거<br><b>3NF:</b> 이행 종속 제거");
        assert_eq!(change_view(&con, change).unwrap().new_answer, r.new_answer, "전후 보기는 굵게를 ** 로");
        // I14 빈칸 번호가 줄면 카드가 지워지므로 막는다. 늘리는 것은 된다 (되돌리면 그 카드는 지워진다)
        let cloze = note(&con, "cloze", "{{c1::A}} 와 {{c2::B}}", "");
        let cards = |con: &Connection| con.query_row("SELECT count(*) FROM card WHERE note_id = (SELECT note_id FROM card WHERE id = ?1)", [cloze], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(cards(&con), 2);
        let e = apply_rewrite(&mut con, cloze, "{{c1::A}} 와 B", "", "edit").unwrap_err();
        assert!(e.contains("카드 1장이 지워져요"), "{e}");
        assert_eq!(cards(&con), 2);
        let change = apply_rewrite(&mut con, cloze, "{{c1::A}} 와 {{c2::B}} 와 {{c3::C}}", "", "edit").unwrap();
        assert_eq!(cards(&con), 3);
        undo(&mut con, change).unwrap();
        assert_eq!(cards(&con), 2);
    }
}
