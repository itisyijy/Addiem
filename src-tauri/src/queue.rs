//! 오늘의 학습 큐. tools/queue_reference.py 를 옮긴 것이고 docs/engineering/study-queue-spec.md 가 명세다.
//! 순수 함수다. DB 를 모른다.

use std::collections::{HashMap, HashSet};

pub const BACKLOG_DAILY_CAP: usize = 200;
pub const BURY_RETURN_DAYS: i64 = 3;
pub const DEFAULT_NEW_LIMIT: i64 = 20;

#[cfg(test)]
const CORE: i64 = 2;

#[derive(Debug, Clone, Default)]
pub struct Deck {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub target_day: Option<i64>,
    pub include_target_day: bool,
    pub daily_new_limit: Option<i64>,
    /// 오늘만 정한 신규 분량 (C4). 오늘 것이 아니면 None 으로 읽는다
    pub today_new: Option<i64>,
    /// 잠시 쉬는 덱. 하위 덱까지 큐에서 뺀다
    pub paused: bool,
    /// 이 날(포함)까지 쉰다. None 이면 다시 시작할 때까지
    pub paused_until: Option<i64>,
    /// 약한 챕터로 확정 (C1-w). 하위 덱까지 복습을 먼저 보여준다. 장수와 일정은 바꾸지 않는다
    pub weak: bool,
    // ---- H 시험 계획 (plan.rs 가 채운다). 목표일 자체는 바꾸지 않고 신규를 나누는 방식만 바꾼다
    /// H2 앞당겨 나누기: 신규를 이 날까지 나눈다 (목표일보다 이르다)
    pub new_until: Option<i64>,
    /// H2 먼저 볼 덱: 이 날부터 신규를 넣는다
    pub new_from: Option<i64>,
    /// H1 "실제 속도에 맞춰 다시 나누기"를 끄면 정해 둔 하루 몫. 매일 다시 나누지 않는다
    pub fixed_new: Option<i64>,
    /// H1 [N일로 당겨 맞추기]: 오늘 몫에 더하거나 빼는 장수
    pub new_shift: i64,
}

impl Deck {
    fn has_config(&self) -> bool {
        self.target_day.is_some() || self.daily_new_limit.is_some() || self.today_new.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct Card {
    pub id: i64,
    pub deck_id: i64,
    pub state: String,
    pub importance: i64,
    pub due_day: Option<i64>,
    pub position: i64,
    pub buried_until: Option<i64>,
    /// 형제 카드 묶음 (빈칸이 아닌 노트의 note_id). 같은 묶음은 하루에 한 장만 (C3-r)
    pub sibling: Option<i64>,
    pub note_id: i64,
    /// 약한 챕터의 카드 (큐가 채운다)
    pub weak: bool,
    /// 기출 카드 (8.4, exam::IS_EXAM). 신규를 넣을 때 같은 중요도 안에서 먼저
    pub exam: bool,
    /// 앞면이 빈 카드 (notetype::Fronts). 큐에 내지 않는다. 상태 · 일정은 그대로라 필드를 채우면 다시 나온다 (NT49)
    pub front_empty: bool,
}

impl Card {
    /// 학습에 나오는 카드: 일시 정지도 앞면 빔도 아니다. 큐 · 계획 예측이 같은 규칙을 쓴다 (Q1-01, SQL 쪽은 notetype::new_left)
    pub fn plays(&self) -> bool {
        self.state != "suspended" && !self.front_empty
    }
}

/// 오늘 이미 한 학습. 계획을 다시 만들 때 오늘 분량이 또 생기지 않게 뺀다
#[derive(Debug, Default)]
pub struct Today {
    /// 덱별로 오늘 처음 학습한 신규 카드 수
    pub introduced: HashMap<i64, i64>,
    /// 오늘 처리한 밀린 복습 수. 하루 상한에서 뺀다
    pub backlog_done: usize,
    /// 오늘 한 장이라도 본 형제 묶음. 나머지 카드는 다음 날로
    pub siblings_seen: HashSet<i64>,
}

#[derive(Debug, Default)]
pub struct QueuePlan {
    pub learning: Vec<Card>,
    pub returning: Vec<Card>,
    pub review: Vec<Card>,
    pub backlog: Vec<Card>,
    pub new: Vec<Card>,
    pub backlog_deferred: usize,
    pub per_deck_new_target: HashMap<i64, i64>,
    pub notes: Vec<String>,
}

impl QueuePlan {
    /// 제시 순서. 복습을 먼저 비우고 신규로 간다 (명세 4.7)
    pub fn ordered(&self) -> Vec<&Card> {
        // 복습 묶음(오늘 예정 + 밀린 것) 안에서 약한 챕터 카드를 먼저. 접어둔 카드 복귀 다음이다 (C1-w)
        let reviews = || self.review.iter().chain(&self.backlog);
        let mut v: Vec<&Card> = self
            .learning
            .iter()
            .chain(&self.returning)
            .chain(reviews().filter(|c| c.weak))
            .chain(reviews().filter(|c| !c.weak))
            .chain(&self.new)
            .collect();
        spread(&mut v);
        v
    }
}

/// 같은 노트의 카드(빈칸 c1·c2)가 바로 이어 나오지 않게 사이에 다른 카드를 한 장 끼운다.
/// 끼울 카드가 없으면(남은 것이 모두 같은 노트) 그대로 둔다
fn spread(v: &mut Vec<&Card>) {
    for i in 1..v.len() {
        if v[i].note_id == v[i - 1].note_id
            && let Some(j) = (i + 1..v.len()).find(|&j| v[j].note_id != v[i - 1].note_id)
        {
            let c = v.remove(j);
            v.insert(i, c);
        }
    }
}

/// 목표일까지 남은 학습 가능 일수. 오늘을 포함한다. None 이면 목표일 없음
pub fn learnable_days(today: i64, target: Option<i64>, include_target_day: bool) -> Option<i64> {
    target.map(|t| t - today + if include_target_day { 1 } else { 0 })
}

/// 신규를 다시 나누기 시작하는 날. 쉬는 덱이면 쉬기가 끝난 다음 날이다 (C4-p: 쉬는 날만큼 남은 날에 몰린다)
pub fn resume_day(deck: &Deck, today: i64) -> i64 {
    match deck.paused_until {
        Some(u) if deck.paused && u >= today => u + 1,
        _ => today,
    }
}

/// 신규 예산을 가진 덱. 설정을 가진 가장 가까운 조상, 없으면 최상위 (명세 4.4)
pub fn budget_owner(deck_id: i64, by_id: &HashMap<i64, &Deck>) -> i64 {
    let mut node = by_id.get(&deck_id).copied();
    let mut fallback = deck_id;
    let mut seen = HashSet::new();
    while let Some(d) = node {
        if !seen.insert(d.id) {
            break; // 부모가 순환하면 멈춘다
        }
        if d.has_config() {
            return d.id;
        }
        fallback = d.id;
        node = d.parent_id.and_then(|p| by_id.get(&p).copied());
    }
    fallback
}

pub fn new_target_for_deck(deck: &Deck, today: i64, remaining_new: i64) -> (i64, String) {
    if remaining_new == 0 {
        return (0, "남은 신규 카드가 없다".into());
    }
    if deck.new_from.is_some_and(|f| today < f) {
        return (0, "먼저 볼 덱의 시험이 끝난 뒤 신규를 넣는다".into());
    }
    if let Some(n) = deck.fixed_new {
        let n = n.clamp(0, remaining_new);
        return (n, format!("하루 몫 {n}장으로 고정 (다시 나누지 않는다)"));
    }
    let cap = deck.daily_new_limit;
    // 앞당겨 나누기면 목표일 대신 그 날까지로 나눈다
    match learnable_days(resume_day(deck, today), deck.new_until.or(deck.target_day), deck.include_target_day) {
        None => {
            let n = cap.unwrap_or(DEFAULT_NEW_LIMIT);
            (n.min(remaining_new), format!("목표일 없음. 하루 상한 {n}장"))
        }
        Some(days) if days <= 0 => {
            let n = cap.map_or(remaining_new, |c| c.min(remaining_new));
            let why = if days < 0 { "목표일이 지났다" } else { "오늘이 목표일이다" };
            (n, format!("{why}. 남은 {remaining_new}장 중 {n}장"))
        }
        Some(days) => {
            // 올림. 나머지가 마지막 날에 몰리지 않게 한다
            let want = (remaining_new + days - 1) / days;
            if let Some(c) = cap.filter(|c| want > *c) {
                return (
                    c,
                    format!("목표일까지 하루 {want}장이 필요한데 상한이 {c}장이다. 이대로면 목표일을 넘긴다"),
                );
            }
            (
                want.min(remaining_new),
                format!("남은 {remaining_new}장 ÷ {days}일 = 하루 {want}장"),
            )
        }
    }
}

/// deck_ids 를 하위 덱까지 넓힌다
fn with_descendants(roots: &[i64], decks: &[Deck]) -> HashSet<i64> {
    let mut out: HashSet<i64> = roots.iter().copied().collect();
    loop {
        let before = out.len();
        for d in decks {
            if d.parent_id.is_some_and(|p| out.contains(&p)) {
                out.insert(d.id);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

pub fn build_queue(today: i64, decks: &[Deck], cards: &[Card], deck_ids: Option<&[i64]>, new_scale: f64) -> QueuePlan {
    build_queue_with_progress(today, decks, cards, deck_ids, new_scale, &Today::default())
}

/// 목표량은 하루를 시작할 때 기준으로 계산하고, 오늘 이미 학습한 만큼(done) 뺀다.
/// 빼지 않으면 학습을 마치고 홈에 돌아올 때마다 남은 카드를 다시 나눠 오늘 분량이 또 생긴다
pub fn build_queue_with_progress(
    today: i64,
    decks: &[Deck],
    cards: &[Card],
    deck_ids: Option<&[i64]>,
    new_scale: f64,
    done: &Today,
) -> QueuePlan {
    let mut plan = QueuePlan::default();
    let by_id: HashMap<i64, &Deck> = decks.iter().map(|d| (d.id, d)).collect();
    let wanted: HashSet<i64> = match deck_ids {
        Some(ids) => with_descendants(ids, decks),
        None => by_id.keys().copied().collect(),
    };
    // 쉬는 덱은 하위 덱까지 뺀다. 다만 쉬는 덱(또는 그 안의 덱)을 직접 골라 학습하면 그 범위의 복습은 보여주고
    // 신규만 넣지 않는다 (C1-p "쉬는 중에도 학습하기", I84). 고른 덱과 그 조상이 "직접 고른" 쪽이다
    let mut chosen = HashSet::new();
    for &id in deck_ids.unwrap_or_default() {
        let mut cur = Some(id);
        while let Some(d) = cur.filter(|d| chosen.insert(*d)) {
            cur = by_id.get(&d).and_then(|x| x.parent_id);
        }
    }
    let paused_ids: Vec<i64> = decks.iter().filter(|d| d.paused).map(|d| d.id).collect();
    let paused = with_descendants(&paused_ids, decks);
    let skipped = with_descendants(&paused_ids.iter().copied().filter(|id| !chosen.contains(id)).collect::<Vec<_>>(), decks);
    let weak = with_descendants(&decks.iter().filter(|d| d.weak).map(|d| d.id).collect::<Vec<_>>(), decks);
    let active = cards.iter().filter(|c| by_id.contains_key(&c.deck_id) && !skipped.contains(&c.deck_id) && c.plays());

    // 1단계. 후보 수집. 밀린 복습과 신규는 범위 밖 덱까지 모은다: 하루 상한 · 예산 덱 몫은 전체 계획에서 고르고
    // 범위로 거른다. 그래야 덱 하나(하위 덱)만 학습해도 홈 · 상위 덱 표의 그 덱 분량과 같다 (QA T3-08)
    let mut new_by_owner: HashMap<i64, Vec<Card>> = HashMap::new();
    let mut all_backlog = vec![];
    for c in active {
        let mine = wanted.contains(&c.deck_id);
        // 보류는 상태가 아니라 기간이다
        if let Some(until) = c.buried_until {
            if mine && until <= today {
                plan.returning.push(c.clone());
            }
            continue;
        }
        match c.state.as_str() {
            "learning" if mine => plan.learning.push(c.clone()),
            "review" | "relearning" => match c.due_day {
                Some(d) if d < today => all_backlog.push(c.clone()),
                Some(d) if d == today && mine => plan.review.push(c.clone()),
                _ => {}
            },
            "new" if !paused.contains(&c.deck_id) => new_by_owner.entry(budget_owner(c.deck_id, &by_id)).or_default().push(c.clone()),
            _ => {}
        }
    }

    // 3단계(복습 먼저). 오래 밀린 것부터, 같으면 중요도 높은 순. 기출은 신규에만 쓴다 (8.4, 복습 일정은 SRS 대로)
    let key = |c: &Card| (c.due_day.unwrap_or(today), -c.importance, c.position, c.id);
    plan.review.sort_by_key(key);
    all_backlog.sort_by_key(key);
    plan.learning.sort_by_key(key);
    plan.returning.sort_by_key(|c| (-c.importance, c.position, c.id));

    // 형제 카드는 하루에 한 장. 학습 단계를 밟는 카드(학습 중 · 재학습)는 끊지 않고, 앞선 것(복귀 > 오늘 복습 > 밀린 복습 > 신규)이 남는다.
    // 오늘 Again 을 받은 복습 카드는 자기 노트가 siblings_seen 에 있다. 재학습을 빼면 오늘 안에 다시 나오지 않는다 (I80)
    let stepping = |c: &Card| c.state == "learning" || c.state == "relearning";
    let mut seen = done.siblings_seen.clone();
    seen.extend(plan.learning.iter().chain(&plan.returning).chain(&plan.review).chain(&all_backlog).filter(|c| stepping(c)).filter_map(|c| c.sibling));
    let mut first = |c: &Card| stepping(c) || c.sibling.is_none_or(|n| seen.insert(n));
    plan.returning.retain(&mut first);
    plan.review.retain(&mut first);
    all_backlog.retain(&mut first);

    // 2단계. 신규 목표량. 예산 덱마다
    let mut done_by_owner: HashMap<i64, i64> = HashMap::new();
    for (deck_id, n) in &done.introduced {
        *done_by_owner.entry(budget_owner(*deck_id, &by_id)).or_default() += n;
    }
    let mut owners: Vec<_> = new_by_owner.into_iter().filter(|(_, g)| g.iter().any(|c| wanted.contains(&c.deck_id))).collect();
    owners.sort_by_key(|(id, _)| *id);
    for (owner, mut group) in owners {
        let deck = by_id[&owner];
        let done = done_by_owner.get(&owner).copied().unwrap_or(0);
        let (mut target, mut why) = match deck.today_new {
            Some(n) => (n.min(group.len() as i64 + done), format!("오늘만 신규 {n}장으로 조정")),
            None => new_target_for_deck(deck, today, group.len() as i64 + done),
        };
        if deck.new_shift != 0 {
            target = (target + deck.new_shift).max(0);
            why.push_str(&format!(" (계획 조정 {:+}장)", deck.new_shift));
        }
        if (new_scale - 1.0).abs() > f64::EPSILON {
            target = (target as f64 * new_scale).round() as i64;
            why.push_str(&format!(" (사용자 조정 x{new_scale})"));
        }
        target = (target - done).clamp(0, group.len() as i64);
        plan.per_deck_new_target.insert(owner, target);
        plan.notes.push(format!("{}: {why}", deck.name));
        // 3단계. 중요도 높은 순, 같으면 기출 먼저, 그다음 원래 순서
        group.sort_by_key(|c| (-c.importance, !c.exam, c.position, c.id));
        plan.new.extend(group.into_iter().filter(&mut first).take(target as usize).filter(|c| wanted.contains(&c.deck_id)));
    }
    // 덱을 가로질러 핵심 -> 일반 -> 보충 (D1 "이 순서로 보여드려요"), 같은 중요도면 기출 먼저. 그 안에서는 덱 순서를 지킨다
    plan.new.sort_by_key(|c| (-c.importance, !c.exam));

    // 4단계. 밀린 복습 상한. 오늘 이미 처리한 만큼 뺀다
    let cap = BACKLOG_DAILY_CAP.saturating_sub(done.backlog_done);
    let in_scope = all_backlog.iter().filter(|c| wanted.contains(&c.deck_id)).count();
    plan.backlog = all_backlog.into_iter().take(cap).filter(|c| wanted.contains(&c.deck_id)).collect();
    plan.backlog_deferred = in_scope - plan.backlog.len();
    for c in plan.review.iter_mut().chain(plan.backlog.iter_mut()) {
        c.weak = weak.contains(&c.deck_id);
    }
    if plan.backlog_deferred > 0 {
        plan.notes.push(format!(
            "밀린 복습 {in_scope}장 중 {}장만 오늘 올렸어요. 하루 {BACKLOG_DAILY_CAP}장씩 차차 처리해요",
            plan.backlog.len()
        ));
    }
    plan
}

pub enum RatingAction {
    Continue,
    Bury,
}

/// 같은 세션에서 두 번째 Again 이면 보류. 2회차는 스케줄러에 넘기지 않는다 (명세 6.3)
pub fn on_rating(session_again_count: i64, rating: u8) -> (RatingAction, bool) {
    if rating != 1 {
        return (RatingAction::Continue, true);
    }
    if session_again_count + 1 >= 2 {
        (RatingAction::Bury, false)
    } else {
        (RatingAction::Continue, true)
    }
}

/// "다음 학습으로" 의 복귀일. "오늘 다시" 는 기한을 비운다 (commands::resolve_buried)
pub fn bury_until(today: i64) -> i64 {
    today + BURY_RETURN_DAYS
}

#[cfg(test)]
mod tests {
    //! tools/queue_reference.py 의 test() 를 옮겼다
    use super::*;
    const T: i64 = 20_000;

    fn cards(spec: &[(&str, i64, Option<i64>)], deck_id: i64, start: i64) -> Vec<Card> {
        spec.iter()
            .enumerate()
            .map(|(i, (state, imp, off))| Card {
                id: start + i as i64,
                deck_id,
                state: state.to_string(),
                importance: *imp,
                due_day: off.map(|o| T + o),
                position: start + i as i64,
                buried_until: None,
                sibling: None,
                note_id: start + i as i64,
                weak: false,
                exam: false,
                front_empty: false,
            })
            .collect()
    }
    fn news(n: usize, deck: i64, start: i64) -> Vec<Card> {
        cards(&vec![("new", 1, None); n], deck, start)
    }
    fn deck(id: i64) -> Deck {
        Deck { id, name: format!("d{id}"), ..Default::default() }
    }

    #[test]
    fn learnable() {
        assert_eq!(learnable_days(T, Some(T + 10), false), Some(10));
        assert_eq!(learnable_days(T, Some(T + 10), true), Some(11));
        assert_eq!(learnable_days(T, Some(T), false), Some(0));
        assert_eq!(learnable_days(T, Some(T - 10), false), Some(-10));
        assert_eq!(learnable_days(T, None, false), None);
    }

    #[test]
    fn scenario1_target_deck_first_study() {
        let d = Deck { target_day: Some(T + 10), ..deck(1) };
        let p = build_queue(T, std::slice::from_ref(&d), &news(100, 1, 1), None, 1.0);
        assert_eq!(p.new.len(), 10);
        let mut mixed = cards(&vec![("new", 0, None); 50], 1, 1);
        mixed.extend(cards(&vec![("new", CORE, None); 50], 1, 100));
        let p = build_queue(T, &[d], &mixed, None, 1.0);
        assert!(p.new.iter().all(|c| c.importance == CORE), "핵심이 먼저");
    }

    #[test]
    fn rounding_and_caps() {
        let d = Deck { target_day: Some(T + 3), ..deck(1) };
        assert_eq!(build_queue(T, &[d], &news(10, 1, 1), None, 1.0).new.len(), 4);
        let d = Deck { target_day: Some(T + 2), daily_new_limit: Some(3), ..deck(1) };
        let p = build_queue(T, &[d], &news(100, 1, 1), None, 1.0);
        assert_eq!(p.new.len(), 3);
        assert!(p.notes.iter().any(|n| n.contains("목표일을 넘긴다")));
    }

    #[test]
    fn target_passed_or_today() {
        let d = Deck { target_day: Some(T - 5), ..deck(1) };
        let p = build_queue(T, &[d], &news(7, 1, 1), None, 1.0);
        assert_eq!(p.new.len(), 7);
        assert!(p.notes.iter().any(|n| n.contains("목표일이 지났다")));
        let d = Deck { target_day: Some(T), ..deck(1) };
        assert_eq!(build_queue(T, &[d], &news(5, 1, 1), None, 1.0).new.len(), 5);
        let d = Deck { target_day: Some(T), include_target_day: true, ..deck(1) };
        assert_eq!(build_queue(T, &[d], &news(5, 1, 1), None, 1.0).new.len(), 5);
    }

    #[test]
    fn empty_and_zero_limit() {
        assert_eq!(build_queue(T, &[deck(1)], &[], None, 1.0).ordered().len(), 0);
        let d = Deck { daily_new_limit: Some(0), ..deck(1) };
        assert_eq!(build_queue(T, &[d], &news(10, 1, 1), None, 1.0).new.len(), 0);
    }

    #[test]
    fn scenario4_backlog() {
        let spec: Vec<_> = (1..=300).map(|d| ("review", 1, Some(-d))).collect();
        let backlog = cards(&spec, 1, 1);
        let p = build_queue(T, &[deck(1)], &backlog, None, 1.0);
        assert_eq!(p.backlog.len(), BACKLOG_DAILY_CAP);
        assert_eq!(p.backlog_deferred, 300 - BACKLOG_DAILY_CAP);
        assert!(p.backlog[0].due_day < p.backlog.last().unwrap().due_day, "오래 밀린 것부터");
        let mut mixed = backlog;
        mixed.extend(news(20, 1, 1000));
        let d = Deck { daily_new_limit: Some(5), ..deck(1) };
        assert_eq!(build_queue(T, &[d], &mixed, None, 1.0).new.len(), 5, "밀린 복습이 신규를 막지 않는다");
    }

    #[test]
    fn weak_chapter_reviews_first() {
        // 덱 2 (약함) 의 복습이 덱 1 복습보다 먼저. 밀린 것도 같다. 장수는 그대로
        let mut c = cards(&[("review", 1, Some(0)), ("review", 1, Some(-2))], 1, 1);
        c.extend(cards(&[("review", 1, Some(0)), ("review", 1, Some(-1))], 2, 10));
        let decks = [deck(1), Deck { weak: true, ..deck(2) }];
        let p = build_queue(T, &decks, &c, None, 1.0);
        let ids: Vec<i64> = p.ordered().iter().map(|c| c.id).collect();
        assert_eq!(ids, [10, 11, 1, 2]);
        assert_eq!(build_queue(T, &[deck(1), deck(2)], &c, None, 1.0).ordered().len(), 4);
    }

    #[test]
    fn exam_new_cards_first_within_same_importance() {
        // 신규: 상한 2장. 핵심(3)이 먼저이고, 일반 중에서는 기출(4)이 앞선 순서(1, 2)보다 먼저 뽑힌다
        let mut c = cards(&[("new", 1, None), ("new", 1, None), ("new", 2, None), ("new", 1, None)], 1, 1);
        c[3].exam = true;
        let one = Deck { daily_new_limit: Some(2), ..deck(1) };
        let ids = |p: &QueuePlan| p.ordered().iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(&build_queue(T, &[one], &c, None, 1.0)), [3, 4]);
        // 덱을 가로질러도: 덱 2 의 기출 일반 카드(20)가 덱 1 의 기출 아닌 일반 카드(1)보다 먼저. 핵심은 넘지 않는다
        let mut c2 = news(2, 2, 20);
        c2[0].exam = true;
        c2.extend(c.clone());
        let decks = [Deck { daily_new_limit: Some(3), ..deck(1) }, Deck { daily_new_limit: Some(1), ..deck(2) }];
        assert_eq!(ids(&build_queue(T, &decks, &c2, None, 1.0)), [3, 4, 20, 1]);
        // 복습 순서에는 쓰지 않는다: 예정일 → 중요도 → 원래 순서 그대로
        let mut r = cards(&[("review", 1, Some(0)), ("review", 1, Some(0)), ("review", 2, Some(0)), ("review", 1, Some(-1)), ("review", 1, Some(-1))], 1, 10);
        r[1].exam = true;
        r[4].exam = true;
        assert_eq!(ids(&build_queue(T, &[deck(1)], &r, None, 1.0)), [12, 10, 11, 13, 14]);
    }

    #[test]
    fn cloze_siblings_are_not_back_to_back() {
        // 빈칸 노트 99 의 c1·c2·c3 (형제 규칙 밖) + 다른 카드 2장
        let mut c = news(5, 1, 1);
        for x in &mut c[..3] {
            x.note_id = 99;
        }
        let p = build_queue(T, &[deck(1)], &c, None, 1.0);
        let notes: Vec<i64> = p.ordered().iter().map(|c| c.note_id).collect();
        assert_eq!(notes.len(), 5);
        assert!(notes.windows(2).all(|w| w[0] != w[1]), "{notes:?}");
    }

    #[test]
    fn siblings_one_per_day() {
        // 노트 7 의 앞뒤 카드: 신규 둘 중 하나만. 노트 8 은 복습이 있으니 신규 형제는 빠진다
        let mut c = news(4, 1, 1);
        c[0].sibling = Some(7);
        c[1].sibling = Some(7);
        c[2].sibling = Some(8);
        let mut r = cards(&[("review", 1, Some(0))], 1, 100);
        r[0].sibling = Some(8);
        c.extend(r);
        let p = build_queue(T, &[deck(1)], &c, None, 1.0);
        let ids: Vec<i64> = p.ordered().iter().map(|c| c.id).collect();
        assert_eq!(ids, [100, 1, 4]);
        // 오늘 이미 본 묶음은 다음 날로
        let done = Today { siblings_seen: [7].into(), ..Default::default() };
        let p = build_queue_with_progress(T, &[deck(1)], &c, None, 1.0, &done);
        assert_eq!(p.ordered().iter().map(|c| c.id).collect::<Vec<_>>(), [100, 4]);
    }

    #[test]
    fn relearning_survives_its_own_note_seen_today() {
        // T1-01: 오늘 예정 복습(노트 7)에서 Again -> 재학습 10분 뒤. 오늘 평가했으니 노트 7 은 siblings_seen 에 있다
        let mut c = cards(&[("relearning", 1, Some(0)), ("review", 1, Some(0)), ("relearning", 1, Some(-1))], 1, 1);
        c[0].sibling = Some(7);
        c[1].sibling = Some(7); // 같은 노트의 다른 카드는 다음 날로
        c[2].sibling = Some(8); // 어제 Again 받고 끝내지 못한 재학습도 끊지 않는다
        let done = Today { siblings_seen: [7, 8].into(), ..Default::default() };
        let p = build_queue_with_progress(T, &[deck(1)], &c, None, 1.0, &done);
        assert_eq!(p.ordered().iter().map(|c| c.id).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn paused_deck_studied_directly_shows_reviews_only() {
        // T1-04: 쉬는 덱을 직접 골라 학습하면 복습(학습 중 · 예정 · 밀린)만, 신규는 넣지 않는다
        let parent = Deck { paused: true, ..deck(1) };
        let child = Deck { parent_id: Some(1), ..deck(2) };
        let mut c = cards(&[("review", 1, Some(0)), ("learning", 1, Some(0)), ("review", 1, Some(-2))], 2, 1);
        c.extend(news(5, 1, 100));
        let decks = [parent, child, deck(3)];
        assert_eq!(build_queue(T, &decks, &c, None, 1.0).ordered().len(), 0, "전체 학습에서는 빠진다");
        let p = build_queue(T, &decks, &c, Some(&[1]), 1.0);
        assert_eq!((p.learning.len(), p.review.len(), p.backlog.len(), p.new.len()), (1, 1, 1, 0));
        assert_eq!(build_queue(T, &decks, &c, Some(&[2]), 1.0).ordered().len(), 3, "쉬는 덱 안의 덱을 골라도 같다");
        // 쉬지 않는 위 덱을 고르면 쉬는 하위 덱은 그대로 빠진다
        let decks = [deck(1), Deck { parent_id: Some(1), paused: true, ..deck(2) }];
        assert_eq!(build_queue(T, &decks, &c, Some(&[1]), 1.0).ordered().iter().map(|c| c.deck_id).collect::<Vec<_>>(), [1; 5]);
    }

    #[test]
    fn backlog_cap_is_shared_and_counts_today() {
        // 덱 1 은 더 오래 밀렸다 (-300..-151), 덱 2 는 -150..-1
        let mut c = cards(&(151..=300).map(|d| ("review", 1, Some(-d))).collect::<Vec<_>>(), 1, 1);
        c.extend(cards(&(1..=150).map(|d| ("review", 1, Some(-d))).collect::<Vec<_>>(), 2, 1000));
        let decks = [deck(1), deck(2)];
        // 덱 2 만 학습해도 전체 계획에서 덱 2 몫(200 - 150 = 50장)만
        let p = build_queue(T, &decks, &c, Some(&[2]), 1.0);
        assert_eq!((p.backlog.len(), p.backlog_deferred), (50, 100));
        // 오늘 이미 180장을 했으면 남은 상한은 20장
        let p = build_queue_with_progress(T, &decks, &c, None, 1.0, &Today { backlog_done: 180, ..Default::default() });
        assert_eq!(p.backlog.len(), 20);
        let p = build_queue_with_progress(T, &decks, &c, None, 1.0, &Today { backlog_done: 250, ..Default::default() });
        assert_eq!((p.backlog.len(), p.backlog_deferred), (0, 300), "상한을 다 쓰면 오늘은 끝");
    }

    #[test]
    fn review_vs_backlog_vs_future() {
        let p = build_queue(T, &[deck(1)], &cards(&[("review", 1, Some(0)), ("review", 1, Some(-3)), ("review", 1, Some(5))], 1, 1), None, 1.0);
        assert_eq!((p.review.len(), p.backlog.len()), (1, 1));
    }

    #[test]
    fn scenario2_bury() {
        assert!(matches!(on_rating(0, 1), (RatingAction::Continue, true)));
        assert!(matches!(on_rating(1, 1), (RatingAction::Bury, false)));
        assert!(matches!(on_rating(1, 3), (RatingAction::Continue, true)));
        assert_eq!(bury_until(T), T + BURY_RETURN_DAYS);
        let mut c = cards(&[("review", 1, Some(0)), ("review", 1, Some(0))], 1, 1);
        c[0].buried_until = Some(T + 2);
        c[1].buried_until = Some(T);
        let p = build_queue(T, &[deck(1)], &c, None, 1.0);
        assert_eq!(p.returning.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(p.returning[0].state, "review", "보류해도 원래 상태를 잃지 않는다");
        // 보류 복귀는 신규 상한을 쓰지 않는다
        let d = Deck { daily_new_limit: Some(3), ..deck(1) };
        let mut c2 = vec![c[1].clone()];
        c2.extend(news(10, 1, 100));
        let p = build_queue(T, &[d], &c2, None, 1.0);
        assert_eq!((p.new.len(), p.returning.len()), (3, 1));
    }

    #[test]
    fn scenario3_mixed_decks_and_selection() {
        let a = Deck { target_day: Some(T + 5), ..deck(1) };
        let b = Deck { daily_new_limit: Some(7), ..deck(2) };
        let mut c = news(50, 1, 1);
        c.extend(news(50, 2, 500));
        let p = build_queue(T, &[a.clone(), b.clone()], &c, None, 1.0);
        assert_eq!(p.per_deck_new_target[&1], 10);
        assert_eq!(p.per_deck_new_target[&2], 7);
        assert_eq!(p.new.len(), 17);
        let p = build_queue(T, &[a, b], &c, Some(&[2]), 1.0);
        assert!(p.new.len() == 7 && p.new.iter().all(|c| c.deck_id == 2));
    }

    #[test]
    fn user_scale_only_new_and_suspended_never() {
        let d = Deck { target_day: Some(T + 5), ..deck(1) };
        assert_eq!(build_queue(T, std::slice::from_ref(&d), &news(50, 1, 1), None, 0.5).new.len(), 5);
        assert_eq!(build_queue(T, &[d], &news(50, 1, 1), None, 2.0).new.len(), 20);
        let p = build_queue(T, &[deck(1)], &cards(&[("suspended", 1, Some(0))], 1, 1), None, 1.0);
        assert_eq!(p.ordered().len(), 0);
    }

    #[test]
    fn empty_front_cards_stay_out() {
        // 앞면이 빈 카드는 어느 상태든 큐에 없고, 신규 몫도 쓰지 않는다 (NT49). 형제 묶음 자리도 차지하지 않는다
        let mut c = cards(&[("review", 1, Some(0)), ("learning", 1, Some(0)), ("review", 1, Some(-3)), ("new", 1, None), ("new", 1, None)], 1, 1);
        for x in &mut c[..4] {
            x.front_empty = true;
        }
        let d = Deck { daily_new_limit: Some(1), ..deck(1) };
        let p = build_queue(T, std::slice::from_ref(&d), &c, None, 1.0);
        assert_eq!(p.ordered().iter().map(|c| c.id).collect::<Vec<_>>(), [5]);
        let mut buried = cards(&[("review", 1, Some(0))], 1, 9);
        buried[0].buried_until = Some(T);
        buried[0].front_empty = true;
        assert!(build_queue(T, &[deck(1)], &buried, None, 1.0).returning.is_empty(), "접어둔 카드가 돌아와도 빈 앞면이면 빠진다");
        let (mut a, mut b) = (cards(&[("review", 1, Some(0))], 1, 20), cards(&[("review", 1, Some(0))], 1, 21));
        a[0].sibling = Some(7);
        b[0].sibling = Some(7);
        a[0].front_empty = true;
        a.extend(b);
        assert_eq!(build_queue(T, &[deck(1)], &a, None, 1.0).ordered().iter().map(|c| c.id).collect::<Vec<_>>(), [21]);
    }

    #[test]
    fn order_learning_returning_review_backlog_new() {
        let mut c = cards(&[("new", 1, None), ("review", 1, Some(0)), ("review", 1, Some(-1)), ("learning", 1, Some(0)), ("review", 1, Some(0))], 1, 1);
        c[4].buried_until = Some(T);
        let p = build_queue(T, &[deck(1)], &c, None, 1.0);
        assert_eq!(p.ordered().iter().map(|c| c.id).collect::<Vec<_>>(), vec![4, 5, 2, 3, 1]);
    }

    #[test]
    fn pause_window_shrinks_learnable_days() {
        // 목표일까지 10일, 3일 쉬면 남은 7일에 나눈다
        let d = Deck { target_day: Some(T + 10), paused: true, paused_until: Some(T + 2), ..deck(1) };
        assert_eq!(resume_day(&d, T), T + 3);
        assert_eq!(new_target_for_deck(&d, T, 70).0, 10);
        let open = Deck { paused: false, ..d.clone() };
        assert_eq!(new_target_for_deck(&open, T, 70).0, 7);
    }

    #[test]
    fn paused_deck_and_its_children_are_skipped() {
        let parent = Deck { paused: true, ..deck(1) };
        let child = Deck { parent_id: Some(1), ..deck(2) };
        let mut c = news(5, 1, 1);
        c.extend(news(5, 2, 100));
        c.extend(news(5, 3, 200));
        let p = build_queue(T, &[parent, child, deck(3)], &c, None, 1.0);
        assert!(p.new.iter().all(|c| c.deck_id == 3), "쉬는 덱과 그 하위 덱은 빠진다");
        assert_eq!(p.new.len(), 5);
    }

    #[test]
    fn today_override_replaces_target_for_today_only() {
        let d = Deck { target_day: Some(T + 10), today_new: Some(3), ..deck(1) };
        assert_eq!(build_queue(T, &[d], &news(100, 1, 1), None, 1.0).new.len(), 3);
    }

    #[test]
    fn already_studied_today_is_subtracted() {
        // 100장 / 10일 = 10장. 오늘 4장 했으면 남은 96장을 다시 나누지 않고 6장만 남는다
        let d = Deck { target_day: Some(T + 10), ..deck(1) };
        let done = Today { introduced: [(1, 4)].into(), ..Default::default() };
        let p = build_queue_with_progress(T, std::slice::from_ref(&d), &news(96, 1, 1), None, 1.0, &done);
        assert_eq!(p.new.len(), 6);
        let done = Today { introduced: [(1, 10)].into(), ..Default::default() };
        let p = build_queue_with_progress(T, &[d], &news(90, 1, 1), None, 1.0, &done);
        assert_eq!(p.new.len(), 0, "오늘 몫을 다 했으면 더 나오지 않는다");
        // 상한 덱도 같다
        let d = Deck { daily_new_limit: Some(20), ..deck(1) };
        let done = Today { introduced: [(1, 15)].into(), ..Default::default() };
        assert_eq!(build_queue_with_progress(T, &[d], &news(100, 1, 1), None, 1.0, &done).new.len(), 5);
    }

    #[test]
    fn plan_adjustments_change_only_the_new_split() {
        // 목표일까지 10일, 신규 100장 = 하루 10장
        let base = Deck { target_day: Some(T + 10), ..deck(1) };
        assert_eq!(new_target_for_deck(&base, T, 100).0, 10);
        // H2 앞당겨 나누기: 5일 안에 끝낸다
        assert_eq!(new_target_for_deck(&Deck { new_until: Some(T + 5), ..base.clone() }, T, 100).0, 20);
        // H2 먼저 볼 덱: 그 날 전에는 0, 그 날부터 남은 날로 나눈다
        let later = Deck { new_from: Some(T + 6), ..base.clone() };
        assert_eq!(new_target_for_deck(&later, T, 100).0, 0);
        assert_eq!(new_target_for_deck(&later, T + 6, 100).0, 25);
        // H1 다시 나누기를 끄면 하루 몫 그대로 (남은 것보다 많이는 아니다)
        let fixed = Deck { fixed_new: Some(12), ..base.clone() };
        assert_eq!(new_target_for_deck(&fixed, T, 100).0, 12);
        assert_eq!(new_target_for_deck(&fixed, T, 5).0, 5);
        // H1 당겨 맞추기: 오늘 몫에 더하고, 큐는 남은 장수를 넘지 않는다
        let p = build_queue(T, &[Deck { new_shift: 6, ..base.clone() }], &news(100, 1, 1), None, 1.0);
        assert_eq!(p.new.len(), 16);
        let p = build_queue(T, &[Deck { new_shift: -30, ..base }], &news(100, 1, 1), None, 1.0);
        assert_eq!(p.new.len(), 0);
    }

    #[test]
    fn subdeck_budget_is_shared() {
        // 실데이터에서 발견: 하위 덱마다 상한을 주면 합산되어 터진다
        let mut decks = vec![Deck { daily_new_limit: Some(10), ..deck(1) }];
        let mut c = vec![];
        for i in 2..22 {
            decks.push(Deck { parent_id: Some(1), ..deck(i) });
            c.extend(news(30, i, i * 100));
        }
        assert_eq!(build_queue(T, &decks, &c, None, 1.0).new.len(), 10);
        decks[1].daily_new_limit = Some(3);
        assert_eq!(build_queue(T, &decks, &c, None, 1.0).new.len(), 13);
        // 상위 덱을 골라 학습하면 하위 덱 카드가 들어온다
        decks[1].daily_new_limit = None;
        assert_eq!(build_queue(T, &decks, &c, Some(&[1]), 1.0).new.len(), 10);
        // QA T3-08: 하위 덱만 골라도 예산 덱 몫은 그 아래 전체에서 고르고, 고른 덱 것만 남긴다 (상위 덱 표와 같은 수)
        let decks = [Deck { target_day: Some(T + 24), ..deck(1) }, Deck { parent_id: Some(1), ..deck(2) }, Deck { parent_id: Some(1), ..deck(3) }];
        let mut c = news(20, 2, 1);
        c.extend(news(12, 3, 100));
        assert_eq!(build_queue(T, &decks, &c, None, 1.0).new.iter().map(|c| c.deck_id).collect::<Vec<_>>(), [2, 2], "32장 ÷ 24일 = 2장");
        let sub = build_queue(T, &decks, &c, Some(&[2]), 1.0);
        assert_eq!((sub.new.len(), sub.notes[0].as_str()), (2, "d1: 남은 32장 ÷ 24일 = 하루 2장"));
        assert_eq!(build_queue(T, &decks, &c, Some(&[3]), 1.0).new.len(), 0);
        // 오늘 다른 하위 덱에서 본 신규도 같은 예산에서 뺀다
        let done = Today { introduced: [(3, 2)].into(), ..Default::default() };
        assert_eq!(build_queue_with_progress(T, &decks, &c[..30], Some(&[2]), 1.0, &done).new.len(), 0);
        // 부모가 유실돼도 멈추지 않는다
        let orphan = Deck { parent_id: Some(999), ..deck(9) };
        assert_eq!(build_queue(T, &[orphan], &news(5, 9, 1), None, 1.0).new.len(), 5);
    }
}
