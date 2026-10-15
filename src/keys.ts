// 단축키 표 (Figma E4 45:5593). 바꿀 수 있는 동작만 여기 있다. Esc · Enter · Tab 같은 고정 키와 화면 안 메뉴 키는 각 화면에 그대로 둔다.
// 키는 KeyboardEvent.code 로 저장한다 (한/영 상태와 무관). 조합은 "Ctrl+KeyZ" 처럼 Ctrl · Alt · Shift 를 앞에 붙인다.
// 표시는 영문 대문자 (KeyE -> "E", Ctrl+KeyZ -> "Ctrl Z")
import { useSyncExternalStore } from "react";

export type Scope = "study" | "app" | "occlusion";
export type ActionId = "reveal" | "rate" | "edit" | "bury" | "undo" | "source" | "grade" | "replay" | "newDeck" | "import" | "addCard" | "save" | "math";

export const SCOPE_LABEL: Record<Scope, string> = { study: "학습 화면", app: "앱 전체", occlusion: "가리기 편집기" };

/** 표의 순서 그대로. rate 는 한 행에 키 4개 (Again · Hard · Good · Easy) */
export const ACTIONS: { id: ActionId; label: string; scope: Scope; keys: string[] }[] = [
  { id: "reveal", label: "답 보기", scope: "study", keys: ["Space"] },
  { id: "rate", label: "평가 · Again / Hard / Good / Easy", scope: "study", keys: ["Digit1", "Digit2", "Digit3", "Digit4"] },
  { id: "edit", label: "카드 편집", scope: "study", keys: ["KeyE"] },
  { id: "bury", label: "접어두기", scope: "study", keys: ["Minus"] },
  { id: "undo", label: "평가 되돌리기", scope: "study", keys: ["Ctrl+KeyZ"] },
  // I2: 연결된 자료가 있는 카드의 답 화면에서 원자료 패널을 열고 닫는다. 학습 화면 · 앱 범위 키와 겹치지 않음 확인 (2026-10-14)
  { id: "source", label: "연결 자료 보기", scope: "study", keys: ["KeyS"] },
  // R2: 서술형 비교 화면(J2)에서 ChatGPT 채점 보조. 학습 화면 키(Space · 1–4 · E · − · Ctrl Z · S)와 고정 키(Esc · B), 앱 범위 키(N · I · A)와 겹치지 않음 확인 (2026-10-15)
  { id: "grade", label: "ChatGPT 채점 보조", scope: "study", keys: ["KeyG"] },
  // AU3: 지금 면의 소리 다시 듣기 (Anki 와 같은 R). 학습 화면 키(Space · 1–4 · E · − · Ctrl Z · S · G) · 고정 키(Esc · B) · 앱 범위 키(N · I · A)와 겹치지 않음 확인.
  // 가리기 편집기의 R(사각형)은 편집기 안에서만이라 따로 (2026-10-15)
  { id: "replay", label: "소리 다시 듣기", scope: "study", keys: ["KeyR"] },
  { id: "newDeck", label: "새 덱", scope: "app", keys: ["KeyN"] },
  { id: "import", label: "가져오기", scope: "app", keys: ["KeyI"] },
  { id: "addCard", label: "카드 추가", scope: "app", keys: ["KeyA"] },
  { id: "save", label: "저장", scope: "app", keys: ["Ctrl+Enter"] },
  { id: "math", label: "수식 넣기", scope: "app", keys: ["Ctrl+KeyM"] },
];
const DEFAULTS = Object.fromEntries(ACTIONS.map((a) => [a.id, a.keys])) as Record<ActionId, string[]>;
export const actionOf = (id: ActionId) => ACTIONS.find((a) => a.id === id)!;

/** 표에 "바꿀 수 없음"으로 보이는 고정 단축키 (숨은 단축키가 없게). 키는 각 화면이 글자로 받는다 */
export const FIXED_ROWS: { label: string; caption: string; scope: Scope; key: string }[] = [
  { label: "그만하기", caption: "창 닫기·취소와 같은 키라 고정", scope: "study", key: "Esc" },
  { label: "접어둔 카드 목록", caption: "접어둔 카드가 있을 때 열고 닫아요", scope: "study", key: "B" },
  { label: "이름 바꾸기", caption: "덱 상세에서", scope: "app", key: "F2" },
  // M 가리기 편집기 (M5). 편집기 안에서만 동작한다
  { label: "도구 · 선택 / 사각형 / 타원", caption: "", scope: "occlusion", key: "V R O" },
  { label: "선택한 칸 지우기", caption: "", scope: "occlusion", key: "Delete" },
  { label: "칸 옮기기", caption: "1px · Shift 10px", scope: "occlusion", key: "방향키" },
  { label: "복제", caption: "", scope: "occlusion", key: "Ctrl D" },
  { label: "묶기 · 풀기", caption: "같은 카드로 묻는다", scope: "occlusion", key: "Ctrl G · Ctrl Shift G" },
  { label: "다음 칸 · 칸 이름", caption: "Shift Tab 이전 칸", scope: "occlusion", key: "Tab · Enter" },
  { label: "실행 취소 · 다시", caption: "50단계", scope: "occlusion", key: "Ctrl Z · Ctrl Shift Z" },
  { label: "화면 이동 · 확대", caption: "Ctrl = / − 로도 확대", scope: "occlusion", key: "Space+끌기 · Ctrl+휠" },
  { label: "가리기 적용 · 변경 저장", caption: "Esc 를 두 번 누르면 마칠지 물어요", scope: "occlusion", key: "Ctrl Enter" },
];

/** 바꿀 수 없는 키 (E4): Esc 그만하기 · 닫기, Enter 확인, Tab 초점 이동. 조합 없이 누른 경우만 */
export const FIXED = new Set(["Escape", "Enter", "Tab"]);

// ---------------------------------------------------------------- 저장 (이 기기의 localStorage)

const STORE = "neoanki.keys";
/** 바꾼 동작만 저장한다. 빈 문자열은 "미지정" (같은 범위에서 다른 동작에 내준 키) */
type Overrides = Partial<Record<ActionId, string[]>>;

function load(): Overrides {
  try {
    const v = JSON.parse(localStorage.getItem(STORE) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

let overrides = load();
let snapshot = { ...DEFAULTS, ...overrides } as Record<ActionId, string[]>;
const listeners = new Set<() => void>();

function commit(next: Overrides) {
  overrides = next;
  snapshot = { ...DEFAULTS, ...overrides } as Record<ActionId, string[]>;
  try {
    if (Object.keys(overrides).length) localStorage.setItem(STORE, JSON.stringify(overrides));
    else localStorage.removeItem(STORE);
  } catch {
    // 저장이 막힌 환경. 이번 실행에만 적용된다
  }
  listeners.forEach((f) => f());
}

export const getKeys = (id: ActionId) => snapshot[id];
export const isChanged = (id: ActionId) => id in overrides;
export const keymap = () => snapshot;

/** 동작의 키를 바꾼다. 기본값과 같으면 바꾼 목록에서 뺀다 */
export function setKeys(id: ActionId, keys: string[]) {
  const next = { ...overrides };
  if (keys.join() === DEFAULTS[id].join()) delete next[id];
  else next[id] = keys;
  commit(next);
}
export const resetKeys = (id: ActionId) => setKeys(id, DEFAULTS[id]);
export const resetAllKeys = () => commit({});
/** 되돌리기 토스트용 */
export const exportKeys = () => ({ ...overrides });
export const importKeys = (o: Overrides) => commit({ ...o });

/** 화면이 바뀐 키로 다시 그려지게 (힌트 줄, 키 칩) */
export function useKeymap() {
  return useSyncExternalStore(
    (f) => {
      listeners.add(f);
      return () => listeners.delete(f);
    },
    () => snapshot,
  );
}

// ---------------------------------------------------------------- 키 읽기 · 맞추기 · 표시

/** 숫자패드는 윗줄 숫자와 같은 키로 본다 (예전 e.key 비교와 같게) */
const SAME: Record<string, string> = { NumpadEnter: "Enter", NumpadSubtract: "Minus", NumpadAdd: "Equal", NumpadDecimal: "Period" };
const normCode = (code: string) => SAME[code] ?? code.replace(/^Numpad(\d)$/, "Digit$1");

/** 키 입력 하나를 저장 형식으로. 조합 키만 누른 경우는 null */
export function chordOf(e: KeyboardEvent): string | null {
  if (["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"].includes(e.code)) return null;
  return [e.ctrlKey || e.metaKey ? "Ctrl" : "", e.altKey ? "Alt" : "", e.shiftKey ? "Shift" : "", normCode(e.code)].filter(Boolean).join("+");
}

/** 이 입력이 동작의 몇 번째 키인가. 아니면 -1 (평가는 0~3) */
export function matchAction(id: ActionId, e: KeyboardEvent): number {
  const c = chordOf(e);
  return c ? snapshot[id].indexOf(c) : -1;
}

const NAMES: Record<string, string> = {
  Space: "Space", Minus: "-", Equal: "=", Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/",
  Backslash: "\\", Backquote: "`", BracketLeft: "[", BracketRight: "]", Enter: "Enter", Backspace: "Backspace",
  Delete: "Del", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Home: "Home", End: "End",
  PageUp: "PgUp", PageDown: "PgDn", Insert: "Ins",
};
/** "Ctrl+KeyZ" -> "Ctrl Z", "Minus" -> "-", "" -> "미지정" */
export function keyLabel(chord: string): string {
  if (!chord) return "미지정";
  return chord
    .split("+")
    .map((p) => NAMES[p] ?? p.replace(/^Key/, "").replace(/^Digit/, ""))
    .join(" ");
}
/** 동작의 지금 키 이름. 평가는 index 로 하나씩 */
export const actionKey = (id: ActionId, index = 0) => keyLabel(snapshot[id][index] ?? "");

/** 대비 계산 같은 순수 함수 검사처럼, 키 이름 변환을 cdpp 로 확인할 수 있게 */
export function selfCheck() {
  const ok = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
  return {
    label: ok([keyLabel("Ctrl+KeyZ"), keyLabel("Minus"), keyLabel("Digit3"), keyLabel("Semicolon"), keyLabel("")], ["Ctrl Z", "-", "3", ";", "미지정"]),
    numpad: chordOf(new KeyboardEvent("keydown", { code: "Numpad2", key: "2" })) === "Digit2",
    ctrl: chordOf(new KeyboardEvent("keydown", { code: "KeyZ", key: "ㅋ", ctrlKey: true })) === "Ctrl+KeyZ",
    modifierOnly: chordOf(new KeyboardEvent("keydown", { code: "ShiftLeft", key: "Shift", shiftKey: true })) === null,
  };
}
