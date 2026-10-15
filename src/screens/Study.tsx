// D1 학습 시작 전 요약 / D2 질문 / D3 답변과 평가 / D4 접어두기 / C3-e 카드 편집
// J1~J3 서술형 · H3 압축 회독 · G3 학습 중 신호 · I2 원자료 패널
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { OcclusionEdit } from "./Occlusion";
import { api, errorText, type CardView, type ExamState, type Level, type NoteEdit, type QueueSummary } from "../api";
import { Badge, Button, Checkbox, Icon, Importance, Kbd, LEVEL_LABEL, RatingButton, Toggle, type Rating } from "../ui";
import { eul, n, useKey, useNav, useOverlayScope } from "../nav";
import { actionKey, getKeys, matchAction, useKeymap } from "../keys";
import { useToast } from "../toast";
import { plainText, renderCardHtml, toStoredHtml } from "../cardHtml";
import { findMath, withMath } from "../math";
import {
  cram as cramApi, exam as examApi, splitLater, takeSignal, written as writtenApi, type Compare, type Keyword, type MixRun, type MixStats, type WrittenCard, type WrittenNote,
} from "../written";
import { Modal } from "./NewDeck";
import { DeckPicker, rootDeckIdOf, type DeckChoice } from "./DeckPicker";
import { RichField, makeCloze, toCloze, fromCloze, type FieldHandle } from "./AddCard";
import { CramHint, KeywordEditor } from "./StudyParts";
import { SegmentPanel, SourcePanel } from "./SourcePanel";
import { useGrade } from "./GradeAssist";
import { ExamField, examHint } from "./Exam";
import { cardSource } from "../sourceApi";
import "../styles/study.css";

/*
 * I2 원자료 패널은 가져오기 쪽이 만든다 (SourcePanel.tsx · sourceApi.ts · source.rs). 패널은 420px 겉모양까지 스스로 그리고,
 * 이 화면은 자리만 잡는다. 카드를 보일 때 새로 물어 둔 값을 패널이 그대로 쓴다 (cardSource 캐시)
 */
const hasSource = (cardId: number) => cardSource(cardId, true).then((s) => s !== null);

// 이 기기에서만 쓰는 편의 설정. 지워져도 기본 동작으로 돌아갈 뿐이다
export const SKIP_SUMMARY = "neo.skipSummary";
const HINTS_SEEN = "neo.ratingHintsSeen";
export function pref(key: string) {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}
export function setPref(key: string, on: boolean) {
  try {
    if (on) localStorage.setItem(key, "1");
    else localStorage.removeItem(key);
  } catch {
    /* 저장이 막힌 환경. 다음 실행에서 기본값을 쓴다 */
  }
}

/** 덱 경로. D2 머리글은 "최상위 › 나머지 · 나머지" */
export function pathLabel(path: string[]) {
  return path.length > 1 ? `${path[0]} › ${path.slice(1).join(" · ")}` : (path[0] ?? "");
}

// ---------------------------------------------------------------- D1

const SCALES = [
  { value: 0.5, word: "가볍게" },
  { value: 1, word: "권장" },
  { value: 1.5, word: "더 많이" },
];
const newOf = (q: QueueSummary) => q.new_core + q.new_normal + q.new_supp;

/** D1. cram 이면 H3 압축 회독 화면 (홈 · D1 의 [압축 회독]) */
/** D1. 시험 전 회독(P1)은 PreExam.tsx */
export function Summary({ deckIds }: { deckIds: number[] | null }) {
  return <Plan deckIds={deckIds} />;
}

function Plan({ deckIds }: { deckIds: number[] | null }) {
  const nav = useNav();
  const [plans, setPlans] = useState<QueueSummary[] | null>(null);
  const [scale, setScale] = useState(1);
  const [skip, setSkip] = useState(() => pref(SKIP_SUMMARY));
  const [error, setError] = useState<string>();
  const skipped = useRef(pref(SKIP_SUMMARY));

  useEffect(() => {
    // "다음부터 바로 시작" 을 켜 두었으면 요약을 건너뛴다
    if (skipped.current) nav({ name: "study", deckIds, scale: 1 });
    else Promise.all(SCALES.map((s) => api.queuePreview(deckIds, s.value))).then(setPlans, (e) => setError(errorText(e)));
  }, [deckIds, nav]);

  const q = plans?.[SCALES.findIndex((s) => s.value === scale)];
  const start = useCallback(() => {
    if (!q || q.total === 0) return;
    setPref(SKIP_SUMMARY, skip);
    nav({ name: "study", deckIds, scale });
  }, [q, skip, nav, deckIds, scale]);
  const back = useCallback(
    () => nav(deckIds?.length === 1 ? { name: "deck", deckId: deckIds[0] } : { name: "home" }),
    [nav, deckIds],
  );
  useKey("Enter", start, !!q);
  useKey("Escape", back);
  if (!q || !plans) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  const reviewCount = q.learning + q.review + q.backlog;
  const t = q.target;
  // 머리 문장은 순서 원칙만. 핵심 우선은 늘 적용되는 규칙이다. 목표일은 그 덱의 신규 수를 나눴을 때만 덧붙인다
  // (큐 사유가 "남은 N장 ÷ M일" 계산식이면 목표일이 수를 정한 것)
  const base = "복습을 먼저 비우고, 신규는 핵심부터 넣었어요.";
  const split = !!t && q.notes.some((x) => x.startsWith(`${t.name}: `) && x.includes("÷"));
  // P9: 시험 전 회독(혼합)에서 본 오늘 예정 카드는 오늘의 학습에서 빠졌다
  const mixed = q.mixed_today > 0 ? ` 시험 전 회독에서 ${n(q.mixed_today)}장을 이미 봤어요.` : "";
  const reason =
    (!t
      ? base
      : t.dday === 0
        ? `${base} ${t.name} 목표일이 오늘이에요.`
        : split
          ? `${base} ${t.name} 목표일까지 ${t.dday}일이라 하루 몫을 나눴어요.`
          : base) + mixed;
  // 핵심 신규 캡션은 숫자와 맞게: 여러 덱에서 오면 그렇다고, 목표 계산식은 결과까지 (하루 N장)
  const perDay = t ? Math.ceil(t.remaining_new / Math.max(1, t.days)) : 0;
  const coreDesc = !t
    ? "핵심으로 정한 카드부터"
    : q.new_decks > 1
      ? `덱 ${n(q.new_decks)}개에서 오늘 몫만큼 · 목표가 가장 가까운 ${t.name}${eul(t.name) === "을" ? "은" : "는"} 하루 ${n(perDay)}장`
      : `${t.name} · 남은 ${n(t.remaining_new)}장 ÷ ${n(t.days)}일 = 하루 ${n(perDay)}장`;
  const reviewDesc = [
    `예정 ${n(q.learning + q.review)}`,
    q.backlog > 0 && `밀린 ${n(q.backlog)}${q.backlog_deferred > 0 ? ` (나머지 ${n(q.backlog_deferred)}장은 차차)` : ""}`,
    "오늘 보지 않으면 일정이 밀려요",
    // C1-w: 복습 묶음 안에서 약한 챕터의 예정 복습을 먼저 (큐가 이미 그 순서로 준다)
    q.weak_first > 0 && `약한 챕터 먼저 · ${n(q.weak_first)}장`,
  ]
    .filter(Boolean)
    .join(" · ");
  const rows = [
    { key: "buried", dot: "warm", title: "접어둔 카드 다시 보기", desc: "지난 학습에서 두 번 막혔던 카드 · 집중력이 좋을 때 먼저", count: q.returning },
    { key: "review", dot: "ink", title: "복습", desc: reviewDesc, count: reviewCount },
    {
      key: "core",
      dot: "core",
      title: "핵심 신규",
      desc: coreDesc,
      count: q.new_core,
    },
    { key: "normal", dot: "accent", title: "일반 신규", desc: `${n(q.new_decks)}개 덱에서 오늘 몫만큼`, count: q.new_normal },
    { key: "supp", dot: "supp", title: "보충 신규", desc: "범위 완주를 위해 뒤쪽에 조금씩", count: q.new_supp },
  ].filter((r) => r.count > 0);

  return (
    <main
      className="summary"
      // 마우스로 누른 분량 버튼 · 체크박스에 초점이 남으면 Enter 가 그 요소로 가서 학습이 시작되지 않는다 (키보드 Tab 은 그대로)
      onMouseDown={(e) => (e.target as HTMLElement).closest(".segment__item, .checkbox") && e.preventDefault()}
    >
      <section className="summary__left">
        <button className="backlink t-label-m c-secondary" onClick={back}>
          <Icon name="chevron-left" size={18} />
          {deckIds?.length === 1 ? "덱" : "홈"}
        </button>
        <div className="summary__gap" />
        <p className="t-body-l c-secondary">오늘의 학습 · {n(q.deck_count)}개 덱</p>
        <p className="summary__count">
          <span className="summary__num">{n(q.total)}</span>
          <span className="t-display-l">장</span>
        </p>
        <div className="summary__bar-wrap">
          <div className="summary__bar">
            {rows.map((r) => (
              <span key={r.key} className={`summary__seg summary__seg--${r.key}`} style={{ flexGrow: r.count }} />
            ))}
          </div>
        </div>
        {newOf(plans[2]) > 0 && (
          <div className="summary__amount">
            <div className="summary__amount-row">
              <span className="t-body-m-strong">신규 분량</span>
              <div className="segment segment--boxed">
                {SCALES.map((s, i) => (
                  <button
                    key={s.value}
                    className={`segment__item t-label-m ${scale === s.value ? "is-selected" : ""}`}
                    onClick={() => setScale(s.value)}
                  >
                    {s.word} {n(newOf(plans[i]))}
                  </button>
                ))}
              </div>
            </div>
            <div className="summary__captions">
              {/* 프리셋에 따라 신규와 합계만 바뀐다 */}
              <p className="t-caption c-secondary">
                오늘 총 {n(q.total)}장 · 복습 {n(q.total - newOf(q))} + 신규 {n(newOf(q))}
              </p>
              <p className="t-caption c-tertiary">복습은 줄이지 않아요. 예정된 복습을 미루면 기억 일정이 무너져요.</p>
            </div>
          </div>
        )}
        <div className="summary__cta">
          <Button icon="arrow-right" label="학습 시작" shortcut="Enter" onClick={start} disabled={q.total === 0} />
          <Checkbox checked={skip} label="다음부터 바로 시작" onChange={setSkip} />
        </div>
        <CramHint deckIds={deckIds} />
      </section>

      <section className="summary__order">
        <header className="summary__order-head">
          <h2 className="t-title-s">이 순서로 보여드려요</h2>
          <p className="t-body-s c-secondary">{q.total === 0 ? "오늘 학습할 카드가 없어요." : reason}</p>
        </header>
        {rows.map((r, i) => (
          <div key={r.key} className="summary__row">
            <span className="t-label-s c-tertiary">{i + 1}</span>
            <span className={`dot dot--${r.dot}`} />
            <span className="summary__row-text">
              <span className="t-body-m-strong">{r.title}</span>
              <span className="t-caption c-tertiary">{r.desc}</span>
            </span>
            <span className="summary__row-count">
              <span className="t-title-m">{n(r.count)}</span>
              <span className="t-caption c-tertiary">장</span>
            </span>
          </div>
        ))}
      </section>
    </main>
  );
}

// ---------------------------------------------------------------- H3 압축 회독

// ---------------------------------------------------------------- D2 ~ D4

const HINT: Record<Rating, string> = { 1: "기억하지 못함", 2: "일부만 기억함", 3: "필요한 내용을 기억함", 4: "바로 정확하게 기억함" };
const RATING_NAME: Record<Rating, string> = { 1: "Again", 2: "Hard", 3: "Good", 4: "Easy" };
const RATING_DOT: Record<Rating, string> = { 1: "again", 2: "hard", 3: "good", 4: "easy" }; // components.css .dot--*
const SEP = "   ·   ";

/** written: J 서술형 정보 (써서 답하기 · 정답 글자 · 필수 키워드). source: I2 연결된 자료가 있다 */
type Shown = { view: CardView; front: string; question: string; answer: string; written: WrittenCard | null; source: boolean };
type Folded = { id: number; question: string; importance: Level; deck_path: string[]; auto: boolean; at: number };
/** 되돌릴 수 있는 일. D2-u: 이 세션의 마지막 평가나 접기 하나뿐 (백엔드는 더 쌓지만 화면에서 막는다). log: H3 압축 회독 기록 id */
type Last = { id: number; rating: Rating | null; phase: "q" | "a"; counted: boolean; folded: boolean; log?: number };
/** 접기 토스트. reason: G3 카드 모양 신호 ("답이 10항목이라"). source: I2 [원자료 보기] */
type FoldToast = { id: number; auto: boolean; reason?: string; source?: boolean };
type AnswerAction = { action: string; due_in_ms: number | null; log?: number };

/** J2 비교 글. 맞은 핵심어는 초록 밑줄, 빠진 핵심어는 빨간 굵은 글씨 (위치는 Rust 가 준 JS 문자열 위치) */
function marked(text: string, hits: [number, number][], misses: [number, number][] = []): ReactNode[] {
  const cls = new Array<string>(text.length).fill("");
  for (const [s, e] of misses) cls.fill("miss", s, e);
  for (const [s, e] of hits) cls.fill("hit", s, e);
  const out: ReactNode[] = [];
  for (let i = 0; i < text.length; ) {
    let j = i;
    while (j < text.length && cls[j] === cls[i]) j++;
    out.push(cls[i] ? <mark key={i} className={`cmp__${cls[i]}`}>{text.slice(i, j)}</mark> : text.slice(i, j));
    i = j;
  }
  return out;
}

const daysAgo = (at: number) => {
  const d = Math.floor((Date.now() - at) / 86_400_000);
  return d < 1 ? "오늘" : `${d}일 전`;
};

/**
 * 뒷면을 질문 부분과 답 부분으로 나눈다. Basic 템플릿은 <hr id=answer> 로 나뉜다.
 * 구분선 없이 {{FrontSide}} 바로 뒤에 답을 붙인 템플릿은 앞면이 그대로 앞에 있으므로 거기서 나눈다
 */
function splitBack(back: string, front: string) {
  const m = back.match(/<hr[^>]*\bid=["']?answer\b["']?[^>]*>/i);
  if (m && m.index !== undefined) return { question: back.slice(0, m.index), answer: back.slice(m.index + m[0].length) };
  const f = front.trim();
  const b = back.trimStart();
  return f && b.startsWith(f) ? { question: f, answer: b.slice(f.length) } : { question: "", answer: back };
}

/**
 * 다크 테마에서 덱 스타일 카드를 어떻게 그릴까 (Figma Dark · D3-s 40:4911). 카드 바깥(틀 · 상단 · 평가 버튼)은 언제나 앱 토큰이다.
 *  night: 덱 CSS 에 .nightMode / .night_mode 가 있다 → 본문에 두 클래스를 붙이고 덱에 맡긴다 (Anki 와 같게)
 *  light: 야간 처리 없이 밝은 바탕(명도 0.6 이상)을 칠한다 → 카드 면 전체를 밝은 토큰으로 (한 화면에 밝은 상자만 섞이지 않게)
 *  other: 그 밖 (직접 만든 카드, 글자색만 정한 덱) → 다크 토큰 그대로, 어두운 글자색 선언만 버린다 (scopeCss)
 * 그림은 거르거나 뒤집지 않는다. 노트타입 CSS 마다 한 번만 정한다
 */
type DeckFace = "night" | "light" | "other";
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

/** cdpp 로 돌리는 검사: await (await import("/src/screens/Study.tsx")).contrastSelfCheck() */
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

function deckFace(css: string): DeckFace {
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
function scopeCss(css: string, stripDarkText: boolean): string {
  if (!css.trim()) return "";
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(`.study__card-content{${css.replace(/\.card\.(nightMode|night_mode)\b|\.(nightMode|night_mode)\.card\b/g, ".$1$2")}}`);
    const rule = sheet.cssRules[0];
    if (sheet.cssRules.length !== 1 || !(rule instanceof CSSStyleRule) || rule.selectorText !== ".study__card-content") return "";
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

/** 토스트 · 목록에 쓰는 한 줄 질문. 붙어 있는 태그 사이에 띄어쓰기를 넣는다 ("OX높음" 이 되지 않게) */
/** 수식은 구분자만 벗겨 TeX 원문으로 ("\(z\)" → "z"). 한 줄 글에서 그린 수식은 읽기 어렵다 */
const lineOf = (html: string) => {
  const text = plainText(html.replace(/></g, "> <"));
  return findMath(text).reduceRight((t, m) => t.slice(0, m.start) + m.tex.trim() + t.slice(m.end), text);
};

/** <html data-theme> 를 따라간다. 학습 중에 설정이나 OS 테마가 바뀌어도 */
function useDarkTheme() {
  const read = () => document.documentElement.dataset.theme === "dark";
  const [dark, setDark] = useState(read);
  useEffect(() => {
    const mo = new MutationObserver(() => setDark(read()));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => mo.disconnect();
  }, []);
  return dark;
}

function ago(at: number) {
  const m = Math.floor((Date.now() - at) / 60_000);
  return m < 1 ? "방금" : m < 60 ? `${m}분 전` : `${Math.floor(m / 60)}시간 전`;
}

export function Study(props: { deckIds: number[] | null; scale: number; cardIds?: number[]; sessionId?: number; cram?: boolean; mix?: MixRun }) {
  const { deckIds, scale, mix } = props;
  const cram = !!props.cram;
  const nav = useNav();
  const notify = useToast();
  const session = useRef<number | null>(null);
  const cramAgain = useRef(new Map<number, number>()); // H3: 이 화면 안의 Again 횟수 (두 번째 Again 에 접는다)
  const cramDone = useRef(new Set<number>()); // H3: 평가한 카드 (마칠 때 "N장을 훑었어요")
  // P2 혼합 회독: 카드마다 남은 자리 (null 전체, 문자열 취약 이유). 평가할 때 앞 자리부터 센다
  const mixRoles = useRef(new Map<number, (string | null)[]>());
  const mixStats = useRef<MixStats>({ full: [0, 0], weak: [0, 0], again: [0, 0], due: 0 });
  const mixReplayed = useRef(false); // 접어둔 카드는 회독 끝에 한 번 더
  const openedAt = useRef(Date.now()); // P3 "N장 회독 · M분": 이 화면에서 쓴 시간 (같은 날 이어 쓴 세션의 쉬던 시간은 빼고)
  const foldedIds = useRef<number[]>([]);
  const [typed, setTyped] = useState(""); // J1 내 답
  const [compare, setCompare] = useState<Compare | null>(null); // J2
  const [srcOpen, setSrcOpen] = useState(false); // I2 세션 동안 열림 상태를 기억한다
  const writeBox = useRef<HTMLTextAreaElement>(null);
  const queue = useRef<number[]>([]);
  const next = useRef<number[]>([]); // 바로 다음에 볼 카드 (D4-m 지금 보기, 되돌리기 전에 보던 카드)
  const later = useRef<{ id: number; due: number }[]>([]); // 이 세션 안에서 다시 나올 카드 (Again, 학습 단계)
  const started = useRef(false);
  const busy = useRef(false);
  const shownAt = useRef(0);
  const scroller = useRef<HTMLDivElement>(null);
  const foldBox = useRef<HTMLDivElement>(null);
  const [total, setTotal] = useState(0);
  const [done, setDone] = useState(0);
  const [card, setCard] = useState<Shown | null>(null);
  const [phase, setPhase] = useState<"q" | "a">("q");
  const [folded, setFolded] = useState<Folded[]>([]);
  const [toast, setToast] = useState<FoldToast | null>(null);
  const [rated, setRated] = useState<{ rating: Rating; question: string } | null>(null); // D2-u 되돌리기 토스트
  const [last, setLast] = useState<Last | null>(null);
  const [undone, setUndone] = useState<Rating | null>(null); // D3-u 방금 되돌린 평가
  const [foldMenu, setFoldMenu] = useState(false);
  const [foldSel, setFoldSel] = useState(0);
  // 확대 보기: 그림 주소, 또는 그림 가리기(M3)면 덮개까지 그린 HTML (원본 그림만 크게 보이면 답이 보인다)
  const [zoom, setZoom] = useState<{ src: string } | { html: string } | null>(null);
  const [fit, setFit] = useState({ overflow: false, more: false });
  const [editing, setEditing] = useState(false);
  const [hints, setHints] = useState(() => !pref(HINTS_SEEN));
  const [error, setError] = useState<string>();
  // R2 채점 보조: 학습 세션마다 한 번 확인 · 근거 칩으로 연 구간 (I2 패널 자리)
  const gradeOk = useRef(false);
  const [srcSeg, setSrcSeg] = useState<number | null>(null);
  const grade = useGrade({ cardId: card?.view.id ?? null, typed, on: phase === "a" && !!compare, confirmed: gradeOk, openSegment: setSrcSeg });

  const finish = useCallback(
    async (completed: boolean) => {
      // H3: 세션이 없다 (오늘의 학습과 따로). 몇 장 훑었는지만 알리고 홈으로
      if (cram) {
        notify({ title: `압축 회독 ${n(cramDone.current.size)}장을 훑었어요`, text: "복습 일정과 오늘의 학습은 그대로예요." });
        return nav({ name: "home" });
      }
      if (session.current === null) return nav({ name: "home" });
      setPref(HINTS_SEEN, true); // "다음 학습부터는 숨겨요"
      const summary = await api.finishSession(session.current, completed, deckIds, scale);
      if (mix) {
        const minutes = Math.max(summary.studied > 0 ? 1 : 0, Math.round((Date.now() - openedAt.current) / 60_000));
        return nav({ name: "mixDone", sessionId: session.current, run: mix, stats: mixStats.current, minutes, completed });
      }
      nav({ name: "done", sessionId: session.current, summary, completed, deckIds, scale });
    },
    [nav, deckIds, scale, cram, notify, mix],
  );

  const show = useCallback(async (id: number, at: "q" | "a" = "q") => {
    // J 서술형 정보 · I2 자료 연결은 없어도 카드는 보인다
    const [view, written, source] = await Promise.all([api.cardView(id), writtenApi.card(id).catch(() => null), hasSource(id)]);
    const parts = splitBack(view.back, view.front);
    // 살균(renderCardHtml) 뒤에 수식을 그린다 (math.ts)
    const [front, question, answer] = (await Promise.all([
      renderCardHtml(view.front),
      renderCardHtml(parts.question),
      renderCardHtml(parts.answer),
    ])).map(withMath);
    setCard({ view, front, question, answer, written, source });
    setPhase(at);
    setUndone(null);
    setTyped("");
    setCompare(null);
    setSrcSeg(null);
    shownAt.current = Date.now();
  }, []);

  /** 다음 카드. 바로 다음으로 끼운 카드, 다시 볼 시각이 된 카드, 원래 순서, 마지막으로 남은 다시 볼 카드 */
  const advance = useCallback(async () => {
    for (;;) {
      const now = Date.now();
      const l = later.current.sort((a, b) => a.due - b.due);
      const id = next.current.shift() ?? (l.length && l[0].due <= now ? l.shift()!.id : (queue.current.shift() ?? l.shift()?.id));
      if (id === undefined && mix && !mixReplayed.current && foldedIds.current.length > 0) {
        // P2 혼합 회독: 접어둔 카드는 회독 끝에 한 번 더 (두 번째 접히면 그대로 둔다)
        mixReplayed.current = true;
        const ids = foldedIds.current;
        for (const f of ids) await api.unburyCard(session.current!, f);
        setFolded([]);
        setDone((d) => d - ids.length);
        queue.current.push(...ids);
        continue;
      }
      if (id === undefined) return finish(true);
      try {
        return await show(id);
      } catch {
        // 편집 중 지워진 빈칸 카드 등. 건너뛴다
      }
    }
  }, [finish, show, mix]);
  useEffect(() => void (foldedIds.current = folded.map((f) => f.id)), [folded]);

  useEffect(() => {
    if (started.current) return; // StrictMode 에서 두 번 부르지 않게
    started.current = true;
    (async () => {
      try {
        // H3 압축 회독은 세션을 열지 않는다 (오늘의 학습 · 완주 지표와 따로)
        const s: { session_id: number | null; card_ids: number[] } = cram
          ? { session_id: null, card_ids: props.cardIds ?? [] }
          : props.cardIds && props.sessionId !== undefined
            ? { session_id: props.sessionId, card_ids: props.cardIds }
            : await api.startSession(deckIds, scale);
        session.current = s.session_id;
        queue.current = [...s.card_ids];
        if (mix) {
          for (const c of mix.cards) mixRoles.current.set(c.id, [...(mixRoles.current.get(c.id) ?? []), c.weak]);
          const weak = mix.cards.filter((c) => c.weak).length;
          mixStats.current = { full: [0, mix.cards.length - weak], weak: [0, weak], again: [0, 0], due: 0 };
        }
        setTotal(s.card_ids.length);
        await advance();
      } catch (e) {
        setError(errorText(e));
      }
    })();
  }, [props.cardIds, props.sessionId, deckIds, scale, advance, cram]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 8000);
    return () => clearTimeout(t);
  }, [toast]);
  useEffect(() => {
    if (!rated) return;
    const t = setTimeout(() => setRated(null), 4000);
    return () => clearTimeout(t);
  }, [rated]);

  // D4-m 팝오버는 바깥을 누르면 닫힌다
  useEffect(() => {
    if (!foldMenu) return;
    const close = (e: MouseEvent) => !foldBox.current?.contains(e.target as Node) && setFoldMenu(false);
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [foldMenu]);

  // D3-l 넘칠 때만 페이드 · "아래에 더 있어요" · 스크롤 안내
  const measure = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    const overflow = el.scrollHeight > el.clientHeight + 1;
    const more = el.scrollHeight - el.scrollTop - el.clientHeight > 1;
    setFit((f) => (f.overflow === overflow && f.more === more ? f : { overflow, more }));
  }, []);
  // 새 카드나 답을 열면 맨 위부터. 그림이 늦게 그려지거나 창 크기가 바뀌면 다시 잰다
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    el.scrollTop = 0;
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    // M3: 아주 긴 그림이라 넘치면 질문 면은 묻는 칸이 보이게 내린다 (그림이 그려진 뒤에 위치를 안다)
    const ask = () => {
      const a = el.querySelector(".io-mask--ask")?.getBoundingClientRect();
      const b = el.getBoundingClientRect();
      if (a && (a.bottom > b.bottom - 40 || a.top < b.top)) el.scrollTop += a.top - b.top - (b.height - a.height) / 2;
    };
    if (phase === "q") {
      ask();
      el.addEventListener("load", ask, true);
    }
    return () => {
      ro.disconnect();
      el.removeEventListener("load", ask, true);
    };
  }, [card, phase, measure]);

  const dark = useDarkTheme();
  const css = card?.view.css ?? "";
  const face = dark && css.trim() ? deckFace(css) : null;
  const noteCss = useMemo(() => scopeCss(css, face === "other"), [css, face]);

  /*
   * 평가 · 접기 · 꺼내기 · 되돌리기. H3 압축 회독은 같은 화면 규칙(두 번째 Again 에 접기, Ctrl Z)을 쓰되
   * 카드 일정과 세션을 건드리지 않는다: 평가는 review_log 에 압축 회독으로만 남기고 접기는 이 화면 안에서만 (cram.rs)
   */
  const backend = useMemo(
    () =>
      cram
        ? {
            answer: async (id: number, r: Rating, ms: number): Promise<AnswerAction> => {
              const log = await cramApi.answer(id, r, ms);
              const again = (cramAgain.current.get(id) ?? 0) + (r === 1 ? 1 : 0);
              cramAgain.current.set(id, again);
              cramDone.current.add(id);
              return { action: r !== 1 ? "next" : again >= 2 ? "buried" : "again_later", due_in_ms: 60_000, log };
            },
            bury: async () => {},
            unbury: async (id: number) => void cramAgain.current.set(id, 0),
            undo: async (u: Last) => {
              if (u.log !== undefined) await cramApi.undo(u.log);
              if (u.rating === 1) cramAgain.current.set(u.id, Math.max(0, (cramAgain.current.get(u.id) ?? 1) - 1));
              return u.id;
            },
          }
        : {
            answer: (id: number, r: Rating, ms: number): Promise<AnswerAction> => api.answer(session.current!, id, r, ms),
            bury: (id: number) => api.buryCard(session.current!, id),
            unbury: (id: number) => api.unburyCard(session.current!, id),
            undo: (_: Last) => api.undoLast(session.current!),
          },
    [cram],
  );

  const guard = async (work: () => Promise<void>) => {
    if (busy.current || !card || (session.current === null && !cram)) return;
    busy.current = true;
    try {
      await work();
      setError(undefined);
    } catch (e) {
      setError(errorText(e));
    } finally {
      busy.current = false;
    }
  };

  const fold = (c: Shown, auto: boolean) => {
    setFolded((list) => [
      ...list,
      { id: c.view.id, question: lineOf(c.view.front), importance: c.view.importance, deck_path: c.view.deck_path, auto, at: Date.now() },
    ]);
    // I2: 두 번째 Again 으로 접히는 카드에 자료가 있으면 [원자료 보기]
    setToast({ id: c.view.id, auto, source: auto && c.source });
    setRated(null); // 접기 토스트가 우선한다. 되돌리기는 그 안에 합친다
    // G3: 카드 모양 문제(G1 규칙: 긴 답 · 여러 물음)도 있으면 둘째 줄과 [나누기 제안]. 같은 카드는 하루 한 번
    if (auto)
      writtenApi.splitSignal(c.view.id).then(
        (reason) => reason && takeSignal(c.view.id) && setToast((t) => (t?.id === c.view.id ? { ...t, reason } : t)),
        () => {},
      );
  };

  // R2: 결과가 오기 전에 평가하면 채점 보조 요청을 멈추고 결과는 버린다
  const rate = (r: Rating) => (grade.stop(), rate2(r));
  const rate2 = (r: Rating) =>
    guard(async () => {
      const c = card!;
      const res = await backend.answer(c.view.id, r, Date.now() - shownAt.current);
      if (mix) {
        const role = mixRoles.current.get(c.view.id)?.shift();
        const st = mixStats.current;
        if (role !== undefined) {
          const k = role === null ? "full" : "weak";
          st[k][0] += 1;
          if (r === 1) st.again[role === null ? 0 : 1] += 1;
        }
        if (mix.dueToday.includes(c.view.id) && role === null) st.due += 1;
      }
      const counted = res.action !== "again_later";
      if (counted) setDone((d) => d + 1);
      else later.current.push({ id: c.view.id, due: Date.now() + (res.due_in_ms ?? 60_000) });
      setLast({ id: c.view.id, rating: r, phase: "a", counted, folded: res.action === "buried", log: res.log });
      if (res.action === "buried") fold(c, true);
      else {
        setToast(null); // 토스트 자리는 하나. 연달아 평가하면 내용만 바뀐다
        setRated({ rating: r, question: lineOf(c.view.front) });
      }
      await advance();
    });

  const bury = () =>
    guard(async () => {
      const c = card!;
      await backend.bury(c.view.id);
      setDone((d) => d + 1);
      setLast({ id: c.view.id, rating: null, phase, counted: true, folded: true });
      fold(c, false);
      await advance();
    });

  /** 접어둔 카드를 다시 꺼낸다. now: 지금 바로 (D4 토스트, 보던 카드는 바로 다음으로), 아니면 바로 다음 카드로 (D4-m) */
  const unfold = (ids: number[], now = false) =>
    guard(async () => {
      for (const id of ids) await backend.unbury(id);
      setFolded((list) => list.filter((f) => !ids.includes(f.id)));
      setDone((d) => d - ids.length);
      setToast(null);
      setFoldMenu(false);
      setLast((l) => (l && ids.includes(l.id) ? null : l)); // 꺼낸 카드의 접기는 이미 풀렸다
      if (now) {
        next.current.unshift(card!.view.id);
        await show(ids[0]);
      } else next.current.unshift(...ids);
    });

  /** D2-u / D3-u. 마지막 평가(또는 접기)를 되돌리고 그 카드의 그 화면으로 돌아간다 */
  const undo = () => {
    if (!last) return notify({ title: "더 이전 평가는 되돌릴 수 없어요", icon: "info" });
    return guard(async () => {
      const u = last;
      const id = await backend.undo(u);
      setLast(null);
      setRated(null);
      if (id === null) return;
      if (id === u.id) {
        if (u.counted) setDone((d) => d - 1);
        if (u.folded) {
          setFolded((list) => list.filter((f) => f.id !== id)); // 두 번째 Again 으로 접혔던 카드면 접힘도 풀린다
          setToast(null);
        }
      }
      later.current = later.current.filter((l) => l.id !== id);
      queue.current = queue.current.filter((x) => x !== id);
      next.current = next.current.filter((x) => x !== id);
      if (card!.view.id !== id) next.current.unshift(card!.view.id);
      try {
        await show(id, u.phase);
        setUndone(u.rating);
      } catch {
        await advance(); // 그 사이 지워진 카드
      }
    });
  };

  const scrollCard = (e: { key: string }) => {
    const el = scroller.current;
    if (!el) return;
    const page = el.clientHeight * 0.85;
    const step: Record<string, number> = { ArrowDown: 48, ArrowUp: -48, PageDown: page, PageUp: -page };
    el.scrollBy({ top: step[e.key] ?? 0, behavior: e.key.startsWith("Page") ? "smooth" : "auto" });
  };

  const zoomImage = (e: ReactMouseEvent<HTMLDivElement>) => {
    const io = (e.target as HTMLElement).closest(".io");
    if (io) setZoom({ html: io.outerHTML });
    else if (e.target instanceof HTMLImageElement) setZoom({ src: e.target.src });
  };

  // ---- J1 직접 써보기 (선택). 쓰지 않고 Space 로 답을 봐도 된다
  const writing = phase === "q" && !!card?.written?.written;
  // 서술형 카드가 나오면 입력 칸에 바로 쓴다 (Esc 로 나와서 Space 로 답 보기)
  useEffect(() => {
    if (writing) writeBox.current?.focus({ preventScroll: true });
  }, [writing, card]);
  // 자동 저장 ("지난번에 쓴 답"). 빈 글은 저장하지 않는다
  useEffect(() => {
    if (!writing || !typed.trim() || !card) return;
    const t = setTimeout(() => writtenApi.saveAnswer(card.view.id, typed).catch(() => {}), 600);
    return () => clearTimeout(t);
  }, [typed, writing, card]);
  /** 답 보기. 쓴 답이 있으면 J2 비교 (핵심어 판정 · 평가 추천) */
  const reveal = () => {
    if (!card || phase !== "q") return;
    if (!card.written?.written || !typed.trim()) return setPhase("a");
    return guard(async () => {
      setCompare(await writtenApi.compare(card.view.id, typed));
      setPhase("a");
    });
  };

  // ---- I2 원자료 패널: 답 화면에서만 (질문 화면에서 열면 답이 보일 수 있다)
  const panel = (srcOpen && phase === "a" && !!card?.source) || (srcSeg !== null && phase === "a");
  const openSourceOf = (id: number) =>
    unfold([id], true).then(() => {
      setPhase("a");
      setSrcOpen(true);
    });

  // 바꿀 수 있는 단축키는 설정 > 단축키(E4) 값으로 (keys.ts). 겹침 창(D4-m 목록 · 확대 보기 · 편집 창)이 떠 있으면 화면 키는 꺼진다
  useKeymap();
  useOverlayScope(foldMenu);
  useOverlayScope(!!zoom);
  const keys = !editing && !!card && !zoom;
  useKey({ action: "reveal" }, () => reveal(), keys && phase === "q");
  useKey({ action: "rate" }, (_, i) => rate((i + 1) as Rating), keys && phase === "a");
  useKey({ action: "edit" }, () => setEditing(true), keys);
  useKey({ action: "bury" }, bury, keys);
  useKey({ action: "undo" }, () => undo(), keys);
  useKey({ action: "source" }, () => setSrcOpen((v) => !v), keys && phase === "a" && !!card?.source);
  useKey({ action: "grade" }, () => grade.start(), keys && phase === "a" && grade.ready);
  useKey("b", () => (setFoldSel(0), setFoldMenu(true)), keys && folded.length > 0);
  useKey(["ArrowDown", "ArrowUp", "PageDown", "PageUp"], scrollCard, keys);
  // D4-m 목록 안의 키는 목록이 받는다 (B 로 닫기 · ↑↓ · Enter · Esc)
  useKey(["ArrowDown", "ArrowUp"], (e) => setFoldSel((i) => Math.max(0, Math.min(folded.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))), foldMenu, { modal: true });
  useKey("Enter", () => folded[foldSel] && unfold([folded[foldSel].id]), foldMenu, { modal: true });
  useKey(["Escape", "b"], () => setFoldMenu(false), foldMenu, { modal: true });
  useKey("Escape", () => setZoom(null), !!zoom, { modal: true });
  useKey("Escape", () => finish(false), !editing);

  // 답을 열면 정답 영역으로 초점을 옮겨 화면 읽기 프로그램이 읽게 한다 (단축키는 버튼이 아니라 그대로 동작)
  const answerBox = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (phase === "a") answerBox.current?.focus({ preventScroll: true });
  }, [phase, card]);

  const position = Math.min(done + 1, total);
  const remaining = Math.max(0, total - position);
  // J2 평가 추천 (되돌린 직후에는 되돌리기 안내가 먼저). R2: 핵심어 검사와 ChatGPT 추천을 한 줄로 합친다. 다르면 테두리 없이 둘 다 알린다
  const kwSuggest = undone === null ? (compare?.suggest ?? null) : null;
  const gptSuggest = undone === null ? grade.rating : null;
  const suggest = kwSuggest !== null && gptSuggest !== null ? (kwSuggest === gptSuggest ? kwSuggest : null) : (kwSuggest ?? gptSuggest);
  const missing = compare?.keywords.filter((k) => !k.hit).map((k) => k.word) ?? [];
  const both = kwSuggest !== null && gptSuggest !== null;
  const recLine = both
    ? kwSuggest === gptSuggest
      ? `${RATING_NAME[kwSuggest]} 추천 · 핵심어 ${missing.length ? `${n(missing.length)}/${n(compare!.keywords.length)} 빠짐` : "모두 씀"} · ChatGPT도 ${RATING_NAME[gptSuggest]} · 따르지 않아도 돼요`
      : `핵심어는 ${RATING_NAME[kwSuggest]} · ChatGPT는 ${RATING_NAME[gptSuggest]} · 따르지 않아도 돼요`
    : gptSuggest !== null
      ? `${RATING_NAME[gptSuggest]} 추천 · ChatGPT · 따르지 않아도 돼요`
      : null;
  const tip = hints || undone !== null || suggest !== null || recLine !== null;
  const rateKeys = getKeys("rate").map((_, i) => actionKey("rate", i));
  const keysQ = writing
    ? [`${actionKey("save")} 비교하기`, `Esc 입력 칸 나가기 (그다음 ${actionKey("reveal")}로 바로 답 보기)`]
    : [`${actionKey("reveal")} 답 보기`, mix && "평가가 복습 일정에 반영돼요", fit.overflow && "↑ ↓ 카드 안 스크롤", `${actionKey("edit")} 편집`, `${actionKey("bury")} 접어두기`, last && `${actionKey("undo")} 되돌리기`, "Esc 그만하기"];
  const keysA = [
    `${rateKeys.join(" · ")} 로 평가`,
    mix && "평가가 복습 일정에 반영돼요",
    grade.ready && `${actionKey("grade")} ChatGPT 채점 보조`,
    fit.overflow && "↑ ↓ 카드 안 스크롤",
    card?.source && `${actionKey("source")} 원자료`,
    `${actionKey("edit")} 편집`,
    last && `${actionKey("undo")} 되돌리기`,
  ];
  const keyLine = (parts: unknown[]) => parts.filter(Boolean).join(SEP);
  const keywords = card?.written?.keywords ?? [];

  return (
    <main
      className={`study ${panel ? "has-source" : ""}`}
      // 마우스로 누른 버튼에는 초점을 두지 않는다. 남은 초점이 Space · Enter 를 가져가 답 보기 등이 안 되는 일을 막는다 (키보드 Tab 은 그대로)
      onMouseDown={(e) => (e.target as HTMLElement).closest(".study__tool, .study__toast-btn, .study__undo-btn, .study__more, .study__folded, .rating, .study__reveal") && e.preventDefault()}
    >
      <div className="study__quit">
        <Button variant="ghost" size="M" icon="x" label="그만하기" shortcut="Esc" onClick={() => finish(false)} />
      </div>
      <div className="study__progress">
        <p className="study__progress-num">
          <span className="t-title-s">{n(position)}</span>
          <span className="t-body-s c-tertiary">/ {n(total)}</span>
        </p>
        <span className="study__progress-track">
          <span className="study__progress-fill" style={{ width: total ? `${(position / total) * 100}%` : 0 }} />
        </span>
      </div>
      <div className="study__right">
        {folded.length > 0 && (
          <div className="fold" ref={foldBox}>
            <button
              className={`study__folded t-label-s ${foldMenu ? "is-open" : ""}`}
              aria-expanded={foldMenu}
              aria-keyshortcuts="B"
              onClick={() => (setFoldSel(0), setFoldMenu((v) => !v))}
            >
              <Icon name="archive" size={14} />
              접어둔 카드 {folded.length}
            </button>
            {foldMenu && (
              <div className="fold__pop" role="dialog" aria-label="접어둔 카드">
                <div className="fold__head">
                  <p className="t-title-s">접어둔 카드 {n(folded.length)}장</p>
                  <p className="t-caption c-tertiary">나머지를 끝내면 마지막에 묶어서 다시 보여드려요</p>
                </div>
                <div className="fold__list">
                  {folded.map((f, i) => (
                    <div key={f.id} className={`fold__row ${i === foldSel ? "is-active" : ""}`} onMouseEnter={() => setFoldSel(i)}>
                      <Importance level={f.importance} />
                      <span className="fold__text">
                        <span className="fold__q t-body-m">{f.question}</span>
                        <span className="t-caption c-tertiary">
                          {f.auto ? "두 번 기억나지 않아 자동으로 접음" : "직접 접어둠 (−)"} · {ago(f.at)}
                        </span>
                      </span>
                      {i === foldSel && <Button variant="ghost" size="M" icon="rotate" label="지금 보기" onClick={() => unfold([f.id])} />}
                    </div>
                  ))}
                </div>
                <div className="fold__foot">
                  <Button variant="ghost" size="M" icon="rotate" label="모두 지금 다시 보기" onClick={() => unfold(folded.map((f) => f.id))} />
                  <span className="t-caption c-tertiary">Esc 닫기</span>
                </div>
              </div>
            )}
          </div>
        )}
        {cram && <Badge tone="accent">압축 회독</Badge>}
        {mix && (
          <Badge tone="accent">
            혼합 회독 · 전체 {n(mixStats.current.full[0])}/{n(mixStats.current.full[1])} · 취약 {n(mixStats.current.weak[0])}/{n(mixStats.current.weak[1])}
          </Badge>
        )}
        <span className="t-body-s c-tertiary">남은 {n(remaining)}장</span>
      </div>

      {card && (
        <div className={`study__col ${phase === "a" ? "is-answer" : ""} ${tip ? "has-tip" : ""} ${writing ? "is-writing" : ""}`}>
          {/* 밝은 덱 카드는 카드 면만 밝은 토큰으로 (tokens.css [data-theme="light"]) */}
          <article className={`study__card ${fit.overflow ? "is-overflow" : ""}`} data-theme={face === "light" ? "light" : undefined}>
            {noteCss && <style>{noteCss}</style>}
            <header className="study__card-head">
              <span className="study__meta">
                <Importance level={card.view.importance} />
                {typeof mixRoles.current.get(card.view.id)?.[0] === "string" && <Badge tone="danger">취약 · {mixRoles.current.get(card.view.id)![0]}</Badge>}
                {card.written?.written && <Badge tone="accent">서술형</Badge>}
                <span className="t-caption c-tertiary">{pathLabel(card.view.deck_path)}</span>
                {card.view.occlusion && (
                  <Badge tone="accent">
                    그림 가리기 · {n(card.view.occlusion.n)}/{n(card.view.occlusion.count)}
                  </Badge>
                )}
              </span>
              <span className="study__tools">
                {phase === "a" && card.source && (
                  <button
                    className={`study__tool study__tool--source ${panel ? "is-on" : ""}`}
                    aria-label="원자료 보기"
                    aria-pressed={panel}
                    aria-keyshortcuts={actionKey("source")}
                    onClick={() => setSrcOpen((v) => !v)}
                  >
                    <Icon name="file" size={16} />
                    <span className="t-label-s">원자료</span>
                    {!panel && <Kbd k={actionKey("source")} />}
                  </button>
                )}
                <button className="study__tool" aria-label="카드 편집" aria-keyshortcuts={actionKey("edit")} onClick={() => setEditing(true)}>
                  <Icon name="pencil" size={16} className="c-secondary" />
                  <Kbd k={actionKey("edit")} />
                </button>
                <button className="study__tool" aria-label="접어두기" aria-keyshortcuts={actionKey("bury")} onClick={bury}>
                  <Icon name="archive" size={16} className="c-secondary" />
                  <Kbd k={actionKey("bury")} />
                </button>
              </span>
            </header>
            <div ref={scroller} className="study__scroll" onScroll={measure}>
              <div className="study__card-content" onClick={zoomImage}>
                <div className={face === "night" ? "nightMode night_mode" : undefined}>
                {phase === "q" ? (
                  <div className="study__q card-html" dangerouslySetInnerHTML={{ __html: card.front }} />
                ) : (
                  <div className="study__a" ref={answerBox} tabIndex={-1} aria-label="정답">
                    {plainText(card.question) || card.question.includes("<img") ? (
                      <>
                        <div className="study__a-q t-title-m c-tertiary card-html" dangerouslySetInnerHTML={{ __html: card.question }} />
                        <div className="study__rule" />
                      </>
                    ) : null}
                    {compare ? (
                      <>
                        {(compare.keywords.length > 0 || grade.button) && (
                          <div className="cmp__kw">
                            {compare.keywords.length > 0 && (
                              <span className="t-label-m">
                                핵심어 {n(compare.keywords.length)}개 중 {n(compare.hits)}개
                              </span>
                            )}
                            {compare.keywords.map((k) => (
                              <span key={k.word} className={`kwhit t-label-s ${k.hit ? "is-hit" : "is-miss"}`}>
                                <Icon name={k.hit ? "check" : "x"} size={12} />
                                {k.word}
                              </span>
                            ))}
                            {/* R2 핵심어 줄 오른쪽 [ChatGPT로 채점 보조 G] + 그 아래 보내는 범위 한 줄 */}
                            {grade.button}
                          </div>
                        )}
                        {grade.scopeLine}
                        <div className="cmp">
                          <div className="cmp__col">
                            <span className="t-caption c-tertiary">내 답</span>
                            <p className="cmp__text">{marked(typed, compare.mine_hits)}</p>
                          </div>
                          <div className="cmp__col">
                            <span className="t-caption c-tertiary">정답</span>
                            <p className="cmp__text">{marked(compare.correct, compare.correct_hits, compare.correct_misses)}</p>
                          </div>
                        </div>
                        {grade.slot}
                      </>
                    ) : (
                      <>
                        {/* J2: 쓰지 않고 답을 봤으면 핵심어만 정답 위에 (스스로 채점하게) */}
                        {card.written?.written && keywords.length > 0 && (
                          <div className="cmp__kw">
                            <span className="t-label-m c-secondary">핵심어</span>
                            {keywords.map((k) => (
                              <span key={k.word} className="kwhit t-label-s">
                                {k.word}
                              </span>
                            ))}
                          </div>
                        )}
                        <div className="study__a-body card-html" dangerouslySetInnerHTML={{ __html: card.answer }} />
                        {card.written?.written && card.written.last && card.written.last_at !== null && (
                          <details className="study__last">
                            <summary className="t-label-s c-secondary">지난번에 쓴 답 · {daysAgo(card.written.last_at)}</summary>
                            <p className="t-body-m c-secondary">{card.written.last}</p>
                          </details>
                        )}
                      </>
                    )}
                  </div>
                )}
                </div>
              </div>
            </div>
            {fit.more && (
              <>
                <span className="study__fade" />
                <button className="study__more" onClick={() => scrollCard({ key: "PageDown" })}>
                  <Icon name="chevron-down" size={14} scaled className="c-secondary" />
                  <span className="t-caption c-secondary">아래에 더 있어요 · 휠 또는 ↓</span>
                </button>
              </>
            )}
          </article>

          {phase === "q" ? (
            <>
              {writing && (
                <div className="write">
                  <p className="write__head">
                    <span className="t-label-m">내 답</span>
                    <span className="t-caption c-tertiary">쓰지 않고 답을 봐도 돼요</span>
                  </p>
                  <textarea
                    ref={writeBox}
                    className="write__input"
                    value={typed}
                    aria-label="내 답"
                    onChange={(e) => setTyped(e.target.value)}
                    onKeyDown={(e) => {
                      // 입력 중에는 학습 단축키가 꺼진다. Ctrl Enter 비교하기, Esc 입력 칸 나가기 (화면의 Esc 그만하기로 가지 않게)
                      if (matchAction("save", e.nativeEvent) >= 0) {
                        e.preventDefault();
                        reveal();
                      } else if (e.key === "Escape") {
                        e.preventDefault();
                        e.currentTarget.blur();
                      }
                    }}
                  />
                  <p className="write__foot t-caption c-tertiary">
                    <span>{n(typed.length)}자 · 자동 저장</span>
                    <span>Esc로 입력 칸 나가기</span>
                  </p>
                </div>
              )}
              <Button
                className="study__reveal"
                label={writing ? "답과 비교하기" : "답 보기"}
                shortcut={writing ? actionKey("save") : actionKey("reveal")}
                aria-keyshortcuts={writing ? actionKey("save") : actionKey("reveal")}
                onClick={reveal}
              />
              {!toast && <p className={`study__keys t-caption c-tertiary ${rated ? "is-pushed" : ""}`}>{keyLine(keysQ)}</p>}
            </>
          ) : (
            <>
              <div className="study__tipwrap">
                {undone !== null ? (
                  <span className="study__tip">
                    <Icon name="rotate" size={16} className="c-accent" />
                    <span className="t-label-s c-accent">평가를 되돌렸어요 · 복습 일정도 원래대로예요. 다시 골라 주세요.</span>
                  </span>
                ) : recLine !== null ? (
                  <span className="study__tip study__tip--warn">
                    <Icon name="info" size={16} />
                    <span className="t-label-s">{recLine}</span>
                  </span>
                ) : suggest !== null ? (
                  <span className="study__tip study__tip--warn">
                    <Icon name="info" size={16} />
                    <span className="t-label-s">
                      {RATING_NAME[suggest]} 추천 ·{" "}
                      {missing.length
                        ? `핵심어 ${n(missing.length)}개가 빠졌어요 (${missing.slice(0, 3).join(", ")}${missing.length > 3 ? " 등" : ""}).`
                        : "핵심어를 모두 썼어요."}{" "}
                      추천은 따르지 않아도 돼요.
                    </span>
                  </span>
                ) : (
                  hints && (
                    <span className="study__tip">
                      <Icon name="info" size={16} className="c-accent" />
                      <span className="t-label-s c-accent">처음이라 평가 기준을 함께 보여드려요. 다음 학습부터는 숨겨요.</span>
                      <button
                        className="study__tip-x c-accent"
                        aria-label="평가 기준 숨기기"
                        onClick={() => {
                          setHints(false);
                          setPref(HINTS_SEEN, true);
                        }}
                      >
                        <Icon name="x" size={14} />
                      </button>
                    </span>
                  )
                )}
              </div>
              <div className="study__ratings" data-undone={undone ?? undefined} data-suggest={suggest ?? undefined}>
                {([1, 2, 3, 4] as const).map((r) => (
                  <RatingButton
                    key={r}
                    rating={r}
                    shortcut={rateKeys[r - 1]}
                    hint={
                      undone !== null
                        ? r === undone
                          ? "방금 고른 평가"
                          : undefined
                        : r === suggest
                          ? `추천 · ${both ? "핵심어 · ChatGPT" : gptSuggest !== null ? "ChatGPT" : HINT[r]}`
                          : both && (r === kwSuggest || r === gptSuggest)
                            ? `추천 · ${r === kwSuggest ? "핵심어" : "ChatGPT"}`
                            : hints
                            ? HINT[r]
                            : undefined
                    }
                    onClick={() => rate(r)}
                  />
                ))}
              </div>
              {!toast && <p className={`study__keys study__keys--a t-caption c-tertiary ${rated ? "is-pushed" : ""}`}>{keyLine(keysA)}</p>}
            </>
          )}
          {error && <p className="t-body-s c-danger">{error}</p>}
        </div>
      )}
      {!card && error && <p className="study__error t-body-m c-danger">{error}</p>}

      {toast ? (
        <div className="study__toast" role="status">
          <span className="study__toast-icon">
            <Icon name="archive" size={18} />
          </span>
          <span className="study__toast-text">
            <span className="t-body-m-strong">{toast.auto ? "방금 카드를 잠시 접어둘게요" : "방금 카드를 접어뒀어요"}</span>
            <span className="t-body-s c-tertiary">
              {toast.reason
                ? `두 번 막혔어요. ${toast.reason}, 나눠서 외우는 게 나을 수도 있어요.`
                : toast.auto
                  ? "같은 학습에서 두 번 기억나지 않았어요. 나머지를 먼저 끝내고 마지막에 다시 보여드릴게요."
                  : "학습을 마치면 다시 볼지 물어볼게요."}
            </span>
          </span>
          {/* G3: 학습을 멈추지 않는다. "나중에 볼 제안"에 넣고 토스트만 닫는다 (D5 접어둔 카드 행에 칩으로 다시 나온다) */}
          {toast.reason && (
            <button
              className="study__toast-btn study__toast-btn--accent t-label-m"
              onClick={() => {
                splitLater.add(toast.id);
                setToast(null);
              }}
            >
              <Icon name="layers" size={14} />
              나누기 제안
            </button>
          )}
          {toast.source && (
            <button className="study__toast-btn t-label-m" onClick={() => openSourceOf(toast.id)}>
              <Icon name="file" size={14} />
              원자료 보기
            </button>
          )}
          <button className="study__toast-btn t-label-m" onClick={() => unfold([toast.id], true)}>
            <Icon name="rotate" size={14} />
            지금 다시 보기
          </button>
          {/* 버튼이 늘면 되돌리기는 키(Ctrl Z)로만. 토스트가 화면 폭을 넘지 않게 */}
          {last?.id === toast.id && !toast.reason && !toast.source && (
            <button className="study__toast-btn t-label-m" onClick={undo}>
              되돌리기
              <Kbd k={actionKey("undo")} tone="onAccent" />
            </button>
          )}
        </div>
      ) : (
        rated && (
          <div className="study__undo" role="status">
            <span className={`dot dot--${RATING_DOT[rated.rating]}`} />
            <span className="t-label-m">{RATING_NAME[rated.rating]}</span>
            <span className="study__undo-q t-caption c-tertiary">{rated.question}</span>
            <button className="study__undo-btn t-label-m" onClick={undo}>
              <Icon name="rotate" size={14} />
              되돌리기
              <Kbd k={actionKey("undo")} tone="onAccent" />
            </button>
          </div>
        )
      )}

      {/* ---- I2 원자료 (통합 지점). 카드는 왼쪽으로 좁아지고 평가 줄이 카드 폭을 따라간다 */}
      {panel && card && (
        <div className="study__source">
          {srcSeg !== null ? (
            <SegmentPanel key={srcSeg} segmentId={srcSeg} onClose={() => setSrcSeg(null)} />
          ) : (
            <SourcePanel cardId={card.view.id} onClose={() => setSrcOpen(false)} />
          )}
        </div>
      )}
      {grade.popover}
      {grade.connectSheet}

      {zoom && (
        <div className="scrim study__zoom" role="dialog" aria-label="그림 크게 보기 · 누르거나 Esc 로 닫기" onMouseDown={() => setZoom(null)}>
          {"src" in zoom ? <img src={zoom.src} alt="" /> : <div className="study__zoom-io" dangerouslySetInnerHTML={{ __html: zoom.html }} />}
        </div>
      )}

      {editing && card && (
        <EditNote
          noteId={card.view.note_id}
          card={card.view}
          onClose={() => setEditing(false)}
          onSaved={async () => {
            setEditing(false);
            try {
              await show(card.view.id, phase);
            } catch {
              await advance(); // 이 카드의 빈칸을 지웠다
            }
          }}
          onDeleted={() => {
            // C2-d: 학습 중 지우면 큐에서 빠지고 다음 카드로
            const id = card.view.id;
            setEditing(false);
            setTotal((t) => t - 1);
            queue.current = queue.current.filter((x) => x !== id);
            next.current = next.current.filter((x) => x !== id);
            later.current = later.current.filter((l) => l.id !== id);
            setLast((l) => (l?.id === id ? null : l));
            advance();
          }}
        />
      )}
    </main>
  );
}

// ---------------------------------------------------------------- C3-e 카드 편집 (학습 중 E · 카드 목록)

type FieldRole = "front" | "rich" | "line";
/** 앞뒤 둘 다 (Basic and reversed): 필드 두 개가 모두 앞면 템플릿에 쓰인다 */
const isReversed = (note: NoteEdit) => note.kind === "basic" && note.front[0] && note.front[1];
const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/**
 * C3-e 배치 규칙. 짧은 값(한 줄 · 30자 이하 · 서식 없음) = 한 줄 입력칸, 앞면 템플릿에 쓰이는 필드 = 툴바 있는 편집기,
 * 나머지 = 툴바 없는 편집기. 질문(직접 만든 카드는 첫 칸, 가져온 노트는 앞면 필드 중 가장 긴 것)은 짧아도 편집기다.
 * 앞면의 유형 · 난이도 같은 짧은 꼬리표는 한 줄 입력칸으로 간다 (Figma C3-e). 앞뒤 둘 다는 두 칸 모두 질문이다 (C3-r)
 */
function fieldRoles(note: NoteEdit): FieldRole[] {
  if (isReversed(note)) return ["front", "front"];
  const len = note.values.map((v) => plainText(v).length);
  const main = note.kind !== "other" ? 0 : note.front.reduce((best, f, i) => (f && (best < 0 || len[i] > len[best]) ? i : best), -1);
  return note.values.map((v, i) =>
    i === main ? "front" : !/[<\n]|\{\{/.test(v) && len[i] <= 30 ? "line" : note.front[i] ? "front" : "rich",
  );
}

export function EditNote({ noteId, card, onClose, onSaved, onDeleted }: {
  noteId: number;
  /** 이 카드만의 중요도 · 하위 덱 이동 · 삭제 (아래 줄). 없으면 노트 내용만 고친다 */
  card?: Pick<CardView, "id" | "importance" | "deck_path" | "occlusion">;
  onClose: () => void;
  onSaved: () => void;
  /** 카드를 지운 뒤. 없으면 onSaved 를 부른다 */
  onDeleted?: () => void;
}) {
  const [note, setNote] = useState<(NoteEdit & { roles: FieldRole[] }) | null>(null);
  const [choices, setChoices] = useState<DeckChoice[] | null>(null);
  const [deckId, setDeckId] = useState<number>();
  const [level, setLevel] = useState<Level | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [rootId, setRootId] = useState<number>();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const fields = useRef<(FieldHandle | null)[]>([]);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);
  const root = card?.deck_path[0];
  // J3 서술형: 필수 키워드 · 제안 · 공부할 때 직접 써서 답하기
  const [wn, setWn] = useState<WrittenNote | null>(null);
  const [kws, setKws] = useState<Keyword[]>([]);
  const [dismissed, setDismissed] = useState<string[]>([]);
  const [write, setWrite] = useState(true);
  const [answerHtml, setAnswerHtml] = useState("");
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [exam, setExam] = useState<ExamState | null>(null); // N1 기출 줄 (저장할 때 쓴다)

  useEffect(() => {
    Promise.all([api.noteEdit(noteId), writtenApi.note(noteId).catch(() => null)]).then(([n, w]) => {
      setExam(n.exam);
      const roles = fieldRoles(n);
      if (w?.native) roles.splice(0, 3, "front", "rich", "rich"); // 질문 · 모범 답안 · 채점 메모
      else if (w?.capable && w.answer_field !== null) roles[w.answer_field] = "rich"; // 제안 칩을 뽑는 칸
      setNote({ ...n, roles });
      if (w?.capable) {
        setWn(w);
        setKws(w.keywords);
        setDismissed(w.dismissed);
        setWrite(w.write);
        setAnswerHtml(w.answer_field !== null ? (n.values[w.answer_field] ?? "") : "");
      }
    }, (e) => setError(errorText(e)));
  }, [noteId]);
  // 모범 답안이 바뀌면 제안을 다시 뽑는다. 이미 정한 키워드와 뺀 말은 빼고
  useEffect(() => {
    if (!wn) return;
    const t = setTimeout(
      () => writtenApi.suggest(answerHtml, [...kws.map((k) => k.word), ...dismissed]).then(setSuggestions, () => setSuggestions([])),
      250,
    );
    return () => clearTimeout(t);
  }, [wn, answerHtml, kws, dismissed]);
  // 열릴 때 첫 필드에 포커스 (짧은 값은 뒤로 모이므로 편집기가 먼저)
  useEffect(() => {
    if (note) (fields.current.find(Boolean) ?? inputs.current.find(Boolean))?.focus();
  }, [note]);

  // 옮길 곳: 이 카드의 최상위 덱과 그 아래 덱. CardView 에 덱 id 가 없어 최상위 덱을 이름으로 찾는다
  useEffect(() => {
    if (root !== undefined) rootDeckIdOf([root]).then(setRootId, (e) => setError(errorText(e)));
  }, [root]);

  const here = choices?.find((c) => c.path.join("\n") === card?.deck_path.slice(1).join("\n"));
  const target = choices?.find((c) => c.id === (deckId ?? here?.id));
  const shownLevel = level ?? card?.importance;
  const cloze = note?.kind === "cloze";
  const reversed = !!note && isReversed(note);
  const label = (i: number) =>
    wn?.native
      ? (["질문", "모범 답안", "채점 메모"][i] ?? note?.names[i] ?? "")
      : note?.kind === "basic"
      ? (reversed ? ["앞면", "뒷면"] : ["질문", "답변"])[i]
      : cloze && i === 0
        ? "본문"
        : cloze && i === 1 && note.names.length === 2
          ? "추가 설명"
          : (note?.names[i] ?? "");

  const save = async () => {
    if (!note || saving) return;
    // 새 빈칸은 원래 노트에 있던 가장 큰 번호 다음부터. 지운 번호를 다시 쓰면 그 카드의 학습 기록이 엉뚱한 빈칸에 붙는다
    const firstFree = Math.max(0, ...note.values.flatMap((v) => [...v.matchAll(/\{\{c(\d+)::/g)].map((m) => Number(m[1])))) + 1;
    const values = note.names.map((_, i) => {
      const input = inputs.current[i];
      // 한 줄 입력칸은 글자만 받는다. 고치지 않았으면 원래 HTML 을 그대로 둔다
      if (input) return input.value === plainText(note.values[i]) ? note.values[i] : escapeHtml(input.value);
      const html = fields.current[i]?.html() ?? "";
      return cloze ? toCloze(html, firstFree) : toStoredHtml(html);
    });
    setSaving(true);
    try {
      await api.updateNote(note.note_id, values);
      if (wn) await writtenApi.saveNote(note.note_id, write, kws, dismissed);
      if (exam && JSON.stringify(exam) !== JSON.stringify(note.exam)) await examApi.set("note", [exam]);
      if (card && target && target.id !== here?.id) await api.moveCard(card.id, target.id);
      // 옮길 덱의 기본값과 같으면 개별 지정을 풀어 덱 설정을 따르게 한다
      if (card && level !== null && level !== card.importance) await api.setCardImportance(card.id, level === target?.importance ? null : level);
      onSaved();
    } catch (e) {
      setError(errorText(e));
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!card || saving) return;
    setSaving(true);
    try {
      await api.deleteCard(card.id);
      (onDeleted ?? onSaved)();
    } catch (e) {
      setError(errorText(e));
      setConfirming(false);
      setSaving(false);
    }
  };
  useKey("Enter", remove, confirming, { modal: true });

  const front = note ? Math.max(0, note.roles.indexOf("front")) : 0;
  // M 그림 가리기: 카드 편집(E)은 가리기 편집기로 연다
  if (note?.kind === "image_occlusion") return <OcclusionEdit noteId={noteId} focus={card?.occlusion?.ordinal} onClose={onClose} onSaved={onSaved} />;
  const preview = note && [note.kind === "basic" ? plainText(note.values[1] ?? "") : "", here?.label ?? ""].filter(Boolean).join(" · ");

  return (
    <>
      {/* 확인 창이 떠 있으면 Esc 는 확인 창만 닫는다 (두 창의 Esc 중 어느 쪽이 먼저 받아도 같게). 하위 덱 드롭다운은 제 Esc 를 먼저 가로챈다 */}
      <Modal
        title="카드 편집"
        sub={`${
          wn?.native
            ? `서술형 카드 · ${card ? `${pathLabel(card.deck_path)} · ` : ""}`
            : note?.kind === "other"
              ? `Anki 노트타입 '${note.type_name}' · 필드 ${note.names.length}개 · `
              : reversed
                ? "앞뒤 둘 다 · 카드 2장 · "
                : ""
        }저장해도 복습 일정은 그대로예요`}
        className={`modal--edit ${wn ? "modal--written" : ""}`}
        onClose={() => (confirming ? setConfirming(false) : onClose())}
      >
        <div
          className="edit"
          onKeyDown={(e) => {
            if (confirming) return;
            if (matchAction("save", e.nativeEvent) >= 0) {
              e.preventDefault();
              save();
            } else if (cloze && e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "c") {
              e.preventDefault();
              makeCloze(fields.current[0]?.el());
            }
          }}
        >
          <div className="edit__fields">
            {note?.names.map((_, i) =>
              note.roles[i] === "line" ? (
                <label key={i} className="edit__item">
                  <span className="t-label-m">{label(i)}</span>
                  <input
                    ref={(el) => {
                      inputs.current[i] = el;
                    }}
                    className="field__input t-body-m"
                    defaultValue={plainText(note.values[i])}
                  />
                </label>
              ) : (
                <div key={i} className="edit__item edit__item--wide">
                  <span className="t-label-m">{label(i)}</span>
                  <RichField
                    ref={(h) => {
                      fields.current[i] = h;
                    }}
                    initial={cloze ? fromCloze(note.values[i]) : note.values[i]}
                    minHeight={44}
                    toolbar={note.roles[i] === "front"}
                    onInput={() => i === wn?.answer_field && setAnswerHtml(fields.current[i]?.html() ?? "")}
                    extraTool={
                      cloze && i === 0 ? (
                        <Button
                          variant="secondary"
                          size="M"
                          label="빈칸 만들기"
                          shortcut="Ctrl Shift C"
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => makeCloze(fields.current[0]?.el())}
                        />
                      ) : undefined
                    }
                  />
                </div>
              ),
            )}
            {wn && (
              <div className="edit__item edit__item--wide edit__written">
                <KeywordEditor
                  keywords={kws}
                  suggestions={suggestions}
                  onChange={setKws}
                  onDismiss={(w) => setDismissed((d) => (d.includes(w) ? d : [...d, w]))}
                />
              </div>
            )}
          </div>
          <div className="edit__rule" />
          {note && exam && <ExamField deckId={rootId ?? null} value={exam} onChange={setExam} hint={examHint(plainText(note.values.join(" ")))} />}
          {wn && (
            <label className="edit__write">
              <Toggle on={write} onChange={setWrite} label="공부할 때 직접 써서 답하기" />
              <span className="t-body-s c-secondary">공부할 때 직접 써서 답하기</span>
            </label>
          )}
          {card && (
            <>
              <div className="edit__card">
                <div className="edit__importance">
                  <span className="edit__importance-label">
                    <span className="t-label-m">중요도</span>
                    <span className="t-caption c-tertiary">
                      {target ? `하위 덱 기본값 ${LEVEL_LABEL[target.importance]} · ` : ""}
                      {/* 중요도는 노트 단위라 앞뒤 둘 다는 두 장이 같이 바뀐다 (C3-r) */}
                      {reversed ? "앞뒤 두 장을 같이 바꿔요" : "이 카드만 바꿔요"}
                    </span>
                  </span>
                  <div className="segment segment--boxed">
                    {([2, 1, 0] as const).map((lvl) => (
                      <button
                        key={lvl}
                        className={`segment__item t-label-m ${shownLevel === lvl ? "is-selected" : ""}`}
                        onClick={() => setLevel(lvl)}
                      >
                        {LEVEL_LABEL[lvl]}
                      </button>
                    ))}
                  </div>
                </div>
                {rootId !== undefined && (
                  <DeckPicker rootDeckId={rootId} value={target?.id} onChange={(d) => setDeckId(d.id)} onLoad={setChoices} relative />
                )}
              </div>
            </>
          )}
          <div className="edit__actions">
            {card && <Button variant="ghost" size="M" icon="trash" label="카드 삭제" className="edit__delete" onClick={() => setConfirming(true)} />}
            <span className="edit__msg t-body-s c-danger">{error}</span>
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
            <Button size="M" icon="check" label="저장" shortcut={actionKey("save")} onClick={save} disabled={!note} busy={saving} busyLabel="저장 중…" />
          </div>
        </div>
      </Modal>

      {confirming && note && (
        <Modal title="이 카드를 지울까요?" onClose={() => setConfirming(false)} variant="confirm" icon="trash">
          <div className="edit__preview">
            <p className="t-body-m">{plainText(cloze ? fromCloze(note.values[front]) : note.values[front])}</p>
            {preview && <p className="t-caption c-tertiary">{preview}</p>}
          </div>
          <p className="t-body-s c-secondary">되돌릴 수 없어요. 이 카드의 복습 기록은 학습 통계에만 남아요.</p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setConfirming(false)} />
            <Button variant="danger" size="M" icon="trash" label="삭제" shortcut="Enter" onClick={remove} busy={saving} />
          </div>
        </Modal>
      )}
    </>
  );
}
