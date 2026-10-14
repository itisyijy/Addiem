// C3 카드 추가 / C3-b 빈칸 카드 / C3-r 앞뒤 둘 다 / C3-n 넣을 곳
import { forwardRef, useEffect, useImperativeHandle, useRef, useState, type KeyboardEvent } from "react";
import { api, errorText, type DeckDetail, type Level } from "../api";
import { Button, Checkbox, Icon, Importance, LEVEL_LABEL, Badge } from "../ui";
import { n, useNav, type Route } from "../nav";
import { actionKey, chordOf, getKeys, matchAction, useKeymap } from "../keys";
import { imageTag, plainText, renderCardHtml, sanitize, toStoredHtml } from "../cardHtml";
import { checkMath, findMath, withMath } from "../math";
import { Modal } from "./NewDeck";
import { DeckPicker, rootDeckIdOf, type DeckChoice } from "./DeckPicker";
import "../styles/study.css";

type Kind = "basic" | "reversed" | "cloze";
const KINDS: [Kind, string][] = [
  ["basic", "질문과 답변"],
  ["reversed", "앞뒤 둘 다"],
  ["cloze", "빈칸"],
];

export function AddCard({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [deck, setDeck] = useState<DeckDetail | null>(null);
  const [rootId, setRootId] = useState<number>();
  const [target, setTarget] = useState(deckId); // 넣을 곳. 기본은 들어온 덱, 연속 입력 중에는 마지막 선택을 유지
  const [targetDeck, setTargetDeck] = useState<DeckChoice | null>(null);
  const [kind, setKind] = useState<Kind>("basic");
  const [importance, setImportance] = useState<Level | null>(null);
  const [keepGoing, setKeepGoing] = useState(true);
  const [added, setAdded] = useState<string[]>([]);
  const [addedCount, setAddedCount] = useState(0);
  const [error, setError] = useState<string>();
  const [leaving, setLeaving] = useState<Route | null>(null); // 작성 중인 내용을 버릴지 묻는 중
  const [, setTick] = useState(0); // 편집기 내용이 바뀌면 미리보기를 다시 그린다
  const q = useRef<FieldHandle>(null);
  const a = useRef<FieldHandle>(null);

  useEffect(() => {
    api.deckDetail(deckId).then(async (d) => {
      setDeck(d);
      // 트리는 최상위 덱부터. 하위 덱 상세에서 들어왔으면 그 하위 덱이 기본으로 골라져 있다
      setRootId(d.parent_id === null ? d.id : await rootDeckIdOf(d.path));
    });
  }, [deckId]);
  useEffect(() => {
    q.current?.focus();
  }, [kind]);

  const refresh = () => setTick((t) => t + 1);
  // 비워두면 넣을 하위 덱의 중요도를 따른다
  const inherited = targetDeck?.importance ?? deck?.own_importance ?? 1;
  const shownLevel = importance ?? inherited;

  const save = async () => {
    const qHtml = q.current?.html() ?? "";
    const aHtml = a.current?.html() ?? "";
    const front = kind === "cloze" ? toCloze(qHtml) : toStoredHtml(qHtml);
    try {
      const made = await api.addCard({ deck_id: target, kind, front, back: toStoredHtml(aHtml), importance });
      setAdded((list) => [...list, plainText(kind === "cloze" ? clozePlain(qHtml) : qHtml)].slice(-5));
      setAddedCount((c) => c + made);
      setError(undefined);
      if (keepGoing) {
        q.current?.clear();
        a.current?.clear();
        q.current?.focus();
        refresh();
      } else {
        nav({ name: "deck", deckId });
      }
    } catch (e) {
      setError(errorText(e));
    }
  };

  // 쓰던 내용이 있으면 말없이 버리지 않고 먼저 묻는다
  const leave = (r: Route) => {
    const typed = [q, a].some((f) => {
      const html = f.current?.html() ?? "";
      return plainText(html).replace(/​/g, "") !== "" || html.includes("<img"); // 빈칸 뒤 공백 문자는 내용이 아니다
    });
    if (typed) setLeaving(r);
    else nav(r);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (leaving) return; // 확인 창의 Esc 는 창이 처리한다 (넣을 곳 드롭다운은 제 키를 스스로 멈춘다)
    if (matchAction("save", e.nativeEvent) >= 0) {
      e.preventDefault();
      save();
    } else if (kind === "cloze" && e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "c") {
      e.preventDefault();
      makeCloze(q.current?.el());
      refresh();
    } else if (e.key === "Escape") {
      leave({ name: "deck", deckId });
    }
  };

  const qHtml = q.current?.html() ?? "";
  const marks = kind === "cloze" ? clozeMarks(qHtml) : [];
  // C3-m: 읽지 못한 수식. 저장은 막지 않는다
  const names = kind === "cloze" ? ["본문", "추가 설명"] : kind === "reversed" ? ["앞면", "뒷면"] : ["질문", "답변"];
  const mathErrors = [qHtml, a.current?.html() ?? ""].flatMap((h, i) => checkMath(h).map((e) => ({ ...e, field: names[i] })));

  return (
    <main className="addcard" onKeyDown={onKeyDown}>
      <button className="backlink t-label-m c-secondary" onClick={() => leave({ name: "deck", deckId })}>
        <Icon name="chevron-left" size={18} />
        {deck?.name ?? ""}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">카드 추가</h1>
          <p className="t-body-s c-secondary">
            {deck?.name}
            {addedCount > 0 && ` · 이번에 ${n(addedCount)}장 추가함`}
          </p>
        </div>
        <Button
          variant="secondary"
          size="M"
          icon="arrow-right"
          label="추가 끝내고 학습 시작"
          onClick={() => leave({ name: "summary", deckIds: [deckId] })}
        />
      </div>

      <div className="addcard__cols">
        <div className="addcard__form">
          <div className="segment segment--boxed">
            {KINDS.map(([k, name]) => (
              <button
                key={k}
                className={`segment__item t-label-m ${kind === k ? "is-selected" : ""}`}
                onClick={() => {
                  setKind(k);
                  refresh();
                }}
              >
                {name}
              </button>
            ))}
          </div>
          <div className="addcard__place">
            <span className="t-label-m">넣을 곳</span>
            {rootId !== undefined && (
              <DeckPicker
                rootDeckId={rootId}
                value={target}
                onChange={(d) => {
                  setTarget(d.id);
                  setTargetDeck(d);
                }}
              />
            )}
          </div>

          {kind !== "cloze" ? (
            <>
              {/* 앞뒤 둘 다는 어느 쪽도 질문이 되므로 앞면 · 뒷면. 질문과 답변을 오가도 쓴 내용은 남는다 */}
              <span className="t-label-m">{kind === "reversed" ? "앞면" : "질문"}</span>
              <RichField key="q-basic" ref={q} minHeight={64} onInput={refresh} />
              <span className="t-label-m">{kind === "reversed" ? "뒷면" : "답변"}</span>
              <RichField key="a-basic" ref={a} minHeight={84} onInput={refresh} />
              {kind === "reversed" && (
                <span className="addcard__chips">
                  <span className="t-caption c-tertiary">만들어질 카드</span>
                  <Badge tone="accent">1 · 앞면 → 뒷면</Badge>
                  <Badge tone="accent">2 · 뒷면 → 앞면</Badge>
                </span>
              )}
              <DropZone onImage={(tag) => a.current?.insert(tag)} />
            </>
          ) : (
            <>
              <span className="addcard__label">
                <span className="t-label-m">본문</span>
                <span className="t-caption c-tertiary">단어를 선택하고 빈칸으로 만드세요. 빈칸마다 카드 1장이 생겨요.</span>
              </span>
              <RichField
                key="q-cloze"
                ref={q}
                minHeight={96}
                onInput={refresh}
                extraTool={
                  <Button
                    variant="secondary"
                    size="M"
                    label="빈칸 만들기"
                    shortcut="Ctrl Shift C"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      makeCloze(q.current?.el());
                      refresh();
                    }}
                  />
                }
              />
              {marks.length > 0 && (
                <span className="addcard__chips">
                  <span className="t-caption c-tertiary">만들어질 카드</span>
                  {marks.map((m, i) => (
                    <Badge key={i} tone="accent">
                      {i + 1} · {m}
                    </Badge>
                  ))}
                </span>
              )}
              <span className="addcard__label">
                <span className="t-label-m">추가 설명</span>
                <span className="t-caption c-tertiary">선택 · 답을 본 뒤에만 보여요</span>
              </span>
              <RichField key="a-cloze" ref={a} minHeight={44} toolbar={false} onInput={refresh} />
            </>
          )}

          <div className="addcard__importance">
            <span className="addcard__importance-label">
              <span className="t-label-m">중요도</span>
              <span className="t-caption c-tertiary">비워두면 하위 덱 설정을 따라요 ({LEVEL_LABEL[inherited]})</span>
            </span>
            <div className="segment segment--boxed">
              {([2, 1, 0] as const).map((lvl) => (
                <button
                  key={lvl}
                  className={`segment__item t-label-m ${shownLevel === lvl ? "is-selected" : ""}`}
                  onClick={() => setImportance(importance === lvl ? null : lvl)}
                >
                  {LEVEL_LABEL[lvl]}
                </button>
              ))}
            </div>
          </div>

          <div className="addcard__submit">
            <Checkbox checked={keepGoing} label="저장 후 바로 다음 카드" onChange={setKeepGoing} />
            <span className="addcard__submit-right">
              {error && <span className="t-body-s c-danger">{error}</span>}
              <Button icon="check" label="저장" shortcut={actionKey("save")} onClick={save} />
            </span>
          </div>
        </div>

        <aside className="addcard__side">
          <p className="t-caption c-tertiary">
            학습할 때 이렇게 보여요
            {kind === "cloze" && marks.length > 0 ? ` · 카드 1 / ${marks.length}` : kind === "reversed" ? " · 카드 2장" : ""}
          </p>
          {kind === "reversed" ? (
            <>
              <Preview kind="basic" qHtml={qHtml} aHtml={a.current?.html() ?? ""} level={shownLevel} head="카드 1 · 앞 → 뒤" />
              <Preview kind="basic" qHtml={a.current?.html() ?? ""} aHtml={qHtml} level={shownLevel} head="카드 2 · 뒤 → 앞" />
              <p className="addcard__note t-caption c-secondary">
                <Icon name="info" size={14} />
                두 장은 같은 날 연달아 나오지 않아요. 하나를 보면 나머지는 다음 날로 미뤄요.
              </p>
            </>
          ) : (
            <Preview kind={kind} qHtml={qHtml} aHtml={a.current?.html() ?? ""} level={shownLevel} />
          )}
          {mathErrors.length > 0 && (
            <div className="addcard__matherr" role="status">
              <Icon name="alert" size={16} className="c-danger" />
              <span className="addcard__matherr-text">
                <span className="t-body-m-strong c-danger">수식 {n(mathErrors.length)}개를 읽지 못했어요</span>
                <span className="t-body-s c-secondary">
                  {mathErrors.map((e) => `${e.field} ${e.line}번째 줄 · ${e.message}`).join(" / ")}. 고치지 않아도 저장되고, 학습할 때는 원문
                  그대로 보여요.
                </span>
              </span>
            </div>
          )}
          {added.length > 0 && (
            <div className="addcard__recent">
              <p className="t-caption c-tertiary">방금 추가한 카드</p>
              {added.map((t, i) => (
                <p key={i} className="addcard__recent-item t-body-s c-secondary">
                  <Icon name="check" size={14} className="c-success" />
                  <span>{t || "(내용 없음)"}</span>
                </p>
              ))}
            </div>
          )}
        </aside>
      </div>

      {leaving && (
        <Modal title="작성 중인 카드가 있어요" onClose={() => setLeaving(null)}>
          <div className="modal__body">
            <p className="t-body-m c-secondary">저장하지 않고 나갈까요?</p>
            <div className="modal__actions">
              <Button variant="ghost" size="M" label="계속 쓰기" shortcut="Esc" onClick={() => setLeaving(null)} />
              <Button variant="danger" size="M" label="버리고 나가기" onClick={() => nav(leaving)} />
            </div>
          </div>
        </Modal>
      )}
    </main>
  );
}

// ---------------------------------------------------------------- 미리보기

/** head 를 주면 C3-r 의 작은 미리보기 (카드 1 · 앞 → 뒤) */
function Preview({ kind, qHtml, aHtml, level, head }: { kind: Kind; qHtml: string; aHtml: string; level: Level; head?: string }) {
  let question: string;
  let answer: string;
  if (kind === "cloze") {
    const marks = clozeMarks(qHtml);
    question = previewCloze(qHtml);
    answer = marks.length ? `정답 · ${marks[0]}` : "";
    if (plainText(aHtml)) answer += `<br>${sanitize(aHtml)}`;
  } else {
    question = sanitize(qHtml);
    answer = sanitize(aHtml);
  }
  // 수식은 살균한 뒤에 그린다 (math.ts)
  question = withMath(question);
  answer = withMath(answer);
  const empty = !plainText(question) && !question.includes("<img");
  return (
    <div className={`preview ${head ? "preview--small" : ""}`}>
      <div className="preview__head">
        <span className="t-label-s c-tertiary">{head ?? "미리보기"}</span>
        <Importance level={level} />
      </div>
      <div
        className={`preview__q t-title-m card-html ${empty ? "c-tertiary" : ""}`}
        dangerouslySetInnerHTML={{ __html: empty ? (head ? "입력하면 여기에 보여요" : "질문을 입력하면 여기에 보여요") : question }}
      />
      <div className="preview__rule" />
      <div className="preview__a t-body-m c-secondary card-html" dangerouslySetInnerHTML={{ __html: answer }} />
    </div>
  );
}

// ---------------------------------------------------------------- 서식 입력 칸

/**
 * 편집기 안 수식 표시 (C3-m). DOM 을 바꾸지 않는 CSS Custom Highlight 로 칠한다.
 * 그래서 표시용 요소가 저장 HTML 에 섞일 일이 없고, 입력 중 커서 · 실행 취소도 그대로다.
 * 한계: ::highlight 는 글꼴을 바꿀 수 없어 고정폭 글꼴은 못 쓴다 (배경 · 글자색 · 밑줄만)
 */
const mathRanges = new Map<HTMLElement, { ok: Range[]; bad: Range[] }>();
function paintMath() {
  if (!("highlights" in CSS)) return; // 지원하지 않으면 표시만 없다
  const all = [...mathRanges.values()];
  CSS.highlights.set("math", new Highlight(...all.flatMap((r) => r.ok)));
  CSS.highlights.set("math-error", new Highlight(...all.flatMap((r) => r.bad)));
}
const escapeText = (t: string) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
/** 글자 노드 하나 안의 수식만 (그리기와 같은 규칙: 서식이 끼어 노드가 나뉜 수식은 수식이 아니다) */
function markMath(el: HTMLElement | null) {
  if (!el) return;
  const ok: Range[] = [];
  const bad: Range[] = [];
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    for (const part of findMath(n.textContent ?? "")) {
      const r = document.createRange();
      r.setStart(n, part.start);
      r.setEnd(n, part.end);
      (checkMath(escapeText(part.source)).length ? bad : ok).push(r);
    }
  }
  mathRanges.set(el, { ok, bad });
  paintMath();
}

export type FieldHandle = { html: () => string; clear: () => void; focus: () => void; insert: (html: string) => void; el: () => HTMLDivElement | null };

export const RichField = forwardRef<
  FieldHandle,
  { minHeight: number; onInput: () => void; toolbar?: boolean; extraTool?: React.ReactNode; initial?: string }
>(
  function RichField({ minHeight, onInput, toolbar = true, extraTool, initial }, ref) {
    const box = useRef<HTMLDivElement>(null);
    const file = useRef<HTMLInputElement>(null);
    useKeymap();
    const mathKey = getKeys("math")[0] ?? "";
    const mathTip = mathKey
      ? `수식 넣기 ${actionKey("math")} · 블록 수식 ${actionKey("math").replace(/^((?:Ctrl )?(?:Alt )?)/, "$1Shift ")}`
      : "수식 넣기";

    // 내용이 바뀔 때마다 수식 표시를 다시 칠하고 부모에 알린다
    const changed = () => {
      markMath(box.current);
      onInput();
    };
    useEffect(() => {
      const el = box.current;
      return () => {
        if (el) mathRanges.delete(el);
        paintMath();
      };
    }, []);

    // 저장된 HTML 로 시작한다 (카드 편집). 그림 파일 이름을 화면용 주소로 바꾼다
    useEffect(() => {
      if (!initial) return;
      renderCardHtml(initial).then((html) => {
        if (box.current) box.current.innerHTML = html;
        markMath(box.current);
      });
    }, [initial]);

    const insert = (html: string) => {
      const el = box.current;
      if (!el) return;
      el.focus();
      const sel = window.getSelection();
      if (!sel || !el.contains(sel.anchorNode)) {
        const range = document.createRange();
        range.selectNodeContents(el);
        range.collapse(false);
        sel?.removeAllRanges();
        sel?.addRange(range);
      }
      document.execCommand("insertHTML", false, html);
      changed();
    };

    /** C3-m: 고른 글자를 \( \) (블록은 \[ \]) 로 감싼다. 고른 게 없으면 빈 괄호 안에 커서 */
    const wrapMath = (block: boolean) => {
      const el = box.current;
      if (!el) return;
      const sel = window.getSelection();
      if (!sel || !el.contains(sel.anchorNode)) insert("");
      const text = window.getSelection()?.toString() ?? "";
      const [open, close] = block ? ["\\[", "\\]"] : ["\\(", "\\)"];
      document.execCommand("insertText", false, open + text + close);
      if (!text) for (let i = 0; i < close.length; i++) window.getSelection()?.modify("move", "backward", "character");
      changed();
    };

    useImperativeHandle(ref, () => ({
      html: () => box.current?.innerHTML ?? "",
      clear: () => {
        if (box.current) box.current.innerHTML = "";
        markMath(box.current);
      },
      focus: () => box.current?.focus(),
      insert,
      el: () => box.current,
    }));

    const cmd = (c: string) => {
      box.current?.focus();
      document.execCommand(c);
      changed();
    };

    const addFiles = async (files: FileList | File[]) => {
      for (const f of Array.from(files).filter((f) => f.type.startsWith("image/"))) insert(await imageTag(f));
    };

    return (
      <div className="rich">
        {toolbar && (
          <div className="rich__tools" onMouseDown={(e) => e.target !== file.current && e.preventDefault()}>
            <button className="rich__tool t-label-m" aria-label="굵게" onClick={() => cmd("bold")}>
              B
            </button>
            <button className="rich__tool t-label-m" aria-label="기울임" onClick={() => cmd("italic")}>
              I
            </button>
            <button className="rich__tool t-label-m" aria-label="밑줄" onClick={() => cmd("underline")}>
              U
            </button>
            <button className="rich__tool" aria-label="목록" onClick={() => cmd("insertUnorderedList")}>
              <Icon name="list" size={16} scaled />
            </button>
            <button className="rich__tool" aria-label="이미지" onClick={() => file.current?.click()}>
              <Icon name="image" size={16} scaled />
            </button>
            <button className="rich__tool rich__math" aria-label={mathTip} title={mathTip} onClick={() => wrapMath(false)}>
              <span className="rich__sum">∑</span>
              <span className="t-label-s">수식</span>
            </button>
            <input ref={file} type="file" accept="image/*" multiple hidden onChange={(e) => e.target.files && addFiles(e.target.files)} />
            {extraTool && <span className="rich__extra">{extraTool}</span>}
          </div>
        )}
        <div
          ref={box}
          className="rich__body t-body-l card-html"
          style={{ minHeight }}
          contentEditable
          suppressContentEditableWarning
          onInput={changed}
          onKeyDown={(e) => {
            // 수식 넣기 (기본 Ctrl M), 같은 키 + Shift 는 블록 수식. 키는 설정 > 단축키에서 바꿀 수 있다
            const c = chordOf(e.nativeEvent);
            const m = getKeys("math")[0];
            if (!c || !m) return;
            const block = !m.includes("Shift+") && c === m.replace(/^((?:Ctrl\+)?(?:Alt\+)?)/, "$1Shift+");
            if (c === m || block) {
              e.preventDefault();
              wrapMath(block);
            }
          }}
          onPaste={(e) => {
            const imgs = Array.from(e.clipboardData.files).filter((f) => f.type.startsWith("image/"));
            if (imgs.length) {
              e.preventDefault();
              addFiles(imgs);
            }
          }}
        />
      </div>
    );
  },
);

function DropZone({ onImage }: { onImage: (tag: string) => void }) {
  const [over, setOver] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const take = async (files: FileList) => {
    for (const f of Array.from(files).filter((f) => f.type.startsWith("image/"))) onImage(await imageTag(f));
  };
  return (
    <button
      className={`dropzone ${over ? "is-over" : ""}`}
      onClick={() => input.current?.click()}
      onDragOver={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        take(e.dataTransfer.files);
      }}
    >
      <Icon name="image" size={18} className="c-tertiary" />
      <span className="t-body-s c-tertiary">
        이미지를 끌어오거나 붙여넣기 (Ctrl V) · 여러 장 가능
      </span>
      <input ref={input} type="file" accept="image/*" multiple hidden onChange={(e) => e.target.files && take(e.target.files)} />
    </button>
  );
}

// ---------------------------------------------------------------- 빈칸

/** 선택한 글자를 빈칸 표시로 감싼다. Cloze 문법은 화면에 드러내지 않는다 (브리프 3.5) */
export function makeCloze(el: HTMLDivElement | null | undefined) {
  const sel = window.getSelection();
  if (!el || !sel || sel.isCollapsed || !el.contains(sel.anchorNode)) return;
  const text = sel.toString();
  if (!text.trim()) return;
  const esc = text.replace(/&/g, "&amp;").replace(/</g, "&lt;");
  document.execCommand("insertHTML", false, `<span class="cloze-mark">${esc}</span>&#8203;`);
}

function marksOf(html: string) {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  return { tpl, marks: Array.from(tpl.content.querySelectorAll("span.cloze-mark")) };
}

function clozeMarks(html: string): string[] {
  return marksOf(html).marks.map((m) => (m.textContent ?? "").trim());
}

/**
 * 편집기 HTML -> Anki Cloze 문법 ({{c1::...}}).
 * 편집할 때 받은 번호(data-c)는 지킨다. 번호가 바뀌면 다른 카드의 학습 기록이 엉뚱한 빈칸에 붙는다
 */
export function toCloze(html: string, firstFree = 1): string {
  const { tpl, marks } = marksOf(toStoredHtml(html).replace(/​/g, ""));
  const used = new Set(marks.map((m) => Number(m.getAttribute("data-c"))).filter((c) => c > 0));
  let next = firstFree; // 편집할 때는 원래 노트의 가장 큰 번호 다음부터 (EditNote)
  marks.forEach((m) => {
    let c = Number(m.getAttribute("data-c"));
    if (!(c > 0)) {
      while (used.has(next)) next++;
      c = next;
      used.add(c);
    }
    const hint = m.getAttribute("data-hint");
    m.replaceWith(`{{c${c}::${m.textContent ?? ""}${hint ? `::${hint}` : ""}}}`);
  });
  return tpl.innerHTML;
}

/** Anki Cloze 문법 -> 편집기 표시. 번호와 힌트를 속성으로 남긴다 */
export function fromCloze(html: string): string {
  const attr = (v: string) => v.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
  return html.replace(
    /\{\{c(\d+)::([\s\S]*?)(?:::([\s\S]*?))?\}\}/g,
    (_, c: string, body: string, hint?: string) =>
      `<span class="cloze-mark" data-c="${c}"${hint ? ` data-hint="${attr(hint)}"` : ""}>${body}</span>`,
  );
}

function clozePlain(html: string): string {
  const { tpl, marks } = marksOf(html.replace(/​/g, ""));
  marks.forEach((m) => m.replaceWith(`[${m.textContent}]`));
  return tpl.innerHTML;
}

function previewCloze(html: string): string {
  const { tpl, marks } = marksOf(sanitize(html));
  marks.forEach((m, i) => {
    if (i === 0) {
      const span = document.createElement("span");
      span.className = "cloze";
      span.textContent = "[      ]";
      m.replaceWith(span);
    } else m.replaceWith(m.textContent ?? "");
  });
  return tpl.innerHTML;
}
