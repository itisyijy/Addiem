// 8.1 노트 유형 명령 (src-tauri/src/notetype.rs). 필드 이름은 Rust 와 같다
import { invoke } from "@tauri-apps/api/core";

export type NativeKind = "basic" | "reversed" | "cloze" | "written" | "list" | "choice" | "io";
/** 앱 기본 유형의 화면 이름 (DB 이름은 Anki 이름 "Basic" 등) */
export const NATIVE_LABEL: Record<NativeKind, string> = {
  basic: "질문과 답변",
  reversed: "양방향",
  cloze: "빈칸",
  written: "서술형",
  io: "그림 가리기",
  list: "목록",
  choice: "선택형",
};

export type TypeRow = {
  id: number;
  name: string;
  native_kind: NativeKind | null;
  editable: boolean;
  cloneable: boolean;
  imported: boolean;
  user_made: boolean;
  cloze: boolean;
  fields: number;
  templates: number;
  notes: number;
  cards: number;
  decks: string[];
};

/** 화면에 쓰는 이름. 앱 기본 유형은 우리말 */
export const typeLabel = (t: { name: string; native_kind: NativeKind | null }) => (t.native_kind ? NATIVE_LABEL[t.native_kind] : t.name);

/** 쉬운 편집 줄 모양. 없으면 면의 기본 (앞면 L · 가운데, 뒷면 M · 왼쪽) */
export type Look = { size: "s" | "m" | "l"; align: "left" | "center" };
export const BASE_LOOK: Record<"front" | "back", Look> = { front: { size: "l", align: "center" }, back: { size: "m", align: "left" } };
export type Part =
  | { kind: "field"; name: string; look?: Look }
  | { kind: "cloze"; name: string; look?: Look }
  | { kind: "type"; name: string }
  | { kind: "tts"; opts: string; name: string }
  | { kind: "front_side" };
export type Layout = { front: Part[]; back: Part[] };

export type TemplateView = { ord: number; name: string; qfmt: string; afmt: string; layout: Layout | null; reason: string | null };
export type Detail = TypeRow & {
  css: string;
  fields: { slot: number; name: string }[];
  templates: TemplateView[];
  /** 줄 상자 CSS. 템플릿이 쓰는데 CSS 에 없으면 저장할 때 붙는다 (미리보기도 붙여 그린다) */
  layout_css: string;
};
export type Sample = { note_id: number; deck: string; slots: string[] };

export type TypeEdit = {
  fields: { slot: number | null; name: string }[];
  templates: { ord: number | null; name: string | null; qfmt: string; afmt: string }[];
  css: string;
};
export type TemplateImpact = { cards: number; reviews: number; notes: number };
export type SaveImpact = {
  empty_fronts: number;
  made: number;
  templates: ({ name: string } & TemplateImpact)[];
  fields: { name: string; notes: number }[];
};

export const ntApi = {
  list: () => invoke<TypeRow[]>("note_types"),
  detail: (id: number) => invoke<Detail>("note_type_detail", { id }),
  clone: (id: number, name: string | null) => invoke<number>("clone_note_type", { id, name }),
  rename: (id: number, name: string) => invoke<void>("rename_note_type", { id, name }),
  remove: (id: number) => invoke<void>("delete_note_type", { id }),
  samples: (id: number) => invoke<Sample[]>("note_type_samples", { id }),
  /** values · names 는 편집 중인 필드 순서. values 가 없으면 이름 샘플 */
  preview: (id: number, qfmt: string, afmt: string, ord: number, values: string[] | null, names: string[]) =>
    invoke<{ front: string; back: string }>("preview_template", { id, qfmt, afmt, ord, values, names }),
  /** HTML 전용이면 이유 문자열로 거부한다 */
  layout: (qfmt: string, afmt: string) => invoke<Layout>("template_layout", { qfmt, afmt }),
  build: (layout: Layout) => invoke<{ qfmt: string; afmt: string }>("build_template_layout", { layout }),
  renameRefs: (texts: string[], old: string, name: string) => invoke<string[]>("rename_field_refs", { texts, old, new: name }),
  /** dryRun: 확인 창의 수만 세고 되돌린다 */
  save: (id: number, edit: TypeEdit, dryRun: boolean) => invoke<SaveImpact>("save_note_type", { id, edit, dryRun }),
};
