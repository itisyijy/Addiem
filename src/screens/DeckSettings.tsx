// C4 목표 및 학습 설정 / C4-b 상한 부족 경고 / C4-p 잠시 쉬기
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, planApi, type ConfigPreview, type DeckConfig, type DeckDetail } from "../api";
import { Button, Icon, Toggle } from "../ui";
import { n, studyDate, useKey, useNav } from "../nav";

const WEEKDAY = "일월화수목금토";
const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const parse = (s: string) => {
  const [y, m, d] = s.split("-").map(Number);
  return new Date(y, m - 1, d);
};
/** 2026. 10. 28. (수) */
const longDate = (s: string) => {
  const d = parse(s);
  return `${d.getFullYear()}. ${d.getMonth() + 1}. ${d.getDate()}. (${WEEKDAY[d.getDay()]})`;
};
/** 10월 28일 */
const shortDate = (s: string) => {
  const d = parse(s);
  return `${d.getMonth() + 1}월 ${d.getDate()}일`;
};
/** 10. 16. (금) */
const dotDate = (s: string) => {
  const d = parse(s);
  return `${d.getMonth() + 1}. ${d.getDate()}. (${WEEKDAY[d.getDay()]})`;
};
const addDays = (s: string, days: number) => {
  const d = parse(s);
  d.setDate(d.getDate() + days);
  return iso(d);
};
/** a 부터 b 까지 (둘 다 포함) 며칠 */
const spanDays = (a: string, b: string) => Math.round((parse(b).getTime() - parse(a).getTime()) / 86_400_000) + 1;
/** 10 아래는 1씩, 그 위는 5씩 */
const step = (v: number | null, dir: 1 | -1, fallback: number) => {
  const cur = v ?? fallback;
  const by = cur < 10 || (dir < 0 && cur <= 10) ? 1 : 5;
  return Math.max(0, Math.min(999, cur + dir * by));
};

export function DeckSettings({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [deck, setDeck] = useState<DeckDetail | null>(null);
  const [saved, setSaved] = useState<DeckConfig | null>(null);
  const [edit, setEdit] = useState<DeckConfig | null>(null);
  const [before, setBefore] = useState<ConfigPreview | null>(null);
  const [after, setAfter] = useState<ConfigPreview | null>(null);
  const [error, setError] = useState<string>();
  // H2 에서 고른 조정이 오늘 신규를 몇 장 바꿨는지 ("충돌 조정으로 +18장"). 미리보기 산식에는 들어가지 않는다
  const [adjust, setAdjust] = useState<{ mode: string; delta: number } | null>(null);
  useEffect(() => {
    planApi.adjust(deckId).then(setAdjust, () => {});
  }, [deckId]);
  const dateInput = useRef<HTMLInputElement>(null);
  const pauseInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.deckDetail(deckId).then((d) => {
      const c: DeckConfig = {
        deck_id: d.id,
        target_date: d.target_date,
        include_target_day: d.include_target_day,
        daily_new_limit: d.daily_new_limit,
        today_new: d.today_new_override,
        paused: d.paused,
        paused_until: d.paused_until,
      };
      setDeck(d);
      setSaved(c);
      setEdit(c);
      return api.previewDeckConfig(c).then(setBefore);
    }).catch((e) => setError(errorText(e)));
  }, [deckId]);

  useEffect(() => {
    if (!edit) return;
    const t = setTimeout(() => api.previewDeckConfig(edit).then(setAfter, (e) => setError(errorText(e))), 120);
    return () => clearTimeout(t);
  }, [edit]);

  const back = useCallback(() => nav({ name: "deck", deckId }), [nav, deckId]);
  useKey("Escape", back);
  if (!deck || !edit || !saved || !before || !after) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  const set = (patch: Partial<DeckConfig>) => setEdit({ ...edit, ...patch });
  const hasDate = edit.target_date !== null;
  const warn = hasDate && ((after.overflow_days ?? 0) > 0 || after.never_finish);
  const limitShown = edit.daily_new_limit ?? (hasDate ? null : 20);
  const todayShown = edit.today_new ?? (edit.paused ? after.today_new : after.per_day); // 쉬는 중이면 오늘 신규는 0
  // 아무것도 바꾸지 않았으면 오른쪽은 "지금 계획"으로 값만 보여준다
  const changed = JSON.stringify(edit) !== JSON.stringify(saved);
  const paused = !!edit.paused;
  const today = iso(studyDate());
  const pausedDays = paused && edit.paused_until ? spanDays(today, edit.paused_until) : null;
  // 목표일은 자동으로 미루지 않는다. 사용자가 고르면 쉬는 날수만큼 민다
  const shifted = hasDate && pausedDays && pausedDays > 0 ? addDays(edit.target_date!, pausedDays) : null;

  // 마지막 학습일 다음 날을 목표일로. 목표일 당일도 학습하면 마지막 학습일이 곧 목표일이다
  const suggested = (() => {
    if (!after.finish_date) return null;
    const d = parse(after.finish_date);
    if (!edit.include_target_day) d.setDate(d.getDate() + 1);
    return iso(d);
  })();

  const save = async () => {
    try {
      await api.saveDeckConfig(edit);
      back();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const days = (p: ConfigPreview) => (p.days === null ? "없음" : `${n(Math.max(p.days, 0))}일`);
  const rows: [string, string, string][] = warn
    ? [
        ["하루 신규 상한", saved.daily_new_limit === null ? "없음" : `${n(saved.daily_new_limit)}장`, `${n(edit.daily_new_limit ?? 0)}장`],
        ["하루 신규 필요", before.needed_per_day === null ? "—" : `${n(before.needed_per_day)}장`, `${n(after.needed_per_day ?? 0)}장`],
        ["목표 완료 예상", before.finish_date ? shortDate(before.finish_date) : "—", after.finish_date ? shortDate(after.finish_date) : "—"],
      ]
    : [
        ["학습 가능 일수", days(before), days(after)],
        [paused ? "다시 시작한 뒤 하루 신규" : "하루 신규 권장", `${n(before.per_day)}장`, `${n(after.per_day)}장`],
        ["오늘 학습량", `${n(before.today_total)}장${saved.paused ? " · 쉬는 중" : ""}`, `${n(after.today_total)}장${paused ? " · 쉬는 중" : ""}`],
      ];

  const formula = (() => {
    const r = after.remaining_new;
    if (paused && !after.resume_date) return `쉬는 동안은 새 카드가 들어오지 않아요. 다시 시작하면 남은 신규 ${n(r)}장을 남은 날로 다시 나눠요`;
    if (!hasDate) return `목표일이 없어서 하루 ${n(after.per_day)}장씩 꾸준히 넣어요. 남은 신규 ${n(r)}장`;
    if (after.days === null || after.days <= 0) return `목표일이 지났거나 오늘이에요. 남은 신규 ${n(r)}장을 상한만큼 넣어요`;
    if (warn && after.never_finish) return `남은 신규 ${n(r)}장 ÷ ${n(after.days)}일 = 하루 ${n(after.needed_per_day ?? 0)}장 필요 · 상한이 0장이라 끝나지 않아요`;
    if (warn) {
      const took = after.days + (after.overflow_days ?? 0);
      return `남은 신규 ${n(r)}장 ÷ ${n(after.days)}일 = 하루 ${n(after.needed_per_day ?? 0)}장 필요 · 상한 ${n(edit.daily_new_limit ?? 0)}장이면 ${n(took)}일 걸려요`;
    }
    const exact = r / after.days;
    if (paused && after.resume_date)
      return `남은 신규 ${n(r)}장 ÷ 쉬지 않는 ${n(after.days)}일 = ${Number.isInteger(exact) ? n(exact) : exact.toFixed(1)} → ${shortDate(after.resume_date)}부터 하루 ${n(Math.ceil(exact))}장`;
    return `남은 신규 ${n(r)}장 ÷ ${n(after.days)}일 = ${Number.isInteger(exact) ? n(exact) : exact.toFixed(1)} → 하루 ${n(Math.ceil(exact))}장${Number.isInteger(exact) ? "" : " (올림)"}`;
  })();

  return (
    <main className="cards">
      <button className="backlink t-label-m c-secondary" onClick={back}>
        <Icon name="chevron-left" size={18} />
        {deck.name}
      </button>
      <div className="deck__title">
        <h1 className="t-title-l">목표 및 학습 설정</h1>
        <p className="t-body-s c-secondary">이 덱에만 적용돼요. 바꾸면 다음 학습부터 계획을 다시 계산해요.</p>
      </div>

      <div className="settings">
        <section className="settings__form">
          <div className="settings__row">
            <span className="settings__label">
              <span className="t-body-m-strong">목표일</span>
              <span className="t-body-s c-secondary">시험일이나 마감일. 남은 신규 카드를 이 날짜까지 나눠요.</span>
            </span>
            <span className="settings__date">
              <span className="segment segment--boxed">
                <button className={`segment__item t-label-m ${!hasDate ? "is-selected" : ""}`} onClick={() => set({ target_date: null })}>
                  목표일 없음
                </button>
                <button
                  className={`segment__item t-label-m ${hasDate ? "is-selected" : ""}`}
                  onClick={() => {
                    if (hasDate) return;
                    const d = studyDate();
                    d.setDate(d.getDate() + 30);
                    set({ target_date: saved.target_date ?? iso(d) });
                  }}
                >
                  날짜 지정
                </button>
              </span>
              {hasDate && (
                <span
                  className="settings__date-field t-body-m"
                  role="button"
                  tabIndex={0}
                  onClick={() => dateInput.current?.showPicker()}
                  onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && dateInput.current?.showPicker()}
                >
                  <span>{longDate(edit.target_date!)}</span>
                  <Icon name="calendar" size={18} scaled className="c-secondary" />
                  <input
                    ref={dateInput}
                    type="date"
                    tabIndex={-1}
                    aria-label="목표일"
                    value={edit.target_date!}
                    min={iso(studyDate())}
                    onChange={(e) => e.target.value && set({ target_date: e.target.value })}
                  />
                </span>
              )}
            </span>
          </div>

          <div className="settings__row">
            <span className="settings__label">
              <span className="t-body-m-strong">목표일 당일도 학습</span>
              <span className="t-body-s c-secondary">시험 시작 전에 복습할 수 없다면 꺼두세요. 기본은 꺼짐이에요.</span>
            </span>
            <Toggle on={edit.include_target_day} label="목표일 당일도 학습" onChange={(v) => set({ include_target_day: v })} />
          </div>

          <div className="settings__row">
            <span className="settings__label">
              <span className="t-body-m-strong">하루 신규 카드 상한</span>
              <span className="t-body-s c-secondary">하위 덱은 이 상한을 나눠 써요. 권장량보다 작으면 경고해요.</span>
            </span>
            <Stepper
              value={limitShown === null ? "없음" : n(limitShown)}
              warn={warn}
              label="하루 신규 카드 상한"
              onReset={edit.daily_new_limit !== null ? () => set({ daily_new_limit: null }) : undefined}
              onMinus={() => set({ daily_new_limit: step(edit.daily_new_limit, -1, after.per_day) })}
              onPlus={() => set({ daily_new_limit: step(edit.daily_new_limit, 1, after.per_day) })}
            />
          </div>

          <div className="settings__row">
            <span className="settings__label">
              <span className="t-body-m-strong">오늘만 신규 분량 조정</span>
              <span className="t-body-s c-secondary">오늘 계획에만 적용돼요. 예정된 복습은 줄이지 않아요.</span>
            </span>
            <Stepper
              value={n(todayShown)}
              label="오늘만 신규 분량"
              onReset={edit.today_new !== null ? () => set({ today_new: null }) : undefined}
              onMinus={() => set({ today_new: step(todayShown, -1, 0) })}
              onPlus={() => set({ today_new: step(todayShown, 1, 0) })}
            />
          </div>

          <div className="settings__row settings__row--last settings__pause">
            <span className="settings__pause-top">
              <span className="settings__label">
                <span className="t-body-m-strong">이 덱 잠시 쉬기</span>
                <span className="t-body-s c-secondary">
                  쉬는 동안 오늘의 학습에서 빠지고 새 카드도 들어오지 않아요. 목표일은 그대로라 다시 시작하면 하루 분량이 늘어요.
                </span>
              </span>
              <Toggle on={paused} label="이 덱 잠시 쉬기" onChange={(v) => set({ paused: v, paused_until: null })} />
            </span>
            {paused && (
              <span className="settings__pause-period">
                <span className="segment segment--boxed">
                  <button className={`segment__item t-label-m ${!edit.paused_until ? "is-selected" : ""}`} onClick={() => set({ paused_until: null })}>
                    다시 시작할 때까지
                  </button>
                  <button
                    className={`segment__item t-label-m ${edit.paused_until ? "is-selected" : ""}`}
                    onClick={() => edit.paused_until || set({ paused_until: addDays(today, 6) })}
                  >
                    날짜까지
                  </button>
                </span>
                {edit.paused_until && (
                  <span
                    className="settings__pause-date t-label-m"
                    role="button"
                    tabIndex={0}
                    onClick={() => pauseInput.current?.showPicker()}
                    onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && pauseInput.current?.showPicker()}
                  >
                    {dotDate(edit.paused_until)}까지 · {n(pausedDays ?? 0)}일
                    <Icon name="calendar" size={16} scaled className="c-secondary" />
                    <input
                      ref={pauseInput}
                      type="date"
                      tabIndex={-1}
                      aria-label="쉬는 마지막 날"
                      value={edit.paused_until}
                      min={today}
                      onChange={(e) => e.target.value && set({ paused_until: e.target.value })}
                    />
                  </span>
                )}
              </span>
            )}
          </div>
        </section>

        <section className="recalc">
          <h2 className="recalc__head t-title-s c-accent">
            <Icon name="rotate" size={18} />
            {changed ? "바꾼 뒤 계획" : "지금 계획"}
          </h2>
          {rows.map(([label, a, b]) => (
            <div key={label} className="recalc__row">
              <span className="t-body-m c-secondary">{label}</span>
              {/* 바뀐 줄만 "전 → 후" */}
              <span className="recalc__change">
                {changed && a !== b && (
                  <>
                    <span className="t-body-m c-tertiary">{a}</span>
                    <Icon name="arrow-right" size={16} className="c-tertiary" />
                  </>
                )}
                <span className="t-title-s">{b}</span>
              </span>
            </div>
          ))}
          <p className="recalc__formula t-body-s c-secondary">{formula}</p>
          {adjust && adjust.delta !== 0 && (
            <p className="recalc__adjust t-body-s c-accent">
              충돌 조정으로 {adjust.delta > 0 ? "+" : ""}
              {n(adjust.delta)}장 · {adjust.mode === "first" ? "먼저 볼 덱의 시험 뒤에 신규를 넣어요" : "늦은 시험의 신규를 앞당겨 나눠요"}
              <button className="recalc__adjust-link t-label-m" onClick={() => nav({ name: "planConflict", back: { name: "deckSettings", deckId } })}>
                조정 보기
              </button>
            </p>
          )}
          {hasDate && !warn && !paused && after.days !== null && after.days > 0 && (
            <div className="recalc__ok">
              <Icon name="check-circle" size={20} className="c-success" />
              <span className="t-body-m-strong c-success">
                {edit.daily_new_limit !== null
                  ? `상한 ${n(edit.daily_new_limit)}장 안에서 목표일까지 끝낼 수 있어요`
                  : `하루 ${n(after.per_day)}장이면 목표일까지 끝낼 수 있어요`}
              </span>
            </div>
          )}
          {warn && (
            <div className="recalc__warn">
              <div className="recalc__warn-head">
                <Icon name="alert" size={20} className="c-warn" />
                <span className="recalc__warn-text">
                  <span className="t-body-m-strong c-warn">
                    {after.never_finish ? "이대로면 목표일까지 끝낼 수 없어요" : `이대로면 목표일을 ${n(after.overflow_days ?? 0)}일 넘겨요`}
                  </span>
                  <span className="t-body-s c-secondary">목표는 그대로 두고 상한만 낮췄어요. 둘 중 하나를 고르거나, 알고 있다면 이대로 저장해도 돼요.</span>
                </span>
              </div>
              <div className="recalc__warn-actions">
                <Button
                  variant="secondary"
                  size="M"
                  label={`상한 ${n(after.needed_per_day ?? 0)}장으로`}
                  onClick={() => set({ daily_new_limit: after.needed_per_day })}
                />
                {suggested && (
                  <Button variant="secondary" size="M" label={`목표일 ${shortDate(suggested)}로`} onClick={() => set({ target_date: suggested })} />
                )}
              </div>
            </div>
          )}
          {paused && (
            <div className="recalc__warn">
              <div className="recalc__warn-head">
                <Icon name="alert" size={20} className="c-warn" />
                <span className="recalc__warn-text">
                  <span className="t-body-m-strong c-warn">
                    {pausedDays === null
                      ? "다시 시작할 때까지 새 카드가 들어오지 않아요"
                      : hasDate && (after.days ?? 0) < (before.days ?? 0) // 목표일을 같이 미뤘으면 몰리지 않는다
                        ? `쉬는 ${n(pausedDays)}일만큼 남은 날에 몰려요`
                        : `${n(pausedDays)}일 동안 새 카드가 들어오지 않아요`}
                  </span>
                  <span className="t-body-s c-secondary">
                    그동안 예정된 복습{after.paused_reviews === null ? "은" : ` 약 ${n(after.paused_reviews)}장은`} 다시 시작하는 날 밀린 복습으로 올라와요 (하루
                    200장까지).
                  </span>
                </span>
              </div>
              {/* 한 번 미뤘으면 다시 보이지 않게 (같은 화면에서 두 번 밀지 않도록) */}
              {shifted && edit.target_date === saved.target_date && (
                <div className="recalc__warn-actions">
                  <Button
                    variant="secondary"
                    size="M"
                    label={`목표일도 ${n(pausedDays!)}일 미루기 (${shortDate(shifted)})`}
                    onClick={() => set({ target_date: shifted })}
                  />
                </div>
              )}
            </div>
          )}
        </section>
      </div>

      <div className="settings__footer">
        {error && <span className="t-body-s c-danger">{error}</span>}
        <Button variant="ghost" label="취소" onClick={back} />
        <Button label="저장" onClick={save} />
      </div>
    </main>
  );
}

/** onReset: 값을 정해 둔 경우에만 왼쪽에 "자동". 누르면 null 로 돌아가 권장대로 계산한다 */
function Stepper({ value, label, warn, onMinus, onPlus, onReset }: {
  value: string;
  label: string;
  warn?: boolean;
  onMinus: () => void;
  onPlus: () => void;
  onReset?: () => void;
}) {
  return (
    <span className="settings__stepper">
      {onReset && (
        <button className="settings__auto t-label-s c-tertiary" aria-label={`${label} 자동으로`} onClick={onReset}>
          자동
        </button>
      )}
      <span className={`stepper ${warn ? "stepper--warn" : ""}`} role="group" aria-label={label}>
        <button className="stepper__btn t-title-s" aria-label="줄이기" onClick={onMinus}>
          −
        </button>
        <span className="stepper__value t-title-s">{value}</span>
        <button className="stepper__btn" aria-label="늘리기" onClick={onPlus}>
          <Icon name="plus" size={16} />
        </button>
      </span>
    </span>
  );
}
