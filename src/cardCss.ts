/** 노트 유형 CSS 를 카드 영역에 가두고, 다크 테마에서 덱 카드 면을 정한다. 학습 화면(Study) · 노트 유형 미리보기(NT2 · NT6)가 같이 쓴다 */

/**
 * 다크 테마에서 덱 스타일 카드를 어떻게 그릴까 (Figma Dark · D3-s 40:4911). 카드 바깥(틀 · 상단 · 평가 버튼)은 언제나 앱 토큰이다.
 *  night: 덱 CSS 에 .nightMode / .night_mode 가 있다 → 본문에 두 클래스를 붙이고 덱에 맡긴다 (Anki 와 같게)
 *  light: 야간 처리 없이 밝은 바탕(명도 0.6 이상)을 칠한다 → 카드 면 전체를 밝은 토큰으로 (한 화면에 밝은 상자만 섞이지 않게)
 *  other: 그 밖 (직접 만든 카드, 글자색만 정한 덱) → 다크 토큰 그대로, 어두운 글자색 선언만 버린다 (scopeCss)
 * 그림은 거르거나 뒤집지 않는다. 노트타입 CSS 마다 한 번만 정한다
 */
export type DeckFace = "night" | "light" | "other";
const faces = new Map<string, DeckFace>();

type RGB = [number, number, number];

let paint: CanvasRenderingContext2D | null = null;
/** CSS 색 → RGB (0~255). 투명하거나 읽을 수 없으면 null */
function rgbOf(color: string): RGB | null {
  paint ??= document.createElement("canvas").getContext("2d");
  if (!paint || !color) return null;
  paint.fillStyle = "rgba(0, 0, 0, 0)";
  paint.fillStyle = color; // 못 읽는 값이면 그대로 남는다. 결과는 "#rrggbb" 또는 "rgba(r, g, b, a)"
  const v = String(paint.fillStyle);
  const n = v.startsWith("#") ? [1, 3, 5].map((i) => parseInt(v.slice(i, i + 2), 16)) : (v.match(/[\d.]+/g) ?? []).map(Number);
  if (n.length === 4 && n[3] === 0) return null;
  return [n[0], n[1], n[2]];
}

/** RGB → HSL (h 0~360, s · l 0~1) */
function toHsl([r, g, b]: RGB): RGB {
  const [x, y, z] = [r / 255, g / 255, b / 255];
  const max = Math.max(x, y, z);
  const min = Math.min(x, y, z);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === x ? ((y - z) / d + 6) % 6 : max === y ? (z - x) / d + 2 : (x - y) / d + 4;
  return [h * 60, s, l];
}
function fromHsl([h, s, l]: RGB): RGB {
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return [r, g, b].map((v) => Math.round((v + m) * 255)) as RGB;
}
const hsl = (color: string) => {
  const c = rgbOf(color);
  if (!c) return null;
  const [, s, l] = toHsl(c);
  return { l, s };
};

/** WCAG 대비 (1~21) */
export function contrastRatio(a: RGB, b: RGB): number {
  const lin = (v: number) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  const lum = ([r, g, b]: RGB) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** 색상(hue) · 채도는 두고 명도만 올려 바탕과 ratio 이상이 되게 (다크 테마의 덱 강조색. "붉은 글씨"는 붉은 채로 읽히게) */
export function liftToContrast(fg: RGB, bg: RGB, ratio = 4.5): RGB {
  if (contrastRatio(fg, bg) >= ratio) return fg;
  const [h, s, l0] = toHsl(fg);
  for (let l = l0; l <= 1; l += 0.01) {
    const c = fromHsl([h, s, Math.min(1, l)]);
    if (contrastRatio(c, bg) >= ratio) return c;
  }
  return [255, 255, 255];
}

/** cdpp 로 돌리는 검사: await (await import("/src/cardCss.ts")).contrastSelfCheck() */
export function contrastSelfCheck() {
  const bg: RGB = [0x1b, 0x1a, 0x21]; // 다크 bg/surface
  const red: RGB = [204, 0, 0]; // 덱의 b { color: #c00 }
  const lifted = liftToContrast(red, bg);
  const hueOf = (c: RGB) => toHsl(c)[0];
  return {
    whiteOnBlack21: Math.abs(contrastRatio([255, 255, 255], [0, 0, 0]) - 21) < 0.01,
    sameIs1: contrastRatio(bg, bg) === 1,
    redBefore: Number(contrastRatio(red, bg).toFixed(2)),
    redAfter: Number(contrastRatio(lifted, bg).toFixed(2)),
    redLiftedTo45: contrastRatio(lifted, bg) >= 4.5,
    redStaysRed: Math.abs(hueOf(lifted) - hueOf(red)) < 2,
    lifted: `rgb(${lifted.join(", ")})`,
    alreadyReadableUnchanged: liftToContrast([255, 200, 200], bg).join() === "255,200,200",
    blueLifted: contrastRatio(liftToContrast([26, 95, 180], bg), bg) >= 4.5,
  };
}

/** 스타일 규칙을 중첩 · @media 안까지 */
function* styleRules(list: CSSRuleList): Generator<CSSStyleRule> {
  for (const r of Array.from(list)) {
    if (r instanceof CSSStyleRule) yield r;
    if ("cssRules" in r) yield* styleRules((r as CSSGroupingRule).cssRules);
  }
}

export function deckFace(css: string): DeckFace {
  let face = faces.get(css);
  if (!face) {
    const sheet = new CSSStyleSheet();
    try {
      sheet.replaceSync(css);
    } catch {
      /* 못 읽는 CSS 는 other */
    }
    const light = [...styleRules(sheet.cssRules)].some((r) => (hsl(r.style.backgroundColor)?.l ?? 0) >= 0.6);
    face = /\.night_?mode\b/i.test(css) ? "night" : light ? "light" : "other";
    faces.set(css, face);
  }
  return face;
}

/** 다크 테마의 other: 어두운 글자색(검정 · 회색 · 남색처럼 어둡거나 탁한 색)은 버리고 text/primary 를 쓴다. 빨강 강조는 남긴다 */
const darkText = (color: string) => {
  const c = hsl(color);
  return !!c && (c.l < 0.35 || (c.l < 0.5 && c.s < 0.3));
};

/**
 * 노트타입 CSS 를 카드 본문 안으로 가둔다 (CSS 중첩). .card 규칙은 맞는 요소가 없어 앱 글꼴을 덮지 않는다.
 * 괄호를 닫고 밖으로 나오는 CSS 는 규칙이 둘 이상으로 갈라지므로 통째로 버린다 (가져온 덱은 신뢰하지 않는다).
 * Anki 는 .card 와 .nightMode 를 같은 body 에 붙인다. 여기서는 .card 를 붙이지 않으므로 ".card.nightMode" 를 ".nightMode" 로 읽는다
 */
export function scopeCss(css: string, stripDarkText: boolean, scope = ".study__card-content"): string {
  if (!css.trim()) return "";
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(`${scope}{${css.replace(/\.card\.(nightMode|night_mode)\b|\.(nightMode|night_mode)\.card\b/g, ".$1$2")}}`);
    const rule = sheet.cssRules[0];
    if (sheet.cssRules.length !== 1 || !(rule instanceof CSSStyleRule) || rule.selectorText !== scope) return "";
    rule.style.cssText = ""; // 선택자 없이 쓴 선언은 본문 글꼴을 덮으므로 버린다
    if (stripDarkText) {
      // ④ 어두운 글자색은 버리고, 남긴 강조색은 다크 카드 면과 4.5:1 이 되도록 명도만 올린다. 바탕을 직접 칠한 규칙은 그 바탕 기준이라 건드리지 않는다
      const bg = rgbOf(getComputedStyle(document.documentElement).getPropertyValue("--bg-surface").trim());
      for (const r of styleRules(rule.cssRules)) {
        if (darkText(r.style.color)) r.style.removeProperty("color");
        else if (bg && !r.style.backgroundColor) {
          const c = rgbOf(r.style.color);
          const lifted = c && liftToContrast(c, bg);
          if (c && lifted && lifted.join() !== c.join()) r.style.color = `rgb(${lifted.join(", ")})`;
        }
      }
    }
    return rule.cssText;
  } catch {
    return "";
  }
}
