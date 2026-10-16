//! 8.1 목록 회상형 · 선택형 (Figma Row Q · DECISIONS Q). 앱 전용 노트 유형 둘, 노트 1개 = 카드 1장 (Q1).
//!
//! 필드 (Q10): "Neo-Anki 목록 회상" = 질문 · 항목 · 순서 · 추가 설명, "Neo-Anki 선택형" = 질문 · 보기1–보기6 · 정답 · 섞기 · 추가 설명.
//!   - 항목: 한 줄에 하나 (<br> 로 나눈다). 순서: "순서대로" | "순서 없이"
//!   - 정답: 보기 번호 ("2", 여럿이면 "1,3"). 섞기: "섞기" | "섞지 않음"
//!
//! 템플릿은 스크립트 없이 필드만 쓴다 (db.rs NATIVE_TYPES). Anki 에서는 한 번에 다 보이는 기본 카드처럼 읽히고,
//! 정답 보기 굵게는 뒷면의 data-answer 속성 선택자(CSS)로 한다. 앱 학습 화면은 템플릿 대신 필드를 읽어 직접 그린다.
//! 다시 가져오면 유형 이름 + 필드로 알아본다 (M10 과 같은 방식). 이름만 같고 필드가 다르면 일반 노트로 (가져오기 결과에 한 줄).
//! 평가는 FSRS 4단계 그대로이고 앱은 추천만 한다 (Q3 · Q7). 항목별 빠뜨림 · 고른 보기는 이 기기에만 남기고 답 면에 "지난번"으로 보인다 (recall_mark, Q9 · Q30)

use crate::ai::{self, AiError};
use crate::commands::AppState;
use crate::db::now_ms;
use crate::{quality, rag, render, written};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub const LIST: &str = "Neo-Anki 목록 회상";
pub const CHOICE: &str = "Neo-Anki 선택형";
const LIST_FIELDS: [&str; 4] = ["질문", "항목", "순서", "추가 설명"];
const CHOICE_FIELDS: [&str; 10] = ["질문", "보기1", "보기2", "보기3", "보기4", "보기5", "보기6", "정답", "섞기", "추가 설명"];
pub const LIST_FIELDS_JSON: &str = r#"["질문","항목","순서","추가 설명"]"#;
pub const CHOICE_FIELDS_JSON: &str = r#"["질문","보기1","보기2","보기3","보기4","보기5","보기6","정답","섞기","추가 설명"]"#;
const ORDERED: &str = "순서대로";
const UNORDERED: &str = "순서 없이";
const SHUFFLE: &str = "섞기";
const NO_SHUFFLE: &str = "섞지 않음";
const MAX_OPTIONS: usize = 6;

pub const LIST_QFMT: &str = "{{질문}}<div class=\"neo-meta\">{{순서}}</div>";
pub const LIST_AFMT: &str =
    "{{FrontSide}}<hr id=answer><div class=\"neo-list\">{{항목}}</div>{{#추가 설명}}<div class=\"neo-extra\">{{추가 설명}}</div>{{/추가 설명}}";
pub const CHOICE_QFMT: &str = concat!(
    "{{질문}}<ol class=\"neo-choice\">",
    "{{#보기1}}<li class=\"o1\">{{보기1}}</li>{{/보기1}}{{#보기2}}<li class=\"o2\">{{보기2}}</li>{{/보기2}}",
    "{{#보기3}}<li class=\"o3\">{{보기3}}</li>{{/보기3}}{{#보기4}}<li class=\"o4\">{{보기4}}</li>{{/보기4}}",
    "{{#보기5}}<li class=\"o5\">{{보기5}}</li>{{/보기5}}{{#보기6}}<li class=\"o6\">{{보기6}}</li>{{/보기6}}</ol>",
);
pub const CHOICE_AFMT: &str = concat!(
    "<div class=\"neo-choice-back\" data-answer=\",{{정답}},\">{{FrontSide}}</div><hr id=answer>",
    "<div class=\"neo-meta\">정답 {{정답}}</div>{{#추가 설명}}<div class=\"neo-extra\">{{추가 설명}}</div>{{/추가 설명}}",
);
/// 내보낼 때만 붙인다 (앱 안에서는 CSS 를 비워 앱 테마를 따른다. M9 와 같다)
pub const CSS: &str = concat!(
    ".neo-meta { margin-top: 8px; font-size: 15px; opacity: .65; }\n",
    ".neo-list, .neo-choice { display: inline-block; text-align: left; }\n",
    ".neo-extra { margin-top: 12px; font-size: 16px; }\n",
    ".neo-choice-back[data-answer*=\",1,\"] .o1, .neo-choice-back[data-answer*=\",2,\"] .o2, .neo-choice-back[data-answer*=\",3,\"] .o3,\n",
    ".neo-choice-back[data-answer*=\",4,\"] .o4, .neo-choice-back[data-answer*=\",5,\"] .o5, .neo-choice-back[data-answer*=\",6,\"] .o6 { font-weight: bold; }\n",
);

fn same(names: &[String], want: &[&str]) -> bool {
    names.len() == want.len() && names.iter().zip(want).all(|(a, b)| a == b)
}

/// 목록 · 선택형 노트 유형인가 (이름 + 필드). Anki 가 이름 뒤에 붙인 꼬리("+" 등)는 받는다
pub(crate) fn is_special(nt: &str, names: &[String]) -> bool {
    (nt.starts_with(LIST) && same(names, &LIST_FIELDS)) || (nt.starts_with(CHOICE) && same(names, &CHOICE_FIELDS))
}

/// 이름은 우리 유형인데 필드가 달라 알아보지 못하는가 (Anki 에서 필드를 지우거나 이름을 바꿨다)
pub(crate) fn unrecognized(nt: &str, names: &[String]) -> bool {
    (nt.starts_with(LIST) || nt.starts_with(CHOICE)) && !is_special(nt, names)
}

/// 학습 화면이 직접 그리는 목록 · 선택형 카드. 질문 · 추가 설명은 HTML (화면이 살균한다), 항목 · 보기는 글자
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Special {
    List { question: String, items: Vec<String>, ordered: bool, extra: String },
    /// 보기: (필드 번호 1–6, 글자). 빈 보기는 뺀다. correct 도 필드 번호
    Choice { question: String, options: Vec<(u32, String)>, correct: Vec<u32>, shuffle: bool, extra: String },
}

fn plain(html: &str) -> String {
    render::strip_tags(html).trim().to_string()
}

/// "1,3" · "1, 3" · "1 3" -> [1, 3]
fn numbers(s: &str) -> Vec<u32> {
    let mut v: Vec<u32> = plain(s).split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

pub(crate) fn special(nt: &str, names: &[String], fields: &str) -> Option<Special> {
    if !is_special(nt, names) {
        return None;
    }
    let v: Vec<String> = serde_json::from_str(fields).ok()?;
    let f = |i: usize| v.get(i).cloned().unwrap_or_default();
    if nt.starts_with(LIST) {
        // 순서 없는 목록도 저장한 순서로 연다 (Q4)
        return Some(Special::List { question: f(0), items: quality::html_lines(&f(1)), ordered: plain(&f(2)) != UNORDERED, extra: f(3) });
    }
    let options: Vec<(u32, String)> =
        (1..=MAX_OPTIONS).filter_map(|i| Some(quality::html_lines(&f(i)).join(" ")).filter(|t| !t.is_empty()).map(|t| (i as u32, t))).collect();
    let correct = numbers(&f(7)).into_iter().filter(|n| options.iter().any(|o| o.0 == *n)).collect();
    Some(Special::Choice { question: f(0), options, correct, shuffle: plain(&f(8)) != NO_SHUFFLE, extra: f(9) })
}

pub(crate) fn card_special(con: &Connection, card_id: i64) -> Res<Option<Special>> {
    let (fields, nt, names): (String, String, String) = con
        .query_row(
            "SELECT n.fields, nt.name, nt.fields FROM card c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id WHERE c.id = ?1",
            [card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(err)?;
    Ok(special(&nt, &serde_json::from_str::<Vec<String>>(&names).unwrap_or_default(), &fields))
}

// ------------------------------------------------------------------ 평가 추천 (순수 함수)

/// Q3 목록: 기억한 비율로 J2 와 같은 규칙 (40% 미만 Again · 80% 미만 Hard · 그 위 Good, Easy 는 추천하지 않는다)
pub(crate) fn suggest_list(missed: usize, total: usize) -> Option<u8> {
    written::suggest(total.saturating_sub(missed), total)
}

/// Q7 선택형 (작성자 제안 · 사용자 확인 대기): 다 맞음 Good · 정답 일부만 Hard · 틀린 보기를 하나라도 고름 Again. 고르지 않으면 없다
pub(crate) fn suggest_choice(picked: &[u32], correct: &[u32]) -> Option<u8> {
    if picked.is_empty() {
        return None;
    }
    Some(if picked.iter().any(|p| !correct.contains(p)) {
        1
    } else if correct.iter().all(|c| picked.contains(c)) {
        3
    } else {
        2
    })
}

/// marks: 목록은 빠뜨린 항목 번호, 선택형은 고른 보기 번호 (1부터). 모르는 번호는 버린다
pub(crate) fn suggest_for(con: &Connection, card_id: i64, marks: &[u32]) -> Res<Option<u8>> {
    let mut marks = marks.to_vec();
    marks.sort_unstable();
    marks.dedup();
    Ok(match card_special(con, card_id)? {
        Some(Special::List { items, .. }) => suggest_list(marks.iter().filter(|m| (1..=items.len() as u32).contains(m)).count(), items.len()),
        Some(Special::Choice { options, correct, .. }) => {
            let picked: Vec<u32> = marks.into_iter().filter(|m| options.iter().any(|o| o.0 == *m)).collect();
            suggest_choice(&picked, &correct)
        }
        None => None,
    })
}

// ------------------------------------------------------------------ 명령: 학습 화면 (Q1 · Q2)

/// 빠뜨림을 고치거나 보기를 고를 때마다 (추천이 바로 바뀐다)
#[tauri::command]
pub fn recall_suggest(state: State<AppState>, card_id: i64, marks: Vec<u32>) -> Res<Option<u8>> {
    let con = state.db.lock().map_err(err)?;
    suggest_for(&con, card_id, &marks)
}

// ------------------------------------------------------------------ 지난번 빠뜨림 · 고른 답 (Q9 · Q20, 초안 Q30–Q33)

/// marks 가 가리키는 것 (목록: 항목 글자, 선택형: 보기 필드 번호 · 글자 + 정답). 노트를 고쳐 이것이 바뀌면 지난번 기록은 엉뚱한 칸을 가리킨다
fn shown(s: &Special) -> String {
    match s {
        Special::List { items, .. } => serde_json::to_string(items),
        Special::Choice { options, correct, .. } => serde_json::to_string(&(options, correct)),
    }
    .unwrap_or_default()
}

/// 평가가 남은 같은 트랜잭션에서만 부른다 (answer · cram_answer). 카드마다 마지막 하나, 이 기기에만 (동기화하지 않는다).
/// 되돌리기에 쓰게 만든 기록 id(log_id)와 바로 전 값(prev)을 같이 둔다. 선택형 marks 는 보기 필드 번호라 섞어도 보기를 따라간다
pub(crate) fn save_marks(con: &Connection, card_id: i64, log_id: i64, marks: &[u32], now: i64) -> Res<()> {
    let shown = card_special(con, card_id)?.as_ref().map(shown);
    con.execute(
        "INSERT INTO recall_mark (card_id, marks, marked_at, shown, log_id) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(card_id) DO UPDATE SET prev = json_object('marks', marks, 'marked_at', marked_at, 'shown', shown, 'log_id', log_id),
             marks = ?2, marked_at = ?3, shown = ?4, log_id = ?5",
        params![card_id, serde_json::to_string(marks).map_err(err)?, now, shown, log_id],
    )
    .map_err(err)?;
    Ok(())
}

/// 그 평가(log_id)를 되돌리면 지난번 기록도 그 전 값으로 (없었으면 지운다). undo_last · cram_undo.
/// ponytail: 전 값은 하나만 둔다. 같은 카드를 두 번 거슬러 되돌리면 지운다 (화면의 되돌리기는 한 번뿐이다)
pub(crate) fn undo_marks(con: &Connection, log_id: i64) -> Res<()> {
    con.execute(
        "UPDATE recall_mark SET marks = json_extract(prev, '$.marks'), marked_at = json_extract(prev, '$.marked_at'),
             shown = json_extract(prev, '$.shown'), log_id = json_extract(prev, '$.log_id'), prev = NULL
         WHERE log_id = ?1 AND prev IS NOT NULL",
        [log_id],
    )
    .map_err(err)?;
    con.execute("DELETE FROM recall_mark WHERE log_id = ?1", [log_id]).map_err(err)?;
    Ok(())
}

/// 학습 화면의 "지난번" (card_view). 노트를 고쳐 항목 · 보기 · 정답이 바뀌었으면 없다 (편집 · 동기화 · 가져오기 어느 길로 바뀌어도 읽을 때 본다)
pub(crate) fn last_marks(con: &Connection, card_id: i64, s: &Special) -> Res<Option<Vec<u32>>> {
    let row: Option<(String, Option<String>)> =
        con.query_row("SELECT marks, shown FROM recall_mark WHERE card_id = ?1", [card_id], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(err)?;
    Ok(row.filter(|(_, at)| at.as_deref() == Some(shown(s).as_str())).and_then(|(m, _)| serde_json::from_str(&m).ok()))
}

// ------------------------------------------------------------------ 명령: 카드 추가 (Q0)

#[derive(Deserialize)]
pub struct ListNew {
    pub(crate) deck_id: i64,
    pub(crate) question: String,
    pub(crate) items: Vec<String>,
    pub(crate) ordered: bool,
    pub(crate) extra: String,
    pub(crate) importance: Option<i64>,
}

#[derive(Deserialize)]
pub struct ChoiceNew {
    pub(crate) deck_id: i64,
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
    /// 정답인 보기 (options 의 0부터 번호)
    pub(crate) correct: Vec<usize>,
    pub(crate) shuffle: bool,
    pub(crate) extra: String,
    pub(crate) importance: Option<i64>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Q4: 항목이 2개 미만이면 저장하지 않는다 (10개 넘는 경고는 화면만)
pub(crate) fn list_fields(c: &ListNew) -> Res<Vec<String>> {
    if render::is_blank(&c.question) {
        return Err("질문을 입력해 주세요.".into());
    }
    let items: Vec<String> = c.items.iter().map(|i| i.trim()).filter(|i| !i.is_empty()).map(esc).collect();
    if items.len() < 2 {
        return Err("항목을 2개 이상 입력해 주세요.".into());
    }
    let order = if c.ordered { ORDERED } else { UNORDERED };
    Ok(vec![c.question.clone(), items.join("<br>"), order.into(), c.extra.clone()])
}

/// Q5: 보기 2–6개, 정답 1개 이상. 빈 보기는 빼고 앞으로 당긴다
pub(crate) fn choice_fields(c: &ChoiceNew) -> Res<Vec<String>> {
    if render::is_blank(&c.question) {
        return Err("질문을 입력해 주세요.".into());
    }
    let kept: Vec<(usize, String)> = c.options.iter().enumerate().filter(|(_, o)| !o.trim().is_empty()).map(|(i, o)| (i, esc(o.trim()))).collect();
    if kept.len() < 2 {
        return Err("보기를 2개 이상 입력해 주세요.".into());
    }
    if kept.len() > MAX_OPTIONS {
        return Err("보기는 6개까지예요.".into());
    }
    let correct: Vec<String> = kept.iter().enumerate().filter(|(_, (i, _))| c.correct.contains(i)).map(|(n, _)| (n + 1).to_string()).collect();
    if correct.is_empty() {
        return Err("정답을 하나 이상 골라 주세요.".into());
    }
    let mut f = vec![c.question.clone()];
    f.extend((0..MAX_OPTIONS).map(|i| kept.get(i).map(|k| k.1.clone()).unwrap_or_default()));
    f.extend([correct.join(","), (if c.shuffle { SHUFFLE } else { NO_SHUFFLE }).into(), c.extra.clone()]);
    Ok(f)
}

/// Q23 필드 저장의 안전망. 전용 편집기(Q24)를 거치지 않은 값도 정답 · 순서 · 섞기 칸이 앱이 읽을 수 있는 값인지 본다 (commands::save_note_fields)
pub(crate) fn check_fields(nt: &str, names: &[String], values: &[String]) -> Res<()> {
    if !is_special(nt, names) {
        return Ok(());
    }
    let v = |i: usize| plain(values.get(i).map_or("", String::as_str));
    if nt.starts_with(LIST) {
        return match v(2).as_str() {
            ORDERED | UNORDERED => Ok(()),
            _ => Err("순서 칸은 \"순서대로\" 또는 \"순서 없이\"로 써 주세요.".into()),
        };
    }
    let answer = v(7);
    let option = |t: &str| t.trim().parse::<usize>().is_ok_and(|n| (1..=MAX_OPTIONS).contains(&n) && values.get(n).is_some_and(|o| !render::is_blank(o)));
    if answer.is_empty() || !answer.split(',').all(option) {
        return Err("정답 칸은 내용이 있는 보기 번호로 써 주세요 (예: 2, 여럿이면 1,3).".into());
    }
    match v(8).as_str() {
        SHUFFLE | NO_SHUFFLE => Ok(()),
        _ => Err("섞기 칸은 \"섞기\" 또는 \"섞지 않음\"으로 써 주세요.".into()),
    }
}

/// 노트 하나 + 카드 한 장. 노트 id 를 돌려준다. tags · batch 는 JSON 가져오기 (E2)
pub(crate) fn insert(con: &Connection, nt: &str, deck_id: i64, fields: &[String], importance: Option<i64>, tags: &str, batch: Option<i64>) -> Res<i64> {
    let now = now_ms();
    let ntid = crate::db::native_type_id(con, if nt == LIST { "list" } else { "choice" }).map_err(err)?;
    con.execute(
        "INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, import_batch_id) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![ntid, serde_json::to_string(fields).map_err(err)?, tags, now, batch],
    )
    .map_err(err)?;
    let nid = con.last_insert_rowid();
    con.execute(
        "INSERT INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, CASE WHEN ?3 IS NULL THEN NULL ELSE 'user' END, ?4, ?4)",
        params![nid, deck_id, importance, now],
    )
    .map_err(err)?;
    Ok(nid)
}

#[tauri::command]
pub fn add_list_card(state: State<AppState>, card: ListNew) -> Res<i64> {
    let fields = list_fields(&card)?;
    let con = state.db.lock().map_err(err)?;
    insert(&con, LIST, card.deck_id, &fields, card.importance, "", None)
}

#[tauri::command]
pub fn add_choice_card(state: State<AppState>, card: ChoiceNew) -> Res<i64> {
    let fields = choice_fields(&card)?;
    let con = state.db.lock().map_err(err)?;
    insert(&con, CHOICE, card.deck_id, &fields, card.importance, "", None)
}

// ------------------------------------------------------------------ 명령: 전용 편집기 (Q24)

/// 편집기에 보인 글자 (special 과 같은 읽기)
fn view(html: &str) -> String {
    quality::html_lines(html).join(" ")
}

/// Q24: 고치지 않은 항목 · 보기 · 칸은 원래 HTML 을 그대로 둔다 (M20 과 같은 생각). 새 값의 글자가 원래 칸의 글자와 같으면 원래 것.
/// new 는 list_fields · choice_fields 값. 질문 · 추가 설명은 편집기 HTML 이라 건드리지 않는다
/// ponytail: 항목은 원래 칸을 <br> 로 나눈 조각과 맞춘다. 다른 줄 나눔(div 등)이면 다 같을 때만 지키고, 서식이 <br> 을 넘는 조각은 순서를 바꾸면 태그가 어긋난다
fn keep_untouched(nt: &str, old: &[String], new: &mut [String]) {
    let keep = |new: &mut String, old: &str| {
        if !new.is_empty() && view(old) == view(new) {
            *new = old.to_string();
        }
    };
    let at = |i: usize| old.get(i).map_or("", String::as_str);
    if nt == LIST {
        if quality::html_lines(at(1)) == quality::html_lines(&new[1]) {
            new[1] = at(1).to_string();
        } else {
            let old_items = at(1).replace("<br />", "<br>").replace("<br/>", "<br>");
            let pieces: Vec<&str> = old_items.split("<br>").collect();
            new[1] = new[1].split("<br>").map(|t| *pieces.iter().find(|p| view(p) == view(t)).unwrap_or(&t)).collect::<Vec<_>>().join("<br>");
        }
        keep(&mut new[2], at(2));
    } else {
        for opt in &mut new[1..=MAX_OPTIONS] {
            if let Some(o) = (1..=MAX_OPTIONS).map(at).find(|o| !opt.is_empty() && view(o) == view(opt)) {
                *opt = o.to_string();
            }
        }
        keep(&mut new[7], at(7));
        keep(&mut new[8], at(8));
    }
}

fn note_fields(state: &State<AppState>, note_id: i64) -> Res<Vec<String>> {
    let con = state.db.lock().map_err(err)?;
    let fields: String = con.query_row("SELECT fields FROM note WHERE id = ?1", [note_id], |r| r.get(0)).map_err(err)?;
    serde_json::from_str(&fields).map_err(err)
}

/// 추가와 같은 검사 · 같은 값으로 다시 만들고(고치지 않은 것은 원래 HTML), 저장은 일반 편집과 같은 길 (update_note → save_note_fields)
#[tauri::command]
pub fn update_list_note(state: State<AppState>, note_id: i64, question: String, items: Vec<String>, ordered: bool, extra: String) -> Res<()> {
    let mut fields = list_fields(&ListNew { deck_id: 0, question, items, ordered, extra, importance: None })?;
    keep_untouched(LIST, &note_fields(&state, note_id)?, &mut fields);
    // 자리 순서 그대로 (보이는 순서를 바꾼 가져온 목록 · 선택형 유형도 자리로 읽는다)
    let slots = (0..fields.len()).collect();
    crate::commands::update_note(state, note_id, fields, Some(slots))
}

#[tauri::command]
pub fn update_choice_note(
    state: State<AppState>,
    note_id: i64,
    question: String,
    options: Vec<String>,
    correct: Vec<usize>,
    shuffle: bool,
    extra: String,
) -> Res<()> {
    let mut fields = choice_fields(&ChoiceNew { deck_id: 0, question, options, correct, shuffle, extra, importance: None })?;
    keep_untouched(CHOICE, &note_fields(&state, note_id)?, &mut fields);
    // 자리 순서 그대로 (보이는 순서를 바꾼 가져온 목록 · 선택형 유형도 자리로 읽는다)
    let slots = (0..fields.len()).collect();
    crate::commands::update_note(state, note_id, fields, Some(slots))
}

// ------------------------------------------------------------------ Q8 오답 보기 제안 (ChatGPT, 초안 Q36–Q41)

/// 제안 하나의 글자 상한. 넘는 제안은 버린다 (보기는 짧은 글이다)
const MAX_DISTRACTOR_CHARS: usize = 80;
/// 보내는 질문 · 보기 글자 상한 (R14 처럼 잘라 보낸다)
const SEND_QUESTION_CHARS: usize = 600;
const SEND_OPTION_CHARS: usize = 200;

const DISTRACTOR_INSTRUCTIONS: &str = "선택형 암기 카드의 오답 보기를 만드는 일을 도와요. 사용자가 보낸 질문 · 정답 · 이미 쓴 오답을 보고, \
\"만들 오답 수\"만큼 새 오답 보기를 만드세요.
- 정답과 뜻이 같거나 정답이 될 수 있는 보기는 쓰지 마세요. 정답이 여러 개면 모두 정답이에요.
- 이미 쓴 보기와 겹치지 않게, 이미 쓴 보기와 같은 문체 · 길이로, 질문과 같은 언어로 쓰세요.
- 보기 글자만 쓰세요. 번호나 설명은 붙이지 마세요.
- 답은 {\"options\": [보기, …]} 모양의 JSON 하나만.";

/// 편집기 보기 칸은 글자다. [sound:] 만 빼고 띄어쓰기를 하나로 (AU6)
fn option_text(t: &str) -> String {
    render::strip_sound(t).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 같은 보기인가: 공백을 모두 빼고 대소문자를 무시한다 ("데이터 링크" = "데이터링크", "syn-ack" = "SYN-ACK")
fn option_key(t: &str) -> String {
    t.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// 보내는 글자 전부. 질문은 편집기 HTML → 화면 글자 (태그 · 그림 · [sound:] 를 뺀다), 정답 · 오답은 보기 칸 글자.
/// 덱 이름 · 복습 기록 · 다른 카드는 받지도 않는다 (편집 중인 글자만 받는 명령이다)
fn distractor_input(question: &str, correct: &[String], wrong: &[String], count: usize) -> String {
    let mut s = format!("질문: {}\n", rag::clip(&render::strip_tags(&render::strip_sound(question)), SEND_QUESTION_CHARS));
    for c in correct {
        s.push_str(&format!("정답: {}\n", rag::clip(c, SEND_OPTION_CHARS)));
    }
    if !wrong.is_empty() {
        s.push_str("이미 쓴 오답:\n");
        for w in wrong {
            s.push_str(&format!("- {}\n", rag::clip(w, SEND_OPTION_CHARS)));
        }
    }
    s.push_str(&format!("만들 오답 수: {count}\n"));
    s
}

fn distractor_schema() -> serde_json::Value {
    // 개수 제한(maxItems)은 strict 스키마가 받지 않을 수 있어 parse_distractors 가 자른다
    ai::object(serde_json::json!({ "options": { "type": "array", "items": { "type": "string" } } }))
}

/// 모델 출력 → 제안. 빈 글 · 너무 긴 글 · 정답이나 보낸 보기와 같은 글 · 제안끼리 겹치는 글은 하나씩 버리고, count 개까지만.
/// 남는 것이 없으면 BadOutput ("알맞은 오답을 만들지 못했어요")
fn parse_distractors(text: &str, taken: &[String], count: usize) -> Result<Vec<String>, AiError> {
    let v = ai::parse_json(text)?;
    let list = v["options"].as_array().ok_or_else(|| AiError::BadOutput("options 가 없어요".into()))?;
    let mut seen: Vec<String> = taken.iter().map(|t| option_key(t)).collect();
    let mut out = vec![];
    for o in list {
        let t = option_text(o.as_str().unwrap_or(""));
        let k = option_key(&t);
        if k.is_empty() || t.chars().count() > MAX_DISTRACTOR_CHARS || seen.contains(&k) {
            continue;
        }
        seen.push(k);
        out.push(t);
        if out.len() == count {
            break;
        }
    }
    if out.is_empty() {
        return Err(AiError::BadOutput("쓸 수 있는 오답 보기가 없어요".into()));
    }
    Ok(out)
}

/// 가짜 응답 (NEOANKI_AI_FAKE). 검증까지 보이게 정답(대소문자 · 띄어쓰기만 다름) · 보낸 보기 · 빈 글 · 너무 긴 글을 앞에 섞는다.
/// 남는 것은 TCP 예시(Figma 237:16413)에 맞춘 고정 목록이라 다른 질문에는 엉뚱하다 (가짜다)
fn fake_distractors(correct: &[String], wrong: &[String]) -> String {
    let mut v: Vec<String> = correct.iter().take(1).map(|c| format!(" {} ", c.to_lowercase())).collect();
    v.extend(wrong.iter().take(1).cloned());
    v.extend([String::new(), "가".repeat(MAX_DISTRACTOR_CHARS + 1)]);
    v.extend(["ACK", "FIN-ACK", "RST", "URG", "PSH", "FIN"].map(String::from));
    serde_json::json!({ "options": v }).to_string()
}

/// Q8 [오답 보기 N개 제안 받기]: 편집 중인 질문(HTML) · 정답 · 보낼 오답 글자를 보내 오답 보기 count 개까지 받는다. 아무것도 저장하지 않는다
/// (넣기는 화면이 편집기 보기 칸에, 저장은 늘 하던 추가 · 편집 길로). 카드 추가에는 아직 카드가 없어 card_id 대신 글자를 받는다
#[tauri::command]
pub async fn ai_distractors(app: AppHandle, question: String, correct: Vec<String>, options: Vec<String>, count: usize) -> Result<Vec<String>, AiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let clean = |v: &[String]| v.iter().map(|t| option_text(t)).filter(|t| !t.is_empty()).collect::<Vec<_>>();
        let (correct, wrong) = (clean(&correct), clean(&options));
        if render::is_blank(&question) {
            return Err(AiError::Other("질문을 먼저 써 주세요.".into()));
        }
        if correct.is_empty() {
            return Err(AiError::Other("정답에 체크한 뒤 받을 수 있어요.".into()));
        }
        // 화면이 남은 자리를 센다 (4지선다까지, Q36). 여기서는 보기 6개를 넘지 않게만
        let count = count.min(MAX_OPTIONS - 1);
        if count == 0 {
            return Err(AiError::Other("보기가 6개라 더 받을 수 없어요.".into()));
        }
        let input = distractor_input(&question, &correct, &wrong, count);
        let schema = distractor_schema();
        let text = ai::generate(&app, &|m, f| ai::request_body(m, DISTRACTOR_INSTRUCTIONS, &input, f.then_some(("distractors", &schema))), || fake_distractors(&correct, &wrong))?;
        parse_distractors(&text, &[correct, wrong].concat(), count)
    })
    .await
    .map_err(|e| AiError::Other(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_names_match_their_json() {
        assert_eq!(serde_json::to_string(&LIST_FIELDS).unwrap(), LIST_FIELDS_JSON);
        assert_eq!(serde_json::to_string(&CHOICE_FIELDS).unwrap(), CHOICE_FIELDS_JSON);
    }

    #[test]
    fn list_suggestion_follows_j2() {
        // 7개 중 5개 = 71% → Hard (Figma Q1-b). 6개 = 85% → Good. 2개 = 28% → Again
        assert_eq!([0, 1, 2, 3, 5, 7].map(|m| suggest_list(m, 7)), [3, 3, 2, 2, 1, 1].map(Some));
        assert_eq!(suggest_list(0, 0), None);
    }

    #[test]
    fn choice_suggestion_q7() {
        assert_eq!(suggest_choice(&[2], &[2]), Some(3), "다 맞음");
        assert_eq!(suggest_choice(&[3], &[2]), Some(1), "틀린 보기");
        assert_eq!(suggest_choice(&[1], &[1, 3]), Some(2), "정답 일부만");
        assert_eq!(suggest_choice(&[1, 3], &[1, 3]), Some(3));
        assert_eq!(suggest_choice(&[1, 2], &[1, 3]), Some(1), "정답을 골라도 틀린 보기가 있으면 Again");
        assert_eq!(suggest_choice(&[], &[2]), None, "고르지 않고 넘기면 추천 없음");
    }

    #[test]
    fn edit_check_q23() {
        let list = LIST_FIELDS.map(String::from);
        let choice = CHOICE_FIELDS.map(String::from);
        let l = |order: &str| ["q", "a<br>b", order, ""].map(String::from);
        assert!(check_fields(LIST, &list, &l("순서 없이")).is_ok());
        assert!(check_fields(LIST, &list, &l("<b>순서대로</b>")).is_ok(), "서식은 본다");
        assert!(check_fields(LIST, &list, &l("아무렇게나")).is_err());
        let c = |answer: &str, shuffle: &str| ["q", "SYN", "SYN-ACK", "ACK", "", "", "", answer, shuffle, ""].map(String::from);
        assert!(check_fields(CHOICE, &choice, &c("2", "섞기")).is_ok());
        assert!(check_fields(CHOICE, &choice, &c("1, 3", "섞지 않음")).is_ok());
        for bad in ["", "4", "7", "0", "둘", "2,x", "2,,3"] {
            assert!(check_fields(CHOICE, &choice, &c(bad, "섞기")).is_err(), "{bad:?}: 범위 밖 · 빈 보기 · 숫자 아님");
        }
        assert!(check_fields(CHOICE, &choice, &c("2", "네")).is_err());
        assert!(check_fields("Basic", &["Front".into(), "Back".into()], &["4".into(), "x".into()]).is_ok(), "다른 유형은 보지 않는다");
    }

    #[test]
    fn editor_round_trip_q24() {
        // 전용 편집기: 필드 → special(채우기) → 추가와 같은 값(저장). 앱이 만든 노트는 그대로 돌아온다
        let names = LIST_FIELDS.map(String::from);
        let new = |items: Vec<String>, ordered, question, extra| ListNew { deck_id: 0, question, items, ordered, extra, importance: None };
        let f = list_fields(&new(vec!["a<b".into(), " c & d ".into()], false, "Q".into(), "<b>x</b>".into())).unwrap();
        let Some(Special::List { question, items, ordered, extra }) = special(LIST, &names, &serde_json::to_string(&f).unwrap()) else { panic!() };
        assert_eq!(list_fields(&new(items, ordered, question, extra)).unwrap(), f);

        // 선택형: 빈 보기(보기2)는 당기고 정답 번호도 따라간다. 화면은 보기 필드 번호 → 0부터 번호로 바꾼다 (Study.tsx EditNote)
        let names = CHOICE_FIELDS.map(String::from);
        let f = ["Q", "SYN", "", "ACK &amp; FIN", "", "", "", "3", "섞지 않음", ""].map(String::from);
        let Some(Special::Choice { question, options, correct, shuffle, extra }) = special(CHOICE, &names, &serde_json::to_string(&f).unwrap()) else { panic!() };
        let c = ChoiceNew {
            deck_id: 0,
            question,
            correct: options.iter().enumerate().filter(|(_, o)| correct.contains(&o.0)).map(|(i, _)| i).collect(),
            options: options.into_iter().map(|o| o.1).collect(),
            shuffle,
            extra,
            importance: None,
        };
        let g = choice_fields(&c).unwrap();
        assert_eq!((&g[1..4], g[7].as_str(), g[8].as_str()), (&["SYN".to_string(), "ACK &amp; FIN".into(), String::new()][..], "2", "섞지 않음"));
        assert!(check_fields(CHOICE, &names, &g).is_ok());
    }

    #[test]
    fn editor_keeps_untouched_html_q24() {
        let json = |f: &[String]| serde_json::to_string(f).unwrap();
        // 선택형: 보기3 만 고치면 서식 있는 보기1 · 섞기 칸은 바이트 그대로
        let names = CHOICE_FIELDS.map(String::from);
        let old = ["Q", "<b>SYN</b>", "SYN-ACK", "ACK", "", "", "", "2", "<i>섞기</i>", ""].map(String::from);
        let Some(Special::Choice { question, options, shuffle, extra, .. }) = special(CHOICE, &names, &json(&old)) else { panic!() };
        let mut texts: Vec<String> = options.into_iter().map(|o| o.1).collect();
        texts[2] = "ACK & FIN".into();
        let mut new = choice_fields(&ChoiceNew { deck_id: 0, question, options: texts, correct: vec![1], shuffle, extra, importance: None }).unwrap();
        keep_untouched(CHOICE, &old, &mut new);
        assert_eq!(new, ["Q", "<b>SYN</b>", "SYN-ACK", "ACK &amp; FIN", "", "", "", "2", "<i>섞기</i>", ""].map(String::from));

        // 목록: 고친 항목만 글자로, 나머지 조각 · 순서 칸은 원래 HTML
        let names = LIST_FIELDS.map(String::from);
        let old = ["Q", "<i>응용</i> 계층<br/>표현 계층<br>세션", "<b>순서대로</b>", ""].map(String::from);
        let Some(Special::List { question, mut items, ordered, extra }) = special(LIST, &names, &json(&old)) else { panic!() };
        items[2] = "세션 계층".into();
        let mut new = list_fields(&ListNew { deck_id: 0, question, items, ordered, extra, importance: None }).unwrap();
        keep_untouched(LIST, &old, &mut new);
        assert_eq!((new[1].as_str(), new[2].as_str()), ("<i>응용</i> 계층<br>표현 계층<br>세션 계층", "<b>순서대로</b>"));
        // 하나도 안 고쳤으면 항목 칸 전체가 그대로 (<br/> · div 줄 나눔도)
        let old = ["Q", "<div>a</div><div><b>b</b></div>", "순서 없이", ""].map(String::from);
        let mut new = list_fields(&ListNew { deck_id: 0, question: "Q".into(), items: vec!["a".into(), "b".into()], ordered: false, extra: String::new(), importance: None }).unwrap();
        keep_untouched(LIST, &old, &mut new);
        assert_eq!(new, old);
    }

    /// 덱 하나 + 노트 하나 (카드 id)
    fn one_card(con: &Connection, nt: &str, fields: &[&str]) -> i64 {
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES (?1, 0, 0)", [nt]).unwrap();
        let fields: Vec<String> = fields.iter().map(|s| s.to_string()).collect();
        let nid = insert(con, nt, con.last_insert_rowid(), &fields, None, "", None).unwrap();
        con.query_row("SELECT id FROM card WHERE note_id = ?1", [nid], |r| r.get(0)).unwrap()
    }

    fn last(con: &Connection, card: i64) -> Option<Vec<u32>> {
        last_marks(con, card, &card_special(con, card).unwrap().unwrap()).unwrap()
    }

    fn set_fields(con: &Connection, card: i64, fields: &[&str]) {
        con.execute("UPDATE note SET fields = ?2 WHERE id = (SELECT note_id FROM card WHERE id = ?1)", params![card, serde_json::to_string(fields).unwrap()]).unwrap();
    }

    #[test]
    fn last_marks_dropped_when_items_or_options_change() {
        // Q30: 노트를 고쳐 항목 · 보기 · 정답이 바뀌면 지난번 기록은 엉뚱한 칸을 가리킨다 → 읽지 않는다 (어느 길로 고쳐도 읽을 때 본다)
        let con = crate::db::open_in_memory().unwrap();
        let list = one_card(&con, LIST, &["OSI?", "응용<br>표현<br>세션", "순서대로", ""]);
        save_marks(&con, list, 1, &[2], 0).unwrap();
        assert_eq!(last(&con, list), Some(vec![2]));
        set_fields(&con, list, &["OSI 7계층은?", "<b>응용</b><br>표현<br>세션", "순서 없이", "설명"]);
        assert_eq!(last(&con, list), Some(vec![2]), "질문 · 서식 · 순서 · 추가 설명만 고치면 그대로");
        set_fields(&con, list, &["OSI 7계층은?", "응용<br>세션<br>표현", "순서대로", ""]);
        assert_eq!(last(&con, list), None, "항목 순서가 바뀌면 2번은 다른 항목이다");

        let choice = one_card(&con, CHOICE, &["Q", "SYN", "SYN-ACK", "ACK", "", "", "", "2", "섞기", ""]);
        save_marks(&con, choice, 2, &[3], 0).unwrap();
        set_fields(&con, choice, &["Q?", "SYN", "SYN-ACK", "ACK", "", "", "", "2", "섞지 않음", "해설"]);
        assert_eq!(last(&con, choice), Some(vec![3]));
        set_fields(&con, choice, &["Q?", "SYN", "SYN-ACK", "ACK", "", "", "", "3", "섞기", ""]);
        assert_eq!(last(&con, choice), None, "정답을 고치면 (Figma Q2-c 노트)");
        set_fields(&con, choice, &["Q", "SYN-ACK", "ACK", "", "", "", "", "1", "섞기", ""]);
        save_marks(&con, choice, 3, &[2], 0).unwrap();
        set_fields(&con, choice, &["Q", "SYN", "SYN-ACK", "ACK", "", "", "", "2", "섞기", ""]);
        assert_eq!(last(&con, choice), None, "보기를 더해 필드 번호가 밀리면 (2번이 ACK 에서 SYN-ACK 로)");

        // 이 기능 전에 남은 기록(그때의 항목이 없다)은 쓰지 않는다
        save_marks(&con, list, 4, &[1], 0).unwrap();
        assert_eq!(last(&con, list), Some(vec![1]));
        con.execute("UPDATE recall_mark SET shown = NULL WHERE card_id = ?1", [list]).unwrap();
        assert_eq!(last(&con, list), None);
    }

    #[test]
    fn last_pick_follows_the_option_not_the_position() {
        // Q30: 선택형 marks 는 보기 필드 번호다. 볼 때마다 섞여도(optionOrder, 화면) 지난번 고른 보기 글자는 같다
        let con = crate::db::open_in_memory().unwrap();
        let card = one_card(&con, CHOICE, &["Q", "SYN", "SYN-ACK", "ACK", "FIN", "", "", "2", "섞기", ""]);
        let Some(Special::Choice { options, .. }) = card_special(&con, card).unwrap() else { panic!() };
        let text = |num: u32| options.iter().find(|o| o.0 == num).unwrap().1.clone();
        let first_order = [4, 3, 1, 2]; // 화면 ② 를 골랐다
        save_marks(&con, card, 1, &[first_order[1]], 0).unwrap();
        let next_order = [2, 1, 4, 3];
        let last = last(&con, card).unwrap();
        let at = next_order.iter().position(|n| last.contains(n)).unwrap();
        assert_eq!((text(last[0]).as_str(), at), ("ACK", 3), "다음엔 ④ 자리에 있어도 ACK 에 붙는다");
        assert_eq!(suggest_for(&con, card, &last).unwrap(), Some(1), "지난번엔 틀렸다 (화면은 이때만 보인다)");
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn distractor_input_is_plain_editor_text_only() {
        // Q8: 질문은 편집기 HTML → 화면 글자. 그림 · [sound:] · 태그는 빠지고, 받은 글자 말고는 아무것도 붙지 않는다
        let q = "<b>TCP 3-way handshake</b>에서 두 번째로<br>오가는 것은? <img src=\"secret.png\"> [sound:voice.mp3]";
        let input = distractor_input(q, &strings(&["SYN-ACK"]), &strings(&["SYN"]), 2);
        assert_eq!(input, "질문: TCP 3-way handshake에서 두 번째로 오가는 것은?\n정답: SYN-ACK\n이미 쓴 오답:\n- SYN\n만들 오답 수: 2\n");
        for leak in ["<", "secret.png", "sound", "voice"] {
            assert!(!input.contains(leak), "{leak}");
        }
        // 정답 여러 개 · 이미 쓴 오답 없음 (✕ 로 모두 뺐다)
        let input = distractor_input("Q", &strings(&["가", "나"]), &[], 3);
        assert_eq!(input, "질문: Q\n정답: 가\n정답: 나\n만들 오답 수: 3\n");
        // 긴 질문은 600자에서 자른다 (R14 와 같다)
        let input = distractor_input(&"가".repeat(700), &strings(&["a"]), &[], 1);
        assert!(input.lines().next().unwrap().ends_with("…(잘림)") && input.lines().next().unwrap().chars().count() < 620);
        assert_eq!(option_text(" 공유된   가치 [sound:a.mp3] "), "공유된 가치", "보기 칸 글자: 소리만 빼고 띄어쓰기 하나로");
    }

    #[test]
    fn distractor_schema_is_strict_options_list() {
        let schema = distractor_schema();
        assert_eq!(schema["required"], serde_json::json!(["options"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["options"]["items"]["type"], "string");
        let body = ai::request_body("m", DISTRACTOR_INSTRUCTIONS, "x", Some(("distractors", &schema)));
        assert_eq!((body["store"].as_bool(), body["text"]["format"]["type"].as_str(), body["text"]["format"]["strict"].as_bool()), (Some(false), Some("json_schema"), Some(true)));
    }

    #[test]
    fn distractors_are_validated() {
        let taken = strings(&["SYN-ACK", "SYN", "데이터 링크 계층"]);
        let ok = |s: &str, n: usize| parse_distractors(s, &taken, n).unwrap();
        assert_eq!(ok(r#"{"options":["ACK","FIN-ACK"]}"#, 2), ["ACK", "FIN-ACK"]);
        assert_eq!(ok("```json\n{\"options\":[\"ACK\"]}\n```", 2), ["ACK"], "코드 블록 · 요청보다 적게");
        // 정답 · 보낸 보기와 같음 (공백 · 대소문자 무시), 빈 글, 너무 긴 글, 제안끼리 겹침은 하나씩 버린다
        let long = "가".repeat(MAX_DISTRACTOR_CHARS + 1);
        let s = serde_json::json!({ "options": [" syn-ack ", "Syn", "데이터링크 계층", "", "  ", long, "ACK", "ack", " FIN   ACK "] }).to_string();
        assert_eq!(ok(&s, 3), ["ACK", "FIN ACK"]);
        assert_eq!(ok(&format!(r#"{{"options":["{}"]}}"#, "가".repeat(MAX_DISTRACTOR_CHARS)), 1).len(), 1, "상한까지는 받는다");
        // 요청한 수 이하로만
        assert_eq!(ok(r#"{"options":["A","B","C","D"]}"#, 2), ["A", "B"]);
        let bad = |s: &str| matches!(parse_distractors(s, &taken, 2), Err(AiError::BadOutput(_)));
        assert!(bad(r#"{"options":["SYN-ACK","syn"]}"#), "남는 것이 없으면 BadOutput");
        assert!(bad(r#"{"options":[]}"#));
        assert!(bad(r#"{"cards":[]}"#), "options 가 없다");
        assert!(bad("오답은 ACK 예요"), "JSON 이 아니다");
        assert!(bad(r#"{"options":[1,2]}"#), "글자가 아니다");
    }

    #[test]
    fn fake_distractors_pass_through_the_same_checks() {
        // 가짜 모드: 정답 · 보낸 보기 · 빈 글 · 긴 글은 검증이 버리고 Figma 예시처럼 ACK · FIN-ACK 가 남는다
        let st = ai::AiState::new();
        let (correct, wrong) = (strings(&["SYN-ACK"]), strings(&["SYN"]));
        let text = ai::fake_text(&st, 0, "ok", || fake_distractors(&correct, &wrong)).unwrap();
        assert_eq!(parse_distractors(&text, &[correct.clone(), wrong.clone()].concat(), 2).unwrap(), ["ACK", "FIN-ACK"]);
        let text = ai::fake_text(&st, 0, "slow", || fake_distractors(&correct, &[])).unwrap();
        assert_eq!(parse_distractors(&text, &correct, 3).unwrap(), ["ACK", "FIN-ACK", "RST"], "slow 도 결과는 ok 와 같다 (기다림만 길다)");
        let text = ai::fake_text(&st, 0, "malformed", || unreachable!()).unwrap();
        assert!(matches!(parse_distractors(&text, &correct, 2), Err(AiError::BadOutput(_))));
        for (mode, e) in [("limit", AiError::Limit), ("usage_limit", AiError::Limit), ("expired", AiError::Expired), ("unavailable", AiError::Unavailable)] {
            assert_eq!(ai::fake_text(&st, 0, mode, String::new), Err(e), "{mode}");
        }
        assert!(matches!(ai::fake_text(&st, 0, "offline", String::new), Err(AiError::Offline(_))));
    }

    #[test]
    fn add_validates_like_q4_q5() {
        let l = |items: &[&str]| ListNew {
            deck_id: 1,
            question: "OSI 7계층은?".into(),
            items: items.iter().map(|s| s.to_string()).collect(),
            ordered: true,
            extra: String::new(),
            importance: None,
        };
        assert!(list_fields(&l(&["하나", "  "])).is_err(), "항목 2개 미만");
        assert_eq!(list_fields(&l(&["a<b", " 표현 계층 "])).unwrap()[1], "a&lt;b<br>표현 계층");
        let c = |options: &[&str], correct: &[usize]| ChoiceNew {
            deck_id: 1,
            question: "Q".into(),
            options: options.iter().map(|s| s.to_string()).collect(),
            correct: correct.to_vec(),
            shuffle: false,
            extra: String::new(),
            importance: None,
        };
        assert!(choice_fields(&c(&["SYN"], &[0])).is_err(), "보기 1개");
        assert!(choice_fields(&c(&["SYN", "ACK"], &[])).is_err(), "정답 없음");
        assert!(choice_fields(&c(&["1", "2", "3", "4", "5", "6", "7"], &[0])).is_err(), "보기 7개");
        // 빈 보기는 빼고 당긴다. 정답 번호도 따라간다
        let f = choice_fields(&c(&["SYN", "", "SYN-ACK", "ACK"], &[2])).unwrap();
        assert_eq!((&f[1..4], f[7].as_str(), f[8].as_str()), (&["SYN".to_string(), "SYN-ACK".into(), "ACK".into()][..], "2", "섞지 않음"));
    }
}
