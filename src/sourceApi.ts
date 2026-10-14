// I 원자료 명령 (src-tauri/src/source.rs · embed.rs). 필드 이름은 Rust 와 같다
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, useSyncExternalStore } from "react";

/** 신뢰도 순 (source.rs SIGNALS) */
export type Signal = "highlight" | "underline" | "red" | "star" | "bold";
export const SIGNAL_LABEL: Record<Signal, string> = { highlight: "하이라이트", underline: "밑줄", red: "붉은 글씨", star: "★", bold: "볼드" };
export const SIGNAL_TONE: Record<Signal, string> = { highlight: "warn", underline: "warn", red: "danger", star: "accent", bold: "neutral" };
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
};
/** 쪽 위 위치 [x, y, w, h], 0~1 비율 (왼쪽 위 기준) */
export type Rect = [number, number, number, number];
export type Link = { card_id: number; method: "manual" | "auto" };
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
export type CardSource = { material: Material; segment: Segment; method: "manual" | "auto" };
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
