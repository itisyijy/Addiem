// Q 목록 회상 · 선택형 (Figma Row Q): 부품 RecallSlot · ChoiceOption, C3 편집 칸 · 미리보기 (Q0), 학습 면 (Q1-a · Q1-b · Q2)
import { useEffect, useRef } from "react";
import { Checkbox, Icon, Kbd, Segment, type Rating } from "../ui";
import { eul, n, ro } from "../nav";
import { plainText } from "../cardHtml";
import { CIRCLED, choiceLabel, listLabel, type ChoiceDraft, type ListDraft, type Special } from "../recall";
import "../styles/recall.css";

// ---------------------------------------------------------------- 부품

/** 목록 한 칸. 기억 = 연 항목(기본), 빠뜨림 = X 로 표시한 항목, 지금 = 다음에 열 칸("?", M3 묻는 칸과 같은 파랑), 가림 = 아직 안 연 칸 */
export type SlotState = "recalled" | "missed" | "now" | "hidden";

export function RecallSlot({ num, state, text, openKey, onClick }: { num: number; state: SlotState; text: string; openKey: string; onClick?: () => void }) {
  return (
    <button type="button" className={`slot slot--${state}`} disabled={!onClick} onClick={onClick} aria-label={state === "hidden" || state === "now" ? `${num}번 항목 (가림)` : undefined}>
      <span className="slot__num t-label-s">{num}</span>
      <span className="slot__text t-body-l">{state === "now" ? "?" : state === "hidden" ? "" : text}</span>
      {state === "recalled" && <Icon name="check" size={16} className="c-success" />}
      {state === "missed" && (
        <span className="slot__mark t-label-s">
          <Icon name="x" size={16} />
          빠뜨림
        </span>
      )}
      {state === "now" && <span className="slot__mark t-label-s">{`${openKey}${ro(openKey)} 열기`}</span>}
    </button>
  );
}

/** 선택형 보기 한 줄. 질문 면: 고르기 전(숫자 키캡) · 고름, 답 면: 정답 · 고른 오답 · 놓친 정답(여러 정답 중 안 고른 것) · 그 밖 */
export type OptionState = "idle" | "picked" | "correct" | "wrong" | "missed" | "other";

export function ChoiceOption({ index, state, text, onClick }: { index: number; state: OptionState; text: string; onClick?: () => void }) {
  const front = state === "idle" || state === "picked";
  return (
    <button type="button" className={`opt opt--${state}`} disabled={!onClick} onClick={onClick} aria-pressed={front ? state === "picked" : undefined}>
      {front ? <Kbd k={String(index + 1)} /> : <span className="opt__num">{CIRCLED[index]}</span>}
      <span className="opt__text t-body-l">{text}</span>
      {(state === "correct" || state === "missed") && (
        <span className="opt__mark t-label-s">
          <Icon name="check" size={16} />
          {state === "correct" ? "정답" : "놓친 정답"}
        </span>
      )}
      {state === "wrong" && (
        <span className="opt__mark t-label-s">
          <Icon name="x" size={16} />내가 고른 답
        </span>
      )}
    </button>
  );
}

const has = (html: string) => !!plainText(html) || html.includes("<img");

function Explain({ label, html }: { label: string; html: string }) {
  return (
    <div className="recall__extra">
      <span className="t-caption c-secondary">{label}</span>
      <div className="t-body-m card-html" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}

// ---------------------------------------------------------------- 학습 면 (D2 · D3)

/**
 * 상태(연 칸 수 · 표시 · 보기 순서)는 Study 가 가진다. marks: 목록은 빠뜨린 항목 번호, 선택형은 고른 보기 번호 (1부터).
 * question · extra 는 살균한 HTML
 */
export function RecallFace({ s, phase, question, extra, opened, marks, order, openKey, onSlot, onPick }: {
  s: Special;
  phase: "q" | "a";
  question: string;
  extra: string;
  opened: number;
  marks: number[];
  order: number[];
  openKey: string;
  onSlot: (num: number) => void;
  onPick: (num: number) => void;
}) {
  if (s.kind === "list") {
    const total = s.items.length;
    const shown = phase === "a" ? total : opened;
    const missed = marks.length;
    const head =
      phase === "a" ? `${n(total)}개 중 ${n(total - missed)}개 기억` : shown ? `${n(shown)}개 열었어요${missed ? ` · ${n(missed)}개 빠뜨림` : ""}` : `${n(total)}개를 떠올려 보세요`;
    return (
      <>
        <div className="recall__q t-title-m c-tertiary card-html" dangerouslySetInnerHTML={{ __html: question }} />
        <p className="recall__sum">
          <span className="t-label-m">{head}</span>
          <span className="t-caption c-tertiary">{phase === "a" ? "항목을 누르거나 X로 빠뜨림을 고칠 수 있어요" : "연 항목은 기억한 것으로 세요. 못 떠올렸으면 X"}</span>
        </p>
        <div className="recall__list">
          {s.items.map((t, i) => {
            const state: SlotState = i < shown ? (marks.includes(i + 1) ? "missed" : "recalled") : i === shown ? "now" : "hidden";
            return <RecallSlot key={i} num={i + 1} state={state} text={t} openKey={openKey} onClick={state === "hidden" ? undefined : () => onSlot(i + 1)} />;
          })}
        </div>
        {phase === "a" && has(extra) && <Explain label="추가 설명" html={extra} />}
      </>
    );
  }
  const text = new Map(s.options);
  const multi = s.correct.length > 1;
  const stateOf = (num: number): OptionState => {
    const picked = marks.includes(num);
    if (phase === "q") return picked ? "picked" : "idle";
    if (s.correct.includes(num)) return multi && marks.length > 0 && !picked ? "missed" : "correct";
    return picked ? "wrong" : "other";
  };
  return (
    <>
      <div className={`recall__q t-title-m card-html ${phase === "a" ? "c-tertiary" : ""}`} dangerouslySetInnerHTML={{ __html: question }} />
      <div className="recall__list">
        {order.map((num, i) => (
          <ChoiceOption key={num} index={i} state={stateOf(num)} text={text.get(num) ?? ""} onClick={phase === "q" ? () => onPick(num) : undefined} />
        ))}
      </div>
      {phase === "a" && has(extra) && <Explain label="해설" html={extra} />}
    </>
  );
}

/** 추천 줄 (Q1-b · Q2). "Hard 추천 · " 뒤에 오는 말 */
export function recallTip(s: Special, marks: number[], order: number[], r: Rating): string {
  if (s.kind === "list") {
    const total = s.items.length;
    const missed = s.items.filter((_, i) => marks.includes(i + 1));
    const names = missed.slice(0, 3).join(", ") + (missed.length > 3 ? " 등" : "");
    return `${n(total)}개 중 ${n(total - missed.length)}개를 기억했어요${missed.length ? ` (${names}${eul(names)} 빠뜨렸어요)` : ""}.`;
  }
  const text = new Map(s.options);
  const list = (nums: number[]) =>
    order
      .filter((x) => nums.includes(x))
      .map((x) => `${CIRCLED[order.indexOf(x)]} ${text.get(x)}`)
      .join(", ");
  if (r === 1) return `고른 답(${list(marks.filter((m) => !s.correct.includes(m)))})이 정답(${list(s.correct)})과 달라요.`;
  if (r === 2) return `정답 ${n(s.correct.length)}개 중 ${n(marks.length)}개를 골랐어요 (놓친 정답: ${list(s.correct.filter((c) => !marks.includes(c)))}).`;
  return s.correct.length > 1 ? `정답 ${n(s.correct.length)}개를 모두 골랐어요.` : `정답(${list(s.correct)})을 골랐어요.`;
}

// ---------------------------------------------------------------- C3 편집 칸 (Q0)

/** 바뀐 뒤 그 줄의 입력 칸에 초점을 둔다 (Enter 로 줄 추가 · Backspace 로 지우기) */
function useFocusRow() {
  const box = useRef<HTMLDivElement>(null);
  const at = useRef<number | null>(null);
  const input = (i: number) => box.current?.querySelectorAll<HTMLInputElement>("input:not([type=checkbox])")[i];
  useEffect(() => {
    if (at.current === null) return;
    input(at.current)?.focus();
    at.current = null;
  });
  return {
    box,
    focus: (i: number) => void (at.current = i),
    /** 다음 줄이 비어 있으면 줄을 더하지 않고 그리로 간다 */
    next: (i: number, rows: string[]) => rows[i] === "" && (input(i)?.focus(), true),
  };
}

/** 한글 조합 중 Enter 는 글자를 마치는 키다 */
const plainEnter = (e: React.KeyboardEvent) => e.key === "Enter" && !e.ctrlKey && !e.metaKey && !e.nativeEvent.isComposing;

/** 목록: 항목(한 줄에 하나 · Enter 로 다음 줄) + [순서대로 | 순서 없이]. 2개 미만은 저장할 때 막고, 10개를 넘으면 경고만 (Q4) */
export function ListEditor({ value, onChange }: { value: ListDraft; onChange: (v: ListDraft) => void }) {
  const { box, focus, next } = useFocusRow();
  const set = (items: string[], at?: number) => {
    if (at !== undefined) focus(at);
    onChange({ ...value, items });
  };
  const count = value.items.filter((t) => t.trim()).length;
  return (
    <>
      <div className="lc__head">
        <span className="addcard__label">
          <span className="t-label-m">항목</span>
          <span className="t-caption c-tertiary">한 줄에 하나 · Enter로 다음 줄</span>
        </span>
        <Segment
          boxed
          label="순서"
          options={[
            { value: "o", label: "순서대로" },
            { value: "u", label: "순서 없이" },
          ]}
          value={value.ordered ? "o" : "u"}
          onChange={(v) => onChange({ ...value, ordered: v === "o" })}
        />
      </div>
      <div className="lc__items" ref={box}>
        {value.items.map((t, i) => (
          <label key={i} className="lc__item">
            <span className="lc__num t-caption c-tertiary">{i + 1}</span>
            <input
              className="lc__input t-body-l"
              value={t}
              placeholder={i === 0 ? "첫 항목" : ""}
              aria-label={`항목 ${i + 1}`}
              onChange={(e) => set(value.items.map((x, j) => (j === i ? e.target.value : x)))}
              onKeyDown={(e) => {
                if (plainEnter(e)) {
                  e.preventDefault();
                  if (!next(i + 1, value.items)) set([...value.items.slice(0, i + 1), "", ...value.items.slice(i + 1)], i + 1);
                } else if (e.key === "Backspace" && !t && value.items.length > 1) {
                  e.preventDefault();
                  set(value.items.filter((_, j) => j !== i), Math.max(0, i - 1));
                }
              }}
            />
          </label>
        ))}
        <button type="button" className="lc__item lc__add c-tertiary" onClick={() => set([...value.items, ""], value.items.length)}>
          <span className="lc__num t-caption">+</span>
          <span className="t-body-l">항목 추가</span>
        </button>
      </div>
      <p className={`t-caption ${count > 10 ? "c-warn" : "c-secondary"}`}>
        {n(count)}개 · 카드 1장으로 묻어요. 10개가 넘으면 둘로 나눠 보길 권해요 (막지는 않아요).
      </p>
    </>
  );
}

/** 선택형: 보기 2–6개 (체크한 것이 정답, 여러 개 가능) + 학습할 때 보기 섞기 (기본 켬, Q5). correct 는 보기의 0부터 번호 */
export function ChoiceEditor({ value, onChange }: { value: ChoiceDraft; onChange: (v: ChoiceDraft) => void }) {
  const { box, focus, next } = useFocusRow();
  const add = (at: number) => {
    if (next(at, value.options) || value.options.length >= 6) return;
    focus(at);
    onChange({ ...value, options: [...value.options.slice(0, at), "", ...value.options.slice(at)], correct: value.correct.map((c) => (c >= at ? c + 1 : c)) });
  };
  const remove = (i: number) => {
    focus(Math.max(0, i - 1));
    onChange({ ...value, options: value.options.filter((_, j) => j !== i), correct: value.correct.filter((c) => c !== i).map((c) => (c > i ? c - 1 : c)) });
  };
  return (
    <>
      <div className="lc__head">
        <span className="addcard__label">
          <span className="t-label-m">보기</span>
          <span className="t-caption c-tertiary">2–6개 · 정답에 체크 (여러 개 가능)</span>
        </span>
        <Checkbox checked={value.shuffle} label="학습할 때 보기 섞기" onChange={(shuffle) => onChange({ ...value, shuffle })} />
      </div>
      <div className="lc__opts" ref={box}>
        {value.options.map((t, i) => {
          const on = value.correct.includes(i);
          return (
            <div key={i} className={`lc__opt ${on ? "is-correct" : ""}`}>
              <Checkbox quiet checked={on} label={`보기 ${i + 1} 정답`} onChange={(v) => onChange({ ...value, correct: v ? [...value.correct, i] : value.correct.filter((c) => c !== i) })} />
              <input
                className="lc__input t-body-m"
                value={t}
                placeholder={`보기 ${i + 1}`}
                aria-label={`보기 ${i + 1}`}
                onChange={(e) => onChange({ ...value, options: value.options.map((x, j) => (j === i ? e.target.value : x)) })}
                onKeyDown={(e) => {
                  if (plainEnter(e)) {
                    e.preventDefault();
                    add(i + 1);
                  } else if (e.key === "Backspace" && !t && value.options.length > 2) {
                    e.preventDefault();
                    remove(i);
                  }
                }}
              />
              {on && <span className="t-label-s c-success">정답</span>}
            </div>
          );
        })}
      </div>
      {value.options.length < 6 && (
        <button type="button" className="lc__more t-label-m c-secondary" onClick={() => add(value.options.length)}>
          <Icon name="plus" size={14} />
          보기 추가
        </button>
      )}
      <p className="t-caption c-secondary">정답이 둘 이상이면 학습할 때 "모두 고르기"로 묻어요. "모두 맞음" 같은 보기가 있으면 섞기를 끄세요. 해설은 추가 설명 칸에 써요.</p>
    </>
  );
}

// ---------------------------------------------------------------- C3 미리보기 (질문 면)

function PreviewQ({ html }: { html: string }) {
  const empty = !has(html);
  return <div className={`lc-preview__q card-html ${empty ? "c-tertiary" : ""}`} dangerouslySetInnerHTML={{ __html: empty ? "질문을 입력하면 여기에 보여요" : html }} />;
}

/** 첫 칸 "?", 나머지 가림 */
export function ListPreview({ question, draft, openKey }: { question: string; draft: ListDraft; openKey: string }) {
  const count = draft.items.filter((t) => t.trim()).length;
  return (
    <div className="preview lc-preview">
      <PreviewQ html={question} />
      <p className="t-caption c-accent">{listLabel(count, draft.ordered)}</p>
      <div className="recall__list">
        {Array.from({ length: Math.max(count, 2) }, (_, i) => (
          <RecallSlot key={i} num={i + 1} state={i === 0 ? "now" : "hidden"} text="" openKey={openKey} />
        ))}
      </div>
    </div>
  );
}

export function ChoicePreview({ question, draft, openKey }: { question: string; draft: ChoiceDraft; openKey: string }) {
  const options = draft.options.filter((t) => t.trim());
  const multi = draft.correct.filter((c) => draft.options[c]?.trim()).length > 1;
  return (
    <div className="preview lc-preview">
      <PreviewQ html={question} />
      <p className="t-caption c-accent">
        {choiceLabel(options.length, multi)} · {draft.shuffle ? "학습할 때는 섞여 나와요" : "보기는 이 순서로 나와요"}
      </p>
      <div className="recall__list">
        {options.map((t, i) => (
          <ChoiceOption key={i} index={i} state="idle" text={t} />
        ))}
      </div>
      {options.length > 0 && <p className="t-caption c-tertiary">{`1–${options.length} 고르기 · ${openKey} 확인`}</p>}
    </div>
  );
}
