//! 시험 전 회독 (P1). 목표일 14일 전부터 연다 (홈 · D1). 방식은 둘이고 기본은 3일 이내 압축, 그 전은 혼합.
//! H3 압축: 아직 불안한 핵심(FSRS 안정도 7일 미만) ∪ 약한 챕터 ∪ 최근 2주 Again. 잘 외운 카드(안정도 30일 이상)는 뺀다.
//!   평가는 SRS 에 넘기지 않는다: review_log 에 kind = 'cram' 으로만 남긴다 (히트맵에는 들어간다).
//!   카드 일정 · 오늘의 계획 · 합격선 지표는 그대로다 (그쪽 쿼리는 kind IS NULL 인 기록만 센다)
//! 8.5 혼합: 배운 카드 전체를 회독(덱마다 진행 저장)하며 취약 카드(압축과 같은 집합)를 사이사이 한 번 더 끼운다.
//!   평가는 일반 학습과 같은 answer 로 일정에 반영한다 (P5). 세션 mode = 'mix' 로 회독 진행을 센다.
//!   오늘 예정 카드를 여기서 보면 그게 오늘의 복습이다 (P9, kind IS NULL 이라 계획 · 지표에도 완료로 들어간다)

use crate::commands::{self, AppState};
use crate::db::{self, now_ms, study_day, study_day_end_ms, DAY_MS};
use crate::queue;
use rusqlite::{params, Connection, OptionalExtension};
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
/// 목표일 며칠 전부터 여는가 (P6)
const OPEN_DAYS: i64 = 14;
/// 이 날 수 이내면 압축이 기본, 그 전은 혼합 (P1)
const CRAM_DAYS: i64 = 3;
/// 기록이 모자랄 때 카드 1장에 잡는 시간 (plan.rs 와 같다)
const DEFAULT_SEC: f64 = 10.0;

/// 범위 세그먼트: 0 핵심만 · 1 핵심 + 약한 챕터 · 2 불안한 카드 전부
type Scope = u8;

struct Item {
    id: i64,
    note: i64,
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
                    EXISTS (SELECT 1 FROM review_log r WHERE r.card_id = c.id AND r.rating = 1 AND r.reviewed_at >= ?1), c.note_id
             FROM card_effective c WHERE c.deck_id IN ({ids}) AND c.state IN ('learning', 'review', 'relearning')
             ORDER BY c.eff_importance DESC, COALESCE(c.stability, c.interval_days), c.id"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([now_ms() - AGAIN_DAYS * DAY_MS], |r| {
            let (importance, stability, deck): (i64, f64, i64) = (r.get(1)?, r.get(2)?, r.get(3)?);
            Ok(Item {
                id: r.get(0)?,
                note: r.get(5)?,
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
    /// 기본 방식: "cram" (3일 이내) | "mix"
    mode: &'static str,
    /// 혼합 회독: 배운 카드 수 (P8 분모) · 이번 회독에서 본 카드 수
    learned: usize,
    round_seen: usize,
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
    let seen = round_seen(&con, t)?;
    let dday = t.target_day.unwrap_or(today) - today;
    Ok(Some(CramPlan {
        deck_id: t.id,
        name: t.name.clone(),
        dday,
        core: count(&|i| i.core),
        core_total: count(&|i| i.core_card),
        weak: count(&|i| i.weak && !i.stable),
        weak_names,
        again: count(&|i| i.again && !i.stable),
        stable: count(&|i| i.stable),
        scopes: [0, 1, 2].map(|s| count(&|i| i.picked(s))),
        sec_per_card: sec_per_card(&con)?,
        mode: if dday <= CRAM_DAYS { "cram" } else { "mix" },
        learned: all.len(),
        round_seen: count(&|i| seen.contains(&i.id)),
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

// ------------------------------------------------------------------ 8.5 혼합 회독

/// 카드 id 모음
fn id_set(con: &Connection, sql: &str, p: impl rusqlite::Params) -> Res<HashSet<i64>> {
    let mut st = con.prepare(sql).map_err(err)?;
    let rows = st.query_map(p, |r| r.get::<_, Option<i64>>(0)).map_err(err)?;
    Ok(rows.filter_map(|r| r.ok().flatten()).collect())
}

/// 이번 회독이 시작된 때: [처음부터 다시] 를 누른 때와 목표일 14일 전 중 늦은 쪽 (목표일이 바뀌면 새 회독, P4)
fn round_since(con: &Connection, t: &queue::Deck) -> Res<i64> {
    let restart: Option<i64> =
        con.query_row("SELECT mix_since FROM deck_config WHERE deck_id = ?1", [t.id], |r| r.get(0)).optional().map_err(err)?.flatten();
    let window = t.target_day.map_or(0, |d| commands::today_start_ms(d - OPEN_DAYS));
    Ok(restart.unwrap_or(0).max(window))
}

/// 이번 회독에서 본 카드 (혼합 세션의 기록)
fn round_seen(con: &Connection, t: &queue::Deck) -> Res<HashSet<i64>> {
    id_set(
        con,
        "SELECT DISTINCT r.card_id FROM review_log r JOIN study_session s ON s.id = r.session_id WHERE s.mode = 'mix' AND r.reviewed_at >= ?1",
        [round_since(con, t)?],
    )
}

/// P2 혼합 순서: 전체 every 장마다 취약 1장 (1/3 이면 2장마다). 취약은 전체 카드 사이에만 들어가 서로 붙지 않고,
/// 앞뒤 카드와 같은 노트(형제 · 같은 카드)면 다음 취약 카드를 쓴다. 한 번에 size 장까지. (카드, 취약 자리인가)
fn mix_order(full: &[(i64, i64)], weak: &[(i64, i64)], every: usize, size: usize) -> Vec<(i64, bool)> {
    let mut out = vec![];
    let mut pending: Vec<(i64, i64)> = weak.to_vec();
    for (i, f) in full.iter().enumerate() {
        if out.len() >= size {
            break;
        }
        out.push((f.0, false));
        if (i + 1) % every.max(1) == 0 && out.len() < size {
            let next = full.get(i + 1).map(|n| n.1);
            if let Some(k) = pending.iter().position(|w| w.1 != f.1 && Some(w.1) != next) {
                out.push((pending.remove(k).0, true));
            }
        }
    }
    out
}

#[derive(Serialize, Debug, PartialEq)]
pub struct MixCard {
    id: i64,
    /// 취약 카드로 끼운 자리면 이유 (P2 "취약 · 이유" 칩)
    weak: Option<&'static str>,
}

/// 혼합 회독 한 번 분량. 이번 회독에서 아직 안 본 배운 카드 + 취약 카드. 오늘 이미 일정에 반영한 카드와 접어둔 카드는 빼고,
/// 오늘 예정이던 카드(밀린 것 포함)를 따로 돌려준다 (여기서 보면 오늘의 학습에서 빠진다)
fn build_mix(con: &Connection, t: &queue::Deck, decks: &[queue::Deck], every: usize, size: usize, now: i64) -> Res<(Vec<MixCard>, Vec<i64>)> {
    let today = study_day(now);
    let (weak_ids, _) = weak_scope(t, decks);
    let all = items(con, &commands::descendants(t.id, decks), &weak_ids)?;
    let seen = round_seen(con, t)?;
    let skip = id_set(
        con,
        "SELECT card_id FROM review_log WHERE fed_to_scheduler = 1 AND kind IS NULL AND reviewed_at >= ?1 UNION SELECT id FROM card WHERE buried_until > ?2",
        params![commands::today_start_ms(today), now],
    )?;
    let open: Vec<&Item> = all.iter().filter(|i| !skip.contains(&i.id)).collect();
    let full: Vec<(i64, i64)> = open.iter().filter(|i| !seen.contains(&i.id)).map(|i| (i.id, i.note)).collect();
    let weak: Vec<(i64, i64)> = open.iter().filter(|i| i.picked(2)).map(|i| (i.id, i.note)).collect();
    let by_id: HashMap<i64, &&Item> = open.iter().map(|i| (i.id, i)).collect();
    let cards: Vec<MixCard> = mix_order(&full, &weak, every, size)
        .into_iter()
        .map(|(id, w)| {
            let i = by_id[&id];
            let why = if i.weak { "약한 챕터" } else if i.again { "최근 Again" } else { "불안한 핵심" };
            MixCard { id, weak: w.then_some(why) }
        })
        .collect();
    let ids = cards.iter().map(|c| c.id.to_string()).collect::<Vec<_>>().join(",");
    let due = id_set(
        con,
        &format!("SELECT id FROM card WHERE id IN ({}) AND due_at <= ?1", if ids.is_empty() { "-1" } else { &ids }),
        [study_day_end_ms(today)],
    )?;
    let mut due: Vec<i64> = due.into_iter().collect();
    due.sort();
    Ok((cards, due))
}

#[derive(Serialize)]
pub struct MixStart {
    session_id: i64,
    cards: Vec<MixCard>,
    due_today: Vec<i64>,
}

/// 혼합 회독 시작. every: 전체 몇 장마다 취약 1장 (1/5 → 4, 1/3 → 2, 1/2 → 1), size: 한 번에 할 양 (60 · 120 · 200).
/// 같은 학습일에 그만둔 혼합 세션이 있으면 이어 쓴다 (Again 횟수가 이어진다). 평가는 commands::answer 로
#[tauri::command]
pub fn mix_start(state: State<AppState>, deck_id: i64, every: usize, size: usize) -> Res<MixStart> {
    let con = state.db.lock().map_err(err)?;
    let now = now_ms();
    let decks = commands::load_decks(&con)?;
    let t = decks.iter().find(|d| d.id == deck_id).ok_or("덱을 찾을 수 없어요.")?;
    let (cards, due_today) = build_mix(&con, t, &decks, every.clamp(1, 4), size.clamp(1, 500), now)?;
    let day = db::date_from_day(study_day(now));
    let resumed: Option<i64> = con
        .query_row(
            "SELECT id FROM study_session WHERE study_day = ?1 AND mode = 'mix' AND end_reason IN ('user_exit','app_exit') ORDER BY started_at DESC LIMIT 1",
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
            con.execute("INSERT INTO study_session (started_at, study_day, mode) VALUES (?1, ?2, 'mix')", params![now, day]).map_err(err)?;
            con.last_insert_rowid()
        }
    };
    Ok(MixStart { session_id, cards, due_today })
}

/// [처음부터 다시] (P4). 이번 회독 진행을 비운다. 기록과 일정은 그대로
#[tauri::command]
pub fn mix_restart(state: State<AppState>, deck_id: i64) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "INSERT INTO deck_config (deck_id, updated_at, mix_since) VALUES (?1, ?2, ?2) ON CONFLICT (deck_id) DO UPDATE SET mix_since = ?2, updated_at = ?2",
        params![deck_id, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// P9 같은 날 두 번 반영하지 않는다: 혼합 세션의 평가는 학습 단계를 잇는 것이거나 그 카드의 오늘 첫 일정 반영 평가일 때만 일정에 반영한다.
/// 일반 학습에서 이미 봤거나 혼합에서 취약 카드로 한 번 더 볼 때는 기록만 남긴다. 일반 세션은 그대로 (commands::answer)
pub(crate) fn mix_feeds(con: &Connection, session_id: i64, card_id: i64, prev_state: &str, today: i64) -> Res<bool> {
    let mix: bool = con
        .query_row("SELECT coalesce(mode = 'mix', 0) FROM study_session WHERE id = ?1", [session_id], |r| r.get(0))
        .optional()
        .map_err(err)?
        .unwrap_or(false);
    if !mix || matches!(prev_state, "learning" | "relearning") {
        return Ok(true);
    }
    let fed: bool = con
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM review_log WHERE card_id = ?1 AND fed_to_scheduler = 1 AND kind IS NULL AND reviewed_at >= ?2)",
            params![card_id, commands::today_start_ms(today)],
            |r| r.get(0),
        )
        .map_err(err)?;
    Ok(!fed)
}

#[derive(Serialize)]
pub struct HotDeck {
    deck_id: i64,
    /// 대상 덱 아래 경로 ("4장 · 회귀")
    name: String,
    again: i64,
}

#[derive(Serialize)]
pub struct MixSummary {
    /// 일정에 반영된 평가 수
    fed: i64,
    learned: usize,
    round_seen: usize,
    /// 이번 세션의 Again 이 한 하위 덱에 몰렸으면 그 덱 (P7 약한 챕터 제안. 이미 약한 챕터면 없다)
    hot: Option<HotDeck>,
}

/// Again 이 몰렸다고 보는 기준: 세션 Again 의 절반 이상이고 3번 이상
const HOT_MIN: i64 = 3;

/// P3 끝난 뒤 요약 (세션 닫기와 접어둔 카드는 finish_session 이 한다)
#[tauri::command]
pub fn mix_summary(state: State<AppState>, session_id: i64, deck_id: i64) -> Res<MixSummary> {
    let con = state.db.lock().map_err(err)?;
    let decks = commands::load_decks(&con)?;
    let t = decks.iter().find(|d| d.id == deck_id).ok_or("덱을 찾을 수 없어요.")?;
    summary(&con, session_id, t, &decks)
}

fn summary(con: &Connection, session_id: i64, t: &queue::Deck, decks: &[queue::Deck]) -> Res<MixSummary> {
    let fed: i64 = con
        .query_row("SELECT count(*) FROM review_log WHERE session_id = ?1 AND fed_to_scheduler = 1", [session_id], |r| r.get(0))
        .map_err(err)?;
    let (weak_ids, _) = weak_scope(t, decks);
    let all = items(con, &commands::descendants(t.id, decks), &weak_ids)?;
    let seen = round_seen(con, t)?;
    let mut st = con
        .prepare("SELECT c.deck_id, count(*) FROM review_log r JOIN card c ON c.id = r.card_id WHERE r.session_id = ?1 AND r.rating = 1 GROUP BY c.deck_id ORDER BY 2 DESC, 1")
        .map_err(err)?;
    let again: Vec<(i64, i64)> = st.query_map([session_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    let total: i64 = again.iter().map(|a| a.1).sum();
    let by_id: HashMap<i64, &queue::Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let depth = commands::path_of(t.id, &by_id).len();
    let hot = again.first().filter(|(d, n)| *n >= HOT_MIN && n * 2 >= total && !weak_ids.contains(d) && *d != t.id).map(|(d, n)| HotDeck {
        deck_id: *d,
        name: commands::path_of(*d, &by_id).get(depth..).unwrap_or_default().join(" · "),
        again: *n,
    });
    Ok(MixSummary { fed, learned: all.len(), round_seen: all.iter().filter(|i| seen.contains(&i.id)).count(), hot })
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

        // 목표일이 14일보다 멀면 열지 않는다
        con.execute("UPDATE deck_config SET target_date = ?1", [db::date_from_day(today + 15)]).unwrap();
        assert!(target(&commands::load_decks(&con).unwrap(), None, today).is_none());
    }

    #[test]
    fn mix_order_spreads_weak_cards() {
        // (카드, 노트). 전체 2장마다 취약 1장, 취약끼리 붙지 않고 앞뒤와 같은 노트면 다음 취약 카드
        let full = [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6)];
        let weak = [(3, 3), (9, 9), (1, 1)];
        let ids = |v: Vec<(i64, bool)>| v.iter().map(|c| if c.1 { -c.0 } else { c.0 }).collect::<Vec<_>>();
        // 2 다음: 3 은 바로 뒤 카드와 같은 노트라 9 를 먼저
        assert_eq!(ids(mix_order(&full, &weak, 2, 100)), [1, 2, -9, 3, 4, -3, 5, 6, -1]);
        assert_eq!(ids(mix_order(&full, &weak, 4, 100)), [1, 2, 3, 4, -3, 5, 6]);
        assert_eq!(ids(mix_order(&full, &weak, 1, 5)), [1, -3, 2, -9, 3]);
        assert!(mix_order(&[], &weak, 2, 100).is_empty(), "전체를 다 돌면 취약만 남기지 않는다");
    }

    #[test]
    fn mix_batch_round_progress_and_same_day_rule() {
        let con = db::open_in_memory().unwrap();
        let now = now_ms();
        let today = study_day(now);
        con.execute_batch(&format!(
            "INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '시험', 0, 0);
             INSERT INTO deck (id, parent_id, name, created_at, updated_at, weak) VALUES (2, 1, '4장', 0, 0, 1);
             INSERT INTO deck (id, parent_id, name, created_at, updated_at) VALUES (3, 1, '5장', 0, 0);
             INSERT INTO deck_config (deck_id, target_date, updated_at) VALUES (1, '{}', 0);",
            db::date_from_day(today + 10)
        ))
        .unwrap();
        // (id, 덱, 중요도, 상태, 안정도, 예정일). 노트는 카드마다 하나
        for (id, deck, imp, st, stab, due) in [
            (1, 1, 2, "review", 3.0, today),     // 불안한 핵심 · 오늘 예정
            (2, 1, 1, "review", 40.0, today + 9), // 잘 외운 카드도 전체 범위에는 든다
            (3, 2, 1, "review", 10.0, today + 2), // 약한 챕터
            (4, 3, 1, "review", 12.0, today + 3),
            (5, 1, 2, "new", 0.0, today),         // 신규는 혼합에 넣지 않는다 (P8)
        ] {
            con.execute("INSERT INTO note (id, note_type_id, fields, created_at, updated_at) VALUES (?1, 1, '[]', 0, 0)", [id]).unwrap();
            con.execute(
                "INSERT INTO card (id, note_id, deck_id, template_ord, state, stability, importance, due_at, created_at, updated_at)
                 VALUES (?1, ?1, ?2, 0, ?3, ?4, ?5, ?6, 0, 0)",
                params![id, deck, st, stab, imp, study_day_end_ms(due) - 1000],
            )
            .unwrap();
        }
        let decks = commands::load_decks(&con).unwrap();
        let t = &decks[0];
        let (cards, due) = build_mix(&con, t, &decks, 2, 100, now).unwrap();
        let w = |id, why| MixCard { id, weak: Some(why) };
        let f = |id| MixCard { id, weak: None };
        assert_eq!(cards, [f(1), f(3), w(1, "불안한 핵심"), f(4), f(2), w(3, "약한 챕터")]);
        assert_eq!(due, [1]);

        // 혼합 세션에서 카드 1 을 처음 보면 일정에 반영, 같은 날 다시 보면(취약 자리) 기록만. 학습 단계를 잇는 평가는 반영
        con.execute("INSERT INTO study_session (id, started_at, study_day, mode) VALUES (7, ?1, 'x', 'mix')", [now]).unwrap();
        con.execute("INSERT INTO study_session (id, started_at, study_day) VALUES (8, ?1, 'x')", [now]).unwrap();
        assert!(mix_feeds(&con, 7, 1, "review", today).unwrap());
        con.execute("INSERT INTO review_log (card_id, session_id, reviewed_at, rating) VALUES (1, 7, ?1, 3)", [now]).unwrap();
        assert!(!mix_feeds(&con, 7, 1, "review", today).unwrap());
        assert!(mix_feeds(&con, 7, 1, "relearning", today).unwrap());
        assert!(mix_feeds(&con, 8, 1, "review", today).unwrap(), "일반 세션은 그대로");

        // 회독 진행: 본 카드는 다음 분량의 전체 범위에서 빠지고, [처음부터 다시] 뒤에는 다시 든다 (오늘 반영한 카드는 오늘은 빼고)
        assert_eq!(round_seen(&con, t).unwrap(), HashSet::from([1]));
        let ids = |cards: Vec<MixCard>| cards.iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(build_mix(&con, t, &decks, 2, 100, now).unwrap().0), [3, 4, 3, 2]);
        con.execute("INSERT INTO deck_config (deck_id, updated_at, mix_since) VALUES (1, 0, ?1) ON CONFLICT (deck_id) DO UPDATE SET mix_since = ?1", [now + 1])
            .unwrap();
        assert!(round_seen(&con, t).unwrap().is_empty());

        // 끝난 뒤: Again 이 약하지 않은 한 하위 덱에 몰리면 그 덱 (3번 이상, 절반 이상)
        for _ in 0..3 {
            con.execute("INSERT INTO review_log (card_id, session_id, reviewed_at, rating, fed_to_scheduler) VALUES (4, 7, ?1, 1, 0)", [now]).unwrap();
        }
        let s = summary(&con, 7, t, &decks).unwrap();
        assert_eq!((s.fed, s.learned, s.hot.as_ref().map(|h| (h.deck_id, h.name.as_str(), h.again))), (1, 4, Some((3, "5장", 3))));
    }
}
