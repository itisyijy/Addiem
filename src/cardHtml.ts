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
    ALLOWED_ATTR: ["src", "alt", "class", "style", "color", "colspan", "rowspan", "data-media", "data-c", "data-hint", "data-snd"],
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

// ---------------------------------------------------------------- AU 소리 칩 (Figma SoundChip)

const lengths = new Map<string, Promise<number | null>>();
/** 소리 길이(초). 파일이 없거나 재생할 수 없는 형식이면 null (AU5 점선 칩) */
function soundLength(name: string) {
  let p = lengths.get(name);
  if (!p) {
    p = mediaUrl(name).then((url) =>
      !url
        ? null
        : new Promise<number | null>((done) => {
            const a = new Audio();
            a.preload = "metadata";
            a.onloadedmetadata = () => done(Number.isFinite(a.duration) ? a.duration : 0);
            a.onerror = () => done(null);
            a.src = url;
          }),
    );
    lengths.set(name, p);
  }
  return p;
}

const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

/** Rust 가 [sound:파일] 자리에 둔 <span class="snd" data-snd> (render.rs) -> 칩. 글자는 CSS 가 data-label 로 그려 카드 글자(plainText)에 섞이지 않는다 */
async function soundChip(mark: Element) {
  const name = mark.getAttribute("data-snd") ?? "";
  const len = await soundLength(name);
  const chip = Object.assign(document.createElement("button"), { type: "button", className: "snd t-label-s" });
  chip.dataset.snd = name;
  if (len === null) {
    chip.disabled = true;
    chip.classList.add("is-missing");
    chip.dataset.label = chip.title = `소리 파일을 찾을 수 없어요 · ${name}`;
    chip.setAttribute("aria-label", `소리 파일을 찾을 수 없어요: ${name}`);
  } else {
    chip.dataset.len = chip.dataset.label = clock(Math.max(1, Math.round(len)));
    chip.setAttribute("aria-label", `소리 듣기 · ${chip.dataset.len}`);
  }
  mark.replaceWith(chip);
}

let run = 0;
let current: { audio: HTMLAudioElement; chip: HTMLElement } | null = null;

function settle(c: { audio: HTMLAudioElement; chip: HTMLElement }) {
  c.audio.ontimeupdate = null;
  c.chip.classList.remove("is-playing");
  c.chip.dataset.label = c.chip.dataset.len ?? "";
}

/** 칩의 소리를 차례로 재생한다. 하던 재생은 멈춘다. 없는 파일 · 재생 실패(자동 재생 막힘 등)는 조용히 건너뛴다 */
export async function playSounds(chips: Iterable<Element>) {
  stopSounds();
  const me = run;
  for (const chip of chips) {
    if (!(chip instanceof HTMLButtonElement) || chip.disabled) continue;
    const url = await mediaUrl(chip.dataset.snd ?? "");
    if (me !== run) return;
    if (!url) continue;
    const audio = new Audio(url);
    const c = (current = { audio, chip });
    const len = chip.dataset.len ?? "";
    chip.classList.add("is-playing");
    chip.dataset.label = `0:00 / ${len}`;
    audio.ontimeupdate = () => (chip.dataset.label = `${clock(audio.currentTime)} / ${len}`);
    await new Promise<void>((done) => {
      audio.onended = audio.onerror = audio.onpause = () => done();
      audio.play().catch(() => done());
    });
    if (me !== run) return; // stopSounds 가 이미 정리했다
    settle(c);
    current = null;
  }
}

export function stopSounds() {
  run++;
  if (!current) return;
  current.audio.pause();
  settle(current);
  current = null;
}

// 카드 HTML 은 문자열로 그려서(dangerouslySetInnerHTML) 칩마다 손잡이를 달 수 없다. 문서 한 곳에서 받는다.
// 재생 중인 칩을 누르면 멈추고, 다시 누르면 처음부터 (SoundChip 설명)
document.addEventListener("click", (e) => {
  const chip = e.target instanceof Element ? e.target.closest("button.snd") : null;
  if (!chip) return;
  if (chip.classList.contains("is-playing")) stopSounds();
  else playSounds([chip]);
});

/** 살균한 뒤 <img src="파일이름"> 을 실제 그림으로, 소리 자리 표시를 칩으로 바꾼다 */
export async function renderCardHtml(html: string): Promise<string> {
  const tpl = document.createElement("template");
  tpl.innerHTML = sanitize(html);
  await Promise.all(Array.from(tpl.content.querySelectorAll("span.snd[data-snd]"), soundChip));
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
