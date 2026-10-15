// G1 카드 점검 · G2 나누기 제안. 검사는 규칙으로만 한다 (모델 없음, 기기 안에서). G2 성격별로 묶기만 ChatGPT 요금제로 (ai.tsx, 누를 때만).
// 이 화면은 카드를 바꾸지 않는다. 바뀌는 건 G2 에서 승인했을 때뿐이고, 한 번의 변경으로 남아 되돌릴 수 있다
import "../styles/quality.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { aiApi, aiError, api, errorText, qualityApi, type AiError, type CardChange, type QualityCard, type QualityFlag, type SplitPart, type SplitProposal } from "../api";
import { n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { Badge, Button, Checkbox, Icon, Spinner } from "../ui";
import { AiConnect, AiErrorBox, ScopeLine, SendList, UsingPlan, refreshAi, sendConfirmed, sentRows, setSendConfirmed, useAiStatus } from "../ai";
import { ragApi, type Sent } from "../sourceApi";
import { SegmentPanel } from "./SourcePanel";
import { Modal } from "./NewDeck";
import { splitLater } from "../written";

type View = "all" | "again" | "long" | "multi";

const has = (c: QualityCard, ...kinds: QualityFlag["kind"][]) => c.flags.some((f) => kinds.includes(f.kind));
// Again 은 모양 문제와 겹칠 때만 붙는다 (quality_check). 그래서 again = "자주 틀리는 긴 카드"
const VIEWS: { value: View; label: string; test: (c: QualityCard) => boolean }[] = [
  { value: "all", label: "모두", test: () => true },
  { value: "again", label: "자주 틀리는 긴 카드", test: (c) => has(c, "again") },
  { value: "long", label: "답이 긴 카드", test: (c) => has(c, "lines", "items") },
  { value: "multi", label: "질문이 여러 개", test: (c) => has(c, "questions") },
];

/** 노랑 = 카드 모양, 회색 = 물음 개수, 빨강 = 학습 결과 (G1 노트) */
function FlagBadge({ f }: { f: QualityFlag }) {
  if (f.kind === "items") return <Badge tone="warn">답 {n(f.n)}항목</Badge>;
  if (f.kind === "lines") return <Badge tone="warn">답 {n(f.n)}줄</Badge>;
  if (f.kind === "questions") return <Badge>질문 {n(f.n)}개</Badge>;
  return <Badge tone="danger">Again {n(f.n)}번</Badge>;
}

const CRITERIA = `검사 기준 · 답이 8줄 또는 항목 6개 이상 · 질문에 물음표나 "와/과 … 은?"이 2개 이상 · 최근 14일 Again 2번 이상`;

/**
 * 그대로 두기 · 나누기 · 되돌리기. G1 화면과 SplitDialog(G3 · D5)가 같이 쓴다.
 * 나누거나 그대로 두기로 정하면 학습 중 "나중에 볼 제안"(splitLater)에서도 뺀다
 */
function useSplitActions(reload: () => void) {
  const toast = useToast();
  const fail = (e: unknown) => toast({ title: "바꾸지 못했어요", text: errorText(e), tone: "error" });
  /** [그대로 두기]: 30일 동안 점검 목록에서 뺀다. 카드는 그대로 */
  const keep = (cardId: number, hide = true): Promise<void> =>
    qualityApi.hide(cardId, hide).then(() => {
      reload();
      if (!hide) return;
      splitLater.remove(cardId);
      toast({
        title: "30일간 점검에서 숨겼어요",
        text: "카드 내용과 학습은 그대로예요.",
        tone: "undo",
        action: { label: "되돌리기", onClick: () => keep(cardId, false) },
      });
    }, fail);
  const undo = (changeId: number) =>
    qualityApi.undo(changeId).then(() => {
      reload();
      toast({ title: "나누기를 되돌렸어요", text: "새 카드를 지우고 원래 카드를 나누기 전 상태로 되돌렸어요." });
    }, fail);
  const done = (cardId: number, changeId: number, count: number, archived: boolean) => {
    splitLater.remove(cardId);
    reload();
    toast({
      title: `${count}장으로 나눴어요`,
      text: archived ? "원래 카드는 학습을 중지했어요. 내용과 복습 기록은 남아요." : "원래 카드도 그대로 학습해요.",
      tone: "undo",
      action: { label: "되돌리기", onClick: () => undo(changeId) },
    });
  };
  return { keep, undo, done };
}

/**
 * G2 를 카드 한 장에 바로 연다 (학습 화면 G3 · D5 "나중에 볼 제안" 칩).
 * 나누거나 그대로 두면 splitLater 에서 빼고 토스트(되돌리기)를 띄운 뒤 닫는다. onChanged: 카드가 바뀐 뒤 화면을 다시 읽을 때
 */
export function SplitDialog({ cardId, onClose, onChanged = () => {} }: { cardId: number; onClose: () => void; onChanged?: () => void }) {
  const act = useSplitActions(onChanged);
  return (
    <SplitModal
      cardId={cardId}
      onClose={onClose}
      onKeep={() => {
        onClose();
        act.keep(cardId);
      }}
      onDone={(id, count, archived) => {
        onClose();
        act.done(cardId, id, count, archived);
      }}
    />
  );
}

export function Quality({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [deckName, setDeckName] = useState("");
  const [cards, setCards] = useState<QualityCard[] | null>(null);
  const [changes, setChanges] = useState<CardChange[]>([]);
  const [view, setView] = useState<View | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [splitting, setSplitting] = useState<number | null>(null);
  const [error, setError] = useState<string>();

  const load = useCallback(
    () =>
      Promise.all([qualityApi.check(deckId), qualityApi.recent(deckId)]).then(([c, r]) => {
        setCards(c);
        setChanges(r);
      }, (e) => setError(errorText(e))),
    [deckId],
  );
  useEffect(() => {
    api.deckDetail(deckId).then((d) => setDeckName(d.name), () => {});
    load();
  }, [deckId, load]);
  const back = useCallback(() => nav({ name: "deck", deckId }), [nav, deckId]);
  useKey("Escape", back, splitting === null);
  const { keep, undo, done: splitDone } = useSplitActions(load);

  if (!cards) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  const shown = cards.filter((c) => !c.hidden);
  const hidden = cards.filter((c) => c.hidden);
  // 기본 보기는 "자주 틀리는 긴 카드". 없으면 모두
  const current = view ?? (shown.some((c) => has(c, "again")) ? "again" : "all");
  const list = shown.filter(VIEWS.find((v) => v.value === current)!.test);

  const row = (c: QualityCard) => (
    <div key={c.id} className="quality__row">
      <div className="quality__text">
        <p className="t-body-m">{c.question}</p>
        <div className="quality__meta">
          <span className="t-caption c-tertiary">{c.path.join(" › ")}</span>
          {c.flags.map((f) => (
            <FlagBadge key={f.kind} f={f} />
          ))}
        </div>
      </div>
      {c.hidden ? (
        <Button variant="ghost" size="M" label="다시 올리기" onClick={() => keep(c.id, false)} />
      ) : (
        <>
          <Button variant="ghost" size="M" label="30일간 점검에서 숨기기" onClick={() => keep(c.id)} />
          <Button variant="secondary" size="M" icon="layers" label="나누기 제안" onClick={() => setSplitting(c.id)} />
        </>
      )}
    </div>
  );
  const done = changes.filter((c) => !c.undone);

  return (
    <main className="cards quality">
      <button className="backlink t-label-m c-secondary" onClick={back}>
        <Icon name="chevron-left" size={18} />
        {deckName}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">카드 점검</h1>
          <p className="t-body-s c-secondary">답이 길거나 질문이 여러 개인 카드를 찾았어요. 고칠지는 직접 정해요. 점검에서 숨겨도 카드와 학습은 그대로예요.</p>
        </div>
        <Badge>기기 안에서 검사 · 규칙 기반</Badge>
      </div>

      <div className="segment segment--boxed" role="radiogroup" aria-label="보기">
        {VIEWS.map((v) => (
          <button
            key={v.value}
            role="radio"
            aria-checked={v.value === current}
            className={`segment__item t-label-m ${v.value === current ? "is-selected" : ""}`}
            onClick={() => setView(v.value)}
          >
            {v.label} {n(shown.filter(v.test).length)}
          </button>
        ))}
      </div>

      {current === "again" && list.length > 0 && (
        <p className="quality__info t-body-s">
          <Icon name="info" size={16} />
          같은 카드를 자꾸 틀린다면, 더 반복하기보다 카드를 나누는 게 나을 수 있어요. 이 목록은 길고 자주 틀리는 카드만 모았어요.
        </p>
      )}

      {list.length > 0 ? (
        <div className="panel quality__list">{list.map(row)}</div>
      ) : (
        <div className="panel quality__empty">
          <span className="t-title-s">{shown.length ? "이 보기에 맞는 카드가 없어요" : "고칠 만한 카드가 없어요"}</span>
          <span className="t-body-s c-secondary">답이 길거나 질문이 여러 개인 카드가 생기면 여기에 모여요.</span>
        </div>
      )}
      <p className="t-caption c-tertiary">{CRITERIA}</p>

      {hidden.length > 0 && (
        <section className="quality__section">
          <button className="quality__fold t-label-m c-secondary" aria-expanded={showHidden} onClick={() => setShowHidden(!showHidden)}>
            숨긴 카드 {n(hidden.length)}
            <Icon name="chevron-down" size={14} className={showHidden ? "is-flipped" : ""} />
          </button>
          {showHidden && <div className="panel quality__list">{hidden.map(row)}</div>}
        </section>
      )}

      {/* G2 노트의 "최근 변경" 목록. 토스트가 사라진 뒤에도 되돌릴 수 있게 */}
      {done.length > 0 && (
        <section className="quality__section">
          <h2 className="t-label-m c-secondary">최근 나눈 카드</h2>
          <div className="panel quality__list">
            {done.map((c) => (
              <div key={c.id} className="quality__row quality__row--change">
                <span className="t-body-m">{c.summary}</span>
                <span className="t-caption c-tertiary">{new Date(c.created_at).toLocaleString("ko-KR", { month: "long", day: "numeric", hour: "numeric", minute: "2-digit" })}</span>
                <Button variant="ghost" size="M" icon="rotate" label="되돌리기" onClick={() => undo(c.id)} />
              </div>
            ))}
          </div>
        </section>
      )}

      {splitting !== null && (
        <SplitModal
          cardId={splitting}
          onClose={() => setSplitting(null)}
          onKeep={() => {
            setSplitting(null);
            keep(splitting);
          }}
          onDone={(id, count, archived) => {
            setSplitting(null);
            splitDone(splitting, id, count, archived);
          }}
        />
      )}
    </main>
  );
}

/** 글자만 옮기므로 잃는 것: 원래 노트 필드의 그림 수와 서식 (G2 노트) */
const FORMATTING = "b, strong, i, em, u, span[style], font, mark, sub, sup, table, [color], [style*='color']";
function lostInSplit(fields: string[]) {
  const doc = new DOMParser().parseFromString(fields.join("<br>"), "text/html");
  return { images: doc.querySelectorAll("img").length, formatted: !!doc.querySelector(FORMATTING) };
}
/** "원래 카드의 그림 2개와 서식은 옮기지 않아요 · …". 잃는 것이 없으면 null */
function lostLine(lost: { images: number; formatted: boolean }, archive: boolean) {
  const what = lost.images && lost.formatted ? `그림 ${lost.images}개와 서식은` : lost.images ? `그림 ${lost.images}개는` : lost.formatted ? "서식은" : null;
  if (!what) return null;
  return `원래 카드의 ${what} 옮기지 않아요 · ${archive ? "학습을 중지한 원래 카드에 남아요" : "원래 카드를 지우면 함께 사라져요"}`;
}

/** 다른 제안: 한 장에 담는 항목 수를 3 → 2 → 4 → 1 순으로 바꾼다 (규칙 분할) */
const PER_ORDER = [3, 2, 4, 1];
const lines = (s: string) => s.split("\n").map((l) => l.trim()).filter(Boolean);

/** G2 나누기 제안. 승인하기 전에는 아무것도 바뀌지 않는다. [규칙으로 나누기 | 성격별로 묶기(ChatGPT 요금제, L4)] */
function SplitModal({ cardId, onClose, onKeep, onDone }: {
  cardId: number;
  onClose: () => void;
  onKeep: () => void;
  onDone: (changeId: number, count: number, archived: boolean) => void;
}) {
  const [p, setP] = useState<SplitProposal | null>(null);
  const [parts, setParts] = useState<SplitPart[]>([]);
  const [editing, setEditing] = useState<number | null>(null);
  const [archive, setArchive] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  // L4 성격별로 묶기. 기본은 규칙. 저절로 보내지 않고, 결과도 승인 전에는 제안일 뿐이다
  const ai = useAiStatus();
  const [mode, setMode] = useState<"rule" | "ai">("rule");
  const [aiP, setAiP] = useState<SplitProposal | null>(null);
  const [aiBusy, setAiBusy] = useState(false);
  const [aiSlow, setAiSlow] = useState(false);
  const [aiFail, setAiFail] = useState<AiError | null>(null);
  const [asking, setAsking] = useState(false);
  const [connect, setConnect] = useState<{ start: "intro" | "wait"; reconsent: boolean } | null>(null);
  const seq = useRef(0);
  const inFlight = useRef(false);
  // R3: 이 카드에 연결된 구간(최대 3개)을 함께 보낸다. 확인 목록에서 뺄 수 있다. 근거 칩 → 오른쪽 I2 패널
  const [segs, setSegs] = useState<Sent[]>([]);
  const [peek, setPeek] = useState(false);
  const [panelSeg, setPanelSeg] = useState<number | null>(null);
  useEffect(() => {
    ragApi.linked(cardId).then(setSegs, () => setSegs([]));
  }, [cardId]);

  const suggest = useCallback(
    (per: number | null) =>
      qualityApi.suggest(cardId, per).then((x) => {
        setP(x);
        setParts(x.parts);
        setEditing(null);
      }, (e) => setError(errorText(e))),
    [cardId],
  );
  useEffect(() => {
    suggest(null);
  }, [suggest]);
  const [lost, setLost] = useState<{ images: number; formatted: boolean } | null>(null);
  useEffect(() => {
    api.cardView(cardId).then((v) => api.noteEdit(v.note_id)).then((e) => setLost(lostInSplit(e.values)), () => {});
  }, [cardId]);
  // 30초가 지나면 [계속 기다리기] [규칙으로 나누기]
  useEffect(() => {
    if (!aiBusy || aiSlow) return;
    const t = setTimeout(() => setAiSlow(true), 30_000);
    return () => clearTimeout(t);
  }, [aiBusy, aiSlow]);
  // 창을 닫으면 보내던 요청도 멈춘다
  useEffect(
    () => () => {
      if (inFlight.current) aiApi.cancel().catch(() => {});
    },
    [],
  );

  /** 규칙 제안으로 돌아간다. 보내던 요청은 멈춘다 */
  const toRule = () => {
    if (inFlight.current) aiApi.cancel().catch(() => {});
    inFlight.current = false;
    seq.current++;
    setAiBusy(false);
    setAiSlow(false);
    setAiFail(null);
    setAsking(false);
    setMode("rule");
    if (p) setParts(p.parts);
    setEditing(null);
  };
  /** 이 카드 한 장을 보낸다 (처음 보내기 확인 · [다시 묶기] · [다시 시도]). 사용량이 줄어든다 */
  const send = () => {
    setSendConfirmed(true, segs.length);
    setAsking(false);
    setPeek(false);
    const my = ++seq.current;
    inFlight.current = true;
    setMode("ai");
    setAiFail(null);
    setAiBusy(true);
    setAiSlow(false);
    setEditing(null);
    aiApi.regroup(cardId, segs.map((s) => s.id)).then(
      (x) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        setAiP(x);
        setParts(x.parts);
        setAiBusy(false);
        refreshAi();
      },
      (e) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        const f = aiError(e);
        setAiBusy(false);
        refreshAi();
        if (f.kind === "cancelled") return toRule();
        if (f.kind === "not_connected") {
          toRule();
          return setConnect({ start: "intro", reconsent: false });
        }
        setAiFail(f);
        // 깨진 응답은 한 조각도 쓰지 않는다: 아래에 규칙 제안을 그대로 보인다
        if (f.kind === "bad_output" && p) setParts(p.parts);
      },
    );
  };
  const pickAi = () => {
    if (mode === "ai" || asking) return;
    if (!ai?.connected) return setConnect({ start: "intro", reconsent: false });
    if (aiP) {
      setMode("ai");
      setParts(aiP.parts);
      setAiFail(null);
      setEditing(null);
      return;
    }
    if (!sendConfirmed(segs.length)) return setAsking(true);
    send();
  };
  // 보내는 범위 한 줄 (R3-s): 보내기 전 / 보낸 뒤
  const sentNow = mode === "ai" && aiP && !aiBusy ? aiP.sent : null;
  const scopeText = sentNow
    ? `보낸 것: ${sentNow.length ? `카드 + 자료 ${n(sentNow.length)}구간` : "카드 한 장"}`
    : segs.length
      ? `카드 + 자료 ${n(segs.length)}구간을 보내요`
      : "카드 한 장만 보내요";

  // L2 연결 창은 이 창 대신 뜬다 (창을 겹치지 않는다). 돌아오면 규칙에서 다시 고른다 (저절로 보내지 않는다)
  if (connect)
    return (
      <AiConnect
        start={connect.start}
        reconsent={connect.reconsent}
        onDone={(ok) => {
          setConnect(null);
          if (ok) toRule();
        }}
      />
    );
  if (!p) return null;
  const total = p.parts.reduce((s, x) => s + lines(x.back).length, 0);
  // 지금과 장수가 달라지는 다음 항목 수. 물음마다 나눈 제안은 항목 수와 상관없다
  const nextPer = (() => {
    if (p.kind === "questions") return null;
    const at = PER_ORDER.indexOf(p.per_card);
    for (let i = 1; i <= PER_ORDER.length; i++) {
      const per = PER_ORDER[(at + i) % PER_ORDER.length];
      if (per < total && Math.max(1, Math.floor(total / per)) !== p.parts.length) return per;
    }
    return null;
  })();
  const aiSel = mode === "ai" || asking;
  // 지금 보이는 제안: ChatGPT 결과가 있고 오류가 없을 때만 ChatGPT 제안
  const fromAi = mode === "ai" && !!aiP && !aiBusy && !aiFail;
  const shown = fromAi && aiP ? aiP : p;
  // 보내는 중 · 오류(깨진 응답 제외)에는 목록이 없다
  const listed = !(mode === "ai" && (aiBusy || (aiFail && aiFail.kind !== "bad_output")));
  const ready = listed ? parts.filter((x) => x.front.trim() && lines(x.back).length) : [];
  const set = (i: number, patch: Partial<SplitPart>) => setParts(parts.map((x, j) => (j === i ? { ...x, ...patch } : x)));
  const apply = () => {
    setBusy(true);
    qualityApi.apply(cardId, ready, archive).then(
      (id) => onDone(id, ready.length, archive),
      (e) => {
        setError(errorText(e));
        setBusy(false);
      },
    );
  };

  return (
    <>
    <Modal
      title="카드 나누기 제안"
      sub={fromAi ? "ChatGPT가 비슷한 내용끼리 나눈 제안이에요. 그대로 쓰기 전에 확인하고 고쳐 주세요." : "규칙으로 나눈 제안이에요. 고쳐서 써도 되고, 나누지 않아도 돼요."}
      onClose={onClose}
      className={`split ${panelSeg !== null ? "has-panel" : ""}`}
    >
      <p className="quality__info split__why t-body-s">
        <Icon name="info" size={16} />
        왜? {shown.why}
      </p>
      {/* R3 근거: ChatGPT 가 실제로 인용한 구간만 (보냈지만 쓰지 않은 구간은 칩이 없다) */}
      {fromAi && aiP && aiP.evidence.length > 0 && (
        <p className="split__evidence">
          <span className="t-label-s c-tertiary">근거</span>
          {aiP.evidence.map((s) => (
            <button key={s.id} type="button" className="src-chip src-chip--surface t-label-s" onClick={() => setPanelSeg(s.id)}>
              <Icon name="file" size={12} />
              {s.material} p.{s.page}
              {s.signals[0] === "highlight" ? " · 하이라이트" : ""}
            </button>
          ))}
        </p>
      )}
      <div className="split__cols">
        <section className="split__orig">
          <span className="t-label-s c-tertiary">지금 카드</span>
          <p className="t-body-m-strong">{p.question}</p>
          {p.answer.map((l, i) => (
            <p key={i} className="t-body-s c-secondary">{l}</p>
          ))}
        </section>
        <span className="split__arrow c-tertiary">
          <Icon name="arrow-right" size={22} scaled />
        </span>
        <section className="split__parts" aria-busy={aiBusy}>
          {mode === "ai" && aiBusy ? (
            <>
              <span className="ai-busy">
                <Spinner size={16} />
                <span className="t-label-m c-secondary">ChatGPT가 나누는 중…</span>
                <button type="button" className="ai-link t-label-s" onClick={toRule}>
                  취소
                </button>
              </span>
              {aiSlow && (
                <span className="ai-slow">
                  <span className="t-caption c-tertiary">30초가 넘게 걸리고 있어요.</span>
                  <Button variant="secondary" size="M" label="계속 기다리기" onClick={() => setAiSlow(false)} />
                  <Button variant="ghost" size="M" label="규칙으로 나누기" onClick={toRule} />
                </span>
              )}
              {[0.78, 0.66, 0.72].map((w, i) => (
                <div key={i} className="ai-skel" aria-hidden>
                  <span style={{ width: `${w * 100}%` }} />
                  <span style={{ width: `${w * 80}%` }} />
                </div>
              ))}
            </>
          ) : (
            <>
              {mode === "ai" && aiFail && <AiErrorBox e={aiFail} onRule={toRule} onRetry={send} onReconnect={(reconsent) => setConnect({ start: "wait", reconsent })} />}
              {listed && (
                <>
                  <span className="t-label-s c-tertiary">
                    나눈 카드 {n(ready.length)}장{fromAi ? " · ChatGPT 제안" : p.kind !== "questions" ? " · 질문만 읽고 답할 수 있는지 확인해 주세요" : ""}
                  </span>
                  {parts.map((x, i) => {
                    const items = lines(x.back);
                    const on = editing === i;
                    return (
                      <div key={i} className={`split__card ${on ? "is-editing" : ""}`}>
                        <div className="split__card-head">
                          {on ? (
                            <input
                              className="split__input t-body-m-strong"
                              value={x.front}
                              placeholder="질문"
                              aria-label={`${i + 1}번째 카드 질문`}
                              autoFocus
                              onChange={(e) => set(i, { front: e.target.value })}
                              onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setEditing(null))}
                            />
                          ) : (
                            <p className={`t-body-m-strong ${x.front.trim() ? "" : "c-tertiary"}`}>{x.front.trim() || "질문을 써 주세요"}</p>
                          )}
                          <span className="split__tools">
                            <button className="split__tool c-secondary" aria-label={on ? "고치기 끝" : `${i + 1}번째 카드 고치기`} aria-pressed={on} onClick={() => setEditing(on ? null : i)}>
                              <Icon name={on ? "check" : "pencil"} size={16} />
                            </button>
                            <button
                              className="split__tool c-secondary"
                              aria-label={`${i + 1}번째 카드 빼기`}
                              onClick={() => {
                                setParts(parts.filter((_, j) => j !== i));
                                setEditing(null);
                              }}
                            >
                              <Icon name="x" size={16} />
                            </button>
                          </span>
                        </div>
                        {on ? (
                          <textarea
                            className="split__input split__textarea t-body-s"
                            value={x.back}
                            placeholder="답. 한 줄에 항목 하나"
                            aria-label={`${i + 1}번째 카드 답`}
                            rows={Math.max(2, x.back.split("\n").length)}
                            onChange={(e) => set(i, { back: e.target.value })}
                            onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setEditing(null))}
                          />
                        ) : (
                          <p className={`t-body-s ${items.length ? "c-secondary" : "c-tertiary"}`}>{items.length ? items.join(" · ") : "답을 써 주세요"}</p>
                        )}
                        {on && (
                          <p className="t-caption c-accent">
                            고치는 중{items.length >= 4 ? ` · 항목이 ${items.length}개라 한 번 더 나눌 수 있어요` : " · 답은 한 줄에 항목 하나"}
                          </p>
                        )}
                      </div>
                    );
                  })}
                  <button
                    className="split__add t-label-m c-tertiary"
                    onClick={() => {
                      setParts([...parts, { front: "", back: "" }]);
                      setEditing(parts.length);
                    }}
                  >
                    <Icon name="plus" size={14} scaled />
                    카드 한 장 더
                  </button>
                </>
              )}
            </>
          )}
        </section>
      </div>
      <div className="split__keep">
        <Checkbox checked={archive} onChange={setArchive} label="원래 카드의 학습을 중지해요 (내용과 복습 기록은 남아요)" />
        {lost && lostLine(lost, archive) && <p className="split__lost t-caption">{lostLine(lost, archive)}</p>}
        <p className="t-caption c-tertiary">
          나눈 카드{listed ? ` ${n(ready.length)}장` : ""}은 신규 카드로 시작하고, 중요도·하위 덱은 원래 카드를 따라요. 카드 점검의 "최근 나눈 카드"에서 되돌리면 원래 상태로 돌아가요.
        </p>
      </div>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="split__foot">
        <span className="split__alts ai-mode">
          {/* L4-s 처음 보낼 때 한 번. Esc 는 이 확인만 닫는다 */}
          {asking && !segs.length && (
            <div className="ai-confirm" role="dialog" aria-label="이 카드 한 장을 ChatGPT로 보내요" onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setAsking(false))}>
              <span className="ai-confirm__head">
                <Icon name="send" size={16} className="c-accent" />
                <span className="t-body-m-strong">이 카드 한 장을 ChatGPT로 보내요</span>
              </span>
              <span className="t-caption c-tertiary">질문과 답 항목 글자만 보내요 · ChatGPT 요금제 사용량에서 줄어요</span>
              <span className="ai-confirm__actions">
                <Button variant="ghost" size="M" label="취소" onClick={() => setAsking(false)} />
                <Button size="M" icon="send" label="보내기" autoFocus onClick={send} />
              </span>
            </div>
          )}
          {/* R3-s 자료 구간이 있으면 목록을 펼친 채로 확인. 구간을 모두 빼면 카드 한 장만 보낸다 */}
          {(asking ? segs.length > 0 : peek) && (
            <div
              className="ai-confirm ai-confirm--list"
              role="dialog"
              aria-label={asking ? `이 카드와 자료 ${segs.length}구간을 ChatGPT로 보내요` : "보낸 것"}
              onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setAsking(false), setPeek(false))}
            >
              <SendList
                compact
                title={asking ? `이 카드와 자료 ${n(segs.length)}구간을 ChatGPT로 보내요` : "보낸 것"}
                rows={[
                  { key: "card", icon: "layers", meta: <span className="t-label-s c-secondary">카드 앞·뒷면</span>, text: `${p.question} / ${p.answer.join(" · ")}` },
                  ...(asking
                    ? sentRows(segs, (id) => setSegs(segs.filter((s) => s.id !== id)), true)
                    : sentRows(aiP?.sent ?? [], () => {}, true).map((r) => ({ ...r, remove: undefined }))),
                ]}
                foot={
                  <div className="grade__confirm-foot">
                    <UsingPlan />
                    <span className="grade__actions">
                      {asking ? (
                        <>
                          <Button variant="ghost" size="M" label="취소" onClick={() => setAsking(false)} />
                          <Button size="M" icon="send" label="보내기" autoFocus onClick={send} />
                        </>
                      ) : (
                        <Button variant="ghost" size="M" label="닫기" autoFocus onClick={() => setPeek(false)} />
                      )}
                    </span>
                  </div>
                }
              />
            </div>
          )}
          <span className="segment segment--boxed" role="radiogroup" aria-label="나누는 방법">
            <button role="radio" aria-checked={!aiSel} className={`segment__item t-label-m ${aiSel ? "" : "is-selected"}`} onClick={toRule}>
              규칙으로 나누기
            </button>
            <button role="radio" aria-checked={aiSel} className={`segment__item t-label-m ${aiSel ? "is-selected" : ""}`} onClick={pickAi}>
              비슷한 내용끼리 나누기
            </button>
          </span>
          <span className="ai-side">
            {ai?.connected ? (
              <>
                <UsingPlan />
                <ScopeLine
                  text={scopeText}
                  open={asking || peek}
                  onToggle={() => (sentNow ? setPeek((v) => !v) : asking ? setAsking(false) : setAsking(true))}
                />
              </>
            ) : !ai?.connected ? (
              <span className="ai-hint t-caption c-tertiary">
                <Icon name="lock" size={14} />
                ChatGPT 연결이 필요해요
              </span>
            ) : null}
            {aiSel ? (
              (aiP || aiFail?.kind === "bad_output") &&
              !aiBusy &&
              !asking && (
                <button type="button" className="ai-mini t-label-s" onClick={send}>
                  <Icon name="rotate" size={14} />
                  다시 나누기
                </button>
              )
            ) : (
              <button type="button" className="ai-mini t-label-s" disabled={nextPer === null} onClick={() => nextPer && suggest(nextPer)}>
                <Icon name="rotate" size={14} />
                다른 제안 받기
              </button>
            )}
          </span>
        </span>
        <span className="split__actions">
          <Button variant="ghost" size="M" label="나누지 않고 30일 숨기기" onClick={onKeep} />
          <Button size="M" icon="check" label={listed ? `${n(ready.length)}장으로 나누기` : "나누기"} disabled={ready.length < 2} busy={busy} busyLabel="나누는 중…" onClick={apply} />
        </span>
      </div>
    </Modal>
    {panelSeg !== null && (
      <div className="split__source">
        <SegmentPanel key={panelSeg} segmentId={panelSeg} onClose={() => setPanelSeg(null)} />
      </div>
    )}
    </>
  );
}
