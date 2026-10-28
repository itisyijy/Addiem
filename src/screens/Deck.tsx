// C1 덱 상세 / C1-a 빈 덱 / C1-b 목표일 없음 / C1-m · C1-r ⋯ 메뉴 / C1-t 오늘 완료 + 하위 덱 목록 / C1-p 쉬는 중
import "../styles/deck.css";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errorText, planApi, type DeckDetail, type Level, type PlanMode, type Subdeck } from "../api";
import { Badge, Button, Checkbox, Icon, Importance, Input, Kbd, StatTile } from "../ui";
import { n, studyDate, useKey, useNav, useOverlayScope } from "../nav";
import { actionKey, plainEnter } from "../keys";
import { useToast, type ToastInput } from "../toast";
import { Modal, NewDeckModal } from "./NewDeck";
import { ExportDeckDialog } from "./ExportDeck";
import { SourcesButton } from "./Sources";
import { ExamLine } from "./Exam";
import { CramHint } from "./StudyParts";
import { useSyncStatus } from "../sync";

const WEEKDAY = "일월화수목금토";
export function dateWithDay(iso: string) {
  const [y, m, d] = iso.split("-").map(Number);
  return `${m}월 ${d}일 (${WEEKDAY[new Date(y, m - 1, d).getDay()]})`;
}
/** queue.rs BURY_RETURN_DAYS. 접어둔 카드는 학습일 기준 3일 뒤 오늘의 학습에 돌아온다 */
export const BURY_DAYS = 3;
/** 지금 접으면 돌아오는 날 "10월 18일 (토)" */
export function buryReturnDay() {
  const d = studyDate();
  d.setDate(d.getDate() + BURY_DAYS);
  return dateWithDay(d.toLocaleDateString("sv-SE"));
}
export const ddayLabel = (dd: number) => (dd === 0 ? "D-day" : dd > 0 ? `D-${dd}` : `D+${-dd}`);
/** "10월 16일(금)". 쉬기 배너·메뉴는 괄호 앞을 띄우지 않는다 (C1-p, C1-r) */
const dayTight = (iso: string) => dateWithDay(iso).replace(" (", "(");
const parse = (iso: string) => {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d);
};
const iso = (t: Date) => `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")}`;
export const addDays = (s: string, days: number) => {
  const t = parse(s);
  t.setDate(t.getDate() + days);
  return iso(t);
};
/** "10월 17일" */
const monthDay = (s: string) => `${parse(s).getMonth() + 1}월 ${parse(s).getDate()}일`;

type Reload = () => Promise<unknown>;

/**
 * role="menu" 의 onKeyDown. ↑↓ 로 항목 사이를 옮기고 Home/End 로 처음·끝 (꺼진 항목은 건너뛴다).
 * 하위 메뉴 안의 항목은 그 하위 메뉴가 따로 받는다 (바로 아래 항목과 한 겹 감싼 항목만 센다)
 */
export function menuKeys(e: KeyboardEvent<HTMLElement>) {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const items = [
    ...e.currentTarget.querySelectorAll<HTMLElement>(":scope > [role=menuitem]:not(:disabled), :scope > div > [role=menuitem]:not(:disabled)"),
  ];
  if (!items.length) return;
  e.preventDefault();
  e.stopPropagation();
  const i = items.indexOf(document.activeElement as HTMLElement);
  const last = items.length - 1;
  const next =
    e.key === "Home" ? 0 : e.key === "End" ? last : e.key === "ArrowDown" ? (i < 0 || i === last ? 0 : i + 1) : i <= 0 ? last : i - 1;
  items[next].focus();
}

// 하위 덱으로 옮겨 가도 메뉴·이름 편집·펼침 상태가 남지 않게 덱마다 새로 그린다
export const Deck = ({ deckId }: { deckId: number }) => <DeckScreen key={deckId} deckId={deckId} />;

function DeckScreen({ deckId }: { deckId: number }) {
  const [d, setD] = useState<DeckDetail | null>(null);
  const [error, setError] = useState<string>();
  const reload = useCallback(() => api.deckDetail(deckId).then(setD, (e) => setError(errorText(e))), [deckId]);
  useEffect(() => {
    reload();
  }, [reload]);
  if (!d) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;
  return d.total === 0 ? <EmptyDeck d={d} reload={reload} /> : <FullDeck d={d} reload={reload} />;
}

/**
 * 잠시 쉬기 ↔ 다시 시작. ⋯ 메뉴(C1-r)와 쉬는 중 배너가 같이 쓴다 (C4 스위치와 같은 상태).
 * pause(until): until 은 쉬는 마지막 날, null 이면 다시 시작할 때까지
 */
function usePause(d: DeckDetail, reload: Reload) {
  const toast = useToast();
  const set = async (paused: boolean, until: string | null) => {
    await api.setDeckPaused(d.id, paused, until);
    await reload();
  };
  const fail = (e: unknown) => toast({ title: "바꾸지 못했어요", text: errorText(e), tone: "error" });
  const resume = () => set(false, null).then(() => toast({ title: "다시 시작했어요", text: d.name }), fail);
  const pause = (until: string | null) =>
    set(true, until).then(
      () =>
        toast({
          title: until ? `${monthDay(until)}까지 쉬어요` : "다시 시작할 때까지 쉬어요",
          text: d.name,
          tone: "undo",
          action: { label: "되돌리기", onClick: () => set(false, null).catch(fail) },
        }),
      fail,
    );
  return { pause, resume };
}

function Header({ d, reload, variant }: { d: DeckDetail; reload: Reload; variant: "empty" | "full" }) {
  const nav = useNav();
  const { pause, resume } = usePause(d, reload);
  const [renaming, setRenaming] = useState(false);
  useKey("F2", useCallback(() => setRenaming(true), []), !renaming);
  const sub =
    d.total === 0
      ? "방금 만든 덱 · 카드 0장"
      : `카드 ${n(d.total)}장${d.subdeck_count > 0 ? ` · 하위 덱 ${n(d.subdeck_count)}개` : ""}`;
  const parentPath = d.path.slice(0, -1);
  return (
    <>
      {/* 하위 덱은 부모 덱으로, 최상위 덱은 홈으로 */}
      <button
        className="backlink t-label-m c-secondary"
        onClick={() => nav(d.parent_id === null ? { name: "home" } : { name: "deck", deckId: d.parent_id })}
      >
        <Icon name="chevron-left" size={18} />
        {parentPath.length ? parentPath.join(" › ") : "홈"}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          {renaming ? (
            <RenameTitle
              d={d}
              sub={sub}
              onDone={async (changed) => {
                if (changed) await reload(); // 옛 이름이 잠깐 보이지 않게 새 이름을 받은 뒤 닫는다
                setRenaming(false);
              }}
            />
          ) : (
            <>
              <h1 className="t-title-l">{d.name}</h1>
              <p className="t-body-s c-secondary">{sub}</p>
            </>
          )}
        </div>
        <div className="deck__actions">
          {variant === "full" && (
            <>
              {/* 카드가 생긴 뒤에도 더 넣을 길이 보이게 (QA T1-08) */}
              <Button variant="secondary" size="M" icon="plus" label="카드 추가" shortcut={actionKey("addCard")} onClick={() => nav({ name: "addCard", deckId: d.id })} />
              <Button variant="secondary" size="M" icon="list" label="카드 보기" onClick={() => nav({ name: "cards", deckId: d.id })} />
            </>
          )}
          {/* I1 [자료 N] (screens/Sources.tsx). 빈 덱에도 둔다: 강의 PDF 부터 넣고 그림·표 · 초안으로 첫 카드를 만들 수 있게 (QA T4-03) */}
          <SourcesButton deckId={d.id} />
          <Button
            variant={variant === "full" ? "secondary" : "ghost"}
            size="M"
            icon="target"
            label="목표 및 학습 설정"
            onClick={() => nav({ name: "deckSettings", deckId: d.id })}
          />
          <MoreMenu d={d} onRename={() => setRenaming(true)} onPause={pause} onResume={resume} />
        </div>
      </div>
      {d.paused && (
        <div className="deck__paused">
          <Icon name="clock" size={18} className="c-tertiary" />
          <p className="t-body-m c-secondary">
            {d.paused_until ? `이 덱은 ${dayTight(d.paused_until)}까지 쉬어요` : "이 덱은 다시 시작할 때까지 쉬어요"} · 다시 시작하면 하루 신규{" "}
            {n(d.per_day)}장
          </p>
          <Button variant="secondary" size="M" icon="rotate" label="지금 다시 시작" onClick={resume} />
        </div>
      )}
    </>
  );
}

/** C1-m 이름 바꾸기. 모달 없이 제목 자리에서. Enter 저장, Esc 취소, 중복이면 빨간 밑줄 + 문구 */
function RenameTitle({ d, sub, onDone }: { d: DeckDetail; sub: string; onDone: (changed: boolean) => void }) {
  const [error, setError] = useState<string>();
  // 입력값은 DOM 에서 바로 읽는다. Esc 로 값을 되돌린 뒤 blur 가 저장하게 해서 저장 경로를 하나로 둔다
  const commit = async (el: HTMLInputElement) => {
    const name = el.value.trim();
    if (!name || name === d.name) return onDone(false);
    try {
      await api.renameDeck(d.id, name);
      onDone(true);
    } catch (e) {
      setError(errorText(e));
      el.focus();
    }
  };
  return (
    <>
      <input
        className={`deck__rename t-title-l ${error ? "is-error" : ""}`}
        aria-label="덱 이름"
        defaultValue={d.name}
        autoFocus
        onFocus={(e) => e.currentTarget.select()}
        onChange={() => setError(undefined)}
        onKeyDown={(e) => {
          // 한글 조합 중 Enter 는 글자만 마친다. 그 Enter 로 칸을 나가면 확정 Enter 가 화면의 "Enter = 학습 시작"으로 샌다 (QA T1-25 · T8-09)
          if (plainEnter(e)) e.currentTarget.blur();
          if (e.key === "Escape") {
            e.currentTarget.value = d.name;
            e.currentTarget.blur();
          }
        }}
        onBlur={(e) => commit(e.currentTarget)}
      />
      <p className={`t-body-s ${error ? "c-danger" : "c-secondary"}`}>{error ?? sub}</p>
    </>
  );
}

// ---------------------------------------------------------------- C1-a

function EmptyDeck({ d, reload }: { d: DeckDetail; reload: Reload }) {
  const nav = useNav();
  useKey({ action: "addCard" }, useCallback(() => nav({ name: "addCard", deckId: d.id }), [nav, d.id]));
  return (
    <main className="deck deck--empty">
      <Header d={d} reload={reload} variant="empty" />
      <section className="deck__empty">
        <span className="deck__empty-icon c-accent">
          <Icon name="layers" size={28} />
        </span>
        <span className="deck__empty-gap" />
        <h2 className="t-title-m">아직 카드가 없어요</h2>
        <p className="t-body-m c-secondary">첫 카드를 하나 넣으면 바로 학습할 수 있어요. 많다면 JSON으로 한 번에 넣어도 돼요.</p>
        <div className="deck__empty-actions">
          <Button icon="plus" label="첫 카드 추가" shortcut={actionKey("addCard")} onClick={() => nav({ name: "addCard", deckId: d.id })} />
          <Button variant="secondary" icon="upload" label="JSON으로 가져오기" onClick={() => nav({ name: "importJson", deckId: d.id })} />
        </div>
      </section>
      {/* 빈 하위 덱만 만들어 둔 덱에서도 하위 덱으로 들어갈 수 있게 */}
      {d.tree.length > 0 && <SubdeckList d={d} reload={reload} />}
    </main>
  );
}

// ---------------------------------------------------------------- C1, C1-b, C1-t, C1-p

/** H2 충돌 조정 (plan.rs). 목표일은 그대로 두고 신규를 나누는 방식만 바뀐 덱 */
type Adjust = { mode: PlanMode; delta: number } | null;

function reasonText(d: DeckDetail, adjust: Adjust) {
  if (d.paused) return d.paused_until ? `쉬는 중 · ${monthDay(addDays(d.paused_until, 1))}부터 다시 올라와요` : "쉬는 중 · 다시 시작할 때까지 올라오지 않아요";
  if (d.unseen === 0) return "남은 신규 카드가 없어요 · 복습만 남았어요";
  if (adjust?.mode === "first" && d.per_day === 0) return "먼저 볼 덱의 시험이 끝난 뒤부터 신규를 넣어요 · 목표일은 그대로";
  if (adjust?.mode === "spread") return `앞선 시험 전에 나눠 넣어요 · 하루 신규 ${n(d.per_day)}장 (충돌 조정 ${adjust.delta > 0 ? "+" : ""}${n(adjust.delta)}장)`;
  // H1 "실제 속도에 맞춰 다시 나누기"를 끈 덱. 큐가 남긴 문장으로 알아본다
  if (d.reason?.startsWith("하루 몫")) return `하루 신규 ${n(d.per_day)}장으로 고정했어요 · 학습 계획에서 바꿀 수 있어요`;
  // 상위 덱 목표를 따르는 하위 덱은 이 덱 설정으로 계산할 수 없다. 큐가 남긴 문장("상위 덱: 남은 …")을 그대로 쓴다
  if (!d.target_date && d.target_from && d.reason) return d.reason;
  if (!d.target_date || d.learnable_days === null) return `목표일 없음 · 하루 신규 상한 ${n(d.per_day)}장대로`;
  // 당일 학습을 끈 목표일 당일은 "지났어요"가 아니다 (QA T3-03). 수는 이 덱 예산이 나눌 신규만 (자기 설정을 가진 하위 덱은 따로, QA T3-08)
  if (d.learnable_days === 0) return `오늘이 목표일이에요 · 남은 신규 ${n(d.budget_unseen)}장을 오늘 모두 올렸어요`;
  if (d.learnable_days < 0) return `목표일이 지났어요 · 남은 신규 ${n(d.budget_unseen)}장을 모두 올렸어요`;
  const needed = Math.ceil(d.budget_unseen / d.learnable_days);
  if (d.daily_new_limit !== null && needed > d.daily_new_limit)
    return `하루 ${n(needed)}장이 필요한데 상한이 ${n(d.daily_new_limit)}장이에요`;
  return `남은 신규 ${n(d.budget_unseen)}장 ÷ ${n(d.learnable_days)}일 = 하루 ${n(needed)}장`;
}

function FullDeck({ d, reload }: { d: DeckDetail; reload: Reload }) {
  const nav = useNav();
  const today = d.today_review + d.today_new + d.today_returning; // 남은 분량
  const planned = d.done_today + today;
  // 쉬는 중에도 막지는 않는다 (C1-p). D1 이 예정 복습만 보여준다
  const canStart = today > 0 || d.paused;
  const start = useCallback(() => canStart && nav({ name: "summary", deckIds: [d.id] }), [nav, d.id, canStart]);
  // Enter 로 학습 시작. 초점이 버튼·메뉴 항목에 있으면 useKey 가 그 요소에 넘긴다
  useKey("Enter", start);
  useKey({ action: "addCard" }, useCallback(() => nav({ name: "addCard", deckId: d.id }), [nav, d.id]));
  // 하위 덱은 목표일이 없으면 상위 덱 목표를 따른다 (C1-t)
  const goal =
    d.target_date && d.dday !== null
      ? { dday: d.dday, text: `${dateWithDay(d.target_date)} 시험` }
      : d.target_from && { dday: d.target_from.dday, text: `상위 덱 「${d.target_from.name}」 목표를 따라요` };
  const showSuggestion = !goal && (d.total >= 20 || d.studied);
  const [adjust, setAdjust] = useState<Adjust>(null);
  useEffect(() => {
    planApi.adjust(d.id).then(setAdjust, () => {});
  }, [d.id, d.per_day]);
  const seen = d.total - d.unseen - d.unseen_empty;

  return (
    <main className="deck">
      <Header d={d} reload={reload} variant="full" />
      <div className="deck__row">
        <section className="deck__today">
          {/* 하나라도 끝내면 큰 숫자는 남은 분량이라는 걸 밝힌다 (C1-t) */}
          <p className="t-body-l c-on-inverse">{d.done_today > 0 ? "오늘 남은" : "오늘"}</p>
          <p className="deck__today-count">
            <span className="deck__today-num">{n(today)}</span>
            <span className="t-display-l">장</span>
          </p>
          <div className="deck__today-mix t-body-m">
            <span>
              <span className="c-on-inverse">복습</span> <b>{n(d.today_review)}</b>
            </span>
            <span>
              <span className="c-on-inverse">신규</span> <b>{n(d.today_new)}</b>
            </span>
            <span>
              <span className="c-on-inverse">접어둔 카드 복귀</span> <b>{n(d.today_returning)}</b>
            </span>
          </div>
          {d.done_today > 0 && (
            <div className="deck__done">
              <div className="deck__done-row">
                <span className="deck__done-count">
                  <span className="t-body-m c-tertiary">오늘 완료</span>
                  <span className="t-title-s">{n(d.done_today)}</span>
                  <span className="t-body-s c-tertiary">/ 계획 {n(planned)}</span>
                </span>
                <span className="t-body-s c-tertiary">남은 {n(today)}장</span>
              </div>
              <div className="deck__done-track">
                <span className="deck__done-fill" style={{ width: `${(d.done_today / planned) * 100}%` }} />
              </div>
            </div>
          )}
          <div className="deck__reason-wrap">
            <span className="deck__reason t-label-s">
              <Icon name="info" size={16} />
              {reasonText(d, adjust)}
            </span>
            {/* 앞면이 빈 카드는 큐 · 남은 수에서 빠진다 (NT49). "오늘 0장"의 이유를 밝힌다 */}
            {d.front_empty.length > 0 && (
              <button type="button" className="deck__blank t-label-s" onClick={() => nav({ name: "cards", deckId: d.id, cardIds: d.front_empty })}>
                <Icon name="alert" size={16} />
                앞면이 빈 카드 {n(d.front_empty.length)}장은 학습에 나오지 않아요 · 카드 목록에서 보기
              </button>
            )}
          </div>
          {d.paused ? (
            <Button variant="secondary" className="deck__start" label="쉬는 중에도 학습하기" shortcut="Enter" onClick={start} />
          ) : (
            <Button
              className="deck__start"
              icon="arrow-right"
              label={today === 0 ? "오늘 학습을 마쳤어요" : d.done_today > 0 ? "이어서 학습" : "학습 시작"}
              shortcut={today > 0 ? "Enter" : undefined}
              disabled={today === 0}
              onClick={start}
            />
          )}
        </section>

        <div className="deck__side">
          {goal ? (
            <section className="deck__goal">
              <div className="deck__goal-head">
                <p className="deck__goal-date">
                  <span className="t-display-l">{ddayLabel(goal.dday)}</span>
                  <span className="t-body-m c-secondary">{goal.text}</span>
                </p>
                {goal.dday < 0 ? (
                  <Badge tone="warn">목표일이 지났어요</Badge>
                ) : d.on_track === null ? null : d.on_track ? (
                  <Badge tone="success">신규 첫 학습 완료 예상</Badge>
                ) : (
                  <Badge tone="warn">신규 첫 학습이 목표일을 넘겨요</Badge>
                )}
              </div>
              <Progress label="학습을 시작한 카드" value={seen} total={d.total} pct={d.seen_pct} color="ink" />
              {d.core_total > 0 ? (
                <Progress label="학습을 시작한 핵심 카드" value={d.core_seen} total={d.core_total} pct={d.core_learned_pct} color="core" />
              ) : (
                <p className="t-body-s c-tertiary">핵심 카드 없음 · 중요도 조정에서 정할 수 있어요</p>
              )}
              <p className="t-caption c-tertiary">
                신규 상태를 벗어난 카드의 비율이에요. 정답률이나 숙달도가 아니고, 완료 예상은 지금 분량 기준 추정이에요.
              </p>
              {/* H1. 날짜별 분량과 쓸 수 있는 시간 */}
              <Button variant="ghost" size="M" icon="calendar" label="계획 보기" className="deck__plan" onClick={() => nav({ name: "plan", deckId: d.id })} />
              {/* P0 시험 전 회독 입구 (목표일 14일 전부터) */}
              <CramHint deckIds={[d.id]} named={false} />
            </section>
          ) : (
            showSuggestion && (
              <section className="deck__suggest">
                <div className="deck__suggest-head">
                  <span className="deck__suggest-icon c-accent">
                    <Icon name="target" size={22} />
                  </span>
                  <span className="t-title-s">목표일 없이 꾸준히 반복하는 덱이에요</span>
                </div>
                <p className="t-body-s c-secondary">
                  시험이나 마감이 생기면 목표일을 정해 보세요. 남은 {n(d.unseen)}장을 날짜에 맞춰 하루 분량으로 나눠 드려요.
                </p>
                <div className="deck__suggest-cta">
                  <Button variant="secondary" size="M" icon="calendar" label="목표일 정하기" onClick={() => nav({ name: "deckSettings", deckId: d.id })} />
                </div>
              </section>
            )
          )}
          <div className="deck__tiles">
            <StatTile
              label="아직 보지 않은 카드"
              value={n(d.unseen)}
              sub={d.finish_days ? `하루 ${n(d.per_day)}장씩 ${n(d.finish_days)}일` : d.unseen ? "오늘은 신규를 넣지 않아요" : d.unseen_empty ? "남은 카드는 앞면이 비어 있어요" : "모두 한 번씩 봤어요"}
            />
            <StatTile
              label="접어둔 카드"
              value={n(d.hard)}
              sub={
                d.hard === 0
                  ? "두 번 막히거나 직접 접은 카드"
                  : `자동 ${n(d.hard_auto)} · 직접 ${n(d.hard - d.hard_auto)} · ${
                      d.hard_next && d.hard_next > studyDate().toLocaleDateString("sv-SE") ? `${+d.hard_next.slice(5, 7)}/${+d.hard_next.slice(8, 10)}` : "오늘"
                    } 복귀`
              }
            />
            <StatTile label="밀린 복습" value={n(d.backlog)} sub="앱 전체 하루 200장까지" />
          </div>
        </div>
      </div>

      <section className="deck__importance">
        <span className="t-label-m">중요도</span>
        {([2, 1, 0] as const).map((lvl, i) => (
          <span key={lvl} className="deck__importance-item">
            <Importance level={lvl} />
            <span className="t-body-s c-secondary">{n(d.importance[i])}장</span>
          </span>
        ))}
        <span className="deck__importance-note t-body-s c-accent">
          {d.unconfirmed > 0
            ? `· 확인하지 않은 제안 ${d.unconfirmed}개`
            : d.importance[0] === 0 && d.importance[2] === 0
              ? "· 중요도 구분 없이 학습 중"
              : ""}
        </span>
        <Button variant="ghost" size="M" icon="chevron-right" label="중요도 조정" onClick={() => nav({ name: "importance", deckId: d.id })} />
      </section>

      {/* N4 기출 한 줄 (목표일과 상관없이, 표시가 없으면 숨긴다) */}
      <ExamLine deckId={d.id} />

      <WeakStrip d={d} />

      {d.tree.length > 0 && <SubdeckList d={d} reload={reload} />}
    </main>
  );
}

function Progress({ label, value, total, pct, color }: { label: string; value: number; total: number; pct: number; color: "ink" | "core" }) {
  return (
    <div className="progress">
      <div className="progress__row">
        <span className="t-body-s c-secondary">{label}</span>
        <span className="t-label-m">
          {n(value)} / {n(total)} · {pct}%
        </span>
      </div>
      <div className="progress__track">
        <span className={`progress__fill progress__fill--${color}`} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

// C1-w 약한 챕터. 중요도(신규를 넣는 순서)와 다른 축: 지금 얼마나 못 외우는가 → 복습을 보여주는 순서

const pct = (r: number) => `${Math.round(r * 100)}%`;
/** 기준을 넘었는가 (빨간 글자·막대). 백엔드 제안 기준과 같다: 복습 20번 이상 + 25% 이상 또는 덱 평균의 1.5배 이상 */
const overAgain = (s: Subdeck, avg: number | null) =>
  s.reviews14 >= 20 && s.again_rate !== null && (s.again_rate >= 0.25 || (avg !== null && s.again_rate >= 1.5 * avg));
/** 제안 근거 한 줄. "최근 14일 기억 못함(Again) 31% (덱 평균 17%) · 복습 48번 · 3번 이상 틀린 카드 6장" */
const evidence = (s: Subdeck, avg: number | null) =>
  [
    `최근 14일 기억 못함(Again) ${pct(s.again_rate ?? 0)}${avg !== null ? ` (덱 평균 ${pct(avg)})` : ""}`,
    `복습 ${n(s.reviews14)}번`,
    s.lapsed > 0 && `3번 이상 틀린 카드 ${n(s.lapsed)}장`,
  ]
    .filter(Boolean)
    .join(" · ");

/** 약함 · 약해 보여요? 배지. 제안 배지에 마우스를 올리거나 초점을 두면 근거 한 줄 */
function WeakBadge({ s, avg }: { s: Subdeck; avg: number | null }) {
  if (s.weak)
    return (
      <span className="weak weak--on t-label-s">
        <Icon name="alert" size={12} />
        취약
      </span>
    );
  const tip = `weak-tip-${s.id}`;
  return (
    <span className="weak weak--suggested t-label-s" tabIndex={0} aria-describedby={tip} onClick={(e) => e.stopPropagation()}>
      <Icon name="alert" size={12} />
      취약 챕터 제안
      <span className="weak__tip t-caption" role="tooltip" id={tip}>
        <span className="t-label-s">왜 제안했나요?</span>
        <span>{evidence(s, avg)}</span>
      </span>
    </span>
  );
}

/** 약함 표시 · 해제 · 아니에요. 바꾼 뒤 덱을 다시 읽는다 */
function useWeak(reload: Reload) {
  const toast = useToast();
  const run = (work: Promise<unknown>, done: Omit<ToastInput, "tone">) =>
    work.then(reload).then(
      () => toast(done),
      (e) => toast({ title: "바꾸지 못했어요", text: errorText(e), tone: "error" }),
    );
  return {
    mark: (s: Subdeck) => run(api.setDeckWeak(s.id, true), { title: "취약 챕터로 지정했어요", text: "이 챕터의 예정 복습이 먼저 나와요" }),
    unmark: (s: Subdeck) => run(api.setDeckWeak(s.id, false), { title: "취약 챕터 지정을 풀었어요", text: s.name }),
    dismiss: (s: Subdeck) => run(api.dismissWeak(s.id), { title: "14일 동안 이 제안을 숨길게요", text: s.name }),
    keep: (s: Subdeck) => run(api.dismissWeak(s.id, true), { title: "취약 챕터 지정을 그대로 둘게요", text: s.name }),
  };
}

/** 하위 덱 id → 이 덱 아래 상대 경로 ("3과목 · 4장 분석 모형") */
const relLabels = (d: DeckDetail) => {
  const paths = new Map<number, string[]>([[d.id, []]]);
  for (const s of d.tree) paths.set(s.id, [...(paths.get(s.parent_id) ?? []), s.name]);
  return (id: number) => paths.get(id)?.join(" · ") ?? "";
};

/** C1-w 중요도 띠 아래 "약한 챕터" 줄. 확정·제안된 챕터가 있을 때만 */
function WeakStrip({ d }: { d: DeckDetail }) {
  const nav = useNav();
  const weak = d.tree.filter((s) => s.weak);
  const suggested = d.tree.filter((s) => s.weak_suggested && !s.weak);
  if (!weak.length && !suggested.length) return null;
  // 줄이 한 줄이라 하위 덱 이름만. 전체 경로는 title 로
  const label = relLabels(d);
  return (
    <section className="deck__weak" aria-label="취약 챕터">
      <span className="t-label-m">취약 챕터</span>
      {weak.length > 0 && (
        <>
          <span className="weak weak--on t-label-s">
            <Icon name="alert" size={12} />
            지정
          </span>
          <span className="deck__weak-names t-body-s c-secondary" title={weak.map((s) => label(s.id)).join(", ")}>
            {weak.map((s) => `${s.name} · ${n(s.cards)}장`).join(", ")}
          </span>
        </>
      )}
      {suggested.length > 0 && (
        <>
          <span className="weak weak--suggested t-label-s">
            <Icon name="alert" size={12} />
            제안
          </span>
          <span className="deck__weak-names t-body-s c-secondary" title={suggested.map((s) => label(s.id)).join(", ")}>
            {suggested.map((s) => `${s.name} · 확인 전`).join(", ")}
          </span>
        </>
      )}
      <span className="deck__weak-gap" />
      {weak.length > 0 && (
        <Button
          variant="ghost"
          size="M"
          icon="chevron-right"
          label="취약 챕터 카드 보기"
          onClick={() => nav({ name: "cards", deckId: d.id, weakOnly: true })}
        />
      )}
    </section>
  );
}

// ---------------------------------------------------------------- C1-t 하위 덱 목록 (+ C1-w 최근 14일 Again)

function SubdeckList({ d, reload }: { d: DeckDetail; reload: Reload }) {
  const nav = useNav();
  const weakAct = useWeak(reload);
  const [open, setOpen] = useState<Set<number>>(() => new Set()); // 처음엔 모두 접힘
  const [creating, setCreating] = useState(false);
  const toggle = (id: number) =>
    setOpen((o) => {
      const next = new Set(o);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  // 과목(하위 덱을 가진 덱)은 자기 카드가 없어 자기 중요도가 의미 없다. 섞이지 않았으면 바로 아래 첫 덱의 값을 보여준다.
  // 깊이 우선이라 i+1 이 첫 자식이고, 뒤에서부터 채우면 이미 계산돼 있다
  const level = new Map<number, Level>();
  for (let i = d.tree.length - 1; i >= 0; i--) {
    const s = d.tree[i];
    const first = s.children > 0 ? level.get(d.tree[i + 1]?.id) : undefined;
    level.set(s.id, first ?? s.importance);
  }
  // 깊이 우선 순서라 부모가 늘 먼저 나온다. 바로 아래이거나, 보이는 부모가 펼쳐졌을 때만 보인다
  const shown = new Set<number>();
  const rows = d.tree.filter((s) => {
    const ok = s.parent_id === d.id || (shown.has(s.parent_id) && open.has(s.parent_id));
    if (ok) shown.add(s.id);
    return ok;
  });
  const stop = (work: () => unknown) => (e: { stopPropagation: () => void }) => {
    e.stopPropagation(); // 행을 누르면 그 하위 덱으로 가므로
    work();
  };

  return (
    <section className="subdecks">
      <div className="subdecks__head">
        <span className="subdecks__title">
          <span className="t-title-s">하위 덱</span>
          <Badge>{n(d.subdeck_count)}</Badge>
        </span>
        <Button variant="ghost" size="M" icon="plus" label="하위 덱 만들기" onClick={() => setCreating(true)} />
      </div>
      <div className="subdecks__row subdecks__row--head t-label-s c-tertiary">
        <span>이름</span>
        <span>중요도</span>
        <span>최근 14일 Again 비율</span>
        <span>{d.done_today > 0 ? "오늘 남은" : "오늘"}</span>
        <span />
      </div>
      {rows.map((s) => {
        const go = () => nav({ name: "deck", deckId: s.id });
        const suggested = s.weak_suggested && !s.weak;
        const over = overAgain(s, d.again_avg);
        const study = s.today > 0 && (
          <Button variant="secondary" size="M" label="학습" aria-label={`${s.name} 학습`} onClick={stop(() => nav({ name: "summary", deckIds: [s.id] }))} />
        );
        return (
          <div
            key={s.id}
            className={`subdecks__row ${s.today === 0 ? "is-idle" : ""} ${suggested ? "is-suggested" : ""}`}
            style={{ paddingLeft: 24 * s.depth }}
            role="button"
            tabIndex={0}
            aria-label={`${s.name}${s.weak ? " · 취약 챕터" : suggested ? " · 취약 챕터 제안" : ""} · 하위 덱 열기`}
            onClick={go}
            onKeyDown={(e) => {
              // preventDefault: 화면의 Enter(학습 시작)가 같은 키를 또 받지 않게
              if (e.key === "Enter" && e.target === e.currentTarget) {
                e.preventDefault();
                go();
              }
            }}
          >
            <span className="subdecks__name">
              {s.children > 0 ? (
                <button
                  className="subdecks__fold"
                  aria-label={`${s.name} ${open.has(s.id) ? "접기" : "펼치기"}`}
                  title={open.has(s.id) ? "접기" : "펼치기"}
                  aria-expanded={open.has(s.id)}
                  onClick={stop(() => toggle(s.id))}
                >
                  <Icon name={open.has(s.id) ? "chevron-down" : "chevron-right"} size={16} scaled />
                </button>
              ) : (
                <span className="subdecks__fold" />
              )}
              <span className={s.children > 0 ? "t-body-m-strong" : "t-body-m"}>{s.name}</span>
              {s.children > 0 && <span className="t-caption c-tertiary">하위 {n(s.children)}개</span>}
              {s.paused && <span className="t-caption c-tertiary">쉬는 중</span>}
              {(s.weak || suggested) && <WeakBadge s={s} avg={d.again_avg} />}
              {/* 확정했는데 덱 평균 아래로 내려왔으면 한 번만 묻는다. 해제는 오른쪽 [해제] */}
              {s.weak && s.weak_recovered && (
                <span className="subdecks__recovered t-caption c-secondary">
                  요즘 괜찮아요 · 해제할까요?
                  <button className="subdecks__recovered-close" aria-label="그대로 둘게요" title="그대로 둘게요" onClick={stop(() => weakAct.keep(s))}>
                    <Icon name="x" size={14} />
                  </button>
                </span>
              )}
            </span>
            <span>{s.mixed ? <Badge>혼합</Badge> : <Importance level={level.get(s.id)!} />}</span>
            {s.again_rate === null || s.reviews14 < 20 ? (
              <span className="t-caption c-tertiary">기록 적음</span>
            ) : (
              <span className="subdecks__again">
                <span className={`t-body-s ${over ? "c-danger" : "c-secondary"}`}>{pct(s.again_rate)}</span>
                <span className="subdecks__again-track" aria-hidden>
                  <span className={`subdecks__again-fill ${over ? "is-over" : ""}`} style={{ width: `${Math.min(s.again_rate, 1) * 100}%` }} />
                </span>
              </span>
            )}
            <span className="subdecks__today t-body-m-strong">{n(s.today)}</span>
            <span className="subdecks__go">
              {suggested ? (
                <>
                  <Button variant="secondary" size="M" icon="alert" label="취약 챕터로 지정" aria-label={`${s.name} 취약 챕터로 지정`} onClick={stop(() => weakAct.mark(s))} />
                  <Button variant="ghost" size="M" label="제안 숨기기" aria-label={`${s.name} 취약 챕터 제안 숨기기`} onClick={stop(() => weakAct.dismiss(s))} />
                </>
              ) : (
                <>
                  {s.weak ? (
                    <Button variant="ghost" size="M" label="해제" aria-label={`${s.name} 취약 챕터 해제`} onClick={stop(() => weakAct.unmark(s))} />
                  ) : (
                    // 아무 행에나 마우스를 올리면 직접 표시할 수 있다 (키보드는 초점이 오면 보인다)
                    <Button
                      className="subdecks__mark"
                      variant="ghost"
                      size="M"
                      label="취약 챕터 지정"
                      aria-label={`${s.name} 취약 챕터로 지정`}
                      onClick={stop(() => weakAct.mark(s))}
                    />
                  )}
                  {study}
                  {!(s.weak && study) && <Icon name="chevron-right" size={16} className="c-tertiary" />}
                </>
              )}
            </span>
          </div>
        );
      })}
      {creating && <NewDeckModal parentId={d.id} onClose={() => setCreating(false)} />}
    </section>
  );
}

// ---------------------------------------------------------------- C1-m · C1-r ⋯ 메뉴

/** C1-r 쉬는 기간 프리셋. 오늘부터 days 일째 되는 날까지 (그날 포함) */
const PAUSE_PRESETS = [
  { label: "3일", days: 3 },
  { label: "1주", days: 7 },
  { label: "2주", days: 14 },
];

function MoreMenu({ d, onRename, onPause, onResume }: {
  d: DeckDetail;
  onRename: () => void;
  onPause: (until: string | null) => void;
  onResume: () => void;
}) {
  const nav = useNav();
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [pauseOpen, setPauseOpen] = useState(false);
  const [pauseByKey, setPauseByKey] = useState(false); // 키보드로 열었으면 하위 메뉴 첫 항목에 초점
  const trigger = useRef<HTMLButtonElement>(null);
  const pauseTrigger = useRef<HTMLButtonElement>(null);
  const [dialog, setDialog] = useState<"subdeck" | "export" | "delete" | null>(null);
  // 날짜 고르기는 메뉴가 닫혀도 달력이 떠 있어야 하므로 입력은 메뉴 밖에 둔다
  const dateInput = useRef<HTMLInputElement>(null);
  const today = iso(studyDate());
  // Esc 로 닫으면 초점을 ⋯ 버튼으로 돌려준다 (사라진 메뉴 항목에 초점이 남지 않게)
  // 메뉴가 떠 있는 동안 화면 단축키는 꺼지고 메뉴가 키를 받는다 (E4 노트)
  useOverlayScope(open);
  useKey(
    "Escape",
    () => {
      setOpen(false);
      trigger.current?.focus();
    },
    open,
    { modal: true },
  );

  useEffect(() => {
    if (!open) return setPauseOpen(false);
    const close = () => setOpen(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);

  // 확인 창 없이 바로 보관하고, 5초 동안 되돌릴 수 있게 한다
  const archive = async () => {
    try {
      await api.archiveDeck(d.id);
    } catch (e) {
      return toast({ title: "보관하지 못했어요", text: errorText(e), tone: "error" });
    }
    toast({
      title: "보관했어요",
      text: d.name,
      tone: "undo",
      action: {
        label: "되돌리기",
        // 홈 목록은 다시 읽지 않으므로 되살린 덱으로 돌아간다
        onClick: () =>
          api.unarchiveDeck(d.id).then(
            () => nav({ name: "deck", deckId: d.id }),
            (e) => toast({ title: "되돌리지 못했어요", text: errorText(e), tone: "error" }),
          ),
      },
    });
    nav({ name: "home" });
  };

  const deleting = [d.subdeck_count > 0 && `하위 덱 ${n(d.subdeck_count)}개`, `카드 ${n(d.total)}장`].filter(Boolean).join(" · ");

  return (
    <div className="menu">
      <button
        ref={trigger}
        className="iconbtn"
        aria-label="덱 관리"
        title="덱 관리"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => {
          e.stopPropagation();
          setOpen(!open);
        }}
      >
        <Icon name="more" size={18} className="c-secondary" />
      </button>
      <input
        ref={dateInput}
        className="deck__date-input"
        type="date"
        tabIndex={-1}
        aria-label="쉬는 마지막 날"
        min={today}
        onChange={(e) => {
          const until = e.target.value;
          e.target.value = ""; // 다음에 같은 날을 골라도 change 가 오게
          if (until) onPause(until);
        }}
      />
      {open && (
        <div className="menu__list menu--panel deck__menu" role="menu" aria-label="덱 관리" onKeyDown={menuKeys}>
          {/* 열리면 첫 항목에 초점. 마우스로 열었을 때는 초점 링이 보이지 않는다 (:focus-visible) */}
          <button role="menuitem" className="menu__item t-body-m" autoFocus onClick={() => setDialog("subdeck")}>
            <Icon name="plus" size={16} scaled />
            <span className="menu__text">하위 덱 만들기</span>
          </button>
          <button role="menuitem" className="menu__item t-body-m" onClick={onRename}>
            <Icon name="pencil" size={16} scaled />
            <span className="menu__text">이름 바꾸기</span>
            <Kbd k="F2" />
          </button>
          {/* G1. 하위 덱까지 규칙으로 검사한다 */}
          <button role="menuitem" className="menu__item t-body-m" disabled={d.total === 0} onClick={() => nav({ name: "quality", deckId: d.id })}>
            <Icon name="layers" size={16} />
            <span className="menu__text">카드 점검</span>
          </button>
          {d.paused ? (
            <button role="menuitem" className="menu__item t-body-m" onClick={onResume}>
              <Icon name="rotate" size={16} scaled />
              <span className="menu__text">다시 시작</span>
            </button>
          ) : (
            <div className="deck__pause" onMouseEnter={() => setPauseOpen(true)} onMouseLeave={() => setPauseOpen(false)}>
              <button
                ref={pauseTrigger}
                role="menuitem"
                aria-haspopup="menu"
                aria-expanded={pauseOpen}
                className="menu__item t-body-m"
                onClick={(e) => {
                  e.stopPropagation();
                  setPauseByKey(false);
                  setPauseOpen((v) => !v);
                }}
                onKeyDown={(e) => {
                  if (e.key !== "Enter" && e.key !== " " && e.key !== "ArrowRight") return;
                  e.preventDefault();
                  setPauseByKey(true);
                  setPauseOpen(true);
                }}
              >
                <Icon name="clock" size={16} scaled />
                <span className="menu__text">잠시 쉬기</span>
                <Icon name="chevron-right" size={14} className="c-tertiary" />
              </button>
              {pauseOpen && (
                <PauseMenu
                  d={d}
                  today={today}
                  focusFirst={pauseByKey}
                  onBack={() => {
                    setPauseOpen(false);
                    pauseTrigger.current?.focus();
                  }}
                  onPick={(until) => {
                    setOpen(false);
                    onPause(until);
                  }}
                  onPickDate={() => {
                    setOpen(false);
                    dateInput.current?.showPicker();
                  }}
                />
              )}
            </div>
          )}
          {/* C1-x. 하위 덱 페이지면 그 하위 덱만 내보낸다 */}
          <button role="menuitem" className="menu__item menu__item--top" disabled={d.total === 0} onClick={() => setDialog("export")}>
            <Icon name="file" size={16} scaled />
            <span className="menu__text">
              <span className="t-body-m">내보내기</span>
              <span className="t-caption c-tertiary">{d.total === 0 ? "내보낼 카드가 없어요" : ".apkg 또는 CSV"}</span>
            </span>
          </button>
          <button role="menuitem" className="menu__item menu__item--top" onClick={archive}>
            <Icon name="archive" size={16} scaled />
            <span className="menu__text">
              <span className="t-body-m">보관</span>
              <span className="t-caption c-tertiary">홈과 오늘의 학습에서 빠져요</span>
            </span>
          </button>
          <div className="menu__sep" role="separator" />
          <button role="menuitem" className="menu__item menu__item--danger menu__item--top" onClick={() => setDialog("delete")}>
            <Icon name="trash" size={16} />
            <span className="menu__text">
              <span className="t-body-m">덱 삭제</span>
              <span className="t-caption c-tertiary">{deleting}</span>
            </span>
          </button>
        </div>
      )}
      {dialog === "subdeck" && <NewDeckModal parentId={d.id} onClose={() => setDialog(null)} />}
      {dialog === "export" && <ExportDeckDialog deckId={d.id} onClose={() => setDialog(null)} />}
      {dialog === "delete" && (
        <DeleteDeckConfirm
          id={d.id}
          name={d.name}
          counts={deleting}
          onClose={() => setDialog(null)}
          onDone={() => {
            toast({ title: "덱을 지웠어요", text: d.name });
            nav({ name: "home" });
          }}
        />
      )}
    </div>
  );
}

/** C1-r 잠시 쉬기 하위 메뉴. 올린 항목에 맞춰 맨 아래 안내가 바뀐다 (목표일은 그대로 + 다시 시작한 뒤 하루 분량) */
function PauseMenu({ d, today, focusFirst, onBack, onPick, onPickDate }: {
  d: DeckDetail;
  today: string;
  /** 키보드로 열었으면 첫 항목에 초점 */
  focusFirst: boolean;
  /** ← 로 하위 메뉴를 닫고 "잠시 쉬기"로 돌아간다 */
  onBack: () => void;
  onPick: (until: string | null) => void;
  onPickDate: () => void;
}) {
  const [active, setActive] = useState<number | "forever" | "date">(0);
  const [perDay, setPerDay] = useState<(number | null)[]>([]); // 프리셋마다 다시 시작한 뒤 하루 신규 (C4-p 와 같은 계산)
  const target = d.target_date;

  useEffect(() => {
    const config = {
      deck_id: d.id,
      target_date: d.target_date,
      include_target_day: d.include_target_day,
      daily_new_limit: d.daily_new_limit,
      today_new: d.today_new_override,
    };
    Promise.all(
      PAUSE_PRESETS.map((p) =>
        api.previewDeckConfig({ ...config, paused: true, paused_until: addDays(today, p.days - 1) }).then(
          (r) => r.per_day,
          () => null,
        ),
      ),
    ).then(setPerDay);
  }, [d, today]);

  // 목표일을 넘기는 기간이면 그 사실을, 아니면 올린 항목에만 다시 시작하는 날을 붙인다
  const caption = (until: string, on: boolean) => {
    const tail =
      target && until > target
        ? until === addDays(target, 1)
          ? " · 목표일 다음 날"
          : " · 목표일 지나서"
        : on
          ? ` · ${parse(addDays(until, 1)).getDate()}일부터 다시`
          : "";
    return `${dayTight(until)}까지${tail}`;
  };
  const keep = target ? `목표일 ${monthDay(target)}은 그대로예요. ` : "";
  const hint = (() => {
    if (active === "forever") return `${keep}다시 시작할 때까지 신규 카드가 들어오지 않아요.`;
    if (active === "date") return `${keep}달력에서 마지막으로 쉬는 날을 골라요.`;
    const after = perDay[active];
    const change =
      after == null
        ? " 그동안 신규 카드가 들어오지 않아요."
        : after === d.per_day
          ? ` 다시 시작한 뒤 하루 신규 ${n(after)}장 그대로.`
          : ` 다시 시작한 뒤 하루 신규 ${n(d.per_day)} → ${n(after)}장.`;
    return `${keep}${PAUSE_PRESETS[active].label} 쉬면${change}`;
  })();

  return (
    // 오른쪽 여백은 메뉴와 하위 메뉴 사이의 다리. 마우스가 건너가는 동안 닫히지 않게
    <div className="deck__submenu" onClick={(e) => e.stopPropagation()}>
      <div
        className="menu__list menu--panel"
        role="menu"
        aria-label="얼마나 쉴까요?"
        onKeyDown={(e) => {
          menuKeys(e);
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            e.stopPropagation();
            onBack();
          }
        }}
      >
        <p className="menu__cap t-caption c-tertiary" aria-hidden>
          얼마나 쉴까요?
        </p>
        {PAUSE_PRESETS.map((p, i) => {
          const until = addDays(today, p.days - 1);
          return (
            <button
              key={p.label}
              role="menuitem"
              className={`menu__item ${active === i ? "is-active" : ""}`}
              autoFocus={focusFirst && i === 0}
              onMouseEnter={() => setActive(i)}
              onFocus={() => setActive(i)}
              onClick={() => onPick(until)}
            >
              <span className="menu__text">
                <span className="t-body-m">{p.label}</span>
                <span className="t-caption c-tertiary">{caption(until, active === i)}</span>
              </span>
            </button>
          );
        })}
        <button
          role="menuitem"
          className={`menu__item t-body-m ${active === "forever" ? "is-active" : ""}`}
          onMouseEnter={() => setActive("forever")}
          onFocus={() => setActive("forever")}
          onClick={() => onPick(null)}
        >
          <span className="menu__text">다시 시작할 때까지</span>
        </button>
        <div className="menu__sep" role="separator" />
        <button
          role="menuitem"
          className={`menu__item t-body-m ${active === "date" ? "is-active" : ""}`}
          onMouseEnter={() => setActive("date")}
          onFocus={() => setActive("date")}
          onClick={onPickDate}
        >
          <Icon name="calendar" size={16} scaled />
          <span className="menu__text">날짜 고르기…</span>
        </button>
        <div className="deck__pause-hint" aria-live="polite">
          <Icon name="info" size={14} className="c-tertiary" />
          <p className="t-caption c-secondary">{hint}</p>
        </div>
      </div>
    </div>
  );
}

/**
 * C1-m · E3 덱 삭제 확인. E1-b 와 같은 무게: 사라지는 것 숫자 + 백업(기본 켜짐) + 덱 이름 입력.
 * counts 는 "하위 덱 2개 · 카드 120장" 처럼 '장'으로 끝나는 말. 토스트와 이동은 부르는 쪽이 정한다
 */
export function DeleteDeckConfirm({ id, name, counts, onClose, onDone }: {
  id: number;
  name: string;
  counts: string;
  onClose: () => void;
  onDone: () => void;
}) {
  const [backup, setBackup] = useState(true);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const ok = typed.trim() === name.trim();
  const sync = useSyncStatus();
  const synced = !!sync?.connected && !!sync.enabled;

  const run = async () => {
    if (!ok || busy) return;
    setBusy(true);
    try {
      if (backup) await api.createBackup("deck");
      await api.deleteDeck(id);
      onDone();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal title="덱을 지울까요?" onClose={onClose} variant="confirm">
      <p className="t-body-m c-secondary">
        「{name}」의 {counts}이 {synced ? "이 기기에서 사라지고, 다음 동기화 때 다른 기기에서도 지워져요" : "이 기기에서 사라져요"}. 되돌릴 수 없어요. 복습 기록은 학습 통계에만 남아요.
      </p>
      <Checkbox checked={backup} label="지우기 전에 백업 파일 만들기 (권장)" onChange={setBackup} />
      <Input
        label="확인을 위해 덱 이름을 그대로 입력해 주세요"
        placeholder={name}
        value={typed}
        autoFocus
        onChange={(e) => {
          setTyped(e.target.value);
          setError(undefined);
        }}
        onKeyDown={(e) => plainEnter(e) && run()}
        error={error}
      />
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
        <Button
          variant="danger"
          size="M"
          icon="trash"
          label={backup ? "백업 후 삭제" : "삭제"}
          busy={busy}
          busyLabel="지우는 중…"
          disabled={!ok}
          onClick={run}
        />
      </div>
    </Modal>
  );
}
