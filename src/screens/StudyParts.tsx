// 학습 화면 묶음의 작은 조각: H3 압축 회독 안내 줄 (홈 · D1) · G3 [나누기 제안] 칩 (D5)과 "나중에 볼 제안"
import { useEffect, useState } from "react";
import { cramApi, type CramPlan } from "../api";
import { Button, Icon } from "../ui";
import { n, studyDate, useNav } from "../nav";
import "../styles/study.css";

// ---------------------------------------------------------------- H3 압축 회독 안내 (홈 · D1)

/** 목표일 3일 전부터 "압축 회독을 할 수 있어요" 한 줄 + [압축 회독]. 열 수 없으면 아무것도 그리지 않는다 */
export function CramHint({ deckIds }: { deckIds: number[] | null }) {
  const nav = useNav();
  const [plan, setPlan] = useState<CramPlan | null>(null);
  useEffect(() => {
    cramApi.plan(deckIds).then(setPlan, () => setPlan(null));
  }, [deckIds]);
  if (!plan) return null;
  return (
    <div className="cram-hint">
      <Icon name="target" size={16} className="c-accent" />
      <span className="cram-hint__text">
        <span className="t-label-m">압축 회독을 할 수 있어요</span>
        <span className="t-caption c-tertiary">
          {plan.name} {plan.dday === 0 ? "시험이 오늘이에요" : `시험까지 ${n(plan.dday)}일`} · 불안한 카드 {n(plan.scopes[2])}장
        </span>
      </span>
      <Button variant="secondary" size="M" label="압축 회독" onClick={() => nav({ name: "summary", deckIds, cram: true })} />
    </div>
  );
}

// ---------------------------------------------------------------- G3 "나중에 볼 제안" (이 기기에서만)

const SIGNALED = "neo.splitSignaled"; // { 카드 id: 신호를 보인 학습일 }
const LATER = "neo.splitLater"; // [카드 id] — D5 접어둔 카드 행에 [나누기 제안] 칩
const today = () => studyDate().toLocaleDateString("sv-SE"); // 로컬 YYYY-MM-DD (toISOString 은 UTC 라 하루 어긋난다)

function read<T>(key: string, fallback: T): T {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "null") ?? fallback;
  } catch {
    return fallback;
  }
}
function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* 저장이 막힌 환경. 신호가 하루 두 번 보이거나 칩이 빠질 뿐이다 */
  }
}

/** 같은 카드에 신호는 하루 한 번. 오늘 처음이면 true 를 돌려주고 기록한다 */
export function takeSignal(cardId: number) {
  const seen = read<Record<string, string>>(SIGNALED, {});
  if (seen[cardId] === today()) return false;
  // 지난 날 기록은 버린다 (쌓이지 않게)
  write(SIGNALED, { ...Object.fromEntries(Object.entries(seen).filter(([, d]) => d === today())), [cardId]: today() });
  return true;
}

export const splitLater = {
  ids: () => read<number[]>(LATER, []),
  has: (cardId: number) => splitLater.ids().includes(cardId),
  add: (cardId: number) => write(LATER, [...new Set([...splitLater.ids(), cardId])]),
  /** G2 제안 창에서 나누거나 그대로 두기로 정했을 때 */
  remove: (cardId: number) => write(LATER, splitLater.ids().filter((id) => id !== cardId)),
};

// ---------------------------------------------------------------- G3 [나누기 제안] 칩 (D5 접어둔 카드 행)

/**
 * 학습 중 [나누기 제안]을 누른 카드에만 보인다 (이 기기의 "나중에 볼 제안" 목록).
 * onOpen 에서 G2 나누기 제안 창을 연다. 나누거나 그대로 두기로 정하면 splitLater.remove(cardId)
 */
export function SplitLaterChip({ cardId, onOpen }: { cardId: number; onOpen: () => void }) {
  if (!splitLater.has(cardId)) return null;
  return (
    <button className="split-chip t-label-s" onClick={onOpen}>
      <Icon name="layers" size={14} />
      나누기 제안
    </button>
  );
}
