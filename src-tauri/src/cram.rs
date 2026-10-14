//! H3 시험 직전 압축 회독. 목표일 3일 전부터 연다 (홈 · D1).
//! 모으는 카드 = 아직 불안한 핵심(FSRS 안정도 7일 미만) ∪ 약한 챕터 ∪ 최근 2주 Again. 잘 외운 카드(안정도 30일 이상)는 뺀다.
//! 평가는 SRS 에 넘기지 않는다: review_log 에 kind = 'cram' 으로만 남긴다 (히트맵에는 들어간다).
//! 카드 일정 · 오늘의 계획 · 합격선 지표는 그대로다 (그쪽 쿼리는 kind IS NULL 인 기록만 센다)

use crate::commands::{self, AppState};
use crate::db::{now_ms, study_day, DAY_MS};
use crate::queue;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const SHAKY_DAYS: f64 = 7.0;
const STABLE_DAYS: f64 = 30.0;
const AGAIN_DAYS: i64 = 14;
/// 목표일 며칠 전부터 여는가
const OPEN_DAYS: i64 = 3;
/// 기록이 모자랄 때 카드 1장에 잡는 시간 (plan.rs 와 같다)
const DEFAULT_SEC: f64 = 10.0;

/// 범위 세그먼트: 0 핵심만 · 1 핵심 + 약한 챕터 · 2 불안한 카드 전부
type Scope = u8;

struct Item {
    id: i64,
    core: bool,
    weak: bool,
    again: bool,
    stable: bool,
    core_card: bool,
}

impl Item {
    fn picked(&self, scope: Scope) -> bool {
        !self.stable && (self.core || (scope >= 1 && self.weak) || (scope >= 2 && self.again))
    }
}

/// 목표일이 오늘 ~ 3일 뒤인 덱 중 가장 가까운 것. deck_ids 가 있으면 그 덱과 위아래로 이어진 덱만
fn target<'a>(decks: &'a [queue::Deck], deck_ids: Option<&[i64]>, today: i64) -> Option<&'a queue::Deck> {
    decks
        .iter()
        .filter(|d| d.target_day.is_some_and(|t| (0..=OPEN_DAYS).contains(&(t - today))))
        .filter(|d| {
            deck_ids.is_none_or(|ids| {
                ids.iter().any(|&i| commands::descendants(d.id, decks).contains(&i) || commands::descendants(i, decks).contains(&d.id))
            })
        })
        .min_by_key(|d| (d.target_day, d.parent_id.is_some()))
}

/// 범위 안에서 한 번이라도 본 카드 (신규 · 보류 상태 제외). 순서: 핵심 먼저, 안정도 낮은 것 먼저
fn items(con: &Connection, scope_ids: &[i64], weak_ids: &HashSet<i64>) -> Res<Vec<Item>> {
    let ids = scope_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.eff_importance, COALESCE(c.stability, c.interval_days), c.deck_id,
                    EXISTS (SELECT 1 FROM review_log r WHERE r.card_id = c.id AND r.rating = 1 AND r.reviewed_at >= ?1)
             FROM card_effective c WHERE c.deck_id IN ({ids}) AND c.state IN ('learning', 'review', 'relearning')
             ORDER BY c.eff_importance DESC, COALESCE(c.stability, c.interval_days), c.id"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([now_ms() - AGAIN_DAYS * DAY_MS], |r| {
            let (importance, stability, deck): (i64, f64, i64) = (r.get(1)?, r.get(2)?, r.get(3)?);
            Ok(Item {
                id: r.get(0)?,
                core: importance == 2 && stability < SHAKY_DAYS,
                weak: weak_ids.contains(&deck),
                again: r.get(4)?,
                stable: stability >= STABLE_DAYS,
                core_card: importance == 2,
            })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

/// 대상 덱 아래에서 약한 챕터(하위 덱까지)와, 화면에 보일 약한 챕터 이름 (대상 덱 아래 경로)
fn weak_scope(target: &queue::Deck, decks: &[queue::Deck]) -> (HashSet<i64>, Vec<String>) {
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let mut ids = HashSet::new();
    let mut names = vec![];
    let depth = commands::path_of(target.id, &by_id).len();
    for id in commands::descendants(target.id, decks) {
        let d = by_id[&id];
        if !d.weak || ids.contains(&id) {
            continue;
        }
        let path = commands::path_of(id, &by_id);
        names.push(if path.len() > depth { path[depth..].join(" · ") } else { d.name.clone() });
        ids.extend(commands::descendants(id, decks));
    }
    (ids, names)
}

/// 카드 1장에 드는 시간 (초). 최근 14일 실측의 중앙값, 1~60초로 자른다 (plan.rs read_time 과 같은 규칙)
fn sec_per_card(con: &Connection) -> Res<f64> {
    let mut st = con.prepare("SELECT elapsed_ms FROM review_log WHERE reviewed_at >= ?1 AND elapsed_ms > 0").map_err(err)?;
    let mut secs: Vec<f64> = st
        .query_map([now_ms() - 14 * DAY_MS], |r| r.get::<_, i64>(0))
        .map_err(err)?
        .filter_map(Result::ok)
        .map(|ms| (ms as f64 / 1000.0).clamp(1.0, 60.0))
        .collect();
    secs.sort_by(f64::total_cmp);
    Ok(if secs.len() >= 20 { secs[secs.len() / 2] } else { DEFAULT_SEC })
}

#[derive(Serialize)]
pub struct CramPlan {
    deck_id: i64,
    name: String,
    /// 목표일까지 남은 날 (0 = 오늘)
    dday: i64,
    /// 아직 불안한 핵심 카드 / 본 적 있는 핵심 카드
    core: usize,
    core_total: usize,
    weak: usize,
    weak_names: Vec<String>,
    /// 최근 2주에 Again 을 누른 카드
    again: usize,
    /// 잘 외워서 뺀 카드 (안정도 30일 이상)
    stable: usize,
    /// 범위별 장수. 겹치는 카드는 한 번만 센다
    scopes: [usize; 3],
    sec_per_card: f64,
}

/// H3 압축 회독을 열 수 있으면 그 계획. 목표일이 3일보다 멀거나 지났으면 없다
#[tauri::command]
pub fn cram_plan(state: State<AppState>, deck_ids: Option<Vec<i64>>) -> Res<Option<CramPlan>> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    let decks = commands::load_decks(&con)?;
    let Some(t) = target(&decks, deck_ids.as_deref(), today) else { return Ok(None) };
    let (weak_ids, weak_names) = weak_scope(t, &decks);
    let all = items(&con, &commands::descendants(t.id, &decks), &weak_ids)?;
    let count = |f: &dyn Fn(&Item) -> bool| all.iter().filter(|i| f(i)).count();
    Ok(Some(CramPlan {
        deck_id: t.id,
        name: t.name.clone(),
        dday: t.target_day.unwrap_or(today) - today,
        core: count(&|i| i.core),
        core_total: count(&|i| i.core_card),
        weak: count(&|i| i.weak && !i.stable),
        weak_names,
        again: count(&|i| i.again && !i.stable),
        stable: count(&|i| i.stable),
        scopes: [0, 1, 2].map(|s| count(&|i| i.picked(s))),
        sec_per_card: sec_per_card(&con)?,
    }))
}

/// 압축 회독할 카드 (핵심 먼저, 안정도 낮은 것 먼저)
#[tauri::command]
pub fn cram_cards(state: State<AppState>, deck_id: i64, scope: Scope) -> Res<Vec<i64>> {
    let con = state.db.lock().map_err(err)?;
    let decks = commands::load_decks(&con)?;
    let t = decks.iter().find(|d| d.id == deck_id).ok_or("덱을 찾을 수 없어요.")?;
    let (weak_ids, _) = weak_scope(t, &decks);
    Ok(items(&con, &commands::descendants(deck_id, &decks), &weak_ids)?.into_iter().filter(|i| i.picked(scope)).map(|i| i.id).collect())
}

/// 압축 회독 평가. 일정은 그대로 두고 기록만 남긴다. 되돌리기에 쓸 기록 id 를 돌려준다
#[tauri::command]
pub fn cram_answer(state: State<AppState>, card_id: i64, rating: u8, elapsed_ms: i64) -> Res<i64> {
    if !(1..=4).contains(&rating) {
        return Err("평가 값이 올바르지 않아요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "INSERT INTO review_log (card_id, reviewed_at, rating, elapsed_ms, prev_interval_days, next_interval_days, prev_state, fed_to_scheduler, kind)
         SELECT id, ?2, ?3, ?4, interval_days, interval_days, state, 0, 'cram' FROM card WHERE id = ?1",
        params![card_id, now_ms(), rating, elapsed_ms],
    )
    .map_err(err)?;
    Ok(con.last_insert_rowid())
}

/// 압축 회독 평가 되돌리기 (Ctrl+Z). 압축 회독 기록만 지운다
#[tauri::command]
pub fn cram_undo(state: State<AppState>, log_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    con.execute("DELETE FROM review_log WHERE id = ?1 AND kind = 'cram'", [log_id]).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn collects_shaky_weak_and_again_but_not_stable() {
        let con = db::open_in_memory().unwrap();
        let now = now_ms();
        let today = study_day(now);
        let exam = db::date_from_day(today + 2);
        con.execute_batch(&format!(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '시험', 0, 0);
             INSERT INTO deck (id, parent_id, name, created_at, updated_at, weak) VALUES (2, 1, '4장', 0, 0, 1);
             INSERT INTO deck (id, name, created_at, updated_at) VALUES (3, '먼 시험', 0, 0);
             INSERT INTO deck_config (deck_id, target_date, updated_at) VALUES (1, '{exam}', 0);
             INSERT INTO note (id, note_type_id, fields, created_at, updated_at) VALUES (1, 1, '[\"q\",\"a\"]', 0, 0);"
        ))
        .unwrap();
        // (id, deck, 중요도, 상태, 안정도)
        for (id, deck, imp, st, stab) in [
            (1, 1, 2, "review", 3.0),   // 불안한 핵심
            (2, 1, 2, "review", 40.0),  // 잘 외운 핵심 → 뺀다
            (3, 2, 1, "review", 10.0),  // 약한 챕터
            (4, 1, 1, "review", 12.0),  // 최근 Again
            (5, 1, 1, "review", 12.0),  // 어디에도 없음
            (6, 1, 2, "new", 0.0),      // 신규는 회독이 아니다
            (7, 3, 2, "review", 1.0),   // 다른 덱
        ] {
            con.execute(
                "INSERT INTO card (id, note_id, deck_id, template_ord, state, stability, importance, created_at, updated_at) VALUES (?1, 1, ?2, ?1, ?3, ?4, ?5, 0, 0)",
                params![id, deck, st, stab, imp],
            )
            .unwrap();
        }
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (4, ?1, 1)", [now - DAY_MS]).unwrap();

        let decks = commands::load_decks(&con).unwrap();
        let t = target(&decks, Some(&[2]), today).expect("하위 덱에서 들어와도 위의 시험 덱");
        assert_eq!(t.id, 1);
        assert!(target(&decks, Some(&[3]), today).is_none(), "목표일 없는 덱");
        let (weak, names) = weak_scope(t, &decks);
        assert_eq!(names, ["4장"]);
        let all = items(&con, &commands::descendants(1, &decks), &weak).unwrap();
        let picked = |s| all.iter().filter(|i| i.picked(s)).map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(picked(0), [1]);
        assert_eq!(picked(1), [1, 3]);
        assert_eq!(picked(2), [1, 3, 4], "핵심 먼저, 안정도 낮은 순");
        assert_eq!(all.iter().filter(|i| i.stable).count(), 1);

        // 목표일이 3일보다 멀면 열지 않는다
        con.execute("UPDATE deck_config SET target_date = ?1", [db::date_from_day(today + 4)]).unwrap();
        assert!(target(&commands::load_decks(&con).unwrap(), None, today).is_none());
    }
}
