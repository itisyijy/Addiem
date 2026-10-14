// I 원자료 명령 (src-tauri/src/source.rs · embed.rs). 필드 이름은 Rust 와 같다
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, useSyncExternalStore } from "react";

// ---------------------------------------------------------------- R 자료 근거 AI (src-tauri/src/rag.rs)

/** 보낼 구간 하나. text 는 보내는 글자 그대로 (600자에서 자르면 끝에 "…(잘림)"). 자료 이름 · 쪽 · 신호는 화면에만 */
export type Sent = {
  id: number;
  material_id: number;
  material: string;
  kind: "pdf" | "pptx";
  page: number;
  signals: Signal[];
  text: string;
  /** manual 직접 연결 · auto 자동 연결 · similar 비슷한 구간 · picked 직접 고름 */
  how: "manual" | "auto" | "similar" | "picked";
};
export type DraftCard = {
  front: string;
  back: string;
  segment_id: number;
  /** 제안: 2 핵심 · 1 일반 */
  importance: 0 | 1 | 2;
  similar: { card_id: number; question: string; score: number } | null;
  /** 근거 구간과 유사도가 낮은 문장 */
  ungrounded: string[];
};
export type Draft = { cards: DraftCard[]; sent: Sent[]; missing: number[]; deck_id: number; root_deck_id: number; checked: boolean };
export type DraftAmount = "few" | "normal" | "many";
/** R2 보내는 범위. 글자는 보내는 그대로 (질문 600자 · 답 1500자에서 자른다) */
export type GradeScope = { question: string; mine: string; model: string; segments: Sent[] };
/** R2 결과. rating: 1 Again · 2 Hard · 3 Good. missing.segments 는 구간 id (기기 안에서 되돌린 것) */
export type Grade = { right: string[]; missing: { text: string; segments: number[] }[]; rating: 1 | 2 | 3; sent: Sent[] };
export const MAX_PICK = 10;

export const ragApi = {
  preview: (segmentIds: number[]) => invoke<Sent[]>("rag_preview", { segmentIds }),
  /** R1-b. 아무것도 바꾸지 않는다. 오류는 AiError */
  draft: (segmentIds: number[], amount: DraftAmount, cloze: boolean) => invoke<Draft>("rag_draft", { segmentIds, amount, cloze }),
  /** R1-c 승인. 되돌리기는 qualityApi.undo(변경 id) */
  apply: (deckId: number, cloze: boolean, cards: { front: string; back: string; segment_id: number; importance: number }[]) =>
    invoke<number>("rag_draft_apply", { deckId, cloze, cards }),
  /** R2 버튼 아래 한 줄 · 첫 확인 목록. 아무것도 보내지 않는다 */
  gradeScope: (cardId: number, answer: string) => invoke<GradeScope>("rag_grade_scope", { cardId, answer }),
  /** R2 채점 보조. 오류는 AiError */
  grade: (cardId: number, answer: string, segmentIds: number[]) => invoke<Grade>("rag_grade", { cardId, answer, segmentIds }),
};

/** "3장_빅데이터분석.pdf · p.13" */
export const placeOf = (s: { material: string; page: number }) => `${s.material} · p.${s.page}`;

/** 신뢰도 순 (source.rs SIGNALS) */
export type Signal = "highlight" | "underline" | "red" | "star" | "bold";
export const SIGNAL_LABEL: Record<Signal, string> = { highlight: "하이라이트", underline: "밑줄", red: "붉은 글씨", star: "★", bold: "볼드" };
/** 밑줄은 하이라이트와 구분되게 회색 + 글자 밑줄 (SignalBadge) */
export const SIGNAL_TONE: Record<Signal, string> = { highlight: "warn", underline: "neutral", red: "danger", star: "accent", bold: "neutral" };
export const SIGNAL_ORDER: Signal[] = ["highlight", "underline", "red", "star", "bold"];

export type Material = {
  id: number;
  deck_id: number;
  file_name: string;
  kind: "pdf" | "pptx";
  pages: number;
  /** pending: PDF 를 아직 읽지 않음 · unsupported: 글자를 읽을 수 없음 (스캔본) */
  status: "pending" | "ready" | "unsupported";
  segments: number;
  /** 카드가 이어진 구간 수 */
  linked: number;
  /** R1-a 빠진 범위: 강조됐는데 이어진 카드가 없는 구간 수 */
  gaps: number;
};
/** 쪽 위 위치 [x, y, w, h], 0~1 비율 (왼쪽 위 기준) */
export type Rect = [number, number, number, number];
/** origin "draft": R1 초안에서 만듦 (method 는 manual) */
export type Link = { card_id: number; method: "manual" | "auto"; origin: "draft" | null };
export type Segment = {
  id: number;
  page: number;
  ord: number;
  text: string;
  signals: Signal[];
  weight: number;
  rects: Rect[] | null;
  before: string | null;
  after: string | null;
  links: Link[];
};
export type NewSegment = Omit<Segment, "id" | "weight" | "links">;
export type Para = { text: string; signals: Signal[] };
/** method "": 근거 칩으로 구간을 열었다 (카드 연결이 아님) */
export type CardSource = { material: Material; segment: Segment; method: "manual" | "auto" | "" };
/** "source-analyze" 이벤트. step 0 은 카드, 그다음 자료 하나씩 */
export type AnalyzeProgress = { step: number; steps: number; label: string; done: number; total: number };
export type ModelStatus = { ready: boolean; downloading: boolean; bytes: number; name: string; analyzing: AnalyzeProgress | null };

export const sourceApi = {
  list: (deckId: number) => invoke<Material[]>("source_list", { deckId }),
  add: (deckId: number, path: string) => invoke<Material>("source_add", { deckId, path }),
  remove: (materialId: number) => invoke<void>("source_delete", { materialId }),
  bytes: (materialId: number) => invoke<ArrayBuffer>("source_bytes", { materialId }),
  savePdf: (materialId: number, pages: number, segments: NewSegment[], readable: boolean) =>
    invoke<Material>("source_save_pdf", { materialId, pages, segments, readable }),
  segments: (materialId: number) => invoke<Segment[]>("source_segments", { materialId }),
  slide: (materialId: number, page: number) => invoke<Para[]>("source_slide", { materialId, page }),
  link: (segmentId: number, cardId: number) => invoke<void>("source_link", { segmentId, cardId }),
  unlink: (segmentId: number, cardId: number) => invoke<void>("source_unlink", { segmentId, cardId }),
  forCard: (cardId: number) => invoke<CardSource | null>("source_for_card", { cardId }),
  /** R 근거 칩: 구간 하나를 I2 패널로 */
  segment: (segmentId: number) => invoke<CardSource>("source_segment", { segmentId }),
  analyze: (deckId: number) => invoke<{ linked: number; suggested: number }>("source_analyze", { deckId }),
  /** 이 기기에서 분석한 뒤에 생긴 카드 수 (분석한 적이 없으면 0) */
  unanalyzed: (deckId: number) => invoke<number>("source_unanalyzed", { deckId }),
  modelStatus: () => invoke<ModelStatus>("model_status"),
  downloadModel: () => invoke<void>("model_download"),
  cancelDownload: () => invoke<void>("model_cancel"),
  deleteModel: () => invoke<void>("model_delete"),
};

/** 신호 배지는 신뢰도 높은 것이 먼저 (I1 노트) */
export const topSignal = (s: { signals: Signal[] }) => SIGNAL_ORDER.find((x) => s.signals.includes(x)) ?? "bold";

/** "약 490MB" (배너 크기 문구는 실제로 받는 크기에서) */
export const mb = (bytes: number) => `${Math.round(bytes / 1e6 / 10) * 10}MB`;

// 모델이 있는지 여러 화면이 같이 본다 (중요도 칩의 출처 태그 · 배너). 받거나 지우면 refreshModel()
let model: ModelStatus | null = null;
const subs = new Set<() => void>();
export function refreshModel() {
  return sourceApi.modelStatus().then((s) => {
    model = s;
    subs.forEach((f) => f());
    return s;
  });
}
export function useModel(): ModelStatus | null {
  const s = useSyncExternalStore(
    (f) => (subs.add(f), () => subs.delete(f)),
    () => model,
  );
  useEffect(() => {
    if (!model) refreshModel().catch(() => {});
  }, []);
  return s;
}

// 카드별 원자료. 학습 화면이 [원자료] 버튼을 보일지 정할 때와 패널이 같은 값을 쓴다 (한 번만 묻는다)
const byCard = new Map<number, Promise<CardSource | null>>();
export const cardSource = (cardId: number, fresh = false) => {
  if (fresh || !byCard.has(cardId)) byCard.set(cardId, sourceApi.forCard(cardId).catch(() => null));
  return byCard.get(cardId)!;
};
/** undefined: 묻는 중 · null: 연결 없음 (버튼을 숨긴다) */
export function useCardSource(cardId: number | null | undefined): CardSource | null | undefined {
  const [s, set] = useState<{ id: number; v: CardSource | null }>();
  useEffect(() => {
    if (cardId == null) return;
    let live = true;
    cardSource(cardId).then((v) => live && set({ id: cardId, v }));
    return () => {
      live = false;
    };
  }, [cardId]);
  return cardId == null ? null : s?.id === cardId ? s.v : undefined;
}
