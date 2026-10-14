// G1 카드 점검 · G2 나누기 제안. 검사는 규칙으로만 한다 (모델 없음, 기기 안에서).
// 이 화면은 카드를 바꾸지 않는다. 바뀌는 건 G2 에서 승인했을 때뿐이고, 한 번의 변경으로 남아 되돌릴 수 있다
import "../styles/quality.css";
import { useCallback, useEffect, useState } from "react";
import { api, errorText, qualityApi, type CardChange, type QualityCard, type QualityFlag, type SplitPart, type SplitProposal } from "../api";
import { n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { Badge, Button, Checkbox, Icon } from "../ui";
import { Modal } from "./NewDeck";
import { splitLater } from "./StudyParts";

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
        title: "점검 목록에서 뺐어요",
        text: "30일 동안 다시 올리지 않아요. 카드는 그대로예요.",
        tone: "undo",
        action: { label: "되돌리기", onClick: () => keep(cardId, false) },
      });
    }, fail);
  const undo = (changeId: number) =>
    qualityApi.undo(changeId).then(() => {
      reload();
      toast({ title: "나누기를 되돌렸어요", text: "새 카드를 지우고 원래 카드를 되살렸어요." });
    }, fail);
  const done = (cardId: number, changeId: number, count: number, archived: boolean) => {
    splitLater.remove(cardId);
    reload();
    toast({
      title: `${count}장으로 나눴어요`,
      text: archived ? "원래 카드는 보관했어요. 복습 기록은 그대로예요." : "원래 카드도 그대로 학습해요.",
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
          <Button variant="ghost" size="M" label="그대로 두기" onClick={() => keep(c.id)} />
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
          <p className="t-body-s c-secondary">답이 길거나 질문이 여러 개인 카드를 찾았어요. 고칠지는 직접 정해요.</p>
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
  return `원래 카드의 ${what} 옮기지 않아요 · ${archive ? "원래 카드는 보관돼요" : "원래 카드를 지우면 함께 사라져요"}`;
}

/** 다른 제안: 한 장에 담는 항목 수를 3 → 2 → 4 → 1 순으로 바꾼다 (규칙 분할) */
const PER_ORDER = [3, 2, 4, 1];
const lines = (s: string) => s.split("\n").map((l) => l.trim()).filter(Boolean);

/** G2 나누기 제안. 승인하기 전에는 아무것도 바뀌지 않는다 */
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
  const ready = parts.filter((x) => x.front.trim() && lines(x.back).length);
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
    <Modal title="카드 나누기 제안" sub="규칙으로 나눈 제안이에요. 고쳐서 써도 되고, 그대로 두어도 돼요." onClose={onClose} className="split">
      <p className="quality__info split__why t-body-s">
        <Icon name="info" size={16} />
        왜? {p.why}
      </p>
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
        <section className="split__parts">
          <span className="t-label-s c-tertiary">나눈 카드 {n(ready.length)}장</span>
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
        </section>
      </div>
      <div className="split__keep">
        <Checkbox checked={archive} onChange={setArchive} label="원래 카드는 보관해요 (학습에서 빠지고, 복습 기록은 그대로 남아요)" />
        {lost && lostLine(lost, archive) && <p className="split__lost t-caption">{lostLine(lost, archive)}</p>}
        <p className="t-caption c-tertiary">
          새 카드 {n(ready.length)}장은 새 카드로 시작하고, 중요도·하위 덱은 원래 카드를 따라요. 카드 점검의 "최근 나눈 카드"에서 언제든 되돌릴 수 있어요.
        </p>
      </div>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="split__foot">
        <span className="split__alts">
          <Button variant="ghost" size="M" icon="rotate" label="다른 제안 받기" disabled={nextPer === null} onClick={() => nextPer && suggest(nextPer)} />
          {/* 성격별로 묶기는 기기 안 모델(B-3)을 받은 경우에만. 지금은 모델이 없어 끈다 */}
          <Button variant="ghost" size="M" icon="layers" label="성격별로 묶기 · 모델이 필요해요" disabled />
        </span>
        <span className="split__actions">
          <Button variant="ghost" size="M" label="그대로 두기" onClick={onKeep} />
          <Button size="M" icon="check" label={`${n(ready.length)}장으로 나누기`} disabled={ready.length < 2} busy={busy} busyLabel="나누는 중…" onClick={apply} />
        </span>
      </div>
    </Modal>
  );
}
