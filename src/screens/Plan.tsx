// H1 학습 계획 (날짜별 분량 + 쓸 수 있는 시간) · H2 목표일 충돌.
// 보여주는 건 남은 양 · 날짜 · 시간뿐이다. 예상 성적 · 합격 가능성 · 준비도 점수는 만들지 않는다 (MVP 9장 Out).
// 목표일은 절대 바꾸지 않는다. 바뀌는 건 신규를 나누는 방식뿐이다 (plan.rs)
import "../styles/plan.css";
import { useCallback, useEffect, useState } from "react";
import { api, errorText, planApi, type PlanBar, type PlanConflict, type PlanMode, type StudyPlan } from "../api";
import { n, ro, useKey, useNav, type Route } from "../nav";
import { useToast } from "../toast";
import { Badge, Button, Icon, Toggle } from "../ui";
import { dateWithDay } from "./Deck";

const md = (iso: string) => {
  const [, m, d] = iso.split("-").map(Number);
  return { m, d };
};
/** "10월 26일" */
const monthDay = (iso: string) => `${md(iso).m}월 ${md(iso).d}일`;
const dayNum = (iso: string) => `${md(iso).d}일`;
/** 막대 아래 날짜: 첫 칸과 달이 바뀌는 칸은 "10/7", 나머지는 날만 */
const tick = (iso: string, first: boolean) => (first || md(iso).d === 1 ? `${md(iso).m}/${md(iso).d}` : String(md(iso).d));

// ---------------------------------------------------------------- H1 학습 계획

export function Plan({ deckId }: { deckId: number | null }) {
  const nav = useNav();
  const toast = useToast();
  const [scope, setScope] = useState<number | null>(deckId);
  const [deckName, setDeckName] = useState("");
  const [p, setP] = useState<StudyPlan | null>(null);
  const [time, setTime] = useState<{ weekday: number | null; weekend: number | null } | null>(null);
  const [error, setError] = useState<string>();

  const load = useCallback(
    () =>
      planApi.plan(scope).then((x) => {
        setP(x);
        setTime((t) => t ?? { weekday: x.weekday_min, weekend: x.weekend_min });
      }, (e) => setError(errorText(e))),
    [scope],
  );
  useEffect(() => {
    load();
  }, [load]);
  useEffect(() => {
    if (deckId !== null) api.deckDetail(deckId).then((d) => setDeckName(d.name), () => {});
  }, [deckId]);
  // 시간은 누를 때마다 저장하지 않고 손을 뗀 뒤 한 번 (−/+ 를 여러 번 누르는 동안 그래프가 깜빡이지 않게)
  useEffect(() => {
    if (!time || !p || (time.weekday === p.weekday_min && time.weekend === p.weekend_min)) return;
    const t = setTimeout(() => planApi.setTime(time.weekday, time.weekend).then(load, (e) => setError(errorText(e))), 350);
    return () => clearTimeout(t);
  }, [time, p, load]);

  const back = useCallback(() => nav(deckId === null ? { name: "home" } : { name: "deck", deckId }), [nav, deckId]);
  useKey("Escape", back);
  if (!p || !time) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  const fail = (e: unknown) => toast({ title: "바꾸지 못했어요", text: errorText(e), tone: "error" });
  const sub =
    scope !== null && p.target_date && p.days_left !== null
      ? `${dateWithDay(p.target_date)} 시험까지 ${n(p.days_left)}일 · 남은 신규 ${n(p.remaining_new)}장`
      : p.exams.length
        ? `다가오는 시험 ${n(p.exams.length)}개 · 남은 신규 ${n(p.remaining_new)}장`
        : `목표일이 없어서 앞으로 2주를 보여줘요 · 남은 신규 ${n(p.remaining_new)}장`;
  const o = p.over;
  const pull = () =>
    o?.to_date &&
    planApi.pull(o.deck_id, o.date, o.to_date, o.n).then(() => {
      load();
      toast({ title: `${o.to_date === p.bars.find((b) => b.kind === "today")?.date ? "오늘" : dayNum(o.to_date!)}로 신규 ${n(o.n)}장을 당겼어요`, text: `${dateWithDay(o.date)} 분량이 그만큼 줄어요.` });
    }, fail);
  const sec = p.sec_per_card;
  const capLine = (w: number | null, e: number | null) =>
    w === null && e === null
      ? "시간을 넣으면 하루에 할 수 있는 장수를 계산해 넘치는 날을 알려 드려요."
      : `${p.measured ? `최근 14일 동안 카드 1장에 보통 ${sec}초` : `기록이 아직 적어 카드 1장에 ${sec}초로 잡았어요`} → ${[
          w !== null && `평일 약 ${n(p.weekday_cap ?? 0)}장`,
          e !== null && `주말 약 ${n(p.weekend_cap ?? 0)}장`,
        ]
          .filter(Boolean)
          .join(", ")}까지 할 수 있어요.`;
  const todayIso = p.bars.find((b) => b.kind === "today")?.date;
  const toLabel = o?.to_date ? (o.to_date === todayIso ? "오늘" : dayNum(o.to_date)) : "";

  return (
    <main className="cards plan">
      <button className="backlink t-label-m c-secondary" onClick={back}>
        <Icon name="chevron-left" size={18} />
        {deckId === null ? "홈" : deckName}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">학습 계획</h1>
          <p className="t-body-s c-secondary">{sub}</p>
        </div>
        {deckId !== null && (
          <span className="segment segment--boxed" role="radiogroup" aria-label="범위">
            {[
              { v: deckId, label: "이 덱" },
              { v: null, label: "모든 덱" },
            ].map((s) => (
              <button
                key={s.label}
                role="radio"
                aria-checked={scope === s.v}
                className={`segment__item t-label-m ${scope === s.v ? "is-selected" : ""}`}
                onClick={() => setScope(s.v)}
              >
                {s.label}
              </button>
            ))}
          </span>
        )}
      </div>

      {p.has_conflict && <PlanConflictBanner includeResolved back={{ name: "plan", deckId }} />}

      <div className="plan__row">
        <section className="panel plan__chart-card">
          <div className="plan__legend t-caption c-secondary">
            <span><i className="plan__key plan__key--review" />복습</span>
            <span><i className="plan__key plan__key--new" />신규</span>
            <span><i className="plan__key plan__key--done" />지난 날 · 한 만큼</span>
          </div>
          <DayChart bars={p.bars} weekdayCap={p.weekday_cap} sec={sec} />
          {p.pace && (
            <p className="plan__pace t-body-s c-secondary">
              <Icon name="rotate" size={16} />
              {p.pace}
            </p>
          )}
        </section>

        <section className="panel plan__side">
          <h2 className="t-title-s">공부할 수 있는 시간</h2>
          <TimeStepper label="평일" value={time.weekday} onStep={(dir) => setTime((t) => t && { ...t, weekday: stepMin(t.weekday, dir) })} />
          <TimeStepper label="주말" value={time.weekend} onStep={(dir) => setTime((t) => t && { ...t, weekend: stepMin(t.weekend, dir) })} />
          <p className="t-caption c-tertiary">{capLine(p.weekday_min, p.weekend_min)}</p>
          {o && (
            <div className="plan__warn">
              <Icon name="alert" size={16} className="c-warn" />
              <span className="plan__warn-text">
                <span className="t-body-m-strong c-warn">{dateWithDay(o.date)} 하루가 시간을 넘겨요</span>
                <span className="t-body-s c-secondary">
                  그날 예정 {n(o.total)}장 · {o.weekend ? "주말" : "평일"} 가능 {n(o.cap)}장.{" "}
                  {o.to_date
                    ? `${o.to_weekend ? "주말인 " : ""}${toLabel}로 신규 ${n(o.n)}장을 당기면 ${o.after <= o.cap ? "맞아요" : `${n(o.total - o.after)}장 줄어요`}.`
                    : "앞날로 당겨 맞출 날이 없어요. 시간을 늘리거나 목표 및 학습 설정에서 하루 분량을 바꿔 보세요."}
                </span>
              </span>
            </div>
          )}
          {o?.to_date && <Button variant="secondary" size="M" label={`${toLabel}로 당겨 맞추기`} onClick={pull} />}
          {scope !== null && p.recompute !== null && (
            <div className="plan__recompute">
              <span className="plan__recompute-text">
                <span className="t-body-m-strong">실제 속도에 맞춰 다시 나누기</span>
                <span className="t-caption c-tertiary">매일 아침 남은 신규를 남은 날에 다시 나눠요. 끄면 지금의 하루 몫을 그대로 써요.</span>
              </span>
              <Toggle
                on={p.recompute}
                label="실제 속도에 맞춰 다시 나누기"
                onChange={(on) => planApi.setRecompute(scope, on).then(load, fail)}
              />
            </div>
          )}
        </section>
      </div>
    </main>
  );
}

/** 5분씩. 5분 아래로 내리면 "안 정함" (상한선과 안내가 사라진다) */
const stepMin = (v: number | null, dir: 1 | -1) => (dir < 0 ? (v === null || v <= 5 ? null : v - 5) : Math.min(600, (v ?? 0) + 5));

function TimeStepper({ label, value, onStep }: { label: string; value: number | null; onStep: (dir: 1 | -1) => void }) {
  return (
    <div className="plan__time">
      <span className="t-body-m">{label}</span>
      <span className="stepper" role="group" aria-label={`${label} 공부할 수 있는 시간`}>
        <button className="stepper__btn t-title-s" aria-label="5분 줄이기" disabled={value === null} onClick={() => onStep(-1)}>
          −
        </button>
        <span className="stepper__value t-label-m">{value === null ? "안 정함" : `${n(value)}분`}</span>
        <button className="stepper__btn" aria-label="5분 늘리기" onClick={() => onStep(1)}>
          <Icon name="plus" size={14} scaled />
        </button>
      </span>
    </div>
  );
}

/** 막대 하나 = 하루. 앞날은 복습 + 신규 쌓기, 지난 날은 한 만큼(회색), 오늘은 테두리, 시험일은 빨간 점선 칸 */
function DayChart({ bars, weekdayCap, sec }: { bars: PlanBar[]; weekdayCap: number | null; sec: number }) {
  const total = (b: PlanBar) => (b.kind === "past" ? b.done : b.review + b.new);
  const max = Math.max(1, ...bars.map(total), weekdayCap ?? 0) * 1.1; // 위에 합계 숫자 자리
  const H = 190;
  const h = (v: number) => Math.max(0, Math.round((v / max) * H));
  // 칸이 많으면 날짜를 걸러 적는다 (오늘 · 시험은 늘)
  const every = bars.length > 32 ? 3 : bars.length > 22 ? 2 : 1;
  const tip = (b: PlanBar) => {
    const head = dateWithDay(b.date);
    if (b.kind === "past") return `${head} · 한 만큼 ${n(b.done)}장`;
    const cap = b.weekend && b.cap !== null ? ` · 주말 가능 ${n(b.cap)}장` : "";
    return `${head} · 복습 ${n(b.review)} · 신규 ${n(b.new)} · 약 ${n(Math.ceil((total(b) * sec) / 60))}분${cap}`;
  };
  return (
    <div className="plan__chart" style={{ ["--n" as string]: bars.length }}>
      {weekdayCap !== null && (
        <div className="plan__cap" style={{ bottom: 24 + h(weekdayCap) }}>
          <span className="t-caption c-warn">평일 가능 {n(weekdayCap)}장</span>
        </div>
      )}
      {bars.map((b, i) => {
        const t = total(b);
        // 마지막 시험일 칸은 비워 둔다 (Figma). 그날 분량은 툴팁에. 모든 덱에서 중간의 시험일은 다른 덱 분량이 있어 그린다
        const empty = b.kind === "exam" && i === bars.length - 1;
        const over = b.kind !== "past" && !empty && b.cap !== null && t > b.cap;
        const label = b.kind === "today" ? "오늘" : b.kind === "exam" ? "시험" : i === 0 || i % every === 0 ? tick(b.date, i === 0) : "";
        return (
          <div key={b.date} className={`plan__col plan__col--${b.kind}`} title={tip(b)} role="img" aria-label={tip(b)}>
            <div className="plan__area">
              {over && <span className="plan__total t-label-s c-warn">{n(t)}</span>}
              <span className="plan__stack">
                {empty ? null : b.kind === "past" ? (
                  <span className={`plan__seg ${b.done ? "plan__seg--done" : "plan__seg--zero"}`} style={b.done ? { height: h(b.done) } : undefined} />
                ) : (
                  <>
                    {b.new > 0 && <span className="plan__seg plan__seg--new" style={{ height: h(b.new) }} />}
                    {b.review > 0 && <span className="plan__seg plan__seg--review" style={{ height: h(b.review) }} />}
                  </>
                )}
              </span>
            </div>
            <span className="plan__date t-caption">{label}</span>
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------- H2 목표일 충돌 (홈 배너 + 조정 화면)

const MODE_LABEL: Record<PlanMode, string> = { spread: "앞당겨 나누기", first: "먼저 볼 덱", keep: "그대로 두기" };
/** 권장: 최고 분량이 더 낮아지는 쪽 (노트는 앞당겨 나누기를 권장하지만, 덱에 따라 반대일 수 있다) */
const recommend = (c: PlanConflict): PlanMode => (c.spread_peak <= c.first_peak ? "spread" : "first");

/**
 * 홈 덱 목록 위 한 줄 배너. 고르지 않은 충돌이 있을 때만 뜬다 (H2 노트).
 * includeResolved: 이미 고른 충돌도 (학습 계획 화면에서 다시 보기)
 */
export function PlanConflictBanner({ includeResolved = false, back }: { includeResolved?: boolean; back?: Route }) {
  const nav = useNav();
  const [c, setC] = useState<PlanConflict | null>(null);
  useEffect(() => {
    planApi.conflict(includeResolved).then(setC, () => {});
  }, [includeResolved]);
  if (!c) return null;
  return (
    <div className={`plan__banner ${c.chosen ? "is-resolved" : ""}`}>
      {c.chosen ? <Icon name="check-circle" size={18} scaled className="c-secondary" /> : <Icon name="alert" size={18} className="c-warn" />}
      <p className="t-body-m">
        두 시험이 가까워요 · {c.chosen ? `${MODE_LABEL[c.chosen]}${ro(MODE_LABEL[c.chosen])} 정했어요` : `${monthDay(c.peak_date)} 하루 ${n(c.peak)}장`}
      </p>
      <Button variant="secondary" size="M" label={c.chosen ? "다시 보기" : "조정하기"} onClick={() => nav({ name: "planConflict", back })} />
    </div>
  );
}

export function PlanConflictScreen({ back }: { back?: Route }) {
  const nav = useNav();
  const toast = useToast();
  const [c, setC] = useState<PlanConflict | null | undefined>(undefined);
  const [mode, setMode] = useState<PlanMode>("spread");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    planApi.conflict(true).then((x) => {
      setC(x);
      if (x) setMode(x.chosen ?? recommend(x));
    }, (e) => setError(errorText(e)));
  }, []);
  const leave = useCallback(() => nav(back ?? { name: "home" }), [nav, back]);
  useKey("Escape", leave);
  if (c === undefined) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;
  if (c === null)
    return (
      <main className="cards plan">
        <button className="backlink t-label-m c-secondary" onClick={leave}>
          <Icon name="chevron-left" size={18} />
          돌아가기
        </button>
        <div className="deck__title">
          <h1 className="t-title-l">겹치는 시험이 없어요</h1>
          <p className="t-body-s c-secondary">목표일이 7일 안에 붙어 있고 하루 분량이 넘치는 두 덱이 생기면 여기서 나눠요.</p>
        </div>
      </main>
    );

  const save = () => {
    setBusy(true);
    planApi.resolve(c.a.id, c.b.id, mode).then(
      () => {
        toast({ title: mode === "keep" ? "그대로 두었어요" : `${MODE_LABEL[mode]}${ro(MODE_LABEL[mode])} 나눴어요`, text: "목표일은 바꾸지 않았어요." });
        leave();
      },
      (e) => {
        setBusy(false);
        setError(errorText(e));
      },
    );
  };
  const recommended = recommend(c);
  const backLabel =
    !back || back.name === "home"
      ? "홈"
      : back.name === "plan"
        ? back.deckId === null
          ? "학습 계획 · 모든 덱"
          : "학습 계획"
        : back.name === "deckSettings"
          ? "목표 및 학습 설정"
          : "돌아가기";
  const limitLabel = c.by_time ? `평일 가능 ${n(c.limit)}장` : `평소의 1.5배 · ${n(c.limit)}장`;
  const [o1, o2] = [c.over_dates[0], c.over_dates[c.over_dates.length - 1]];
  const overRange = o1 === o2 ? dayNum(o1) : md(o1).m === md(o2).m ? `${md(o1).d}–${dayNum(o2)}` : `${monthDay(o1)}–${monthDay(o2)}`;
  const options: { value: PlanMode; title: string; text: string; recommended?: boolean }[] = [
    {
      value: "spread",
      title: "앞당겨 나누기",
      recommended: recommended === "spread",
      text: `${c.b.name} 신규를 지금부터 하루 ${n(Math.max(0, c.spread_extra))}장씩 더해 최고 분량 ${n(c.peak)} → ${n(c.spread_peak)}장.`,
    },
    {
      value: "first",
      title: `${c.a.name} 먼저`,
      recommended: recommended === "first",
      text: `${dayNum(c.a.target)}까지는 ${c.a.name}만 신규를 넣고, 그 뒤 ${n(c.first_days)}일에 ${c.b.name} 신규를 몰아요. 최고 분량 ${n(c.first_peak)}장.`,
    },
    {
      value: "keep",
      title: "그대로 두기",
      text: `각 덱의 목표일 산식만 써요. ${overRange}은 ${c.by_time ? "쓸 수 있는 시간" : "평소의 1.5배"}를 넘겨요.`,
    },
  ];

  return (
    <main className="cards plan">
      <button className="backlink t-label-m c-secondary" onClick={leave}>
        <Icon name="chevron-left" size={18} />
        {backLabel}
      </button>
      <div className="deck__title">
        <h1 className="t-title-l">두 시험이 가까워요</h1>
        <p className="t-body-s c-secondary">
          {c.a.name} {monthDay(c.a.target)} · {c.b.name} {monthDay(c.b.target)}. 지금대로면 {monthDay(c.peak_date)} 하루 {n(c.peak)}장까지 몰려요.
        </p>
      </div>
      <section className="panel plan__chart-card">
        <div className="plan__legend t-caption c-secondary">
          <span><i className="plan__key plan__key--a" />{c.a.name}</span>
          <span><i className="plan__key plan__key--b" />{c.b.name}</span>
          {c.bars.some((b) => b.other > 0) && <span><i className="plan__key plan__key--done" />그 외</span>}
          <span className="c-tertiary">· 지금 계획대로면</span>
        </div>
        <ConflictChart c={c} limitLabel={limitLabel} />
      </section>
      <div className="plan__options" role="radiogroup" aria-label="나누는 방법">
        {options.map((o) => (
          <button key={o.value} role="radio" aria-checked={mode === o.value} className={`plan__option ${mode === o.value ? "is-selected" : ""}`} onClick={() => setMode(o.value)}>
            <span className="plan__option-head">
              <span className="t-body-m-strong">{o.title}</span>
              {o.recommended && <Badge tone="accent">권장</Badge>}
            </span>
            <span className="t-body-s c-secondary">{o.text}</span>
          </button>
        ))}
      </div>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="plan__actions">
        <Button variant="ghost" label="나중에" onClick={leave} />
        <Button icon="check" label={mode === "keep" ? "그대로 두기" : "이렇게 나누기"} busy={busy} onClick={save} />
      </div>
    </main>
  );
}

/** 날짜별 쌓기 막대. 먼저 오는 시험 덱 chart/1 (아래), 늦은 덱 chart/2 (위), 그 외 회색 */
function ConflictChart({ c, limitLabel }: { c: PlanConflict; limitLabel: string }) {
  const max = Math.max(1, ...c.bars.map((b) => b.a + b.b + b.other), c.limit) * 1.12;
  const H = 150;
  const h = (v: number) => Math.round((v / max) * H);
  return (
    <div className="plan__chart plan__chart--conflict">
      {c.limit > 0 && (
        <div className="plan__cap" style={{ bottom: 40 + h(c.limit) }}>
          <span className="t-caption c-warn">{limitLabel}</span>
        </div>
      )}
      {c.bars.map((b) => {
        const t = b.a + b.b + b.other;
        const tip = `${dateWithDay(b.date)} · ${c.a.name} ${n(b.a)} · ${c.b.name} ${n(b.b)}${b.other ? ` · 그 외 ${n(b.other)}` : ""}`;
        return (
          <div key={b.date} className="plan__col" title={tip} role="img" aria-label={tip}>
            <div className="plan__area">
              {b.limit > 0 && t > b.limit && <span className="plan__total t-label-s c-warn">{n(t)}</span>}
              <span className="plan__stack">
                {b.b > 0 && <span className="plan__seg plan__seg--b" style={{ height: h(b.b) }} />}
                {b.a > 0 && <span className="plan__seg plan__seg--a" style={{ height: h(b.a) }} />}
                {b.other > 0 && <span className="plan__seg plan__seg--done" style={{ height: h(b.other) }} />}
              </span>
            </div>
            <span className="plan__date t-caption">{`${md(b.date).m}/${md(b.date).d}`}</span>
            <span className="plan__exam t-caption c-danger">{b.date === c.a.target ? `${c.a.name} 시험` : ""}</span>
          </div>
        );
      })}
    </div>
  );
}
