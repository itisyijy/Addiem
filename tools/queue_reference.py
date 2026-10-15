"""오늘의 학습 큐 참조 구현.

docs/engineering/study-queue-spec.md 의 실행 가능한 형태다.
명세가 애매하면 이 코드가 정답이고, 코드가 바뀌면 명세도 바꾼다.

이건 스파이크 산출물이다. 실제 앱은 ADR 0001에 따라 Rust로 만든다.
순수 함수라 옮기기 쉽고, 아래 테스트는 그대로 Rust 테스트 케이스가 된다.

    python tools/queue_reference.py            # 테스트 실행
    python tools/queue_reference.py <db.sqlite> # 실제 DB로 큐 구성
"""

from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
import math
import sys

# 중요도
BOOST, NORMAL, CORE = 0, 1, 2

# 임의로 정한 값. DECISIONS-PENDING-REVIEW.md D1~D3 참고
BACKLOG_DAILY_CAP = 200      # 밀린 복습을 하루에 최대 몇 장까지 올릴 것인가
BURY_RETURN_DAYS = 3         # 보류 카드를 며칠 뒤에 다시 보여줄 것인가
DEFAULT_NEW_LIMIT = 20       # 목표일도 상한도 없는 덱의 하루 신규 카드
DAY_CUTOFF_HOUR = 4          # 하루 경계. 새벽 4시


# ------------------------------------------------------------------ 학습일

def study_day(moment: datetime) -> date:
    """시각 -> 학습일. 새벽 4시 이전은 전날로 친다.

    자정 넘겨 공부하는 사용자가 하루를 두 번 세지 않게 한다.
    """
    return (moment - timedelta(hours=DAY_CUTOFF_HOUR)).date()


def learnable_days(today: date, target: date | None, include_target_day: bool) -> int | None:
    """목표일까지 남은 학습 가능 일수. 오늘을 포함한다.

    None 이면 목표일이 없는 덱이다.
    0 이하이면 목표일이 지났거나 오늘이 마지막이다 (호출자가 처리한다).
    """
    if target is None:
        return None
    return (target - today).days + (1 if include_target_day else 0)


# ------------------------------------------------------------------ 입력

@dataclass
class Deck:
    id: int
    name: str = ""
    parent_id: int | None = None
    target_date: date | None = None
    include_target_day: bool = False
    daily_new_limit: int | None = None

    @property
    def has_config(self):
        return self.target_date is not None or self.daily_new_limit is not None


def budget_owner(deck_id, by_id):
    """신규 카드 예산을 가진 덱을 찾는다.

    하위 덱마다 상한을 따로 주면 합산되어 터진다. 실측 컬렉션은 하위 덱이 140개라
    각 20장이면 오늘 신규가 1,699장이 됐다.

    규칙: 설정을 가진 가장 가까운 조상 덱이 예산을 갖는다.
          아무도 설정이 없으면 최상위 덱이 갖는다.
    """
    node = by_id.get(deck_id)
    fallback = node
    seen = set()
    while node is not None and node.id not in seen:
        seen.add(node.id)
        if node.has_config:
            return node.id
        fallback = node
        node = by_id.get(node.parent_id) if node.parent_id else None
    return fallback.id if fallback else deck_id


@dataclass
class Card:
    id: int
    deck_id: int
    state: str                      # new | learning | review | relearning | suspended
    importance: int = NORMAL        # card_effective.eff_importance
    due_day: date | None = None     # 복습 예정일. 신규는 None
    position: int = 0               # 같은 그룹 안의 원래 순서 (Anki의 due 필드)
    buried_until: date | None = None


@dataclass
class QueuePlan:
    review: list = field(default_factory=list)      # 오늘 예정 복습
    backlog: list = field(default_factory=list)     # 밀린 복습 (오늘 올린 만큼만)
    learning: list = field(default_factory=list)    # 학습 단계 진행 중
    new: list = field(default_factory=list)         # 오늘 투입할 신규
    returning: list = field(default_factory=list)   # 이전 세션 보류에서 돌아온 카드
    backlog_deferred: int = 0                       # 상한에 걸려 오늘 안 올린 밀린 복습
    per_deck_new_target: dict = field(default_factory=dict)
    notes: list = field(default_factory=list)       # 사용자에게 보여줄 설명

    @property
    def ordered(self):
        """실제로 보여줄 순서. 복습을 먼저 비우고 신규로 간다 (D3)."""
        return self.learning + self.returning + self.review + self.backlog + self.new

    @property
    def total(self):
        return len(self.ordered)

    def summary(self):
        """학습 시작 전 요약 화면(D1)이 쓰는 값."""
        core = sum(1 for c in self.ordered if c.importance == CORE)
        return {
            "복습": len(self.review) + len(self.backlog) + len(self.learning),
            "신규": len(self.new),
            "밀린 복습": len(self.backlog),
            "보류에서 돌아옴": len(self.returning),
            "핵심": core,
            "합계": self.total,
        }


# ------------------------------------------------------------------ 신규 목표량

def new_target_for_deck(deck: Deck, today: date, remaining_new: int) -> tuple[int, str]:
    """덱 하나의 오늘 신규 카드 목표량과 그 이유.

    MVP 범위 6.6의 확정 산식:
        권장 신규 = 남은 신규 카드 수 / 남은 학습 가능 일수
    """
    if remaining_new == 0:
        return 0, "남은 신규 카드가 없다"

    days = learnable_days(today, deck.target_date, deck.include_target_day)
    cap = deck.daily_new_limit

    if days is None:
        n = cap if cap is not None else DEFAULT_NEW_LIMIT
        return min(n, remaining_new), f"목표일 없음. 하루 상한 {n}장"

    if days <= 0:
        # 목표일이 지났거나 오늘이 마지막이다. 남은 것을 전부 올리되 상한은 지킨다.
        # 상한마저 없으면 전부 올린다. 사용자가 줄일 수 있다 (6.6 사용자 통제)
        n = remaining_new if cap is None else min(cap, remaining_new)
        why = "목표일이 지났다" if days < 0 else "오늘이 목표일이다"
        return n, f"{why}. 남은 {remaining_new}장 중 {n}장"

    # 올림. 나눠떨어지지 않을 때 마지막 날에 몰리지 않게 한다
    want = math.ceil(remaining_new / days)
    if cap is not None and want > cap:
        # 상한이 이기면 목표일을 못 맞춘다. 숨기지 않고 알린다 (설명 가능성)
        return cap, (f"목표일까지 하루 {want}장이 필요한데 상한이 {cap}장이다. "
                     f"이대로면 목표일을 넘긴다")
    return min(want, remaining_new), f"남은 {remaining_new}장 / {days}일 = 하루 {want}장"


# ------------------------------------------------------------------ 큐 구성

def build_queue(today: date, decks, cards, deck_ids=None, new_scale: float = 1.0) -> QueuePlan:
    """오늘의 큐를 만든다. 순수 함수다.

    deck_ids  : 학습할 덱을 제한한다. None 이면 전부
    new_scale : 사용자가 분량을 조정한 배수 (6.6 사용자 통제)
    """
    plan = QueuePlan()
    by_id = {d.id: d for d in decks}
    wanted = set(deck_ids) if deck_ids is not None else set(by_id)

    pool = [c for c in cards if c.deck_id in wanted and c.state != "suspended"]

    # --- 1단계. 후보 수집
    for c in pool:
        # 보류는 상태가 아니라 기간이다. 보류 전 상태를 그대로 유지한다
        if c.buried_until is not None:
            if c.buried_until <= today:
                plan.returning.append(c)
            continue        # 기한 전이면 오늘 큐에 없다
        if c.state == "learning":
            plan.learning.append(c)
        elif c.state in ("review", "relearning"):
            if c.due_day is None or c.due_day > today:
                continue
            (plan.backlog if c.due_day < today else plan.review).append(c)

    # --- 2단계. 신규 목표량.
    # 하위 덱별로 세지 않고 예산을 가진 조상 덱 단위로 묶는다
    new_by_owner = {}
    for c in pool:
        if c.state == "new":
            new_by_owner.setdefault(budget_owner(c.deck_id, by_id), []).append(c)

    for owner, group in new_by_owner.items():
        target, why = new_target_for_deck(by_id[owner], today, len(group))
        if new_scale != 1.0:
            target = max(0, min(len(group), round(target * new_scale)))
            why += f" (사용자 조정 x{new_scale:g})"
        plan.per_deck_new_target[owner] = target
        plan.notes.append(f"{by_id[owner].name or owner}: {why}")

        # --- 3단계. 정렬. 중요도 높은 순, 같으면 원래 순서
        group.sort(key=lambda c: (-c.importance, c.position, c.id))
        plan.new.extend(group[:target])

    # --- 3단계(이어서). 복습은 오래 밀린 것부터, 같으면 중요도 높은 순
    key = lambda c: (c.due_day or today, -c.importance, c.position, c.id)
    plan.review.sort(key=key)
    plan.backlog.sort(key=key)
    plan.learning.sort(key=key)
    plan.returning.sort(key=lambda c: (-c.importance, c.position, c.id))

    # --- 4단계. 상한. 밀린 복습이 수백 장 쌓여도 오늘을 마비시키지 않는다
    if len(plan.backlog) > BACKLOG_DAILY_CAP:
        plan.backlog_deferred = len(plan.backlog) - BACKLOG_DAILY_CAP
        plan.backlog = plan.backlog[:BACKLOG_DAILY_CAP]
        plan.notes.append(
            f"밀린 복습 {plan.backlog_deferred + BACKLOG_DAILY_CAP}장 중 {BACKLOG_DAILY_CAP}장만 "
            f"오늘 올렸다. 나머지는 차차 처리한다")

    # 보류에서 돌아온 카드는 신규 상한을 소비하지 않는다.
    # 이미 시작한 카드이고, 진도를 막지 않는 것이 6.8의 목적이다
    return plan


# ------------------------------------------------------------------ 보류 판정

def on_rating(session_again_count: int, rating: int) -> tuple[str, bool]:
    """평가 하나를 처리한다 -> (다음 상태, SRS 스케줄러에 넘길 것인가).

    MVP 6.8: 같은 세션에서 두 번 Again이면 보류한다.
    2회차 Again은 기록은 남기되 스케줄러에 넘기지 않는다 (A5).
    """
    if rating != 1:                       # Again이 아니면 평범하게 진행
        return "continue", True
    if session_again_count + 1 >= 2:
        return "bury", False
    return "continue", True


def bury_until(today: date, user_choice: str) -> date | None:
    """세션 말에 사용자가 고른 결과 -> 언제 다시 보여줄 것인가.

    user_choice: 'today' 오늘 다시 (기한을 비워 바로 다시 나온다) | 'later' 다음 학습으로
    """
    return None if user_choice == "today" else today + timedelta(days=BURY_RETURN_DAYS)


# ================================================================== 테스트

def _cards(spec, deck_id=1, start=1):
    """('new', 중요도, due오프셋) 목록을 Card 리스트로."""
    out, i = [], start
    for state, imp, off in spec:
        due = None if off is None else date(2026, 9, 20) + timedelta(days=off)
        out.append(Card(id=i, deck_id=deck_id, state=state, importance=imp,
                        due_day=due, position=i))
        i += 1
    return out


def test():
    T = date(2026, 9, 20)
    ok = 0

    # ---- 학습일 경계
    assert study_day(datetime(2026, 9, 20, 3, 59)) == date(2026, 9, 19), "새벽 4시 전은 전날"
    assert study_day(datetime(2026, 9, 20, 4, 0)) == date(2026, 9, 20)
    assert study_day(datetime(2026, 9, 20, 23, 59)) == date(2026, 9, 20)
    ok += 3

    # ---- 학습 가능 일수
    assert learnable_days(T, date(2026, 9, 30), False) == 10
    assert learnable_days(T, date(2026, 9, 30), True) == 11, "당일 포함이면 하루 더"
    assert learnable_days(T, T, False) == 0, "오늘이 목표일이고 당일 제외"
    assert learnable_days(T, date(2026, 9, 10), False) == -10, "목표일이 지났다"
    assert learnable_days(T, None, False) is None
    ok += 5

    # ---- 시나리오 1. 목표일이 있는 덱의 첫 학습 (명세 9장)
    d = Deck(1, "시험덱", target_date=T + timedelta(days=10))
    cards = _cards([("new", NORMAL, None)] * 100)
    p = build_queue(T, [d], cards)
    assert len(p.new) == 10, f"100장 / 10일 = 10장, got {len(p.new)}"
    assert p.review == [] and p.backlog == []
    ok += 2

    # 중요도가 높은 것이 먼저 들어간다
    cards = _cards([("new", BOOST, None)] * 50 + [("new", CORE, None)] * 50)
    p = build_queue(T, [d], cards)
    assert all(c.importance == CORE for c in p.new), "핵심이 먼저"
    ok += 1

    # ---- 나눠떨어지지 않으면 올림. 마지막 날에 몰리지 않는다
    d3 = Deck(1, "d", target_date=T + timedelta(days=3))
    p = build_queue(T, [d3], _cards([("new", NORMAL, None)] * 10))
    assert len(p.new) == 4, f"10/3 -> 올림 4, got {len(p.new)}"
    ok += 1

    # ---- 상한이 목표량보다 작으면 상한이 이기고, 사용자에게 알린다
    dc = Deck(1, "d", target_date=T + timedelta(days=2), daily_new_limit=3)
    p = build_queue(T, [dc], _cards([("new", NORMAL, None)] * 100))
    assert len(p.new) == 3
    assert any("목표일을 넘긴다" in n for n in p.notes), "숨기지 않고 알려야 한다"
    ok += 2

    # ---- 엣지: 목표일이 지났다 (명세 8장)
    dp = Deck(1, "d", target_date=T - timedelta(days=5))
    p = build_queue(T, [dp], _cards([("new", NORMAL, None)] * 7))
    assert len(p.new) == 7, "남은 것을 전부 올린다"
    assert any("목표일이 지났다" in n for n in p.notes)
    ok += 2

    # ---- 엣지: 오늘이 목표일이고 당일 제외 -> 나누기 0이 나면 안 된다
    dt = Deck(1, "d", target_date=T, include_target_day=False)
    p = build_queue(T, [dt], _cards([("new", NORMAL, None)] * 5))
    assert len(p.new) == 5
    ok += 1
    # 당일 포함이면 하루치로 나눈다
    dt2 = Deck(1, "d", target_date=T, include_target_day=True)
    p = build_queue(T, [dt2], _cards([("new", NORMAL, None)] * 5))
    assert len(p.new) == 5, "1일 남았으니 5장 전부"
    ok += 1

    # ---- 엣지: 카드가 없다 / 볼 카드가 없다
    p = build_queue(T, [Deck(1, "빈덱")], [])
    assert p.total == 0 and p.summary()["합계"] == 0
    ok += 1

    # ---- 엣지: 하루 상한이 0이면 신규가 안 나온다
    dz = Deck(1, "d", daily_new_limit=0)
    p = build_queue(T, [dz], _cards([("new", NORMAL, None)] * 10))
    assert len(p.new) == 0
    ok += 1

    # ---- 시나리오 4. 밀린 복습이 쌓였다 (명세 9장)
    backlog = _cards([("review", NORMAL, -d) for d in range(1, 301)])
    p = build_queue(T, [Deck(1, "d")], backlog)
    assert len(p.backlog) == BACKLOG_DAILY_CAP, "상한까지만"
    assert p.backlog_deferred == 300 - BACKLOG_DAILY_CAP
    assert p.backlog[0].due_day < p.backlog[-1].due_day, "오래 밀린 것부터"
    assert any("차차 처리" in n for n in p.notes)
    ok += 4

    # 밀린 복습이 많아도 신규는 계속 나온다. 진도가 막히면 안 된다
    mixed = backlog + _cards([("new", NORMAL, None)] * 20, start=1000)
    p = build_queue(T, [Deck(1, "d", daily_new_limit=5)], mixed)
    assert len(p.new) == 5, "밀린 복습이 신규를 막지 않는다"
    ok += 1

    # ---- 예정 복습과 밀린 복습이 구분된다
    p = build_queue(T, [Deck(1, "d")], _cards([("review", NORMAL, 0), ("review", NORMAL, -3),
                                               ("review", NORMAL, 5)]))
    assert len(p.review) == 1 and len(p.backlog) == 1, "미래 예정은 오늘 큐에 없다"
    ok += 2

    # ---- 시나리오 2. 보류 카드 (명세 9장)
    assert on_rating(0, 1) == ("continue", True), "1회차 Again은 정상 진행"
    assert on_rating(1, 1) == ("bury", False), "2회차 Again은 보류, 스케줄러에 안 넘김"
    assert on_rating(1, 3) == ("continue", True), "Good이면 카운트와 무관"
    ok += 3
    assert bury_until(T, "today") is None, "오늘 다시는 기한을 비운다 (앱과 같다)"
    assert bury_until(T, "later") == T + timedelta(days=BURY_RETURN_DAYS)
    ok += 2

    # 보류 기한이 안 됐으면 오늘 큐에 없다
    c_b = Card(1, 1, "review", due_day=T, buried_until=T + timedelta(days=2))
    c_r = Card(2, 1, "review", due_day=T, buried_until=T)
    p = build_queue(T, [Deck(1, "d")], [c_b, c_r])
    assert [c.id for c in p.returning] == [2]
    ok += 1

    # 보류에서 돌아온 카드는 신규 상한을 소비하지 않는다
    p = build_queue(T, [Deck(1, "d", daily_new_limit=3)],
                    [c_r] + _cards([("new", NORMAL, None)] * 10, start=100))
    assert len(p.new) == 3 and len(p.returning) == 1
    ok += 1

    # ---- 시나리오 3. 목표일 있는 덱과 없는 덱을 함께 (명세 9장)
    d_goal = Deck(1, "목표", target_date=T + timedelta(days=5))
    d_free = Deck(2, "장기", daily_new_limit=7)
    cards = (_cards([("new", NORMAL, None)] * 50, deck_id=1)
             + _cards([("new", NORMAL, None)] * 50, deck_id=2, start=500))
    p = build_queue(T, [d_goal, d_free], cards)
    assert p.per_deck_new_target[1] == 10, "50/5"
    assert p.per_deck_new_target[2] == 7, "상한"
    assert len(p.new) == 17
    ok += 3

    # 덱을 골라 학습하면 나머지는 빠진다
    p = build_queue(T, [d_goal, d_free], cards, deck_ids=[2])
    assert len(p.new) == 7 and all(c.deck_id == 2 for c in p.new)
    ok += 1

    # ---- 사용자 분량 조정
    p = build_queue(T, [d_goal], _cards([("new", NORMAL, None)] * 50), new_scale=0.5)
    assert len(p.new) == 5, "10장의 절반"
    p = build_queue(T, [d_goal], _cards([("new", NORMAL, None)] * 50), new_scale=2.0)
    assert len(p.new) == 20
    ok += 2

    # ---- 보류/정지 카드는 기본 제외
    p = build_queue(T, [Deck(1, "d")], [Card(1, 1, "suspended", due_day=T)])
    assert p.total == 0, "정지 카드는 절대 안 나온다"
    ok += 1

    # ---- 순서: 학습중 -> 보류복귀 -> 예정복습 -> 밀린복습 -> 신규
    p = build_queue(T, [Deck(1, "d")], [
        Card(1, 1, "new"), Card(2, 1, "review", due_day=T),
        Card(3, 1, "review", due_day=T - timedelta(days=1)),
        Card(4, 1, "learning", due_day=T),
        Card(5, 1, "review", due_day=T, buried_until=T)])
    assert [c.id for c in p.ordered] == [4, 5, 2, 3, 1]
    ok += 1

    # 보류해도 원래 상태를 잃지 않는다 (스파이크 5에서 발견한 설계 결함)
    rev = Card(1, 1, "review", due_day=T, buried_until=T)
    p = build_queue(T, [Deck(1, "d")], [rev])
    assert p.returning[0].state == "review", "복귀할 때 review였는지 learning이었는지 알아야 한다"
    ok += 1

    # ---- 하위 덱이 많아도 신규 상한이 합산되지 않는다 (실데이터에서 발견)
    root = Deck(1, "과목", daily_new_limit=10)
    subs = [Deck(i, f"챕터{i}", parent_id=1) for i in range(2, 22)]   # 하위 덱 20개
    cards = []
    for i, sd in enumerate(subs):
        cards += _cards([("new", NORMAL, None)] * 30, deck_id=sd.id, start=i * 100 + 1)
    p = build_queue(T, [root] + subs, cards)
    assert len(p.new) == 10, f"하위 덱 20개여도 상한은 10장, got {len(p.new)}"
    ok += 1

    # 하위 덱이 자기 설정을 가지면 그 덱은 따로 예산을 받는다
    subs[0].daily_new_limit = 3
    p = build_queue(T, [root] + subs, cards)
    assert len(p.new) == 13, f"10 + 3, got {len(p.new)}"
    ok += 1

    # 예산 덱을 못 찾아도 무한 루프에 빠지지 않는다 (부모가 순환/유실)
    orphan = Deck(9, "고아", parent_id=999)
    p = build_queue(T, [orphan], _cards([("new", NORMAL, None)] * 5, deck_id=9))
    assert len(p.new) == 5
    ok += 1

    # 예산 덱이 목표일을 가지면 하위 덱 카드 전체가 그 기준으로 분배된다
    root2 = Deck(1, "시험", target_date=T + timedelta(days=10))
    subs2 = [Deck(i, f"c{i}", parent_id=1) for i in range(2, 6)]
    cards2 = []
    for i, sd in enumerate(subs2):
        cards2 += _cards([("new", NORMAL, None)] * 25, deck_id=sd.id, start=i * 100 + 1)
    p = build_queue(T, [root2] + subs2, cards2)
    assert len(p.new) == 10, f"전체 100장 / 10일, got {len(p.new)}"
    ok += 1

    print(f"큐 참조 구현 테스트 통과: {ok}건")


def from_db(path):
    """실제 적재된 DB로 큐를 구성해본다."""
    import sqlite3
    con = sqlite3.connect(path)
    today = study_day(datetime.now())
    decks, cards = [], []
    for did, name, parent in con.execute("SELECT id, name, parent_id FROM deck"):
        # 설정은 최상위 덱에만 준다. 하위 덱은 상속한다
        decks.append(Deck(did, name, parent_id=parent,
                          daily_new_limit=None if parent else DEFAULT_NEW_LIMIT))
    for cid, did, state, imp, due, pos, bur in con.execute(
            """SELECT id, deck_id, state, eff_importance, due_at, anki_card_id, buried_until
               FROM card_effective"""):
        cards.append(Card(cid, did, state, imp,
                          date.fromtimestamp(due / 1000) if due else None, pos or 0,
                          date.fromtimestamp(bur / 1000) if bur else None))
    p = build_queue(today, decks, cards)
    print(f"\n실제 DB로 큐 구성  ({path})")
    print(f"  학습일 {today},  카드 {len(cards):,}장,  덱 {len(decks)}개")
    for k, v in p.summary().items():
        print(f"    {k:<16}{v:>8,}")
    print(f"  오늘 상위 5장의 중요도: {[c.importance for c in p.ordered[:5]]}")


if __name__ == "__main__":
    test()
    if len(sys.argv) > 1:
        from_db(sys.argv[1])
