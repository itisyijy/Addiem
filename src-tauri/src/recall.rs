//! 8.1 목록 회상형 · 선택형 (Figma Row Q · DECISIONS Q). 앱 전용 노트 유형 둘, 노트 1개 = 카드 1장 (Q1).
//!
//! 필드 (Q10): "Neo-Anki 목록 회상" = 질문 · 항목 · 순서 · 추가 설명, "Neo-Anki 선택형" = 질문 · 보기1–보기6 · 정답 · 섞기 · 추가 설명.
//!   - 항목: 한 줄에 하나 (<br> 로 나눈다). 순서: "순서대로" | "순서 없이"
//!   - 정답: 보기 번호 ("2", 여럿이면 "1,3"). 섞기: "섞기" | "섞지 않음"
//!
//! 템플릿은 스크립트 없이 필드만 쓴다 (db.rs NATIVE_TYPES). Anki 에서는 한 번에 다 보이는 기본 카드처럼 읽히고,
//! 정답 보기 굵게는 뒷면의 data-answer 속성 선택자(CSS)로 한다. 앱 학습 화면은 템플릿 대신 필드를 읽어 직접 그린다.
//! 다시 가져오면 유형 이름 + 필드로 알아본다 (M10 과 같은 방식). 이름만 같고 필드가 다르면 일반 노트로 (가져오기 결과에 한 줄).
//! 평가는 FSRS 4단계 그대로이고 앱은 추천만 한다 (Q3 · Q7). 항목별 빠뜨림 · 고른 보기는 이 기기에만 남긴다 (recall_mark, Q9)

use crate::commands::AppState;
use crate::db::now_ms;
use crate::{quality, render, written};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;

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

/// Q9 평가할 때. 카드마다 마지막 하나만, 이 기기에만 (recall_mark 는 동기화하지 않는다). "지난번 빠뜨림" 표시는 다음 버전
#[tauri::command]
pub fn save_recall_marks(state: State<AppState>, card_id: i64, marks: Vec<u32>) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "INSERT INTO recall_mark (card_id, marks, marked_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(card_id) DO UPDATE SET marks = ?2, marked_at = ?3",
        params![card_id, serde_json::to_string(&marks).map_err(err)?, now_ms()],
    )
    .map_err(err)?;
    Ok(())
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
        return Err("질문을 입력해주세요.".into());
    }
    let items: Vec<String> = c.items.iter().map(|i| i.trim()).filter(|i| !i.is_empty()).map(esc).collect();
    if items.len() < 2 {
        return Err("항목을 2개 이상 입력해주세요.".into());
    }
    let order = if c.ordered { ORDERED } else { UNORDERED };
    Ok(vec![c.question.clone(), items.join("<br>"), order.into(), c.extra.clone()])
}

/// Q5: 보기 2–6개, 정답 1개 이상. 빈 보기는 빼고 앞으로 당긴다
pub(crate) fn choice_fields(c: &ChoiceNew) -> Res<Vec<String>> {
    if render::is_blank(&c.question) {
        return Err("질문을 입력해주세요.".into());
    }
    let kept: Vec<(usize, String)> = c.options.iter().enumerate().filter(|(_, o)| !o.trim().is_empty()).map(|(i, o)| (i, esc(o.trim()))).collect();
    if kept.len() < 2 {
        return Err("보기를 2개 이상 입력해주세요.".into());
    }
    if kept.len() > MAX_OPTIONS {
        return Err("보기는 6개까지예요.".into());
    }
    let correct: Vec<String> = kept.iter().enumerate().filter(|(_, (i, _))| c.correct.contains(i)).map(|(n, _)| (n + 1).to_string()).collect();
    if correct.is_empty() {
        return Err("정답을 하나 이상 골라주세요.".into());
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

/// 노트 하나 + 카드 한 장. 노트 id 를 돌려준다
pub(crate) fn insert(con: &Connection, nt: &str, deck_id: i64, fields: &[String], importance: Option<i64>) -> Res<i64> {
    let now = now_ms();
    let ntid: i64 = con
        .query_row("SELECT id FROM note_type WHERE name = ?1 AND anki_model_id IS NULL ORDER BY id LIMIT 1", [nt], |r| r.get(0))
        .map_err(err)?;
    con.execute(
        "INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![ntid, serde_json::to_string(fields).map_err(err)?, now],
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
    insert(&con, LIST, card.deck_id, &fields, card.importance)
}

#[tauri::command]
pub fn add_choice_card(state: State<AppState>, card: ChoiceNew) -> Res<i64> {
    let fields = choice_fields(&card)?;
    let con = state.db.lock().map_err(err)?;
    insert(&con, CHOICE, card.deck_id, &fields, card.importance)
}

// ------------------------------------------------------------------ 명령: 전용 편집기 (Q24)

/// 추가와 같은 검사 · 같은 값으로 다시 만들고, 저장은 일반 편집과 같은 길 (update_note → save_note_fields)
/// ponytail: 항목 · 보기는 글자로 다시 쓴다 (Anki 에서 넣은 서식은 빠진다). 지켜야 하면 고치지 않은 칸은 원래 HTML 을 둔다
#[tauri::command]
pub fn update_list_note(state: State<AppState>, note_id: i64, question: String, items: Vec<String>, ordered: bool, extra: String) -> Res<()> {
    let fields = list_fields(&ListNew { deck_id: 0, question, items, ordered, extra, importance: None })?;
    crate::commands::update_note(state, note_id, fields)
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
    let fields = choice_fields(&ChoiceNew { deck_id: 0, question, options, correct, shuffle, extra, importance: None })?;
    crate::commands::update_note(state, note_id, fields)
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
