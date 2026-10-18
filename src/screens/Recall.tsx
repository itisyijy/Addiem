// Q 목록 회상 · 선택형 (Figma Row Q): 부품 RecallSlot · ChoiceOption, C3 편집 칸 · 미리보기 (Q0), 학습 면 (Q1-a · Q1-b · Q2), Q8 오답 보기 제안
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Button, Checkbox, Icon, Kbd, Segment, type IconName, type Rating } from "../ui";
import { eul, eun, gwa, iga, n, ro } from "../nav";
import { plainText } from "../cardHtml";
import { aiApi, aiError, type AiError } from "../api";
import { AiConnect, ScopeLine, SendList, UsingPlan, openUsage, refreshAi, sendConfirmed, setSendConfirmed, useAiStatus, type SendRow } from "../ai";
import { CIRCLED, choiceLabel, listLabel, optionKey, recall, wantedDistractors, type ChoiceDraft, type ListDraft, type Special } from "../recall";
import "../styles/recall.css";

// ---------------------------------------------------------------- 부품

/** 목록 한 칸. 기억 = 연 항목(기본), 빠뜨림 = X 로 표시한 항목, 지금 = 다음에 열 칸("?", M3 묻는 칸과 같은 파랑), 가림 = 아직 안 연 칸 */
export type SlotState = "recalled" | "missed" | "now" | "hidden";

/** Figma 속성 "지난번" (RecallSlot · ChoiceOption). 답 면에만 */
function LastPill({ label }: { label: string }) {
  return (
    <span className="last-pill t-label-s">
      <Icon name="rotate" size={12} />
      {label}
    </span>
  );
}

export function RecallSlot({ num, state, text, openKey, last, onClick }: { num: number; state: SlotState; text: string; openKey: string; last?: boolean; onClick?: () => void }) {
  return (
    <button type="button" className={`slot slot--${state}`} disabled={!onClick} onClick={onClick} aria-label={state === "hidden" || state === "now" ? `${num}번 항목 (가림)` : undefined}>
      <span className="slot__num t-label-s">{num}</span>
      <span className="slot__text t-body-l">{state === "now" ? "?" : state === "hidden" ? "" : text}</span>
      {last && <LastPill label="지난번 빠뜨림" />}
      {state === "recalled" && <Icon name="check" size={16} className="c-success" />}
      {state === "missed" && (
        <span className="slot__mark t-label-s">
          <Icon name="x" size={16} />
          기억 못함
        </span>
      )}
      {state === "now" && <span className="slot__mark t-label-s">{`${openKey}${ro(openKey)} 열기`}</span>}
    </button>
  );
}

/** 선택형 보기 한 줄. 질문 면: 고르기 전(숫자 키캡) · 고름, 답 면: 정답 · 고른 오답 · 놓친 정답(여러 정답 중 안 고른 것) · 그 밖 */
export type OptionState = "idle" | "picked" | "correct" | "wrong" | "missed" | "other";

export function ChoiceOption({ index, state, text, last, onClick }: { index: number; state: OptionState; text: string; last?: boolean; onClick?: () => void }) {
  const front = state === "idle" || state === "picked";
  return (
    <button type="button" className={`opt opt--${state}`} disabled={!onClick} onClick={onClick} aria-pressed={front ? state === "picked" : undefined}>
      {front ? <Kbd k={String(index + 1)} /> : <span className="opt__num">{CIRCLED[index]}</span>}
      <span className="opt__text t-body-l">{text}</span>
      {last && <LastPill label="지난번 고름" />}
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

function LastLine({ text }: { text: string }) {
  return (
    <p className="recall__last t-label-s c-secondary">
      <Icon name="rotate" size={14} />
      {text}
    </p>
  );
}

/** Q31 · Q33: "지난번" 표시를 붙일 번호. 목록은 지난번 빠뜨린 항목 모두, 선택형은 지난번에 고른 오답 (오답이 없었으면 없다) */
export function lastOn(s: Special, last: number[] | null): number[] {
  if (!last) return [];
  return s.kind === "list" ? last : last.filter((m) => !s.correct.includes(m));
}

/** Q36: 한 줄에 넣는 항목 · 보기 글자는 20자까지 (넘으면 "…"). 조사는 원래 글자로 고른다 */
const CLIP = 20;
const named = (t: string, josa: (w: string) => string) => {
  const c = [...t];
  return `${c.length > CLIP ? `${c.slice(0, CLIP).join("").trimEnd()}…` : t}${josa(t)}`;
};

/** Q36 요약 아래 · 보기 아래 한 줄. 사실만 쓴다 (추천은 이번 결과로만). last 는 lastOn 의 번호, 보일 것이 없으면 null */
function lastTip(s: Special, last: number[], marks: number[], order: number[]): string | null {
  if (!last.length) return null;
  if (s.kind === "list") {
    const now = s.items.filter((_, i) => last.includes(i + 1) && !marks.includes(i + 1));
    const tail =
      now.length === 0
        ? ""
        : now.length === 1
          ? ` · 그중 ${named(now[0], eun)} 이번에 기억했어요`
          : now.length === last.length
            ? " · 이번엔 모두 기억했어요"
            : ` · 그중 ${n(now.length)}개는 이번에 기억했어요`;
    return `지난번엔 ${n(last.length)}개를 빠뜨렸어요${tail}`;
  }
  // 번호는 이번 화면의 번호 (보기를 섞어도 보기를 따라간다). 지난번 오답이 하나면 보기 글자도 쓴다
  const nums = order.flatMap((x, i) => (last.includes(x) ? [CIRCLED[i]] : [])).join(", ");
  const text = last.length === 1 ? new Map(s.options).get(last[0]) : undefined;
  const was = text === undefined ? `${nums}${eul(nums)}` : `${nums} ${named(text, eul)}`;
  const wrong = marks.filter((m) => !s.correct.includes(m));
  if (wrong.length) return wrong.length === last.length && wrong.every((m) => last.includes(m)) ? `이번에도 ${was} 골랐어요` : `지난번엔 ${was} 골랐어요`;
  if (marks.length === s.correct.length) return `지난번엔 ${was} 골랐어요 · 이번엔 맞혔어요`;
  // 정답 일부만 (Hard): 줄이 길어 번호만
  if (marks.length) return `지난번엔 ${nums}${eul(nums)} 골랐어요 · 이번엔 정답 ${n(s.correct.length)}개 중 ${n(marks.length)}개를 골랐어요`;
  return `지난번엔 ${was} 골랐어요`;
}

/**
 * 상태(연 칸 수 · 표시 · 보기 순서)는 Study 가 가진다. marks: 목록은 빠뜨린 항목 번호, 선택형은 고른 보기 번호 (1부터).
 * last: 지난번 marks (card_view.last_marks, 답 면에만 보인다. 질문 면에 보이면 힌트가 된다). question · extra 는 살균한 HTML
 */
export function RecallFace({ s, phase, question, extra, opened, marks, last, order, openKey, onSlot, onPick }: {
  s: Special;
  phase: "q" | "a";
  question: string;
  extra: string;
  opened: number;
  marks: number[];
  last: number[] | null;
  order: number[];
  openKey: string;
  onSlot: (num: number) => void;
  onPick: (num: number) => void;
}) {
  const was = phase === "a" ? lastOn(s, last) : [];
  const tip = lastTip(s, was, marks, order);
  if (s.kind === "list") {
    const total = s.items.length;
    const shown = phase === "a" ? total : opened;
    const missed = marks.length;
    const head =
      phase === "a"
        ? `직접 표시한 결과: ${n(total)}개 중 ${n(total - missed)}개 기억`
        : shown
          ? `${n(shown)}개 열었어요${missed ? ` · ${n(missed)}개 기억 못함` : ""}`
          : `${n(total)}개를 떠올려 보세요`;
    return (
      <>
        <div className="recall__q t-title-m c-tertiary card-html" dangerouslySetInnerHTML={{ __html: question }} />
        <p className="recall__sum">
          <span className="t-label-m">{head}</span>
          <span className="t-caption c-tertiary">{phase === "a" ? "항목을 누르거나 X로 기억 여부를 바꿀 수 있어요" : "항목을 연 뒤 못 떠올렸던 것은 X로 \"기억 못함\" 표시해 주세요"}</span>
        </p>
        {tip && <LastLine text={tip} />}
        <div className="recall__list">
          {s.items.map((t, i) => {
            const state: SlotState = i < shown ? (marks.includes(i + 1) ? "missed" : "recalled") : i === shown ? "now" : "hidden";
            return (
              <RecallSlot key={i} num={i + 1} state={state} text={t} openKey={openKey} last={was.includes(i + 1)} onClick={state === "hidden" ? undefined : () => onSlot(i + 1)} />
            );
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
          <ChoiceOption key={num} index={i} state={stateOf(num)} text={text.get(num) ?? ""} last={was.includes(num)} onClick={phase === "q" ? () => onPick(num) : undefined} />
        ))}
      </div>
      {tip && <LastLine text={tip} />}
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
  // 보기 번호만 (보기 글자를 쓰면 줄이 카드보다 넓어졌다)
  const list = (nums: number[]) => order.flatMap((x, i) => (nums.includes(x) ? [CIRCLED[i]] : [])).join(", ");
  const right = list(s.correct);
  if (r === 1) {
    const wrong = list(marks.filter((m) => !s.correct.includes(m)));
    return `고른 답 ${wrong}${iga(wrong)} 정답 ${right}${gwa(right)} 달라요.`;
  }
  if (r === 2) return `정답 ${n(s.correct.length)}개 중 ${n(marks.length)}개를 골랐어요 (놓친 정답: ${list(s.correct.filter((c) => !marks.includes(c)))}).`;
  return s.correct.length > 1 ? `정답 ${n(s.correct.length)}개를 모두 골랐어요.` : `정답 ${right}${eul(right)} 골랐어요.`;
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

/**
 * 선택형: 보기 2–6개 (체크한 것이 정답, 여러 개 가능) + 학습할 때 보기 섞기 (기본 켬, Q5). correct 는 보기의 0부터 번호.
 * question: 있으면 보기 아래에 Q8 오답 보기 제안 줄 (편집 중인 질문 HTML 을 보낼 때 읽는다). onUnplaced: 넣지 않은 제안 수 (저장 토스트, Q43)
 */
export function ChoiceEditor({ value, onChange, question, onUnplaced }: {
  value: ChoiceDraft;
  onChange: (v: ChoiceDraft) => void;
  question?: () => string;
  onUnplaced?: (count: number) => void;
}) {
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
      {question && <DistractorLine question={question} value={value} onChange={onChange} onUnplaced={onUnplaced} />}
      <p className="t-caption c-secondary">정답이 둘 이상이면 학습할 때 "모두 고르기"로 묻어요. "모두 맞음" 같은 보기가 있으면 섞기를 끄세요. 해설은 추가 설명 칸에 써요.</p>
    </>
  );
}

// ---------------------------------------------------------------- Q8 오답 보기 제안 (Figma 237:16413 · 다크 237:16796, Q37–Q43)

type DqPhase = "idle" | "confirm" | "busy" | "done" | "error";

/** 한 줄 오류 (L4-e 와 같은 문구 결, 버튼은 글자 링크 하나) */
function dqError(f: AiError): [IconName, string, string] {
  if (f.kind === "expired") return ["link-off", "c-danger", "ChatGPT 연결이 끊겼어요"];
  if (f.kind === "scope") return ["link-off", "c-danger", "사용량 권한이 필요해요"];
  if (f.kind === "limit") return ["gauge", "c-warn", "요금제 사용 한도에 닿았어요 · 잠시 뒤 다시 해 주세요"];
  if (f.kind === "bad_output") return ["alert", "c-warn", "알맞은 오답을 만들지 못했어요 · 다시 시도해 주세요"];
  if (f.kind === "offline") return ["wifi-off", "c-secondary", "인터넷에 연결되어 있지 않아요"];
  if (f.kind === "unavailable") return ["alert", "c-warn", "지금은 ChatGPT 요금제를 쓸 수 없어요 · 잠시 뒤 다시 해 보세요"];
  if (f.kind === "not_eligible") return ["alert", "c-warn", "이 요금제에서는 쓸 수 없어요"];
  return ["alert", "c-warn", f.message];
}

function DqRow({ icon, tone, text, action }: { icon: IconName; tone: string; text: string; action?: [string, () => void] }) {
  return (
    <div className="dq__err" role="status">
      <span className="dq__lead">
        <Icon name={icon} size={16} className={tone} />
        <span className="t-label-m">{text}</span>
      </span>
      {action && (
        <button type="button" className="ai-link t-label-m" onClick={action[1]}>
          {action[0]}
        </button>
      )}
    </div>
  );
}

/**
 * 보기 목록 아래 한 줄. ChatGPT 가 연결돼 있을 때만 보인다 (Q40, 이 창에서 난 오류는 남긴다).
 * 제안은 점선 보기로만 있고 [넣기] 전에는 편집기 값에 없다 (저장되지 않는다). 기다리는 동안에도 보기를 쓰고 저장할 수 있다:
 * 부모가 저장 뒤 key 를 바꾸거나 창을 닫으면 여기서 요청을 멈춘다
 */
function DistractorLine({ question, value, onChange, onUnplaced }: {
  question: () => string;
  value: ChoiceDraft;
  onChange: (v: ChoiceDraft) => void;
  onUnplaced?: (count: number) => void;
}) {
  const ai = useAiStatus();
  const [phase, setPhase] = useState<DqPhase>("idle");
  const [offered, setOffered] = useState<string[]>([]);
  const [sentWhat, setSentWhat] = useState("");
  const [skip, setSkip] = useState<string[]>([]); // 보낼 목록에서 ✕ 로 뺀 이미 쓴 보기 (이 창 동안)
  const [prior, setPrior] = useState<string[]>([]); // 이 창에서 받은 제안 모두 (넣은 것 · 뺀 것 · 남은 것). 다시 받을 때 피할 보기로
  const [fail, setFail] = useState<AiError | null>(null);
  const [slow, setSlow] = useState(false);
  const [reconnect, setReconnect] = useState<boolean | null>(null); // [다시 연결] (사용량 권한을 다시 묻는가)
  const seq = useRef(0);
  const inFlight = useRef(false);
  const latest = useRef(value);
  const report = useRef(onUnplaced);
  useEffect(() => {
    latest.current = value;
    report.current = onUnplaced;
  });
  // 저장할 때 버려질 제안 수 (부모의 저장 토스트, Q43)
  useEffect(() => {
    report.current?.(phase === "done" ? offered.length : 0);
  }, [phase, offered]);
  useEffect(
    () => () => {
      if (inFlight.current) aiApi.cancel().catch(() => {});
      report.current?.(0);
    },
    [],
  );
  useEffect(() => {
    if (phase !== "busy") return setSlow(false);
    const t = setTimeout(() => setSlow(true), 30_000);
    return () => clearTimeout(t);
  }, [phase]);
  // 낮은 창의 카드 편집(E): 받은 제안이 보기 목록과 함께 보이게 창 안에서 맞춘다. 보기 머리를 위로, 그래도 제안이 넘치면 제안 끝까지
  // (700px 은 study.css 의 낮은 창과 같다. 800 이상은 그대로)
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (phase !== "done" || !el?.closest(".modal--edit") || !matchMedia("(max-height: 700px)").matches) return;
    el.parentElement?.querySelector(".lc__head")?.scrollIntoView({ block: "start" });
    el.scrollIntoView({ block: "nearest" });
  }, [phase]);

  if (!ai?.connected && phase !== "error" && reconnect === null) return null;

  const filled = value.options.flatMap((t, i) => (t.trim() ? [{ t: t.trim(), on: value.correct.includes(i) }] : []));
  const correct = filled.filter((o) => o.on).map((o) => o.t);
  const sending = filled.filter((o) => !o.on && !skip.includes(o.t)).map((o) => o.t);
  // 넣은 제안은 보기 칸에 있어 이미 쓴 보기로 간다. 나머지(뺀 것 · 남은 것 · 넣은 뒤 고친 것)는 피할 보기로
  const avoid = prior.filter((t) => !value.options.some((o) => optionKey(o) === optionKey(t)));
  const want = wantedDistractors(filled.length);
  const room = value.options.some((o) => !o.trim()) || value.options.length < 6;
  const what = `질문 · 정답${correct.length > 1 ? ` ${n(correct.length)}개` : ""}${sending.length ? ` · 이미 쓴 보기 ${n(sending.length)}개` : ""}${avoid.length ? ` · 앞서 받은 제안 ${n(avoid.length)}개` : ""}`;

  const send = () => {
    setSendConfirmed(true);
    const my = ++seq.current;
    inFlight.current = true;
    setFail(null);
    setPhase("busy");
    const sent = `보낸 것 · ${what}`;
    recall.distractors(question(), correct, sending, avoid, want).then(
      (got) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        refreshAi();
        // 기다리는 동안 직접 쓴 보기와 겹치는 제안은 뺀다
        const have = latest.current.options.map(optionKey);
        const fresh = got.filter((t) => !have.includes(optionKey(t)));
        if (!fresh.length) {
          setFail({ kind: "bad_output", message: "" });
          return setPhase("error");
        }
        setPrior((p) => [...p, ...fresh]);
        setOffered(fresh);
        setSentWhat(sent);
        setPhase("done");
      },
      (e) => {
        if (seq.current !== my) return;
        inFlight.current = false;
        refreshAi();
        const f = aiError(e);
        if (f.kind === "cancelled" || f.kind === "not_connected") return setPhase("idle");
        setFail(f);
        setPhase("error");
      },
    );
  };
  /** 처음 보낼 때 한 번 묻는다 (L4 와 같은 이 기기 확인, Q39). ponytail: 편집 창마다 묻게 하려면 sendConfirmed() 대신 이 창의 ref */
  const start = () => (sendConfirmed() ? send() : setPhase("confirm"));
  const cancel = () => {
    if (inFlight.current) aiApi.cancel().catch(() => {});
    inFlight.current = false;
    seq.current++;
    setPhase("idle");
  };
  const keep = (left: string[]) => {
    setOffered(left);
    if (!left.length) setPhase("idle");
  };
  /** [넣기]: 첫 빈 보기 칸에, 없으면 끝에 오답(체크 없음)으로. 그사이 같은 보기를 썼으면 넣지 않고 지운다. 보기 6개에서 멈춘다 */
  const insert = (list: string[]) => {
    const options = [...value.options];
    let correctNow = value.correct;
    let left = offered;
    for (const t of list) {
      if (!options.some((o) => optionKey(o) === optionKey(t))) {
        const at = options.findIndex((o) => !o.trim());
        if (at < 0 && options.length >= 6) break;
        if (at < 0) options.push(t);
        else {
          options[at] = t;
          correctNow = correctNow.filter((c) => c !== at); // 빈 칸에 남은 정답 체크는 따라오지 않는다
        }
      }
      left = left.filter((x) => x !== t);
    }
    onChange({ ...value, options, correct: correctNow });
    keep(left);
  };

  let body;
  if (phase === "busy")
    body = (
      <div className="dq__wait" role="status">
        <span className="dq__lead">
          <Icon name="rotate" size={16} />
          <span className="t-label-m">{slow ? "오래 걸려요… 보기를 직접 써도 돼요" : "ChatGPT가 오답 보기를 만드는 중… 보기를 직접 써도 돼요"}</span>
        </span>
        <button type="button" className="ai-link t-label-m" onClick={cancel}>
          취소
        </button>
      </div>
    );
  else if (phase === "error" && fail) {
    const [icon, tone, text] = dqError(fail);
    const action: [string, () => void] | undefined =
      fail.kind === "expired" || fail.kind === "scope" ? ["다시 연결", () => setReconnect(fail.kind === "scope")]
      : fail.kind === "limit" ? ["사용량 관리", openUsage]
      : fail.kind === "not_eligible" ? undefined
      : ["다시 시도", send];
    body = <DqRow icon={icon} tone={tone} text={text} action={action} />;
  } else if (phase === "done")
    body = (
      <>
        <p className="dq__warn t-label-m">
          <Icon name="alert" size={16} />
          AI가 지은 보기예요. 사실은 정답이 아닌지 확인한 뒤 넣어 주세요.
        </p>
        {offered.map((t) => (
          <div key={t} className="dq__opt">
            <Icon name="sparkles" size={14} className="c-accent" />
            <span className="dq__text t-body-m">{t}</span>
            <span className="t-label-s c-accent">제안</span>
            <Button variant="secondary" size="M" label="넣기" disabled={!room} onClick={() => insert([t])} />
            <Button variant="ghost" size="M" label="빼기" onClick={() => keep(offered.filter((x) => x !== t))} />
          </div>
        ))}
        <span className="dq__more">
          <button type="button" className="ai-link t-label-m" disabled={!room} onClick={() => insert(offered)}>
            모두 넣기
          </button>
          <button type="button" className="ai-link t-label-m" disabled={!want || !correct.length} onClick={send}>
            다시 제안
          </button>
          <span className="t-caption c-tertiary">{sentWhat}</span>
        </span>
      </>
    );
  else if (!want) body = <DqRow icon="info" tone="c-secondary" text="보기가 6개라 더 받을 수 없어요" />;
  else {
    const rows: SendRow[] = [
      { key: "q", icon: "layers", meta: <span className="t-label-s c-secondary">질문</span>, text: plainText(question()) },
      { key: "a", icon: "check", meta: <span className="t-label-s c-secondary">정답</span>, text: correct.join(" · ") },
      ...sending.map((t) => ({
        key: `w-${t}`,
        icon: "list" as const,
        meta: <span className="t-label-s c-secondary">이미 쓴 보기</span>,
        text: t,
        remove: () => setSkip([...skip, t]),
        removeLabel: "이 보기 빼기",
      })),
      ...avoid.map((t) => ({
        key: `p-${t}`,
        icon: "sparkles" as const,
        meta: <span className="t-label-s c-secondary">앞서 받은 제안</span>,
        text: t,
        remove: () => setPrior(prior.filter((x) => x !== t)), // 빼면 피하지 않는다 (다시 나올 수 있다)
        removeLabel: "이 제안 빼기",
      })),
    ];
    body = (
      <>
        <button type="button" className="dq__line" disabled={!correct.length} onClick={start}>
          <Icon name="sparkles" size={16} />
          <span className="t-label-m">{`오답 보기 ${n(want)}개 제안 받기`}</span>
          <span className="t-caption c-tertiary">{!correct.length ? "정답에 체크하면 받을 수 있어요" : filled.length < 4 ? "4지선다까지 채워요" : "보기는 6개까지예요"}</span>
        </button>
        <ScopeLine text={`${what}${eul(what)} 보내요`} open={phase === "confirm"} onToggle={() => setPhase((p) => (p === "confirm" ? "idle" : "confirm"))} />
        {phase === "confirm" && (
          // L4-s · R2-s 와 같은 확인. Esc 는 이 확인만 닫는다 (편집 창은 그대로)
          <div className="dq__confirm" role="dialog" aria-label="OpenAI로 보낼 내용" onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setPhase("idle"))}>
            <SendList
              compact
              rows={rows}
              foot={
                <div className="grade__confirm-foot">
                  <UsingPlan />
                  <span className="grade__actions">
                    <Button variant="ghost" size="M" label="취소" onClick={() => setPhase("idle")} />
                    <Button size="M" icon="send" label="보내기" autoFocus disabled={!correct.length} onClick={send} />
                  </span>
                </div>
              }
            />
          </div>
        )}
      </>
    );
  }
  return (
    <div className="dq" ref={box}>
      {body}
      {reconnect !== null &&
        createPortal(
          <AiConnect
            start="wait"
            reconsent={reconnect}
            onDone={(ok) => {
              setReconnect(null);
              if (ok) setPhase("idle");
            }}
          />,
          document.body,
        )}
    </div>
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
