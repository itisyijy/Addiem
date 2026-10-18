// A1 빈 홈 / A2 사용 중인 홈 / A2-b 오늘 학습 끝 / A2-p 쉬는 덱이 있을 때
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { api, errorText, type Home as HomeData } from "../api";
import { Badge, Button, DeckRow, Icon, Kbd } from "../ui";
import { useToast } from "../toast";
import { actionKey, useKeymap } from "../keys";
import { CramHint } from "./StudyParts";
import { PlanConflictBanner } from "./Plan";
import { addDays } from "./Deck";
import { dateLabel, n, useKey, useNav } from "../nav";
import { DriveLoadLink, useSyncStatus } from "../sync";
import { RESTORED_KEY } from "./Settings";

// 백업으로 복구한 뒤 다시 불러왔으면 그 백업의 시각 (페이지를 불러올 때 한 번 읽고 지운다, DU49)
let restored = (() => {
  try {
    const v = sessionStorage.getItem(RESTORED_KEY);
    sessionStorage.removeItem(RESTORED_KEY);
    return v;
  } catch {
    return null;
  }
})();

export function Home() {
  const toast = useToast();
  const nav = useNav();
  const [data, setData] = useState<HomeData | null>(null);
  const [error, setError] = useState<string>();
  const reload = useCallback(() => api.home().then(setData, (e) => setError(errorText(e))), []);
  useEffect(() => {
    api.home().then(
      (d) => {
        setData(d);
        if (restored) {
          toast({
            title: `${restored} 백업으로 복구했어요`,
            text: "복구 전 상태도 백업해 뒀어요",
            action: { label: "설정 › 이 기기 백업", onClick: () => nav({ name: "settings" }) },
            ms: 5000,
          });
          restored = null;
        }
        // 쉬기 날짜가 지나 오늘 다시 시작한 덱
        else if (d.resumed.length)
          toast({
            title: d.resumed.length === 1 ? `${d.resumed[0]} 쉬기를 마쳤어요` : `덱 ${d.resumed.length}개의 쉬기를 마쳤어요`,
            text: "오늘 분량에 다시 들어가요",
          });
      },
      (e) => setError(errorText(e)),
    );
  }, [toast, nav]);
  // K Drive 받기로 데이터가 바뀌면 다시 읽는다 (다른 기기에서 만든 덱이 바로 보이게)
  const changed = useSyncStatus()?.changed_seq ?? 0;
  useEffect(() => {
    if (changed) reload();
  }, [changed, reload]);
  if (!data) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;
  if (!data.has_decks) return <EmptyHome />;
  return data.review + data.new > 0 ? <ActiveHome data={data} reload={reload} /> : <DoneHome data={data} reload={reload} />;
}

// ---------------------------------------------------------------- A1

function EmptyHome() {
  const nav = useNav(); // N · I 단축키는 App 에서 전역으로 건다
  useKeymap(); // 설정(E4)에서 키를 바꾸면 칩도 바로 바뀌게
  const synced = !!useSyncStatus()?.enabled;
  return (
    <main className="a1">
      <span className="pill t-label-s c-secondary">
        <span className="dot dot--local" />
        {synced ? "이 기기에 저장 · Google Drive 동기화 사용 중" : "기본 저장 위치는 이 기기예요"}
      </span>
      <h1 className="t-display-l">아직 학습할 덱이 없어요</h1>
      <p className="t-body-l c-secondary">
        덱은 학습 카드를 모아 두는 묶음이에요. 과목이나 시험별로 새로 만들거나, 쓰던 Anki 덱을 가져오면 바로 시작할 수 있어요.
      </p>
      <div className="a1__choices">
        <button className="a1__card a1__card--primary" onClick={() => nav({ name: "newDeck" })}>
          <span className="a1__badge a1__badge--accent c-inverse">
            <Icon name="layers" />
          </span>
          <span className="a1__spacer" />
          <span className="t-title-m">새 덱 만들기</span>
          <span className="t-body-s c-secondary">이름만 정하면 바로 카드를 넣을 수 있어요.</span>
          <span className="a1__foot">
            <Kbd k={actionKey("newDeck")} />
            <Icon name="arrow-right" size={20} />
          </span>
        </button>
        <button className="a1__card" onClick={() => nav({ name: "import" })}>
          <span className="a1__badge a1__badge--subtle c-accent">
            <Icon name="upload" />
          </span>
          <span className="a1__spacer" />
          <span className="t-title-m">가져오기</span>
          <span className="t-body-s c-secondary">Anki 덱(.apkg · .colpkg) 또는 JSON 파일</span>
          <span className="a1__foot">
            <Kbd k={actionKey("import")} />
            <Icon name="arrow-right" size={20} />
          </span>
        </button>
      </div>
      {/* K6 두 번째 기기의 입구. 두 카드와 같은 무게로 두지 않는다 */}
      <DriveLoadLink />
    </main>
  );
}

// ---------------------------------------------------------------- A2

function ActiveHome({ data, reload }: { data: HomeData; reload: () => void }) {
  const nav = useNav();
  const total = data.review + data.new;
  const start = useCallback(() => nav({ name: "summary", deckIds: null }), [nav]);
  useKey("Enter", start);
  const barWidth = 420 - 3; // 두 막대 사이 간격 3px
  const reviewWidth = Math.round((data.review / total) * barWidth);

  return (
    <main className="home">
      <section className="home__today">
        <DatePill />
        <div className="home__gap" />
        <p className="t-display-l c-tertiary">오늘 남은 카드</p>
        <BigCount value={total} />
        <div className="home__mix">
          <div className="home__bar">
            {data.review > 0 && <span className="home__bar-review" style={{ width: reviewWidth }} />}
            {data.new > 0 && <span className="home__bar-new" style={{ width: barWidth - reviewWidth }} />}
          </div>
          <div className="home__legend t-body-m">
            <span className="c-secondary">
              <span className="dot dot--ink" /> 복습 {n(data.review)}
            </span>
            <span className="c-secondary">
              <span className="dot dot--accent" /> 신규 {n(data.new)}
            </span>
            <span className="c-tertiary">·&nbsp;&nbsp;완료 {n(data.done_today)}</span>
          </div>
        </div>
        <div className="home__cta">
          <Button icon="arrow-right" label="오늘 학습 시작" shortcut="Enter" onClick={start} />
        </div>
        {/* H3 목표일 0–3일 전에만 보인다 */}
        <div className="home__cram">
          <CramHint deckIds={null} dismissible />
        </div>
      </section>

      <div className="home__side">
        {/* H2 목표일 충돌: 아직 고르지 않은 충돌이 있을 때만 */}
        <PlanConflictBanner />
        <DeckPanel title="덱별 오늘 학습량" caption="최상위 덱 기준 · 하위 덱 합산" data={data} mode="today" reload={reload} />
      </div>
    </main>
  );
}

// ---------------------------------------------------------------- A2-b

function DoneHome({ data, reload }: { data: HomeData; reload: () => void }) {
  const nav = useNav();
  const firstDeck = data.decks[0]?.id ?? data.idle_decks[0]?.id;
  return (
    <main className="home">
      <section className="home__today">
        <DatePill />
        <div className="home__gap" />
        <p className="t-display-l c-tertiary">오늘 남은 카드</p>
        <BigCount value={0} />
        <p className="home__done-line t-body-l c-secondary">
          {data.done_today > 0 ? `${n(data.done_today)}장을 모두 마쳤어요. ` : "오늘 예정된 카드가 없어요. "}
          {data.tomorrow > 0 ? `내일은 복습 ${n(data.tomorrow)}장이 예정돼 있어요.` : "내일 예정된 복습도 없어요."}
        </p>
        <div className="home__actions">
          {firstDeck !== undefined && (
            <Button variant="secondary" icon="plus" label="카드 추가" onClick={() => nav({ name: "addCard", deckId: firstDeck })} />
          )}
          {data.buried > 0 && firstDeck !== undefined && (
            <Button
              variant="ghost"
              icon="archive"
              label={`접어둔 카드 ${n(data.buried)}장 보기`}
              onClick={() => nav({ name: "cards", deckId: null, buriedOnly: true })}
            />
          )}
        </div>
        <div className="home__cram">
          <CramHint deckIds={null} dismissible />
        </div>
      </section>

      <div className="home__side">
        <PlanConflictBanner />
        <DeckPanel title="내 덱" caption="오늘 남은 카드" data={data} mode="done" reload={reload} />
      </div>
    </main>
  );
}

// ---------------------------------------------------------------- 공통

function DatePill() {
  return (
    <span className="pill t-label-s c-secondary">
      <span className="dot dot--accent" />
      {dateLabel()}
    </span>
  );
}

function BigCount({ value }: { value: number }) {
  return (
    <p className="home__count">
      <span className="home__count-num">{n(value)}</span>
      <span className="t-display-l">장</span>
    </p>
  );
}

const md = (iso: string) => `${+iso.slice(5, 7)}/${+iso.slice(8, 10)}`;

type PanelProps = { title: string; caption: string; data: HomeData; mode: "today" | "done"; reload: () => void };

function DeckPanel({ title, caption, data, mode, reload }: PanelProps) {
  const nav = useNav();
  const toast = useToast();
  const badge = (dday: number | null) => (dday === null || dday < 0 ? undefined : dday === 0 ? "D-day" : `D-${dday}`);
  // A2-p: 오늘 몫을 끝낸 덱도 "오늘 볼 카드가 없는 덱" 쪽에 한 줄로 둔다
  const finished = mode === "today" ? data.decks.filter((d) => d.review + d.new === 0) : [];
  const active = mode === "today" ? data.decks.filter((d) => d.review + d.new > 0) : data.decks;
  const paused = data.idle_decks.filter((d) => d.paused);
  const idleCount = data.idle_decks.length + finished.length;
  const [showIdle, setShowIdle] = useState(paused.length > 0);
  const [resuming, setResuming] = useState<number>();

  const resume = async (id: number, name: string) => {
    setResuming(id);
    try {
      await api.setDeckPaused(id, false);
      toast({ title: `${name} 덱을 다시 시작했어요`, text: "오늘 분량에 바로 들어가요" });
      reload();
    } catch (e) {
      toast({ title: "다시 시작하지 못했어요", text: errorText(e), tone: "error" });
    } finally {
      setResuming(undefined);
    }
  };
  const open = (id: number) => () => nav({ name: "deck", deckId: id });

  return (
    <section className="panel home__decks">
      <header className="panel__head">
        <span className="t-title-s">{title}</span>
        <span className="home__head-right">
          <span className="t-caption c-tertiary">{caption}</span>
          {/* H1 모든 덱의 학습 계획 */}
          <button className="home__plan-link t-caption c-secondary" onClick={() => nav({ name: "plan", deckId: null })}>
            계획
            <Icon name="chevron-right" size={14} scaled />
          </button>
        </span>
      </header>
      {active.map((d) => (
        <DeckRow
          key={d.id}
          name={d.name}
          badge={badge(d.dday)}
          counts={mode === "today" ? `복습 ${n(d.review)} · 신규 ${n(d.new)}` : `완료 · 내일 복습 ${n(d.tomorrow)}`}
          total={d.review + d.new}
          onStudy={mode === "today" ? () => nav({ name: "summary", deckIds: [d.id] }) : undefined}
          onOpen={open(d.id)}
        />
      ))}
      {idleCount > 0 && (
        <>
          <button className="home__idle t-body-s c-tertiary" onClick={() => setShowIdle(!showIdle)} aria-expanded={showIdle}>
            오늘 볼 카드가 없는 덱 {idleCount}개
            <Icon name="chevron-down" size={16} className={showIdle ? "is-flipped" : ""} />
          </button>
          {showIdle && (
            <>
              {paused.map((d) => (
                <IdleRow key={d.id} name={d.name} onOpen={open(d.id)}>
                  <Badge>{d.paused_until ? `쉬는 중 · ${md(d.paused_until)}까지 · ${md(addDays(d.paused_until, 1))}부터 다시` : "쉬는 중"}</Badge>
                  <span className="home__idle-fill" />
                  <Button
                    variant="ghost"
                    size="M"
                    icon="rotate"
                    label="다시 시작"
                    busy={resuming === d.id}
                    onClick={(e) => {
                      e.stopPropagation();
                      resume(d.id, d.name);
                    }}
                  />
                </IdleRow>
              ))}
              {finished.map((d) => (
                <IdleRow key={d.id} name={d.name} onOpen={open(d.id)}>
                  <span className="home__idle-fill" />
                  <span className="t-caption c-tertiary">{d.tomorrow > 0 ? `오늘 끝 · 내일 복습 ${n(d.tomorrow)}` : "오늘 끝"}</span>
                </IdleRow>
              ))}
              {data.idle_decks
                .filter((d) => !d.paused)
                .map((d) => (
                  <IdleRow key={d.id} name={d.name} onOpen={open(d.id)}>
                    <span className="home__idle-fill" />
                    <span className="t-caption c-tertiary">{d.next_due ? `예정 없음 · 다음 복습 ${md(d.next_due)}` : "예정 없음"}</span>
                  </IdleRow>
                ))}
            </>
          )}
        </>
      )}
      {data.archived > 0 && (
        <button className="home__archived t-caption c-tertiary" onClick={() => nav({ name: "settings", focus: "archived" })}>
          <Icon name="archive" size={14} scaled />
          보관한 덱 {n(data.archived)}
          <Icon name="chevron-right" size={14} scaled />
        </button>
      )}
    </section>
  );
}

function IdleRow({ name, onOpen, children }: { name: string; onOpen: () => void; children: ReactNode }) {
  return (
    <div
      className="home__idle-row"
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => e.key === "Enter" && e.target === e.currentTarget && onOpen()}
    >
      <span className="t-body-m c-secondary">{name}</span>
      {children}
    </div>
  );
}
