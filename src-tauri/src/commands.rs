//! 화면이 부르는 명령. 화면 하나 = 명령 하나를 기본으로 한다.

use crate::db::{self, now_ms, study_day, study_day_end_ms};
use crate::queue::{self, RatingAction};
use crate::notetype::{self, IS_CLOZE, TEMPLATE_OF_CARD};
use crate::render;
use crate::srs::{self, SrsState};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub fsrs: fsrs::FSRS,
    pub data_dir: std::path::PathBuf,
    /// 실행 취소 스택. 앱을 닫으면 사라진다 (Anki 와 같다)
    pub undo: Mutex<Vec<Undo>>,
    /// B2-p 가져오기 취소 요청
    pub import_cancel: std::sync::atomic::AtomicBool,
    /// 시작할 때 학습 데이터를 열지 못한 이유 (S1-6). 있으면 지금 연결은 빈 메모리 DB 다
    pub startup_error: Mutex<Option<String>>,
}

/// session_card_state 한 줄: again_count, is_buried, buried_at, buried_reason, shown_count
type SessionCardRow = (i64, i64, Option<i64>, Option<String>, i64);

/// 되돌리기 위해 바꾸기 직전의 카드와 세션 상태를 남긴다
pub struct Undo {
    session_id: i64,
    card_id: i64,
    card: (SrsState, Option<i64>, Option<String>), // 일정, buried_until, buried_reason
    session_card: Option<SessionCardRow>,
    log_id: Option<i64>,
}

const UNDO_LIMIT: usize = 20;

fn snapshot(con: &Connection, session_id: i64, card_id: i64) -> Res<Undo> {
    let buried: (Option<i64>, Option<String>) = con
        .query_row("SELECT buried_until, buried_reason FROM card WHERE id = ?1", [card_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?;
    let session_card = con
        .query_row(
            "SELECT again_count, is_buried, buried_at, buried_reason, shown_count FROM session_card_state WHERE session_id = ?1 AND card_id = ?2",
            params![session_id, card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(err)?;
    Ok(Undo { session_id, card_id, card: (load_srs(con, card_id)?, buried.0, buried.1), session_card, log_id: None })
}

fn push_undo(state: &AppState, u: Undo) -> Res<()> {
    let mut stack = state.undo.lock().map_err(err)?;
    stack.push(u);
    if stack.len() > UNDO_LIMIT {
        stack.remove(0);
    }
    Ok(())
}

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ------------------------------------------------------------------ 공통 로딩

pub(crate) fn load_decks(con: &Connection) -> Res<Vec<queue::Deck>> {
    let mut st = con
        .prepare(
            "SELECT d.id, d.name, d.parent_id, c.target_date, COALESCE(c.include_target_day,0), c.daily_new_limit,
                    CASE WHEN c.today_new_day = ?1 THEN c.today_new END, COALESCE(c.paused, 0), c.paused_until, d.weak
             FROM deck d LEFT JOIN deck_config c ON c.deck_id = d.id WHERE d.archived_at IS NULL",
        )
        .map_err(err)?;
    let today = study_day(now_ms());
    let rows = st
        .query_map([db::date_from_day(today)], |r| {
            let target: Option<String> = r.get(3)?;
            let until = r.get::<_, Option<String>>(8)?.as_deref().and_then(db::day_from_date);
            Ok(queue::Deck {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                target_day: target.as_deref().and_then(db::day_from_date),
                include_target_day: r.get::<_, i64>(4)? == 1,
                daily_new_limit: r.get(5)?,
                today_new: r.get(6)?,
                // 날짜가 지난 쉬기는 끝난 것으로 본다 (홈이 자동으로 다시 시작시킨다)
                paused: r.get::<_, i64>(7)? == 1 && until.is_none_or(|u| u >= today),
                paused_until: until,
                weak: r.get::<_, i64>(9)? == 1,
                ..Default::default()
            })
        })
        .map_err(err)?;
    let mut decks: Vec<queue::Deck> = rows.collect::<Result<_, _>>().map_err(err)?;
    // H 시험 계획: 충돌 조정 · 당겨 맞추기 · 하루 몫 고정을 큐가 따르게 한다 (목표일은 그대로)
    crate::plan::adjust_decks(con, &mut decks, today)?;
    Ok(decks)
}

pub(crate) fn load_cards(con: &Connection) -> Res<Vec<queue::Card>> {
    let fronts = crate::notetype::Fronts::load(con)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.deck_id, c.state, c.eff_importance, c.due_at, COALESCE(c.anki_card_id, c.id), c.buried_until,
                    CASE WHEN {IS_CLOZE} THEN NULL ELSE c.note_id END, c.note_id, {}, n.note_type_id, c.template_ord, n.fields
             FROM card_effective c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id",
            crate::exam::IS_EXAM
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| {
            Ok(queue::Card {
                id: r.get(0)?,
                deck_id: r.get(1)?,
                state: r.get(2)?,
                importance: r.get(3)?,
                due_day: r.get::<_, Option<i64>>(4)?.map(study_day),
                position: r.get(5)?,
                buried_until: r.get::<_, Option<i64>>(6)?.map(study_day),
                sibling: r.get(7)?,
                note_id: r.get(8)?,
                weak: false,
                exam: r.get(9)?,
                front_empty: fronts.empty(r.get(10)?, r.get(11)?, &r.get::<_, String>(12)?),
            })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

pub(crate) fn today_start_ms(today: i64) -> i64 {
    study_day_end_ms(today - 1) + 1
}

/// 덱별로 오늘 처음 학습한 신규 카드 수
pub(crate) fn introduced_today(con: &Connection, today: i64) -> Res<HashMap<i64, i64>> {
    let mut st = con
        .prepare(
            "SELECT c.deck_id, count(DISTINCT r.card_id) FROM review_log r JOIN card c ON c.id = r.card_id
             WHERE r.prev_state = 'new' AND coalesce(r.from_buried, 0) = 0 AND r.reviewed_at >= ?1 GROUP BY c.deck_id",
        )
        .map_err(err)?;
    let rows = st
        .query_map([today_start_ms(today)], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

/// 오늘 이미 한 학습 (큐가 오늘 분량을 다시 만들지 않게)
fn today_progress(con: &Connection, today: i64) -> Res<queue::Today> {
    let since = today_start_ms(today);
    // 밀린 복습 하루 상한(큐 명세의 200장)에서 뺀다
    let backlog_done: i64 = con
        .query_row("SELECT count(DISTINCT card_id) FROM review_log WHERE overdue = 1 AND reviewed_at >= ?1", [since], |r| r.get(0))
        .map_err(err)?;
    let mut st = con
        .prepare("SELECT DISTINCT c.note_id FROM review_log r JOIN card c ON c.id = r.card_id WHERE r.reviewed_at >= ?1 AND r.kind IS NULL")
        .map_err(err)?;
    let siblings_seen = st.query_map([since], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    Ok(queue::Today { introduced: introduced_today(con, today)?, backlog_done: backlog_done as usize, siblings_seen })
}

pub(crate) fn root_of(id: i64, decks: &HashMap<i64, &queue::Deck>) -> i64 {
    let mut cur = id;
    for _ in 0..64 {
        match decks.get(&cur).and_then(|d| d.parent_id) {
            Some(p) if decks.contains_key(&p) => cur = p,
            _ => break,
        }
    }
    cur
}

/// 최상위부터 이 덱까지의 이름
pub(crate) fn path_of(id: i64, by_id: &HashMap<i64, &queue::Deck>) -> Vec<String> {
    let mut path = vec![];
    let mut cur = Some(id);
    while let Some(d) = cur.and_then(|c| by_id.get(&c)) {
        path.insert(0, d.name.clone());
        cur = d.parent_id;
        if path.len() > 64 {
            break; // 부모가 순환하면 멈춘다
        }
    }
    path
}

pub(crate) fn descendants(root: i64, decks: &[queue::Deck]) -> Vec<i64> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        out.extend(decks.iter().filter(|d| d.parent_id == Some(cur)).map(|d| d.id));
        i += 1;
    }
    out
}

fn plan_for(con: &Connection, deck_ids: Option<&[i64]>, scale: f64, day: i64) -> Res<(Vec<queue::Deck>, queue::QueuePlan)> {
    let decks = load_decks(con)?;
    let cards = load_cards(con)?;
    let done = if day == study_day(now_ms()) { today_progress(con, day)? } else { queue::Today::default() };
    let plan = queue::build_queue_with_progress(day, &decks, &cards, deck_ids, scale, &done);
    Ok((decks, plan))
}

// ------------------------------------------------------------------ A. 홈

#[derive(Serialize)]
pub struct HomeDeck {
    id: i64,
    name: String,
    review: usize,
    new: usize,
    tomorrow: usize,
    dday: Option<i64>,
}

#[derive(Serialize)]
pub struct Home {
    has_decks: bool,
    review: usize,
    new: usize,
    done_today: i64,
    decks: Vec<HomeDeck>,
    idle_decks: Vec<IdleDeck>,
    tomorrow: usize,
    buried: i64,
    /// 보관한 최상위 덱 수 (A2-p "보관한 덱 N")
    archived: i64,
    /// 쉬기 날짜가 지나 오늘 자동으로 다시 시작한 덱 (A2-p 토스트)
    resumed: Vec<String>,
}

/// A2 "오늘 볼 카드가 없는 덱" 한 줄
#[derive(Serialize)]
pub struct IdleDeck {
    id: i64,
    name: String,
    paused: bool,
    /// 'YYYY-MM-DD' 까지 쉰다. 쉬는 중이고 날짜가 없으면 null
    paused_until: Option<String>,
    /// 다음 복습일 'YYYY-MM-DD' ("예정 없음 · 다음 복습 10/18")
    next_due: Option<String>,
}

#[tauri::command]
pub fn home(state: State<AppState>) -> Res<Home> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    // 쉬기 날짜가 지난 덱은 다시 시작한다
    let today_date = db::date_from_day(today);
    let resumed: Vec<String> = {
        let mut st = con
            .prepare("SELECT d.name FROM deck_config c JOIN deck d ON d.id = c.deck_id WHERE c.paused = 1 AND c.paused_until < ?1")
            .map_err(err)?;
        let rows = st.query_map([&today_date], |r| r.get(0)).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    con.execute("UPDATE deck_config SET paused = 0, paused_until = NULL WHERE paused = 1 AND paused_until < ?1", [&today_date])
        .map_err(err)?;
    let (decks, plan) = plan_for(&con, None, 1.0, today)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();

    let mut per_root: HashMap<i64, (usize, usize)> = HashMap::new();
    for c in plan.ordered() {
        let e = per_root.entry(root_of(c.deck_id, &by_id)).or_default();
        if c.state == "new" && c.buried_until.is_none() { e.1 += 1 } else { e.0 += 1 }
    }
    // A2-b: 오늘을 다 끝낸 덱은 "완료 · 내일 복습 N" 으로 보여준다
    let cards = load_cards(&con)?;
    let tomorrow_plan = queue::build_queue(today + 1, &decks, &cards, None, 1.0);
    let mut tomorrow_per_root: HashMap<i64, usize> = HashMap::new();
    for c in tomorrow_plan.ordered().into_iter().filter(|c| c.state != "new") {
        *tomorrow_per_root.entry(root_of(c.deck_id, &by_id)).or_default() += 1;
    }
    let mut next_due: HashMap<i64, i64> = HashMap::new();
    for c in cards.iter().filter(|c| c.state != "new" && c.buried_until.is_none()) {
        if let Some(d) = c.due_day.filter(|&d| d > today) {
            let e = next_due.entry(root_of(c.deck_id, &by_id)).or_insert(d);
            *e = (*e).min(d);
        }
    }
    // 오늘 학습한 적이 있는 최상위 덱 (오늘 몫을 끝낸 덱)
    let mut studied_roots = std::collections::HashSet::new();
    {
        let mut st = con
            .prepare("SELECT DISTINCT c.deck_id FROM review_log r JOIN card c ON c.id = r.card_id WHERE r.reviewed_at >= ?1 AND r.kind IS NULL")
            .map_err(err)?;
        for id in st.query_map([today_start_ms(today)], |r| r.get::<_, i64>(0)).map_err(err)? {
            studied_roots.insert(root_of(id.map_err(err)?, &by_id));
        }
    }
    let roots: Vec<&queue::Deck> = decks.iter().filter(|d| d.parent_id.is_none()).collect();
    let mut active = vec![];
    let mut idle = vec![];
    for d in &roots {
        let dday = d.target_day.map(|t| t - today);
        let tomorrow = tomorrow_per_root.get(&d.id).copied().unwrap_or(0);
        match per_root.get(&d.id) {
            // 쉬는 덱은 오늘 학습했더라도 "오늘 볼 카드가 없는 덱" 쪽 맨 위에 둔다 (A2-p)
            _ if d.paused => idle.insert(
                0,
                IdleDeck {
                    id: d.id,
                    name: d.name.clone(),
                    paused: true,
                    paused_until: d.paused_until.map(db::date_from_day),
                    next_due: None,
                },
            ),
            Some(&(review, new)) => active.push(HomeDeck { id: d.id, name: d.name.clone(), review, new, tomorrow, dday }),
            None if studied_roots.contains(&d.id) => {
                active.push(HomeDeck { id: d.id, name: d.name.clone(), review: 0, new: 0, tomorrow, dday })
            }
            None => idle.push(IdleDeck {
                id: d.id,
                name: d.name.clone(),
                paused: false,
                paused_until: None,
                next_due: next_due.get(&d.id).map(|&x| db::date_from_day(x)),
            }),
        }
    }
    // 목표일이 가까운 덱을 위로, 나머지는 분량 순 (브리프 1장)
    active.sort_by_key(|d| (d.dday.map_or(i64::MAX, |x| x.max(0)), usize::MAX - (d.review + d.new)));

    let done_today: i64 = con
        .query_row(
            "SELECT count(DISTINCT card_id) FROM review_log WHERE reviewed_at >= ?1 AND session_id IS NOT NULL",
            [today_start_ms(today)],
            |r| r.get(0),
        )
        .map_err(err)?;
    let buried: i64 = con
        .query_row(
            "SELECT count(*) FROM card c JOIN deck d ON d.id = c.deck_id WHERE c.buried_until IS NOT NULL AND d.archived_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    let tomorrow = tomorrow_plan.ordered().iter().filter(|c| c.state != "new").count();
    record_daily_plan(&con, today, &roots, &per_root)?;

    Ok(Home {
        has_decks: !roots.is_empty(),
        review: per_root.values().map(|v| v.0).sum(),
        new: per_root.values().map(|v| v.1).sum(),
        done_today,
        decks: active,
        idle_decks: idle,
        tomorrow,
        buried,
        archived: con
            .query_row(
                "SELECT count(*) FROM deck d LEFT JOIN deck p ON p.id = d.parent_id
                 WHERE d.archived_at IS NOT NULL AND (p.id IS NULL OR p.archived_at IS NULL)",
                [],
                |r| r.get(0),
            )
            .map_err(err)?,
        resumed,
    })
}

/// 그날 처음 본 계획을 남긴다. 이미 있으면 그대로 둔다 (합격선 ①의 분모)
fn record_daily_plan(con: &Connection, today: i64, roots: &[&queue::Deck], per_root: &HashMap<i64, (usize, usize)>) -> Res<()> {
    let day = db::date_from_day(today);
    for d in roots {
        let (review, new) = per_root.get(&d.id).copied().unwrap_or((0, 0));
        con.execute(
            "INSERT OR IGNORE INTO daily_plan (study_day, deck_id, planned_review, planned_new, has_target, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![day, d.id, review as i64, new as i64, d.target_day.is_some() as i64, now_ms()],
        )
        .map_err(err)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ B1. 새 덱

#[tauri::command]
pub fn create_deck(state: State<AppState>, name: String, parent_id: Option<i64>) -> Res<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err("덱 이름을 입력해 주세요.".into());
    }
    if name.contains("::") {
        return Err("이름에 :: 는 쓸 수 없어요. 하위 덱은 덱 안에서 만들어요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    let dup: Option<i64> = con
        .query_row(
            "SELECT id FROM deck WHERE name = ?1 AND parent_id IS ?2",
            params![name, parent_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    if dup.is_some() {
        return Err("같은 이름의 덱이 이미 있어요.".into());
    }
    let now = now_ms();
    con.execute(
        "INSERT INTO deck (parent_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![parent_id, name, now],
    )
    .map_err(err)?;
    Ok(con.last_insert_rowid())
}

#[tauri::command]
pub fn rename_deck(state: State<AppState>, deck_id: i64, name: String) -> Res<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err("덱 이름을 입력해 주세요.".into());
    }
    if name.contains("::") {
        return Err("이름에 :: 는 쓸 수 없어요. 하위 덱은 덱 안에서 만들어요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    // 최상위 덱은 parent_id 가 NULL 이라 UNIQUE 제약이 걸리지 않는다. create_deck 처럼 직접 본다
    let dup: bool = con
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM deck d JOIN deck me ON me.id = ?1 WHERE d.name = ?2 AND d.parent_id IS me.parent_id AND d.id <> ?1)",
            params![deck_id, name],
            |r| r.get(0),
        )
        .map_err(err)?;
    if dup {
        return Err("같은 이름의 덱이 이미 있어요.".into());
    }
    con.execute("UPDATE deck SET name = ?2, updated_at = ?3 WHERE id = ?1", params![deck_id, name, now_ms()])
        .map_err(|e| if e.to_string().contains("UNIQUE") { "같은 이름의 덱이 이미 있어요.".into() } else { err(e) })?;
    Ok(())
}

#[tauri::command]
pub fn archive_deck(state: State<AppState>, deck_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    let decks = load_decks(&con)?;
    for id in descendants(deck_id, &decks) {
        con.execute("UPDATE deck SET archived_at = ?2 WHERE id = ?1", params![id, now_ms()]).map_err(err)?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct ArchivedDeck {
    id: i64,
    name: String,
    archived_at: i64,
    cards: i64,
    subdecks: i64,
    target_date: Option<String>,
    /// 목표일이 지난 덱 (E3 회색 배지)
    target_passed: bool,
}

/// 보관한 최상위 덱 (부모가 보관되지 않은 덱 포함). 되살리거나 지울 때 쓴다
#[tauri::command]
pub fn archived_decks(state: State<AppState>) -> Res<Vec<ArchivedDeck>> {
    let con = state.db.lock().map_err(err)?;
    let mut st = con
        .prepare(
            "WITH RECURSIVE tops AS (
                 SELECT d.id FROM deck d LEFT JOIN deck p ON p.id = d.parent_id
                 WHERE d.archived_at IS NOT NULL AND (p.id IS NULL OR p.archived_at IS NULL)
             ),
             sub(top, id) AS (SELECT id, id FROM tops UNION ALL SELECT sub.top, d.id FROM deck d JOIN sub ON d.parent_id = sub.id)
             SELECT d.id, d.name, d.archived_at,
                    (SELECT count(*) FROM card c WHERE c.deck_id IN (SELECT id FROM sub WHERE sub.top = d.id)),
                    (SELECT count(*) - 1 FROM sub WHERE sub.top = d.id),
                    cfg.target_date
             FROM deck d JOIN tops t ON t.id = d.id LEFT JOIN deck_config cfg ON cfg.deck_id = d.id ORDER BY d.archived_at DESC",
        )
        .map_err(err)?;
    let today = study_day(now_ms());
    let rows = st
        .query_map([], |r| {
            let target_date: Option<String> = r.get(5)?;
            Ok(ArchivedDeck {
                id: r.get(0)?,
                name: r.get(1)?,
                archived_at: r.get(2)?,
                cards: r.get(3)?,
                subdecks: r.get(4)?,
                target_passed: target_date.as_deref().and_then(db::day_from_date).is_some_and(|t| t < today),
                target_date,
            })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

/// 덱을 잠시 쉬게 하거나 다시 학습한다. 하위 덱까지 오늘의 큐에서 빠진다.
/// until 은 'YYYY-MM-DD' (그날 포함). 없으면 다시 시작할 때까지. 목표일은 자동으로 미루지 않는다 (C4-p)
#[tauri::command]
pub fn set_deck_paused(state: State<AppState>, deck_id: i64, paused: bool, until: Option<String>) -> Res<()> {
    if let Some(u) = &until {
        db::day_from_date(u).ok_or("날짜 형식이 올바르지 않아요.")?;
    }
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "INSERT INTO deck_config (deck_id, paused, paused_until, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(deck_id) DO UPDATE SET paused = ?2, paused_until = ?3, updated_at = ?4",
        params![deck_id, paused as i64, if paused { until } else { None }, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// 보관한 덱과 그 하위 덱을 되살린다. 보관 중에도 일정은 흘렀으므로 지금 밀린 복습 수를 돌려준다 (E3 토스트)
#[tauri::command]
pub fn unarchive_deck(state: State<AppState>, deck_id: i64) -> Res<i64> {
    let con = state.db.lock().map_err(err)?;
    let sub = "WITH RECURSIVE sub(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM deck d JOIN sub ON d.parent_id = sub.id)";
    con.execute(&format!("{sub} UPDATE deck SET archived_at = NULL, updated_at = ?2 WHERE id IN (SELECT id FROM sub)"), params![deck_id, now_ms()])
        .map_err(err)?;
    con.query_row(
        &format!(
            "{sub} SELECT count(*) FROM card WHERE deck_id IN (SELECT id FROM sub) AND state IN ('review','relearning') AND due_at < ?2"
        ),
        params![deck_id, today_start_ms(study_day(now_ms()))],
        |r| r.get(0),
    )
    .map_err(err)
}

/// 덱과 하위 덱, 그 안의 카드를 지운다. 복습 기록은 남는다 (schema: ON DELETE SET NULL)
#[tauri::command]
pub fn delete_deck(state: State<AppState>, deck_id: i64) -> Res<()> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    delete_deck_rows(&tx, deck_id)?;
    tx.commit().map_err(err)
}

fn delete_deck_rows(tx: &Connection, deck_id: i64) -> Res<()> {
    // 보관한 하위 덱도 포함한다 (load_decks 는 보관한 덱을 빼므로 SQL 로 직접 훑는다)
    let sub = "WITH RECURSIVE sub(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM deck d JOIN sub ON d.parent_id = sub.id)";
    let notes: Vec<i64> = {
        let mut st = tx.prepare(&format!("{sub} SELECT DISTINCT note_id FROM card WHERE deck_id IN (SELECT id FROM sub)")).map_err(err)?;
        st.query_map([deck_id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    tx.execute(&format!("{sub} DELETE FROM card WHERE deck_id IN (SELECT id FROM sub)"), [deck_id]).map_err(err)?;
    // 이 덱에 카드가 있던 노트 중 카드가 하나도 남지 않은 것만 지운다. 빈칸 노트의 카드가 다른 덱에 있을 수 있고,
    // 다른 곳의 카드 없는 노트(동기화가 되살리는 중인 노트 등)는 이 덱과 상관없다 (T3-07: 그 노트의 나중 수정을 모든 기기에서 지웠다)
    tx.execute(&format!("DELETE FROM note WHERE id IN ({}) AND NOT EXISTS (SELECT 1 FROM card WHERE card.note_id = note.id)", id_list(&notes)), [])
        .map_err(err)?;
    tx.execute("DELETE FROM deck WHERE id = ?1", [deck_id]).map_err(err)?;
    Ok(())
}

// ------------------------------------------------------------------ C1. 덱 상세

#[derive(Serialize)]
pub struct Subdeck {
    id: i64,
    name: String,
    cards: i64,
    importance: i64,
    importance_source: String,
    /// C1-t 트리: 부모와 깊이 (이 덱 바로 아래가 1)
    parent_id: i64,
    depth: usize,
    /// 오늘 볼 카드 (이 하위 덱 아래 전부). 행들의 합이 덱의 오늘 분량과 같다
    today: usize,
    unseen: i64,
    /// 바로 아래 하위 덱 수
    children: usize,
    /// 아래 덱들의 중요도가 섞였는가 ([나뉨])
    mixed: bool,
    paused: bool,
    /// C1-w 최근 14일 (오늘 포함) 복습 수와 Again 비율. 복습이 없으면 None
    reviews14: i64,
    again_rate: Option<f64>,
    /// 3번 이상 틀린 카드 (lapses ≥ 3). 제안 근거 줄
    lapsed: i64,
    /// 약함으로 확정
    weak: bool,
    /// "약해 보여요?" 제안: 복습 20번 이상 + (Again 이 덱 평균의 1.5배 이상 또는 25% 이상). [아니에요] 뒤 14일은 제안하지 않는다
    weak_suggested: bool,
    /// 확정했는데 요즘 덱 평균 아래로 내려옴 ("요즘 괜찮아요 · 해제할까요?" 한 번만)
    weak_recovered: bool,
}

impl Subdeck {
    fn shallow(s: &Subdeck) -> Subdeck {
        Subdeck { name: s.name.clone(), importance_source: s.importance_source.clone(), ..*s }
    }
}

/// 하위 덱 목표가 상위 덱에서 온다 (C1-t "상위 덱 목표 D-12를 따라요")
#[derive(Serialize)]
pub struct TargetFrom {
    name: String,
    dday: i64,
}

#[derive(Serialize)]
pub struct DeckDetail {
    id: i64,
    /// 하위 덱이면 바로 위 덱 (뒤로가기)
    parent_id: Option<i64>,
    name: String,
    path: Vec<String>,
    total: i64,
    unseen: i64,
    /// 접어둔 카드 (buried_until 이 있다)
    hard: i64,
    /// 그중 같은 학습에서 두 번 막혀 자동으로 접힌 카드. 나머지는 직접 접었다 (C1 "자동 n · 직접 n")
    hard_auto: i64,
    /// 접어둔 카드가 가장 먼저 오늘의 학습에 돌아오는 날 'YYYY-MM-DD'
    hard_next: Option<String>,
    backlog: usize,
    today_review: usize,
    today_new: usize,
    today_returning: usize,
    reason: Option<String>,
    target_date: Option<String>,
    include_target_day: bool,
    daily_new_limit: Option<i64>,
    /// C4 "오늘만 신규 분량 조정" 으로 저장한 값 (오늘 것만)
    today_new_override: Option<i64>,
    dday: Option<i64>,
    seen_pct: i64,
    core_total: i64,
    core_seen: i64,
    core_learned_pct: i64,
    /// 하루 신규 분량과 그 분량으로 끝내는 데 걸리는 날
    per_day: i64,
    finish_days: Option<i64>,
    learnable_days: Option<i64>,
    /// 목표일이 있을 때만. 이 분량으로 목표일 안에 끝나는가
    on_track: Option<bool>,
    subdeck_count: usize,
    studied: bool,
    importance: [i64; 3],
    own_importance: i64,
    unconfirmed: i64,
    /// 바로 아래 하위 덱 (depth 1). tree 의 일부다
    subdecks: Vec<Subdeck>,
    /// 모든 하위 덱. 깊이 우선 (C1-t)
    tree: Vec<Subdeck>,
    target_from: Option<TargetFrom>,
    paused_until: Option<String>,
    /// 오늘(학습일) 이 덱에서 본 카드 수 (MVP 6.9)
    done_today: i64,
    /// 잠시 쉬는 덱
    paused: bool,
    /// C1-w 이 덱 전체의 최근 14일 Again 비율 (덱 평균)
    again_avg: Option<f64>,
    /// J1 · C4 이 덱이 "써서 답하기"를 켰는가
    write_mode: bool,
    /// 위 덱의 "써서 답하기"를 따르면 그 덱 이름
    write_from: Option<String>,
}

/// C1-w 제안 기준
const WEAK_MIN_REVIEWS: i64 = 20;
const WEAK_RATIO_OF_AVG: f64 = 1.5;
const WEAK_ABS_RATE: f64 = 0.25;
const WEAK_DAYS: i64 = 14;

/// 덱들(하위 덱 포함)의 최근 14일 복습 수와 Again 수
fn again_counts(con: &Connection, ids: &str, today: i64) -> Res<(i64, i64)> {
    con.query_row(
        &format!(
            "SELECT count(*), coalesce(sum(r.rating = 1), 0) FROM review_log r JOIN card c ON c.id = r.card_id
             WHERE c.deck_id IN ({ids}) AND r.reviewed_at >= ?1 AND r.kind IS NULL"
        ),
        [today_start_ms(today - WEAK_DAYS + 1)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(err)
}

#[tauri::command]
pub fn deck_detail(state: State<AppState>, deck_id: i64) -> Res<DeckDetail> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    let (decks, plan) = plan_for(&con, Some(&[deck_id]), 1.0, today)?;
    let ids = descendants(deck_id, &decks);
    let id_list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let q = |sql: &str| -> Res<i64> {
        con.query_row(&sql.replace("$IDS", &id_list), [], |r| r.get(0)).map_err(err)
    };

    let me = decks.iter().find(|d| d.id == deck_id).ok_or("덱을 찾을 수 없어요.")?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let path = path_of(deck_id, &by_id);

    let total = q("SELECT count(*) FROM card WHERE deck_id IN ($IDS)")?;
    let unseen = q("SELECT count(*) FROM card WHERE deck_id IN ($IDS) AND state = 'new'")?;
    let hard = q("SELECT count(*) FROM card WHERE deck_id IN ($IDS) AND buried_until IS NOT NULL")?;
    let hard_auto = q("SELECT count(*) FROM card WHERE deck_id IN ($IDS) AND buried_until IS NOT NULL AND buried_reason = 'two_again'")?;
    let hard_next: Option<i64> =
        con.query_row(&format!("SELECT min(buried_until) FROM card WHERE deck_id IN ({id_list})"), [], |r| r.get(0)).map_err(err)?;
    let core_total = q("SELECT count(*) FROM card_effective WHERE deck_id IN ($IDS) AND eff_importance = 2")?;
    let core_seen = q("SELECT count(*) FROM card_effective WHERE deck_id IN ($IDS) AND eff_importance = 2 AND state <> 'new'")?;
    let imp = |lvl: i64| q(&format!("SELECT count(*) FROM card_effective WHERE deck_id IN ($IDS) AND eff_importance = {lvl}"));
    let unconfirmed = q("SELECT count(*) FROM deck WHERE id IN ($IDS) AND importance_source IN ('rule','tag','material')")?;

    let today_new = plan.new.len();
    // 상위 덱 목표를 따르는 하위 덱은 그 목표(예산 덱) 기준으로 센다. 사유 줄과 숫자가 같게
    let owner = by_id.get(&queue::budget_owner(deck_id, &by_id)).copied().unwrap_or(me);
    let owner_unseen = if owner.id == deck_id {
        unseen
    } else {
        let ids = descendants(owner.id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        con.query_row(&format!("SELECT count(*) FROM card WHERE deck_id IN ({ids}) AND state = 'new'"), [], |r| r.get(0)).map_err(err)?
    };
    let (per_day, _) = queue::new_target_for_deck(owner, today, owner_unseen);
    let finish_days = (per_day > 0 && owner_unseen > 0).then(|| (owner_unseen + per_day - 1) / per_day);
    let learnable = queue::learnable_days(today, owner.target_day, owner.include_target_day);
    let on_track = learnable.map(|d| finish_days.unwrap_or(0) <= d.max(0) || owner_unseen == 0);
    let studied = q("SELECT count(*) FROM review_log r JOIN card c ON c.id = r.card_id WHERE c.deck_id IN ($IDS)")? > 0;
    let reason = plan.notes.iter().find(|n| n.starts_with(&me.name)).map(|n| n[me.name.len() + 2..].to_string())
        .or_else(|| plan.notes.first().cloned());

    let config: Option<(Option<String>, i64, Option<i64>)> = con
        .query_row(
            "SELECT target_date, include_target_day, daily_new_limit FROM deck_config WHERE deck_id = ?1",
            [deck_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(err)?;
    let (target_date, include_target_day, daily_new_limit) = config.unwrap_or((None, 0, None));
    let write_src = write_mode_source(&con, deck_id)?;

    // C1-w: 덱 평균 (이 덱 전체)
    let (all_reviews, all_again) = again_counts(&con, &id_list, today)?;
    let again_avg = (all_reviews > 0).then(|| all_again as f64 / all_reviews as f64);
    let today_date = db::date_from_day(today);

    // C1-t: 하위 덱 트리. 깊이 우선, 이름 순
    let mut tree: Vec<Subdeck> = vec![];
    let mut stack: Vec<(i64, usize)> = {
        let mut kids: Vec<&queue::Deck> = decks.iter().filter(|d| d.parent_id == Some(deck_id)).collect();
        kids.sort_by(|a, b| b.name.cmp(&a.name));
        kids.iter().map(|d| (d.id, 1)).collect()
    };
    let ordered = plan.ordered();
    while let Some((id, depth)) = stack.pop() {
        let d = by_id[&id];
        let sub = descendants(id, &decks);
        let sub_ids = sub.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let (cards, unseen): (i64, i64) = con
            .query_row(&format!("SELECT count(*), coalesce(sum(state = 'new'), 0) FROM card WHERE deck_id IN ({sub_ids})"), [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(err)?;
        let (importance, importance_source, weak, dismissed, recovery_shown): (i64, String, bool, Option<String>, bool) = con
            .query_row(
                "SELECT importance, importance_source, weak, weak_dismissed_until, weak_recovery_shown FROM deck WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(err)?;
        let (reviews14, again14) = again_counts(&con, &sub_ids, today)?;
        let again_rate = (reviews14 > 0).then(|| again14 as f64 / reviews14 as f64);
        let lapsed: i64 = con
            .query_row(&format!("SELECT count(*) FROM card WHERE deck_id IN ({sub_ids}) AND lapses >= 3"), [], |r| r.get(0))
            .map_err(err)?;
        let high = match (again_rate, again_avg) {
            (Some(r), Some(avg)) => r >= WEAK_ABS_RATE || (avg > 0.0 && r >= avg * WEAK_RATIO_OF_AVG),
            _ => false,
        };
        let weak_suggested = !weak && reviews14 >= WEAK_MIN_REVIEWS && high && dismissed.is_none_or(|u| u < today_date);
        let weak_recovered = weak && !recovery_shown && reviews14 >= WEAK_MIN_REVIEWS && matches!((again_rate, again_avg), (Some(r), Some(a)) if r < a);
        let levels: i64 = con
            .query_row(
                &format!("SELECT count(DISTINCT d.importance) FROM deck d WHERE d.id IN ({sub_ids}) AND EXISTS (SELECT 1 FROM card c WHERE c.deck_id = d.id)"),
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let mut kids: Vec<&queue::Deck> = decks.iter().filter(|k| k.parent_id == Some(id)).collect();
        kids.sort_by(|a, b| b.name.cmp(&a.name));
        tree.push(Subdeck {
            id,
            name: d.name.clone(),
            cards,
            importance,
            importance_source,
            parent_id: d.parent_id.unwrap_or(deck_id),
            depth,
            today: ordered.iter().filter(|c| sub.contains(&c.deck_id)).count(),
            unseen,
            children: kids.len(),
            mixed: levels > 1,
            paused: d.paused,
            reviews14,
            again_rate,
            lapsed,
            weak,
            weak_suggested,
            weak_recovered,
        });
        stack.extend(kids.iter().map(|k| (k.id, depth + 1)));
    }
    let target_from = (me.target_day.is_none())
        .then(|| queue::budget_owner(deck_id, &by_id))
        .filter(|owner| *owner != deck_id)
        .and_then(|owner| by_id.get(&owner))
        .and_then(|o| o.target_day.map(|t| TargetFrom { name: o.name.clone(), dday: t - today }));

    Ok(DeckDetail {
        id: deck_id,
        parent_id: me.parent_id,
        name: me.name.clone(),
        path,
        total,
        unseen,
        hard,
        hard_auto,
        // buried_until 은 ms 다 (큐는 study_day 로 읽는다)
        hard_next: hard_next.map(|ms| db::date_from_day(study_day(ms))),
        backlog: plan.backlog.len() + plan.backlog_deferred,
        today_review: plan.ordered().len() - today_new - plan.returning.len(),
        today_new,
        today_returning: plan.returning.len(),
        reason,
        dday: target_date.as_deref().and_then(db::day_from_date).map(|t| t - today),
        target_date,
        include_target_day: include_target_day == 1,
        daily_new_limit,
        today_new_override: me.today_new,
        seen_pct: if total > 0 { (total - unseen) * 100 / total } else { 0 },
        core_total,
        core_seen,
        core_learned_pct: if core_total > 0 { core_seen * 100 / core_total } else { 0 },
        per_day,
        finish_days,
        learnable_days: learnable,
        on_track,
        subdeck_count: ids.len() - 1,
        studied,
        importance: [imp(2)?, imp(1)?, imp(0)?],
        own_importance: con.query_row("SELECT importance FROM deck WHERE id = ?1", [deck_id], |r| r.get(0)).map_err(err)?,
        unconfirmed,
        subdecks: tree.iter().filter(|s| s.depth == 1).map(Subdeck::shallow).collect(),
        tree,
        target_from,
        again_avg,
        write_mode: write_src.as_ref().is_some_and(|(id, _)| *id == deck_id),
        write_from: write_src.filter(|(id, _)| *id != deck_id).map(|(_, name)| name),
        paused_until: me.paused_until.map(db::date_from_day),
        paused: me.paused,
        done_today: con
            .query_row(
                &format!(
                    "SELECT count(DISTINCT r.card_id) FROM review_log r JOIN card c ON c.id = r.card_id
                     WHERE r.reviewed_at >= ?1 AND c.deck_id IN ({id_list}) AND r.kind IS NULL"
                ),
                [today_start_ms(today)],
                |r| r.get(0),
            )
            .map_err(err)?,
    })
}

// ------------------------------------------------------------------ C3. 카드 추가

#[derive(Deserialize)]
pub struct NewCard {
    pub(crate) deck_id: i64,
    pub(crate) kind: String, // basic | reversed (앞뒤 둘 다) | cloze
    pub(crate) front: String,
    pub(crate) back: String,
    pub(crate) importance: Option<i64>,
}

/// 노트 하나를 만들고 카드를 필요한 만큼 만든다. 만든 카드 수를 돌려준다
pub(crate) fn insert_note(con: &Connection, card: &NewCard, tags: &str, batch: Option<i64>) -> Res<usize> {
    let now = now_ms();
    let (ords, kind): (Vec<u32>, &str) = match card.kind.as_str() {
        "cloze" => (render::cloze_numbers(&card.front).into_iter().map(|n| n - 1).collect(), "cloze"),
        "reversed" => (vec![0, 1], "reversed"),
        _ => (vec![0], "basic"),
    };
    if ords.is_empty() {
        return Err("빈칸을 하나 이상 만들어 주세요.".into());
    }
    let ntid = db::native_type_id(con, kind).map_err(err)?;
    let fields = serde_json::to_string(&[&card.front, &card.back]).map_err(err)?;
    con.execute(
        "INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, import_batch_id) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![ntid, fields, tags, now, batch],
    )
    .map_err(err)?;
    let nid = con.last_insert_rowid();
    for ord in &ords {
        con.execute(
            "INSERT INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, CASE WHEN ?4 IS NULL THEN NULL ELSE 'user' END, ?5, ?5)",
            params![nid, card.deck_id, ord, card.importance, now],
        )
        .map_err(err)?;
    }
    Ok(ords.len())
}

#[tauri::command]
pub fn add_card(state: State<AppState>, card: NewCard) -> Res<usize> {
    // 편집기는 빈 칸에도 &nbsp; 나 <br> 를 남긴다. 화면에 보이는 글자로 판단한다
    if render::is_blank(&card.front) {
        return Err(if card.kind == "cloze" { "본문을 입력해 주세요." } else { "질문을 입력해 주세요." }.into());
    }
    if card.kind != "cloze" && render::is_blank(&card.back) {
        return Err("답변을 입력해 주세요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    insert_note(&con, &card, "", None)
}

// ------------------------------------------------------------------ 카드 편집 (D2/D3 단축키 E)

#[derive(Serialize)]
pub struct NoteEdit {
    note_id: i64,
    /// basic | cloze | image_occlusion | list | choice | other. other 는 가져온 덱의 노트 유형이다. 필드 이름을 그대로 보여준다.
    /// image_occlusion 은 화면이 occlusion_note 로 가리기 편집기(M2)를 연다
    kind: String,
    /// 노트 유형 이름 ("Anki 노트타입 '…'")과 id (디자인 NT13 머리의 [편집])
    note_type_id: i64,
    type_name: String,
    /// 보이는 순서 (8.1 노트 유형: 지운 필드는 빠지고 순서는 field_order)
    names: Vec<String>,
    values: Vec<String>,
    /// names 의 자리 번호 (note.fields 의 위치). update_note 에 그대로 돌려준다
    slots: Vec<usize>,
    /// 필드마다 앞면 템플릿에 쓰이는가 (툴바 있는 편집기로 보여준다)
    front: Vec<bool>,
    /// 8.4 N1 기출 줄
    exam: crate::exam::ExamState,
    /// Q24 목록 · 선택형 전용 편집기를 채운다 (학습 화면과 같은 읽기)
    special: Option<crate::recall::Special>,
}

#[tauri::command]
pub fn note_edit(state: State<AppState>, note_id: i64) -> Res<NoteEdit> {
    let con = state.db.lock().map_err(err)?;
    let (fields, ntid, exam): (String, i64, crate::exam::ExamState) = con
        .query_row("SELECT fields, note_type_id, exam_hits, exam_names FROM note WHERE id = ?1", [note_id], |r| {
            Ok((r.get(0)?, r.get(1)?, crate::exam::ExamState::of(note_id, r.get(2)?, r.get(3)?)))
        })
        .map_err(err)?;
    let nt = notetype::load(&con, ntid)?;
    let stored: Vec<String> = serde_json::from_str(&fields).map_err(err)?;
    let visible = nt.visible();
    let slots: Vec<usize> = visible.iter().map(|(s, _)| *s).collect();
    let names: Vec<String> = visible.into_iter().map(|(_, n)| n).collect();
    let values: Vec<String> = slots.iter().map(|s| stored.get(*s).cloned().unwrap_or_default()).collect();
    // 그림 가리기 · 목록 · 선택형은 자리로 읽는다 (그 편집기들은 자리 순서 그대로 저장한다)
    let kind = if crate::occlusion::is_note(&con, note_id)? && nt.slots.len() >= 4 {
        "image_occlusion"
    } else if crate::recall::is_special(&nt.name, &nt.slots) {
        // Q 목록 · 선택형. 화면은 전용 편집기(Q24)로 연다
        if nt.name.starts_with(crate::recall::LIST) { "list" } else { "choice" }
    } else if nt.cloze() {
        "cloze"
    } else if names.len() == 2 {
        "basic"
    } else {
        "other"
    };
    // 앞면 템플릿이 쓰는 필드 (I44)
    let used: Vec<String> = nt.templates.iter().flat_map(|t| notetype::refs(&t.qfmt)).collect();
    let front = names.iter().map(|n| used.contains(n)).collect();
    let special = crate::recall::special(&nt.name, &nt.slots, &fields);
    Ok(NoteEdit { note_id, kind: kind.into(), note_type_id: nt.id, type_name: nt.name, names, values, slots, front, exam, special })
}

/// 필드를 고친다. values 는 note_edit 의 보이는 순서, slots 는 그 자리 번호 (없으면 지금 보이는 순서). 지운 자리 값은 그대로 둔다.
/// 빈칸 노트면 빈칸 번호에 맞춰 카드를 만들거나 지운다 (지운 카드의 기록은 review_log 에 남는다)
#[tauri::command]
pub fn update_note(state: State<AppState>, note_id: i64, values: Vec<String>, slots: Option<Vec<usize>>) -> Res<()> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let (ntid, stored): (i64, String) =
        tx.query_row("SELECT note_type_id, fields FROM note WHERE id = ?1", [note_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
    let nt = notetype::load(&tx, ntid)?;
    let slots = slots.unwrap_or_else(|| nt.visible().into_iter().map(|(s, _)| s).collect());
    if slots.len() != values.len() || slots.iter().any(|s| *s >= nt.slots.len()) {
        return Err("필드 수가 노트 유형과 달라요.".into());
    }
    let mut full: Vec<String> = serde_json::from_str(&stored).map_err(err)?;
    full.resize(nt.slots.len(), String::new());
    for (s, v) in slots.iter().zip(values) {
        full[*s] = v;
    }
    save_note_fields(&tx, note_id, &full)?;
    tx.commit().map_err(err)
}

/// update_note 의 본문. values 는 자리 순서 (짧으면 빈 값으로 채운다). 그림 가리기 저장(occlusion::save) · 목록 · 선택형 편집도 같은 길로 카드를 맞춘다.
/// 일반 유형은 앞면이 새로 비지 않게 된 템플릿의 카드를 만든다 (Anki 처럼 만들기만)
pub(crate) fn save_note_fields(tx: &Connection, note_id: i64, values: &[String]) -> Res<()> {
    let ntid: i64 = tx.query_row("SELECT note_type_id FROM note WHERE id = ?1", [note_id], |r| r.get(0)).map_err(err)?;
    let nt = notetype::load(tx, ntid)?;
    if values.len() > nt.slots.len() {
        return Err("필드 수가 노트 유형과 달라요.".into());
    }
    let mut values = values.to_vec();
    values.resize(nt.slots.len(), String::new());
    crate::recall::check_fields(&nt.name, &nt.slots, &values)?;
    if nt.visible().first().is_none_or(|(s, _)| render::is_blank(&values[*s])) {
        return Err("첫 칸을 비울 수 없어요.".into());
    }
    let now = now_ms();
    tx.execute(
        "UPDATE note SET fields = ?2, updated_at = ?3 WHERE id = ?1",
        params![note_id, serde_json::to_string(&values).map_err(err)?, now],
    )
    .map_err(err)?;

    if !nt.cloze() {
        notetype::add_missing_cards(tx, &nt, note_id, &values)?;
        return Ok(());
    }
    let ords = notetype::card_ords(&nt, &values);
    if ords.is_empty() {
        return Err("빈칸을 하나 이상 만들어 주세요.".into());
    }
    let existing: Vec<(i64, u32, i64)> = {
        let mut st = tx.prepare("SELECT id, template_ord, deck_id FROM card WHERE note_id = ?1").map_err(err)?;
        let rows = st.query_map([note_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    let deck_id = existing.first().map(|e| e.2).ok_or("카드를 찾을 수 없어요.")?;
    for ord in &ords {
        if !existing.iter().any(|e| e.1 == *ord) {
            tx.execute(
                "INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![note_id, deck_id, ord, now],
            )
            .map_err(err)?;
        }
    }
    for (id, ord, _) in &existing {
        if !ords.contains(ord) {
            tx.execute("DELETE FROM card WHERE id = ?1", [id]).map_err(err)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct CardRow {
    id: i64,
    note_id: i64,
    deck_path: Vec<String>,
    question: String,
    answer: String,
    state: String,
    importance: i64,
    /// 카드에 직접 정한 중요도인가 (C2 "개별")
    own_importance: bool,
    due_at: Option<i64>,
    lapses: i64,
    buried: bool,
    /// 접어둔 카드가 돌아오는 학습일 "2026-10-19" (C2 상태 배지. 덱 상세 hard_next 와 같은 계산)
    buried_until: Option<String>,
    /// 8.4 노트의 기출 (N2 같은 시험 건너뛰기 · 되돌리기)
    exam: crate::exam::ExamState,
    /// M4 그림 가리기 카드 (축소 그림 · 편집기 칸 선택)
    occlusion: Option<IoPos>,
    /// AU6 소리가 있는 칸 [질문, 답]. 목록은 칩 대신 스피커 아이콘만
    sounds: [bool; 2],
    /// 앞면이 빈 카드 (학습에 나오지 않는다, NT49). C2 상태 배지 "앞면 빔"
    front_empty: bool,
}

/// 그림 가리기 카드의 자리 (M3 배지 "그림 가리기 · n/N", M4 행)
#[derive(Serialize)]
pub struct IoPos {
    /// 칸 번호 cN (편집기 focus)
    ordinal: u32,
    /// 노트 카드 중 몇 번째 (1부터)
    n: u32,
    /// 노트의 카드 수
    count: u32,
}

/// IO 템플릿 카드면 자리. 별칭 c(card) 의 열 (template_ord, 같은 노트 카드 수, 이 카드까지 수)
const IO_POS: &str = "(SELECT count(*) FROM card x WHERE x.note_id = c.note_id), (SELECT count(*) FROM card x WHERE x.note_id = c.note_id AND x.template_ord <= c.template_ord)";

fn io_pos(qfmt: Option<&str>, ord: u32, count: u32, n: u32) -> Option<IoPos> {
    qfmt.is_some_and(crate::occlusion::is_template).then_some(IoPos { ordinal: ord + 1, n, count })
}

#[derive(Serialize)]
pub struct CardList {
    rows: Vec<CardRow>,
    /// limit 을 적용하기 전 일치하는 카드 수
    total: i64,
}

/// 뒷면에서 답 부분만. Basic 템플릿은 <hr id=answer> 뒤가 답이다
pub(crate) fn answer_part(back: &str) -> &str {
    let lower = back.to_ascii_lowercase();
    match lower.find("id=answer").or_else(|| lower.find("id=\"answer\"")) {
        Some(i) => back[i..].find('>').map_or(back, |j| &back[i + j + 1..]),
        None => back,
    }
}

/// 8.4 표 카드(질문 아래 표): 목록 한 줄은 표 앞 질문만, 빈칸 답은 가린 칸 글자만 (표 글자가 질문 · 답에 이어 붙지 않게).
/// 표가 없거나 표 앞에 글이 없으면 None (원래대로)
fn table_brief(front: &str, back: &str) -> Option<(String, String)> {
    let at = front.to_ascii_lowercase().find("<table")?;
    let question = render::strip_tags(&front[..at]);
    if question.is_empty() {
        return None;
    }
    let answer = back.split("<span class=\"cloze\">").skip(1).filter_map(|s| s.split("</span>").next()).map(render::strip_tags).collect::<Vec<_>>().join(" · ");
    let answer = if answer.is_empty() { render::strip_tags(answer_part(back)) } else { answer };
    Some((question, answer))
}

/// C2 검색 대상: 필드 원문 (JSON 의 \" 만 " 로 푼다)
const SEARCH_TEXT: &str = "replace(n.fields, '\\\"', '\"')";

/// AU6 검색에서 소리 파일 이름은 뺀다. 원문 LIKE 로 걸린 노트 중 [sound:] 가 있는 것만 소리를 지우고 다시 본다.
/// 걸린 곳이 소리 이름뿐인 노트 id. LIKE 처럼 ASCII 만 대소문자를 가리지 않는다. like 는 이스케이프한 query.
/// ponytail: 결과를 NOT IN 목록으로 SQL 에 넣는다. "mp3" 처럼 소리 노트 전부가 걸리면 목록이 길어진다 (실측 DECISIONS AU19)
fn sound_only_hits(con: &Connection, query: &str, like: &str) -> Res<Vec<i64>> {
    let q = query.to_ascii_lowercase();
    let mut st = con
        .prepare(&format!("SELECT n.id, {SEARCH_TEXT} FROM note n WHERE instr(n.fields, '[sound:') > 0 AND {SEARCH_TEXT} LIKE '%' || ?1 || '%' ESCAPE '\\'"))
        .map_err(err)?;
    let rows = st.query_map([like], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(err)?;
    let mut out = vec![];
    for row in rows {
        let (id, text) = row.map_err(err)?;
        if !render::strip_sound(&text).to_ascii_lowercase().contains(&q) {
            out.push(id);
        }
    }
    Ok(out)
}

/// C2. query 는 필드 원문에서 찾는다 (HTML 포함, 소리 파일 이름은 빼고). buried_only 는 A2-b "접어둔 카드 보기"
#[tauri::command]
pub fn list_cards(
    state: State<AppState>,
    deck_id: Option<i64>,
    query: String,
    limit: Option<i64>,
    buried_only: Option<bool>,
    card_ids: Option<Vec<i64>>,
    weak_only: Option<bool>,
) -> Res<CardList> {
    let con = state.db.lock().map_err(err)?;
    let decks = load_decks(&con)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let ids = match deck_id {
        Some(id) => descendants(id, &decks),
        None => decks.iter().map(|d| d.id).collect(),
    };
    // C1-w [약한 챕터 카드 보기]: 약함으로 확정한 하위 덱 아래만
    let ids: Vec<i64> = if weak_only.unwrap_or(false) {
        let weak: std::collections::HashSet<i64> =
            decks.iter().filter(|d| d.weak).flat_map(|d| descendants(d.id, &decks)).collect();
        ids.into_iter().filter(|i| weak.contains(i)).collect()
    } else {
        ids
    };
    let ids = if ids.is_empty() { "-1".to_string() } else { ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",") };
    // LIKE 의 와일드카드를 글자 그대로 찾게 한다
    let q = query.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let q = q.as_str();
    let sound_only = if q.is_empty() { vec![] } else { sound_only_hits(&con, query.trim(), q)? };
    let filter = format!(
        "c.deck_id IN ({ids}) AND (?1 = '' OR {SEARCH_TEXT} LIKE '%' || ?1 || '%' ESCAPE '\\') {} {} {}",
        if buried_only.unwrap_or(false) { "AND c.buried_until IS NOT NULL" } else { "" },
        // C1-xr [카드 보기]: 이 카드들만
        card_ids.map_or(String::new(), |c| format!("AND c.id IN ({})", if c.is_empty() { "-1".into() } else { id_list(&c) })),
        if sound_only.is_empty() { String::new() } else { format!("AND n.id NOT IN ({})", id_list(&sound_only)) }
    );
    let total: i64 = con
        .query_row(&format!("SELECT count(*) FROM card c JOIN note n ON n.id = c.note_id WHERE {filter}"), [q], |r| r.get(0))
        .map_err(err)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.note_id, c.deck_id, n.fields, c.state, c.eff_importance, c.importance IS NOT NULL, c.due_at, c.lapses,
                    c.buried_until, c.template_ord, nt.id, t.qfmt, t.afmt, n.exam_hits, n.exam_names, {IO_POS}
             FROM card_effective c JOIN note n ON n.id = c.note_id
             JOIN note_type nt ON nt.id = n.note_type_id
             LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {TEMPLATE_OF_CARD}
             WHERE {filter}
             ORDER BY c.id LIMIT ?2"
        ))
        .map_err(err)?;
    let field_names = note_field_names(&con)?;
    let fronts = crate::notetype::Fronts::load(&con)?;
    let rows = st
        .query_map(params![q, limit.unwrap_or(100)], |r| {
            let fields: String = r.get(3)?;
            let nt: i64 = r.get(11)?;
            let qfmt: Option<String> = r.get(12)?;
            let afmt: Option<String> = r.get(13)?;
            let rendered = render_fields(&field_names, nt, &fields, qfmt.as_deref(), afmt.as_deref(), r.get(10)?);
            let occlusion = io_pos(qfmt.as_deref(), r.get(10)?, r.get(16)?, r.get(17)?);
            // 그림 가리기 답은 칸 이름(질문 칸에 보인다)이라 답변 칸에는 덧붙임(Back Extra, 4번째 필드)만
            let answer = match occlusion {
                Some(_) => serde_json::from_str::<Vec<String>>(&fields).ok().and_then(|v| v.get(3).map(|x| render::strip_tags(x))).unwrap_or_default(),
                None => render::strip_tags(answer_part(&rendered.back)),
            };
            let (question, answer) = table_brief(&rendered.front, &rendered.back).unwrap_or((render::strip_tags(&rendered.front), answer));
            let buried_until: Option<i64> = r.get(9)?;
            Ok(CardRow {
                id: r.get(0)?,
                note_id: r.get(1)?,
                deck_path: path_of(r.get(2)?, &by_id),
                question,
                answer,
                state: r.get(4)?,
                importance: r.get(5)?,
                own_importance: r.get(6)?,
                due_at: r.get(7)?,
                lapses: r.get(8)?,
                buried: buried_until.is_some(),
                buried_until: buried_until.map(|ms| db::date_from_day(study_day(ms))),
                exam: crate::exam::ExamState::of(r.get(1)?, r.get(14)?, r.get(15)?),
                occlusion,
                sounds: [rendered.front.as_str(), answer_part(&rendered.back)].map(|h| h.contains(render::SOUND_MARK)),
                front_empty: fronts.empty(nt, r.get(10)?, &fields),
            })
        })
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    Ok(CardList { rows, total })
}

/// 카드 하나의 중요도. None 이면 덱 설정을 따른다
#[tauri::command]
pub fn set_cards_importance(state: State<AppState>, card_ids: Vec<i64>, level: Option<i64>) -> Res<()> {
    if card_ids.is_empty() {
        return Ok(());
    }
    if level.is_some_and(|l| !(0..=2).contains(&l)) {
        return Err("중요도 값이 올바르지 않아요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    // 앞뒤 카드처럼 빈칸이 아닌 노트의 형제 카드는 중요도가 같이 간다 (C3-r)
    let ids = id_list(&card_ids);
    con.execute(
        &format!(
            "UPDATE card SET importance = ?1, importance_source = CASE WHEN ?1 IS NULL THEN NULL ELSE 'user' END, updated_at = ?2
             WHERE id IN ({ids}) OR note_id IN (
               SELECT c.note_id FROM card c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id
               WHERE c.id IN ({ids}) AND NOT {IS_CLOZE})"
        ),
        params![level, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// C1-w [약함으로] / [해제]. 다시 확정하면 "요즘 괜찮아요" 안내도 다시 한 번 띄울 수 있다
#[tauri::command]
pub fn set_deck_weak(state: State<AppState>, deck_id: i64, weak: bool) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    con.execute("UPDATE deck SET weak = ?2, weak_recovery_shown = 0, updated_at = ?3 WHERE id = ?1", params![deck_id, weak, now_ms()])
        .map_err(err)?;
    Ok(())
}

/// C1-w [아니에요]: 14일 동안 다시 제안하지 않는다. recovered 면 "요즘 괜찮아요" 안내를 닫는다 (한 번만)
#[tauri::command]
pub fn dismiss_weak(state: State<AppState>, deck_id: i64, recovered: bool) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    if recovered {
        con.execute("UPDATE deck SET weak_recovery_shown = 1 WHERE id = ?1", [deck_id]).map_err(err)?;
    } else {
        let until = db::date_from_day(study_day(now_ms()) + WEAK_DAYS);
        con.execute("UPDATE deck SET weak_dismissed_until = ?2 WHERE id = ?1", params![deck_id, until]).map_err(err)?;
    }
    Ok(())
}

/// "1,2,3". 숫자만 들어가므로 SQL 에 바로 넣어도 된다
fn id_list(ids: &[i64]) -> String {
    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

/// 카드를 다른 덱으로 옮긴다 (C3-e 하위 덱 이동). 일정과 기록은 그대로다
#[tauri::command]
pub fn move_cards(state: State<AppState>, card_ids: Vec<i64>, deck_id: i64) -> Res<()> {
    if card_ids.is_empty() {
        return Ok(());
    }
    let con = state.db.lock().map_err(err)?;
    let ok: bool = con
        .query_row("SELECT count(*) FROM deck WHERE id = ?1 AND archived_at IS NULL", [deck_id], |r| r.get::<_, i64>(0))
        .map_err(err)?
        > 0;
    if !ok {
        return Err("옮길 덱을 찾을 수 없어요.".into());
    }
    con.execute(&format!("UPDATE card SET deck_id = ?1, updated_at = ?2 WHERE id IN ({})", id_list(&card_ids)), params![deck_id, now_ms()])
        .map_err(err)?;
    Ok(())
}

/// C2-r · C2-b 접어두기 (학습 밖). 다음 학습에서 빼고 3일 뒤 다시 올린다
#[tauri::command]
pub fn bury_cards(state: State<AppState>, card_ids: Vec<i64>) -> Res<()> {
    if card_ids.is_empty() {
        return Ok(());
    }
    let con = state.db.lock().map_err(err)?;
    let now = now_ms();
    let until = study_day_end_ms(queue::bury_until(study_day(now)) - 1) + 1;
    con.execute(
        &format!("UPDATE card SET buried_until = ?1, buried_reason = 'user', updated_at = ?2 WHERE id IN ({})", id_list(&card_ids)),
        params![until, now],
    )
    .map_err(err)?;
    Ok(())
}

/// 접어두기 되돌리기 (C2 토스트, C2 "접어둔 카드"에서 꺼내기)
#[tauri::command]
pub fn unbury_cards(state: State<AppState>, card_ids: Vec<i64>) -> Res<()> {
    if card_ids.is_empty() {
        return Ok(());
    }
    let con = state.db.lock().map_err(err)?;
    con.execute(&format!("UPDATE card SET buried_until = NULL, buried_reason = NULL WHERE id IN ({})", id_list(&card_ids)), [])
        .map_err(err)?;
    Ok(())
}

/// 카드를 지운다. 노트에 남은 카드가 없으면 노트도 지운다. 복습 기록은 남는다 (review_log.card_id 가 NULL 이 된다)
#[tauri::command]
pub fn delete_cards(state: State<AppState>, card_ids: Vec<i64>) -> Res<()> {
    if card_ids.is_empty() {
        return Ok(());
    }
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let ids = id_list(&card_ids);
    let notes: Vec<i64> = {
        let mut st = tx.prepare(&format!("SELECT DISTINCT note_id FROM card WHERE id IN ({ids})")).map_err(err)?;
        st.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    tx.execute(&format!("DELETE FROM card WHERE id IN ({ids})"), []).map_err(err)?;
    tx.execute(
        &format!("DELETE FROM note WHERE id IN ({}) AND NOT EXISTS (SELECT 1 FROM card WHERE note_id = note.id)", id_list(&notes)),
        [],
    )
    .map_err(err)?;
    tx.commit().map_err(err)
}

/// 노트 유형 id -> (이름, 자리별 필드 이름). 지운 자리는 "" (notetype.rs). 전에는 이름 키라 같은 이름의 가져온 유형과 앱 유형이 서로 덮였다
pub(crate) fn note_field_names(con: &Connection) -> Res<HashMap<i64, (String, Vec<String>)>> {
    let mut st = con.prepare("SELECT id, name, fields FROM note_type").map_err(err)?;
    let rows = st
        .query_map([], |r| {
            let fields: String = r.get(2)?;
            Ok((r.get::<_, i64>(0)?, (r.get::<_, String>(1)?, serde_json::from_str::<Vec<String>>(&fields).unwrap_or_default())))
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

pub(crate) fn render_fields(
    names: &HashMap<i64, (String, Vec<String>)>,
    note_type: i64,
    fields_json: &str,
    qfmt: Option<&str>,
    afmt: Option<&str>,
    ord: u32,
) -> render::Rendered {
    let values: Vec<String> = serde_json::from_str(fields_json).unwrap_or_default();
    let map: HashMap<String, String> = names
        .get(&note_type)
        .map(|(_, n)| n.iter().cloned().zip(values.iter().cloned()).filter(|(k, _)| !k.is_empty()).collect())
        .unwrap_or_default();
    match (qfmt, afmt) {
        (Some(q), Some(a)) => render::render_card(q, a, &map, ord),
        _ => render::Rendered { front: values.first().cloned().unwrap_or_default(), back: values.get(1).cloned().unwrap_or_default() },
    }
}

// ------------------------------------------------------------------ C4. 목표와 학습 설정

#[derive(Deserialize)]
pub struct DeckConfigInput {
    deck_id: i64,
    target_date: Option<String>,
    include_target_day: bool,
    daily_new_limit: Option<i64>,
    /// 오늘만 신규 분량. None 이면 계산값을 쓴다
    today_new: Option<i64>,
    /// 잠시 쉬기 (C4-p). None 이면 지금 상태를 그대로 둔다
    #[serde(default)]
    paused: Option<bool>,
    #[serde(default)]
    paused_until: Option<String>,
    /// J1 "이 덱은 써서 답하기". None 이면 지금 상태를 그대로 둔다
    #[serde(default)]
    write_mode: Option<bool>,
}

/// J1 · C4: 이 덱이나 가장 가까운 위 덱 중 "써서 답하기"를 켠 덱 (id, 이름). 없으면 템플릿대로
pub(crate) fn write_mode_source(con: &Connection, deck_id: i64) -> Res<Option<(i64, String)>> {
    con.query_row(
        "WITH RECURSIVE up(id, depth) AS (
             SELECT ?1, 0
             UNION ALL SELECT d.parent_id, up.depth + 1 FROM deck d JOIN up ON d.id = up.id WHERE d.parent_id IS NOT NULL AND up.depth < 64)
         SELECT d.id, d.name FROM up JOIN deck_config c ON c.deck_id = up.id JOIN deck d ON d.id = up.id
         WHERE c.write_mode = 1 ORDER BY up.depth LIMIT 1",
        [deck_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(err)
}

#[tauri::command]
pub fn save_deck_config(state: State<AppState>, config: DeckConfigInput) -> Res<()> {
    if let Some(d) = &config.target_date {
        db::day_from_date(d).ok_or("날짜 형식이 올바르지 않아요.")?;
    }
    if config.daily_new_limit.is_some_and(|n| n < 0) || config.today_new.is_some_and(|n| n < 0) {
        return Err("신규 카드 수는 0 이상이어야 해요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    if let Some(u) = &config.paused_until {
        db::day_from_date(u).ok_or("날짜 형식이 올바르지 않아요.")?;
    }
    let today = config.today_new.map(|_| db::date_from_day(study_day(now_ms())));
    con.execute(
        "INSERT INTO deck_config (deck_id, target_date, include_target_day, daily_new_limit, today_new, today_new_day, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(deck_id) DO UPDATE SET target_date = ?2, include_target_day = ?3, daily_new_limit = ?4,
                                            today_new = ?5, today_new_day = ?6, updated_at = ?7",
        params![config.deck_id, config.target_date, config.include_target_day as i64, config.daily_new_limit, config.today_new, today, now_ms()],
    )
    .map_err(err)?;
    if let Some(p) = config.paused {
        con.execute(
            "UPDATE deck_config SET paused = ?2, paused_until = ?3 WHERE deck_id = ?1",
            params![config.deck_id, p as i64, if p { config.paused_until.clone() } else { None }],
        )
        .map_err(err)?;
    }
    if let Some(w) = config.write_mode {
        con.execute("UPDATE deck_config SET write_mode = ?2 WHERE deck_id = ?1", params![config.deck_id, w.then_some(1)]).map_err(err)?;
    }
    Ok(())
}

/// 설정을 저장하기 전에 결과를 미리 본다 (C4-b 상한 부족 경고)
#[derive(Serialize)]
pub struct ConfigPreview {
    remaining_new: i64,
    days: Option<i64>,
    /// 하루 신규 권장 (상한 적용, 오늘만 조정은 빼고)
    per_day: i64,
    needed_per_day: Option<i64>,
    overflow_days: Option<i64>,
    finish_date: Option<String>,
    /// 하루 0장이라 끝나지 않는다
    never_finish: bool,
    /// 이 설정으로 오늘 이 덱에서 볼 카드 수 (복습 + 신규)
    today_total: usize,
    /// 쉬는 동안 예정된 복습 수. 다시 시작하는 날 밀린 복습으로 온다 (C4-p). 기한 없는 쉬기면 null
    paused_reviews: Option<i64>,
    /// 다시 시작하는 날 'YYYY-MM-DD'
    resume_date: Option<String>,
    /// 그중 신규
    today_new: usize,
}

#[tauri::command]
pub fn preview_deck_config(state: State<AppState>, config: DeckConfigInput) -> Res<ConfigPreview> {
    let con = state.db.lock().map_err(err)?;
    let decks = load_decks(&con)?;
    let ids = descendants(config.deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let remaining_new: i64 = con
        .query_row(&format!("SELECT count(*) FROM card WHERE deck_id IN ({ids}) AND state = 'new'"), [], |r| r.get(0))
        .map_err(err)?;
    let today = study_day(now_ms());
    let target = config.target_date.as_deref().and_then(db::day_from_date);
    let current = decks.iter().find(|d| d.id == config.deck_id);
    let paused = config.paused.unwrap_or(current.is_some_and(|d| d.paused));
    let paused_until = match config.paused {
        Some(_) => config.paused_until.as_deref().and_then(db::day_from_date),
        None => current.and_then(|d| d.paused_until),
    };
    let deck = queue::Deck {
        target_day: target,
        include_target_day: config.include_target_day,
        daily_new_limit: config.daily_new_limit,
        paused,
        paused_until,
        ..Default::default()
    };
    let resume = queue::resume_day(&deck, today);
    let days = queue::learnable_days(resume, target, config.include_target_day);
    let (per_day, _) = queue::new_target_for_deck(&deck, today, remaining_new);

    // 저장했다고 치고 오늘 계획을 다시 짠다
    let mut decks = decks;
    if let Some(d) = decks.iter_mut().find(|d| d.id == config.deck_id) {
        d.target_day = target;
        d.include_target_day = config.include_target_day;
        d.daily_new_limit = config.daily_new_limit;
        d.today_new = config.today_new;
        d.paused = paused;
        d.paused_until = paused_until;
    }
    let paused_reviews = match (paused, paused_until) {
        (true, Some(u)) => Some(
            con.query_row(
                &format!("SELECT count(*) FROM card WHERE deck_id IN ({ids}) AND state IN ('review','relearning') AND due_at <= ?1"),
                [study_day_end_ms(u)],
                |r| r.get(0),
            )
            .map_err(err)?,
        ),
        _ => None,
    };
    let plan = queue::build_queue_with_progress(
        today,
        &decks,
        &load_cards(&con)?,
        Some(&[config.deck_id]),
        1.0,
        &today_progress(&con, today)?,
    );
    let needed = days.filter(|d| *d > 0).map(|d| (remaining_new + d - 1) / d);
    let finish_days = if per_day > 0 { Some((remaining_new + per_day - 1) / per_day) } else { None };
    let overflow = match (finish_days, days) {
        (Some(f), Some(d)) if f > d => Some(f - d),
        _ => None,
    };
    Ok(ConfigPreview {
        remaining_new,
        days,
        per_day,
        needed_per_day: needed,
        overflow_days: overflow,
        finish_date: finish_days.map(|f| db::date_from_day(resume + f - 1)),
        paused_reviews,
        resume_date: paused.then(|| paused_until.map(|u| db::date_from_day(u + 1))).flatten(),
        never_finish: per_day == 0 && remaining_new > 0,
        // 쉬면 오늘의 학습에서 빠진다 (C4-p). 덱을 직접 골라 학습할 때만 복습이 나온다 (I84)
        today_total: if paused { 0 } else { plan.ordered().len() },
        today_new: plan.new.len(),
    })
}

// ------------------------------------------------------------------ B4. 중요도

#[tauri::command]
pub fn set_deck_importance(state: State<AppState>, deck_id: i64, level: i64) -> Res<()> {
    if !(0..=2).contains(&level) {
        return Err("중요도 값이 올바르지 않아요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "UPDATE deck SET importance = ?2, importance_source = 'user', importance_reason = NULL, updated_at = ?3 WHERE id = ?1",
        params![deck_id, level, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// 제안된 값을 그대로 확정한다
#[tauri::command]
pub fn confirm_importance(state: State<AppState>, deck_ids: Vec<i64>) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    for id in deck_ids {
        con.execute("UPDATE deck SET importance_source = 'user' WHERE id = ?1", [id]).map_err(err)?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct BoardDeck {
    id: i64,
    /// 이 덱 아래 경로. "2과목 · 핵심 개념"
    label: String,
    /// 같은 경로를 단계별로. 첫 단계가 과목이다 (B4-g 과목별 묶음). 덱 자신이면 빈 배열
    path: Vec<String>,
    cards: i64,
    importance: i64,
    source: String,
    reason: Option<String>,
}

/// B4. 카드가 직접 들어 있는 덱들. 카드는 자기 덱의 중요도를 상속하므로 중요도는 이 단위로 정한다
#[tauri::command]
pub fn importance_board(state: State<AppState>, deck_id: i64) -> Res<Vec<BoardDeck>> {
    let con = state.db.lock().map_err(err)?;
    // N3 기출 근거는 카드 이동 · 자료 연결 · 기출 표시로 바뀐다. 볼 때 맞춘다 (바뀐 덱만 고친다)
    crate::exam::suggest_all(&con)?;
    let decks = load_decks(&con)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let base = path_of(deck_id, &by_id);
    let ids = descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut st = con
        .prepare(&format!(
            "SELECT d.id, count(c.id), d.importance, d.importance_source, d.importance_reason
             FROM deck d JOIN card c ON c.deck_id = d.id WHERE d.id IN ({ids}) GROUP BY d.id ORDER BY d.id"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            let full = path_of(id, &by_id);
            let rel = full.get(base.len()..).unwrap_or_default();
            Ok(BoardDeck {
                id,
                label: if rel.is_empty() { base.last().cloned().unwrap_or_default() } else { rel.join(" · ") },
                path: rel.to_vec(),
                cards: r.get(1)?,
                importance: r.get(2)?,
                source: r.get(3)?,
                reason: r.get(4)?,
            })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

// ------------------------------------------------------------------ D1. 학습 시작 전 요약

#[derive(Serialize)]
pub struct QueueSummary {
    total: usize,
    learning: usize,
    returning: usize,
    review: usize,
    backlog: usize,
    backlog_deferred: usize,
    new_core: usize,
    new_normal: usize,
    new_supp: usize,
    core_total: usize,
    /// 복습 묶음 중 약한 챕터라 먼저 보여주는 장수 (D1 "약한 챕터 먼저 · N장")
    weak_first: usize,
    /// 오늘 카드가 있는 최상위 덱 수
    deck_count: usize,
    /// 오늘 신규를 내놓는 예산 덱 수
    new_decks: usize,
    notes: Vec<String>,
    target: Option<TargetInfo>,
    /// 오늘 시험 전 회독(혼합)에서 일정에 반영한 카드 수. D1 "시험 전 회독에서 N장을 이미 봤어요" (P9)
    mixed_today: i64,
}

/// 목표일이 가장 가까운 덱. D1 "시험이 12일 남아 핵심 신규를 먼저" 문구에 쓴다
#[derive(Serialize)]
pub struct TargetInfo {
    name: String,
    dday: i64,
    remaining_new: i64,
    days: i64,
}

#[tauri::command]
pub fn queue_preview(state: State<AppState>, deck_ids: Option<Vec<i64>>, scale: f64) -> Res<QueueSummary> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    let (decks, plan) = plan_for(&con, deck_ids.as_deref(), scale, today)?;
    let n = |lvl: i64| plan.new.iter().filter(|c| c.importance == lvl).count();
    let wanted: Vec<i64> = match &deck_ids {
        Some(ids) => ids.iter().flat_map(|i| descendants(*i, &decks)).collect(),
        None => decks.iter().map(|d| d.id).collect(),
    };
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let target = match decks
        .iter()
        .filter(|d| wanted.contains(&d.id))
        .filter_map(|d| d.target_day.map(|t| (d, t - today)))
        .filter(|(_, dd)| *dd >= 0)
        .min_by_key(|(_, dd)| *dd)
    {
        Some((d, dday)) => {
            let ids = descendants(d.id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            let remaining_new: i64 = con
                .query_row(&format!("SELECT count(*) FROM card WHERE state = 'new' AND deck_id IN ({ids})"), [], |r| r.get(0))
                .map_err(err)?;
            let days = queue::learnable_days(today, d.target_day, d.include_target_day).unwrap_or(1).max(1);
            Some(TargetInfo { name: d.name.clone(), dday, remaining_new, days })
        }
        None => None,
    };
    let ordered = plan.ordered();
    let roots: std::collections::HashSet<i64> = ordered.iter().map(|c| root_of(c.deck_id, &by_id)).collect();
    let mixed_today: i64 = con
        .query_row(
            &format!(
                "SELECT count(DISTINCT r.card_id) FROM review_log r JOIN study_session s ON s.id = r.session_id JOIN card c ON c.id = r.card_id
                 WHERE s.mode = 'mix' AND r.fed_to_scheduler = 1 AND r.reviewed_at >= ?1 AND c.deck_id IN ({})",
                if wanted.is_empty() { "-1".into() } else { id_list(&wanted) }
            ),
            [today_start_ms(today)],
            |r| r.get(0),
        )
        .map_err(err)?;
    Ok(QueueSummary {
        total: ordered.len(),
        learning: plan.learning.len(),
        returning: plan.returning.len(),
        review: plan.review.len(),
        backlog: plan.backlog.len(),
        backlog_deferred: plan.backlog_deferred,
        new_core: n(2),
        new_normal: n(1),
        new_supp: n(0),
        core_total: ordered.iter().filter(|c| c.importance == 2).count(),
        weak_first: plan.review.iter().chain(&plan.backlog).filter(|c| c.weak).count(),
        deck_count: roots.len(),
        new_decks: plan.per_deck_new_target.values().filter(|n| **n > 0).count(),
        notes: plan.notes.clone(),
        target,
        mixed_today,
    })
}

// ------------------------------------------------------------------ D2~D5. 학습 세션

#[derive(Serialize)]
pub struct SessionStart {
    session_id: i64,
    card_ids: Vec<i64>,
}

#[tauri::command]
pub fn start_session(state: State<AppState>, deck_ids: Option<Vec<i64>>, scale: f64) -> Res<SessionStart> {
    let con = state.db.lock().map_err(err)?;
    let now = now_ms();
    let today = study_day(now);
    let (_, plan) = plan_for(&con, deck_ids.as_deref(), scale, today)?;
    // 같은 학습일에 중간에 그만둔 세션이 있으면 이어 쓴다. Again 횟수와 접어둔 카드가 이어진다 (큐 명세 6.3).
    // 끝까지 마친 세션은 이어 쓰지 않는다
    let day = db::date_from_day(today);
    let resumed: Option<i64> = con
        .query_row(
            "SELECT id FROM study_session WHERE study_day = ?1 AND end_reason IN ('user_exit','app_exit') AND mode IS NULL ORDER BY started_at DESC LIMIT 1",
            [&day],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    let session_id = match resumed {
        Some(id) => {
            con.execute("UPDATE study_session SET ended_at = NULL, end_reason = NULL WHERE id = ?1", [id]).map_err(err)?;
            id
        }
        None => {
            con.execute("INSERT INTO study_session (started_at, study_day) VALUES (?1, ?2)", params![now, day]).map_err(err)?;
            con.last_insert_rowid()
        }
    };
    Ok(SessionStart { session_id, card_ids: plan.ordered().iter().map(|c| c.id).collect() })
}

#[derive(Serialize)]
pub struct CardView {
    id: i64,
    note_id: i64,
    /// 노트타입 CSS. 화면이 카드 영역 안으로 가둬서 쓴다
    css: String,
    front: String,
    back: String,
    deck_path: Vec<String>,
    importance: i64,
    state: String,
    /// M3 그림 가리기 배지
    occlusion: Option<IoPos>,
    /// Q 목록 회상 · 선택형. 있으면 학습 화면이 템플릿 대신 이것으로 그린다
    special: Option<crate::recall::Special>,
}

#[tauri::command]
pub fn card_view(state: State<AppState>, card_id: i64) -> Res<CardView> {
    let con = state.db.lock().map_err(err)?;
    let names = note_field_names(&con)?;
    let decks = load_decks(&con)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    con.query_row(
        &format!(
            "SELECT c.id, n.fields, nt.id, t.qfmt, t.afmt, c.template_ord, c.deck_id, c.eff_importance, c.state, n.id, coalesce(nt.css, ''), {IO_POS}
         FROM card_effective c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id
         LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {TEMPLATE_OF_CARD}
         WHERE c.id = ?1"
        ),
        [card_id],
        |r| {
            let fields: String = r.get(1)?;
            let nt: i64 = r.get(2)?;
            let q: Option<String> = r.get(3)?;
            let a: Option<String> = r.get(4)?;
            let rendered = render_fields(&names, nt, &fields, q.as_deref(), a.as_deref(), r.get(5)?);
            let special = names.get(&nt).and_then(|(name, n)| crate::recall::special(name, n, &fields));
            Ok(CardView {
                id: r.get(0)?,
                note_id: r.get(9)?,
                css: r.get(10)?,
                front: rendered.front,
                back: rendered.back,
                deck_path: path_of(r.get(6)?, &by_id),
                importance: r.get(7)?,
                state: r.get(8)?,
                occlusion: io_pos(q.as_deref(), r.get(5)?, r.get(11)?, r.get(12)?),
                special,
            })
        },
    )
    .map_err(err)
}

#[derive(Serialize)]
pub struct AnswerResult {
    /// next: 다음 카드로 | again_later: 이 세션 안에서 다시 나온다 | buried: 접어뒀다
    action: String,
    due_in_ms: Option<i64>,
}

pub(crate) fn load_srs(con: &Connection, card_id: i64) -> Res<SrsState> {
    con.query_row(
        "SELECT state, srs_step, stability, difficulty, due_at, interval_days, last_reviewed_at, reps, lapses FROM card WHERE id = ?1",
        [card_id],
        |r| {
            Ok(SrsState {
                state: r.get(0)?,
                step: r.get(1)?,
                stability: r.get(2)?,
                difficulty: r.get(3)?,
                due_at: r.get(4)?,
                interval_days: r.get(5)?,
                last_reviewed_at: r.get(6)?,
                reps: r.get(7)?,
                lapses: r.get(8)?,
            })
        },
    )
    .map_err(err)
}

#[tauri::command]
pub fn answer(state: State<AppState>, session_id: i64, card_id: i64, rating: u8, elapsed_ms: i64) -> Res<AnswerResult> {
    let mut con = state.db.lock().map_err(err)?;
    let (result, undo) = answer_at(&mut con, &state.fsrs, session_id, card_id, rating, elapsed_ms, now_ms())?;
    drop(con);
    if let Some(u) = undo {
        push_undo(&state, u)?;
    }
    Ok(result)
}

/// answer 의 본체 (테스트가 시각을 정해 부른다). 되돌리기 기록이 없으면 평가를 남기지 않은 것이다
fn answer_at(con: &mut Connection, fsrs: &fsrs::FSRS, session_id: i64, card_id: i64, rating: u8, elapsed_ms: i64, now: i64) -> Res<(AnswerResult, Option<Undo>)> {
    if !(1..=4).contains(&rating) {
        return Err("평가 값이 올바르지 않아요.".into());
    }
    let today = study_day(now);
    // 큐 명세 8장: 학습 중 하루 경계를 넘었다. 평가는 남기지 않고 세션을 닫는다. 화면이 새 날의 학습을 다시 연다 (I83)
    let session_day: Option<String> =
        con.query_row("SELECT study_day FROM study_session WHERE id = ?1", [session_id], |r| r.get(0)).optional().map_err(err)?;
    if session_day.is_some_and(|d| d != db::date_from_day(today)) {
        con.execute("UPDATE study_session SET ended_at = ?2, end_reason = 'day_rollover' WHERE id = ?1 AND ended_at IS NULL", params![session_id, now])
            .map_err(err)?;
        return Ok((AnswerResult { action: "day_rollover".into(), due_in_ms: None }, None));
    }
    let tx = con.transaction().map_err(err)?;
    let mut undo = snapshot(&tx, session_id, card_id)?;
    let prev = load_srs(&tx, card_id)?;
    tx.execute(
        "INSERT OR IGNORE INTO session_card_state (session_id, card_id) VALUES (?1, ?2)",
        params![session_id, card_id],
    )
    .map_err(err)?;
    let again_count: i64 = tx
        .query_row("SELECT again_count FROM session_card_state WHERE session_id = ?1 AND card_id = ?2", params![session_id, card_id], |r| r.get(0))
        .map_err(err)?;
    let (action, feed) = queue::on_rating(again_count, rating);
    // P9 혼합 회독: 그 카드의 오늘 첫 일정 반영 평가(또는 학습 단계를 잇는 평가)만 일정에 반영한다
    let feed = feed && crate::cram::mix_feeds(&tx, session_id, card_id, &prev.state, today)?;
    tx.execute(
        "UPDATE session_card_state SET shown_count = shown_count + 1, again_count = again_count + ?3 WHERE session_id = ?1 AND card_id = ?2",
        params![session_id, card_id, (rating == 1) as i64],
    )
    .map_err(err)?;

    let next = if feed { Some(srs::review(fsrs, &prev, rating, now)) } else { None };
    let overdue = matches!(prev.state.as_str(), "review" | "relearning") && prev.due_at.is_some_and(|d| study_day(d) < today);
    // 접어둔 카드가 돌아와(큐의 보류 복귀) 평가됐다. 신규 상태여도 오늘 신규 상한을 쓰지 않는다 (명세 4.6, I82)
    let from_buried = undo.card.1.is_some();
    tx.execute(
        "INSERT INTO review_log (card_id, session_id, reviewed_at, rating, elapsed_ms, prev_interval_days, next_interval_days, prev_state, fed_to_scheduler, overdue, from_buried)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            card_id,
            session_id,
            now,
            rating,
            elapsed_ms,
            prev.interval_days,
            next.as_ref().map(|n| n.interval_days),
            prev.state,
            feed as i64,
            overdue as i64,
            from_buried as i64
        ],
    )
    .map_err(err)?;
    undo.log_id = Some(tx.last_insert_rowid());

    let result = match (action, next) {
        (RatingAction::Bury, _) => {
            let until = study_day_end_ms(queue::bury_until(today) - 1) + 1;
            tx.execute(
                "UPDATE session_card_state SET is_buried = 1, buried_at = ?3, buried_reason = 'two_again' WHERE session_id = ?1 AND card_id = ?2",
                params![session_id, card_id, now],
            )
            .map_err(err)?;
            tx.execute("UPDATE card SET buried_until = ?2, buried_reason = 'two_again', updated_at = ?3 WHERE id = ?1", params![card_id, until, now])
                .map_err(err)?;
            AnswerResult { action: "buried".into(), due_in_ms: None }
        }
        (RatingAction::Continue, Some(n)) => {
            tx.execute(
                "UPDATE card SET state = ?2, srs_step = ?3, stability = ?4, difficulty = ?5, due_at = ?6, interval_days = ?7,
                        last_reviewed_at = ?8, reps = ?9, lapses = ?10, buried_until = NULL, buried_reason = NULL, updated_at = ?8
                 WHERE id = ?1",
                params![card_id, n.state, n.step, n.stability, n.difficulty, n.due_at, n.interval_days, now, n.reps, n.lapses],
            )
            .map_err(err)?;
            let due = n.due_at.unwrap_or(now);
            if study_day(due) == today {
                AnswerResult { action: "again_later".into(), due_in_ms: Some(due - now) }
            } else {
                AnswerResult { action: "next".into(), due_in_ms: Some(due - now) }
            }
        }
        (RatingAction::Continue, None) => AnswerResult { action: "next".into(), due_in_ms: None },
    };
    tx.commit().map_err(err)?;
    Ok((result, Some(undo)))
}

/// D4. 사용자가 직접 접어둔다 (단축키 −)
#[tauri::command]
pub fn bury_card(state: State<AppState>, session_id: i64, card_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    let now = now_ms();
    let undo = snapshot(&con, session_id, card_id)?;
    let until = study_day_end_ms(queue::bury_until(study_day(now)) - 1) + 1;
    con.execute(
        "INSERT INTO session_card_state (session_id, card_id, is_buried, buried_at, buried_reason) VALUES (?1, ?2, 1, ?3, 'user')
         ON CONFLICT(session_id, card_id) DO UPDATE SET is_buried = 1, buried_at = ?3, buried_reason = 'user'",
        params![session_id, card_id, now],
    )
    .map_err(err)?;
    con.execute("UPDATE card SET buried_until = ?2, buried_reason = 'user', updated_at = ?3 WHERE id = ?1", params![card_id, until, now])
        .map_err(err)?;
    drop(con);
    push_undo(&state, undo)
}

/// 실행 취소 (Ctrl+Z). 이 세션에서 방금 한 평가나 접기를 되돌리고 그 카드 id 를 돌려준다.
/// 되돌린 평가의 복습 기록은 지운다. review_log 는 append-only 지만, 방금 잘못 누른 키는 일어나지 않은 일로 본다 (DECISIONS I22)
#[tauri::command]
pub fn undo_last(state: State<AppState>, session_id: i64) -> Res<Option<i64>> {
    let u = {
        let mut stack = state.undo.lock().map_err(err)?;
        match stack.last() {
            Some(u) if u.session_id == session_id => stack.pop(),
            _ => None,
        }
    };
    let Some(u) = u else { return Ok(None) };
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let (s, buried_until, buried_reason) = &u.card;
    tx.execute(
        "UPDATE card SET state = ?2, srs_step = ?3, stability = ?4, difficulty = ?5, due_at = ?6, interval_days = ?7,
                last_reviewed_at = ?8, reps = ?9, lapses = ?10, buried_until = ?11, buried_reason = ?12, updated_at = ?13
         WHERE id = ?1",
        params![u.card_id, s.state, s.step, s.stability, s.difficulty, s.due_at, s.interval_days, s.last_reviewed_at, s.reps, s.lapses, buried_until, buried_reason, now_ms()],
    )
    .map_err(err)?;
    match &u.session_card {
        Some((again, is_buried, at, reason, shown)) => tx.execute(
            "UPDATE session_card_state SET again_count = ?3, is_buried = ?4, buried_at = ?5, buried_reason = ?6, shown_count = ?7 WHERE session_id = ?1 AND card_id = ?2",
            params![u.session_id, u.card_id, again, is_buried, at, reason, shown],
        ),
        None => tx.execute("DELETE FROM session_card_state WHERE session_id = ?1 AND card_id = ?2", params![u.session_id, u.card_id]),
    }
    .map_err(err)?;
    if let Some(id) = u.log_id {
        tx.execute("DELETE FROM review_log WHERE id = ?1", [id]).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(Some(u.card_id))
}

/// 접어두기 취소 ("지금 다시 보기")
#[tauri::command]
pub fn unbury_card(state: State<AppState>, session_id: i64, card_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    // Again 횟수도 되돌린다. 안 그러면 다시 꺼낸 카드가 Again 한 번에 또 접힌다
    con.execute(
        "UPDATE session_card_state SET is_buried = 0, again_count = 0 WHERE session_id = ?1 AND card_id = ?2",
        params![session_id, card_id],
    )
    .map_err(err)?;
    con.execute("UPDATE card SET buried_until = NULL, buried_reason = NULL WHERE id = ?1", [card_id]).map_err(err)?;
    Ok(())
}

#[derive(Serialize)]
pub struct BuriedCard {
    id: i64,
    question: String,
    reason: String,
    importance: i64,
    deck_path: Vec<String>,
}

/// D5. 학습량은 이 세션이 아니라 오늘(학습일) 기준이다. 화면 문구가 "오늘 학습한 카드" 다
#[derive(Serialize)]
pub struct SessionSummary {
    studied: i64,
    new_done: i64,
    review_done: i64,
    again: i64,
    minutes: i64,
    buried: Vec<BuriedCard>,
    /// 지금 다시 계산한 오늘 남은 분량 (같은 덱 범위, 같은 신규 배율)
    remaining_review: usize,
    remaining_new: usize,
    tomorrow: usize,
    tomorrow_review: usize,
    tomorrow_new: usize,
    tomorrow_decks: Vec<TomorrowDeck>,
}

/// D5-c 내일 예정 한 줄 (최상위 덱)
#[derive(Serialize)]
pub struct TomorrowDeck {
    name: String,
    review: usize,
    new: usize,
    dday: Option<i64>,
}

#[tauri::command]
pub fn finish_session(state: State<AppState>, session_id: i64, completed: bool, deck_ids: Option<Vec<i64>>, scale: f64) -> Res<SessionSummary> {
    let con = state.db.lock().map_err(err)?;
    let now = now_ms();
    con.execute(
        "UPDATE study_session SET ended_at = ?2, end_reason = ?3 WHERE id = ?1 AND ended_at IS NULL",
        params![session_id, now, if completed { "completed" } else { "user_exit" }],
    )
    .map_err(err)?;
    let today = study_day(now);
    let since = today_start_ms(today);
    let (decks, plan) = plan_for(&con, deck_ids.as_deref(), scale, today)?;
    // 학습량은 남은 분량과 같은 덱 범위로 센다. 섞으면 "40장 중" 같은 뜻 없는 합이 나온다
    let scope = match &deck_ids {
        Some(ids) => ids.iter().flat_map(|i| descendants(*i, &decks)).collect::<Vec<_>>(),
        None => decks.iter().map(|d| d.id).collect(),
    };
    let scope = scope.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let one = |cond: &str| -> Res<i64> {
        con.query_row(
            &format!("SELECT {cond} FROM review_log r JOIN card c ON c.id = r.card_id WHERE r.reviewed_at >= ?1 AND c.deck_id IN ({scope}) AND r.kind IS NULL"),
            [since],
            |r| r.get(0),
        )
        .map_err(err)
    };
    let studied = one("count(DISTINCT r.card_id)")?;
    // 접어둔 카드 복귀는 신규로 세지 않는다 (D1 · 홈과 같게, I82)
    let new_done = one("count(DISTINCT CASE WHEN r.prev_state = 'new' AND coalesce(r.from_buried, 0) = 0 THEN r.card_id END)")?;
    let again = one("coalesce(sum(r.rating = 1), 0)")?;
    let started: i64 = con.query_row("SELECT started_at FROM study_session WHERE id = ?1", [session_id], |r| r.get(0)).map_err(err)?;

    let names = note_field_names(&con)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, n.fields, nt.id, t.qfmt, t.afmt, c.template_ord, s.buried_reason, c.eff_importance, c.deck_id
             FROM session_card_state s JOIN card_effective c ON c.id = s.card_id JOIN note n ON n.id = c.note_id
             JOIN note_type nt ON nt.id = n.note_type_id
             LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {TEMPLATE_OF_CARD}
             WHERE s.session_id = ?1 AND s.is_buried = 1 ORDER BY s.buried_at"
        ))
        .map_err(err)?;
    let buried = st
        .query_map([session_id], |r| {
            let fields: String = r.get(1)?;
            let nt: i64 = r.get(2)?;
            let q: Option<String> = r.get(3)?;
            let a: Option<String> = r.get(4)?;
            let rendered = render_fields(&names, nt, &fields, q.as_deref(), a.as_deref(), r.get(5)?);
            Ok(BuriedCard {
                id: r.get(0)?,
                question: render::strip_tags(&rendered.front),
                reason: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                importance: r.get(7)?,
                deck_path: path_of(r.get(8)?, &by_id),
            })
        })
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;

    // D5-c "내일 예정". 모든 덱, 최상위 덱별. 목표일이 가까운 덱부터
    let cards = load_cards(&con)?;
    let next = queue::build_queue(today + 1, &decks, &cards, None, 1.0);
    let tomorrow = next.ordered().len();
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let mut per_root: HashMap<i64, (usize, usize)> = HashMap::new();
    for c in next.ordered() {
        let e = per_root.entry(root_of(c.deck_id, &by_id)).or_default();
        if c.state == "new" { e.1 += 1 } else { e.0 += 1 }
    }
    let mut tomorrow_decks: Vec<TomorrowDeck> = per_root
        .into_iter()
        .filter_map(|(id, (review, new))| {
            let d = by_id.get(&id)?;
            Some(TomorrowDeck { name: d.name.clone(), review, new, dday: d.target_day.map(|t| t - today).filter(|x| *x >= 0) })
        })
        .collect();
    tomorrow_decks.sort_by_key(|d| (d.dday.unwrap_or(i64::MAX), usize::MAX - (d.review + d.new)));
    Ok(SessionSummary {
        studied,
        new_done,
        review_done: studied - new_done,
        again,
        minutes: ((now - started) / 60_000).max(if studied > 0 { 1 } else { 0 }),
        buried,
        remaining_review: plan.ordered().len() - plan.new.len(),
        remaining_new: plan.new.len(),
        tomorrow,
        tomorrow_review: next.ordered().len() - next.new.len(),
        tomorrow_new: next.new.len(),
        tomorrow_decks,
    })
}

/// D5. 접어둔 카드를 오늘 다시 볼지(true) 다음 학습으로 넘길지(false). 오늘이면 다시 볼 카드 id 를 돌려준다
#[tauri::command]
pub fn resolve_buried(state: State<AppState>, session_id: i64, today: bool) -> Res<Vec<i64>> {
    let con = state.db.lock().map_err(err)?;
    let mut st = con
        .prepare("SELECT card_id FROM session_card_state WHERE session_id = ?1 AND is_buried = 1")
        .map_err(err)?;
    let ids: Vec<i64> = st.query_map([session_id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    if today {
        for id in &ids {
            con.execute("UPDATE card SET buried_until = NULL, buried_reason = NULL WHERE id = ?1", [id]).map_err(err)?;
            con.execute("UPDATE session_card_state SET is_buried = 0, again_count = 0 WHERE session_id = ?1 AND card_id = ?2", params![session_id, id])
                .map_err(err)?;
        }
        Ok(ids)
    } else {
        Ok(vec![])
    }
}

// ------------------------------------------------------------------ 미디어 (C3 이미지 첨부, 가져온 덱의 그림)

pub(crate) fn media_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("media")
}

/// 파일 이름만 받는다. 경로 구분자나 상위 경로는 거부한다 (미디어 폴더 밖을 읽지 못하게)
pub(crate) fn safe_media_name(name: &str) -> Res<&str> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
        return Err("미디어 이름이 올바르지 않아요.".into());
    }
    Ok(name)
}

/// data_dir 기준 `<dir>/<안전한 이름>` 경로인가. 동기화로 받은 경로가 다른 곳의 파일을 가리키지 않게
pub(crate) fn safe_rel_path(rel: &str, dir: &str) -> bool {
    rel.strip_prefix(dir).and_then(|r| r.strip_prefix('/')).is_some_and(|n| safe_media_name(n).is_ok())
}

pub(crate) fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("flac") => "audio/flac",
        _ => "application/octet-stream",
    }
}

/// 이미지를 저장하고 카드 HTML 에 쓸 파일 이름을 돌려준다. 같은 내용이면 같은 이름이다
#[tauri::command]
pub fn save_media(state: State<AppState>, name: String, data_base64: String) -> Res<String> {
    use base64::Engine;
    use std::hash::{Hash, Hasher};
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_base64.trim()).map_err(err)?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("이미지가 너무 커요. 20MB 이하만 넣을 수 있어요.".into());
    }
    let ext = name
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("png")
        .to_ascii_lowercase();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let hash = format!("{:016x}", h.finish());
    let file = format!("neo-{hash}.{ext}");
    let dir = media_dir(&state.data_dir);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(&file);
    if !path.exists() {
        std::fs::write(&path, &bytes).map_err(err)?;
    }
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "INSERT OR IGNORE INTO media (filename, rel_path, sha256, byte_size, mime_type, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![file, format!("media/{file}"), hash, bytes.len() as i64, mime_of(&file), now_ms()],
    )
    .map_err(err)?;
    Ok(file)
}

/// data URL 로 화면에 넣는 미디어의 상한 (T7-01). 100MB 그림이면 렌더러가 11분 넘게 멈췄다. 넘으면 화면이 "너무 커서" 자리 표시를 그린다
const MAX_INLINE: u64 = 20 << 20;

/// 카드 HTML 의 <img src="파일이름"> 을 화면에 띄우기 위한 data URL. MAX_INLINE 을 넘으면 `too_large:<바이트>` 오류 (cardHtml.ts)
#[tauri::command]
pub fn read_media(state: State<AppState>, name: String) -> Res<String> {
    media_data_url(&state.data_dir, &name)
}

fn media_data_url(data_dir: &std::path::Path, name: &str) -> Res<String> {
    use base64::Engine;
    let name = safe_media_name(name)?;
    let path = media_dir(data_dir).join(name);
    let missing = |_| format!("미디어 파일을 찾을 수 없어요: {name}");
    let size = std::fs::metadata(&path).map_err(missing)?.len();
    if size > MAX_INLINE {
        return Err(format!("too_large:{size}"));
    }
    let bytes = std::fs::read(&path).map_err(missing)?;
    Ok(format!("data:{};base64,{}", mime_of(name), base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// 사용 중인 데이터 위치 (E1)
#[derive(Serialize)]
pub struct AppInfo {
    /// 학습 데이터를 열지 못한 이유 (S1-6)
    startup_error: Option<String>,
    data_dir: String,
    backup_dir: String,
    /// 가장 최근 백업 파일의 시각 (ms)
    last_backup: Option<i64>,
    cutoff_hour: i64,
    db_bytes: u64,
    /// 그림 · 미디어 합 (K4 올리는 것)
    media_bytes: i64,
    cards: i64,
    decks: i64,
    reviews: i64,
    version: String,
}

fn backup_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("backups")
}

fn last_backup(dir: &std::path::Path) -> Option<i64> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "zip"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

/// 파일 하나로 백업한다: DB 사본(VACUUM INTO) + 미디어 폴더. 복원은 zip 을 풀어 데이터 폴더에 두면 된다
pub(crate) fn write_backup(con: &Connection, data_dir: &std::path::Path) -> Res<std::path::PathBuf> {
    use std::io::Write;
    let dir = backup_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let db_copy = dir.join(format!(".neoanki-{stamp}.sqlite"));
    let _ = std::fs::remove_file(&db_copy);
    con.execute("VACUUM INTO ?1", [db_copy.to_string_lossy()]).map_err(err)?;

    let out = dir.join(format!("neoanki-{stamp}.zip"));
    let result = (|| -> Res<()> {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&out).map_err(err)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("neoanki.sqlite", opts).map_err(err)?;
        zip.write_all(&std::fs::read(&db_copy).map_err(err)?).map_err(err)?;
        // 그림과 원자료(PDF 등)는 이미 압축돼 있다. 다시 압축하지 않는다. models/ 는 앱이 받는 것이라 넣지 않는다
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for sub in ["media", "sources"] {
            let Ok(entries) = std::fs::read_dir(data_dir.join(sub)) else { continue };
            for e in entries.filter_map(Result::ok).filter(|e| e.path().is_file()) {
                zip.start_file(format!("{sub}/{}", e.file_name().to_string_lossy()), stored).map_err(err)?;
                zip.write_all(&std::fs::read(e.path()).map_err(err)?).map_err(err)?;
            }
        }
        zip.finish().map_err(err)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&db_copy);
    if result.is_err() {
        let _ = std::fs::remove_file(&out);
    }
    result.map(|_| out)
}

#[tauri::command]
pub fn app_info(state: State<AppState>) -> Res<AppInfo> {
    let con = state.db.lock().map_err(err)?;
    let n = |sql: &str| -> Res<i64> { con.query_row(sql, [], |r| r.get(0)).map_err(err) };
    let db_path = state.data_dir.join("neoanki.sqlite");
    Ok(AppInfo {
        startup_error: state.startup_error.lock().map_err(err)?.clone(),
        data_dir: state.data_dir.display().to_string(),
        backup_dir: backup_dir(&state.data_dir).display().to_string(),
        last_backup: last_backup(&backup_dir(&state.data_dir)),
        cutoff_hour: db::cutoff_hour(),
        // 아직 본 파일에 옮기지 않은 쓰기(-wal)까지: 연결 직후 K4 가 보여 주는 크기와 같게 (T3-15)
        db_bytes: [&db_path, &db_path.with_extension("sqlite-wal")].iter().map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum(),
        media_bytes: n("SELECT coalesce(sum(byte_size), 0) FROM media")?,
        cards: n("SELECT count(*) FROM card")?,
        decks: n("SELECT count(*) FROM deck WHERE archived_at IS NULL")?,
        reviews: n("SELECT count(*) FROM review_log")?,
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

// ------------------------------------------------------------------ MVP 합격선 (mvp-scope 12장)

/// ① 그날의 계획과 실제 (목표일이 있는 덱 합산)
#[derive(Serialize)]
pub struct PlanDay {
    day: String,
    planned: i64,
    done: i64,
}

#[derive(Serialize)]
pub struct MvpMetrics {
    /// 목표일이 있는 덱 수. 0 이면 ① 은 계산하지 않는다 (E2-0)
    target_decks: usize,
    /// ① 최근 14일 (오늘 제외) 중 계획이 있던 날. 날짜 오름차순
    plan_days: Vec<PlanDay>,
    /// ① 일별 완료율의 중앙값. 0~1. 계획이 있던 날만으로 계산한다
    plan_median: Option<f64>,
    /// ① 판정 전 남은 날 계산용: 목표일 덱의 계획이 처음 남은 날
    first_plan_day: Option<String>,
    /// ② 판정 전 남은 날 계산용: 이 앱에서 처음 학습한 날
    first_session_day: Option<String>,
    /// ② 보류가 생긴 세션들 (오래된 것부터 최근 20개). true = 끝까지 마침
    sessions: Vec<bool>,
    /// ② 보류가 생긴 세션 중 남은 예정 카드를 끝낸 세션의 비율. 0~1
    bury_rate: Option<f64>,
    bury_sessions: i64,
    bury_completed: i64,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 })
}

/// 계획 완료율 = 그날 그 덱에서 본 카드 수 / 그날 처음 계획한 카드 수 (1을 넘지 않는다)
fn plan_rate(planned: i64, done: i64) -> f64 {
    (done as f64 / planned as f64).min(1.0)
}

#[tauri::command]
pub fn mvp_metrics(state: State<AppState>) -> Res<MvpMetrics> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    let decks = load_decks(&con)?;
    let target_decks = decks.iter().filter(|d| d.target_day.is_some()).count();

    // 날짜별로 목표일 덱들의 계획을 합친다
    let rows: Vec<(String, i64, i64)> = {
        let mut st = con
            .prepare(
                "SELECT study_day, deck_id, planned_review + planned_new FROM daily_plan
                 WHERE has_target = 1 AND planned_review + planned_new > 0 AND study_day >= ?1 AND study_day < ?2
                 ORDER BY study_day",
            )
            .map_err(err)?;
        st.query_map(params![db::date_from_day(today - 14), db::date_from_day(today)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?
    };
    let mut plan_days: Vec<PlanDay> = vec![];
    for (day, deck_id, planned) in rows {
        let d = db::day_from_date(&day).ok_or("날짜 형식이 올바르지 않아요.")?;
        let ids = descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let done: i64 = con
            .query_row(
                &format!(
                    "SELECT count(DISTINCT r.card_id) FROM review_log r JOIN card c ON c.id = r.card_id
                     WHERE c.deck_id IN ({ids}) AND r.reviewed_at BETWEEN ?1 AND ?2 AND r.kind IS NULL"
                ),
                params![today_start_ms(d), study_day_end_ms(d)],
                |r| r.get(0),
            )
            .map_err(err)?;
        // 덱마다 계획보다 많이 한 것은 계획만큼으로 친다 (다른 덱의 미달을 덮지 않게)
        let done = done.min(planned);
        match plan_days.last_mut() {
            Some(p) if p.day == day => {
                p.planned += planned;
                p.done += done;
            }
            _ => plan_days.push(PlanDay { day, planned, done }),
        }
    }
    let first_plan_day: Option<String> = con
        .query_row("SELECT min(study_day) FROM daily_plan WHERE has_target = 1 AND planned_review + planned_new > 0", [], |r| r.get(0))
        .map_err(err)?;

    let first_session_day: Option<i64> =
        con.query_row("SELECT min(started_at) FROM study_session", [], |r| r.get(0)).map_err(err)?;
    let mut sessions: Vec<bool> = {
        let mut st = con
            .prepare(
                "SELECT s.end_reason = 'completed' FROM study_session s
                 WHERE s.ended_at IS NOT NULL AND s.mode IS NULL
                   AND EXISTS (SELECT 1 FROM session_card_state x WHERE x.session_id = s.id AND x.buried_at IS NOT NULL)
                 ORDER BY s.started_at DESC LIMIT 20",
            )
            .map_err(err)?;
        st.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    sessions.reverse();
    let (bury_sessions, bury_completed): (i64, i64) = con
        .query_row(
            "SELECT count(*), coalesce(sum(s.end_reason = 'completed'), 0) FROM study_session s
             WHERE EXISTS (SELECT 1 FROM session_card_state x WHERE x.session_id = s.id AND x.buried_at IS NOT NULL)
               AND s.ended_at IS NOT NULL AND s.mode IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err)?;

    Ok(MvpMetrics {
        target_decks,
        plan_median: median(plan_days.iter().map(|p| plan_rate(p.planned, p.done)).collect()),
        plan_days,
        first_plan_day,
        first_session_day: first_session_day.map(|ms| db::date_from_day(study_day(ms))),
        sessions,
        bury_rate: (bury_sessions > 0).then(|| bury_completed as f64 / bury_sessions as f64),
        bury_sessions,
        bury_completed,
    })
}

/// E2-h 히트맵 한 칸
#[derive(Serialize)]
pub struct HeatDay {
    day: String,
    /// 그날 평가한 카드 수 (같은 카드를 여러 번 봐도 한 장)
    cards: i64,
    /// 그날 학습한 최상위 덱 수 (툴팁 "덱 3개")
    decks: i64,
}

/// 히트맵 기간: 52주 (학기·시험 주기가 한눈에, E2-h)
pub const HEATMAP_DAYS: i64 = 52 * 7;

/// E2-h 학습 기록 히트맵. 오늘까지 52주. deck_id 가 있으면 그 덱(하위 덱 포함)만. 기록이 없는 날은 빠진다
#[tauri::command]
pub fn heatmap(state: State<AppState>, deck_id: Option<i64>) -> Res<Vec<HeatDay>> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    let decks = load_decks(&con)?;
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let scope = match deck_id {
        Some(id) => format!("AND c.deck_id IN ({})", id_list(&descendants(id, &decks))),
        None => String::new(),
    };
    let mut st = con
        .prepare(&format!(
            "SELECT r.reviewed_at, r.card_id, c.deck_id FROM review_log r LEFT JOIN card c ON c.id = r.card_id WHERE r.reviewed_at >= ?1 {scope}"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([today_start_ms(today - HEATMAP_DAYS + 1)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)))
        .map_err(err)?;
    let mut days: std::collections::BTreeMap<i64, (std::collections::HashSet<i64>, std::collections::HashSet<i64>)> = Default::default();
    for row in rows {
        let (at, card, deck) = row.map_err(err)?;
        let e = days.entry(study_day(at)).or_default();
        // 지운 카드의 기록도 센다 (card_id 가 NULL 이면 기록 id 대신 시각으로 구분)
        e.0.insert(card.unwrap_or(-at));
        if let Some(d) = deck {
            e.1.insert(root_of(d, &by_id));
        }
    }
    Ok(days
        .into_iter()
        .filter(|(d, _)| *d <= today)
        .map(|(d, (c, k))| HeatDay { day: db::date_from_day(d), cards: c.len() as i64, decks: k.len() as i64 })
        .collect())
}

/// E1 "백업 만들기". 만든 파일 경로를 돌려준다
#[tauri::command]
pub async fn create_backup(state: State<'_, AppState>) -> Res<String> {
    let con = state.db.lock().map_err(err)?;
    write_backup(&con, &state.data_dir).map(|p| p.display().to_string())
}

/// S1-6 "다시 시도". 학습 데이터를 다시 연다
#[tauri::command]
pub fn retry_open(state: State<AppState>) -> Res<()> {
    let con = db::open(&state.data_dir.join("neoanki.sqlite")).map_err(|e| format!("이번에도 열지 못했어요: {e}"))?;
    *state.db.lock().map_err(err)? = con;
    *state.startup_error.lock().map_err(err)? = None;
    Ok(())
}

/// S1-6 "백업에서 복구". 가장 최근 백업 zip 으로 되돌린다.
/// 지금 파일은 지우지 않고 neoanki-before-restore-<시각>.sqlite 로 옆에 남긴다. 미디어는 없는 것만 채운다
#[tauri::command]
pub async fn restore_backup(state: State<'_, AppState>) -> Res<String> {
    use std::io::Read;
    let dir = backup_dir(&state.data_dir);
    let latest = std::fs::read_dir(&dir)
        .map_err(|_| "백업 파일이 없어요.".to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "zip"))
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .ok_or("백업 파일이 없어요.")?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&latest).map_err(err)?).map_err(|_| "백업 파일을 읽을 수 없어요.".to_string())?;

    let mut guard = state.db.lock().map_err(err)?;
    // 파일을 옮기려면 연결부터 닫는다 (Windows 는 열린 파일을 옮기지 못한다)
    *guard = db::open_in_memory().map_err(err)?;
    let db_path = state.data_dir.join("neoanki.sqlite");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for suffix in ["", "-wal", "-shm"] {
        let from = std::path::PathBuf::from(format!("{}{suffix}", db_path.display()));
        if from.exists() {
            let to = state.data_dir.join(format!("neoanki-before-restore-{stamp}.sqlite{suffix}"));
            std::fs::rename(&from, &to).map_err(err)?;
        }
    }
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(err)?;
        let name = f.name().map_err(err)?.into_owned();
        let target = if name == "neoanki.sqlite" {
            db_path.clone()
        } else if let Some((sub, file)) = name.split_once('/').filter(|(sub, _)| ["media", "sources"].contains(sub)) {
            let dir = state.data_dir.join(sub);
            if safe_media_name(file).is_err() || dir.join(file).exists() {
                continue;
            }
            std::fs::create_dir_all(&dir).map_err(err)?;
            dir.join(file)
        } else {
            continue;
        };
        let mut buf = vec![];
        f.read_to_end(&mut buf).map_err(err)?;
        std::fs::write(target, buf).map_err(err)?;
    }
    *guard = db::open(&db_path).map_err(|e| format!("백업도 열지 못했어요: {e}"))?;
    *state.startup_error.lock().map_err(err)? = None;
    Ok(latest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

/// E1 "폴더 열기"
#[tauri::command]
pub fn open_backup_dir(state: State<AppState>) -> Res<()> {
    let dir = backup_dir(&state.data_dir);
    std::fs::create_dir_all(&dir).map_err(err)?;
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program).arg(&dir).spawn().map_err(err)?;
    Ok(())
}

/// E1 "하루 시작 시각". 0시부터 12시 사이
#[tauri::command]
pub fn set_cutoff_hour(state: State<AppState>, hour: i64) -> Res<()> {
    if !(0..=12).contains(&hour) {
        return Err("하루 시작 시각은 0시부터 12시 사이여야 해요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    db::set_cutoff_hour(&con, hour).map_err(err)
}

/// E1-b 모든 데이터 삭제. backup 이면 먼저 백업 파일을 만든다. 만든 백업 경로를 돌려준다
#[tauri::command]
pub async fn wipe_all(state: State<'_, AppState>, backup: bool) -> Res<Option<String>> {
    let mut con = state.db.lock().map_err(err)?;
    let saved = if backup { Some(write_backup(&con, &state.data_dir)?.display().to_string()) } else { None };
    let tx = con.transaction().map_err(err)?;
    // Drive 동기화 중이면 "이 기기에서" 지우는 것이 다른 기기로 번지지 않게 먼저 끊는다 (Drive 데이터는 그대로, K10)
    crate::sync::forget_local(&tx).map_err(err)?;
    // K7 두 기기에서 고친 카드 · K36 노트 유형의 진 쪽 내용도 학습 데이터다 (연결 끊기와 달리 지운다)
    tx.execute_batch("DROP TABLE IF EXISTS sync_conflict; DROP TABLE IF EXISTS sync_type_conflict;").map_err(err)?;
    tx.execute_batch(
        "DELETE FROM session_card_state; DELETE FROM study_session; DELETE FROM review_log; DELETE FROM card;
         DELETE FROM note; DELETE FROM card_template; DELETE FROM note_type; DELETE FROM deck_config; DELETE FROM deck;
         DELETE FROM media; DELETE FROM import_issue; DELETE FROM import_batch;
         DELETE FROM card_change; DELETE FROM plan_setting; DELETE FROM embedding_cache;",
    )
    .map_err(err)?;
    db::seed_note_types(&tx).map_err(err)?;
    tx.commit().map_err(err)?;
    let _ = con.execute_batch("VACUUM");
    let _ = std::fs::remove_dir_all(media_dir(&state.data_dir));
    // 원자료 복사본도 사용자 데이터다. models/ 는 앱 자원이라 남긴다
    let _ = std::fs::remove_dir_all(state.data_dir.join("sources"));
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use crate::db::{now_ms, study_day, study_day_end_ms};
    use rusqlite::{params, Connection};

    /// 덱 하나에 기본 카드 n장 (하루 신규 상한 limit). 카드 id
    fn deck_with_cards(con: &Connection, n: usize, limit: Option<i64>) -> Vec<i64> {
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('t', 0, 0)", []).unwrap();
        let deck_id = con.last_insert_rowid();
        con.execute("INSERT INTO deck_config (deck_id, daily_new_limit, updated_at) VALUES (?1, ?2, 0)", params![deck_id, limit]).unwrap();
        for i in 0..n {
            let card = super::NewCard { deck_id, kind: "basic".into(), front: format!("q{i}"), back: "a".into(), importance: None };
            super::insert_note(con, &card, "", None).unwrap();
        }
        con.prepare("SELECT id FROM card ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    /// 오늘 학습일과 그 한가운데 시각 (경계 근처에서 돌려도 같은 날)
    fn mid_today() -> (i64, i64) {
        let today = study_day(now_ms());
        (today, (super::today_start_ms(today) + study_day_end_ms(today)) / 2)
    }

    fn session(con: &Connection, day: i64) -> i64 {
        con.execute("INSERT INTO study_session (started_at, study_day) VALUES (0, ?1)", [crate::db::date_from_day(day)]).unwrap();
        con.last_insert_rowid()
    }

    fn plan_ids(con: &Connection, today: i64) -> (Vec<i64>, usize) {
        let (_, plan) = super::plan_for(con, None, 1.0, today).unwrap();
        (plan.ordered().iter().map(|c| c.id).collect(), plan.new.len())
    }

    #[test]
    fn relearning_card_stays_today_after_again() {
        // T1-01: 오늘 예정 복습에서 Again -> 재학습. 자기 노트를 오늘 봤다고 형제 규칙이 빼면 안 된다
        let mut con = crate::db::open_in_memory().unwrap();
        let ids = deck_with_cards(&con, 1, None);
        let (today, now) = mid_today();
        con.execute(
            "UPDATE card SET state = 'review', stability = 5, difficulty = 5, interval_days = 5, due_at = ?2, last_reviewed_at = ?3, reps = 3 WHERE id = ?1",
            params![ids[0], now - 1000, now - 5 * crate::db::DAY_MS],
        )
        .unwrap();
        let s = session(&con, today);
        assert_eq!(super::answer_at(&mut con, &fsrs::FSRS::default(), s, ids[0], 1, 1000, now).unwrap().0.action, "again_later");
        assert_eq!(plan_ids(&con, today).0, [ids[0]], "재학습 카드는 오늘 큐에 남는다 (이어서 학습 · 홈 · D5 남은 수)");
    }

    #[test]
    fn returning_new_card_does_not_use_new_cap() {
        // T1-05: 상한 3 덱에 복귀 카드(신규 상태) 1 + 신규 3. 복귀 1 + 신규 1 을 학습하면 남은 신규는 2장
        let mut con = crate::db::open_in_memory().unwrap();
        let ids = deck_with_cards(&con, 4, Some(3));
        let (today, now) = mid_today();
        con.execute("UPDATE card SET buried_until = ?2, buried_reason = 'user' WHERE id = ?1", params![ids[0], super::today_start_ms(today)]).unwrap();
        let (_, plan) = super::plan_for(&con, None, 1.0, today).unwrap();
        assert_eq!((plan.returning.len(), plan.new.len()), (1, 3));
        let s = session(&con, today);
        for id in [ids[0], ids[1]] {
            super::answer_at(&mut con, &fsrs::FSRS::default(), s, id, 3, 1000, now).unwrap();
        }
        assert_eq!(plan_ids(&con, today).1, 2, "복귀 카드는 신규 상한을 쓰지 않는다 (명세 4.6)");
        assert_eq!(super::introduced_today(&con, today).unwrap().values().sum::<i64>(), 1);
    }

    #[test]
    fn rating_after_day_boundary_ends_session() {
        // T1-06: 어제 학습일에 연 세션에서 경계 뒤에 평가하면 남기지 않고 day_rollover 로 닫는다
        let mut con = crate::db::open_in_memory().unwrap();
        let ids = deck_with_cards(&con, 1, None);
        let (today, now) = mid_today();
        let s = session(&con, today - 1);
        let (r, undo) = super::answer_at(&mut con, &fsrs::FSRS::default(), s, ids[0], 3, 1000, now).unwrap();
        assert_eq!((r.action.as_str(), undo.is_none()), ("day_rollover", true));
        let (reason, logs): (String, i64) = con
            .query_row("SELECT end_reason, (SELECT count(*) FROM review_log) FROM study_session WHERE id = ?1", [s], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((reason.as_str(), logs), ("day_rollover", 0));
        let s2 = session(&con, today);
        assert_eq!(super::answer_at(&mut con, &fsrs::FSRS::default(), s2, ids[0], 3, 1000, now).unwrap().0.action, "again_later");
    }

    #[test]
    fn median_and_plan_rate() {
        assert_eq!(super::median(vec![]), None);
        assert_eq!(super::median(vec![0.2, 0.9, 0.5]), Some(0.5));
        assert!((super::median(vec![0.4, 0.8]).unwrap() - 0.6).abs() < 1e-9);
        assert_eq!(super::plan_rate(10, 15), 1.0, "계획보다 많이 해도 100%");
        assert_eq!(super::plan_rate(10, 7), 0.7);
    }

    #[test]
    fn deleting_a_deck_removes_only_its_own_orphaned_notes() {
        // T3-07: 덱을 지울 때 카드 없는 노트를 모두 지우면, 다른 기기에서 되살아나는 중인 노트(나중에 고친 내용)까지 지워 모든 기기로 퍼진다
        let con = crate::db::open_in_memory().unwrap();
        let note = |deck: Option<i64>| {
            con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES ((SELECT min(id) FROM note_type), '[\"q\",\"a\"]', 0, 0)", []).unwrap();
            let n = con.last_insert_rowid();
            if let Some(d) = deck {
                con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, 0, 0, 0)", [n, d]).unwrap();
            }
            n
        };
        let deck = |name: &str, parent: Option<i64>| {
            con.execute("INSERT INTO deck (name, parent_id, created_at, updated_at) VALUES (?1, ?2, 0, 0)", rusqlite::params![name, parent]).unwrap();
            con.last_insert_rowid()
        };
        let gone = deck("지울 덱", None);
        let sub = deck("하위", Some(gone));
        let kept = deck("남을 덱", None);
        let (in_deck, in_sub, elsewhere, orphan) = (note(Some(gone)), note(Some(sub)), note(Some(kept)), note(None));
        super::delete_deck_rows(&con, gone).unwrap();
        let alive = |id: i64| con.query_row("SELECT count(*) FROM note WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).unwrap() == 1;
        assert!(!alive(in_deck) && !alive(in_sub), "지운 덱과 하위 덱의 노트");
        assert!(alive(elsewhere), "다른 덱의 노트");
        assert!(alive(orphan), "이 덱과 상관없는 카드 없는 노트는 그대로");
    }

    #[test]
    fn reversed_note_makes_two_cards() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('t', 0, 0)", []).unwrap();
        let card = super::NewCard { deck_id: con.last_insert_rowid(), kind: "reversed".into(), front: "앞".into(), back: "뒤".into(), importance: None };
        assert_eq!(super::insert_note(&con, &card, "", None).unwrap(), 2);
        let ords: Vec<i64> =
            con.prepare("SELECT template_ord FROM card ORDER BY template_ord").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(ords, [0, 1]);
    }

    #[test]
    fn search_skips_sound_file_names() {
        let con = crate::db::open_in_memory().unwrap();
        for f in [r#"["abandon [sound:abandon.mp3]","버리다"]"#, r#"["mp3 플레이어","[sound:x.ogg]"]"#] {
            con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES ((SELECT min(id) FROM note_type), ?1, 0, 0)", [f]).unwrap();
        }
        // list_cards 와 같은 조합: 원문 LIKE 에 걸리고 소리 이름뿐인 노트는 뺀다
        let found = |q: &str| -> Vec<i64> {
            let hidden = super::sound_only_hits(&con, q, q).unwrap();
            let sql = format!("SELECT n.id FROM note n WHERE {} LIKE '%' || ?1 || '%' ORDER BY n.id", super::SEARCH_TEXT);
            con.prepare(&sql).unwrap().query_map([q], |r| r.get(0)).unwrap().map(Result::unwrap).filter(|id| !hidden.contains(id)).collect()
        };
        assert_eq!(found("abandon"), [1]);
        assert_eq!(found("mp3"), [2], "소리 파일 이름은 찾지 않는다. 글자에 있는 mp3 는 찾는다");
        assert_eq!(found("ABANDON.MP3"), Vec::<i64>::new(), "대소문자를 가리지 않는 것도 LIKE 와 같다");
        assert_eq!(found("ogg"), Vec::<i64>::new());
        assert_eq!(found("버리"), [1]);
    }

    #[test]
    fn table_card_rows_show_question_only() {
        let front = "<div>t-검정의 귀무가설은?</div><table><tr><th>검정</th><th>귀무가설</th></tr><tr><td>t-검정</td><td><span class=\"cloze\">[...]</span></td></tr></table>";
        let back = front.replace("[...]", "두 집단 평균이 같다");
        assert_eq!(super::table_brief(front, &back), Some(("t-검정의 귀무가설은?".into(), "두 집단 평균이 같다".into())));
        assert_eq!(super::table_brief("<table><tr><td>a</td></tr></table>", "b"), None, "표 앞에 글이 없으면 원래대로");
        assert_eq!(super::table_brief("질문", "답"), None);
    }

    #[test]
    fn answer_part_cuts_front_side() {
        assert_eq!(super::answer_part("질문<hr id=answer>답"), "답");
        assert_eq!(super::answer_part("Q<HR id=\"answer\">A"), "A");
        assert_eq!(super::answer_part("빈칸 답"), "빈칸 답");
    }

    #[test]
    fn write_mode_follows_the_nearest_deck() {
        let con = crate::db::open_in_memory().unwrap();
        for (id, parent) in [(1, None), (2, Some(1)), (3, Some(2))] {
            con.execute("INSERT INTO deck (id, parent_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 0)", rusqlite::params![id, parent, format!("d{id}")]).unwrap();
        }
        assert_eq!(super::write_mode_source(&con, 3).unwrap(), None, "켠 덱이 없으면 템플릿대로");
        con.execute("INSERT INTO deck_config (deck_id, write_mode, updated_at) VALUES (1, 1, 0)", []).unwrap();
        assert_eq!(super::write_mode_source(&con, 3).unwrap(), Some((1, "d1".into())), "위 덱을 따른다");
        con.execute("INSERT INTO deck_config (deck_id, write_mode, updated_at) VALUES (3, 1, 0)", []).unwrap();
        assert_eq!(super::write_mode_source(&con, 3).unwrap().map(|s| s.0), Some(3), "가장 가까운 덱");
    }

    #[test]
    fn huge_media_is_not_inlined() {
        // T7-01: 큰 그림을 data URL 로 넣으면 렌더러가 멈춘다. 상한을 넘으면 크기만 알린다 (화면이 "너무 커서" 로 그린다)
        let dir = std::env::temp_dir().join(format!("neoanki-media-{}-{}", std::process::id(), crate::db::now_ms()));
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/small.png"), b"png").unwrap();
        std::fs::File::create(dir.join("media/big.png")).unwrap().set_len(super::MAX_INLINE + 1).unwrap();
        assert_eq!(super::media_data_url(&dir, "small.png").unwrap(), "data:image/png;base64,cG5n");
        assert_eq!(super::media_data_url(&dir, "big.png").unwrap_err(), format!("too_large:{}", super::MAX_INLINE + 1));
        assert!(super::media_data_url(&dir, "none.png").unwrap_err().contains("찾을 수 없어요"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
