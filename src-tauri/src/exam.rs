//! 8.4 기출 이력. "이 내용이 이 시험에 나왔다"는 사실만 적는다: 시험 이름이나 날짜, 횟수. 점수·확률은 만들지 않는다 (MVP 9장 Out).
//! 노트(카드 내용)와 자료 구간에 단다. 노트에 다는 것은 태그가 노트에 있고, 앞뒤 카드처럼 같은 내용의 카드가 함께 가기 때문이다.
//! note · source_segment 의 exam_hits(횟수) · exam_names(이름 JSON 배열). 0 이면 기출 아님(지운 것), NULL 이면 아직 정하지 않음
//! (노트는 태그에서 채운다).
//! 쓰는 곳은 두 군데이고 둘 다 한 줄로 설명된다:
//!   - 큐: 같은 중요도 안에서 기출 카드를 먼저 (queue.rs)
//!   - 중요도 제안(N3 · B4): 하위 덱의 기출 비율이 덱 안 상위 25% 이고 기출 카드가 5장 이상이면 핵심 (suggest_all)

use crate::commands::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 카드가 기출인가: 노트에 표시했거나, 기출로 표시한 자료 구간과 이어졌다 (끊은 자동 연결 제외).
/// 별칭 c(card) · n(note) 가 있는 쿼리에 넣는다
pub const IS_EXAM: &str = "(COALESCE(n.exam_hits, 0) > 0 OR EXISTS (SELECT 1 FROM source_link xl JOIN source_segment xs ON xs.id = xl.segment_id
     WHERE xl.card_id = c.id AND xl.method != 'rejected' AND xs.exam_hits > 0))";

/// N3 핵심 제안: 덱 안 기출 비율 상위 이 비율 (올림, 같은 비율은 함께)
const TOP_SHARE: f64 = 0.25;
/// N3 핵심 제안: 기출 카드가 이보다 적은 하위 덱은 비율이 높아도 제안하지 않는다 (한두 장으로 핵심이 되지 않게)
const MIN_CARDS: i64 = 5;
/// 근거 줄 머리. 중요도 근거(importance_reason)는 줄마다 "기출 · …" / "자료 · …" (화면이 출처 태그를 줄에서 읽는다)
pub const REASON_HEAD: &str = "기출 · ";

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct ExamState {
    /// target 이 note 면 노트 id, segment 면 구간 id
    pub id: i64,
    /// 기출 횟수. None 이면 기출 아님
    pub hits: Option<i64>,
    /// 시험 이름이나 날짜. 모르면 비어 있다 (횟수보다 적을 수 있다)
    pub names: Vec<String>,
}

impl ExamState {
    /// 읽은 열 그대로 (0 · NULL 은 표시 없음)
    pub fn of(id: i64, hits: Option<i64>, names: Option<String>) -> ExamState {
        ExamState { id, hits: hits.filter(|h| *h > 0), names: names_of(names) }
    }
}

fn table(target: &str) -> Res<&'static str> {
    match target {
        "note" => Ok("note"),
        "segment" => Ok("source_segment"),
        _ => Err("기출 표시 대상이 올바르지 않아요.".into()),
    }
}

pub(crate) fn names_json(names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| serde_json::to_string(names).unwrap_or_default())
}

fn names_of(json: Option<String>) -> Vec<String> {
    json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}

/// 기출 한 번 더하기 (한 장 · 여러 장 · 자료 구간). exam 은 시험 이름이나 날짜.
/// 같은 이름이 이미 있거나, 이름 없이 다시 표시하면 그대로다. 그래서 같은 것을 여러 번 눌러도 횟수가 불지 않는다.
/// 횟수는 아는 이름 수보다 작지 않다. 이름 없이 표시해 둔 것에 이름을 붙이면 횟수는 그대로다. 바뀐 행 수를 돌려준다
pub fn mark(con: &Connection, target: &str, ids: &[i64], exam: Option<&str>) -> Res<usize> {
    let t = table(target)?;
    let exam = exam.map(|e| e.trim()).filter(|e| !e.is_empty());
    let mut changed = 0;
    for &id in ids {
        let Some((hits, names)) = con
            .query_row(&format!("SELECT exam_hits, exam_names FROM {t} WHERE id = ?1"), [id], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get(1)?)))
            .optional()
            .map_err(err)?
        else {
            continue;
        };
        let mut names = names_of(names);
        let hits = match (exam, hits.filter(|h| *h > 0)) {
            (Some(e), _) if names.iter().any(|n| n == e) => continue,
            (None, Some(_)) => continue,
            (Some(e), h) => {
                names.push(e.into());
                h.unwrap_or(0).max(names.len() as i64)
            }
            (None, None) => 1,
        };
        con.execute(&format!("UPDATE {t} SET exam_hits = ?2, exam_names = ?3 WHERE id = ?1"), params![id, hits, names_json(&names)])
            .map_err(err)?;
        changed += 1;
    }
    Ok(changed)
}

/// 값을 그대로 쓴다: 지우기(hits 없음) · 되돌리기(화면이 기억한 이전 값) · 횟수 고치기.
/// 지운 것은 NULL 이 아니라 0 으로 남긴다. NULL 이면 다음에 열 때 태그에서 다시 채워진다
pub fn set(con: &Connection, target: &str, states: &[ExamState]) -> Res<()> {
    let t = table(target)?;
    for s in states {
        let hits = s.hits.filter(|h| *h > 0).map_or(0, |h| h.max(s.names.len() as i64));
        let names = if hits > 0 { names_json(&s.names) } else { None };
        con.execute(&format!("UPDATE {t} SET exam_hits = ?2, exam_names = ?3 WHERE id = ?1"), params![s.id, hits, names]).map_err(err)?;
    }
    Ok(())
}

/// target "card" 는 카드의 노트에 단다 (자료 구간 표시를 연결된 카드에 퍼뜨릴 때, N3)
#[tauri::command]
pub fn mark_exam(state: State<AppState>, target: String, ids: Vec<i64>, exam: Option<String>) -> Res<usize> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let (target, ids) = if target == "card" { ("note".to_string(), notes_of(&tx, &ids)?) } else { (target, ids) };
    let n = mark(&tx, &target, &ids, exam.as_deref())?;
    suggest_all(&tx)?;
    tx.commit().map_err(err)?;
    Ok(n)
}

/// 카드들의 노트 (한 번씩)
fn notes_of(con: &Connection, card_ids: &[i64]) -> Res<Vec<i64>> {
    let list = card_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut st = con.prepare(&format!("SELECT DISTINCT note_id FROM card WHERE id IN ({}) ORDER BY note_id", if list.is_empty() { "-1" } else { &list })).map_err(err)?;
    let rows = st.query_map([], |r| r.get(0)).map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

#[tauri::command]
pub fn set_exam(state: State<AppState>, target: String, states: Vec<ExamState>) -> Res<()> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    set(&tx, &target, &states)?;
    suggest_all(&tx)?;
    tx.commit().map_err(err)
}

// ------------------------------------------------------------------ 덱의 기출 (N1 · N2 시험 목록, N4 한 줄)

/// 글자 속 연도 (없으면 0). 시험 목록은 연도 최근 순 (N1)
fn year(name: &str) -> i64 {
    name.as_bytes().windows(4).find(|w| w.iter().all(u8::is_ascii_digit)).and_then(|w| std::str::from_utf8(w).ok()?.parse().ok()).unwrap_or(0)
}

/// (카드, 횟수, 이름들, 신규인가)
type ExamRow = (i64, i64, Vec<String>, bool);

/// 덱(하위 덱 포함, None 이면 모든 덱) 카드의 노트 기출
fn deck_exam(con: &Connection, deck_id: Option<i64>) -> Res<Vec<ExamRow>> {
    let decks = crate::commands::load_decks(con)?;
    let ids = match deck_id {
        Some(id) => crate::commands::descendants(id, &decks),
        None => decks.iter().map(|d| d.id).collect(),
    };
    let ids = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, n.exam_hits, n.exam_names, c.state = 'new' FROM card c JOIN note n ON n.id = c.note_id
             WHERE c.deck_id IN ({ids}) AND n.exam_hits > 0 ORDER BY c.id"
        ))
        .map_err(err)?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, names_of(r.get(2)?), r.get(3)?))).map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

#[derive(Serialize)]
pub struct ExamName {
    name: String,
    /// 이 덱에서 이 시험이 표시된 카드 수
    cards: usize,
}

/// N1 · N2 이 덱에서 쓴 시험 (연도 최근 순)
#[tauri::command]
pub fn exam_list(state: State<AppState>, deck_id: Option<i64>) -> Res<Vec<ExamName>> {
    let con = state.db.lock().map_err(err)?;
    let mut by: std::collections::HashMap<String, usize> = Default::default();
    for (_, _, names, _) in deck_exam(&con, deck_id)? {
        for n in names {
            *by.entry(n).or_default() += 1;
        }
    }
    let mut out: Vec<ExamName> = by.into_iter().map(|(name, cards)| ExamName { name, cards }).collect();
    out.sort_by(|a, b| year(&b.name).cmp(&year(&a.name)).then(a.name.cmp(&b.name)));
    Ok(out)
}

#[derive(Serialize)]
pub struct ExamSummary {
    cards: usize,
    /// 2회 이상
    twice: usize,
    /// 시험 수 (이름 있는 것)
    exams: usize,
    /// 아직 안 본 (신규) 기출 카드
    unseen: usize,
    /// [기출 카드만 보기] (C2 cardIds)
    card_ids: Vec<i64>,
}

/// N4 덱 상세 한 줄. 표시가 없으면 cards = 0 (화면이 줄을 숨긴다)
#[tauri::command]
pub fn exam_summary(state: State<AppState>, deck_id: i64) -> Res<ExamSummary> {
    let con = state.db.lock().map_err(err)?;
    let rows = deck_exam(&con, Some(deck_id))?;
    let exams: std::collections::HashSet<&String> = rows.iter().flat_map(|r| &r.2).collect();
    Ok(ExamSummary {
        cards: rows.len(),
        twice: rows.iter().filter(|r| r.1 >= 2).count(),
        exams: exams.len(),
        unseen: rows.iter().filter(|r| r.3).count(),
        card_ids: rows.iter().map(|r| r.0).collect(),
    })
}

// ------------------------------------------------------------------ N3 중요도 제안

/// (덱, 카드 수, 기출 카드 수, 2회 이상 카드 수)
type DeckExam = (i64, i64, i64, i64);

/// 덱마다 기출 카드 수. 기출 = IS_EXAM (노트 표시 · 기출 구간과 이은 카드). 2회 이상은 노트 표시 횟수
fn deck_counts(con: &Connection, ids: &str) -> Res<Vec<DeckExam>> {
    let mut st = con
        .prepare(&format!(
            "SELECT c.deck_id, count(*), sum(CASE WHEN {IS_EXAM} THEN 1 ELSE 0 END), sum(CASE WHEN COALESCE(n.exam_hits, 0) >= 2 THEN 1 ELSE 0 END)
             FROM card c JOIN note n ON n.id = c.note_id WHERE c.deck_id IN ({ids}) GROUP BY c.deck_id ORDER BY c.deck_id"
        ))
        .map_err(err)?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

/// 덱 안(카드가 직접 든 덱끼리) 기출 비율 상위 25% 이면서 기출 카드 5장 이상인 덱 → 근거 한 줄.
/// 덱이 하나뿐이면 견줄 곳이 없어 제안하지 않는다. 같은 비율은 함께 들어간다
fn picks(rows: &[DeckExam]) -> HashMap<i64, String> {
    let ratio = |r: &DeckExam| r.2 as f64 / r.1.max(1) as f64;
    let mut v: Vec<&DeckExam> = rows.iter().filter(|r| r.1 > 0).collect();
    if v.len() < 2 {
        return HashMap::new();
    }
    v.sort_by(|a, b| ratio(b).total_cmp(&ratio(a)).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let top = ((v.len() as f64) * TOP_SHARE).ceil() as usize;
    let cut = ratio(v[top - 1]);
    v.into_iter()
        .filter(|r| r.2 >= MIN_CARDS && ratio(r) >= cut && ratio(r) > 0.0)
        .map(|r| {
            let twice = if r.3 > 0 { format!(" (2회 이상 {}장)", r.3) } else { String::new() };
            (r.0, format!("{REASON_HEAD}이 하위 덱 카드 {}장 중 {}장에 기출 표시{twice}", r.1, r.2))
        })
        .collect()
}

/// N3 기출 근거를 하위 덱 중요도 제안에 맞춘다 (최상위 덱마다). 확정(user) · 태그 · 이름 규칙 제안은 덮지 않는다.
/// 근거는 importance_source 'material'(데이터에서 나온 제안)로 남기고 줄 머리로 기출 · 자료를 가른다:
/// 'exam' 값을 새로 넣으면 deck 표의 CHECK 를 바꿔야 하고(표 다시 만들기), 동기화로 받은 옛 기기가 그 값을 넣지 못한다.
///   - 기본값 덱이 기준을 넘으면 핵심 + 기출 줄
///   - 자료 제안이 핵심이면 기출 줄을 앞에 더한다 (근거 2가지). 자료 제안이 핵심이 아니면 기출이 이긴다 (자료 줄은 버린다)
///   - 기준 밑으로 내려가면 기출 줄만 뺀다. 남는 줄이 없으면 기본값으로
///
/// 바뀐 행만 고친다 (동기화 변경 기록이 불지 않게). 고친 덱 수
pub fn suggest_all(con: &Connection) -> Res<usize> {
    let decks = crate::commands::load_decks(con)?;
    let mut changed = 0;
    for root in decks.iter().filter(|d| d.parent_id.is_none()) {
        let ids = crate::commands::descendants(root.id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let picks = picks(&deck_counts(con, &ids)?);
        let mut st = con
            .prepare(&format!("SELECT id, importance, importance_source, importance_reason FROM deck WHERE id IN ({ids}) AND importance_source IN ('default', 'material')"))
            .map_err(err)?;
        let rows: Vec<(i64, i64, String, Option<String>)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        for (id, level, source, reason) in rows {
            let material: Vec<&str> = reason.as_deref().unwrap_or_default().lines().filter(|l| !l.is_empty() && !l.starts_with(REASON_HEAD)).collect();
            let want: (i64, &str, Option<String>) = match (picks.get(&id), source.as_str()) {
                (Some(e), "material") if level == 2 && !material.is_empty() => (2, "material", Some(format!("{e}\n{}", material.join("\n")))),
                (Some(e), _) => (2, "material", Some(e.clone())),
                (None, "material") if material.is_empty() => (1, "default", None),
                (None, "material") => (level, "material", Some(material.join("\n"))),
                (None, _) => continue,
            };
            if want != (level, source.as_str(), reason.clone()) {
                con.execute(
                    "UPDATE deck SET importance = ?2, importance_source = ?3, importance_reason = ?4 WHERE id = ?1",
                    params![id, want.0, want.1, want.2],
                )
                .map_err(err)?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

// ------------------------------------------------------------------ 가져오기의 기출 태그

/// 태그 하나가 기출이면 시험 이름 (없으면 빈 문자열). 태그 경로(::)에 '기출' 단계가 있으면 기출이고 그 뒤 단계가 이름이다.
/// "기출" → "", "기출::2023::중간" → "2023 중간", "정보처리::기출" → "". Anki 태그는 띄어쓰기를 못 써서 _ 는 띄어쓰기로 읽는다
fn exam_tag(tag: &str) -> Option<String> {
    let parts: Vec<&str> = tag.split("::").collect();
    let i = parts.iter().position(|p| *p == "기출")?;
    Some(parts[i + 1..].join(" ").replace('_', " ").trim().to_string())
}

/// 노트 태그 → (횟수, 이름). 횟수는 이름 붙은 기출 태그 수, 이름 없는 것만 있으면 1
pub fn from_tags(tags: &str) -> Option<(i64, Vec<String>)> {
    let mut found = false;
    let mut names: Vec<String> = vec![];
    for name in tags.split_whitespace().filter_map(exam_tag) {
        found = true;
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    found.then(|| ((names.len() as i64).max(1), names))
}

/// 내보내기: 기출 표시 → 노트 태그 (from_tags 의 거꾸로, N20). NULL(정하지 않음)이면 태그 그대로, 0 이면 기출 태그를 뺀다.
/// 이름마다 "기출::이름"(띄어쓰기는 _), 이름 없이 표시만 했으면 "기출". 기출 아닌 태그와 표시와 맞는 기출 태그("과목::기출::2023" 처럼
/// 앞 단계가 있는 것도)는 그대로 둔다. 맞는지는 가져오기와 같은 exam_tag 로 본다
pub fn to_tags(tags: &str, hits: Option<i64>, names: Option<String>) -> String {
    let tags: Vec<&str> = tags.split_whitespace().collect();
    let Some(hits) = hits else { return tags.join(" ") };
    let names = names_of(names);
    let mut want: Vec<String> = match (hits > 0, names.is_empty()) {
        (false, _) => vec![],
        (true, true) => vec!["기출".into()],
        (true, false) => names.iter().map(|n| format!("기출::{}", n.split_whitespace().collect::<Vec<_>>().join("_"))).collect(),
    };
    let mut out: Vec<String> = vec![];
    for t in tags {
        let Some(name) = exam_tag(t) else {
            out.push(t.into());
            continue;
        };
        if let Some(i) = want.iter().position(|w| exam_tag(w).as_ref() == Some(&name)) {
            want.remove(i);
            out.push(t.into());
        }
    }
    out.extend(want);
    out.join(" ")
}

/// 아직 정하지 않은(NULL) 노트의 기출 태그를 표시로 옮긴다. 가져온 직후와 앱을 열 때 (이 기능보다 먼저 가져온 노트, 다른 기기에서 받은 노트).
/// 사용자가 고친 값(0 포함)은 덮지 않는다. 어느 기기든 같은 태그로 같은 값을 만들므로 동기화 변경 기록은 남기지 않는다
/// (applying, 동기화 엔진의 apply 와 같은 방식). 호출하는 쪽이 트랜잭션 안에서 부른다: 실패하면 플래그도 함께 되돌아간다
pub fn fill_from_tags(con: &Connection) -> rusqlite::Result<usize> {
    let notes: Vec<(i64, String)> = {
        let mut st = con.prepare("SELECT id, tags FROM note WHERE exam_hits IS NULL AND tags LIKE '%기출%'")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    let found: Vec<_> = notes.into_iter().filter_map(|(id, tags)| from_tags(&tags).map(|e| (id, e))).collect();
    if found.is_empty() {
        return Ok(0);
    }
    con.execute("UPDATE sync_flags SET applying = 1", [])?;
    for (id, (hits, names)) in &found {
        con.execute("UPDATE note SET exam_hits = ?2, exam_names = ?3 WHERE id = ?1", params![id, hits, names_json(names)])?;
    }
    con.execute("UPDATE sync_flags SET applying = 0", [])?;
    Ok(found.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn year_sort_key() {
        assert_eq!((year("2023년 필기 1회"), year("실기 2022"), year("중간고사")), (2023, 2022, 0));
    }

    #[test]
    fn tags_to_exam() {
        assert_eq!(from_tags("중요도::최우선 기출"), Some((1, vec![])));
        assert_eq!(from_tags("기출::2023::중간 기출::2024_기말 기출"), Some((2, vec!["2023 중간".into(), "2024 기말".into()])));
        assert_eq!(from_tags("정보처리기사::기출::2022 정보처리기사::기출::2022"), Some((1, vec!["2022".into()])), "같은 이름은 한 번");
        assert_eq!(from_tags("기출문제 비기출 과목::1"), None, "단계 이름이 정확히 '기출'일 때만");
    }

    fn note_state(con: &Connection, id: i64) -> (Option<i64>, Option<String>) {
        con.query_row("SELECT exam_hits, exam_names FROM note WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[test]
    fn mark_set_and_tags() {
        let con = db::open_in_memory().unwrap();
        for (id, tags) in [(1, ""), (2, ""), (3, "기출::2023"), (4, "기출")] {
            con.execute("INSERT INTO note (id, note_type_id, fields, tags, created_at, updated_at) VALUES (?1, 1, '[]', ?2, 0, 0)", params![id, tags])
                .unwrap();
        }
        // 이름 없이 여러 장 한꺼번에. 다시 눌러도 횟수가 불지 않는다
        assert_eq!(mark(&con, "note", &[1, 2], None).unwrap(), 2);
        assert_eq!(mark(&con, "note", &[1, 2], None).unwrap(), 0);
        assert_eq!(note_state(&con, 1), (Some(1), None));
        // 이름을 붙이면 이름 없던 1번이 그 이름이 된다. 새 이름이면 횟수가 는다. 같은 이름은 그대로
        mark(&con, "note", &[1], Some(" 2024 기말 ")).unwrap();
        assert_eq!(note_state(&con, 1), (Some(1), Some(r#"["2024 기말"]"#.into())));
        mark(&con, "note", &[1], Some("2023 중간")).unwrap();
        assert_eq!(mark(&con, "note", &[1], Some("2023 중간")).unwrap(), 0);
        assert_eq!(note_state(&con, 1), (Some(2), Some(r#"["2024 기말","2023 중간"]"#.into())));
        // 그대로 쓰기: 지우기는 0 · 되돌리기. 횟수는 이름 수보다 작지 않다
        set(&con, "note", &[ExamState { id: 2, hits: None, names: vec![] }, ExamState { id: 1, hits: Some(1), names: vec!["a".into(), "b".into()] }]).unwrap();
        assert_eq!(note_state(&con, 2), (Some(0), None));
        assert_eq!(note_state(&con, 1).0, Some(2));
        assert_eq!(mark(&con, "note", &[2], None).unwrap(), 1, "지운 노트도 다시 표시할 수 있다");
        assert!(mark(&con, "card", &[1], None).is_err());

        // 태그는 정하지 않은 노트에만. 표시가 있는 노트(1)는 그대로, 사용자가 지운 표시(0)는 다시 채우지 않는다
        con.execute("UPDATE note SET tags = '기출::2020' WHERE id = 1", []).unwrap();
        con.execute("UPDATE sync_flags SET enabled = 1", []).unwrap();
        assert_eq!(fill_from_tags(&con).unwrap(), 2);
        assert_eq!(note_state(&con, 3), (Some(1), Some(r#"["2023"]"#.into())));
        assert_eq!(note_state(&con, 4), (Some(1), None));
        assert_eq!(note_state(&con, 1).0, Some(2));
        // 동기화가 켜져 있어도 변경 기록을 남기지 않는다 (기기마다 같은 값을 만든다). 플래그는 다시 꺼진다
        let n = |sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(n("SELECT count(*) FROM sync_outbox"), 0);
        assert_eq!(n("SELECT applying FROM sync_flags"), 0);
        set(&con, "note", &[ExamState { id: 3, hits: None, names: vec![] }]).unwrap();
        assert_eq!(n("SELECT count(*) FROM sync_outbox"), 1, "사용자가 고친 것은 올린다");
        assert_eq!(fill_from_tags(&con).unwrap(), 0);
        assert_eq!(note_state(&con, 3), (Some(0), None));
    }
}
