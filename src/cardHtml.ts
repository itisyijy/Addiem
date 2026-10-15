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
    ALLOWED_ATTR: ["src", "alt", "class", "style", "color", "colspan", "rowspan", "data-media", "data-c", "data-hint", "data-snd", "data-tts", "data-text", "data-speed"],
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
    chip.dataset.idle = chip.dataset.label = clock(Math.max(1, Math.round(len)));
    chip.setAttribute("aria-label", `소리 듣기 · ${chip.dataset.idle}`);
  }
  mark.replaceWith(chip);
}

// ---------------------------------------------------------------- AU20 읽어주기 칩 (SoundChip Kind=읽어주기)

const langPrefix = (lang: string) => lang.split(/[_-]/)[0].toLowerCase();
const names = new Intl.DisplayNames(["ko"], { type: "language" });
/** "en_US" -> "영어" */
function langName(lang: string) {
  try {
    return names.of(langPrefix(lang)) ?? lang;
  } catch {
    return lang; // 언어 코드 꼴이 아니다
  }
}

let engine: Promise<string[]> | null = null;
/** 내장 음성(Supertonic 3, tts.rs)이 읽는 언어. 음성 파일이 없는 빌드면 비어 있다 */
const engineLangs = () => (engine ??= invoke<string[]>("tts_langs").catch(() => []));

let voices: Promise<SpeechSynthesisVoice[]> | null = null;
/** 이 기기의 OS 음성. 글자를 밖으로 보내는 온라인 음성은 뺀다 (AU20). WebView2 는 목록을 늦게 채운다 */
function osVoices() {
  return (voices ??= new Promise((done) => {
    if (!("speechSynthesis" in window)) return done([]);
    const local = () => speechSynthesis.getVoices().filter((v) => v.localService);
    if (speechSynthesis.getVoices().length) return done(local());
    speechSynthesis.addEventListener("voiceschanged", () => done(local()), { once: true });
    setTimeout(() => done(local()), 1500); // 음성이 하나도 없는 기기
  }));
}

/** 그 언어의 OS 음성. 지역까지 맞는 것(en-US)이 먼저 */
async function osVoice(lang: string) {
  const vs = await osVoices();
  const tag = lang.replace("_", "-").toLowerCase();
  return vs.find((v) => v.lang.toLowerCase() === tag) ?? vs.find((v) => langPrefix(v.lang) === langPrefix(lang)) ?? null;
}

/** AU23 가져오기 참고 줄용: 이 기기에서 읽을 수 있는 언어 (내장 음성 + OS 음성, "en") */
export async function voiceLangs() {
  const [built, os] = await Promise.all([engineLangs(), osVoices()]);
  return [...new Set([...built, ...os.map((v) => langPrefix(v.lang))])];
}

let preload: Promise<unknown> | null = null;

/** Rust 가 {{tts}} 자리에 둔 <span class="snd" data-tts data-text> (render.rs) -> 칩. 이 기기에 그 언어 음성이 없으면 점선 칩 (AU23) */
async function ttsChip(mark: Element) {
  const lang = mark.getAttribute("data-tts") ?? "";
  const name = langName(lang);
  const chip = Object.assign(document.createElement("button"), { type: "button", className: "snd t-label-s" });
  chip.dataset.tts = lang;
  chip.dataset.text = mark.getAttribute("data-text") ?? "";
  const speed = mark.getAttribute("data-speed");
  if (speed) chip.dataset.speed = speed;
  const built = (await engineLangs()).includes(langPrefix(lang));
  // 엔진을 미리 올린다 (약 0.8초). 누르기 전에 끝나 있게
  if (built) preload ??= invoke("tts_preload").catch(() => undefined);
  if (!built && !(await osVoice(lang))) {
    chip.disabled = true;
    chip.dataset.label = `이 기기에 ${name} 음성 없음`;
    chip.title = "Windows 설정 › 시간 및 언어 › 음성에서 추가해요";
    chip.setAttribute("aria-label", `이 기기에 ${name} 음성이 없어요`);
  } else {
    chip.dataset.idle = chip.dataset.label = `읽어주기 · ${name}`;
    chip.dataset.busy = `읽는 중 · ${name}`;
    chip.setAttribute("aria-label", chip.dataset.idle);
  }
  mark.replaceWith(chip);
}

const spoken = new Map<string, Promise<string>>();
/** 내장 음성이 읽어 준 소리 (data URL). 이 실행 동안 메모리에만 둔다 (노트 · 미디어 · 내보내기에 쓰지 않는다, OpenRAIL-M). 실패하면 "" */
function ttsUrl(lang: string, text: string, speed: number) {
  const key = `${lang}|${speed}|${text}`;
  let p = spoken.get(key);
  if (!p) {
    p = invoke<string>("tts_speak", { lang, text, speed }).catch(() => "");
    spoken.set(key, p);
    // ponytail: 오래된 것부터 버린다 (64개 ≈ 수십 MB). 모자라면 늘린다
    if (spoken.size > 64) spoken.delete(spoken.keys().next().value!);
  }
  return p;
}

let run = 0;
let current: { chip: HTMLElement; stop: () => void } | null = null;

function settle(chip: HTMLElement) {
  chip.classList.remove("is-playing", "is-loading");
  chip.dataset.label = chip.dataset.idle ?? "";
}

/** 소리 하나를 끝까지 (멈추면 바로) */
function playUrl(url: string, chip: HTMLElement, onTime?: (a: HTMLAudioElement) => void) {
  const audio = new Audio(url);
  current = { chip, stop: () => audio.pause() };
  if (onTime) audio.ontimeupdate = () => onTime(audio);
  return new Promise<void>((done) => {
    audio.onended = audio.onerror = audio.onpause = () => {
      audio.ontimeupdate = null;
      done();
    };
    audio.play().catch(() => done());
  });
}

/** 내장 음성이 없는 언어(중국어 등)나 엔진이 뜨지 않을 때 OS 음성으로 */
function sayOs(voice: SpeechSynthesisVoice, chip: HTMLElement, text: string, rate: number) {
  const u = Object.assign(new SpeechSynthesisUtterance(text), { voice, lang: voice.lang, rate });
  current = { chip, stop: () => speechSynthesis.cancel() };
  return new Promise<void>((done) => {
    u.onend = u.onerror = () => done();
    speechSynthesis.speak(u);
  });
}

/** 칩을 차례로 재생한다 (소리 · 읽어주기를 나온 순서대로). 하던 재생은 멈춘다. 없는 파일 · 재생 실패(자동 재생 막힘 등)는 조용히 건너뛴다 */
export async function playSounds(chips: Iterable<Element>) {
  stopSounds();
  const me = run;
  for (const chip of chips) {
    if (!(chip instanceof HTMLButtonElement) || chip.disabled) continue;
    current = { chip, stop: () => {} };
    chip.classList.add("is-playing");
    const lang = chip.dataset.tts;
    if (lang !== undefined) {
      // 누르자마자 읽는 중. 소리를 만드는 동안(0.5–0.9초)은 일시정지 대신 회전 표시
      chip.dataset.label = chip.dataset.busy ?? "";
      chip.classList.add("is-loading");
      const text = chip.dataset.text ?? "";
      const speed = Number(chip.dataset.speed) || 1;
      const url = (await engineLangs()).includes(langPrefix(lang)) ? await ttsUrl(lang, text, speed) : "";
      const voice = url ? null : await osVoice(lang);
      if (me !== run) return;
      chip.classList.remove("is-loading");
      if (url) await playUrl(url, chip);
      else if (voice) await sayOs(voice, chip, text, speed);
    } else {
      const url = await mediaUrl(chip.dataset.snd ?? "");
      if (me !== run) return;
      const len = chip.dataset.idle ?? "";
      chip.dataset.label = `0:00 / ${len}`;
      if (url) await playUrl(url, chip, (a) => (chip.dataset.label = `${clock(a.currentTime)} / ${len}`));
    }
    if (me !== run) return; // stopSounds 가 이미 정리했다
    settle(chip);
    current = null;
  }
}

export function stopSounds() {
  run++;
  if (!current) return;
  current.stop();
  settle(current.chip);
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
  await Promise.all([
    ...Array.from(tpl.content.querySelectorAll("span.snd[data-snd]"), soundChip),
    ...Array.from(tpl.content.querySelectorAll("span.snd[data-tts]"), ttsChip),
  ]);
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
