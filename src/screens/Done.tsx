// D5 학습 완료 / D5-b 중간에 그만둔 경우 / D5-c 접어둔 카드 없음 / D5-d 중간에 그만둠 + 접어둔 카드 없음
import { useEffect, useRef, useState } from "react";
import { api, errorText, type SessionSummary } from "../api";
import { Badge, Button, Icon, Importance, StatTile } from "../ui";
import { getCutoff, hourLabel, n, useKey, useNav } from "../nav";
import { actionKey } from "../keys";
import { useToast } from "../toast";
import { SplitLaterChip } from "./StudyParts";
import { BURY_DAYS, buryReturnDay } from "./Deck";
import { SplitDialog } from "./Quality";
import "../styles/done.css";

export function Done({ sessionId, summary: s, completed, deckIds, scale }: {
  sessionId: number;
  summary: SessionSummary;
  completed: boolean;
  deckIds: number[] | null;
  scale: number;
}) {
  const nav = useNav();
  const toast = useToast();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false); // 두 번 누르지 않게 (QA T3-16)
  // G3: 학습 중 [나누기 제안]을 눌러 둔 카드는 여기서 G2 나누기 창을 연다
  const [splitting, setSplitting] = useState<number | null>(null);
  const remaining = s.remaining_review + s.remaining_new;
  const finished = completed && remaining === 0;
  const folded = s.buried.length > 0;
  const home = () => nav({ name: "home" });
  // 같은 날 다시 들어가면 백엔드가 앞 세션을 이어 쓴다
  const resume = () => nav({ name: "summary", deckIds });
  const userFolded = s.buried.filter((b) => b.reason === "user").length;
  const foldedWhy =
    userFolded === 0 ? "두 번 기억나지 않아 자동으로 접어둔 카드예요." : userFolded === s.buried.length ? "직접 접어둔 카드예요." : "자동으로 또는 직접 접어둔 카드예요.";

  // 접은 카드를 고르지 않고 떠나면 [N일 뒤 다시 보기]와 같다 (QA T2-08). 백엔드에도 답해 두어 같은 날 이어 쓴 세션의 끝 화면이 또 묻지 않게.
  // 되돌리기 · 오늘 다시 보기로 학습에 들어가면 묻지 않은 것이다. StrictMode 의 가짜 언마운트에는 답하지 않는다 (다시 마운트되면 alive)
  const decided = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      setTimeout(() => !alive.current && !decided.current && folded && api.resolveBuried(sessionId, false).catch(() => {}));
    };
  }, [sessionId, folded]);

  const resolve = async (today: boolean) => {
    if (busy) return;
    decided.current = true;
    setBusy(true);
    try {
      const ids = await api.resolveBuried(sessionId, today);
      if (today) nav({ name: "study", deckIds, scale, cardIds: ids, sessionId });
      else nav({ name: "home" });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  // 마지막 카드의 평가(또는 두 번째 Again 의 접기)를 여기서 되돌리고 그 카드로 학습에 다시 들어간다 (QA T2-02).
  // 방금 끝난 학습에서 한 평가 · 접기가 없으면 버튼도 Ctrl Z 도 없다. 백엔드도 한 번만 꺼낸다 (QA V3-01)
  const undo = async () => {
    if (busy || !s.undoable) return;
    setBusy(true);
    try {
      const id = await api.undoLast(sessionId);
      if (id !== null) {
        decided.current = true;
        return nav({ name: "study", deckIds, scale, cardIds: [id], sessionId });
      }
      toast({ title: "더 이전 평가는 되돌릴 수 없어요", icon: "info" });
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
  };
  useKey({ action: "undo" }, () => void undo(), s.undoable);
  // Enter = 화면의 주 버튼 (접은 카드 [N일 뒤 다시 보기] · 남은 카드 [이어서 하기] · [홈으로]), Esc = 홈 (QA T2-07)
  useKey("Enter", () => (folded ? void resolve(false) : finished ? home() : resume()));
  useKey("Escape", home);
  // 남은 카드 중 오늘 이미 본 카드(학습 단계)는 학습한 카드에도 들어 있다. 두 칸의 합이 처음 분량과 맞게 따로 적는다 (QA T2-01).
  // 머리말 · 복습 칸 · 오른쪽 제목도 같은 수로: 다시 볼 카드는 "예정 복습"이나 "남은 N장"에 넣지 않는다 (QA V3-02 · V6-02)
  const unseen = remaining - s.remaining_seen;
  const reviewLeft = s.remaining_review - s.remaining_seen;

  return (
    <main className="done">
      <section className="done__left">
        <span className="pill">
          <span className={`dot ${finished ? "dot--local" : "dot--warm"}`} />
          <span className="t-label-s c-secondary">
            {finished ? `오늘 예정 ${n(s.studied)}장 모두 봤어요` : `오늘 예정 중 ${n(unseen)}장이 남았어요`}
          </span>
          {/* 다시 볼 카드는 따로 한 조각 (Figma 351:26006) */}
          {!finished && s.remaining_seen > 0 && (
            <>
              <span className="dot dot--ink" />
              <span className="t-label-s c-secondary">다시 볼 {n(s.remaining_seen)}장</span>
            </>
          )}
        </span>
        <div className="done__gap" />
        <p className="t-display-l c-tertiary">오늘 학습한 카드</p>
        <p className="summary__count">
          <span className="summary__num">{n(s.studied)}</span>
          <span className="t-display-l">장</span>
        </p>
        <div className="done__tiles">
          <StatTile
            label="복습"
            value={n(s.review_done)}
            sub={finished ? undefined : reviewLeft === 0 ? "예정 복습 모두 끝" : `예정 복습 ${n(reviewLeft)}장 남음`}
          />
          <StatTile label="신규" value={n(s.new_done)} sub={finished ? undefined : `${n(s.new_done + s.remaining_new)}장 중`} />
          <StatTile
            label="남은 카드"
            value={n(unseen)}
            sub={finished ? undefined : s.remaining_seen > 0 ? `+ 다시 볼 ${n(s.remaining_seen)}장` : "오늘 안에 이어서"}
          />
        </div>
        <div className="done__note">
          {s.undoable && <Button variant="ghost" size="M" icon="rotate" label="마지막 평가 되돌리기" shortcut={actionKey("undo")} onClick={undo} disabled={busy} />}
          {error && !folded && <p className="t-body-s c-danger">{error}</p>}
        </div>
        {/* D5-d: 접어둔 카드가 없으면 오른쪽 패널 제목이 같은 말을 한다 */}
        {!finished && folded && (
          <p className="done__note t-body-m c-secondary">{hourLabel(getCutoff())} 전까지는 이어서 할 수 있어요. 못 본 신규는 내일 계획에 자연히 반영돼요.</p>
        )}
        {/* 접어둔 카드가 없으면 오른쪽 패널이 행동을 맡는다 (같은 버튼 두 개 금지, D5-c) */}
        {folded && (
          <div className="done__home">
            <Button variant="ghost" icon="chevron-left" label="홈으로" onClick={home} />
            {!finished && <Button icon="arrow-right" label="이어서 학습" onClick={resume} />}
          </div>
        )}
      </section>

      {!folded && (
        <section className="done__folded done__plan">
          <header className="done__folded-head">
            <span className={`done__folded-icon ${finished ? "done__icon--ok" : ""}`}>
              <Icon name={finished ? "check" : "clock"} size={20} />
            </span>
            <h2 className="t-title-m">
              {finished
                ? "접어둔 카드가 남지 않았어요"
                : s.remaining_seen > 0
                  ? `남은 ${n(unseen)}장과 다시 볼 ${n(s.remaining_seen)}장은 오늘 안에 이어서 할 수 있어요`
                  : `남은 ${n(remaining)}장은 오늘 안에 이어서 할 수 있어요`}
            </h2>
          </header>
          <p className="t-body-s c-secondary">
            {finished ? "오늘 예정 학습을 마쳤어요. 내일 예상 분량을 확인해 보세요." : "접어둔 카드가 남지 않았어요. 여기서 멈추면 내일 계획은 이렇게 잡혀요."}
          </p>
          <p className="done__tomorrow">
            <span className="t-body-m c-tertiary">내일</span>
            <span className="t-title-l">{n(s.tomorrow)}장</span>
            <span className="t-body-s c-secondary">
              복습 {n(s.tomorrow_review)} · 신규 {n(s.tomorrow_new)}
            </span>
          </p>
          {s.tomorrow_decks.length > 0 && (
            <div className="done__list">
              {s.tomorrow_decks.slice(0, 4).map((d, k) => (
                <div key={k} className="done__row done__plan-row">
                  <span className="done__plan-name t-body-m">{d.name}</span>
                  {d.dday !== null && d.dday >= 0 && <Badge tone="warn">{d.dday === 0 ? "D-day" : `D-${d.dday}`}</Badge>}
                  <span className="t-body-s c-secondary">
                    복습 {n(d.review)} · 신규 {n(d.new)}
                  </span>
                </div>
              ))}
              {/* 4줄 넘는 덱은 한 줄로 묶어 합계가 "내일 N장"과 맞게 */}
              {s.tomorrow_decks.length > 4 && (
                <div className="done__row done__plan-row">
                  <span className="done__plan-name t-body-m c-tertiary">
                    외 덱 {n(s.tomorrow_decks.length - 4)}개 · {n(s.tomorrow_decks.slice(4).reduce((a, d) => a + d.review + d.new, 0))}장
                  </span>
                </div>
              )}
            </div>
          )}
          <div className="done__actions">
            {finished ? (
              <>
                <Button className="done__wide" icon="chevron-left" label="홈으로" shortcut="Enter" onClick={home} />
                <p className="t-caption c-tertiary">
                  내일 {hourLabel(getCutoff())}
                  {getCutoff() === 0 ? "이" : "가"} 지나면 홈의 "오늘"에 올라와요
                </p>
              </>
            ) : (
              <>
                <Button className="done__wide" icon="arrow-right" label={`이어서 하기 · ${n(remaining)}장`} shortcut="Enter" onClick={resume} />
                <Button className="done__wide" variant="ghost" icon="chevron-left" label="여기서 멈추고 홈으로" onClick={home} />
                {/* "내일" 숫자는 못 본 신규가 산식으로 다시 나뉜 값이다 (이월 아님, 큐 명세 D5) */}
                {/* 목표일이 없으면 나눌 날이 없다. 내일 하루 몫(상한)에서 이어진다 (QA T3-25) */}
                {s.remaining_new > 0 && (
                  <p className="t-caption c-tertiary">
                    {s.has_target
                      ? `못 본 신규 ${n(s.remaining_new)}장은 이월되지 않고 남은 날에 나뉘어 들어가요`
                      : `목표일이 없어 내일 하루 몫(상한 ${n(s.new_cap)}장)에서 이어서 나와요`}
                  </p>
                )}
              </>
            )}
          </div>
        </section>
      )}

      {folded && (
        <section className="done__folded">
          <header className="done__folded-head">
            <span className="done__folded-icon">
              <Icon name="archive" size={20} />
            </span>
            <h2 className="t-title-m">접어둔 카드 {n(s.buried.length)}장</h2>
          </header>
          <p className="t-body-s c-secondary">{foldedWhy} 사라지지 않고 {buryReturnDay()}부터 다시 나와요. 오늘 다시 볼 수도 있어요.</p>
          <div className="done__list">
            {s.buried.map((b) => (
              <div key={b.id} className="done__row">
                <Importance level={b.importance} />
                <span className="done__row-text">
                  <span className="t-body-m">{b.question || "(내용 없음)"}</span>
                  <span className="t-caption c-tertiary">{b.deck_path.join(" · ")}</span>
                </span>
                <SplitLaterChip cardId={b.id} onOpen={() => setSplitting(b.id)} />
              </div>
            ))}
          </div>
          <div className="done__actions">
            <Button className="done__wide" icon="arrow-right" label={`${BURY_DAYS}일 뒤 다시 보기`} shortcut="Enter" onClick={() => resolve(false)} disabled={busy} />
            <p className="t-caption c-tertiary">{buryReturnDay()}부터 오늘의 학습에 다시 포함돼요</p>
            <Button
              className="done__wide"
              variant="secondary"
              icon="rotate"
              label={`오늘 다시 보기 · ${n(s.buried.length)}장`}
              onClick={() => resolve(true)}
              disabled={busy}
            />
            {/* 막지 않는다. 다른 곳으로 가도 접은 카드는 N일 뒤로 넘어간다 (QA T2-08) */}
            <p className="t-caption c-tertiary">다른 곳으로 가면 {BURY_DAYS}일 뒤에 다시 나와요</p>
            {error && <p className="t-body-s c-danger">{error}</p>}
          </div>
        </section>
      )}
      {splitting !== null && <SplitDialog cardId={splitting} onClose={() => setSplitting(null)} />}
    </main>
  );
}
