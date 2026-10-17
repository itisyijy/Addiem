//! H1 학습 계획 · H2 목표일 충돌. 큐 규칙(queue.rs)으로 앞으로의 하루 분량을 내다본다.
//! 예상 성적 · 합격 가능성 · 준비도 점수는 만들지 않는다 (MVP 9장 Out). 남은 양 · 날짜 · 시간만 보여준다.
//! 목표일은 절대 바꾸지 않는다. 바꾸는 것은 신규를 나누는 방식(queue::Deck 의 new_* 필드)뿐이다

use crate::commands::{self, AppState};
use crate::db::{self, now_ms, study_day, DAY_MS};
use crate::queue::{self, Card, Deck};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 신규 카드를 처음 본 뒤 다시 나오는 날 (Good 기준 FSRS 첫 간격의 근사).
/// ponytail: 고정 근사다. 카드별 안정도로 내다보려면 날마다 srs 를 돌려야 하고, 계획 화면에는 이 정도면 충분하다
/// FSRS 기본값에서 처음 Good 의 안정도가 약 2.3일, 그 뒤 대략 세 배씩 늘어난다
const NEW_REVIEW_OFFSETS: [i64; 3] = [2, 8, 25];
const PAST_DAYS: i64 = 7;
/// 그래프가 담는 앞날 (그 뒤는 잘린다)
const MAX_FUTURE: i64 = 42;
/// 목표일이 없을 때 보여주는 앞날
const NO_TARGET_DAYS: i64 = 14;
/// 기록이 모자랄 때 카드 1장에 잡는 시간
const DEFAULT_SEC: f64 = 10.0;
/// H2: 두 목표일이 이 날수 안이면 "가깝다"
const CONFLICT_GAP: i64 = 7;

// ------------------------------------------------------------------ 큐가 따르는 조정 (load_decks 가 부른다)

/// 충돌 조정 · 오늘 당겨 맞추기 · 하루 몫 고정을 덱에 싣는다. 목표일은 그대로 둔다
pub(crate) fn adjust_decks(con: &Connection, decks: &mut [Deck], today: i64) -> Res<()> {
    let pairs = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Res<HashMap<i64, i64>> {
        let mut st = con.prepare(sql).map_err(err)?;
        let rows = st.query_map(p, |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)
    };
    let fixed = pairs("SELECT deck_id, fixed_new_per_day FROM deck_config WHERE fixed_new_per_day IS NOT NULL", &[])?;
    let shift = pairs("SELECT deck_id, sum(delta) FROM plan_shift WHERE day = ?1 GROUP BY deck_id", &[&db::date_from_day(today)])?;
    for d in decks.iter_mut() {
        d.fixed_new = fixed.get(&d.id).copied();
        d.new_shift = shift.get(&d.id).copied().unwrap_or(0);
    }
    let mut st = con.prepare("SELECT deck_a, deck_b, mode FROM plan_conflict").map_err(err)?;
    let rows: Vec<(i64, i64, String)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    for (a, b, mode) in rows {
        let Some(ta) = decks.iter().find(|d| d.id == a).and_then(|d| d.target_day) else { continue };
        if let Some(d) = decks.iter_mut().find(|d| d.id == b) {
            apply_mode(d, &mode, ta);
        }
    }
    Ok(())
}

/// H2 선택을 늦은 시험 덱(b)에 싣는다. ta 는 먼저 오는 시험일
fn apply_mode(b: &mut Deck, mode: &str, ta: i64) {
    match mode {
        // 앞당겨 나누기: 먼저 오는 시험 전에 신규를 다 넣는다
        "spread" => b.new_until = Some(ta),
        // 먼저 볼 덱: 먼저 오는 시험이 끝난 다음 날부터 신규를 넣는다
        "first" => b.new_from = Some(ta + 1),
        _ => {}
    }
}

// ------------------------------------------------------------------ 내다보기 (순수 함수)

#[derive(Default, Clone, Debug)]
pub(crate) struct DayLoad {
    pub review: i64,
    pub new: i64,
    /// 최상위 덱별 (복습 + 신규)
    pub by_root: HashMap<i64, i64>,
    /// 예산 덱별 신규
    pub new_by_owner: HashMap<i64, i64>,
}

impl DayLoad {
    pub fn total(&self) -> i64 {
        self.review + self.new
    }
}

/// d 날에 이 덱이(조상 포함) 쉬고 있는가
fn paused_on(id: i64, d: i64, by_id: &HashMap<i64, &Deck>) -> bool {
    let mut cur = by_id.get(&id).copied();
    for _ in 0..64 {
        let Some(x) = cur else { return false };
        if x.paused && x.paused_until.is_none_or(|u| d <= u) {
            return true;
        }
        cur = x.parent_id.and_then(|p| by_id.get(&p).copied());
    }
    false
}

/// today 부터 n 일 동안 날마다의 분량. 복습 = 예정된 것 + 밀린 것(하루 200장씩) + 이 계획의 신규가 다시 나오는 것.
/// 신규 = 큐와 같은 산식(new_target_for_deck)을 날마다 남은 장수로 다시 돌린 것.
/// shifts: (예산 덱, 날) → 당겨 맞추기. done_new: 예산 덱별 오늘 이미 넣은 신규 (큐처럼 오늘 몫에서 뺀다).
/// backlog_done: 오늘 이미 본 밀린 복습 (하루 200장 상한에서 뺀다)
#[allow(clippy::too_many_arguments)]
pub(crate) fn project(
    today: i64,
    n: i64,
    decks: &[Deck],
    cards: &[Card],
    scope: &HashSet<i64>,
    shifts: &HashMap<(i64, i64), i64>,
    done_new: &HashMap<i64, i64>,
    backlog_done: i64,
) -> Vec<DayLoad> {
    let n = n.max(1) as usize;
    let by_id: HashMap<i64, &Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let root = |id: i64| commands::root_of(id, &by_id);
    let mut rev: Vec<HashMap<i64, i64>> = vec![HashMap::new(); n];
    // 밀린 복습은 큐처럼 모든 덱에서 오래된 것부터 하루 200장 (상한은 함께 쓰고, 범위 안의 것만 센다)
    let mut backlog: Vec<(i64, i64, i64, i64, bool)> = vec![];
    let mut remaining: HashMap<i64, i64> = HashMap::new();
    for c in cards.iter().filter(|c| by_id.contains_key(&c.deck_id) && c.plays()) {
        let mine = scope.contains(&c.deck_id);
        if c.state == "new" {
            if mine {
                *remaining.entry(queue::budget_owner(c.deck_id, &by_id)).or_default() += 1;
            }
            continue;
        }
        let due = c.due_day.unwrap_or(today).max(c.buried_until.unwrap_or(i64::MIN));
        if due < today {
            backlog.push((due, -c.importance, c.position, root(c.deck_id), mine));
        } else if let Some(slot) = rev.get_mut((due - today) as usize).filter(|_| mine) {
            *slot.entry(root(c.deck_id)).or_default() += 1;
        }
    }
    let mut owners: Vec<i64> = remaining.keys().copied().collect();
    owners.sort();
    backlog.sort();
    let mut next = 0;
    let mut out = vec![];
    for i in 0..n {
        let d = today + i as i64;
        let mut day = DayLoad::default();
        let cap = queue::BACKLOG_DAILY_CAP as i64 - if i == 0 { backlog_done } else { 0 };
        let end = (next + cap.max(0) as usize).min(backlog.len());
        for &(_, _, _, r, mine) in &backlog[next..end] {
            if mine {
                *day.by_root.entry(r).or_default() += 1;
                day.review += 1;
            }
        }
        next = end;
        for (r, k) in &rev[i] {
            *day.by_root.entry(*r).or_default() += k;
            day.review += k;
        }
        for &owner in &owners {
            let deck = by_id[&owner];
            let left = remaining[&owner];
            if left == 0 || paused_on(owner, d, &by_id) {
                continue;
            }
            // 오늘은 큐처럼: 하루 시작 기준으로 계산하고 이미 넣은 만큼 뺀다
            let done = if i == 0 { done_new.get(&owner).copied().unwrap_or(0) } else { 0 };
            let mut target = match deck.today_new {
                Some(t) if i == 0 => t.min(left + done),
                _ => queue::new_target_for_deck(deck, d, left + done).0,
            };
            target += shifts.get(&(owner, d)).copied().unwrap_or(0);
            let k = (target - done).clamp(0, left);
            if k == 0 {
                continue;
            }
            remaining.insert(owner, left - k);
            day.new += k;
            *day.new_by_owner.entry(owner).or_default() += k;
            *day.by_root.entry(root(owner)).or_default() += k;
            for off in NEW_REVIEW_OFFSETS {
                if let Some(slot) = rev.get_mut(i + off as usize) {
                    *slot.entry(root(owner)).or_default() += k;
                }
            }
        }
        out.push(day);
    }
    out
}

/// 0 = 일요일 (1970-01-01 은 목요일)
fn weekday(d: i64) -> i64 {
    (d + 4).rem_euclid(7)
}

fn is_weekend(d: i64) -> bool {
    matches!(weekday(d), 0 | 6)
}

/// 하루 상한 (쓸 수 있는 시간 ÷ 카드당 시간). 시간을 넣지 않았으면 None
fn cap_for(d: i64, time: &Time) -> Option<i64> {
    let min = if is_weekend(d) { time.weekend_min } else { time.weekday_min }?;
    Some(((min * 60) as f64 / time.sec_per_card).floor() as i64)
}

/// d 날이 상한을 넘으면, 그 전의 가장 가까운 여유 있는 날로 신규를 얼마나 옮기면 되는지.
/// (넘는 날, 옮길 날, 장수, 예산 덱). 옮길 신규가 없으면 장수 0
pub(crate) fn find_over(today: i64, days: &[DayLoad], cap: impl Fn(i64) -> Option<i64>) -> Option<(i64, Option<i64>, i64, i64)> {
    let (i, over) = days.iter().enumerate().find_map(|(i, x)| {
        let c = cap(today + i as i64)?;
        (x.total() > c).then_some((i, x.total() - c))
    })?;
    let d = today + i as i64;
    let Some((&owner, &new)) = days[i].new_by_owner.iter().max_by_key(|(id, k)| (**k, -**id)) else {
        return Some((d, None, 0, 0));
    };
    for j in (0..i).rev() {
        let slack = cap(today + j as i64).map_or(0, |c| c - days[j].total());
        if slack > 0 {
            return Some((d, Some(today + j as i64), over.min(slack).min(new), owner));
        }
    }
    Some((d, None, 0, owner))
}

// ------------------------------------------------------------------ DB 에서 읽기

struct Time {
    weekday_min: Option<i64>,
    weekend_min: Option<i64>,
    sec_per_card: f64,
    measured: bool,
}

fn read_time(con: &Connection) -> Res<Time> {
    let get = |k: &str| -> Res<Option<i64>> {
        con.query_row("SELECT value FROM plan_setting WHERE key = ?1", [k], |r| r.get::<_, String>(0))
            .optional()
            .map(|v| v.and_then(|v| v.parse().ok()))
            .map_err(err)
    };
    // 최근 14일 실측. 자리를 비운 기록이 평균을 끌어올리지 않게 1~60초로 자르고 중앙값을 쓴다
    let mut st = con.prepare("SELECT elapsed_ms FROM review_log WHERE reviewed_at >= ?1 AND elapsed_ms > 0 AND kind IS NULL").map_err(err)?;
    let mut secs: Vec<f64> = st
        .query_map([now_ms() - 14 * DAY_MS], |r| r.get::<_, i64>(0))
        .map_err(err)?
        .filter_map(Result::ok)
        .map(|ms| (ms as f64 / 1000.0).clamp(1.0, 60.0))
        .collect();
    secs.sort_by(f64::total_cmp);
    let measured = secs.len() >= 20;
    let sec_per_card = if measured { secs[secs.len() / 2] } else { DEFAULT_SEC };
    Ok(Time { weekday_min: get("weekday_min")?, weekend_min: get("weekend_min")?, sec_per_card, measured })
}

fn read_shifts(con: &Connection, today: i64) -> Res<HashMap<(i64, i64), i64>> {
    // 지난 날의 당겨 맞추기는 더 쓰지 않는다
    con.execute("DELETE FROM plan_shift WHERE day < ?1", [db::date_from_day(today)]).map_err(err)?;
    let mut st = con.prepare("SELECT deck_id, day, sum(delta) FROM plan_shift GROUP BY deck_id, day").map_err(err)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
        .map_err(err)?
        .filter_map(Result::ok)
        .filter_map(|(id, day, k)| db::day_from_date(&day).map(|d| ((id, d), k)));
    Ok(rows.collect())
}

/// 예산 덱별 오늘 이미 넣은 신규
fn done_new_by_owner(con: &Connection, today: i64, decks: &[Deck]) -> Res<HashMap<i64, i64>> {
    let by_id: HashMap<i64, &Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let mut out = HashMap::new();
    for (deck, k) in commands::introduced_today(con, today)? {
        *out.entry(queue::budget_owner(deck, &by_id)).or_default() += k;
    }
    Ok(out)
}

/// 오늘 이미 본 밀린 복습. 큐(commands::today_progress)와 같은 값이어야 오늘 막대가 큐와 맞는다 (그래서 kind 로 거르지 않는다)
fn backlog_done(con: &Connection, today: i64) -> Res<i64> {
    con.query_row("SELECT count(DISTINCT card_id) FROM review_log WHERE overdue = 1 AND reviewed_at >= ?1", [commands::today_start_ms(today)], |r| r.get(0))
        .map_err(err)
}

/// 지난 날 실제로 본 카드 수 (학습일별)
fn done_by_day(con: &Connection, scope: &HashSet<i64>, from: i64) -> Res<HashMap<i64, i64>> {
    let mut st = con
        .prepare("SELECT r.reviewed_at, r.card_id, c.deck_id FROM review_log r JOIN card c ON c.id = r.card_id WHERE r.reviewed_at >= ?1 AND r.kind IS NULL")
        .map_err(err)?;
    let rows = st
        .query_map([commands::today_start_ms(from)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
        .map_err(err)?;
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    let mut out: HashMap<i64, i64> = HashMap::new();
    for (at, card, deck) in rows.filter_map(Result::ok) {
        if scope.contains(&deck) && seen.insert((study_day(at), card)) {
            *out.entry(study_day(at)).or_default() += 1;
        }
    }
    Ok(out)
}

fn scope_of(deck_id: Option<i64>, decks: &[Deck]) -> HashSet<i64> {
    match deck_id {
        Some(id) => commands::descendants(id, decks).into_iter().collect(),
        None => decks.iter().map(|d| d.id).collect(),
    }
}

// ------------------------------------------------------------------ H1 학습 계획

#[derive(Serialize)]
pub struct PlanBar {
    date: String,
    weekend: bool,
    /// past | today | future | exam
    kind: &'static str,
    review: i64,
    new: i64,
    /// 지난 날 실제로 본 카드
    done: i64,
    /// 이날의 장수 상한 (시간을 넣었을 때)
    cap: Option<i64>,
}

#[derive(Serialize)]
pub struct Exam {
    date: String,
    deck_id: i64,
    name: String,
}

#[derive(Serialize)]
pub struct Over {
    date: String,
    weekend: bool,
    total: i64,
    cap: i64,
    /// 신규를 옮길 날. 옮길 신규나 여유 있는 날이 없으면 null
    to_date: Option<String>,
    to_weekend: bool,
    n: i64,
    /// 당긴 뒤 그날 예정 (다시 내다본 값)
    after: i64,
    deck_id: i64,
}

#[derive(Serialize)]
pub struct StudyPlan {
    bars: Vec<PlanBar>,
    exams: Vec<Exam>,
    /// 덱 계획일 때 그 덱의 시험 (목표일, 없으면 상위 덱 목표)
    target_date: Option<String>,
    days_left: Option<i64>,
    remaining_new: i64,
    sec_per_card: f64,
    measured: bool,
    weekday_min: Option<i64>,
    weekend_min: Option<i64>,
    weekday_cap: Option<i64>,
    weekend_cap: Option<i64>,
    over: Option<Over>,
    /// "실제 속도에 맞춰 다시 나누기"로 하루 몫이 바뀐 이유 한 줄
    pace: Option<String>,
    /// 다시 나누기 (덱 계획 · 목표일이 있을 때만). 끄면 하루 몫 고정
    recompute: Option<bool>,
    /// 목표일이 가까운 두 덱이 있어 H2 조정을 볼 수 있다
    has_conflict: bool,
}

#[tauri::command]
pub fn study_plan(state: State<AppState>, deck_id: Option<i64>) -> Res<StudyPlan> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    let decks = commands::load_decks(&con)?;
    let cards = commands::load_cards(&con)?;
    let by_id: HashMap<i64, &Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let scope = scope_of(deck_id, &decks);
    let time = read_time(&con)?;

    // 시험: 덱 계획이면 그 덱(없으면 가장 가까운 상위 덱) 목표일, 모든 덱이면 최상위 덱들의 다가오는 목표일
    let mut exams: Vec<Exam> = match deck_id {
        Some(id) => {
            let mut cur = by_id.get(&id).copied();
            let mut found = None;
            while let Some(d) = cur {
                if let Some(t) = d.target_day {
                    found = Some(Exam { date: db::date_from_day(t), deck_id: d.id, name: d.name.clone() });
                    break;
                }
                cur = d.parent_id.and_then(|p| by_id.get(&p).copied());
            }
            found.into_iter().collect()
        }
        None => decks
            .iter()
            .filter(|d| d.parent_id.is_none())
            .filter_map(|d| d.target_day.map(|t| Exam { date: db::date_from_day(t), deck_id: d.id, name: d.name.clone() }))
            .collect(),
    };
    exams.retain(|e| db::day_from_date(&e.date).is_some_and(|t| t >= today));
    exams.sort_by(|a, b| a.date.cmp(&b.date));
    let last_exam = exams.iter().filter_map(|e| db::day_from_date(&e.date)).max();
    let future = last_exam.map_or(NO_TARGET_DAYS, |t| t - today + 1).clamp(1, MAX_FUTURE);

    let shifts = read_shifts(&con, today)?;
    let (done_new, backlog_seen) = (done_new_by_owner(&con, today, &decks)?, backlog_done(&con, today)?);
    let days = project(today, future, &decks, &cards, &scope, &shifts, &done_new, backlog_seen);
    let done = done_by_day(&con, &scope, today - PAST_DAYS)?;
    let exam_days: HashSet<i64> = exams.iter().filter_map(|e| db::day_from_date(&e.date)).collect();
    let mut bars: Vec<PlanBar> = (today - PAST_DAYS..today)
        .map(|d| PlanBar {
            date: db::date_from_day(d),
            weekend: is_weekend(d),
            kind: "past",
            review: 0,
            new: 0,
            done: done.get(&d).copied().unwrap_or(0),
            cap: cap_for(d, &time),
        })
        .collect();
    for (i, x) in days.iter().enumerate() {
        let d = today + i as i64;
        bars.push(PlanBar {
            date: db::date_from_day(d),
            weekend: is_weekend(d),
            kind: if i == 0 { "today" } else if exam_days.contains(&d) { "exam" } else { "future" },
            review: x.review,
            new: x.new,
            done: if i == 0 { done.get(&d).copied().unwrap_or(0) } else { 0 },
            cap: cap_for(d, &time),
        });
    }
    let over = find_over(today, &days, |d| cap_for(d, &time)).map(|(d, to, n, owner)| {
        let total = days[(d - today) as usize].total();
        // 당긴 뒤를 다시 내다본다. 당긴 신규가 복습으로 돌아와 그날이 오히려 늘면 권하지 않는다
        let after = to.filter(|_| n > 0).map(|to| {
            let mut s = shifts.clone();
            *s.entry((owner, d)).or_default() -= n;
            *s.entry((owner, to)).or_default() += n;
            project(today, future, &decks, &cards, &scope, &s, &done_new, backlog_seen)[(d - today) as usize].total()
        });
        let to = to.filter(|_| after.is_some_and(|a| a < total));
        Over {
            date: db::date_from_day(d),
            weekend: is_weekend(d),
            total,
            cap: cap_for(d, &time).unwrap_or(0),
            to_date: to.map(db::date_from_day),
            to_weekend: to.is_some_and(is_weekend),
            n: if to.is_some() { n } else { 0 },
            after: after.filter(|_| to.is_some()).unwrap_or(total),
            deck_id: owner,
        }
    });
    let remaining_new = cards.iter().filter(|c| scope.contains(&c.deck_id) && c.plays() && c.state == "new").count() as i64;
    let target = deck_id.and_then(|_| exams.first()).and_then(|e| db::day_from_date(&e.date));
    let owner_deck = deck_id.map(|id| queue::budget_owner(id, &by_id)).and_then(|o| by_id.get(&o).copied());
    let recompute = owner_deck.filter(|d| d.target_day.is_some()).map(|d| d.fixed_new.is_none());
    let pace = match (deck_id, recompute) {
        // daily_plan 은 최상위 덱 단위로만 남는다. H2 조정이 걸린 덱은 바뀐 이유가 충돌 조정이라 속도 탓으로 말하지 않는다
        (Some(id), Some(true)) if by_id.get(&id).is_some_and(|d| d.parent_id.is_none() && d.new_from.is_none() && d.new_until.is_none()) => pace_line(&con, id, today, &decks, days.first().map_or(0, |x| x.new), target.map(|t| t - today))?,
        _ => None,
    };
    Ok(StudyPlan {
        bars,
        target_date: target.map(db::date_from_day),
        days_left: target.map(|t| t - today),
        exams,
        remaining_new,
        sec_per_card: (time.sec_per_card * 10.0).round() / 10.0,
        measured: time.measured,
        weekday_min: time.weekday_min,
        weekend_min: time.weekend_min,
        weekday_cap: time.weekday_min.map(|m| ((m * 60) as f64 / time.sec_per_card) as i64),
        weekend_cap: time.weekend_min.map(|m| ((m * 60) as f64 / time.sec_per_card) as i64),
        over,
        pace,
        recompute,
        has_conflict: find_conflicts(&con, today, &decks, &cards, &time, true)?.into_iter().next().is_some(),
    })
}

/// 지난 7일 계획 대비 한 비율과, 밀린 신규를 남은 날에 다시 나눠 하루 몫이 바뀐 이유.
/// 바뀌지 않았거나 기록(daily_plan)이 없으면 None
fn pace_line(con: &Connection, deck_id: i64, today: i64, decks: &[Deck], now_new: i64, days_left: Option<i64>) -> Res<Option<String>> {
    let mut st = con
        .prepare("SELECT study_day, planned_review + planned_new, planned_new FROM daily_plan WHERE deck_id = ?1 AND study_day >= ?2 AND study_day < ?3 ORDER BY study_day")
        .map_err(err)?;
    let rows: Vec<(String, i64, i64)> = st
        .query_map(params![deck_id, db::date_from_day(today - 7), db::date_from_day(today)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    // 비교는 가장 최근(어제 쪽) 계획의 하루 신규와
    let Some(last) = rows.last() else { return Ok(None) };
    let scope: HashSet<i64> = commands::descendants(deck_id, decks).into_iter().collect();
    let done = done_by_day(con, &scope, today - 7)?;
    // 날별로 넣은 신규
    let mut st = con
        .prepare("SELECT r.reviewed_at, c.id, c.deck_id FROM review_log r JOIN card c ON c.id = r.card_id WHERE r.prev_state = 'new' AND coalesce(r.from_buried, 0) = 0 AND r.reviewed_at >= ?1 AND r.kind IS NULL")
        .map_err(err)?;
    let mut new_done: HashMap<i64, HashSet<i64>> = HashMap::new();
    for (at, card, deck) in st
        .query_map([commands::today_start_ms(today - 7)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
        .map_err(err)?
        .filter_map(Result::ok)
    {
        if scope.contains(&deck) {
            new_done.entry(study_day(at)).or_default().insert(card);
        }
    }
    let (mut planned, mut did, mut behind) = (0, 0, 0);
    for (day, total, new) in &rows {
        let d = db::day_from_date(day).unwrap_or(today);
        planned += total;
        did += done.get(&d).copied().unwrap_or(0);
        behind += (new - new_done.get(&d).map_or(0, |s| s.len() as i64)).max(0);
    }
    let was = last.2;
    if planned == 0 || was == now_new {
        return Ok(None);
    }
    let pct = (did * 100 / planned).min(100);
    let left = days_left.map_or(String::new(), |d| format!(" 남은 {d}일에"));
    // 밀린 장수로 설명되는 변화일 때만 "밀린 N장을" (목표일을 바꾼 것 같은 다른 이유를 밀린 탓으로 말하지 않는다)
    Ok(Some(if behind > 0 && (now_new - was).abs() <= behind {
        format!("지난 7일 동안 계획의 {pct}%를 했어요. 밀린 {behind}장을{left} 다시 나눠서 하루 신규가 {was}장 → {now_new}장이 됐어요.")
    } else {
        format!("지난 7일 동안 계획의 {pct}%를 했어요. 남은 신규를{left} 다시 나눠서 하루 신규가 {was}장 → {now_new}장이 됐어요.")
    }))
}

/// 쓸 수 있는 시간 (분). None 이면 지운다 (점선 · 안내가 사라진다)
#[tauri::command]
pub fn set_study_time(state: State<AppState>, weekday_min: Option<i64>, weekend_min: Option<i64>) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    for (k, v) in [("weekday_min", weekday_min), ("weekend_min", weekend_min)] {
        match v {
            Some(m) if (0..=24 * 60).contains(&m) => {
                con.execute("INSERT INTO plan_setting (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2", params![k, m.to_string()])
            }
            Some(_) => return Err("시간은 0분에서 24시간 사이로 정해 주세요.".into()),
            None => con.execute("DELETE FROM plan_setting WHERE key = ?1", [k]),
        }
        .map_err(err)?;
    }
    Ok(())
}

/// [N일로 당겨 맞추기]: from 날의 신규 n 장을 to 날로 옮긴다 (예산 덱 기준). 큐는 그날 몫에서 더하고 뺀다
#[tauri::command]
pub fn pull_plan(state: State<AppState>, deck_id: i64, from_date: String, to_date: String, n: i64) -> Res<()> {
    db::day_from_date(&from_date).ok_or("날짜 형식이 올바르지 않아요.")?;
    db::day_from_date(&to_date).ok_or("날짜 형식이 올바르지 않아요.")?;
    if n <= 0 {
        return Err("옮길 카드가 없어요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    for (day, delta) in [(&from_date, -n), (&to_date, n)] {
        con.execute("INSERT INTO plan_shift (deck_id, day, delta, created_at) VALUES (?1, ?2, ?3, ?4)", params![deck_id, day, delta, now_ms()])
            .map_err(err)?;
    }
    Ok(())
}

/// "실제 속도에 맞춰 다시 나누기". 끄면 지금의 하루 몫을 고정해 둔다 (C4 에서 정한 몫을 그대로 쓴다)
#[tauri::command]
pub fn set_recompute(state: State<AppState>, deck_id: i64, on: bool) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    let decks = commands::load_decks(&con)?;
    let by_id: HashMap<i64, &Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let owner = queue::budget_owner(deck_id, &by_id);
    let fixed = if on {
        None
    } else {
        let deck = Deck { fixed_new: None, new_shift: 0, ..(*by_id.get(&owner).ok_or("덱을 찾을 수 없어요.")?).clone() };
        let ids = commands::descendants(owner, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let left = crate::notetype::new_left(&con, &ids)?;
        Some(queue::new_target_for_deck(&deck, study_day(now_ms()), left).0)
    };
    con.execute(
        "INSERT INTO deck_config (deck_id, fixed_new_per_day, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(deck_id) DO UPDATE SET fixed_new_per_day = ?2, updated_at = ?3",
        params![owner, fixed, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

// ------------------------------------------------------------------ H2 목표일 충돌

#[derive(Serialize, Clone)]
pub struct ConflictDeck {
    id: i64,
    name: String,
    target: String,
}

#[derive(Serialize, Clone)]
pub struct ConflictBar {
    date: String,
    a: i64,
    b: i64,
    other: i64,
    /// 이날 넘는 기준선
    limit: i64,
}

#[derive(Serialize, Clone)]
pub struct Conflict {
    /// 시험이 먼저인 덱
    a: ConflictDeck,
    b: ConflictDeck,
    peak_date: String,
    peak: i64,
    /// 기준선: 쓸 수 있는 시간이면 평일 상한, 아니면 최근 30일 평균의 1.5배
    limit: i64,
    by_time: bool,
    bars: Vec<ConflictBar>,
    /// 앞당겨 나누기: 최고 분량, b 의 하루 신규가 오늘 몇 장 늘어나는지
    spread_peak: i64,
    spread_extra: i64,
    /// 먼저 볼 덱(a): 최고 분량, a 시험 뒤 b 신규를 몰아 넣는 날수
    first_peak: i64,
    first_days: i64,
    /// 그대로 두면 기준을 넘는 날들
    over_dates: Vec<String>,
    /// 이미 고른 것 (spread | first | keep)
    chosen: Option<String>,
}

/// 최근 30일 하루 평균 (공부한 날만). 기록이 없으면 None
fn avg30(con: &Connection, today: i64, decks: &[Deck]) -> Res<Option<f64>> {
    let all: HashSet<i64> = decks.iter().map(|d| d.id).collect();
    let done = done_by_day(con, &all, today - 30)?;
    let days: Vec<i64> = done.iter().filter(|(d, k)| **d < today && **k > 0).map(|(_, k)| *k).collect();
    Ok((!days.is_empty()).then(|| days.iter().sum::<i64>() as f64 / days.len() as f64))
}

fn find_conflicts(con: &Connection, today: i64, decks: &[Deck], cards: &[Card], time: &Time, include_resolved: bool) -> Res<Vec<Conflict>> {
    let mut roots: Vec<&Deck> = decks.iter().filter(|d| d.parent_id.is_none() && d.target_day.is_some_and(|t| t >= today)).collect();
    if roots.len() < 2 {
        return Ok(vec![]);
    }
    roots.sort_by_key(|d| d.target_day);
    let avg = avg30(con, today, decks)?;
    let by_time = time.weekday_min.is_some() || time.weekend_min.is_some();
    let limit_on = |d: i64| -> Option<i64> { if by_time { cap_for(d, time) } else { avg.map(|a| (a * 1.5).round() as i64) } };
    let all: HashSet<i64> = decks.iter().map(|d| d.id).collect();
    let shifts = read_shifts(con, today)?;
    let done_new = done_new_by_owner(con, today, decks)?;
    let backlog_seen = backlog_done(con, today)?;
    let mut out = vec![];
    for w in roots.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ta, tb) = (a.target_day.unwrap_or(today), b.target_day.unwrap_or(today));
        if tb - ta > CONFLICT_GAP {
            continue;
        }
        let chosen: Option<String> = con
            .query_row("SELECT mode FROM plan_conflict WHERE deck_a = ?1 AND deck_b = ?2", params![a.id, b.id], |r| r.get(0))
            .optional()
            .map_err(err)?;
        if chosen.is_some() && !include_resolved {
            continue;
        }
        // 늦은 시험 전날까지 (그림과 같다)
        let n = (tb - today).clamp(1, MAX_FUTURE);
        // 이 두 덱의 조정을 뺀 날것 (그대로 두기)과, 고를 수 있는 두 가지
        let with = |mode: &str| -> Vec<DayLoad> {
            let mut ds = decks.to_vec();
            for d in ds.iter_mut().filter(|d| d.id == b.id) {
                d.new_until = None;
                d.new_from = None;
                apply_mode(d, mode, ta);
            }
            project(today, n, &ds, cards, &all, &shifts, &done_new, backlog_seen)
        };
        let keep = with("keep");
        let over: Vec<usize> = (0..keep.len()).filter(|&i| limit_on(today + i as i64).is_some_and(|l| keep[i].total() > l)).collect();
        if over.is_empty() {
            continue;
        }
        let peak_i = (0..keep.len()).max_by_key(|&i| (keep[i].total(), -(i as i64))).unwrap_or(0);
        let spread = with("spread");
        let first = with("first");
        let peak = |v: &[DayLoad]| v.iter().map(DayLoad::total).max().unwrap_or(0);
        let b_new_today = |v: &[DayLoad]| v.first().map_or(0, |x| x.new_by_owner.get(&b.id).copied().unwrap_or(0));
        let limit = limit_on(today).or_else(|| (0..keep.len()).find_map(|i| limit_on(today + i as i64))).unwrap_or(0);
        out.push(Conflict {
            a: ConflictDeck { id: a.id, name: a.name.clone(), target: db::date_from_day(ta) },
            b: ConflictDeck { id: b.id, name: b.name.clone(), target: db::date_from_day(tb) },
            peak_date: db::date_from_day(today + peak_i as i64),
            peak: keep[peak_i].total(),
            limit,
            by_time,
            bars: keep
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    let (ka, kb) = (x.by_root.get(&a.id).copied().unwrap_or(0), x.by_root.get(&b.id).copied().unwrap_or(0));
                    ConflictBar {
                        date: db::date_from_day(today + i as i64),
                        a: ka,
                        b: kb,
                        other: x.total() - ka - kb,
                        limit: limit_on(today + i as i64).unwrap_or(0),
                    }
                })
                .collect(),
            spread_peak: peak(&spread),
            spread_extra: b_new_today(&spread) - b_new_today(&keep),
            first_peak: peak(&first),
            // a 시험 다음 날부터 b 시험 전날까지 (목표일 당일도 학습하면 그날까지)
            first_days: (tb - ta - 1 + b.include_target_day as i64).max(1),
            over_dates: over.iter().map(|&i| db::date_from_day(today + i as i64)).collect(),
            chosen,
        });
    }
    Ok(out)
}

/// 홈 배너와 H2 화면. include_resolved 가 false 면 아직 고르지 않은 첫 충돌만
#[tauri::command]
pub fn plan_conflict(state: State<AppState>, include_resolved: Option<bool>) -> Res<Option<Conflict>> {
    let con = state.db.lock().map_err(err)?;
    let today = study_day(now_ms());
    // 충돌 판단은 이미 고른 조정을 빼고 해야 하므로 load_decks 의 조정 위에서 b 만 되돌려 계산한다
    let decks = commands::load_decks(&con)?;
    let cards = commands::load_cards(&con)?;
    let time = read_time(&con)?;
    Ok(find_conflicts(&con, today, &decks, &cards, &time, include_resolved.unwrap_or(false))?.into_iter().next())
}

/// H2 에서 고른 것. 목표일은 바꾸지 않는다. keep 도 남겨 배너를 다시 띄우지 않는다
#[tauri::command]
pub fn resolve_plan_conflict(state: State<AppState>, deck_a: i64, deck_b: i64, mode: String) -> Res<()> {
    if !["spread", "first", "keep"].contains(&mode.as_str()) {
        return Err("알 수 없는 선택이에요.".into());
    }
    let con = state.db.lock().map_err(err)?;
    con.execute(
        "INSERT INTO plan_conflict (deck_a, deck_b, mode, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(deck_a, deck_b) DO UPDATE SET mode = ?3, created_at = ?4",
        params![deck_a, deck_b, mode, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

#[derive(Serialize)]
pub struct PlanAdjust {
    /// spread | first
    mode: String,
    /// 오늘 하루 신규가 조정 때문에 몇 장 달라졌는지 (+18)
    delta: i64,
}

/// C4 "바꾼 뒤 계획"의 "충돌 조정으로 +18장"
#[tauri::command]
pub fn plan_adjust(state: State<AppState>, deck_id: i64) -> Res<Option<PlanAdjust>> {
    let con = state.db.lock().map_err(err)?;
    let mode: Option<String> = con
        .query_row("SELECT mode FROM plan_conflict WHERE deck_b = ?1 AND mode <> 'keep'", [deck_id], |r| r.get(0))
        .optional()
        .map_err(err)?;
    let Some(mode) = mode else { return Ok(None) };
    let decks = commands::load_decks(&con)?;
    let Some(adjusted) = decks.iter().find(|d| d.id == deck_id) else { return Ok(None) };
    let raw = Deck { new_until: None, new_from: None, ..adjusted.clone() };
    let ids = commands::descendants(deck_id, &decks).iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let left = crate::notetype::new_left(&con, &ids)?;
    let today = study_day(now_ms());
    let delta = queue::new_target_for_deck(adjusted, today, left).0 - queue::new_target_for_deck(&raw, today, left).0;
    Ok(Some(PlanAdjust { mode, delta }))
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: i64 = 20_376; // 2025-10-14 (화요일)

    fn deck(id: i64) -> Deck {
        Deck { id, name: format!("d{id}"), ..Default::default() }
    }
    fn news(n: i64, deck: i64, start: i64) -> Vec<Card> {
        (0..n)
            .map(|i| Card {
                id: start + i,
                deck_id: deck,
                state: "new".into(),
                importance: 1,
                due_day: None,
                position: start + i,
                buried_until: None,
                sibling: None,
                note_id: start + i,
                weak: false,
                exam: false,
                front_empty: false,
            })
            .collect()
    }
    fn reviews(due: &[i64], deck: i64, start: i64) -> Vec<Card> {
        due.iter()
            .enumerate()
            .map(|(i, &d)| Card { state: "review".into(), due_day: Some(d), ..news(1, deck, start + i as i64).remove(0) })
            .collect()
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(0), 4, "1970-01-01 목요일");
        let sat = db::day_from_date("2026-10-17").unwrap();
        assert!(is_weekend(sat) && is_weekend(sat + 1) && !is_weekend(sat + 2));
    }

    #[test]
    fn projection_leaves_out_empty_front_cards() {
        // Q1-01: 앞면이 빈 카드는 큐에 나오지 않는다. 계획의 신규 · 복습 예측과 남은 수도 같다
        let d = Deck { target_day: Some(T + 2), ..deck(1) };
        let scope: HashSet<i64> = [1].into();
        let mut cards = news(4, 1, 1);
        cards.extend(reviews(&[T, T], 1, 10));
        for c in cards.iter_mut().filter(|c| c.id <= 3 || c.id == 10) {
            c.front_empty = true;
        }
        let days = project(T, 3, std::slice::from_ref(&d), &cards, &scope, &HashMap::new(), &HashMap::new(), 0);
        assert_eq!((days[0].new, days[0].review), (1, 1), "빈 앞면 신규 3장 · 복습 1장은 세지 않는다");
        let q = queue::build_queue(T, std::slice::from_ref(&d), &cards, None, 1.0);
        assert_eq!((q.new.len(), q.review.len()), (1, 1), "큐와 같다");
    }

    #[test]
    fn projection_follows_the_queue_formula() {
        // 신규 100장, 목표일까지 10일 → 하루 10장. 첫날 10장은 2·8·25일 뒤 복습으로 돌아온다
        let d = Deck { target_day: Some(T + 10), ..deck(1) };
        let scope: HashSet<i64> = [1].into();
        let days = project(T, 11, std::slice::from_ref(&d), &news(100, 1, 1), &scope, &HashMap::new(), &HashMap::new(), 0);
        assert_eq!(days[0].new, 10);
        assert_eq!(days.iter().map(|x| x.new).sum::<i64>(), 100, "목표일 전에 다 넣는다");
        assert_eq!((days[1].review, days[2].review), (0, 10), "첫 복습은 2일 뒤");
        assert_eq!(days[8].review, 20, "8일 뒤 + 6일째 신규의 2일 뒤");
        // 오늘 이미 4장 넣었으면 오늘은 6장만 (큐와 같다)
        let days = project(T, 3, std::slice::from_ref(&d), &news(96, 1, 1), &scope, &HashMap::new(), &[(1, 4)].into(), 0);
        assert_eq!(days[0].new, 6);
        // 당겨 맞추기: 오늘 +5
        let days = project(T, 3, &[d], &news(100, 1, 1), &scope, &[((1, T), 5)].into(), &HashMap::new(), 0);
        assert_eq!(days[0].new, 15);
    }

    #[test]
    fn projection_counts_backlog_and_due_reviews() {
        let mut c = reviews(&[T - 3, T - 1, T, T + 2], 1, 1);
        c.extend(news(0, 1, 100));
        let scope: HashSet<i64> = [1].into();
        let days = project(T, 3, &[deck(1)], &c, &scope, &HashMap::new(), &HashMap::new(), 0);
        assert_eq!((days[0].review, days[1].review, days[2].review), (3, 0, 1));
        // 밀린 복습: 덱 2 의 더 오래된 150장이 하루 200장 상한을 먼저 쓴다 (큐와 같다). 오늘 이미 30장 봤으면 오늘은 20장
        let mut c = reviews(&vec![T - 9; 150], 2, 1000);
        c.extend(reviews(&vec![T - 2; 100], 1, 2000));
        let days = project(T, 2, &[deck(1), deck(2)], &c, &scope, &HashMap::new(), &HashMap::new(), 30);
        assert_eq!((days[0].review, days[1].review), (20, 80));
    }

    #[test]
    fn paused_days_have_no_new() {
        let d = Deck { target_day: Some(T + 10), paused: true, paused_until: Some(T + 1), ..deck(1) };
        let days = project(T, 4, &[d], &news(80, 1, 1), &[1].into(), &HashMap::new(), &HashMap::new(), 0);
        assert_eq!((days[0].new, days[1].new), (0, 0));
        assert_eq!(days[2].new, 10, "쉬고 난 8일에 80장");
    }

    #[test]
    fn over_cap_pulls_to_nearest_earlier_day_with_slack() {
        let mk = |rev: i64, new: i64| DayLoad { review: rev, new, new_by_owner: [(1, new)].into(), ..Default::default() };
        // 상한 100. 3일째 108장 → 1일째(여유 10)로 신규 8장
        let days = vec![mk(40, 50), mk(50, 50), mk(58, 50)];
        assert_eq!(find_over(T, &days, |_| Some(100)), Some((T + 2, Some(T), 8, 1)));
        // 여유 있는 날이 없으면 옮길 날 없음
        let days = vec![mk(50, 50), mk(58, 50)];
        assert_eq!(find_over(T, &days, |_| Some(100)), Some((T + 1, None, 0, 1)));
        // 시간을 넣지 않았으면 안내도 없다
        assert_eq!(find_over(T, &days, |_| None), None);
    }

    #[test]
    fn modes_only_change_the_later_deck() {
        let mut b = Deck { target_day: Some(T + 10), ..deck(2) };
        apply_mode(&mut b, "spread", T + 5);
        assert_eq!(queue::new_target_for_deck(&b, T, 100).0, 20);
        let mut b = Deck { target_day: Some(T + 10), ..deck(2) };
        apply_mode(&mut b, "first", T + 5);
        assert_eq!(queue::new_target_for_deck(&b, T, 100).0, 0);
        assert_eq!(b.target_day, Some(T + 10), "목표일은 그대로");
    }

    #[test]
    fn adjust_decks_reads_plan_tables() {
        let con = db::open_in_memory().unwrap();
        for (id, t) in [(1, "2026-10-26"), (2, "2026-10-28")] {
            con.execute("INSERT INTO deck (id, name, created_at, updated_at) VALUES (?1, ?2, 0, 0)", params![id, format!("d{id}")]).unwrap();
            con.execute("INSERT INTO deck_config (deck_id, target_date, updated_at) VALUES (?1, ?2, 0)", params![id, t]).unwrap();
        }
        let today = db::day_from_date("2026-10-14").unwrap();
        con.execute("INSERT INTO plan_conflict (deck_a, deck_b, mode, created_at) VALUES (1, 2, 'first', 0)", []).unwrap();
        con.execute("INSERT INTO plan_shift (deck_id, day, delta, created_at) VALUES (1, '2026-10-14', 6, 0)", []).unwrap();
        con.execute("UPDATE deck_config SET fixed_new_per_day = 12 WHERE deck_id = 1", []).unwrap();
        let mut decks = vec![
            Deck { id: 1, target_day: db::day_from_date("2026-10-26"), ..Default::default() },
            Deck { id: 2, target_day: db::day_from_date("2026-10-28"), ..Default::default() },
        ];
        adjust_decks(&con, &mut decks, today).unwrap();
        assert_eq!((decks[0].fixed_new, decks[0].new_shift), (Some(12), 6));
        assert_eq!(decks[1].new_from, db::day_from_date("2026-10-27"));
        assert_eq!(decks[1].target_day, db::day_from_date("2026-10-28"));
    }
}
