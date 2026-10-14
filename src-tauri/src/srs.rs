//! 카드 한 장의 다음 일정. ADR 0001 7장: SRS 는 카드 한 장만 다루고, 무엇을 언제 보여줄지는 큐가 정한다.
//!
//! 기억 상태(stability, difficulty)와 간격은 fsrs 크레이트가 계산한다.
//! fsrs 크레이트는 학습 단계를 다루지 않으므로, 단계 전이는 py-fsrs 6 의 Scheduler 규칙을 그대로 옮겼다
//! (스파이크 5 에서 검증한 동작: 새 카드 Again 1분, Good 10분, 다음 Good 졸업).

use crate::db::DAY_MS;
use fsrs::{MemoryState, FSRS};

const MIN: i64 = 60_000;
pub const LEARNING_STEPS: [i64; 2] = [MIN, 10 * MIN];
pub const RELEARNING_STEPS: [i64; 1] = [10 * MIN];
pub const DESIRED_RETENTION: f32 = 0.9;

#[derive(Debug, Clone, PartialEq)]
pub struct SrsState {
    pub state: String, // new | learning | review | relearning
    pub step: Option<u32>,
    pub stability: Option<f32>,
    pub difficulty: Option<f32>,
    pub due_at: Option<i64>,
    pub interval_days: f64,
    pub last_reviewed_at: Option<i64>,
    pub reps: i64,
    pub lapses: i64,
}

#[cfg(test)]
impl SrsState {
    pub fn new_card() -> Self {
        Self {
            state: "new".into(),
            step: None,
            stability: None,
            difficulty: None,
            due_at: None,
            interval_days: 0.0,
            last_reviewed_at: None,
            reps: 0,
            lapses: 0,
        }
    }
}

/// rating: 1=Again 2=Hard 3=Good 4=Easy
pub fn review(fsrs: &FSRS, card: &SrsState, rating: u8, now: i64) -> SrsState {
    let rating = rating.clamp(1, 4);
    let memory = match (card.stability, card.difficulty) {
        (Some(s), Some(d)) => Some(MemoryState { stability: s, difficulty: d }),
        _ => None,
    };
    let elapsed_days = card
        .last_reviewed_at
        .map(|t| ((now - t).max(0) / DAY_MS) as u32)
        .unwrap_or(0);
    let next = fsrs
        .next_states(memory, DESIRED_RETENTION, elapsed_days)
        .expect("FSRS 상태 계산 실패");
    let chosen = match rating {
        1 => &next.again,
        2 => &next.hard,
        3 => &next.good,
        _ => &next.easy,
    };
    let days = |interval: f32| (interval.round() as i64).max(1);

    let mut out = card.clone();
    out.stability = Some(chosen.memory.stability);
    out.difficulty = Some(chosen.memory.difficulty);
    out.last_reviewed_at = Some(now);
    out.reps += 1;

    let graduate = |out: &mut SrsState| {
        let d = days(chosen.interval);
        out.state = "review".into();
        out.step = None;
        out.interval_days = d as f64;
        out.due_at = Some(now + d * DAY_MS);
    };
    let at_step = |out: &mut SrsState, steps: &[i64], step: usize| {
        out.step = Some(step as u32);
        out.interval_days = 0.0;
        out.due_at = Some(now + steps[step]);
    };

    match card.state.as_str() {
        "review" => {
            if rating == 1 {
                out.lapses += 1;
                out.state = "relearning".into();
                at_step(&mut out, &RELEARNING_STEPS, 0);
            } else {
                graduate(&mut out);
            }
        }
        state => {
            // new, learning, relearning: 단계를 밟는다
            let steps: &[i64] = if state == "relearning" { &RELEARNING_STEPS } else { &LEARNING_STEPS };
            out.state = if state == "relearning" { "relearning" } else { "learning" }.into();
            let s = card.step.unwrap_or(0) as usize;
            match rating {
                1 => at_step(&mut out, steps, 0),
                2 => {
                    // py-fsrs: 첫 단계에서 Hard 는 첫 두 단계의 평균, 단계가 하나뿐이면 1.5배
                    out.step = Some(s as u32);
                    out.interval_days = 0.0;
                    let wait = if s == 0 && steps.len() >= 2 {
                        (steps[0] + steps[1]) / 2
                    } else if s == 0 {
                        steps[0] * 3 / 2
                    } else {
                        steps[s.min(steps.len() - 1)]
                    };
                    out.due_at = Some(now + wait);
                }
                3 => {
                    if s + 1 >= steps.len() {
                        graduate(&mut out);
                    } else {
                        at_step(&mut out, steps, s + 1);
                    }
                }
                _ => graduate(&mut out),
            }
        }
    }
    out
}

/// Anki 의 SM-2 상태(간격, ease) -> FSRS 기억 상태. 복습 이력을 이전할 때 쓴다 (import.rs)
pub fn from_sm2(fsrs: &FSRS, interval_days: f32, ease_permille: i64) -> Option<MemoryState> {
    if interval_days <= 0.0 || ease_permille <= 0 {
        return None;
    }
    fsrs.memory_state_from_sm2(ease_permille as f32 / 1000.0, interval_days, DESIRED_RETENTION)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;

    #[test]
    fn new_card_steps_match_spike_5() {
        // 스파이크 5 의 기록: Again 1분, Again 1분, Good 10분, Good 졸업(1일)
        let f = FSRS::default();
        let mut c = SrsState::new_card();
        let mut now = T0;
        let mut waits = vec![];
        for r in [1, 1, 3, 3] {
            c = review(&f, &c, r, now);
            waits.push((c.state.clone(), (c.due_at.unwrap() - now) / MIN));
            now = c.due_at.unwrap();
        }
        assert_eq!(waits[0], ("learning".into(), 1));
        assert_eq!(waits[1], ("learning".into(), 1));
        assert_eq!(waits[2], ("learning".into(), 10));
        assert_eq!(waits[3].0, "review");
        assert!(waits[3].1 >= 24 * 60, "졸업하면 하루 이상");
        assert_eq!(c.reps, 4);
        assert!(c.stability.is_some() && c.difficulty.is_some());
    }

    #[test]
    fn easy_graduates_immediately() {
        let f = FSRS::default();
        let c = review(&f, &SrsState::new_card(), 4, T0);
        assert_eq!(c.state, "review");
        assert!(c.interval_days >= 1.0);
    }

    #[test]
    fn lapse_goes_to_relearning_then_back() {
        let f = FSRS::default();
        let c = review(&f, &SrsState::new_card(), 4, T0);
        let t1 = c.due_at.unwrap();
        let c = review(&f, &c, 1, t1);
        assert_eq!((c.state.as_str(), c.lapses), ("relearning", 1));
        assert_eq!(c.due_at.unwrap() - t1, 10 * MIN);
        let c = review(&f, &c, 3, c.due_at.unwrap());
        assert_eq!(c.state, "review", "재학습 단계가 하나뿐이라 Good 한 번에 복귀");
    }

    #[test]
    fn longer_success_gives_longer_interval() {
        let f = FSRS::default();
        let c = review(&f, &SrsState::new_card(), 4, T0);
        let i1 = c.interval_days;
        let c = review(&f, &c, 3, c.due_at.unwrap());
        assert!(c.interval_days > i1, "Good 을 이어가면 간격이 늘어난다");
    }

    #[test]
    fn sm2_conversion() {
        let f = FSRS::default();
        let m = from_sm2(&f, 10.0, 2500).unwrap();
        assert!(m.stability > 1.0 && (1.0..=10.0).contains(&m.difficulty));
        assert!(from_sm2(&f, 0.0, 2500).is_none(), "신규 카드는 변환하지 않는다");
    }
}
