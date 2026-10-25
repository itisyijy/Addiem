# 스파이크 5. FSRS 적합성

## 문서 상태

- 실행일: 2026-09-20
- 대상: `fsrs` (Python, Open Spaced Repetition)
- 목적: [ADR 0001](../adr/0001-tech-stack.md) 6장과 7장에 답한다
- 상태: **완료.** 4개 질문 모두 답을 얻었고 스키마 결함 2개를 발견했다

## 왜 Python으로 확인했나

Rust 툴체인이 없어 `fsrs-rs`를 직접 돌릴 수 없었다. 대신 같은 조직(Open Spaced Repetition)의 Python 구현으로 확인했다.

**알고리즘과 API 모양이 동일하므로 아래 답은 그대로 옮겨진다.** 다만 `fsrs-rs`의 실제 함수 시그니처와 라이선스는 Rust 환경이 준비되면 재확인해야 한다.

## 1. ADR 6장의 네 가지 질문

### Q1. Again / Hard / Good / Easy 네 등급을 그대로 입력으로 받는가

**받는다. 값까지 우리 스키마와 정확히 일치한다.**

```python
Rating = {'Again': 1, 'Hard': 2, 'Good': 3, 'Easy': 4}
```

`review_log.rating INTEGER CHECK (rating BETWEEN 1 AND 4)` 와 그대로 맞는다. 변환 계층이 필요 없다.

MVP 6.7의 평가 기준도 사용자 진술과 일치한다.

> 아예 모르겠으면 again, 몇개가 기억이 안나거나 디테일 부족할 때 hard, 기억해냈으면 good, 보자마자 모든게 떠오르고 쉽다면 easy

### Q2. 스케줄러 파라미터를 덱별로 다르게 줄 수 있는가

**줄 수 있다.** 스케줄러가 생성자에서 전부 받는다.

| 파라미터 | 기본값 |
|---|---|
| `parameters` | 21개 튜플 (FSRS 가중치) |
| `desired_retention` | 0.9 |
| `learning_steps` | 2단계 (1분, 10분) |
| `relearning_steps` | 1단계 |
| `maximum_interval` | 36500일 |
| `enable_fuzzing` | True |

덱별로 다른 스케줄러 인스턴스를 만들면 된다. 스케줄러는 카드 상태를 안에 들고 있지 않으므로 인스턴스를 여러 개 두어도 문제가 없다.

**`enable_fuzzing`은 꺼야 한다.** 같은 입력에 다른 결과가 나오면 큐 테스트가 불가능하다. 실사용에서는 켜도 되지만 테스트에서는 끈다.

### Q3. 학습 단계를 라이브러리가 관리하는가, 호출자가 관리하는가

**라이브러리가 관리한다.** 이게 ADR 7장의 분담과 정확히 맞는다.

```text
새 카드 → Learning
  Again  -> Learning   1분 뒤    stability 0.21  difficulty 6.41
  Again  -> Learning   1분 뒤    stability 0.08  difficulty 8.81
  Good   -> Learning  10분 뒤    stability 0.10  difficulty 8.79
  Good   -> Review     1일 뒤    stability 0.13  difficulty 8.78
  Good   -> Review     1일 뒤    stability 0.15  difficulty 8.77
```

`State`는 `Learning / Review / Relearning` 세 가지이고, 단계 안의 위치는 `Card.step` 이 들고 있다.

ADR 7장에서 "Again 직후 같은 세션 내 재등장 = SRS 담당"이라고 정한 것이 그대로 성립한다.

### Q4. 라이선스

**MIT.** Copyright (c) 2022 Open Spaced Repetition. 배포하는 데스크톱 앱에 제약이 없다.

## 2. 큐가 필요로 하는 것

```python
new_card, log = scheduler.review_card(card, rating, review_datetime=None, review_duration=None)
```

**카드 한 장만 넘기고 한 장만 돌려받는다.** "오늘 볼 카드 목록"을 반환하는 API가 아니다.

ADR 7장의 경계 한 줄이 그대로 성립한다.

> SRS 라이브러리는 카드 한 장의 다음 일정만 계산한다.
> 오늘 무엇을 어떤 순서로 보여줄지는 전부 Addiem 큐가 결정한다.

보류를 유발한 2회차 Again을 스케줄러에 넘기지 않는 것도 간단하다. **`review_card`를 호출하지 않으면 된다.** 카드 상태가 그대로 유지된다.

## 3. 발견한 스키마 결함 두 개

이 스파이크의 실질적 성과다. 코드를 짜기 전에 발견해서 고쳤다.

### 결함 1. `Card.step` 을 저장할 자리가 없었다

FSRS의 카드 상태는 이렇게 구성된다.

```text
card_id, state, step, stability, difficulty, due, last_review
```

`step`은 learning/relearning 단계 안의 위치다. 스키마에 `stability`, `difficulty`, `due_at`, `last_reviewed_at` 은 있었지만 **`step`이 없었다.**

없으면 앱을 껐다 켰을 때 "1분 단계였는지 10분 단계였는지"를 복원하지 못한다. 학습 중인 카드가 전부 처음 단계로 되돌아간다.

→ `card.srs_step INTEGER` 추가.

### 결함 2. `state = 'buried'` 가 보류 전 상태를 지웠다

보류(MVP 6.8)를 카드 상태의 한 값으로 두었는데, 그러면 보류 전이 `review` 였는지 `learning` 이었는지 잃어버린다. 3일 뒤 복귀할 때 어떤 상태로 돌려놓을지 알 수 없다.

**보류는 상태가 아니라 기간이다.**

```sql
-- 전
state CHECK (state IN ('new','learning','review','relearning','buried','suspended'))

-- 후
state CHECK (state IN ('new','learning','review','relearning','suspended'))
buried_until  INTEGER,
buried_reason TEXT CHECK (buried_reason IN ('two_again','user'))
```

큐는 `buried_until` 이 오늘 이하인지만 보고, 카드의 원래 상태는 건드리지 않는다. 참조 구현에 이 불변식을 검사하는 테스트를 추가했다.

## 4. 남은 확인 사항

| 항목 | 언제 |
|---|---|
| `fsrs-rs` 의 실제 API 시그니처 | Rust 환경 준비 후 |
| `fsrs-rs` 라이선스 재확인 | 같음 |
| Anki의 `ivl` / `factor` → FSRS 상태 매핑 | 복습 이력 이전을 구현할 때 |
| 최적 파라미터 학습 기능이 있는가 | Later. MVP는 기본 파라미터로 충분하다 |

세 번째가 실질적으로 남은 과제다. 실측 컬렉션에 복습 기록 7,745건이 있는데, SM-2 기반 `ivl`/`factor` 를 FSRS의 `stability`/`difficulty` 로 정확히 변환하는 일반해는 없다. 선택지는 이렇다.

| 방식 | 결과 |
|---|---|
| `revlog` 를 처음부터 재생한다 | 가장 정확하다. 7,745건이면 비용도 감당된다 |
| `ivl` 을 초기 stability로 근사한다 | 간단하지만 difficulty 추정이 없다 |
| 새로 시작한다 | MVP 6.4.2가 이미 허용한 경로다 |

**재생 방식을 먼저 시도해볼 만하다.** FSRS가 카드 한 장의 평가를 순서대로 받을 수 있으므로, 기존 `revlog` 를 시간순으로 먹이면 현재 상태가 자연스럽게 나온다. 이 스파이크에서 그 가능성은 확인했다.

## 5. 재현

```bash
pip install fsrs
python - <<'EOF'
from fsrs import Scheduler, Card, Rating
s, c = Scheduler(enable_fuzzing=False), Card()
for r in (Rating.Again, Rating.Again, Rating.Good, Rating.Good):
    c, log = s.review_card(c, r)
    print(c.state.name, c.step, round(c.stability, 2), round(c.difficulty, 2))
EOF
```
