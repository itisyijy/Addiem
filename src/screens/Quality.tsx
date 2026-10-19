// G1 카드 점검 · G2 나누기 제안 · 8.2 답이 안 맞아 보임 · 서식 제안 (CQ1–CQ3). 검사는 규칙으로만 한다 (모델 없음, 기기 안에서).
// ChatGPT 요금제는 누를 때만: G2 성격별로 묶기 · CQ3 AI로 확인 (ai.tsx).
// 이 화면은 카드를 바꾸지 않는다. 바뀌는 건 G2 · CQ2 · CQ3 에서 승인했을 때뿐이고, 한 번의 변경으로 남아 되돌릴 수 있다
import "../styles/quality.css";
import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { aiApi, aiError, api, errorText, qualityApi, type AiError, type CardChange, type CardCheck, type QualityCard, type QualityFlag, type Rewrite, type SplitPart, type SplitProposal } from "../api";
import { n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { Badge, Button, Checkbox, Icon, Spinner } from "../ui";
import { AiConnect, AiErrorBox, ScopeLine, SendList, UsingPlan, refreshAi, sendConfirmed, sentRows, setSendConfirmed, useAiStatus } from "../ai";
import { placeOf, ragApi, type Sent } from "../sourceApi";
import { SegmentPanel } from "./SourcePanel";
import { DiscardModal, Modal } from "./NewDeck";
import { splitLater } from "../written";

type View = "all" | "mismatch" | "format" | "again" | "long" | "multi";

const has = (c: QualityCard, ...kinds: QualityFlag["kind"][]) => c.flags.some((f) => kinds.includes(f.kind));
const SHAPE: QualityFlag["kind"][] = ["lines", "items", "questions"];
// Again 은 모양 문제와 겹칠 때만 붙는다 (quality_check). 그래서 again = "자주 틀리는 긴 카드". 탭 순서는 CQ1
const VIEWS: { value: View; label: string; test: (c: QualityCard) => boolean }[] = [
  { value: "all", label: "모두", test: () => true },
  { value: "mismatch", label: "답이 안 맞아 보임", test: (c) => has(c, "mismatch") },
  { value: "format", label: "서식 제안", test: (c) => has(c, "format") },
  { value: "again", label: "자주 틀리는 긴 카드", test: (c) => has(c, "again") },
  { value: "long", label: "답이 긴 카드", test: (c) => has(c, "lines", "items") },
  { value: "multi", label: "질문이 여러 개", test: (c) => has(c, "questions") },
];

/** 노랑 = 카드 모양 · 답이 안 맞아 보임, 회색 = 물음 개수 · 센 사실, 보라 = 서식 제안, 빨강 = 학습 결과 (G1 · CQ1 노트) */
function FlagBadge({ f }: { f: QualityFlag }) {
  if (f.kind === "items") return <Badge tone="warn">답 {n(f.n)}항목</Badge>;
  if (f.kind === "lines") return <Badge tone="warn">답 {n(f.n)}줄</Badge>;
  if (f.kind === "questions") return <Badge>질문 {n(f.n)}개</Badge>;
  if (f.kind === "mismatch") return <Badge tone="warn">답이 안 맞아 보여요</Badge>;
  if (f.kind === "format")
    return (
      <>
        <Badge tone="accent">서식 제안</Badge>
        {f.badge && <Badge>{f.badge}</Badge>}
      </>
    );
  return <Badge tone="danger">Again {n(f.n)}번</Badge>;
}

const CRITERIA = `검사 기준 · 답이 8줄 또는 항목 6개 이상 · 질문에 물음표나 "와/과 … 은?"이 2개 이상 · 최근 14일 Again 2번 이상`;
const CRITERIA_FIX = `검사 기준 · 답이 안 맞아 보임: 질문이 묻는 꼴(차이 · 이유 · 순서 · 개수 · 수)과 답의 꼴이 다르거나, 답이 비었거나 질문을 되풀이해요 · 서식 제안: 한 줄에 번호 3개 이상, "이름:"이 한 줄에 2번 이상, 또는 긴 줄에 쉼표로 이은 짧은 항목 3개 이상`;
/** "최근 고친 카드" 둘째 줄 (CQ1) */
const HOW: Record<NonNullable<CardChange["how"]>, string> = { format: "서식 고치기", ai: "AI로 확인한 뒤 고침", edit: "직접 고침" };

function ago(ms: number) {
  const m = Math.floor((Date.now() - ms) / 60_000);
  if (m < 1) return "방금";
  if (m < 60) return `${m}분 전`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}시간 전`;
  const d = Math.floor(h / 24);
  return d === 1 ? "어제" : `${d}일 전`;
}

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
      toast({ title: "나누기를 되돌렸어요", text: "나눈 카드를 지우고 원래 카드를 나누기 전 상태로 되돌렸어요." });
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
  const toast = useToast();
  const ai = useAiStatus();
  const [deckName, setDeckName] = useState("");
  const [cards, setCards] = useState<QualityCard[] | null>(null);
  const [changes, setChanges] = useState<CardChange[]>([]);
  const [view, setView] = useState<View | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [splitting, setSplitting] = useState<number | null>(null);
  // CQ2 서식 고치기 · CQ3 AI로 확인 (같은 비교 창) · [전후 보기]
  const [rewriting, setRewriting] = useState<{ cardId: number; mode: "format" | "ai" } | null>(null);
  const [viewing, setViewing] = useState<CardChange | null>(null);
  // 고친 뒤 또 고친 카드를 되돌리기 전에 묻는다 (CQ2 "그 뒤에 고친 내용이 있어요")
  const [forceUndo, setForceUndo] = useState<number | null>(null);
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
  useKey("Escape", back, splitting === null && rewriting === null && viewing === null && forceUndo === null);
  const { keep, undo, done: splitDone } = useSplitActions(load);

  /** 고쳐 쓰기 되돌리기. 그 뒤에 또 고쳤으면 확인을 받고 force 로 (확인 없이 덮는 길은 없다) */
  const undoRewrite = (changeId: number, force = false) =>
    qualityApi.undo(changeId, force).then(
      (ok) => {
        if (!ok) return setForceUndo(changeId);
        setForceUndo(null);
        setViewing(null);
        load();
        toast({ title: "고친 것을 되돌렸어요", text: "카드를 고치기 전 글로 돌아갔어요." });
      },
      (e) => toast({ title: "되돌리지 못했어요", text: errorText(e), tone: "error" }),
    );
  const rewriteDone = (changeId: number, how: "format" | "ai") => {
    setRewriting(null);
    load();
    toast({ title: how === "format" ? "서식을 고쳤어요" : "카드를 고쳤어요", tone: "undo", action: { label: "되돌리기", onClick: () => undoRewrite(changeId) } });
  };
  /** [카드 편집]: 카드 목록에서 그 카드의 편집 창(E)을 연다 */
  const edit = (cardId: number) => nav({ name: "cards", deckId, cardIds: [cardId], edit: cardId });

  if (!cards) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  const shown = cards.filter((c) => !c.hidden);
  const hidden = cards.filter((c) => c.hidden);
  // 기본 보기는 "자주 틀리는 긴 카드". 없으면 모두
  const current = view ?? (shown.some((c) => has(c, "again")) ? "again" : "all");
  const list = shown.filter(VIEWS.find((v) => v.value === current)!.test);

  const row = (c: QualityCard) => {
    const fixOnly = has(c, "mismatch") && !has(c, "format", ...SHAPE);
    return (
      <div key={c.id} className="quality__row">
        <div className="quality__text">
          <p className="t-body-m">{c.question}</p>
          <div className="quality__meta">
            <span className="t-caption c-tertiary">{c.path.join(" › ")}</span>
            {c.flags.map((f) => (
              <FlagBadge key={f.kind} f={f} />
            ))}
          </div>
          {c.flags.map(
            (f) =>
              f.reason && (
                <p key={f.kind} className="quality__reason t-caption c-secondary">
                  {f.kind === "format" ? "제안" : "근거"} · {f.reason}
                </p>
              ),
          )}
        </div>
        {c.hidden ? (
          <Button variant="ghost" size="M" label="다시 올리기" onClick={() => keep(c.id, false)} />
        ) : (
          <>
            <Button variant="ghost" size="M" label={fixOnly ? "맞아요 · 숨기기" : "30일간 점검에서 숨기기"} onClick={() => keep(c.id)} />
            {has(c, "format") && <Button variant="secondary" size="M" icon="pencil" label="서식 고치기" onClick={() => setRewriting({ cardId: c.id, mode: "format" })} />}
            {has(c, "mismatch") &&
              (ai?.connected ? (
                <Button variant="secondary" size="M" icon="sparkles" label="AI로 확인" onClick={() => setRewriting({ cardId: c.id, mode: "ai" })} />
              ) : (
                <Button variant="secondary" size="M" icon="pencil" label="카드 편집" onClick={() => edit(c.id)} />
              ))}
            {has(c, ...SHAPE) && <Button variant="secondary" size="M" icon="layers" label="나누기 제안" onClick={() => setSplitting(c.id)} />}
          </>
        )}
      </div>
    );
  };
  const done = changes.filter((c) => !c.undone && c.kind === "split");
  const fixed = changes.filter((c) => !c.undone && c.kind === "rewrite");
  const fixInfo = (current === "all" || current === "mismatch" || current === "format") && list.some((c) => has(c, "mismatch", "format"));

  return (
    <main className="cards quality">
      <button className="backlink t-label-m c-secondary" onClick={back}>
        <Icon name="chevron-left" size={18} />
        {deckName}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">카드 점검</h1>
          <p className="t-body-s c-secondary">답이 길거나, 질문과 답이 맞지 않아 보이거나, 줄을 나누면 읽기 쉬운 카드를 찾았어요. 고칠지는 직접 정해요.</p>
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
      {fixInfo && (
        <p className="quality__info t-body-s">
          <Icon name="info" size={16} />
          규칙으로 찾은 것이라 틀릴 수 있어요. 서식 제안은 내용은 그대로 두고 줄바꿈 · 굵게만 바꿔요. 고치기 전에 바뀌는 곳을 보여 드리고, 고친 뒤에도 되돌릴 수 있어요.
        </p>
      )}

      {list.length > 0 ? (
        <div className="panel quality__list">{list.map(row)}</div>
      ) : (
        <div className="panel quality__empty">
          <span className="t-title-s">{shown.length ? "이 보기에 맞는 카드가 없어요" : "고칠 만한 카드가 없어요"}</span>
          <span className="t-body-s c-secondary">답이 길거나, 질문과 답이 맞지 않아 보이거나, 서식을 고칠 카드가 생기면 여기에 모여요.</span>
        </div>
      )}

      {/* CQ1 "최근 고친 카드": 7일 동안 [전후 보기] [되돌리기] */}
      {fixed.length > 0 && (
        <section className="panel quality__fixed">
          <div className="quality__fixed-head">
            <h2 className="t-label-m">최근 고친 카드 {n(fixed.length)}</h2>
            <span className="t-caption c-tertiary">7일 동안 되돌릴 수 있어요</span>
          </div>
          {fixed.map((c) => (
            <div key={c.id} className="quality__row quality__row--change">
              <span className="quality__text">
                <span className="t-body-m">{c.summary}</span>
                <span className="t-caption c-tertiary">
                  {HOW[c.how ?? "edit"]} · {ago(c.created_at)}
                </span>
              </span>
              <Button variant="ghost" size="M" label="전후 보기" onClick={() => setViewing(c)} />
              <Button variant="ghost" size="M" icon="rotate" label="되돌리기" onClick={() => undoRewrite(c.id)} />
            </div>
          ))}
        </section>
      )}

      <p className="t-caption c-tertiary">{CRITERIA}</p>
      <p className="t-caption c-tertiary">{CRITERIA_FIX}</p>

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
      {rewriting !== null && (
        <RewriteModal
          cardId={rewriting.cardId}
          mode={rewriting.mode}
          onClose={() => setRewriting(null)}
          onKeep={() => {
            setRewriting(null);
            keep(rewriting.cardId);
          }}
          onEdit={() => edit(rewriting.cardId)}
          onDone={(id) => rewriteDone(id, rewriting.mode)}
        />
      )}
      {viewing !== null && <ChangeModal change={viewing} onClose={() => setViewing(null)} onUndo={() => undoRewrite(viewing.id)} />}
      {forceUndo !== null && (
        <DiscardModal
          title="그 뒤에 고친 내용이 있어요"
          text="되돌리면 그 뒤에 고친 내용도 사라져요."
          keep="그대로 두기"
          discard="되돌리기"
          onKeep={() => setForceUndo(null)}
          onDiscard={() => undoRewrite(forceUndo, true)}
        />
      )}
    </main>
  );
}

// ---------------------------------------------------------------- CQ2 · CQ3 고쳐 쓰기 전후 비교

/** 고쳐 쓰기 글자 (quality.rs tidy 와 같다): 줄마다 띄어쓰기를 하나로, 빈 줄은 뺀다 */
const tidy = (s: string) =>
  s
    .split("\n")
    .map((l) => l.trim().split(/\s+/).join(" "))
    .filter(Boolean)
    .join("\n");

/** 낱말 · 줄바꿈 조각. **…** 는 표시를 떼고 굵게 */
/** cut: 낱말 끝에서 뺀 쉼표 · 세미콜론 (쉼표 목록을 줄로 나눌 때) */
type Tok = { t: string; bold: boolean; cut?: string };
function tokens(text: string): Tok[] {
  const out: Tok[] = [];
  let bold = false;
  tidy(text)
    .split("\n")
    .forEach((line, i) => {
      if (i) out.push({ t: "\n", bold: false });
      for (let w of line.split(" ").filter(Boolean)) {
        if (w.startsWith("**")) (bold = true), (w = w.slice(2));
        const end = w.endsWith("**");
        if (end) w = w.slice(0, -2);
        if (w) out.push({ t: w, bold });
        if (end) bold = false;
      }
    });
  return out;
}

type Op = { tok: Tok; op: "same" | "del" | "add"; boldNow?: boolean };
/** 견줄 때는 끝 쉼표 · 세미콜론을 뺀다 ("하드웨어," 와 "하드웨어"는 같은 낱말, 뺀 쉼표만 따로 보인다) */
const key = (t: string) => t.replace(/[,;、，；]+$/, "") || t;
/** 낱말 단위 차이 (가장 긴 공통 부분). ponytail: O(n·m) 표, 카드 한 장 글이라 충분하다 */
function diffOps(before: string, after: string): Op[] {
  const a = tokens(before);
  const b = tokens(after);
  const L = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) L[i][j] = key(a[i].t) === key(b[j].t) ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
  const ops: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && key(a[i].t) === key(b[j].t)) {
      const cut = a[i].t.startsWith(b[j].t) ? a[i].t.slice(b[j].t.length) : "";
      ops.push({ tok: cut ? { ...b[j], cut } : b[j], op: "same", boldNow: b[j].bold && !a[i].bold });
      i++;
      j++;
    }
    else if (j >= b.length || (i < a.length && L[i + 1][j] >= L[i][j + 1])) ops.push({ tok: a[i], op: "del" }), i++;
    else ops.push({ tok: b[j], op: "add" }), j++;
  }
  return ops;
}

/** 고친 쪽 글: 줄 나눔 ↵ · 지운 글자 취소선 · 더한 글자 밑줄 · 굵게 (CQ2) */
function Diff({ before, after }: { before: string; after: string }) {
  const out: ReactNode[] = [];
  // 같은 종류(그대로 · 지움 · 더함)로 이어진 낱말은 한 덩어리로 (밑줄 · 취소선이 띄어쓰기까지 이어지게)
  const run: { op: Op["op"] | null; words: Tok[] } = { op: null, words: [] };
  let lineStart = true;
  const flush = () => {
    if (!run.op) return;
    const k = out.length;
    if (!lineStart) out.push(" ");
    const words = run.words.flatMap((w, i) => [i ? " " : "", w.bold ? <strong key={i}>{w.t}</strong> : w.t, w.cut ? <del key={`c${i}`}>{w.cut}</del> : ""]);
    out.push(run.op === "del" ? <del key={k}>{words}</del> : run.op === "add" ? <ins key={k}>{words}</ins> : <Fragment key={k}>{words}</Fragment>);
    run.op = null;
    run.words = [];
    lineStart = false;
  };
  for (const o of diffOps(before, after)) {
    if (o.tok.t === "\n") {
      flush();
      const k = out.length;
      if (o.op === "del") {
        out.push(" ", <del key={k} className="rewrite__nl">↵</del>);
        lineStart = false;
      } else {
        if (o.op === "add") out.push(" ", <span key={`${k}m`} className="rewrite__nl">↵</span>);
        out.push(<br key={k} />);
        lineStart = true;
      }
      continue;
    }
    if (run.op !== o.op) flush();
    run.op = o.op;
    run.words.push(o.tok);
  }
  flush();
  return <>{out}</>;
}

/** "바뀌는 곳: 뒷면 줄바꿈 6곳 · 글자는 그대로예요" (CQ2). 바뀐 낱말은 이어진 묶음 하나를 한 곳으로 센다 */
function changesLine(before: [string, string], after: [string, string]) {
  let words = 0;
  const sides = (["앞면", "뒷면"] as const).flatMap((side, s) => {
    let nl = 0;
    let bold = 0;
    let cut = 0;
    let w = 0;
    let run = false;
    for (const o of diffOps(before[s], after[s])) {
      const changed = o.op !== "same";
      if (o.tok.t === "\n") nl += changed && o.op === "add" ? 1 : 0;
      else if (changed && !run) w++;
      if (o.boldNow) bold++;
      if (o.tok.cut) cut++;
      run = changed && o.tok.t !== "\n";
    }
    words += w;
    const parts = [nl && `줄바꿈 ${n(nl)}곳`, bold && `굵게 ${n(bold)}곳`, cut && `쉼표 ${n(cut)}개 빼기`, w && `글자 ${n(w)}곳`].filter(Boolean);
    return parts.length ? [`${side} ${parts.join(" · ")}`] : [];
  });
  if (!sides.length) return "바뀌는 곳이 없어요";
  return `바뀌는 곳: ${sides.join(" · ")}${words ? "" : " · 글자는 그대로예요"}`;
}

/** 지금 카드 | → | 오른쪽 (G2 나누기 창과 같은 틀) */
function Compare({ before, children, label = "지금 카드" }: { before: [string, string]; children: ReactNode; label?: string }) {
  return (
    <div className="rewrite__cols">
      <section className="rewrite__card rewrite__card--before" aria-label={label}>
        <span className="t-label-s c-tertiary">{label}</span>
        <span className="t-caption c-tertiary">앞면</span>
        <p className="t-body-m-strong">{before[0]}</p>
        <span className="t-caption c-tertiary">뒷면</span>
        <p className="rewrite__text t-body-m">{before[1] || <span className="c-tertiary">비어 있어요</span>}</p>
      </section>
      <span className="rewrite__arrow c-tertiary">
        <Icon name="arrow-right" size={18} />
      </span>
      {children}
    </div>
  );
}

/** 고친 카드 (보기 · 손보기). 칸을 누르면 바로 고친다. 고친 곳 표시는 내가 쓴 글 기준으로 다시 그린다 (CQ3 다른 상태) */
function FixedCard({ before, value, onChange, locked, readOnly }: {
  before: [string, string];
  value: [string, string];
  onChange?: (v: [string, string]) => void;
  locked?: [boolean, boolean];
  readOnly?: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const field = (i: 0 | 1, label: string) => (
    <>
      <span className="t-caption c-tertiary">{label}</span>
      {editing ? (
        <>
          <textarea
            className="split__input split__textarea rewrite__input t-body-m"
            value={value[i]}
            disabled={locked?.[i]}
            aria-label={`고친 카드 ${label}`}
            rows={Math.max(2, value[i].split("\n").length)}
            autoFocus={i === 1}
            onChange={(e) => onChange?.(i ? [value[0], e.target.value] : [e.target.value, value[1]])}
            onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setEditing(false))}
          />
          {locked?.[i] && <span className="t-caption c-tertiary">그림 · 소리 · 서식이 있는 칸이라 카드 편집에서 고쳐 주세요</span>}
        </>
      ) : (
        <p className={`rewrite__text ${i ? "t-body-m" : "t-body-m-strong"}`}>
          <Diff before={before[i]} after={value[i]} />
        </p>
      )}
    </>
  );
  return (
    <section
      className={`rewrite__card rewrite__card--after ${editing ? "is-editing" : ""}`}
      aria-label="고친 카드"
      onClick={() => !readOnly && !editing && setEditing(true)}
    >
      <span className="rewrite__head">
        <span className="t-label-s c-tertiary">고친 카드</span>
        {!readOnly && (
          <button
            type="button"
            className="split__tool c-secondary"
            aria-label={editing ? "손보기 끝" : "고친 카드 손보기"}
            aria-pressed={editing}
            onClick={(e) => (e.stopPropagation(), setEditing(!editing))}
          >
            <Icon name={editing ? "check" : "pencil"} size={16} />
          </button>
        )}
      </span>
      {field(0, "앞면")}
      {field(1, "뒷면")}
    </section>
  );
}

/** 근거 줄 (CQ3): 고친 안이 기댄 자료 구간 + [자료 열기]. 자료로 확인하지 못했으면 주황 한 줄 */
function Evidence({ check, onOpen }: { check: CardCheck; onOpen: (id: number) => void }) {
  if (!check.evidence.length)
    return (
      <p className="rewrite__evidence is-warn t-body-s">
        <Icon name="alert" size={16} />
        자료로 확인하지 못했어요 · ChatGPT가 일반 지식으로 고쳤어요
      </p>
    );
  return (
    <>
      {check.evidence.map((s) => (
        <div key={s.id} className="rewrite__evidence">
          <Icon name="file" size={16} />
          <span className="rewrite__evidence-text">
            <span className="t-label-m">근거 · {placeOf(s)}</span>
            <span className="t-body-s c-secondary">"{s.text}"</span>
          </span>
          <Button variant="ghost" size="M" label="자료 열기" onClick={() => onOpen(s.id)} />
        </div>
      ))}
    </>
  );
}

/**
 * CQ2 서식 고치기 · CQ3 AI로 확인. 승인([이대로 고치기]) 전에는 아무것도 바뀌지 않는다.
 * AI 는 누를 때만 보내고(row [AI로 확인]), 처음 보낼 때 한 번 보낼 내용을 묻는다 (L4 · R3 와 같은 확인, 연결 구간은 뺄 수 있다)
 */
function RewriteModal({ cardId, mode, onClose, onKeep, onEdit, onDone }: {
  cardId: number;
  mode: "format" | "ai";
  onClose: () => void;
  onKeep: () => void;
  onEdit: () => void;
  onDone: (changeId: number) => void;
}) {
  const [r, setR] = useState<Rewrite | null>(null);
  const [value, setValue] = useState<[string, string]>(["", ""]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  // CQ3
  const [segs, setSegs] = useState<Sent[] | null>(mode === "ai" ? null : []);
  const [asking, setAsking] = useState(false);
  const [peek, setPeek] = useState(false);
  const [check, setCheck] = useState<CardCheck | null>(null);
  const [aiBusy, setAiBusy] = useState(false);
  const [aiFail, setAiFail] = useState<AiError | null>(null);
  const [connect, setConnect] = useState<{ start: "intro" | "wait"; reconsent: boolean } | null>(null);
  const [panelSeg, setPanelSeg] = useState<number | null>(null);
  const seq = useRef(0);
  const inFlight = useRef(false);
  const started = useRef(false);

  useEffect(() => {
    qualityApi.rewrite(cardId).then(
      (x) => {
        setR(x);
        setValue([x.new_question, x.new_answer]);
      },
      (e) => setError(errorText(e)),
    );
    if (mode === "ai") ragApi.linked(cardId).then(setSegs, () => setSegs([]));
  }, [cardId, mode]);
  // 창을 닫으면 보내던 요청도 멈춘다
  useEffect(
    () => () => {
      if (inFlight.current) aiApi.cancel().catch(() => {});
    },
    [],
  );

  const send = (ids: number[]) => {
    setSendConfirmed(true, ids.length);
    setAsking(false);
    const my = ++seq.current;
    inFlight.current = true;
    setAiFail(null);
    setCheck(null);
    setAiBusy(true);
    aiApi.checkCard(cardId, ids).then(
      (x) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        setAiBusy(false);
        refreshAi();
        setCheck(x);
        if (x.fix) setValue([x.fix.question, x.fix.answer]);
      },
      (e) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        setAiBusy(false);
        refreshAi();
        const f = aiError(e);
        if (f.kind === "cancelled") return onClose();
        if (f.kind === "not_connected") return setConnect({ start: "intro", reconsent: false });
        setAiFail(f);
      },
    );
  };
  // [AI로 확인]을 눌러 연 창: 카드와 연결 구간을 읽은 뒤 한 번 보낸다 (처음이면 먼저 묻는다)
  useEffect(() => {
    if (mode !== "ai" || !r || segs === null || started.current) return;
    started.current = true;
    if (sendConfirmed(segs.length)) send(segs.map((s) => s.id));
    else setAsking(true);
  }, [mode, r, segs]);
  const stop = () => {
    if (inFlight.current) aiApi.cancel().catch(() => {});
    inFlight.current = false;
    seq.current++;
    onClose();
  };

  if (connect)
    return (
      <AiConnect
        start={connect.start}
        reconsent={connect.reconsent}
        onDone={() => {
          setConnect(null);
          onClose();
        }}
      />
    );
  if (!r)
    return error ? (
      <Modal title="카드 고치기" onClose={onClose} className="split rewrite">
        <p className="t-body-s c-danger">{error}</p>
      </Modal>
    ) : null;

  const before: [string, string] = [r.question, r.answer];
  const changed = tidy(value[0]) !== r.question || tidy(value[1]) !== r.answer;
  const apply = () => {
    setBusy(true);
    qualityApi.rewriteApply(cardId, value[0], value[1], mode).then(onDone, (e) => {
      setError(errorText(e));
      setBusy(false);
    });
  };
  const fix = mode === "format" || !!check?.fix;
  const noFix = mode === "ai" && !!check && !check.fix && !check.matches;
  const sentCount = (check?.sent ?? segs ?? []).length;
  const sentLine = `보낸 것 · 질문 · 답${sentCount ? ` · 연결된 자료 ${n(sentCount)}곳` : ""}`;
  const title = mode === "format" ? "서식 고치기" : check?.fix ? "답 고치기 제안 · ChatGPT" : "답 확인 · ChatGPT";
  const sub =
    mode === "format" ? (
      "규칙으로 고친 제안이에요. 승인하기 전에는 아무것도 바뀌지 않아요."
    ) : check ? (
      <>
        <button type="button" className="rewrite__sent" aria-expanded={peek} onClick={() => setPeek(!peek)}>
          {sentLine}
        </button>
        {check.fix ? ". 승인하기 전에는 아무것도 바뀌지 않아요." : ". 아무것도 바뀌지 않았어요."}
      </>
    ) : (
      "질문과 답을 ChatGPT로 확인해요. 승인하기 전에는 아무것도 바뀌지 않아요."
    );
  const sendRows = (list: Sent[], removable: boolean) => [
    { key: "card", icon: "layers" as const, meta: <span className="t-label-s c-secondary">카드 앞·뒷면</span>, text: `${r.question} / ${r.answer.split("\n").join(" · ")}` },
    ...sentRows(list, removable ? (id) => setSegs((segs ?? []).filter((s) => s.id !== id)) : () => {}, true),
  ];

  let right: ReactNode;
  if (mode === "ai" && asking)
    right = (
      <div className="rewrite__card rewrite__card--plain ai-confirm--list" role="dialog" aria-label="ChatGPT로 보낼 내용">
        <SendList
          compact
          title={segs?.length ? `이 카드와 자료 ${n(segs.length)}구간을 ChatGPT로 보내요` : "이 카드 한 장을 ChatGPT로 보내요"}
          rows={sendRows(segs ?? [], true)}
          foot={
            <div className="grade__confirm-foot">
              <UsingPlan />
              <span className="grade__actions">
                <Button variant="ghost" size="M" label="취소" onClick={onClose} />
                <Button size="M" icon="send" label="보내기" autoFocus onClick={() => send((segs ?? []).map((s) => s.id))} />
              </span>
            </div>
          }
        />
      </div>
    );
  else if (mode === "ai" && (aiBusy || (!check && !aiFail)))
    right = (
      <section className="rewrite__card rewrite__card--plain" aria-busy>
        <span className="ai-busy">
          <Spinner size={16} />
          <span className="t-label-m c-secondary">ChatGPT가 확인하는 중…</span>
          <button type="button" className="ai-link t-label-s" onClick={stop}>
            취소
          </button>
        </span>
        {[0.78, 0.66].map((w, i) => (
          <div key={i} className="ai-skel" aria-hidden>
            <span style={{ width: `${w * 100}%` }} />
            <span style={{ width: `${w * 80}%` }} />
          </div>
        ))}
      </section>
    );
  else if (mode === "ai" && aiFail)
    right = (
      <section className="rewrite__card rewrite__card--plain">
        <AiErrorBox
          e={aiFail}
          onClose={onClose}
          onRetry={() => send((segs ?? []).map((s) => s.id))}
          onReconnect={(reconsent) => setConnect({ start: "wait", reconsent })}
          retryLabel="다시 확인"
          copy={{
            bad_output: ["ChatGPT 답을 쓰지 못했어요", "형식이 맞지 않아 쓰지 않았어요. 카드는 그대로예요."],
            offline: ["인터넷에 연결되어 있지 않아요", "카드는 그대로예요. 연결되면 다시 확인해 주세요."],
            unavailable: ["지금은 ChatGPT 요금제를 쓸 수 없어요", "잠시 뒤 다시 해 보세요. 카드는 그대로예요."],
            error: ["ChatGPT로 확인하지 못했어요", aiFail.message],
          }}
        />
      </section>
    );
  else if (noFix && check)
    right = (
      <section className="rewrite__card rewrite__card--check" aria-label="확인해 주세요">
        <span className="t-label-s c-tertiary">확인해 주세요</span>
        {(check.checks.length ? check.checks : ["질문과 답을 직접 확인해 주세요."]).map((c, i) => (
          <p key={i} className="t-body-m">
            {check.checks.length ? `${i + 1}. ` : ""}
            {c}
          </p>
        ))}
        <span className="t-caption c-secondary">자료를 연결하면 그 근거로 다시 확인할 수 있어요.</span>
        <span className="rewrite__actions">
          <Button variant="secondary" size="M" label="카드 편집" onClick={onEdit} />
        </span>
      </section>
    );
  else if (mode === "ai" && check?.matches)
    right = (
      <section className="rewrite__card rewrite__card--check" aria-label="ChatGPT 판단">
        <span className="t-label-s c-tertiary">고칠 곳이 없어요</span>
        <p className="t-body-m">ChatGPT는 이 답이 질문에 맞는다고 봤어요. 규칙이 잘못 짚었을 수 있어요.</p>
      </section>
    );
  else right = <FixedCard before={before} value={value} onChange={setValue} locked={r.locked} />;

  return (
    <>
      <Modal title={title} sub={sub} onClose={stop} className={`split rewrite ${panelSeg !== null ? "has-panel" : ""}`}>
        {mode === "format" ? (
          <p className="quality__info split__why t-body-s">
            <Icon name="info" size={16} />
            {changesLine(before, value)}
          </p>
        ) : check?.matches ? (
          <p className="quality__info split__why t-body-s">
            <Icon name="check" size={16} />
            질문에 맞는 답 같아요. {check.reason}
          </p>
        ) : noFix && check ? (
          <p className="rewrite__warn t-body-s">
            <Icon name="alert" size={16} />
            수정안을 만들지 않았어요 · 근거가 부족해요. {check.reason}
          </p>
        ) : check?.fix ? (
          <p className="quality__info split__why t-body-s">
            <Icon name="sparkles" size={16} />
            왜 고치나요? {check.reason}
          </p>
        ) : (
          r.why && (
            <p className="quality__info split__why t-body-s">
              <Icon name="info" size={16} />
              근거 · {r.why}
            </p>
          )
        )}
        {peek && check && (
          <div className="ai-confirm ai-confirm--list rewrite__peek" role="dialog" aria-label="보낸 것" onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setPeek(false))}>
            <SendList compact title="보낸 것" rows={sendRows(check.sent, false)} sent foot={<Button variant="ghost" size="M" label="닫기" autoFocus onClick={() => setPeek(false)} />} />
          </div>
        )}
        <Compare before={before}>{right}</Compare>
        {mode === "ai" && check?.fix && <Evidence check={check} onOpen={setPanelSeg} />}
        {fix && (
          <p className="t-caption c-tertiary">
            {mode === "format" ? (
              <>
                <span className="rewrite__nl">↵</span> 줄을 나눈 곳 · 지운 글자는 취소선, 더한 글자는 밑줄로 보여요
              </>
            ) : (
              "지운 글자는 취소선, 더한 글자는 밑줄 · 근거에 없는 내용은 넣지 않게 했어요"
            )}
          </p>
        )}
        {error && <p className="t-body-s c-danger">{error}</p>}
        <div className="split__foot">
          <span className="t-caption c-secondary">
            {fix ? `고친 뒤 7일 동안 카드 점검의 "최근 고친 카드"에서 되돌릴 수 있어요` : noFix ? "근거 없이 고친 답은 만들지 않아요" : ""}
          </span>
          <span className="split__actions">
            {fix ? (
              <>
                <Button variant="ghost" size="M" label={mode === "format" ? "고치지 않기" : "그대로 두기"} onClick={stop} />
                <Button size="M" icon="check" label="이대로 고치기" disabled={!changed} busy={busy} busyLabel="고치는 중…" onClick={apply} />
              </>
            ) : check && !check.fix ? (
              <>
                <Button variant="ghost" size="M" label="맞아요 · 숨기기" onClick={onKeep} />
                <Button size="M" label="닫기" onClick={onClose} />
              </>
            ) : (
              !asking && <Button variant="ghost" size="M" label="닫기" onClick={stop} />
            )}
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

/** [전후 보기]: 고친 변경의 전후 (읽기만). [되돌리기]는 목록과 같은 길 */
function ChangeModal({ change, onClose, onUndo }: { change: CardChange; onClose: () => void; onUndo: () => void }) {
  const [r, setR] = useState<Rewrite | null>(null);
  const [error, setError] = useState<string>();
  useEffect(() => {
    qualityApi.rewriteChange(change.id).then(setR, (e) => setError(errorText(e)));
  }, [change.id]);
  if (!r && !error) return null;
  return (
    <Modal title="고친 전후" sub={`${HOW[change.how ?? "edit"]} · ${ago(change.created_at)}`} onClose={onClose} className="split rewrite">
      {r ? (
        <Compare before={[r.question, r.answer]} label="고치기 전">
          <FixedCard before={[r.question, r.answer]} value={[r.new_question, r.new_answer]} readOnly />
        </Compare>
      ) : (
        <p className="t-body-s c-danger">{error}</p>
      )}
      <div className="split__foot">
        <span className="t-caption c-secondary">7일 동안 되돌릴 수 있어요</span>
        <span className="split__actions">
          <Button variant="ghost" size="M" label="닫기" onClick={onClose} />
          <Button variant="secondary" size="M" icon="rotate" label="되돌리기" onClick={onUndo} />
        </span>
      </div>
    </Modal>
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
  const [discard, setDiscard] = useState<(() => void) | null>(null); // 고친 카드를 버리고 할 일 (묻는 중)
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
    // 규칙 제안을 고치다가 보내기 확인만 닫는 것이면 고친 것을 그대로 둔다
    if (p && mode === "ai") setParts(p.parts);
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
  // 보이는 제안에서 고친 것(질문 · 답 · 빼기 · 더하기)이 있으면 다른 제안으로 바꾸기 전에 묻는다 (QA T5-02)
  const edited = listed && JSON.stringify(parts) !== JSON.stringify(shown.parts);
  const guard = (f: () => void) => () => (edited ? setDiscard(() => f) : f());
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
              {placeOf(s)}
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
              {mode === "ai" && aiFail && <AiErrorBox e={aiFail} onRule={guard(toRule)} onRetry={guard(send)} onReconnect={(reconsent) => setConnect({ start: "wait", reconsent })} />}
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
                    : sentRows(aiP?.sent ?? [], () => {}, true)),
                ]}
                sent={!asking}
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
            <button role="radio" aria-checked={!aiSel} className={`segment__item t-label-m ${aiSel ? "" : "is-selected"}`} onClick={mode === "ai" ? guard(toRule) : toRule}>
              규칙으로 나누기
            </button>
            <button role="radio" aria-checked={aiSel} className={`segment__item t-label-m ${aiSel ? "is-selected" : ""}`} onClick={aiSel ? pickAi : guard(pickAi)}>
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
                <button type="button" className="ai-mini t-label-s" onClick={guard(send)}>
                  <Icon name="rotate" size={14} />
                  다시 나누기
                </button>
              )
            ) : (
              <button type="button" className="ai-mini t-label-s" disabled={nextPer === null} onClick={guard(() => nextPer && suggest(nextPer))}>
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
    {discard && (
      <DiscardModal
        title="고친 카드가 있어요"
        text="다른 제안으로 바꾸면 고친 질문과 답은 사라져요."
        keep="계속 고치기"
        discard="버리고 바꾸기"
        onKeep={() => setDiscard(null)}
        onDiscard={() => {
          setDiscard(null);
          discard();
        }}
      />
    )}
    </>
  );
}
