// J 서술형 · H3/8.5 시험 전 회독 · 8.4 기출 · G3 학습 중 신호 (src-tauri/src/written.rs · cram.rs · exam.rs). 필드 이름은 Rust 와 같다
import { invoke } from "@tauri-apps/api/core";
import { studyDate } from "./nav";
import type { ExamState } from "./api";

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

/** 8.3 개념 관계 하나. verdict: ok 맞음 · missing 빠짐 · reversed 거꾸로 · negated 부정 · wrong 틀림 (llm.rs) */
export type Relation = { rel: string; verdict: "ok" | "missing" | "reversed" | "negated" | "wrong" };
/** 8.3 개념 관계 확인 결과 (참고용, 평가 추천에는 쓰지 않는다). verdict 는 관계 판정에서 규칙으로 낸 전체 판정 */
export type Concept = { relations: Relation[]; verdict: "correct" | "partial" | "missing" | "wrong"; ms: number };
/** 8.3 로컬 모델 상태 (llm.rs LlmStatus). free: 남은 공간 (모르면 null), need: 받기 전에 비워 둘 공간.
 * cpu_ok · engine: 이 PC 에서 쓸 수 있는가 (CPU AVX2 · 엔진 파일), off: 엔진이 다시 띄워도 멈춰 이번 학습에서 껐다, last_ms: 지난 확인 한 번 */
export type LlmStatus = {
  ready: boolean;
  downloading: boolean;
  bytes: number;
  free: number | null;
  need: number;
  name: string;
  short: string;
  license: string;
  source: string;
  cpu_ok: boolean;
  engine: boolean;
  off: boolean;
  last_ms: number | null;
  /** 마지막 받기가 실패하거나 멈춘 까닭 (다 받으면 null) */
  failed: string | null;
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
  /** 기본 방식 (3일 이내 압축) */
  mode: "cram" | "mix";
  /** 혼합 회독: 배운 카드 · 이번 회독에서 본 카드 */
  learned: number;
  round_seen: number;
  /** 오늘 이미 일정에 반영해서 이번 분량에서 빼는 카드 */
  today_seen: number;
  /** 이 덱의 오늘 하루 신규 몫 (큐 · 계획 그래프와 같은 산식) */
  new_per_day: number;
};

/** 혼합 회독 한 자리. weak: 취약 카드로 끼운 자리면 이유 */
export type MixCard = { id: number; weak: string | null };
export type MixSummary = { fed: number; learned: number; round_seen: number; hot: { deck_id: number; name: string; again: number } | null };
/** 혼합 회독 한 번 (P1 → P2 → P3). before: 시작 전 값 (P3 의 "→" 비교) */
export type MixRun = {
  deckId: number;
  name: string;
  cards: MixCard[];
  dueToday: number[];
  before: { seen: number; learned: number; today: number };
  /** P3 [이어서 N장]이 같은 비율 · 분량으로 다음 묶음을 만든다 */
  every: number;
  size: number;
  /** 이 묶음을 시작한 시각 (P3 이 이 뒤 기록만 센다) */
  startedAt: number;
};
/** P2 에서 센 것 (P3) */
export type MixStats = { full: [number, number]; weak: [number, number]; again: [number, number]; due: number };

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
  /** 8.3 개념 관계 확인 (로컬 모델, 백그라운드). 형식이 틀리면 null. 새 확인 · stopConcept 이 앞의 것을 멈춘다 */
  concept: (cardId: number, answer: string) => invoke<Concept | null>("concept_check", { cardId, answer }),
  /** 서술형 카드가 나오면 (답을 쓰는 동안): 모델을 읽어 두고 관계 뽑기 · 판정 앞부분을 미리 한다. 모델이 없으면 아무것도 하지 않는다 */
  prepareConcept: (cardId: number, token: number) => invoke<void>("concept_prepare", { cardId, token }),
  /** 그 표의 미리 하기를 멈춘다 (카드를 떠날 때, 엔진 잠금을 놓게) */
  stopPrepare: (token: number) => invoke<void>("llm_stop_prepare", { token }),
  stopConcept: () => invoke<void>("llm_stop"),
};

/** 8.3 개념 관계 확인 모델 (llm.rs). 받기 진행은 "llm-progress" 이벤트 */
export const llm = {
  status: () => invoke<LlmStatus>("llm_status"),
  download: () => invoke<void>("llm_download"),
  cancel: () => invoke<void>("llm_cancel"),
  remove: () => invoke<void>("llm_delete"),
  /** 학습 화면을 떠나면 메모리를 돌려준다 */
  release: () => invoke<void>("llm_release"),
};

/** 8.4 기출 (exam.rs). target "note" 면 노트 id, "segment" 면 자료 구간 id */
export const exam = {
  /** 시험 하나를 더한다. 같은 시험이 이미 있는 것은 건너뛴다. "card" 는 카드의 노트에. 바뀐 수 */
  mark: (target: "note" | "segment" | "card", ids: number[], name: string | null) => invoke<number>("mark_exam", { target, ids, exam: name }),
  /** 그대로 쓰기: 빼기 (hits null) · 되돌리기 */
  set: (target: "note" | "segment", states: ExamState[]) => invoke<void>("set_exam", { target, states }),
  list: (deckId: number | null) => invoke<{ name: string; cards: number }[]>("exam_list", { deckId }),
  summary: (deckId: number) =>
    invoke<{ cards: number; twice: number; exams: number; unseen: number; card_ids: number[] }>("exam_summary", { deckId }),
};

export const cram = {
  plan: (deckIds: number[] | null) => invoke<CramPlan | null>("cram_plan", { deckIds }),
  cards: (deckId: number, scope: number) => invoke<number[]>("cram_cards", { deckId, scope }),
  /** marks: Q 목록 · 선택형 (압축 회독 결과도 "지난번"이 된다) */
  answer: (cardId: number, rating: number, elapsedMs: number, marks: number[] | null) => invoke<number>("cram_answer", { cardId, rating, elapsedMs, marks }),
  undo: (logId: number) => invoke<void>("cram_undo", { logId }),
  /** 8.5 혼합: every = 전체 몇 장마다 취약 1장, size = 한 번에 할 양 */
  mixCards: (deckId: number, every: number, size: number) => invoke<MixCard[]>("mix_cards", { deckId, every, size }),
  mixStart: (deckId: number, every: number, size: number) =>
    invoke<{ session_id: number; cards: MixCard[]; due_today: number[] }>("mix_start", { deckId, every, size }),
  mixRestart: (deckId: number) => invoke<void>("mix_restart", { deckId }),
  /** since: 이번 묶음을 시작한 시각 (같은 날 이어 쓴 세션의 앞 묶음은 빼고 센다) */
  mixSummary: (sessionId: number, deckId: number, since: number) => invoke<MixSummary>("mix_summary", { sessionId, deckId, since }),
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
