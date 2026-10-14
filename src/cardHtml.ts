// 카드 HTML. 가져온 덱의 HTML 은 신뢰하지 않는다 (ADR 0001 2장: 외부 HTML 은 신뢰 경계)
import DOMPurify from "dompurify";
import { invoke } from "@tauri-apps/api/core";

const ALLOWED_TAGS = [
  "b", "strong", "i", "em", "u", "s", "br", "p", "div", "span", "ul", "ol", "li", "img", "hr",
  "sub", "sup", "table", "thead", "tbody", "tr", "td", "th", "font", "code", "pre", "blockquote",
  "h1", "h2", "h3", "h4", "h5", "h6",
];

/**
 * 스크립트, 이벤트 속성, 외부 주소를 지운다.
 * 이미지는 앱 안에 저장된 파일 이름(neo-xxx.png, 가져온 덱의 원래 이름)과 data:image 만 허용한다.
 * 인라인 색은 살린다. 인터뷰에서 "붉은 글씨" 가 중요한 강조 신호였다.
 */
export function sanitize(html: string): string {
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR: ["src", "alt", "class", "style", "color", "colspan", "rowspan", "data-media", "data-c", "data-hint"],
    ALLOWED_URI_REGEXP: /^(?:data:image\/[a-z+.-]+;base64,|[^:/?#\\]+$)/i,
  });
}

const cache = new Map<string, Promise<string>>();

function mediaUrl(name: string) {
  let p = cache.get(name);
  if (!p) {
    p = invoke<string>("read_media", { name }).catch(() => "");
    cache.set(name, p);
  }
  return p;
}

/** 살균한 뒤 <img src="파일이름"> 을 실제 그림으로 바꾼다 */
export async function renderCardHtml(html: string): Promise<string> {
  const tpl = document.createElement("template");
  tpl.innerHTML = sanitize(html);
  const imgs = Array.from(tpl.content.querySelectorAll("img"));
  await Promise.all(
    imgs.map(async (img) => {
      const src = img.getAttribute("src") ?? "";
      if (!src || src.startsWith("data:")) return;
      const url = await mediaUrl(src);
      if (url) {
        img.setAttribute("data-media", src);
        img.setAttribute("src", url);
      } else {
        img.replaceWith(Object.assign(document.createElement("span"), { className: "missing-media", textContent: `[그림 없음: ${src}]` }));
      }
    }),
  );
  return tpl.innerHTML;
}

/** 편집기 HTML -> 저장할 HTML. 화면용 data URL 을 파일 이름으로 되돌린다 */
export function toStoredHtml(editorHtml: string): string {
  const tpl = document.createElement("template");
  tpl.innerHTML = editorHtml;
  tpl.content.querySelectorAll("img[data-media]").forEach((img) => {
    img.setAttribute("src", img.getAttribute("data-media") ?? "");
    img.removeAttribute("data-media");
  });
  return sanitize(tpl.innerHTML).trim();
}

export function plainText(html: string): string {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  return (tpl.content.textContent ?? "").replace(/\s+/g, " ").trim();
}

/** 이미지 파일을 저장하고 편집기에 넣을 <img> 를 만든다 */
export async function imageTag(file: File): Promise<string> {
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
  const name = await invoke<string>("save_media", { name: file.name || "image.png", dataBase64: dataUrl.split(",")[1] ?? "" });
  return `<img src="${dataUrl}" data-media="${name}" alt="">`;
}
