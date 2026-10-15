// Q 목록 회상 · 선택형 (src-tauri/src/recall.rs). 필드 이름은 Rust 와 같다
import { invoke } from "@tauri-apps/api/core";
import type { Level } from "./api";

/** 학습 화면이 템플릿 대신 그리는 카드 (card_view.special). 보기 · 정답은 보기 필드 번호(1–6) */
export type ListSpecial = { kind: "list"; question: string; items: string[]; ordered: boolean; extra: string };
export type ChoiceSpecial = { kind: "choice"; question: string; options: [number, string][]; correct: number[]; shuffle: boolean; extra: string };
export type Special = ListSpecial | ChoiceSpecial;

/** C3 편집 중인 값 */
export type ListDraft = { items: string[]; ordered: boolean };
export type ChoiceDraft = { options: string[]; correct: number[]; shuffle: boolean };
export const EMPTY_LIST: ListDraft = { items: [""], ordered: true };
export const EMPTY_CHOICE: ChoiceDraft = { options: ["", ""], correct: [], shuffle: true };

type Edit = { question: string; extra: string };
type Common = Edit & { deck_id: number; importance: Level | null };

export const recall = {
  addList: (card: Common & ListDraft) => invoke<number>("add_list_card", { card }),
  /** correct: options 의 0부터 번호 */
  addChoice: (card: Common & ChoiceDraft) => invoke<number>("add_choice_card", { card }),
  /** marks: 목록은 빠뜨린 항목 번호, 선택형은 고른 보기 번호 (1부터). 1 Again · 2 Hard · 3 Good */
  suggest: (cardId: number, marks: number[]) => invoke<1 | 2 | 3 | null>("recall_suggest", { cardId, marks }),
  saveMarks: (cardId: number, marks: number[]) => invoke<void>("save_recall_marks", { cardId, marks }),
  /** Q24 전용 편집기. 추가와 같은 검사 · 같은 값으로 저장한다 */
  updateList: (noteId: number, card: Edit & ListDraft) => invoke<void>("update_list_note", { noteId, ...card }),
  updateChoice: (noteId: number, card: Edit & ChoiceDraft) => invoke<void>("update_choice_note", { noteId, ...card }),
};

/** Q24 편집기를 채울 값. 보기 필드 번호(빈 보기는 빠져 있다)를 편집기의 0부터 번호로 */
export function choiceDraft(s: ChoiceSpecial): ChoiceDraft {
  const options = s.options.map((o) => o[1]);
  return { options: [...options, "", ""].slice(0, Math.max(2, options.length)), correct: s.options.flatMap(([num], i) => (s.correct.includes(num) ? [i] : [])), shuffle: s.shuffle };
}

/** 보기 순서 (필드 번호). 섞기가 켜져 있으면 볼 때마다 섞는다 (Q5) */
export function optionOrder(s: ChoiceSpecial): number[] {
  const order = s.options.map((o) => o[0]);
  if (s.shuffle)
    for (let i = order.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [order[i], order[j]] = [order[j], order[i]];
    }
  return order;
}

/** 답 면 번호 (평가 키 1–4 와 헷갈리지 않게, Q6) */
export const CIRCLED = "①②③④⑤⑥";

export const listLabel = (count: number, ordered: boolean) => `목록 · ${count}개 · ${ordered ? "순서대로" : "순서 없이"}`;
export const choiceLabel = (count: number, multi: boolean) => `선택형 · ${count}지 · ${multi ? "모두 고르기" : "하나 고르기"}`;
