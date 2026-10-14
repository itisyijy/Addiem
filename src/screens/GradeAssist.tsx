// R2 서술형 채점 보조 (Figma Row R: R2 81:9865 · R2-s 81:10044, 노트 81:10038 · 81:10176).
// J2 비교 화면에서 누를 때만 보낸다: 내 답 · 모범 답안 · 자료 구간(연결된 것 먼저, 없으면 비슷한 구간). 결과는 참고용이고 평가는 사용자가 한다.
// 학습 흐름을 막지 않는다: 기다리는 동안에도 1–4 가 되고, 결과 전에 평가하면 요청을 멈추고 결과는 버린다 (Study 가 stop() 을 부른다)
import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";
import { aiApi, aiError, type AiError } from "../api";
import { AiConnect, AiErrorBox, ScopeLine, SendList, UsingPlan, refreshAi, sentRows, useAiStatus, type SendRow } from "../ai";
import { ragApi, type Grade, type GradeScope, type Sent } from "../sourceApi";
import { actionKey } from "../keys";
import { n, useKey, useOverlayScope } from "../nav";
import { Button, Icon, Spinner } from "../ui";

type Phase = "idle" | "confirm" | "busy" | "done" | "error";

/** R2 오류 문구 (규칙은 L4-e 와 같다. 핵심어 검사와 평가는 그대로 된다는 말이 주 메시지) */
const COPY = (e: AiError): Partial<Record<AiError["kind"], [string, string]>> => ({
  limit: ["사용량 한도에 도달했어요", "ChatGPT 설정에서 요금제나 이 앱의 한도를 확인해 주세요. 평가는 그대로 할 수 있어요."],
  expired: ["ChatGPT 로그인이 만료됐어요", "다시 로그인하면 이어서 쓸 수 있어요. 이 답은 아직 보내지 않았어요."],
  offline: ["인터넷에 연결되어 있지 않아요", "핵심어 검사와 평가는 인터넷 없이도 그대로예요."],
  unavailable: ["지금은 ChatGPT 요금제를 쓸 수 없어요", "잠시 뒤 다시 해 보세요. 핵심어 검사는 그대로예요."],
  bad_output: ["채점 보조를 받지 못했어요", "ChatGPT 답의 형식이 맞지 않아 쓰지 않았어요. 핵심어 검사는 그대로예요."],
  error: ["채점 보조를 받지 못했어요", e.message],
});

const what = (segs: number) => `내 답 · 모범 답${segs ? ` · 자료 ${n(segs)}구간` : ""}`;

/**
 * J2 의 채점 보조. on: 쓴 답과 비교 화면이 있을 때. confirmed: 학습 세션마다 한 번 확인 (Study 의 ref).
 * 돌려주는 것: 핵심어 줄 오른쪽 버튼, 그 아래 범위 한 줄, 비교 칸 아래 자리(확인 · 기다림 · 오류 · 결과), ChatGPT 추천, 시작 · 멈춤
 */
export function useGrade({ cardId, typed, on, confirmed, openSegment }: {
  cardId: number | null;
  typed: string;
  on: boolean;
  confirmed: MutableRefObject<boolean>;
  openSegment: (id: number) => void;
}) {
  const ai = useAiStatus();
  const [scope, setScope] = useState<GradeScope | null>(null);
  const [segs, setSegs] = useState<Sent[]>([]);
  const [phase, setPhase] = useState<Phase>("idle");
  const [result, setResult] = useState<Grade | null>(null);
  const [fail, setFail] = useState<AiError | null>(null);
  const [slow, setSlow] = useState(false);
  const [showSent, setShowSent] = useState(false);
  const [connect, setConnect] = useState<{ start: "intro" | "wait"; reconsent: boolean } | null>(null);
  const seq = useRef(0);
  const confirmBox = useRef<HTMLDivElement>(null);

  // 카드가 바뀌면 처음부터. 범위는 비교 화면이 뜰 때 기기 안에서 고른다 (보내지 않는다)
  useEffect(() => {
    seq.current++;
    setPhase("idle");
    setResult(null);
    setFail(null);
    setShowSent(false);
    setScope(null);
    setSegs([]);
    if (!on || cardId === null) return;
    let live = true;
    ragApi.gradeScope(cardId, typed).then(
      (s) => {
        if (!live) return;
        setScope(s);
        setSegs(s.segments);
      },
      () => {},
    );
    return () => {
      live = false;
    };
    // typed 는 비교할 때 정해진다 (비교 화면에서는 바뀌지 않는다)
  }, [cardId, on]);
  useEffect(() => {
    if (phase !== "busy") return setSlow(false);
    const t = setTimeout(() => setSlow(true), 30_000);
    return () => clearTimeout(t);
  }, [phase]);

  // 확인 목록은 카드 위에 뜨는 겹침 창이다: [보내기]에 초점 (Enter 로 보낸다), Esc 는 목록만 닫는다 (학습을 그만두지 않게).
  // 떠 있는 동안 화면 단축키는 쉬고 평가 버튼은 눌러진다. 기다리는 동안에는 아무것도 막지 않는다
  useOverlayScope(phase === "confirm");
  useEffect(() => {
    if (phase === "confirm") confirmBox.current?.querySelector<HTMLButtonElement>(".btn--primary")?.focus({ preventScroll: true });
  }, [phase]);
  useKey("Escape", () => setPhase("idle"), phase === "confirm", { modal: true });

  const stop = useCallback(() => {
    if (phase === "busy") aiApi.cancel().catch(() => {});
    seq.current++;
    setPhase((p) => (p === "busy" || p === "confirm" ? "idle" : p));
  }, [phase]);

  const send = () => {
    if (cardId === null) return;
    confirmed.current = true;
    const my = ++seq.current;
    setFail(null);
    setPhase("busy");
    ragApi.grade(cardId, typed, segs.map((s) => s.id)).then(
      (g) => {
        if (seq.current !== my) return; // 그 사이 평가했다: 결과는 버린다
        setResult(g);
        setPhase("done");
        refreshAi();
      },
      (e) => {
        if (seq.current !== my) return;
        refreshAi();
        const f = aiError(e);
        if (f.kind === "cancelled") return setPhase("idle");
        if (f.kind === "not_connected") {
          setPhase("idle");
          return setConnect({ start: "intro", reconsent: false });
        }
        setFail(f);
        setPhase("error");
      },
    );
  };

  /** 버튼 · G. 연결 전이면 L2, 세션의 첫 전송이면 목록을 펼쳐 확인 */
  const start = () => {
    if (!on || phase === "busy") return;
    if (!ai?.connected) return setConnect({ start: "intro", reconsent: false });
    if (!confirmed.current) return setPhase("confirm");
    send();
  };

  const rows: SendRow[] = scope
    ? [
        { key: "question", icon: "layers", meta: <span className="t-label-s c-secondary">질문</span>, text: scope.question },
        { key: "mine", icon: "pencil", meta: <span className="t-label-s c-secondary">내 답</span>, text: scope.mine },
        { key: "model", icon: "check", meta: <span className="t-label-s c-secondary">모범 답안</span>, text: scope.model },
        ...sentRows(segs, (id) => setSegs(segs.filter((s) => s.id !== id)), true),
      ]
    : [];

  const button = on && (
    <Button
      variant="secondary"
      size="M"
      className="grade__btn"
      icon="sparkles"
      label="ChatGPT로 채점 보조"
      shortcut={actionKey("grade")}
      disabled={phase === "busy"}
      aria-keyshortcuts={actionKey("grade")}
      onClick={start}
    />
  );
  // 버튼 아래 늘 보이는 범위 한 줄. [보기]는 같은 확인 목록을 펼친다
  const scopeLine = on && scope && phase !== "done" && (
    <span className="grade__scope">
      <ScopeLine text={`${what(segs.length)}을 보내요`} open={phase === "confirm"} onToggle={() => setPhase((p) => (p === "confirm" ? "idle" : p === "busy" ? p : "confirm"))} />
    </span>
  );

  const popover = on && phase === "confirm" && (
    <div className="grade__confirm" ref={confirmBox} role="dialog" aria-label="채점 보조로 보낼 내용">
      <SendList
        compact
        rows={rows}
        foot={
          <div className="grade__confirm-foot">
            <UsingPlan />
            <span className="grade__actions">
              <Button variant="ghost" size="M" label="취소" onClick={() => setPhase("idle")} />
              <Button size="M" icon="send" label="보내기" onClick={send} />
            </span>
          </div>
        }
      />
    </div>
  );

  const slot = !on ? null : phase === "busy" ? (
    <div className="grade grade--wait" role="status">
      <Spinner size={16} />
      <span className="t-label-m c-secondary">{slow ? "오래 걸려요" : "ChatGPT가 답을 보는 중… 평가는 지금 해도 돼요"}</span>
      <button type="button" className="ai-link t-label-s" onClick={stop}>
        취소
      </button>
    </div>
  ) : phase === "error" && fail ? (
    <AiErrorBox
      e={fail}
      copy={COPY(fail)}
      retryLabel="다시 받기"
      onRetry={send}
      onClose={() => setPhase("idle")}
      onReconnect={(reconsent) => setConnect({ start: "wait", reconsent })}
    />
  ) : phase === "done" && result ? (
    <section className="grade" aria-label="ChatGPT 채점 보조">
      <div className="grade__head">
        <span className="grade__title">
          <Icon name="sparkles" size={16} className="c-accent" />
          <span className="t-label-m">ChatGPT 채점 보조</span>
          <span className="t-caption c-tertiary">참고용이에요 · 평가는 직접 해요</span>
        </span>
        <ScopeLine text={`보낸 것: ${what(result.sent.length)}`} open={showSent} onToggle={() => setShowSent((v) => !v)} />
      </div>
      {showSent && (
        <SendList
          compact
          rows={[
            ...rows.slice(0, 3),
            ...sentRows(result.sent, () => {}, true).map((r) => ({ ...r, remove: undefined })),
          ]}
        />
      )}
      <div className="grade__cols">
        <div className="grade__col">
          <span className="grade__col-head c-success">
            <Icon name="check" size={14} />
            <span className="t-label-s">맞은 점</span>
          </span>
          {result.right.length ? result.right.map((t, i) => <p key={i} className="t-body-s">{t}</p>) : <p className="t-body-s c-tertiary">없어요</p>}
        </div>
        <div className="grade__col">
          <span className="grade__col-head c-warn">
            <Icon name="x" size={14} />
            <span className="t-label-s">빠진 점</span>
          </span>
          {result.missing.length ? (
            result.missing.map((m, i) => (
              <div key={i} className="grade__miss">
                <p className="t-body-s">{m.text}</p>
                {m.segments.map((id) => {
                  const s = result.sent.find((x) => x.id === id);
                  return (
                    s && (
                      <button key={id} type="button" className="src-chip t-label-s" onClick={() => openSegment(id)}>
                        <Icon name="file" size={12} />
                        {s.material} p.{s.page}
                      </button>
                    )
                  );
                })}
              </div>
            ))
          ) : (
            <p className="t-body-s c-tertiary">없어요</p>
          )}
        </div>
      </div>
    </section>
  ) : null;

  const connectSheet = connect && <AiConnect start={connect.start} reconsent={connect.reconsent} onDone={() => setConnect(null)} />;

  return {
    /** ChatGPT 추천 (결과가 있을 때만) */
    rating: phase === "done" && result ? result.rating : null,
    busy: phase === "busy",
    /** 단축키 G 를 받을 때. 결과가 있으면 받지 않는다 (실수로 다시 보내 사용량을 쓰지 않게, 다시 받으려면 버튼) */
    ready: on && (phase === "idle" || phase === "error"),
    start,
    stop,
    button,
    scopeLine,
    slot,
    popover,
    connectSheet,
  };
}
