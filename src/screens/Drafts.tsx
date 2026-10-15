// R1 자료에서 카드 초안 (Figma Row R: R1-b 80:9464 · R1-c 80:9793 · R1-e 81:9717, 노트 80:9787 · 80:10051 · 81:9860).
// R1-b: 고른 구간을 "OpenAI로 보낼 내용"으로 매번 펼쳐 보이고(이 칸이 곧 확인이다) 초안을 받는다.
// R1-c: 초안은 이 화면에만 있다. 승인한 카드만 만들고(한 번의 변경으로 되돌릴 수 있다), 기기 안 검사(비슷한 카드 · 근거에 없는 말)를 칩으로 짚는다
import "../styles/quality.css";
import "../styles/drafts.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { aiApi, aiError, api, errorText, qualityApi, type AiError, type CardView } from "../api";
import { AiConnect, AiErrorBox, SendList, UsingPlan, refreshAi, sentRows } from "../ai";
import { ragApi, SIGNAL_LABEL, type Draft, type DraftAmount, type DraftCard, type Sent } from "../sourceApi";
import { renderCardHtml } from "../cardHtml";
import { n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { Button, Checkbox, Icon, LEVEL_LABEL, Segment, Spinner, type Level } from "../ui";
import { Modal } from "./NewDeck";
import { DeckPicker } from "./DeckPicker";
import { SegmentPanel } from "./SourcePanel";

const PER: Record<DraftAmount, string> = { few: "1장", normal: "1–2장", many: "1–3장" };

/** R1-e: 창 안 오류의 제목 · 본문 (규칙은 L4-e 와 같다) */
const COPY = (e: AiError) => ({
  limit: ["사용량 한도에 도달했어요", "ChatGPT 설정에서 요금제나 이 앱의 한도를 확인해 주세요. 고른 구간은 그대로 있어요."] as [string, string],
  expired: ["ChatGPT 로그인이 만료됐어요", "다시 로그인하면 이어서 만들 수 있어요. 아직 아무것도 보내지 않았어요."] as [string, string],
  offline: ["인터넷에 연결되어 있지 않아요", "미연결 강조 구간 보기와 [기존 카드 연결]은 인터넷 없이도 돼요."] as [string, string],
  unavailable: ["지금은 ChatGPT 요금제를 쓸 수 없어요", "잠시 뒤 다시 해 보세요. 고른 구간은 그대로 있어요."] as [string, string],
  bad_output: ["초안을 만들지 못했어요", "ChatGPT 답의 형식이 맞지 않아 쓰지 않았어요. 고른 구간은 그대로 있어요."] as [string, string],
  error: ["초안을 만들지 못했어요", e.message] as [string, string],
});

/** R1-b 카드 초안 만들기. 보내기 전에 구간을 하나씩 뺄 수 있고, 모두 빼면 [초안 만들기]가 꺼진다 */
export function DraftSend({ picked, amount: amount0 = "normal", cloze: cloze0 = false, onClose, onDone }: {
  picked: number[];
  amount?: DraftAmount;
  cloze?: boolean;
  onClose: () => void;
  onDone: (draft: Draft, picked: number[], amount: DraftAmount, cloze: boolean) => void;
}) {
  const [rows, setRows] = useState<Sent[] | null>(null);
  const [amount, setAmount] = useState(amount0);
  const [cloze, setCloze] = useState(cloze0);
  const [busy, setBusy] = useState(false);
  const [slow, setSlow] = useState(false);
  const [fail, setFail] = useState<AiError | null>(null);
  const [connect, setConnect] = useState<{ start: "intro" | "wait"; reconsent: boolean } | null>(null);
  const seq = useRef(0);
  const inFlight = useRef(false);
  useEffect(() => {
    ragApi.preview(picked).then(setRows, (e) => setFail(aiError(e)));
  }, [picked]);
  useEffect(() => {
    if (!busy) return setSlow(false);
    const t = setTimeout(() => setSlow(true), 30_000);
    return () => clearTimeout(t);
  }, [busy]);
  // 창을 닫으면 보내던 요청도 멈춘다
  useEffect(
    () => () => {
      if (inFlight.current) aiApi.cancel().catch(() => {});
    },
    [],
  );
  const stop = () => {
    if (inFlight.current) aiApi.cancel().catch(() => {});
    inFlight.current = false;
    seq.current++;
    setBusy(false);
  };
  const send = () => {
    if (!rows?.length) return;
    const my = ++seq.current;
    const ids = rows.map((r) => r.id);
    inFlight.current = true;
    setFail(null);
    setBusy(true);
    ragApi.draft(ids, amount, cloze).then(
      (d) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        refreshAi();
        onDone(d, ids, amount, cloze);
      },
      (e) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        setBusy(false);
        refreshAi();
        const f = aiError(e);
        if (f.kind === "cancelled") return;
        if (f.kind === "not_connected") return setConnect({ start: "intro", reconsent: false });
        setFail(f);
      },
    );
  };
  // 연결 창은 이 창 대신 뜬다. 돌아오면 저절로 보내지 않는다
  if (connect) return <AiConnect start={connect.start} reconsent={connect.reconsent} onDone={() => setConnect(null)} />;
  return (
    <Modal title="카드 초안 만들기" sub="고른 구간으로 ChatGPT가 초안을 만들어요. 승인하기 전에는 카드가 생기지 않아요." onClose={onClose} className="draft-send">
      <div className="draft-opts">
        <div className="draft-opt">
          <span className="draft-opt__label">
            <span className="t-label-m">만들 양</span>
            <span className="t-caption c-tertiary">구간마다 {PER[amount]}</span>
          </span>
          <Segment
            boxed
            label="만들 양"
            disabled={busy}
            value={amount}
            onChange={setAmount}
            options={[
              { value: "few", label: "적게" },
              { value: "normal", label: "보통" },
              { value: "many", label: "많이" },
            ]}
          />
        </div>
        <div className="draft-opt">
          <span className="t-label-m">카드 유형</span>
          <Segment
            boxed
            label="카드 유형"
            disabled={busy}
            value={cloze ? "cloze" : "basic"}
            onChange={(v) => setCloze(v === "cloze")}
            options={[
              { value: "basic", label: "질문과 답" },
              { value: "cloze", label: "빈칸" },
            ]}
          />
        </div>
      </div>
      {fail && (
        <AiErrorBox
          e={fail}
          copy={COPY(fail)}
          retryLabel="다시 만들기"
          onRetry={send}
          onClose={onClose}
          onReconnect={(reconsent) => setConnect({ start: "wait", reconsent })}
        />
      )}
      {rows && (
        <SendList
          locked={busy}
          rows={sentRows(rows, (id) => setRows(rows.filter((r) => r.id !== id)))}
          foot={
            <p className="ai-send__foot t-caption c-tertiary">
              <Icon name="info" size={14} />
              그 밖에 Neo-Anki가 붙이는 카드 형식 안내문만 함께 가요. 덱 이름·다른 카드·복습 기록은 보내지 않아요.
            </p>
          }
        />
      )}
      <div className="draft-send__foot">
        {busy ? (
          <span className="ai-busy">
            <Spinner size={16} />
            <span className="t-label-m c-secondary">{slow ? "오래 걸려요 · [취소]로 멈출 수 있어요" : "ChatGPT가 초안을 만드는 중…"}</span>
          </span>
        ) : (
          <UsingPlan />
        )}
        <span className="draft-send__actions">
          {/* 보내는 중에는 요청만 멈추고 창은 둔다 (고른 구간 그대로) */}
          <Button variant="ghost" size="M" label="취소" onClick={busy ? stop : onClose} />
          <Button size="M" icon="send" label="초안 만들기" busy={busy} busyLabel="만드는 중…" disabled={!rows?.length} onClick={send} />
        </span>
      </div>
    </Modal>
  );
}

type Item = DraftCard & { key: number; on: boolean; level: Level };
const itemsOf = (d: Draft): Item[] => d.cards.map((c, i) => ({ ...c, key: i, on: !c.similar, level: c.importance }));
const short = (s: string, max = 24) => (s.length > max ? `${s.slice(0, max)}…` : s);
/** 핵심 → 일반 → 보충 → 핵심 */
const NEXT_LEVEL: Record<Level, Level> = { 2: 1, 1: 0, 0: 2 };

/** "3장_빅데이터분석.pdf 13–14쪽에서" (자료가 여럿이면 "… 외 N개") */
function whereFrom(sent: Sent[]) {
  const first = sent[0];
  if (!first) return "";
  const same = sent.filter((s) => s.material_id === first.material_id).map((s) => s.page);
  const [lo, hi] = [Math.min(...same), Math.max(...same)];
  const unit = first.kind === "pptx" ? "장" : "쪽";
  const others = new Set(sent.map((s) => s.material_id)).size - 1;
  return `${first.material} ${lo === hi ? lo : `${lo}–${hi}`}${unit}${others ? ` 외 ${others}개` : ""}에서`;
}

type Panel = { seg: number } | { card: number };

/** R1-c 초안 검토. 승인하기 전에는 아무것도 바뀌지 않는다 */
export function Drafts(props: { deckId: number; materialId: number; draft: Draft; picked: number[]; amount: DraftAmount; cloze: boolean }) {
  const nav = useNav();
  const toast = useToast();
  const [draft, setDraft] = useState(props.draft);
  const [opts, setOpts] = useState({ picked: props.picked, amount: props.amount, cloze: props.cloze });
  const [items, setItems] = useState(() => itemsOf(props.draft));
  const [view, setView] = useState<"all" | "similar" | "ungrounded">("all");
  const [editing, setEditing] = useState<number | null>(null);
  const [panel, setPanel] = useState<Panel | null>(null);
  const [deckId, setDeckId] = useState(draft.deck_id);
  const [asking, setAsking] = useState(false);
  const [resend, setResend] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const back = useCallback(() => nav({ name: "sources", deckId: props.deckId, materialId: props.materialId, gaps: true }), [nav, props.deckId, props.materialId]);
  const leave = () => (items.length ? setAsking(true) : back());
  useKey("Escape", () => (panel ? setPanel(null) : leave()), editing === null && !asking && !resend);

  const seg = (id: number) => draft.sent.find((s) => s.id === id);
  const set = (key: number, patch: Partial<Item>) => setItems((list) => list.map((x) => (x.key === key ? { ...x, ...patch } : x)));
  const similar = items.filter((x) => x.similar).length;
  const ungrounded = items.filter((x) => x.ungrounded.length).length;
  const shown = items.filter((x) => (view === "similar" ? x.similar : view === "ungrounded" ? x.ungrounded.length : true));
  const chosen = items.filter((x) => x.on && x.front.trim() && (opts.cloze || x.back.trim()));

  const apply = async () => {
    setBusy(true);
    try {
      const change = await ragApi.apply(deckId, opts.cloze, chosen.map((x) => ({ front: x.front, back: x.back, segment_id: x.segment_id, importance: x.level })));
      toast({
        title: `카드 ${n(chosen.length)}장을 만들었어요`,
        text: "자료 구간과 이어 두었어요.",
        tone: "undo",
        action: {
          label: "되돌리기",
          onClick: () => qualityApi.undo(change).then(() => toast({ title: "카드 만들기를 되돌렸어요" }), (e) => toast({ title: "되돌리지 못했어요", text: errorText(e), tone: "error" })),
        },
      });
      back();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <main className={`drafts ${panel ? "has-panel" : ""}`}>
      <button className="backlink t-label-m c-secondary" onClick={leave}>
        <Icon name="chevron-left" size={18} />
        학습 자료 · 미연결 강조 구간
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">카드 초안 {n(items.length)}장</h1>
          <p className="t-body-s c-secondary">{whereFrom(draft.sent)} · ChatGPT 제안 · 승인한 카드만 만들어요</p>
        </div>
        {/* 기기 안 검사를 하지 않았으면 0 이 아니라 모르는 것이라 거르기 칸을 숨긴다 */}
        <Segment
          boxed
          label="초안 거르기"
          value={view}
          onChange={setView}
          options={[
            { value: "all" as const, label: `모두 ${n(items.length)}` },
            ...(draft.checked
              ? [
                  { value: "similar" as const, label: `비슷한 카드 있음 ${n(similar)}` },
                  { value: "ungrounded" as const, label: `근거와 다름 ${n(ungrounded)}` },
                ]
              : []),
          ]}
        />
      </div>
      {draft.missing.length > 0 && (
        <p className="drafts__note t-body-s c-secondary">
          <Icon name="info" size={16} />
          초안이 안 나온 구간 {n(draft.missing.length)} · {draft.missing.map((id) => seg(id)).filter(Boolean).map((s) => `p.${s!.page}`).join(", ")} · 다시 만들거나 [기존 카드 연결]로 직접 연결해 주세요.
        </p>
      )}
      {!draft.checked && <p className="t-caption c-tertiary">분석 모델이 없어 비슷한 카드·근거 검사는 하지 않았어요. 중요도 화면에서 모델을 받을 수 있어요.</p>}

      <div className="drafts__body">
        <div className="drafts__list">
          {shown.map((x) => {
            const s = seg(x.segment_id);
            const on = editing === x.key;
            return (
              <div key={x.key} className={`draft ${x.on ? "" : "is-off"} ${on ? "is-editing" : ""}`}>
                <span className="draft__check">
                  <Checkbox quiet label={`${x.front} 만들기`} checked={x.on} onChange={(v) => set(x.key, { on: v })} />
                </span>
                <div className="draft__main">
                  {on ? (
                    <div className="draft__faces">
                      <input
                        className="split__input t-body-m-strong"
                        value={x.front}
                        aria-label="앞면"
                        autoFocus
                        onChange={(e) => set(x.key, { front: e.target.value })}
                        onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setEditing(null))}
                      />
                      <textarea
                        className="split__input split__textarea t-body-s"
                        value={x.back}
                        aria-label="뒷면"
                        rows={Math.max(2, x.back.split("\n").length)}
                        onChange={(e) => set(x.key, { back: e.target.value })}
                        onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setEditing(null))}
                      />
                    </div>
                  ) : (
                    <button type="button" className="draft__faces" aria-label="이 초안 고치기" onClick={() => setEditing(x.key)}>
                      <span className="draft__front t-body-m-strong">{x.front}</span>
                      <span className="draft__back t-body-s">{x.back}</span>
                    </button>
                  )}
                  <span className="draft__chips">
                    {s && (
                      <button type="button" className="src-chip t-label-s" onClick={() => setPanel({ seg: s.id })}>
                        <Icon name="file" size={12} />
                        {s.material} p.{s.page}
                        {s.signals[0] ? ` · ${SIGNAL_LABEL[s.signals[0]]}` : ""}
                      </button>
                    )}
                    <button
                      type="button"
                      className={`lvl-chip t-label-s ${x.level === 2 ? "is-core" : ""}`}
                      aria-label={`중요도 ${LEVEL_LABEL[x.level]} 제안, 눌러서 바꾸기`}
                      onClick={() => set(x.key, { level: NEXT_LEVEL[x.level] })}
                    >
                      {LEVEL_LABEL[x.level]} 제안
                    </button>
                    {x.similar && (
                      <button type="button" className="warn-chip t-label-s" onClick={() => setPanel({ card: x.similar!.card_id })}>
                        <Icon name="layers" size={12} />
                        비슷한 카드가 있어요 · "{short(x.similar.question)}"
                      </button>
                    )}
                    {x.ungrounded.length > 0 && (
                      <button type="button" className="warn-chip t-label-s" title={x.ungrounded.join("\n")} onClick={() => setPanel({ seg: x.segment_id })}>
                        <Icon name="alert" size={12} />
                        근거 구간에 없는 말이 있어요 · "{short(x.ungrounded[0])}"
                      </button>
                    )}
                  </span>
                </div>
                <span className="draft__tools">
                  <button className="split__tool c-secondary" aria-label={on ? "고치기 끝" : "고치기"} aria-pressed={on} onClick={() => setEditing(on ? null : x.key)}>
                    <Icon name={on ? "check" : "pencil"} size={16} />
                  </button>
                  <button className="split__tool c-secondary" aria-label="이 초안 빼기" onClick={() => setItems(items.filter((y) => y.key !== x.key))}>
                    <Icon name="x" size={16} />
                  </button>
                </span>
              </div>
            );
          })}
          {shown.length === 0 && <p className="drafts__empty t-body-s c-tertiary">{items.length ? "이 보기에 맞는 초안이 없어요" : "남은 초안이 없어요"}</p>}
        </div>
        {panel && ("seg" in panel ? <SegmentPanel key={panel.seg} segmentId={panel.seg} onClose={() => setPanel(null)} /> : <CardPeek key={panel.card} cardId={panel.card} onClose={() => setPanel(null)} />)}
      </div>

      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="drafts__bar">
        <span className="drafts__bar-left">
          <span className="t-label-m">{n(chosen.length)}장 고름</span>
          <DeckPicker rootDeckId={draft.root_deck_id} value={deckId} onChange={(d) => setDeckId(d.id)} relative prefix="넣을 곳" />
          <span className="t-caption c-tertiary">중요도는 제안대로 · 바꾸려면 칩을 눌러요</span>
        </span>
        <span className="drafts__bar-right">
          <Button variant="ghost" size="M" icon="rotate" label="다시 만들기" onClick={() => setResend(true)} />
          <Button size="M" icon="check" label={`${n(chosen.length)}장 만들기`} disabled={!chosen.length} busy={busy} busyLabel="만드는 중…" onClick={apply} />
        </span>
      </div>

      {asking && (
        <Modal title={`초안 ${n(items.length)}장을 버릴까요?`} onClose={() => setAsking(false)} variant="confirm" icon="trash">
          <p className="t-body-m c-secondary">초안은 이 화면에만 있어요. 다시 만들려면 ChatGPT 요금제 사용량을 다시 써요.</p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="계속 보기" shortcut="Esc" onClick={() => setAsking(false)} />
            <Button variant="danger" size="M" icon="trash" label="버리기" onClick={back} />
          </div>
        </Modal>
      )}
      {/* [다시 만들기]: 사용량을 쓰므로 같은 미리보기(R1-b)를 다시 거친다 */}
      {resend && (
        <DraftSend
          picked={opts.picked}
          amount={opts.amount}
          cloze={opts.cloze}
          onClose={() => setResend(false)}
          onDone={(d, picked, amount, cloze) => {
            setResend(false);
            setDraft(d);
            setItems(itemsOf(d));
            setOpts({ picked, amount, cloze });
            setDeckId(d.deck_id);
            setPanel(null);
            setEditing(null);
          }}
        />
      )}
    </main>
  );
}

/** "비슷한 카드가 있어요"를 누르면 그 카드 (I2 패널 자리) */
function CardPeek({ cardId, onClose }: { cardId: number; onClose: () => void }) {
  const [view, setView] = useState<{ v: CardView; front: string; back: string } | null>(null);
  useEffect(() => {
    // 뒷면 템플릿은 앞면을 다시 담는다 ({{FrontSide}}<hr id=answer>). 답 부분만
    const answer = (html: string) => html.split(/<hr id=["']?answer["']?\s*\/?>/i).pop() ?? html;
    api.cardView(cardId).then(async (v) => setView({ v, front: await renderCardHtml(v.front), back: await renderCardHtml(answer(v.back)) }), () => setView(null));
  }, [cardId]);
  if (!view) return null;
  return (
    <aside className="src-panel" aria-label="비슷한 카드">
      <div className="src-panel__head">
        <span className="src-panel__title">
          <span className="t-title-s">비슷한 카드</span>
          <span className="t-caption c-tertiary">{view.v.deck_path.join(" › ")}</span>
        </span>
        <button className="src-icon-btn" aria-label="닫기" onClick={onClose}>
          <Icon name="x" size={18} />
        </button>
      </div>
      <div className="src-panel__text">
        <div className="t-body-m card-html" dangerouslySetInnerHTML={{ __html: view.front }} />
        <div className="t-body-s c-secondary card-html" dangerouslySetInnerHTML={{ __html: view.back }} />
      </div>
      <p className="t-caption c-tertiary">같은 내용이면 이 초안은 체크를 끈 채 두세요.</p>
    </aside>
  );
}
