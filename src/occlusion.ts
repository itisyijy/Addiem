// M 그림 가리기. 칸은 Anki Image Occlusion 형식 그대로 백엔드(occlusion.rs)가 읽고 쓴다. 좌표는 그림 대비 비율 0–1
import { invoke } from "@tauri-apps/api/core";
import { baseName, savedImage } from "./cardHtml";

export type Shape = {
  /** cN. 카드 번호이자 칸의 안정된 id. 같은 번호 = 묶음. 0 은 Anki 글자 칸 (카드 없음) */
  ordinal: number;
  /** rect | ellipse | polygon | text */
  kind: string;
  left: number;
  top: number;
  width: number;
  height: number;
  points: [number, number][];
  label: string;
  /** 고치지 않는 Anki 속성 (angle · fill · text …). 그대로 돌려준다 */
  extra: [string, string][];
};
export type OcclusionNote = {
  note_id: number;
  deck_id: number;
  image: string;
  header: string;
  back_extra: string;
  shapes: Shape[];
  hide_all: boolean;
  cards: { ordinal: number; card_id: number; reviewed: boolean }[];
};
export type OcclusionInput = {
  note_id: number | null;
  deck_id: number;
  image: string;
  header: string;
  back_extra: string;
  shapes: Shape[];
  hide_all: boolean;
  importance: number | null;
};

export const occlusionApi = {
  note: (noteId: number) => invoke<OcclusionNote>("occlusion_note", { noteId }),
  save: (note: OcclusionInput) => invoke<number>("save_occlusion", { note }),
};

/** 카드가 되는 번호 (작은 것부터) */
export const ordinalsOf = (shapes: Shape[]) => [...new Set(shapes.filter((s) => s.ordinal > 0).map((s) => s.ordinal))].sort((a, b) => a - b);
/** 묶음의 이름 (첫 칸에 둔다) */
export const labelOf = (shapes: Shape[], n: number) => shapes.find((s) => s.ordinal === n && s.label.trim())?.label.trim() ?? "";
/** 카드가 되는 칸 수 (글자 칸 빼고) */
export const boxCount = (shapes: Shape[]) => shapes.filter((s) => s.ordinal > 0 && s.kind !== "text").length;

export type Picture = { name: string; url: string; width: number; height: number; fileName: string };

export function naturalSize(url: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    img.onerror = () => resolve({ width: 0, height: 0 });
    img.src = url;
  });
}

/** M7: GIF · SVG · HEIC 는 막는다 (정지 GIF 도 막으므로 "움직이는" 이라 하지 않는다, QA T1-26) */
const REFUSED = "GIF · SVG · HEIC 그림은 쓸 수 없어요. PNG · JPG · WEBP · BMP로 바꿔 넣어 주세요.";
const refused = (name: string) => /\.(gif|svg|heic|heif)$/i.test(name);

/** 원본 그대로 미디어 폴더에 복사한다 (save_media) */
export async function storeImage(file: File): Promise<Picture> {
  if (refused(file.name) || /gif|svg|hei[cf]/i.test(file.type) || !file.type.startsWith("image/")) throw new Error(REFUSED);
  const url = await new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
  const name = await invoke<string>("save_media", { name: file.name || "image.png", dataBase64: url.split(",")[1] ?? "" });
  return { name, url, fileName: file.name || name, ...(await naturalSize(url)) };
}

/** 머리에 보일 그림 이름. 앱이 저장한 내부 이름(neo-해시)은 보이지 않는다 (QA T1-36). 원래 이름은 남기지 않으므로 빈 글자 */
export const shownName = (media: string) => (/^neo-[0-9a-f]{16}\.\w+$/.test(media) ? "" : media);

/** 탐색기에서 끌어 놓은 그림 (경로, save_media_path) */
export async function storeImagePath(path: string): Promise<Picture> {
  const fileName = baseName(path);
  if (refused(fileName)) throw new Error(REFUSED);
  const { name, url } = await savedImage(path);
  return { name, url, fileName, ...(await naturalSize(url)) };
}
