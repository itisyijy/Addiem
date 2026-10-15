// Rust 명령(src-tauri/src/commands.rs)의 타입과 호출. 필드 이름은 Rust 와 같다
import type { Sent } from "./sourceApi";
import { invoke } from "@tauri-apps/api/core";

export type Level = 0 | 1 | 2;

export type HomeDeck = { id: number; name: string; review: number; new: number; tomorrow: number; dday: number | null };
export type Home = {
  has_decks: boolean;
  review: number;
  new: number;
  done_today: number;
  decks: HomeDeck[];
  /** 오늘 볼 카드가 없는 덱. 쉬는 덱이 맨 위 (A2-p) */
  idle_decks: { id: number; name: string; paused: boolean; paused_until: string | null; next_due: string | null }[];
  tomorrow: number;
  buried: number;
  /** 보관한 최상위 덱 수 */
  archived: number;
  /** 쉬기 날짜가 지나 오늘 다시 시작한 덱 이름 (토스트) */
  resumed: string[];
};

export type Subdeck = {
  id: number;
  name: string;
  cards: number;
  importance: Level;
  importance_source: string;
  /** C1-t 트리: 부모 덱 id 와 깊이 (이 덱 바로 아래가 1) */
  parent_id: number;
  depth: number;
  /** 오늘 볼 카드 (아래 덱 전부). 행들의 합이 덱의 오늘 분량과 같다 */
  today: number;
  unseen: number;
  /** 바로 아래 하위 덱 수 */
  children: number;
  /** 아래 덱들의 중요도가 섞였는가 ([나뉨]) */
  mixed: boolean;
  paused: boolean;
  /** C1-w 최근 14일(오늘 포함) 복습 수와 Again 비율 0~1. 복습이 없으면 null. 20번 미만이면 "기록 적음" */
  reviews14: number;
  again_rate: number | null;
  /** 3번 이상 틀린 카드 (제안 근거 줄) */
  lapsed: number;
  /** 약함으로 확정 */
  weak: boolean;
  /** "약해 보여요?" 제안 (복습 20번 이상 + 덱 평균의 1.5배 이상 또는 25% 이상, [아니에요] 뒤 14일 쉼) */
  weak_suggested: boolean;
  /** 확정했는데 덱 평균 아래로 내려옴 ("요즘 괜찮아요 · 해제할까요?" 한 번만) */
  weak_recovered: boolean;
};
export type DeckDetail = {
  id: number;
  /** 하위 덱이면 바로 위 덱 */
  parent_id: number | null;
  name: string;
  path: string[];
  total: number;
  unseen: number;
  hard: number;
  backlog: number;
  today_review: number;
  today_new: number;
  today_returning: number;
  reason: string | null;
  target_date: string | null;
  include_target_day: boolean;
  daily_new_limit: number | null;
  today_new_override: number | null;
  dday: number | null;
  seen_pct: number;
  core_total: number;
  core_seen: number;
  core_learned_pct: number;
  per_day: number;
  finish_days: number | null;
  learnable_days: number | null;
  on_track: boolean | null;
  subdeck_count: number;
  studied: boolean;
  importance: [number, number, number]; // 핵심, 일반, 보충
  done_today: number;
  paused: boolean;
  /** 'YYYY-MM-DD' 까지 쉰다 (C1-p). 기한 없이 쉬면 null */
  paused_until: string | null;
  own_importance: Level;
  unconfirmed: number;
  /** 바로 아래 하위 덱 */
  subdecks: Subdeck[];
  /** 모든 하위 덱, 깊이 우선 (C1-t) */
  tree: Subdeck[];
  /** 이 덱에 목표일이 없고 상위 덱 목표를 따를 때 (C1-t "상위 덱 목표 D-12를 따라요") */
  target_from: { name: string; dday: number } | null;
  /** C1-w 이 덱 전체의 최근 14일 Again 비율 (덱 평균) */
  again_avg: number | null;
  /** J1 · C4 이 덱이 "써서 답하기"를 켰는가 */
  write_mode: boolean;
  /** 위 덱의 "써서 답하기"를 따르면 그 덱 이름 */
  write_from: string | null;
};

export type NewCard = { deck_id: number; kind: "basic" | "reversed" | "cloze"; front: string; back: string; importance: Level | null };
export type CardRow = {
  id: number;
  note_id: number;
  deck_path: string[];
  question: string;
  answer: string;
  state: "new" | "learning" | "review" | "relearning" | "suspended";
  importance: Level;
  own_importance: boolean;
  due_at: number | null;
  lapses: number;
  buried: boolean;
  /** 8.4 노트의 기출. id 는 노트 id */
  exam: ExamState;
};
/** 8.4 기출 (exam.rs). hits null 이면 표시 없음 */
export type ExamState = { id: number; hits: number | null; names: string[] };
export type CardList = { rows: CardRow[]; total: number };

export type DeckConfig = {
  deck_id: number;
  target_date: string | null;
  include_target_day: boolean;
  daily_new_limit: number | null;
  today_new: number | null;
  /** 잠시 쉬기 (C4-p). 빼면 지금 상태를 그대로 둔다 */
  paused?: boolean;
  /** 'YYYY-MM-DD' 까지 (그날 포함). null 이면 다시 시작할 때까지 */
  paused_until?: string | null;
  /** J1 "이 덱은 써서 답하기". 빼면 지금 상태를 그대로 둔다 */
  write_mode?: boolean;
};
export type ConfigPreview = {
  remaining_new: number;
  days: number | null;
  per_day: number;
  needed_per_day: number | null;
  overflow_days: number | null;
  finish_date: string | null;
  never_finish: boolean;
  today_total: number;
  today_new: number;
  /** 쉬는 동안 예정된 복습 수. 다시 시작하는 날 밀린 복습으로 온다. 기한 없는 쉬기면 null */
  paused_reviews: number | null;
  /** 다시 시작하는 날 */
  resume_date: string | null;
};

export type QueueSummary = {
  total: number;
  learning: number;
  returning: number;
  review: number;
  backlog: number;
  backlog_deferred: number;
  new_core: number;
  new_normal: number;
  new_supp: number;
  core_total: number;
  /** 복습 묶음 중 약한 챕터라 먼저 보여주는 장수 (D1 "약한 챕터 먼저 · N장") */
  weak_first: number;
  deck_count: number;
  new_decks: number;
  notes: string[];
  target: { name: string; dday: number; remaining_new: number; days: number } | null;
};

export type CardView = { id: number; note_id: number; css: string; front: string; back: string; deck_path: string[]; importance: Level; state: string };
export type NoteEdit = {
  note_id: number;
  /** image_occlusion: 가리기 편집기(M2)로 연다 */
  kind: "basic" | "cloze" | "image_occlusion" | "other";
  /** 노트 유형 이름 */
  type_name: string;
  names: string[];
  values: string[];
  /** 필드마다 앞면 템플릿에 쓰이는가 */
  front: boolean[];
};
export type AnswerResult = { action: "next" | "again_later" | "buried"; due_in_ms: number | null };
export type BuriedCard = { id: number; question: string; reason: string; importance: Level; deck_path: string[] };
export type SessionSummary = {
  studied: number;
  new_done: number;
  review_done: number;
  again: number;
  minutes: number;
  buried: BuriedCard[];
  remaining_review: number;
  remaining_new: number;
  tomorrow: number;
  tomorrow_review: number;
  tomorrow_new: number;
  tomorrow_decks: { name: string; review: number; new: number; dday: number | null }[];
};

export type ImportIssue = { severity: "skipped" | "warning" | "info"; kind: string; detail: string; index: number | null };
export type ImportResult = {
  source: string;
  kind: "apkg" | "colpkg" | "json";
  decks: number;
  subdecks: number;
  notes: number;
  cards: number;
  media: number;
  skipped: number;
  reviews: number;
  history: boolean;
  filtered_decks: number;
  filtered_cards: number;
  check_cards: number;
  root_deck_id: number | null;
  roots: number[];
  issues: ImportIssue[];
};
export type JsonPreview = {
  ok: number;
  skipped: number;
  warnings: number;
  issues: ImportIssue[];
  roots: string[];
  new_decks: number;
  error: string | null;
};
export type AnkiPreview = {
  file_name: string;
  bytes: number;
  decks_top: number;
  decks_sub: number;
  notes: number;
  cards: number;
  media: number;
  note_types: number;
  note_types_ok: number;
  degraded: { feature: string; note_types: string[]; cards: number }[];
  filtered_decks: number;
  filtered_cards: number;
  reviews: number;
  /** 복습 기록을 옮기면 오늘 기준 밀려 있을 복습 수 (B2-r) */
  overdue: number;
  /** [latex] 태그를 쓰는 카드. 그중 LaTeX 문서 전체라 원문으로 보이는 카드 (B2 "LaTeX 태그 N장 · 일부는 원문으로 보여요") */
  latex_cards: number;
  latex_raw_cards: number;
  /** B2-s 덱 고르기. 카드가 있는 실제 덱(필터 덱이면 원래 덱)마다 그 덱 자기 카드의 합계. id 는 importAnki 의 deckIds */
  deck_tree: {
    id: number;
    path: string[];
    cards: number;
    notes: number;
    /** 이미 앱에 있는 노트. 가져와도 건너뛴다 */
    existing_notes: number;
    /** 아래부터는 실제로 들어갈 것만 (이미 있는 노트의 카드는 뺀다) */
    new_cards: number;
    reviews: number;
    overdue: number;
    latex_cards: number;
    latex_raw_cards: number;
    /** 쓰는 노트타입 (Anki 모델 id) */
    types: number[];
    degraded: { feature: string; note_types: string[]; cards: number }[];
  }[];
  /** 원본과 다르게 보일 수 있는 노트타입 (Anki 모델 id) */
  degraded_types: number[];
  /** 지운 카드의 기록. 덱을 전부 골랐을 때만 함께 옮긴다 */
  orphan_reviews: number;
};
/** import_anki 가 보내는 "import-progress" 이벤트 (B2-p) */
export type ImportProgress = { step: "unzip" | "decks" | "cards" | "media" | "done"; done: number; total: number };
/** export_apkg 결과. missing_media: 노트가 쓰지만 미디어 폴더에 없어 빠진 파일 수 */
export type ExportResult = {
  notes: number;
  cards: number;
  media: number;
  missing_media: number;
  reviews: number;
  path: string;
  /** 찾지 못한 미디어를 쓰는 카드 (C1-xr [카드 보기]) */
  missing_cards: number[];
};
/** C1-xe 덱 상자 */
export type ExportPreview = { path: string[]; subdecks: number; notes: number; cards: number; media: number; reviews: number };
/** export_apkg 가 보내는 "export-progress" 이벤트 (C1-xp) */
export type ExportProgress = { step: "notes" | "reviews" | "media" | "zip"; done: number; total: number };
export type BoardDeck ={ id: number; label: string; path: string[]; cards: number; importance: Level; source: string; reason: string | null };
export type MvpMetrics = {
  /** 목표일이 있는 덱 수. 0 이면 ① 대신 "목표 정하기" (E2-0) */
  target_decks: number;
  /** ① 최근 14일(오늘 제외) 중 계획이 있던 날, 목표일 덱 합산. 날짜 오름차순 */
  plan_days: { day: string; planned: number; done: number }[];
  /** ① 일별 완료율 중앙값 0~1 */
  plan_median: number | null;
  /** ① 목표일 덱의 계획이 처음 남은 날 ("판정 전 · N일 남음") */
  first_plan_day: string | null;
  /** ② 판정 전 남은 날 계산용: 이 앱에서 처음 학습한 날 */
  first_session_day: string | null;
  /** ② 보류가 생긴 세션 (오래된 것부터 최근 20개). true = 끝까지 마침 */
  sessions: boolean[];
  bury_rate: number | null;
  bury_sessions: number;
  bury_completed: number;
};

/** E2-h 하루 한 칸 */
export type HeatDay = { day: string; cards: number; decks: number };

export type ArchivedDeck = {
  id: number;
  name: string;
  archived_at: number;
  cards: number;
  subdecks: number;
  target_date: string | null;
  /** 목표일이 지난 덱 (E3 회색 배지) */
  target_passed: boolean;
};
export type AppInfo = {
  /** 학습 데이터를 열지 못한 이유 (S1-6). 있으면 지금은 빈 데이터다 */
  startup_error: string | null;
  data_dir: string;
  backup_dir: string;
  last_backup: number | null;
  cutoff_hour: number;
  db_bytes: number;
  cards: number;
  decks: number;
  reviews: number;
  version: string;
};

export const api = {
  home: () => invoke<Home>("home"),
  createDeck: (name: string, parentId: number | null = null) => invoke<number>("create_deck", { name, parentId }),
  renameDeck: (deckId: number, name: string) => invoke<void>("rename_deck", { deckId, name }),
  archiveDeck: (deckId: number) => invoke<void>("archive_deck", { deckId }),
  deleteDeck: (deckId: number) => invoke<void>("delete_deck", { deckId }),
  archivedDecks: () => invoke<ArchivedDeck[]>("archived_decks"),
  /** until 'YYYY-MM-DD' (그날 포함). null 이면 다시 시작할 때까지 */
  setDeckPaused: (deckId: number, paused: boolean, until: string | null = null) => invoke<void>("set_deck_paused", { deckId, paused, until }),
  /** 되살린 덱의 지금 밀린 복습 수를 돌려준다 (E3 토스트) */
  unarchiveDeck: (deckId: number) => invoke<number>("unarchive_deck", { deckId }),
  deckDetail: (deckId: number) => invoke<DeckDetail>("deck_detail", { deckId }),
  addCard: (card: NewCard) => invoke<number>("add_card", { card }),
  /** deckId 가 null 이면 모든 덱 (홈의 "접어둔 카드 보기") */
  /** cardIds: 이 카드들만 (C1-xr "찾지 못한 미디어" [카드 보기]) */
  listCards: (deckId: number | null, query = "", limit?: number, buriedOnly = false, cardIds: number[] | null = null, weakOnly = false) =>
    invoke<CardList>("list_cards", { deckId, query, limit, buriedOnly, cardIds, weakOnly }),
  /** C1-w [약함으로] / [해제] */
  setDeckWeak: (deckId: number, weak: boolean) => invoke<void>("set_deck_weak", { deckId, weak }),
  /** C1-w [아니에요] (14일 제안 쉼) / recovered: "요즘 괜찮아요" 안내를 닫음 */
  dismissWeak: (deckId: number, recovered = false) => invoke<void>("dismiss_weak", { deckId, recovered }),
  /** 여러 장 (C2-b). 빈칸이 아닌 노트의 형제 카드도 같이 바뀐다 */
  setCardsImportance: (cardIds: number[], level: Level | null) => invoke<void>("set_cards_importance", { cardIds, level }),
  setCardImportance: (cardId: number, level: Level | null) => invoke<void>("set_cards_importance", { cardIds: [cardId], level }),
  deleteCards: (cardIds: number[]) => invoke<void>("delete_cards", { cardIds }),
  deleteCard: (cardId: number) => invoke<void>("delete_cards", { cardIds: [cardId] }),
  moveCards: (cardIds: number[], deckId: number) => invoke<void>("move_cards", { cardIds, deckId }),
  moveCard: (cardId: number, deckId: number) => invoke<void>("move_cards", { cardIds: [cardId], deckId }),
  /** 학습 밖 접어두기 (C2-r · C2-b). 3일 뒤 다시 */
  buryCards: (cardIds: number[]) => invoke<void>("bury_cards", { cardIds }),
  unburyCards: (cardIds: number[]) => invoke<void>("unbury_cards", { cardIds }),
  /** E2-h 최근 26주 하루별 평가한 카드 수. 0 인 날은 빠진다 */
  heatmap: (deckId: number | null) => invoke<HeatDay[]>("heatmap", { deckId }),
  saveDeckConfig: (config: DeckConfig) => invoke<void>("save_deck_config", { config }),
  previewDeckConfig: (config: DeckConfig) => invoke<ConfigPreview>("preview_deck_config", { config }),
  setDeckImportance: (deckId: number, level: Level) => invoke<void>("set_deck_importance", { deckId, level }),
  confirmImportance: (deckIds: number[]) => invoke<void>("confirm_importance", { deckIds }),
  queuePreview: (deckIds: number[] | null, scale: number) => invoke<QueueSummary>("queue_preview", { deckIds, scale }),
  startSession: (deckIds: number[] | null, scale: number) =>
    invoke<{ session_id: number; card_ids: number[] }>("start_session", { deckIds, scale }),
  cardView: (cardId: number) => invoke<CardView>("card_view", { cardId }),
  answer: (sessionId: number, cardId: number, rating: 1 | 2 | 3 | 4, elapsedMs: number) =>
    invoke<AnswerResult>("answer", { sessionId, cardId, rating, elapsedMs }),
  buryCard: (sessionId: number, cardId: number) => invoke<void>("bury_card", { sessionId, cardId }),
  /** 이 세션에서 방금 한 평가나 접기를 되돌린다. 되돌린 카드 id, 없으면 null */
  undoLast: (sessionId: number) => invoke<number | null>("undo_last", { sessionId }),
  unburyCard: (sessionId: number, cardId: number) => invoke<void>("unbury_card", { sessionId, cardId }),
  finishSession: (sessionId: number, completed: boolean, deckIds: number[] | null, scale: number) =>
    invoke<SessionSummary>("finish_session", { sessionId, completed, deckIds, scale }),
  resolveBuried: (sessionId: number, today: boolean) => invoke<number[]>("resolve_buried", { sessionId, today }),
  previewJson: (text: string, deckId: number | null) => invoke<JsonPreview>("preview_json", { text, deckId }),
  importJson: (text: string, source: string, deckId: number | null) => invoke<ImportResult>("import_json", { text, source, deckId }),
  inspectAnki: (path: string) => invoke<AnkiPreview>("inspect_anki", { path }),
  /** deckIds: B2-s 에서 고른 덱 (deck_tree 의 id). 없으면 전부 */
  importAnki: (path: string, withHistory = false, deckIds: number[] | null = null) =>
    invoke<ImportResult>("import_anki", { path, withHistory, deckIds }),
  /** B2-p 취소. 진행 중인 가져오기를 멈추고 되돌린다 */
  cancelImport: () => invoke<void>("cancel_import"),
  /** B2-j 끌어놓은 .json 파일을 읽는다 */
  readJsonFile: (path: string) => invoke<string>("read_json_file", { path }),
  /** B2-c 끌어놓은 CSV 의 바이트 (인코딩은 화면이 고른다) */
  readTableFile: (path: string) => invoke<ArrayBuffer>("read_table_file", { path }),
  saveText: (path: string, text: string) => invoke<void>("save_text", { path, text }),
  /** 덱을 하위 덱과 함께 .apkg 로. path 는 저장 대화상자에서. withHistory 가 false 면 모두 새 카드로 */
  exportApkg: (deckId: number, path: string, withHistory: boolean) => invoke<ExportResult>("export_apkg", { deckId, path, withHistory }),
  /** 멈추면 만들던 파일만 지운다. 오류 문구는 "내보내기를 멈췄어요…" */
  cancelExport: () => invoke<void>("cancel_export"),
  exportPreview: (deckId: number) => invoke<ExportPreview>("export_preview", { deckId }),
  /** 탐색기에서 파일을 고른 채로 연다 */
  revealFile: (path: string) => invoke<void>("reveal_file", { path }),
  /** C1-xc. columns: front | back | deck | tags | importance | due. plainText: 서식 빼고 글자만 */
  exportCsv: (deckId: number, path: string, columns: string[], plainText: boolean) =>
    invoke<{ rows: number; path: string }>("export_csv", { deckId, path, columns, plainText }),
  importanceBoard: (deckId: number) => invoke<BoardDeck[]>("importance_board", { deckId }),
  noteEdit: (noteId: number) => invoke<NoteEdit>("note_edit", { noteId }),
  updateNote: (noteId: number, values: string[]) => invoke<void>("update_note", { noteId, values }),
  readMedia: (name: string) => invoke<string>("read_media", { name }),
  appInfo: () => invoke<AppInfo>("app_info"),
  mvpMetrics: () => invoke<MvpMetrics>("mvp_metrics"),
  createBackup: () => invoke<string>("create_backup"),
  retryOpen: () => invoke<void>("retry_open"),
  /** 가장 최근 백업으로 되돌린다. 쓴 백업 파일 이름을 돌려준다 */
  restoreBackup: () => invoke<string>("restore_backup"),
  openBackupDir: () => invoke<void>("open_backup_dir"),
  setCutoffHour: (hour: number) => invoke<void>("set_cutoff_hour", { hour }),
  wipeAll: (backup: boolean) => invoke<string | null>("wipe_all", { backup }),
};

// ---------------------------------------------------------------- G 카드 점검 · 나누기 (quality.rs)

/** lines · items · questions = 카드 모양 (노랑/회색), again = 최근 14일 Again (빨강) */
export type QualityFlag = { kind: "lines" | "items" | "questions" | "again"; n: number };
export type QualityCard = { id: number; question: string; path: string[]; flags: QualityFlag[]; hidden: boolean };
/** 글자 그대로. back 은 줄바꿈으로 항목을 나눈다 (만들 때 HTML 로 바뀐다) */
export type SplitPart = { front: string; back: string };
export type SplitProposal = {
  card_id: number;
  question: string;
  answer: string[];
  why: string;
  parts: SplitPart[];
  per_card: number;
  flags: QualityFlag[];
  /** questions 면 한 장 항목 수를 바꿔도 같은 제안이다. nature = ChatGPT 성격별로 묶기 (aiApi.regroup) */
  kind: "items" | "lines" | "questions" | "nature";
  /** R3: 함께 보낸 자료 구간 · 그중 ChatGPT 가 인용한 구간 (근거 칩). 규칙 제안은 비어 있다 */
  sent: Sent[];
  evidence: Sent[];
};
export type CardChange = { id: number; summary: string; card_id: number | null; created_at: number; undone: boolean };

export const qualityApi = {
  check: (deckId: number) => invoke<QualityCard[]>("quality_check", { deckId }),
  /** [그대로 두기] 30일 동안 목록에서 뺀다. hide=false 면 다시 올린다 */
  hide: (cardId: number, hide: boolean) => invoke<void>("hide_quality", { cardId, hide }),
  suggest: (cardId: number, perCard: number | null = null) => invoke<SplitProposal>("split_suggest", { cardId, perCard }),
  /** 승인. 변경 id 를 돌려준다 (되돌리기용) */
  apply: (cardId: number, parts: SplitPart[], archiveOriginal: boolean) => invoke<number>("split_apply", { cardId, parts, archiveOriginal }),
  undo: (changeId: number) => invoke<void>("undo_change", { changeId }),
  recent: (deckId: number) => invoke<CardChange[]>("recent_changes", { deckId }),
};

// ---------------------------------------------------------------- G2 성격별로 묶기 · ChatGPT 요금제 (ai.rs)

export type AiStatus = {
  connected: boolean;
  email: string | null;
  model: string | null;
  /** 이번 달 보낸 카드와 마지막으로 보낸 시각. 이 기기에서 센 수 (사용량 추정 아님) */
  sent_month: number;
  sent_at: number | null;
};
/** 로그인 결과 ("ai-auth" 이벤트). first: 이 설치에서 처음 연결 (L3 환영을 한 번만) */
export type AiAuthEvent = {
  ok: boolean;
  reason: "timeout" | "cancelled" | "denied" | "scope" | "not_eligible" | "offline" | "error" | null;
  message: string | null;
  email: string | null;
  first: boolean;
};
export type AiErrorKind = "not_connected" | "expired" | "scope" | "not_eligible" | "unavailable" | "limit" | "offline" | "bad_output" | "cancelled" | "error";
export type AiError = { kind: AiErrorKind; message: string };
export const aiApi = {
  status: () => invoke<AiStatus>("ai_status"),
  /** 브라우저를 열고 로그인 주소를 돌려준다 ([링크 복사]). 결과는 "ai-auth". reconsent: 사용량 권한을 다시 묻는다, newAccount: 다른 계정으로 */
  connect: (reconsent = false, newAccount = false) => invoke<string>("ai_connect", { reconsent, newAccount }),
  cancelConnect: () => invoke<void>("ai_connect_cancel"),
  /** 폐기를 확인하지 못했으면 false (ChatGPT 설정에서 끊을 수 있다) */
  disconnect: () => invoke<boolean>("ai_disconnect"),
  openUsage: () => invoke<void>("ai_open_usage"),
  /** 카드 한 장을 보내 같은 모양의 제안을 받는다. 아무것도 바꾸지 않는다 (승인은 qualityApi.apply). 오류는 AiError */
  regroup: (cardId: number, segmentIds: number[] = []) => invoke<SplitProposal>("ai_regroup", { cardId, segmentIds }),
  cancel: () => invoke<void>("ai_cancel"),
};
/** ai_regroup 오류는 { kind, message }. 다른 오류(글자)도 받는다 */
export const aiError = (e: unknown): AiError =>
  e && typeof e === "object" && "kind" in e ? (e as AiError) : { kind: "error", message: errorText(e) };

// ---------------------------------------------------------------- H 학습 계획 · 목표일 충돌 (plan.rs). 예상 점수 · 합격 가능성은 없다

export type PlanBar = {
  date: string;
  weekend: boolean;
  kind: "past" | "today" | "future" | "exam";
  review: number;
  new: number;
  /** 지난 날 실제로 본 카드 */
  done: number;
  /** 시간을 넣었을 때 이날의 장수 상한 */
  cap: number | null;
};
export type PlanExam = { date: string; deck_id: number; name: string };
/** to_date 가 null 이면 당겨도 줄지 않는다. after: 당긴 뒤 그날 예정 (다시 내다본 값) */
export type PlanOver = { date: string; weekend: boolean; total: number; cap: number; to_date: string | null; to_weekend: boolean; n: number; after: number; deck_id: number };
export type StudyPlan = {
  bars: PlanBar[];
  exams: PlanExam[];
  target_date: string | null;
  days_left: number | null;
  remaining_new: number;
  sec_per_card: number;
  /** 최근 14일 기록이 충분하면 true. 아니면 10초로 잡는다 */
  measured: boolean;
  weekday_min: number | null;
  weekend_min: number | null;
  weekday_cap: number | null;
  weekend_cap: number | null;
  over: PlanOver | null;
  pace: string | null;
  /** 다시 나누기 (덱 계획 · 목표일이 있을 때만) */
  recompute: boolean | null;
  has_conflict: boolean;
};
export type PlanMode = "spread" | "first" | "keep";
export type ConflictDeck = { id: number; name: string; target: string };
export type PlanConflict = {
  a: ConflictDeck;
  b: ConflictDeck;
  peak_date: string;
  peak: number;
  limit: number;
  /** 기준선이 쓸 수 있는 시간이면 true, 아니면 최근 30일 평균의 1.5배 */
  by_time: boolean;
  bars: { date: string; a: number; b: number; other: number; limit: number }[];
  spread_peak: number;
  spread_extra: number;
  first_peak: number;
  first_days: number;
  over_dates: string[];
  chosen: PlanMode | null;
};

export const planApi = {
  plan: (deckId: number | null) => invoke<StudyPlan>("study_plan", { deckId }),
  /** 분. null 이면 지운다 */
  setTime: (weekdayMin: number | null, weekendMin: number | null) => invoke<void>("set_study_time", { weekdayMin, weekendMin }),
  pull: (deckId: number, fromDate: string, toDate: string, n: number) => invoke<void>("pull_plan", { deckId, fromDate, toDate, n }),
  setRecompute: (deckId: number, on: boolean) => invoke<void>("set_recompute", { deckId, on }),
  /** includeResolved=false: 아직 고르지 않은 충돌만 (홈 배너) */
  conflict: (includeResolved = false) => invoke<PlanConflict | null>("plan_conflict", { includeResolved }),
  resolve: (deckA: number, deckB: number, mode: PlanMode) => invoke<void>("resolve_plan_conflict", { deckA, deckB, mode }),
  /** C4 "충돌 조정으로 +18장" */
  adjust: (deckId: number) => invoke<{ mode: PlanMode; delta: number } | null>("plan_adjust", { deckId }),
};

// ================================================================ K · Google Drive 보관·동기화 (src-tauri/src/sync) — 시작
// 네트워크는 모두 Rust 가 쓴다 (웹뷰 CSP connect-src 는 연결 뒤에도 그대로). 상태는 "sync-status" 이벤트로도 온다

export type SyncState = "off" | "paused" | "idle" | "syncing" | "offline" | "login" | "quota" | "update" | "choice" | "gone" | "locked" | "error";
export type SyncDevice = {
  uid: string;
  name: string;
  at: number | null;
  me: boolean;
  /** 90일 동안 맞추지 않음 ("오래 안 씀") */
  stale: boolean;
  reviews: number | null;
  reviews_at: number | null;
};
export type SyncStatus = {
  /** Google 클라이언트가 들어 있는 빌드 (또는 개발용 폴더). false 면 K1-n */
  configured: boolean;
  folder: boolean;
  connected: boolean;
  /** K3 [나중에] 면 false */
  enabled: boolean;
  account: string | null;
  state: SyncState;
  phase: "pull" | "push" | "media" | "snapshot" | "backup" | "restore" | null;
  done: number;
  total: number;
  last_sync: number | null;
  /** 아직 올리지 않은 변경 묶음 (복습 기록은 세션 단위) */
  pending: number;
  pending_reviews: number;
  pending_notes: number;
  usage: number | null;
  quota: [number, number] | null;
  devices: SyncDevice[];
  device_name: string;
  backup: "daily" | "weekly" | "off";
  lock: boolean;
  /** 다른 기기가 되돌리기 · 새로 시작을 했다 (K6-m). header.reason.kind: restore | fresh | lock | unlock */
  choice: { gen: number; header: SyncHeader; locked: boolean } | null;
  conflicts: number;
  conflict_seq: number;
  error: string | null;
  studying: boolean;
  /** 받기로 이 기기 데이터가 바뀔 때마다 커진다 */
  changed_seq: number;
  closing: boolean;
};
export type SyncHeader = {
  gen?: number;
  device?: string;
  device_name?: string | null;
  at?: number;
  reason?: { kind?: "restore" | "fresh" | "lock" | "unlock" | "new" | "compact"; backup_at?: number | null; device_name?: string | null };
  counts?: SyncCounts;
};
export type SyncCounts = { decks: number; cards: number; reviews: number; media_bytes: number };
export type SyncProbe = {
  account: string;
  /** Drive 에 Neo-Anki 데이터가 있으면 (K6-f · K6-m) */
  remote: { gen: number; locked: boolean; devices: number; bytes: number; device: string | null; at: number | null; counts: SyncCounts | null } | null;
  local: SyncCounts & { empty: boolean };
  device_name: string;
};
/** "sync-auth" 이벤트 (K2 결과) */
export type SyncAuthEvent = { ok: boolean; reason: "timeout" | "cancelled" | "denied" | "scopes" | "error" | null; message: string | null; probe: SyncProbe | null };
export type SyncJoinMode = "new" | "take" | "merge" | "fresh";
export type SyncJoinOpts = { mode: SyncJoinMode; device_name?: string; backup?: "daily" | "weekly" | "off"; password?: string; later?: boolean };
export type DriveBackup = { name: string; at: number; size: number; device: string | null; cards: number | null; reviews: number | null };
/** K9 머리: 무엇이 사라지는지 */
export type RestorePreview = {
  name: string;
  at: number;
  device: string | null;
  cards: number;
  reviews: number;
  /** 이 백업 뒤의 복습 기록 · 카드 변경 (되돌리면 빠진다. 방금 남긴 이 기기 백업에는 있다) */
  lost_reviews: number;
  lost_changes: number;
  /** 다음 동기화에서 고르는 창을 보게 될 다른 기기 */
  others: string[];
};
export type SyncVersion = { device: string; at: number; fields: string[]; current: boolean };
export type SyncConflict = { id: number; question: string; deck_path: string[]; versions: [SyncVersion, SyncVersion] };

export const syncApi = {
  status: () => invoke<SyncStatus>("sync_status"),
  /** 학습 화면에 들어가면 true. 나오면(학습을 마칠 때) 받기 -> 올리기 */
  pause: (paused: boolean) => invoke<void>("sync_pause", { paused }),
  now: () => invoke<void>("sync_now"),
  /** K8 [기다리지 않고 닫기] */
  closeNow: () => invoke<void>("sync_close_now"),
  /** K1 [Google Drive 연결]. 브라우저를 열고 로그인 주소를 돌려준다 ([링크 복사]). 개발용 폴더면 null. 결과는 "sync-auth" */
  connect: () => invoke<string | null>("sync_connect"),
  cancelConnect: () => invoke<void>("sync_connect_cancel"),
  probe: () => invoke<SyncProbe>("sync_probe"),
  join: (opts: SyncJoinOpts) => invoke<void>("sync_join", { opts }),
  disconnect: (deleteRemote: boolean) => invoke<void>("sync_disconnect", { deleteRemote }),
  settings: (opts: { device_name?: string; backup?: "daily" | "weekly" | "off" }) => invoke<void>("sync_settings", { opts }),
  /** 암호로 잠그기. null 이면 푼다. 다른 기기는 K6-m 을 거친다 */
  setLock: (password: string | null) => invoke<void>("sync_set_lock", { password }),
  unlock: (password: string) => invoke<void>("sync_unlock", { password }),
  backups: () => invoke<DriveBackup[]>("sync_backups"),
  restorePreview: (name: string) => invoke<RestorePreview>("sync_restore_preview", { name }),
  restore: (name: string) => invoke<void>("sync_restore", { name }),
  /** [그림까지 묶어 내려받기]. path 는 저장 대화상자에서 */
  backupFull: (name: string, path: string) => invoke<string>("sync_backup_full", { name, path }),
  deleteBackup: (name: string) => invoke<void>("sync_delete_backup", { name }),
  pruneBackups: () => invoke<number>("sync_prune_backups"),
  openFolder: () => invoke<void>("sync_open_folder"),
  conflicts: () => invoke<SyncConflict[]>("sync_conflicts"),
  useConflict: (id: number) => invoke<void>("sync_conflict_use", { id }),
  conflictsSeen: () => invoke<void>("sync_conflicts_seen"),
};
// ================================================================ K · Google Drive 보관·동기화 — 끝

/** Rust 쪽 오류는 사용자에게 보여줄 문장으로 온다 */
export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "알 수 없는 오류가 났어요.";
}
