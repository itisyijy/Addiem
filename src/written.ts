// J 서술형 · H3 압축 회독 · G3 학습 중 신호 (src-tauri/src/written.rs · cram.rs). 필드 이름은 Rust 와 같다
import { invoke } from "@tauri-apps/api/core";
import { studyDate } from "./nav";

/** J3 필수 키워드. source: user(직접 추가) | suggested(제안을 눌러 확정) */
export type Keyword = { word: string; synonyms: string[]; source: string };

export type WrittenCard = { written: boolean; answer: string; keywords: Keyword[]; last: string | null; last_at: number | null };

/** J2 비교. 위치는 JS 문자열 위치 [시작, 끝) */
export type Compare = {
  correct: string;
  keywords: { word: string; hit: boolean }[];
  hits: number;
  /** 1 Again · 2 Hard · 3 Good. 키워드가 없으면 null */
  suggest: 1 | 2 | 3 | null;
  mine_hits: [number, number][];
  correct_hits: [number, number][];
  correct_misses: [number, number][];
};

export type WrittenNote = {
  capable: boolean;
  native: boolean;
  write: boolean;
  answer_field: number | null;
  keywords: Keyword[];
  dismissed: string[];
};

export type CramPlan = {
  deck_id: number;
  name: string;
  dday: number;
  core: number;
  core_total: number;
  weak: number;
  weak_names: string[];
  again: number;
  stable: number;
  /** 핵심만 · 핵심 + 약한 챕터 · 불안한 카드 전부 */
  scopes: [number, number, number];
  sec_per_card: number;
};

export const written = {
  card: (cardId: number) => invoke<WrittenCard>("written_card", { cardId }),
  saveAnswer: (cardId: number, answer: string) => invoke<void>("save_written_answer", { cardId, answer }),
  compare: (cardId: number, answer: string) => invoke<Compare>("compare_written", { cardId, answer }),
  note: (noteId: number) => invoke<WrittenNote>("written_note", { noteId }),
  saveNote: (noteId: number, write: boolean, keywords: Keyword[], dismissed: string[]) =>
    invoke<void>("save_written_note", { noteId, write, keywords, dismissed }),
  suggest: (answer: string, exclude: string[]) => invoke<string[]>("suggest_keywords", { answer, exclude }),
  add: (card: { deck_id: number; question: string; answer: string; memo: string; importance: number | null; write: boolean; keywords: Keyword[] }) =>
    invoke<number>("add_written_card", { card }),
  /** G3 토스트 둘째 줄의 이유 ("답이 10항목이라"). 카드 모양 문제가 없으면 null */
  splitSignal: (cardId: number) => invoke<string | null>("split_signal", { cardId }),
};

export const cram = {
  plan: (deckIds: number[] | null) => invoke<CramPlan | null>("cram_plan", { deckIds }),
  cards: (deckId: number, scope: number) => invoke<number[]>("cram_cards", { deckId, scope }),
  answer: (cardId: number, rating: number, elapsedMs: number) => invoke<number>("cram_answer", { cardId, rating, elapsedMs }),
  undo: (logId: number) => invoke<void>("cram_undo", { logId }),
};

// ---------------------------------------------------------------- G3 "나중에 볼 제안" (이 기기에서만)

const SIGNALED = "neo.splitSignaled"; // { 카드 id: 신호를 보인 학습일 }
const LATER = "neo.splitLater"; // [카드 id] — D5 접어둔 카드 행에 [나누기 제안] 칩
const today = () => studyDate().toLocaleDateString("sv-SE"); // 로컬 YYYY-MM-DD (toISOString 은 UTC 라 하루 어긋난다)

function read<T>(key: string, fallback: T): T {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "null") ?? fallback;
  } catch {
    return fallback;
  }
}
function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* 저장이 막힌 환경. 신호가 하루 두 번 보이거나 칩이 빠질 뿐이다 */
  }
}

/** 같은 카드에 신호는 하루 한 번. 오늘 처음이면 true 를 돌려주고 기록한다 */
export function takeSignal(cardId: number) {
  const seen = read<Record<string, string>>(SIGNALED, {});
  if (seen[cardId] === today()) return false;
  // 지난 날 기록은 버린다 (쌓이지 않게)
  write(SIGNALED, { ...Object.fromEntries(Object.entries(seen).filter(([, d]) => d === today())), [cardId]: today() });
  return true;
}

export const splitLater = {
  ids: () => read<number[]>(LATER, []),
  has: (cardId: number) => splitLater.ids().includes(cardId),
  add: (cardId: number) => write(LATER, [...new Set([...splitLater.ids(), cardId])]),
  /** G2 제안 창에서 나누거나 그대로 두기로 정했을 때 */
  remove: (cardId: number) => write(LATER, splitLater.ids().filter((id) => id !== cardId)),
};
