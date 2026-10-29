// P1 시험 전 회독 (방식 [압축 | 혼합]) · P3 혼합 회독 끝난 뒤. 압축은 H3 그대로, 혼합은 8.5 (cram.rs)
import { useCallback, useEffect, useState } from "react";
import { api, errorText } from "../api";
import { Badge, Button, Icon, Segment } from "../ui";
import { eul, n, useKey, useNav, type Route } from "../nav";
import { useToast } from "../toast";
import { cram as cramApi, type CramPlan, type MixCard, type MixRun, type MixStats, type MixSummary } from "../written";
import { Modal } from "./NewDeck";
import "../styles/study.css";

const CRAM_SCOPES = ["핵심만", "핵심 + 취약 챕터", "불안한 카드 전부"];
/** P2 취약 비율: 일반 몇 장마다 취약 1장. 취약 카드가 많으면 남는 것은 끝에 이어 나와 실제 비율은 달라서 % 로 적지 않는다 (QA T3-15) */
const RATIOS = [
  { label: "4장마다", every: 4 },
  { label: "2장마다", every: 2 },
  { label: "1장마다", every: 1 },
];
const SIZES = [60, 120, 200];
const MAX_SIZE = 500; // mix_cards 상한

/**
 * 혼합 회독 한 묶음을 열고 학습 화면으로 (P1 [혼합 회독 시작] · P3 [이어서 N장]).
 * 시작 값(회독 진행 · 오늘의 학습 장수)은 여기서 새로 읽는다. P3 의 "a% → b%"와 "72 → 50장"이 이번 묶음만 가리키게
 */
async function beginMix(nav: (r: Route) => void, deckIds: number[] | null, deckId: number, name: string, every: number, size: number) {
  const startedAt = Date.now();
  const [plan, today] = await Promise.all([cramApi.plan([deckId]), api.queuePreview(null, 1)]);
  const s = await cramApi.mixStart(deckId, every, size);
  const run: MixRun = {
    deckId,
    name,
    cards: s.cards,
    dueToday: s.due_today,
    before: { seen: plan?.round_seen ?? 0, learned: plan?.learned ?? 0, today: today.total },
    every,
    size,
    startedAt,
  };
  nav({ name: "study", deckIds, scale: 1, cardIds: s.cards.map((c) => c.id), sessionId: s.session_id, mix: run });
}

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
      await beginMix(nav, deckIds, plan.deck_id, plan.name, every, mix.length);
    } catch (e) {
      setError(errorText(e));
    }
  }, [plan, count, isMix, scope, every, mix.length, nav, deckIds]);
  const [asking, setAsking] = useState(false);
  const restart = async () => {
    if (!plan) return;
    await cramApi.mixRestart(plan.deck_id).catch((e) => setError(errorText(e)));
    await load();
    setAsking(false);
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
      title: "취약 챕터",
      desc: weakNames.length === 0 ? "취약 챕터로 지정한 곳이 없어요" : weakNames.length === 1 ? weakNames[0] : `${weakNames[0]} 외 ${n(weakNames.length - 1)}개`,
      count: plan.weak,
    },
    { key: "again", dot: "accent", title: "최근 2주에 틀린 카드", desc: "Again 1번 이상", count: plan.again },
  ];
  const weakCount = mix.filter((c) => c.weak).length;
  // 일반 카드 사이에 다 못 끼운 취약 카드 (끝에 줄지어 나온다)
  const lastFull = mix.map((c) => !c.weak).lastIndexOf(true);
  const weakTail = lastFull < 0 ? 0 : mix.length - 1 - lastFull;
  const pct = plan && plan.learned > 0 ? Math.round((plan.round_seen * 100) / plan.learned) : 0;

  return (
    <main className="summary cram" onMouseDown={(e) => (e.target as HTMLElement).closest(".segment__item") && e.preventDefault()}>
      <section className="summary__left">
        <button className="backlink t-label-m c-secondary" onClick={back}>
          <Icon name="chevron-left" size={18} />
          {deckIds?.length === 1 ? "덱" : "홈"}
        </button>
        <div className="cram__gap" />
        {/* 덱 이름이 길면 이름만 말줄임. "시험까지 n일"은 늘 보인다 (QA T6-06) */}
        {plan && (
          <Badge tone={plan.dday <= 3 ? "danger" : "warn"}>
            <span className="cram__exam-name" title={plan.name}>{plan.name}</span>
            <span className="cram__exam-day">{plan.dday === 0 ? "시험이 오늘이에요" : `시험까지 ${n(plan.dday)}일`}</span>
          </Badge>
        )}
        <h1 className="cram__title">시험 전 회독</h1>
        {plan ? (
          <>
            <div className="cram__mode">
              <Segment
                boxed
                label="방식"
                options={[
                  { label: "압축 · 일정 유지", value: "cram" as const },
                  { label: "혼합 · 일정 반영", value: "mix" as const },
                ]}
                value={mode}
                onChange={setMode}
              />
            </div>
            <p className="cram__lead t-body-s c-secondary">
              {isMix
                ? "혼합: 이미 학습한 카드 전체를 한 번 돌며 취약 카드를 사이사이에 끼워요. 평가가 복습 일정에 반영돼요."
                : "압축: 고른 범위에서 불안한 카드만 모아 훑어요. 잘 외운 카드는 빼고, 복습 일정은 그대로예요."}
            </p>
            <p className="summary__count">
              <span className="cram__num">{n(count)}</span>
              <span className="t-title-m c-secondary">장{count > 0 && ` · 약 ${n(minutes)}분`}</span>
            </p>
            {isMix && (
              <div className="cram__round">
                <span className="cram__round-head t-caption">
                  <span className="c-secondary">이번 회독에서 확인한 배운 카드</span>
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
                <Button icon="rotate" label="처음부터 다시" onClick={() => setAsking(true)} />
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
              취약 카드를 배운 카드 사이에 고르게 끼워요 · 같은 카드는 한 번만
            </p>
          </header>
          <div className="summary__row">
            <span className="dot dot--accent" />
            <span className="summary__row-text">
              <span className="t-body-m-strong">이미 학습한 카드 전체</span>
              <span className="t-caption c-tertiary">이번 회독에서 아직 안 본 배운 카드 · 신규 카드는 오늘의 학습에서 배워요</span>
            </span>
            <span className="t-title-m">{n(count - weakCount)}</span>
          </div>
          <div className="summary__row">
            <span className="dot dot--danger" />
            <span className="summary__row-text">
              <span className="t-body-m-strong">끼워 넣는 취약 카드</span>
              <span className="t-caption c-tertiary">취약 챕터 · 최근 2주 Again · 아직 불안한 핵심</span>
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
              <span className="t-body-m-strong">취약 카드 비율</span>
              <span className="t-caption c-tertiary">
                일반 {every === 1 ? "1장" : `${every}장`}마다 취약 1장{weakTail > 0 && ` · 남는 취약 ${n(weakTail)}장은 끝에 이어서 나와요`}
              </span>
            </span>
            <Segment boxed label="취약 카드 비율" options={RATIOS.map((r) => ({ label: r.label, value: r.every }))} value={every} onChange={setEvery} />
          </div>
          <div className="cram__scope">
            <span className="summary__row-text">
              <span className="t-body-m-strong">한 번에</span>
              <span className="t-caption c-tertiary">
                카드당 보통 약 {n(Math.round(plan.sec_per_card))}초 기준{plan.today_seen > 0 && ` · 오늘 이미 본 ${n(plan.today_seen)}장은 빼요`}
              </span>
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
      {asking && <RestartAsk onClose={() => setAsking(false)} onRestart={restart} />}
    </main>
  );
}

/** [처음부터 다시] 확인 (P1 · P3). 회독 진행을 0% 로 되돌리기 전에 묻는다 (Enter · 클릭 한 번에 지워지지 않게, QA T3-12 · V3-06) */
function RestartAsk({ onClose, onRestart }: { onClose: () => void; onRestart: () => void }) {
  return (
    <Modal title="회독을 처음부터 다시 셀까요?" onClose={onClose} variant="confirm" icon="rotate">
      <p className="t-body-m c-secondary">회독 진행이 0%로 돌아가요. 평가 기록과 복습 일정은 그대로예요.</p>
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
        <Button size="M" icon="rotate" label="처음부터 다시" onClick={onRestart} />
      </div>
    </Modal>
  );
}

/**
 * P3 혼합 회독 끝난 뒤 (끝까지 돌았든 중간에 그만뒀든 같은 틀). 숫자만 (P7): 몇 장 · 몇 분, 회독 진행 a% → b%, 전체 · 취약 Again,
 * 약한 챕터 제안. 세션 닫기와 접어둔 카드는 학습 화면이 finish_session 으로 했다
 */
export function MixDone({ sessionId, run, stats, minutes, completed }: { sessionId: number; run: MixRun; stats: MixStats; minutes: number; completed: boolean }) {
  const nav = useNav();
  const notify = useToast();
  const [sum, setSum] = useState<MixSummary>();
  const [today, setToday] = useState<number>();
  const [next, setNext] = useState<number>();
  const [weakSet, setWeakSet] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    cramApi.mixSummary(sessionId, run.deckId, run.startedAt).then(setSum, () => {});
    api.queuePreview(null, 1).then((q) => setToday(q.total), () => {});
    cramApi.mixCards(run.deckId, run.every, run.size).then((c) => setNext(c.length), () => setNext(0));
  }, [sessionId, run.deckId, run.every, run.size, run.startedAt]);
  const home = () => nav({ name: "home" });
  useKey("Escape", home);
  const markWeak = async () => {
    if (!sum?.hot) return;
    await api.setDeckWeak(sum.hot.deck_id, true);
    setWeakSet(true);
    notify({ title: `${sum.hot.name}${eul(sum.hot.name)} 취약 챕터로 지정했어요`, text: "복습에서 이 챕터 카드를 먼저 보여드려요." });
  };
  const roundDone = !!sum && sum.learned > 0 && sum.round_seen >= sum.learned;
  const goOn = () => beginMix(nav, [run.deckId], run.deckId, run.name, run.every, run.size).catch((e) => setError(errorText(e)));
  const restart = async () => {
    await cramApi.mixRestart(run.deckId).catch((e) => setError(errorText(e)));
    nav({ name: "summary", deckIds: [run.deckId], cram: true });
  };
  // 회독 진행을 0% 로 되돌리기 전에 묻는다 (Enter 한 번에 지워지지 않게, QA T3-12)
  const [asking, setAsking] = useState(false);
  useKey("Enter", () => (roundDone ? setAsking(true) : next ? goOn() : undefined), !!sum && (roundDone || !!next));
  const rated = stats.full[0] + stats.weak[0];
  const pct = (seen: number, learned: number) => (learned ? Math.round((seen * 100) / learned) : 0);
  const againLine = (a: number, seen: number) => (seen === 0 ? "이번엔 없었어요" : a ? `Again ${n(a)}장 · 나머지는 기억났어요` : "Again 없이 모두 기억났어요");

  return (
    <main className="summary cram">
      <section className="summary__left">
        <div className="cram__gap" />
        <Badge tone="accent">{completed ? "혼합 회독 끝" : "혼합 회독 · 여기까지"}</Badge>
        <p className="summary__count">
          <span className="cram__num">{n(rated)}</span>
          <span className="t-title-m c-secondary">장 회독 · {n(minutes)}분</span>
        </p>
        {sum && (
          <div className="cram__round">
            <span className="cram__round-head t-caption">
              <span className="c-secondary">이번 회독에서 확인한 배운 카드</span>
              <span className="t-label-s">
                {pct(run.before.seen, run.before.learned)}% → {pct(sum.round_seen, sum.learned)}% · {n(sum.round_seen)} / {n(sum.learned)}장
              </span>
            </span>
            <span className="cram__bar">
              <span style={{ width: `${pct(sum.round_seen, sum.learned)}%` }} />
            </span>
          </div>
        )}
        {roundDone && <p className="cram__done t-title-s">이미 학습한 카드를 한 번 다 봤어요</p>}
        <div className="cram__cta">
          {roundDone ? (
            <Button icon="rotate" label="처음부터 다시" shortcut="Enter" onClick={() => setAsking(true)} />
          ) : next ? (
            <Button icon="arrow-right" label={`이어서 ${n(next)}장`} shortcut="Enter" onClick={goOn} />
          ) : null}
          <Button variant={roundDone || next ? "ghost" : "secondary"} label="홈으로" onClick={home} />
        </div>
        {sum && !roundDone && next === 0 && <p className="t-body-s c-tertiary">오늘 볼 수 있는 카드는 다 봤어요. 내일 이어서 돌아요.</p>}
        {sum && (
          <p className="cram__note t-caption c-secondary">
            <Icon name="info" size={14} />
            평가 {n(sum.fed)}회가 복습 일정에 반영됐어요
            {stats.due > 0 && today !== undefined && ` · 오늘 예정이던 ${n(stats.due)}장을 여기서 봐서 오늘의 학습이 ${n(run.before.today)} → ${n(today)}장으로 줄었어요`}
          </p>
        )}
        {error && <p className="t-body-s c-danger">{error}</p>}
      </section>

      <section className="summary__order cram__parts">
        <header className="summary__order-head">
          <h2 className="t-title-s">이번에 본 것</h2>
        </header>
        <div className="summary__row">
          <span className="summary__row-text">
            <span className="t-body-m-strong">이미 학습한 카드</span>
            <span className="t-caption c-tertiary">{againLine(stats.again[0], stats.full[0])}</span>
          </span>
          <span className="t-title-m">{n(stats.full[0])}</span>
        </div>
        <div className="summary__row">
          <span className="summary__row-text">
            <span className="t-body-m-strong">취약 카드</span>
            <span className="t-caption c-tertiary">{againLine(stats.again[1], stats.weak[0])}</span>
          </span>
          <span className="t-title-m">{n(stats.weak[0])}</span>
        </div>
        {sum?.hot && (
          <div className="summary__row">
            <span className="summary__row-text">
              <span className="t-body-m-strong c-warn">취약 챕터 제안 1</span>
              <span className="t-caption c-tertiary">
                {sum.hot.name} — 이번 회독 Again {n(sum.hot.again)}장
              </span>
            </span>
            <Button variant="secondary" size="M" label={weakSet ? "지정했어요" : "취약 챕터로 지정"} disabled={weakSet} onClick={markWeak} />
          </div>
        )}
      </section>
      {asking && <RestartAsk onClose={() => setAsking(false)} onRestart={restart} />}
    </main>
  );
}
