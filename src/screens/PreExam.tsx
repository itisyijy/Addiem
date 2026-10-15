// P1 시험 전 회독 (방식 [압축 | 혼합]) · P3 혼합 회독 끝난 뒤. 압축은 H3 그대로, 혼합은 8.5 (cram.rs)
import { useCallback, useEffect, useState } from "react";
import { api, errorText } from "../api";
import { Badge, Button, Icon, Segment, StatTile } from "../ui";
import { eul, n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { cram as cramApi, type CramPlan, type MixCard, type MixRun, type MixStats, type MixSummary } from "../written";
import "../styles/study.css";

const CRAM_SCOPES = ["핵심만", "핵심 + 약한 챕터", "불안한 카드 전부"];
/** P2 취약 비율: 전체 몇 장마다 취약 1장 (1/5 · 1/3 · 1/2) */
const RATIOS = [
  { label: "적게", every: 4 },
  { label: "보통", every: 2 },
  { label: "많이", every: 1 },
];
const SIZES = [60, 120, 200];
const MAX_SIZE = 500; // mix_cards 상한

export function PreExam({ deckIds }: { deckIds: number[] | null }) {
  const nav = useNav();
  const [plan, setPlan] = useState<CramPlan | null>();
  const [mode, setMode] = useState<"cram" | "mix">("cram");
  const [scope, setScope] = useState(2);
  const [every, setEvery] = useState(2);
  const [size, setSize] = useState(120);
  // 지금 돌 수 있는 순서 전부 (한 번에 할 양은 앞에서 자른다. 작은 분량은 큰 분량의 앞부분이다)
  const [all, setAll] = useState<MixCard[]>([]);
  const [error, setError] = useState<string>();
  const load = useCallback(
    () =>
      cramApi.plan(deckIds).then((p) => {
        setPlan(p);
        if (p) setMode(p.mode);
      }, (e) => setError(errorText(e))),
    [deckIds],
  );
  useEffect(() => void load(), [load]);
  useEffect(() => {
    if (plan && mode === "mix") cramApi.mixCards(plan.deck_id, every, MAX_SIZE).then(setAll, (e) => setError(errorText(e)));
  }, [plan, mode, every]);
  // 남은 양보다 큰 선택지는 끄고, 남은 양을 넘는 첫 선택지는 "전부 N장" (카드 수를 넘겨 세지 않는다)
  const fits = SIZES.findIndex((s) => s >= all.length);
  const sizes = SIZES.map((s, i) => (fits >= 0 && i > fits ? { label: `${s}장`, value: s, disabled: true } : { label: i === fits ? `전부 ${n(all.length)}장` : `${s}장`, value: s }));
  const pickedSize = fits >= 0 && SIZES.indexOf(size) > fits ? SIZES[fits] : size;
  const mix = all.slice(0, pickedSize);

  const isMix = mode === "mix";
  const count = isMix ? mix.length : (plan?.scopes[scope] ?? 0);
  const roundDone = !!plan && plan.learned > 0 && plan.round_seen >= plan.learned;
  const back = useCallback(
    () => nav(deckIds?.length === 1 ? { name: "deck", deckId: deckIds[0] } : { name: "home" }),
    [nav, deckIds],
  );
  const start = useCallback(async () => {
    if (!plan || count === 0) return;
    try {
      if (!isMix) {
        const ids = await cramApi.cards(plan.deck_id, scope);
        return nav({ name: "study", deckIds, scale: 1, cardIds: ids, cram: true });
      }
      const [s, today] = await Promise.all([cramApi.mixStart(plan.deck_id, every, mix.length), api.queuePreview(null, 1)]);
      const run: MixRun = {
        deckId: plan.deck_id,
        name: plan.name,
        cards: s.cards,
        dueToday: s.due_today,
        before: { seen: plan.round_seen, learned: plan.learned, today: today.total },
      };
      nav({ name: "study", deckIds, scale: 1, cardIds: s.cards.map((c) => c.id), sessionId: s.session_id, mix: run });
    } catch (e) {
      setError(errorText(e));
    }
  }, [plan, count, isMix, scope, every, mix.length, nav, deckIds]);
  const restart = async () => {
    if (!plan) return;
    await cramApi.mixRestart(plan.deck_id).catch((e) => setError(errorText(e)));
    await load();
  };
  useKey("Enter", start, count > 0);
  useKey("Escape", back);
  if (plan === undefined) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  const minutes = Math.max(1, Math.ceil((count * (plan?.sec_per_card ?? 10)) / 60));
  const weakNames = plan?.weak_names ?? [];
  const rows = plan && [
    { key: "core", dot: "core", title: "아직 불안한 핵심 카드", desc: `핵심 ${n(plan.core_total)}장 중 기억 안정도 7일 미만`, count: plan.core },
    {
      key: "weak",
      dot: "danger",
      title: "약한 챕터",
      desc: weakNames.length === 0 ? "약한 챕터로 정한 곳이 없어요" : weakNames.length === 1 ? weakNames[0] : `${weakNames[0]} 외 ${n(weakNames.length - 1)}개`,
      count: plan.weak,
    },
    { key: "again", dot: "accent", title: "최근 2주에 틀린 카드", desc: "Again 1번 이상", count: plan.again },
  ];
  const weakCount = mix.filter((c) => c.weak).length;
  const pct = plan && plan.learned > 0 ? Math.round((plan.round_seen * 100) / plan.learned) : 0;

  return (
    <main className="summary cram" onMouseDown={(e) => (e.target as HTMLElement).closest(".segment__item") && e.preventDefault()}>
      <section className="summary__left">
        <button className="backlink t-label-m c-secondary" onClick={back}>
          <Icon name="chevron-left" size={18} />
          {deckIds?.length === 1 ? "덱" : "홈"}
        </button>
        <div className="cram__gap" />
        {plan && <Badge tone={plan.dday <= 3 ? "danger" : "warn"}>{plan.name} {plan.dday === 0 ? "시험이 오늘이에요" : `시험까지 ${n(plan.dday)}일`}</Badge>}
        <h1 className="cram__title">시험 전 회독</h1>
        {plan ? (
          <>
            <div className="cram__mode">
              <Segment boxed label="방식" options={[{ label: "압축", value: "cram" as const }, { label: "혼합", value: "mix" as const }]} value={mode} onChange={setMode} />
            </div>
            <p className="cram__lead t-body-s c-secondary">
              {isMix
                ? "혼합: 범위 전체를 한 번 돌며 불안한 카드를 사이사이에 끼워요. 평가가 복습 일정에 반영돼요 (압축은 기록만)."
                : "압축: 범위 전체를 한 번 훑되, 이미 잘 외운 카드는 빼고 불안한 카드만 모았어요."}
            </p>
            <p className="summary__count">
              <span className="cram__num">{n(count)}</span>
              <span className="t-title-m c-secondary">장 · 약 {n(minutes)}분</span>
            </p>
            {isMix && (
              <div className="cram__round">
                <span className="cram__round-head t-caption">
                  <span className="c-secondary">이번 회독</span>
                  <span className="t-label-s">{n(plan.round_seen)} / {n(plan.learned)}장 · {pct}%</span>
                </span>
                <span className="cram__bar"><span style={{ width: `${pct}%` }} /></span>
              </div>
            )}
            <p className="cram__note t-caption c-secondary">
              <Icon name="info" size={14} />
              {isMix
                ? "혼합 회독의 평가는 복습 일정에 반영돼요. 오늘 예정인 카드를 여기서 보면 오늘의 학습에서 빠져요."
                : "압축 회독은 복습 일정에 반영하지 않아요. 평가는 기록만 하고, 오늘의 학습은 그대로 남아요."}
            </p>
            <div className="cram__cta">
              {isMix && roundDone ? (
                <Button icon="rotate" label="처음부터 다시" onClick={restart} />
              ) : (
                <Button icon="arrow-right" label={isMix ? "혼합 회독 시작" : "압축 회독 시작"} onClick={start} disabled={count === 0} />
              )}
              <Button variant="ghost" label="평소 학습" onClick={() => nav({ name: "summary", deckIds })} />
            </div>
            {isMix && !roundDone && count === 0 && <p className="t-body-s c-tertiary">오늘 볼 수 있는 카드는 다 봤어요. 내일 이어서 돌아요.</p>}
          </>
        ) : (
          <p className="cram__lead t-body-m c-secondary">목표일 14일 전부터 할 수 있어요. 덱 설정에서 목표일을 정하면 열려요.</p>
        )}
        {error && <p className="t-body-s c-danger">{error}</p>}
      </section>

      {plan && isMix && (
        <section className="summary__order cram__parts">
          <header className="summary__order-head">
            <h2 className="t-title-s">이번 {n(count)}장은</h2>
            <p className="t-caption c-tertiary">
              취약 카드를 전체 사이에 고르게 끼워요 · 전체 {every === 1 ? "1장" : `${every}장`}마다 취약 1장
            </p>
          </header>
          <div className="summary__row">
            <span className="dot dot--accent" />
            <span className="summary__row-text">
              <span className="t-body-m-strong">전체 범위</span>
              <span className="t-caption c-tertiary">이번 회독에서 아직 안 본 배운 카드 · 신규는 오늘의 학습으로</span>
            </span>
            <span className="t-title-m">{n(count - weakCount)}</span>
          </div>
          <div className="summary__row">
            <span className="dot dot--danger" />
            <span className="summary__row-text">
              <span className="t-body-m-strong">취약 끼워 넣기</span>
              <span className="t-caption c-tertiary">약한 챕터 · 최근 2주 Again · 아직 불안한 핵심</span>
            </span>
            <span className="t-title-m">{n(weakCount)}</span>
          </div>
          <div className="cram__order">
            <span className="t-caption c-tertiary">도는 순서</span>
            <span className="cram__strip" aria-hidden>
              {mix.slice(0, 18).map((c, i) => (
                <span key={i} className={c.weak ? "is-weak" : ""} />
              ))}
              {mix.length > 18 && <span className="cram__more t-caption c-tertiary">…</span>}
            </span>
          </div>
          <div className="cram__scope">
            <span className="summary__row-text">
              <span className="t-body-m-strong">취약 비율</span>
              <span className="t-caption c-tertiary">1/5 · 1/3 · 1/2</span>
            </span>
            <Segment boxed label="취약 비율" options={RATIOS.map((r) => ({ label: r.label, value: r.every }))} value={every} onChange={setEvery} />
          </div>
          <div className="cram__scope">
            <span className="summary__row-text">
              <span className="t-body-m-strong">한 번에</span>
              <span className="t-caption c-tertiary">카드당 평균 {n(Math.round(plan.sec_per_card))}초 기준</span>
            </span>
            <Segment boxed label="한 번에" options={sizes} value={pickedSize} onChange={setSize} />
          </div>
        </section>
      )}

      {plan && !isMix && rows && (
        <section className="summary__order cram__parts">
          <header className="summary__order-head">
            <h2 className="t-title-s">무엇을 모았나요</h2>
            <p className="t-caption c-tertiary">겹치는 카드는 한 번만 세요</p>
          </header>
          {rows.map((r) => (
            <div key={r.key} className="summary__row">
              <span className={`dot dot--${r.dot}`} />
              <span className="summary__row-text">
                <span className="t-body-m-strong">{r.title}</span>
                <span className="t-caption c-tertiary">{r.desc}</span>
              </span>
              <span className="t-title-m">{n(r.count)}</span>
            </div>
          ))}
          <div className="summary__row cram__out">
            <span className="dot dot--strong" />
            <span className="summary__row-text c-tertiary">
              <span className="t-body-m-strong">잘 외운 카드는 뺐어요</span>
              <span className="t-caption">안정도 30일 이상 · {n(plan.stable)}장</span>
            </span>
            <span className="t-title-m c-tertiary">−</span>
          </div>
          <div className="cram__scope">
            <span className="t-body-m-strong">범위</span>
            <Segment boxed label="범위" options={CRAM_SCOPES.map((label, i) => ({ label, value: i }))} value={scope} onChange={setScope} />
          </div>
        </section>
      )}
    </main>
  );
}

/** P3 혼합 회독 끝난 뒤. 숫자만 (P7). 세션 닫기와 접어둔 카드는 학습 화면이 finish_session 으로 했다 */
export function MixDone({ sessionId, run, stats }: { sessionId: number; run: MixRun; stats: MixStats }) {
  const nav = useNav();
  const notify = useToast();
  const [sum, setSum] = useState<MixSummary>();
  const [today, setToday] = useState<number>();
  const [weakSet, setWeakSet] = useState(false);
  useEffect(() => {
    cramApi.mixSummary(sessionId, run.deckId).then(setSum, () => {});
    api.queuePreview(null, 1).then((q) => setToday(q.total), () => {});
  }, [sessionId, run.deckId]);
  const home = () => nav({ name: "home" });
  const again = () => nav({ name: "summary", deckIds: [run.deckId], cram: true });
  useKey("Escape", home);
  const markWeak = async () => {
    if (!sum?.hot) return;
    await api.setDeckWeak(sum.hot.deck_id, true);
    setWeakSet(true);
    notify({ title: `${sum.hot.name}${eul(sum.hot.name)} 약한 챕터로 정했어요`, text: "복습에서 이 챕터 카드를 먼저 보여드려요." });
  };
  const rated = stats.full[0] + stats.weak[0];

  return (
    <main className="summary cram">
      <section className="summary__left">
        <div className="cram__gap" />
        <Badge tone="accent">{run.name} · 혼합 회독</Badge>
        <h1 className="cram__title">{n(rated)}장을 돌았어요</h1>
        {sum && (
          <div className="cram__round">
            <span className="cram__round-head t-caption">
              <span className="c-secondary">이번 회독</span>
              <span className="t-label-s">
                {n(run.before.seen)} → {n(sum.round_seen)} / {n(sum.learned)}장
              </span>
            </span>
            <span className="cram__bar">
              <span style={{ width: `${sum.learned ? Math.round((sum.round_seen * 100) / sum.learned) : 0}%` }} />
            </span>
          </div>
        )}
        <div className="done__tiles">
          <StatTile label="전체" value={`${n(stats.full[0])} / ${n(stats.full[1])}`} sub={`Again ${n(stats.again[0])}`} />
          <StatTile label="취약" value={`${n(stats.weak[0])} / ${n(stats.weak[1])}`} sub={`Again ${n(stats.again[1])}`} />
        </div>
        {sum && (
          <p className="cram__note t-caption c-secondary">
            <Icon name="info" size={14} />
            평가 {n(sum.fed)}개가 복습 일정에 반영됐어요
            {stats.due > 0 && today !== undefined && ` · 오늘 예정이던 ${n(stats.due)}장을 여기서 봐서 오늘의 학습이 ${n(run.before.today)} → ${n(today)}장으로 줄었어요`}
          </p>
        )}
        {sum?.hot && (
          <div className="cram-hint">
            <Icon name="alert" size={16} className="c-danger" />
            <span className="cram-hint__text">
              <span className="t-label-m">약해 보이는 챕터: {sum.hot.name}</span>
              <span className="t-caption c-tertiary">이번 회독 Again {n(sum.hot.again)}번이 여기에 몰렸어요</span>
            </span>
            <Button variant="secondary" size="M" label={weakSet ? "정했어요" : "약한 챕터로"} disabled={weakSet} onClick={markWeak} />
          </div>
        )}
        <div className="cram__cta">
          <Button icon="arrow-right" label="이어서 회독" onClick={again} />
          <Button variant="ghost" icon="chevron-left" label="홈으로" onClick={home} />
        </div>
      </section>
    </main>
  );
}
