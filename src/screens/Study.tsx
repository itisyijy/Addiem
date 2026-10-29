// D1 학습 시작 전 요약 / D2 질문 / D3 답변과 평가 / D4 접어두기 / C3-e 카드 편집
// J1~J3 서술형 · H3 압축 회독 · G3 학습 중 신호 · I2 원자료 패널 · Q1/Q2 목록 회상 · 선택형
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { OcclusionEdit } from "./Occlusion";
import { api, errorText, type CardView, type ExamState, type Level, type NoteEdit, type QueueSummary } from "../api";
import { Badge, Button, Checkbox, Icon, Importance, Kbd, LEVEL_LABEL, RatingButton, Toggle, type Rating } from "../ui";
import { eul, n, ro, useKey, useNav, useOverlayScope, type Route } from "../nav";
import { actionKey, getKeys, matchAction, useKeymap } from "../keys";
import { useToast } from "../toast";
import { plainText, playSounds, renderCardHtml, stopSounds, toStoredHtml } from "../cardHtml";
import { findMath, withMath } from "../math";
import {
  cram as cramApi, exam as examApi, splitLater, takeSignal, written as writtenApi, type Compare, type Keyword, type MixRun, type MixStats, type WrittenCard, type WrittenNote,
} from "../written";
import { Modal, ModalCancel } from "./NewDeck";
import { DeckPicker, rootDeckIdOf, type DeckChoice } from "./DeckPicker";
import { RichField, makeCloze, toCloze, fromCloze, tagRefs, TYPE_LABEL, type FieldHandle } from "./AddCard";
import { CramHint, KeywordEditor } from "./StudyParts";
import { SegmentPanel, SourcePanel } from "./SourcePanel";
import { useGrade } from "./GradeAssist";
import { useConcept } from "./ConceptCheck";
import { ExamField, examHint } from "./Exam";
import { cardSource } from "../sourceApi";
import { ChoiceEditor, lastOn, ListEditor, RecallFace, recallTip } from "./Recall";
import { choiceDraft, choiceLabel, listLabel, optionOrder, recall, type ChoiceDraft, type ListDraft } from "../recall";
import { deckFace, scopeCss } from "../cardCss";
import "../styles/study.css";

/*
 * I2 원자료 패널은 가져오기 쪽이 만든다 (SourcePanel.tsx · sourceApi.ts · source.rs). 패널은 420px 겉모양까지 스스로 그리고,
 * 이 화면은 자리만 잡는다. 카드를 보일 때 새로 물어 둔 값을 패널이 그대로 쓴다 (cardSource 캐시)
 */
const hasSource = (cardId: number) => cardSource(cardId, true).then((s) => s !== null);

// 이 기기에서만 쓰는 편의 설정. 지워져도 기본 동작으로 돌아갈 뿐이다
export const SKIP_SUMMARY = "neo.skipSummary";
const HINTS_SEEN = "neo.ratingHintsSeen";
/** AU4 소리 자동 재생 끔 (기본은 켬) · AU1 처음 본 소리 카드 안내 */
export const NO_AUTOPLAY = "neo.soundAutoplayOff";
const SOUND_TIP_SEEN = "neo.soundTipSeen";
/** AU21 읽어주기 자동 재생 켬 (기본은 끔) · 처음 본 읽어주기 카드 안내 */
export const TTS_AUTOPLAY = "neo.ttsAutoplay";
const TTS_TIP_SEEN = "neo.ttsTipSeen";
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

  // 오늘 이미 평가해 다시 볼 카드는 복습이라 부르지 않는다 (홈 · 덱 상세 · 끝 화면과 같은 말, QA V9-04)
  const reviewCount = q.learning + q.review + q.backlog - q.again;
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
    `예정 ${n(Math.max(0, q.learning + q.review - q.again))}`,
    q.backlog > 0 && `밀린 ${n(q.backlog)}${q.backlog_deferred > 0 ? ` (나머지 ${n(q.backlog_deferred)}장은 차차)` : ""}`,
    "오늘 보지 않으면 일정이 밀려요",
    // C1-w: 복습 묶음 안에서 약한 챕터의 예정 복습을 먼저 (큐가 이미 그 순서로 준다)
    q.weak_first > 0 && `취약 챕터 먼저 · ${n(q.weak_first)}장`,
  ]
    .filter(Boolean)
    .join(" · ");
  const rows = [
    { key: "again", dot: "ink", title: "다시 볼 카드", desc: "오늘 평가한 카드를 짧은 간격을 두고 한 번 더", count: q.again },
    { key: "buried", dot: "warm", title: "접어둔 카드 다시 보기", desc: "접어둔 뒤 3일이 지나 돌아온 카드 · 집중력이 좋을 때 먼저", count: q.returning },
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
                {/* 남은 신규가 권장 이하면 [더 많이]도 같은 수라 숨긴다 */}
                {SCALES.map((s, i) => i === 2 && newOf(plans[2]) <= newOf(plans[1]) ? null : (
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
                오늘 총 {n(q.total)}장 · 복습 {n(q.total - newOf(q) - q.again)}
                {q.again > 0 && ` + 다시 볼 ${n(q.again)}`} + 신규 {n(newOf(q))}
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
/** 평가 · 접기 직전의 카드 면: Q 연 칸 · 표시(빠뜨림 · 고른 보기) · 보기 순서, J1 쓴 답 · J2 비교. 되돌리면 그대로 돌아온다 (섞기는 볼 때마다 새로 해서 다시 만들 수 없다) */
type Face = { opened: number; marks: number[]; order: number[]; typed: string; compare: Compare | null };
/** 되돌릴 수 있는 일. D2-u: 이 세션의 마지막 평가나 접기 하나뿐 (백엔드는 더 쌓지만 화면에서 막는다). log: H3 압축 회독 기록 id */
type Last = { id: number; rating: Rating | null; phase: "q" | "a"; counted: boolean; folded: boolean; log?: number; face: Face };
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

/** AU J2 비교 화면은 정답 HTML 대신 비교 글자라, 정답의 소리 칩만 뽑아 글자 뒤에 둔다 */
function soundChips(html: string) {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  return Array.from(tpl.content.querySelectorAll("button.snd"), (c) => c.outerHTML).join("");
}

const daysAgo = (at: number) => {
  const d = Math.floor((Date.now() - at) / 86_400_000);
  return d < 1 ? "오늘" : `${d}일 전`;
};

/**
 * 뒷면을 질문 부분과 답 부분으로 나눈다. Basic 템플릿은 <hr id=answer> 로 나뉜다.
 * 구분선 없이 {{FrontSide}} 바로 뒤에 답을 붙인 템플릿은 앞면이 그대로 앞에 있으므로 거기서 나눈다
 */
export function splitBack(back: string, front: string) {
  const m = back.match(/<hr[^>]*\bid=["']?answer\b["']?[^>]*>/i);
  if (m && m.index !== undefined) return { question: back.slice(0, m.index), answer: back.slice(m.index + m[0].length) };
  const f = front.trim();
  const b = back.trimStart();
  return f && b.startsWith(f) ? { question: f, answer: b.slice(f.length) } : { question: "", answer: back };
}


/** 토스트 · 목록에 쓰는 한 줄 질문. 붙어 있는 태그 사이에 띄어쓰기를 넣는다 ("OX높음" 이 되지 않게) */
/** 수식은 구분자만 벗겨 TeX 원문으로 ("\(z\)" → "z"). 한 줄 글에서 그린 수식은 읽기 어렵다 */
/** 목록 · 선택형은 앞면 템플릿이 순서 · 보기까지 붙여 질문 필드만 (D5 와 같다, recall.rs Special::question) */
const lineOf = (v: CardView) => {
  const text = plainText((v.special?.question ?? v.front).replace(/></g, "> <"));
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

export function Study(props: { deckIds: number[] | null; scale: number; cardIds?: number[]; sessionId?: number; cram?: boolean; mix?: MixRun; edit?: number }) {
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
  const mixReplayed = useRef(false); // 접어둔 카드는 회독(혼합 · 압축) 끝에 한 번 더
  const mixDue = useRef(new Set<number>()); // P3 "오늘 예정이던 N장": 자리(전체 · 취약)와 상관없이 한 번씩, 오늘의 학습에서 빠진 카드만 (QA T3-04 · V6-03)
  const openedAt = useRef(Date.now()); // P3 "N장 회독 · M분": 이 화면에서 쓴 시간 (같은 날 이어 쓴 세션의 쉬던 시간은 빼고)
  const foldedIds = useRef<number[]>([]);
  const [typed, setTyped] = useState(""); // J1 내 답
  // Q 목록 회상 · 선택형: 연 칸 수 · 표시(목록은 빠뜨린 항목, 선택형은 고른 보기. 1부터) · 보기 순서(필드 번호) · 평가 추천
  const [opened, setOpened] = useState(0);
  const [marks, setMarks] = useState<number[]>([]);
  const [order, setOrder] = useState<number[]>([]);
  const [lcSuggest, setLcSuggest] = useState<Rating | null>(null);
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
  // 진행 "N / 전체": 이 화면에서 한 번이라도 평가하거나 접은 카드 수. 학습 단계로 다시 나오는 카드는 한 번만 센다 (QA T2-01)
  const seen = useRef(new Set<number>());
  const [done, setDone] = useState(0);
  /** 처음 본 카드면 세고 true */
  const see = (id: number) => {
    if (seen.current.has(id)) return false;
    seen.current.add(id);
    setDone(seen.current.size);
    return true;
  };
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
  // NT13 노트 유형 편집기에서 돌아왔다: 그 카드가 먼저 나오면 카드 편집을 다시 연다 (바뀐 배치가 바로 보이게)
  const reopen = useRef(props.edit);
  useEffect(() => {
    if (!card || reopen.current === undefined) return;
    if (card.view.id === reopen.current) setEditing(true);
    reopen.current = undefined;
  }, [card]);
  const [hints, setHints] = useState(() => !pref(HINTS_SEEN));
  const [soundTip, setSoundTip] = useState<{ id: number; tts: boolean } | null>(null); // AU1 · AU21 이 카드에서 한 번 안내
  const [error, setError] = useState<string>();
  // R2 채점 보조: 학습 세션마다 한 번 확인 · 근거 칩으로 연 구간 (I2 패널 자리)
  const gradeOk = useRef(false);
  const [srcSeg, setSrcSeg] = useState<number | null>(null);
  const grade = useGrade({ cardId: card?.view.id ?? null, typed, on: phase === "a" && !!compare, confirmed: gradeOk, openSegment: setSrcSeg });
  // 8.3 개념 관계 확인 (로컬 모델을 받았고 이 PC 에서 쓸 수 있을 때 비교 화면에서 저절로, 참고용)
  const concept = useConcept({ cardId: card?.view.id ?? null, typed, on: phase === "a" && !!compare, writing: phase === "q" && !!card?.written?.written, toastBusy: !!rated || !!toast });

  const ended = useRef(false); // 세션을 닫았다 (Esc 두 번에 두 번 닫지 않게 · 떠날 때 닫을지)
  const finish = useCallback(
    async (completed: boolean) => {
      if (ended.current) return;
      ended.current = true;
      // H3: 세션이 없다 (오늘의 학습과 따로). 몇 장 훑었는지만 알리고 홈으로
      if (cram) {
        notify({ title: `압축 회독 ${n(cramDone.current.size)}장을 훑었어요`, text: "복습 일정과 오늘의 학습은 그대로예요." });
        return nav({ name: "home" });
      }
      if (session.current === null) return nav({ name: "home" });
      setPref(HINTS_SEEN, true); // "다음 학습부터는 숨겨요"
      let summary;
      try {
        summary = await api.finishSession(session.current, completed, deckIds, scale);
      } catch (e) {
        ended.current = false;
        return setError(errorText(e));
      }
      if (mix) {
        const minutes = Math.max(summary.studied > 0 ? 1 : 0, Math.round((Date.now() - openedAt.current) / 60_000));
        return nav({ name: "mixDone", sessionId: session.current, run: mix, stats: mixStats.current, minutes, completed });
      }
      nav({ name: "done", sessionId: session.current, summary, completed, deckIds, scale });
    },
    [nav, deckIds, scale, cram, notify, mix],
  );
  // 그만하기 없이 화면을 떠나면(연결 자료 열기 · 노트 유형 편집 등) 그만둔 것으로 닫는다. 같은 날 다시 시작하면 이어 쓴다 (QA T2-04 · T6-13)
  useEffect(
    () => () => {
      if (!ended.current && session.current !== null) api.finishSession(session.current, false, deckIds, scale).catch(() => {});
    },
    // 처음 값으로 충분하다 (화면이 사는 동안 바뀌지 않는다)
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const show = useCallback(async (id: number, at: "q" | "a" = "q", face?: Face) => {
    // J 서술형 정보 · I2 자료 연결은 없어도 카드는 보인다
    const [view, written, source] = await Promise.all([api.cardView(id), writtenApi.card(id).catch(() => null), hasSource(id)]);
    // Q 목록 · 선택형은 템플릿 대신 필드로 그린다 (질문 · 추가 설명만 HTML)
    const sp = view.special;
    const parts = sp ? { question: sp.question, answer: sp.extra } : splitBack(view.back, view.front);
    // 살균(renderCardHtml) 뒤에 수식을 그린다 (math.ts)
    const [front, question, answer] = (await Promise.all([
      renderCardHtml(view.front),
      renderCardHtml(parts.question),
      renderCardHtml(parts.answer),
    ])).map(withMath);
    setCard({ view, front, question, answer, written, source });
    const html = view.front + view.back;
    if (!pref(TTS_TIP_SEEN) && !pref(TTS_AUTOPLAY) && html.includes("data-tts=")) {
      setPref(TTS_TIP_SEEN, true);
      setSoundTip({ id, tts: true });
    } else if (!pref(SOUND_TIP_SEEN) && !pref(NO_AUTOPLAY) && html.includes("data-snd=")) {
      setPref(SOUND_TIP_SEEN, true);
      setSoundTip({ id, tts: false });
    }
    setOpened(face?.opened ?? 0);
    setMarks(face?.marks ?? []);
    setOrder(face?.order ?? (sp?.kind === "choice" ? optionOrder(sp) : []));
    setPhase(at);
    setUndone(null);
    setTyped(face?.typed ?? "");
    setCompare(face?.compare ?? null);
    setSrcSeg(null);
    shownAt.current = Date.now();
  }, []);

  /** 다음 카드. 바로 다음으로 끼운 카드, 다시 볼 시각이 된 카드, 원래 순서, 마지막으로 남은 다시 볼 카드 */
  const advance = useCallback(async () => {
    for (;;) {
      const now = Date.now();
      const l = later.current.sort((a, b) => a.due - b.due);
      const id = next.current.shift() ?? (l.length && l[0].due <= now ? l.shift()!.id : (queue.current.shift() ?? l.shift()?.id));
      if (id === undefined && (mix || cram) && !mixReplayed.current && foldedIds.current.length > 0) {
        // P2 혼합 · H3 압축 회독: 접어둔 카드는 묶음 끝에 한 번 더 (두 번째 접히면 그대로 둔다, P14 · P20)
        mixReplayed.current = true;
        const ids = foldedIds.current;
        for (const f of ids) {
          if (cram) cramAgain.current.set(f, 0); // 압축은 접기가 이 화면 안에만 있다
          else await api.unburyCard(session.current!, f);
        }
        setFolded([]);
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
  }, [finish, show, mix, cram]);
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
    const box = el?.parentElement;
    if (!el || !box) return;
    // 넘침은 넘침 모양(is-overflow: 아래 48px 비움) 없이 잰다. 앞 카드의 모양이 남은 채로 재면 들어가는 카드도 넘친다고 본다.
    // 모양은 바로 되돌리고, 모양을 빼는 동안 줄어든 스크롤 위치도 되돌린다
    const had = box.classList.contains("is-overflow");
    const top = el.scrollTop;
    box.classList.remove("is-overflow");
    const overflow = el.scrollHeight > el.clientHeight + 1;
    if (had) {
      box.classList.add("is-overflow");
      el.scrollTop = top;
    }
    const more = overflow && el.scrollHeight - el.scrollTop - el.clientHeight > 1;
    setFit((f) => (f.overflow === overflow && f.more === more ? f : { overflow, more }));
  }, []);
  // 스크롤로는 넘침이 바뀌지 않는다. "아래에 더 있어요"만 다시 잰다
  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    const left = el.scrollHeight - el.scrollTop - el.clientHeight > 1;
    setFit((f) => {
      const more = f.overflow && left;
      return f.more === more ? f : { ...f, more };
    });
  };
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
  const css = card?.view.special ? "" : (card?.view.css ?? ""); // Q 목록 · 선택형은 템플릿을 쓰지 않는다
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
            answer: async (id: number, r: Rating, ms: number, marks: number[] | null): Promise<AnswerAction> => {
              const log = await cramApi.answer(id, r, ms, marks);
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
            answer: (id: number, r: Rating, ms: number, marks: number[] | null): Promise<AnswerAction> => api.answer(session.current!, id, r, ms, marks),
            bury: (id: number) => api.buryCard(session.current!, id),
            unbury: (id: number) => api.unburyCard(session.current!, id),
            undo: (_: Last) => api.undoLast(session.current!),
          },
    [cram],
  );

  const faceNow: Face = { opened, marks, order, typed, compare }; // 되돌리기가 돌려놓을 지금 면

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
    // 바로 이어 부르는 advance 가 묶음 끝 다시 보기에 쓴다. 상태(folded)는 다음 그리기에서야 바뀐다 (마지막 카드가 접힐 때)
    foldedIds.current = [...foldedIds.current, c.view.id];
    setFolded((list) => [
      ...list,
      { id: c.view.id, question: lineOf(c.view), importance: c.view.importance, deck_path: c.view.deck_path, auto, at: Date.now() },
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
  const rate = (r: Rating) => (grade.stop(), concept.stop(), rate2(r));
  const rate2 = (r: Rating) =>
    guard(async () => {
      const c = card!;
      // Q9 빠뜨림 · 고른 보기는 평가가 남을 때 같이 저장된다 (이 기기에만. 되돌리면 그 전 값으로)
      const res = await backend.answer(c.view.id, r, Date.now() - shownAt.current, c.view.special ? marks : null);
      if (res.action === "day_rollover") {
        ended.current = true; // 백엔드가 세션을 닫았다
        // 큐 명세 8장: 학습 중 하루 경계를 넘었다. 세션은 닫혔고 방금 평가는 남지 않았다. 새 날의 학습을 다시 짠다
        notify({ title: "새 학습일이 시작됐어요", text: "오늘의 학습을 새로 만들었어요. 방금 평가는 남기지 않았어요." });
        return nav(mix ? { name: "home" } : { name: "summary", deckIds });
      }
      if (mix) {
        const role = mixRoles.current.get(c.view.id)?.shift();
        const st = mixStats.current;
        if (role !== undefined) {
          const k = role === null ? "full" : "weak";
          st[k][0] += 1;
          if (r === 1) st.again[role === null ? 0 : 1] += 1;
        }
        // Again 으로 오늘 다시 볼 카드는 오늘의 학습에 남는다. 거기서 빠진 카드만 센다 (QA V6-03)
        if (mix.dueToday.includes(c.view.id)) {
          if (res.action === "again_later") mixDue.current.delete(c.view.id);
          else mixDue.current.add(c.view.id);
          st.due = mixDue.current.size;
        }
      }
      const counted = see(c.view.id);
      if (res.action === "again_later") later.current.push({ id: c.view.id, due: Date.now() + (res.due_in_ms ?? 60_000) });
      setLast({ id: c.view.id, rating: r, phase: "a", counted, folded: res.action === "buried", log: res.log, face: faceNow });
      if (res.action === "buried") fold(c, true);
      else {
        setToast(null); // 토스트 자리는 하나. 연달아 평가하면 내용만 바뀐다
        setRated({ rating: r, question: lineOf(c.view) });
      }
      await advance();
    });

  const bury = () =>
    guard(async () => {
      const c = card!;
      await backend.bury(c.view.id);
      setLast({ id: c.view.id, rating: null, phase, counted: see(c.view.id), folded: true, face: faceNow });
      fold(c, false);
      await advance();
    });

  /** 접어둔 카드를 다시 꺼낸다. now: 지금 바로 (D4 토스트, 보던 카드는 바로 다음으로), 아니면 바로 다음 카드로 (D4-m) */
  const unfold = (ids: number[], now = false) =>
    guard(async () => {
      for (const id of ids) await backend.unbury(id);
      setFolded((list) => list.filter((f) => !ids.includes(f.id)));
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
        if (u.counted) {
          seen.current.delete(id);
          setDone(seen.current.size);
        }
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
        await show(id, u.phase, id === u.id ? u.face : undefined);
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
  const unsaved = useRef<{ id: number; text: string } | null>(null);
  const flushAnswer = useCallback(() => {
    const u = unsaved.current;
    unsaved.current = null;
    if (u) writtenApi.saveAnswer(u.id, u.text).catch(() => {});
  }, []);
  useEffect(() => {
    if (!writing || !card) return;
    unsaved.current = typed.trim() ? { id: card.view.id, text: typed } : null;
    if (!unsaved.current) return;
    const t = setTimeout(flushAnswer, 600);
    return () => clearTimeout(t);
  }, [typed, writing, card, flushAnswer]);
  // 기다리는 동안 카드가 바뀌거나 화면을 떠나면(Esc 두 번 등) 바로 저장한다 (QA T2-10)
  useEffect(() => flushAnswer, [card, flushAnswer]);
  /** 답 보기. 쓴 답이 있으면 J2 비교 (핵심어 판정 · 평가 추천) */
  const reveal = () => {
    if (!card || phase !== "q") return;
    // Q1-a 한 칸씩 연다. 다 열면 평가 단계 (Q1-b). Q2 는 고른 뒤 확인 = 답 보기
    const sp = card.view.special;
    if (sp?.kind === "list") {
      setOpened(opened + 1);
      if (opened + 1 >= sp.items.length) setPhase("a");
      return;
    }
    if (sp) return setPhase("a");
    if (!card.written?.written || !typed.trim()) return setPhase("a");
    return guard(async () => {
      setCompare(await writtenApi.compare(card.view.id, typed));
      setPhase("a");
    });
  };

  // ---- Q 목록 회상 · 선택형
  const special = card?.view.special ?? null;
  const toggleMiss = (num: number) => setMarks((m) => (m.includes(num) ? m.filter((x) => x !== num) : [...m, num]));
  /** 칸 누르기: 연 칸은 빠뜨림을 바꾸고, 다음 칸("?")은 연다 */
  const onSlot = (num: number) => (phase === "q" && num > opened ? reveal() : toggleMiss(num));
  /** 정답이 여럿이면 여러 개, 하나면 하나만 */
  const pick = (num: number) =>
    setMarks((m) => (special?.kind === "choice" && special.correct.length > 1 ? (m.includes(num) ? m.filter((x) => x !== num) : [...m, num]) : m[0] === num ? [] : [num]));
  const openAll = () => {
    if (special?.kind !== "list") return;
    setOpened(special.items.length);
    setPhase("a");
  };
  // 빠뜨림을 고치거나 보기를 고르면 추천이 바로 바뀐다
  useEffect(() => {
    if (!card?.view.special || phase !== "a") return setLcSuggest(null);
    let live = true;
    recall.suggest(card.view.id, marks).then((r) => live && setLcSuggest(r), () => live && setLcSuggest(null));
    return () => {
      live = false;
    };
  }, [card, phase, marks]);

  // ---- I2 원자료 패널: 답 화면에서만 (질문 화면에서 열면 답이 보일 수 있다)
  const panel = (srcOpen && phase === "a" && !!card?.source) || (srcSeg !== null && phase === "a");
  const openSourceOf = (id: number) =>
    unfold([id], true).then(() => {
      setPhase("a");
      setSrcOpen(true);
    });

  /*
   * AU2 자동 재생은 Anki 와 같게: 질문은 카드를 열 때, 답을 열면 뒷면에만 있는 소리(정답 부분)를 차례로.
   * Anki 도 {{FrontSide}} 로 들어온 앞면 소리는 답 면에서 다시 틀지 않는다. AU3 R 은 지금 면 (답 면은 앞면 소리부터).
   * AU21 · AU22 읽어주기도 같은 때 나온 순서대로 섞는다. 자동 재생은 소리(기본 켬)와 읽어주기(기본 끔)를 따로 따르고, R 은 둘 다
   */
  const playSide = useCallback(
    (auto: boolean) => {
      const el = scroller.current;
      if (!el) return;
      const chips = Array.from(el.querySelectorAll(phase === "q" ? ".study__q .snd" : auto ? ".study__a-body .snd, .cmp__snd .snd" : ".study__a .snd"));
      playSounds(auto ? chips.filter((c) => (c.hasAttribute("data-tts") ? pref(TTS_AUTOPLAY) : !pref(NO_AUTOPLAY))) : chips);
    },
    [phase],
  );
  useEffect(() => {
    stopSounds(); // 다음 카드로 넘어가면 멈춘다
    if (card) playSide(true);
  }, [card, playSide]);
  useEffect(() => stopSounds, []);

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
  useKey({ action: "replay" }, () => playSide(false), keys);
  useKey("b", () => (setFoldSel(0), setFoldMenu(true)), keys && folded.length > 0);
  useKey(["ArrowDown", "ArrowUp", "PageDown", "PageUp"], scrollCard, keys);
  // Q2 · Q6: X 방금 연 항목 빠뜨림, Enter 남은 칸 모두 열기, 1–6 보기 고르기 (질문 면에서만. 답 면의 1–4 는 평가)
  useKey("x", () => toggleMiss(phase === "a" ? opened || (special?.kind === "list" ? special.items.length : 0) : opened), keys && special?.kind === "list" && (phase === "a" || opened > 0));
  useKey("Enter", openAll, keys && phase === "q" && special?.kind === "list");
  useKey(["1", "2", "3", "4", "5", "6"], (_, i) => order[i] !== undefined && pick(order[i]), keys && phase === "q" && special?.kind === "choice");
  // D4-m 목록 안의 키는 목록이 받는다 (B 로 닫기 · ↑↓ · Enter · Esc)
  useKey(["ArrowDown", "ArrowUp"], (e) => setFoldSel((i) => Math.max(0, Math.min(folded.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))), foldMenu, { modal: true });
  useKey("Enter", () => folded[foldSel] && unfold([folded[foldSel].id]), foldMenu, { modal: true });
  useKey(["Escape", "b"], () => setFoldMenu(false), foldMenu, { modal: true });
  useKey("Escape", () => setZoom(null), !!zoom, { modal: true });
  useKey("Escape", () => finish(false), !editing);

  // 답을 열면 정답 영역으로 초점을 옮겨 화면 읽기 프로그램이 읽게 한다 (단축키는 버튼이 아니라 그대로 동작)
  const answerBox = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const box = answerBox.current;
    if (phase !== "a" || !box) return;
    box.focus({ preventScroll: true });
    // Q2: 낮은 창(640)에서는 보기가 두어 개만 보여 고른 오답이 아래에 숨는다. 정답, 지난번 고른 오답(Q36), 이번에 고른 보기(여럿이면 첫째) 차례로
    // 카드 안에서 보이게 내린다. 다 안 들어가면 뒤의 것(이번에 고른 보기)이 보인다. 넘침 표시(is-overflow)로 스크롤 칸 높이가 바뀐 뒤에 잰다
    if (special?.kind !== "choice") return;
    const opts = box.querySelectorAll(".opt");
    const at = (nums: number[]) => opts[order.findIndex((x) => nums.includes(x))];
    const f = requestAnimationFrame(() => {
      at(special.correct)?.scrollIntoView({ block: "nearest" });
      at(lastOn(special, card?.view.last_marks ?? null))?.scrollIntoView({ block: "nearest" });
      at(marks)?.scrollIntoView({ block: "nearest" });
    });
    return () => cancelAnimationFrame(f);
  }, [phase, card]);

  const position = Math.min(done + 1, total);
  const remaining = Math.max(0, total - position);
  // J2 평가 추천 (되돌린 직후에는 되돌리기 안내가 먼저). R2: 핵심어 검사와 ChatGPT 추천을 한 줄로 합친다. 다르면 테두리 없이 둘 다 알린다
  const kwSuggest = undone === null ? (compare?.suggest ?? null) : null;
  const gptSuggest = undone === null ? grade.rating : null;
  const suggest = special
    ? undone === null
      ? lcSuggest
      : null
    : kwSuggest !== null && gptSuggest !== null
      ? kwSuggest === gptSuggest
        ? kwSuggest
        : null
      : (kwSuggest ?? gptSuggest);
  const missing = compare?.keywords.filter((k) => !k.hit).map((k) => k.word) ?? [];
  const both = kwSuggest !== null && gptSuggest !== null;
  const recLine = both
    ? kwSuggest === gptSuggest
      ? `${RATING_NAME[kwSuggest]} 추천 · 핵심어 검사 ${missing.length ? `${n(missing.length)}/${n(compare!.keywords.length)} 빠짐` : "모두 씀"} · ChatGPT 검토도 ${RATING_NAME[gptSuggest]} · 직접 평가해 주세요`
      : `핵심어 검사는 ${RATING_NAME[kwSuggest]} · ChatGPT 검토는 ${RATING_NAME[gptSuggest]} · 직접 평가해 주세요`
    : gptSuggest !== null
      ? `${RATING_NAME[gptSuggest]} 추천 · ChatGPT 검토 · 직접 평가해 주세요`
      : null;
  const tip = hints || undone !== null || suggest !== null || recLine !== null || (!!card && soundTip?.id === card.view.id);
  // D2-u 되돌리기 토스트 자리. Q1 목록은 카드가 길어 [다음 항목] · 평가 줄이 토스트 자리(아래에서 70)까지 내려온다 (창 높이와 상관없이).
  // 목록에서는 진행 막대 아래에 띄우고 단축키 줄은 비키지 않는다
  const undoTop = special?.kind === "list";
  const pushKeys = rated && !undoTop ? "is-pushed" : "";
  // AU21 읽어주기 첫 안내는 칩이 처음 나온 면에 (질문 면에 칩이 있으면 질문 면부터, 답 면에서도 이어서)
  const qTip = phase === "q" && !special && !writing && !!card && !!soundTip?.tts && soundTip.id === card.view.id && card.front.includes("data-tts");
  const ttsTip = (
    <span className="study__tip">
      <Icon name="speech" size={16} className="c-accent" />
      <span className="t-label-s c-accent">
        읽어주기는 눌러야 읽어요 (자동 재생 꺼짐) · {actionKey("replay")}
        {ro(actionKey("replay"))} 들어요 · 설정에서 켤 수 있어요
      </span>
      <button className="study__tip-x c-accent" aria-label="읽어주기 안내 숨기기" onClick={() => setSoundTip(null)}>
        <Icon name="x" size={14} />
      </button>
    </span>
  );
  const rateKeys = getKeys("rate").map((_, i) => actionKey("rate", i));
  const replayHint = (html = "") => html.includes('class="snd') && `${actionKey("replay")} 다시 듣기`;
  const keysQ = special
    ? special.kind === "list"
      ? [`${actionKey("reveal")} 다음 항목`, "X 방금 연 항목 기억 못함", "Enter 남은 항목 모두 열기", "Esc 그만하기"]
      : [`1–${n(order.length)} 고르기`, `${actionKey("reveal")} 확인`, `${actionKey("edit")} 편집`, `${actionKey("bury")} 접어두기`, "Esc 그만하기"]
    : writing
    ? [`${actionKey("save")} 비교하기`, `Esc 입력 칸 나가기 (그다음 ${actionKey("reveal")}로 바로 답 보기)`]
    : [`${actionKey("reveal")} 답 보기`, replayHint(card?.front), mix && "평가가 복습 일정에 반영돼요", fit.overflow && "↑ ↓ 카드 안 스크롤", `${actionKey("edit")} 편집`, `${actionKey("bury")} 접어두기`, last && `${actionKey("undo")} 되돌리기`, "Esc 그만하기"];
  const keysA = [
    `${rateKeys.join(" · ")} 로 평가`,
    replayHint(card?.view.back),
    special?.kind === "list" && "항목 누르기 · X 기억 여부 바꾸기",
    mix && "평가가 복습 일정에 반영돼요",
    grade.ready && `${actionKey("grade")} ChatGPT 채점 보조`,
    fit.overflow && "↑ ↓ 카드 안 스크롤",
    card?.source && `${actionKey("source")} 연결 자료`,
    `${actionKey("edit")} 편집`,
    last && `${actionKey("undo")} 되돌리기`,
  ];
  const keyLine = (parts: unknown[]) => parts.filter(Boolean).join(SEP);
  const keywords = card?.written?.keywords ?? [];

  return (
    <main
      className={`study ${panel ? "has-source" : ""}`}
      // 마우스로 누른 버튼에는 초점을 두지 않는다. 남은 초점이 Space · Enter 를 가져가 답 보기 등이 안 되는 일을 막는다 (키보드 Tab 은 그대로)
      onMouseDown={(e) => (e.target as HTMLElement).closest(".study__tool, .study__toast-btn, .study__undo-btn, .study__more, .study__folded, .rating, .study__reveal, .snd, .slot, .opt") && e.preventDefault()}
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
        {/* 학습 단계로 다시 나올 카드는 "N / 전체"에 없다. 끝 화면처럼 따로 적어 "남은 0장" 뒤에도 카드가 나오는 까닭을 밝힌다 (QA V6-01) */}
        <span className="t-body-s c-tertiary">
          남은 {n(remaining)}장{later.current.length > 0 && ` · 다시 볼 ${n(later.current.length)}장`}
        </span>
      </div>

      {card && (
        <div className={`study__col ${phase === "a" ? "is-answer" : ""} ${tip ? "has-tip" : ""} ${qTip ? "has-qtip" : ""} ${writing ? "is-writing" : ""} ${special ? `is-recall is-${special.kind}` : ""}`}>
          {/* 밝은 덱 카드는 카드 면만 밝은 토큰으로 (tokens.css [data-theme="light"]) */}
          <article className={`study__card ${fit.overflow ? "is-overflow" : ""}`} data-theme={face === "light" ? "light" : undefined}>
            {noteCss && <style>{noteCss}</style>}
            <header className="study__card-head">
              <span className="study__meta">
                <Importance level={card.view.importance} />
                {typeof mixRoles.current.get(card.view.id)?.[0] === "string" && <Badge tone="danger">취약 · {mixRoles.current.get(card.view.id)![0]}</Badge>}
                {card.written?.written && <Badge tone="accent">서술형</Badge>}
                <span className="t-caption c-tertiary">{pathLabel(card.view.deck_path)}</span>
                {special && (
                  <Badge tone="accent">
                    {special.kind === "list" ? listLabel(special.items.length, special.ordered) : choiceLabel(special.options.length, special.correct.length > 1)}
                  </Badge>
                )}
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
                    aria-label="연결 자료 보기"
                    aria-pressed={panel}
                    aria-keyshortcuts={actionKey("source")}
                    onClick={() => setSrcOpen((v) => !v)}
                  >
                    <Icon name="file" size={16} />
                    <span className="t-label-s">연결 자료</span>
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
            <div ref={scroller} className="study__scroll" onScroll={onScroll}>
              <div className="study__card-content" onClick={zoomImage}>
                <div className={face === "night" ? "nightMode night_mode" : undefined}>
                {special ? (
                  <div className="study__a recall" ref={phase === "a" ? answerBox : undefined} tabIndex={-1} aria-label={phase === "a" ? "정답" : undefined}>
                    <RecallFace
                      s={special}
                      phase={phase}
                      question={card.question}
                      extra={card.answer}
                      opened={opened}
                      marks={marks}
                      last={card.view.last_marks}
                      order={order}
                      openKey={actionKey("reveal")}
                      onSlot={onSlot}
                      onPick={pick}
                    />
                  </div>
                ) : phase === "q" ? (
                  <div className="study__q card-html" dangerouslySetInnerHTML={{ __html: card.front }} />
                ) : (
                  <div className="study__a" ref={answerBox} tabIndex={-1} aria-label="정답">
                    {plainText(card.question) || card.question.includes("<img") || card.question.includes('class="snd') ? (
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
                                등록 핵심어 {n(compare.keywords.length)}개 중 {n(compare.hits)}개 확인
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
                            <span className="t-caption c-tertiary">모범 답안</span>
                            <p className="cmp__text">
                              {marked(compare.correct, compare.correct_hits, compare.correct_misses)}
                              {card.answer.includes('class="snd') && <span className="cmp__snd card-html" dangerouslySetInnerHTML={{ __html: soundChips(card.answer) }} />}
                            </p>
                          </div>
                        </div>
                        {concept.slot}
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
              {qTip && <div className="study__tipwrap">{ttsTip}</div>}
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
                      } else if (!typed && last && matchAction("undo", e.nativeEvent) >= 0) {
                        // 다음 카드가 서술형이면 입력 칸에 초점이 와 있다. 칸이 비었으면 되돌릴 글이 없으니 토스트의 Ctrl Z = 평가 되돌리기 (QA Q5 L2)
                        e.preventDefault();
                        undo();
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
                label={writing ? "답과 비교하기" : special?.kind === "list" ? "다음 항목" : special && marks.length ? "확인" : "답 보기"}
                shortcut={writing ? actionKey("save") : actionKey("reveal")}
                aria-keyshortcuts={writing ? actionKey("save") : actionKey("reveal")}
                onClick={reveal}
              />
              {(!toast || undoTop) && <p className={`study__keys t-caption c-tertiary ${pushKeys}`}>{keyLine(keysQ)}</p>}
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
                ) : suggest !== null && special ? (
                  <span className="study__tip study__tip--warn">
                    <Icon name="info" size={16} />
                    <span className="t-label-s">
                      {RATING_NAME[suggest]} 추천 · {recallTip(special, marks, order, suggest)} 추천은 따르지 않아도 돼요.
                    </span>
                  </span>
                ) : suggest !== null ? (
                  <span className="study__tip study__tip--warn">
                    <Icon name="info" size={16} />
                    <span className="t-label-s">
                      핵심어 기준 {RATING_NAME[suggest]} 추천 ·{" "}
                      {missing.length
                        ? `핵심어 ${n(missing.length)}개가 빠졌어요 (${missing.slice(0, 3).join(", ")}${missing.length > 3 ? " 등" : ""}).`
                        : "핵심어를 모두 썼어요."}{" "}
                      답의 의미를 확인하고 직접 평가해 주세요.
                    </span>
                  </span>
                ) : soundTip?.id === card.view.id && soundTip.tts ? (
                  ttsTip
                ) : soundTip?.id === card.view.id ? (
                  <span className="study__tip">
                    <Icon name="volume-2" size={16} className="c-accent" />
                    <span className="t-label-s c-accent">
                      소리는 카드를 열 때 · 답을 볼 때 저절로 재생해요. {actionKey("replay")}
                      {ro(actionKey("replay"))} 다시 들어요. 설정에서 끌 수 있어요.
                    </span>
                    <button className="study__tip-x c-accent" aria-label="소리 안내 숨기기" onClick={() => setSoundTip(null)}>
                      <Icon name="x" size={14} />
                    </button>
                  </span>
                ) : (
                  hints && (
                    <span className="study__tip">
                      <Icon name="info" size={16} className="c-accent" />
                      <span className="t-label-s c-accent">기억한 정도를 고르면 다음 복습 일정을 정해요 · 기준은 처음에만 함께 보여요</span>
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
              {(!toast || undoTop) && <p className={`study__keys study__keys--a t-caption c-tertiary ${pushKeys}`}>{keyLine(keysA)}</p>}
            </>
          )}
          {error && <p className="t-body-s c-danger">{error}</p>}
        </div>
      )}
      {!card && error && <p className="study__error t-body-m c-danger">{error}</p>}

      {toast ? (
        // Q1 목록: 되돌리기 토스트처럼 위(top 70)에 한 줄 (아래에 두면 [다음 항목]을 가린다)
        <div className={`study__toast ${undoTop ? "is-top" : ""}`} role="status">
          <span className="study__toast-icon">
            <Icon name="archive" size={18} />
          </span>
          <span className="study__toast-text">
            <span className="t-body-m-strong">{toast.auto ? "방금 카드를 잠시 접어둘게요" : "방금 카드를 접어뒀어요"}</span>
            {!undoTop && <span className="t-body-s c-on-inverse">
              {toast.reason
                ? `두 번 막혔어요. ${toast.reason}, 나눠서 외우는 게 나을 수도 있어요.`
                : toast.auto
                  ? "같은 학습에서 두 번 기억나지 않았어요. 나머지를 먼저 끝내고 마지막에 다시 보여드릴게요."
                  : cram || mix
                    ? "나머지를 먼저 끝내고 마지막에 다시 보여드릴게요."
                    : "학습을 마치면 다시 볼지 물어볼게요."}
            </span>}
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
              연결 자료 보기
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
          <div className={`study__undo ${undoTop ? "is-top" : ""}`} role="status">
            <span className={`dot dot--${RATING_DOT[rated.rating]}`} />
            <span className="t-label-m">{RATING_NAME[rated.rating]}</span>
            <span className="study__undo-q t-caption c-on-inverse">{rated.question}</span>
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
          // 편집기에서 돌아오면 같은 덱을 이어서 (새 세션, 평가한 카드는 다시 나오지 않는다). 회독 · 혼합 · 다시 보기 묶음은 처음 화면으로
          back={cram || mix || props.cardIds ? { name: "summary", deckIds, cram: cram || !!mix } : { name: "study", deckIds, scale, edit: card.view.id }}
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

export function EditNote({ noteId, card, back, onClose, onSaved, onDeleted }: {
  noteId: number;
  /** NT13 [편집] 으로 노트 유형 편집기에 갔다가 돌아올 곳 */
  back?: Route;
  /** 이 카드만의 중요도 · 하위 덱 이동 · 삭제 (아래 줄). 없으면 노트 내용만 고친다 */
  card?: Pick<CardView, "id" | "importance" | "deck_path" | "occlusion">;
  onClose: () => void;
  onSaved: () => void;
  /** 카드를 지운 뒤. 없으면 onSaved 를 부른다 */
  onDeleted?: () => void;
}) {
  // own: 사용자 · 가져온 유형 (카드 추가 NT6 과 같은 칸). clozeAt: [빈칸 만들기]를 붙일 칸
  const [note, setNote] = useState<(NoteEdit & { roles: FieldRole[]; own: boolean; clozeAt: boolean[] }) | null>(null);
  const [choices, setChoices] = useState<DeckChoice[] | null>(null);
  const [deckId, setDeckId] = useState<number>();
  const [level, setLevel] = useState<Level | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [rootId, setRootId] = useState<number>();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const toast = useToast();
  const unplaced = useRef(0); // Q8 넣지 않은 오답 보기 제안 (저장하면 버려진다, Q43)
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
  // Q24 목록 · 선택형은 카드 추가(Q0)와 같은 편집기. 질문 · 추가 설명은 첫 칸 · 마지막 칸
  const [list, setList] = useState<ListDraft | null>(null);
  const [choice, setChoice] = useState<ChoiceDraft | null>(null);
  // NT13 머리 "노트 유형 · 이름 [편집]". touched: 무엇이든 고쳤다 ([편집]으로 떠나기 전에 묻는다)
  const [typeLabel, setTypeLabel] = useState<string>();
  const [touched, setTouched] = useState(false);
  const loaded = useRef({ list: "null", choice: "null" }); // 목록 · 선택형 처음 값 (넣은 오답 보기도 고친 것이다, QA T7-13)
  const [leaving, setLeaving] = useState<Route | null>(null);
  const nav = useNav();

  useEffect(() => {
    api
      .noteEdit(noteId)
      .then((n) => Promise.all([n, writtenApi.note(noteId).catch(() => null), api.noteTypeDetail(n.note_type_id).catch(() => null)] as const))
      .then(([n, w, d]) => {
      setTypeLabel(d ? (d.native_kind ? (TYPE_LABEL[d.native_kind] ?? d.name) : d.name) : n.type_name);
      setExam(n.exam);
      // 사용자 · 가져온 유형은 카드 추가(NT6)와 같은 칸: 필드 순서대로 세로로, 앞면 템플릿이 쓰는 필드는 툴바 있는 편집기 (I44).
      // 같은 노트를 추가할 때와 고칠 때 칸 자리가 같다. 앱 기본 유형은 C3-e 배치 그대로
      const own = !!d && !d.native_kind && !n.special;
      const roles: FieldRole[] = own ? n.front.map((f) => (f ? "front" : "rich")) : fieldRoles(n);
      if (w?.native) roles.splice(0, 3, "front", "rich", "rich"); // 질문 · 모범 답안 · 채점 메모
      else if (!own && w?.capable && w.answer_field !== null) roles[w.answer_field] = "rich"; // 제안 칩을 뽑는 칸
      const clozeRefs = own && d ? d.templates.flatMap((t) => tagRefs(t.qfmt)).filter((r) => r.cloze).map((r) => r.name) : [];
      const clozeAt = n.names.map((name, i) => (own ? clozeRefs.includes(name) : n.kind === "cloze" && i === 0));
      setNote({ ...n, roles, own, clozeAt });
      const l = n.special?.kind === "list" ? { items: n.special.items.length ? n.special.items : [""], ordered: n.special.ordered } : null;
      const c = n.special?.kind === "choice" ? choiceDraft(n.special) : null;
      loaded.current = { list: JSON.stringify(l), choice: JSON.stringify(c) };
      setList(l);
      setChoice(c);
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
    note?.own
      ? note.names[i]
      : wn?.native
      ? (["질문", "모범 답안", "채점 메모"][i] ?? note?.names[i] ?? "")
      : note?.kind === "basic"
      ? (reversed ? ["앞면", "뒷면"] : ["질문", "답변"])[i]
      : cloze && i === 0
        ? "본문"
        : cloze && i === 1 && note.names.length === 2
          ? "추가 설명"
          : (note?.names[i] ?? "");

  /** after: 저장한 뒤 onSaved 대신 (NT13 [저장하고 이동]). 실패하면 부르지 않고 오류를 남긴다 */
  const save = async (after?: () => void) => {
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
    const edit = () => ({ question: toStoredHtml(fields.current[0]?.html() ?? ""), extra: toStoredHtml(fields.current[note.names.length - 1]?.html() ?? "") });
    setSaving(true);
    try {
      if (list) await recall.updateList(note.note_id, { ...edit(), ...list });
      else if (choice) {
        await recall.updateChoice(note.note_id, { ...edit(), ...choice });
        if (unplaced.current) toast({ title: "카드를 저장했어요", text: `넣지 않은 제안 ${n(unplaced.current)}개는 버렸어요`, icon: "check-circle" });
      }
      else await api.updateNote(note.note_id, values, note.slots); // 보이는 순서의 값을 제 자리로 (순서를 바꾼 유형)
      if (wn) await writtenApi.saveNote(note.note_id, write, kws, dismissed);
      if (exam && JSON.stringify(exam) !== JSON.stringify(note.exam)) await examApi.set("note", [exam]);
      if (card && target && target.id !== here?.id) await api.moveCard(card.id, target.id);
      // 옮길 덱의 기본값과 같으면 개별 지정을 풀어 덱 설정을 따르게 한다
      if (card && level !== null && level !== card.importance) await api.setCardImportance(card.id, level === target?.importance ? null : level);
      (after ?? onSaved)();
    } catch (e) {
      setError(errorText(e));
      setSaving(false);
    }
  };

  // 무엇이든 고쳤다: 닫을 때(Esc · 바깥 · X · 취소) · [편집]으로 떠날 때 버릴지 먼저 묻는다 (QA T1-02 · T8-05 · T7-13)
  const changed =
    !!note &&
    (touched ||
      (level !== null && level !== card?.importance) ||
      (deckId !== undefined && deckId !== here?.id) ||
      JSON.stringify(exam) !== JSON.stringify(note.exam) ||
      JSON.stringify(list) !== loaded.current.list ||
      JSON.stringify(choice) !== loaded.current.choice ||
      (!!wn && (write !== wn.write || JSON.stringify(kws) !== JSON.stringify(wn.keywords))));

  /** NT13 [편집]: 그 노트 유형 편집기로. 고친 것이 있으면 버릴지 먼저 묻는다 */
  const toType = () => {
    if (!note) return;
    const to: Route = { name: "noteTypeEdit", id: note.note_type_id, note: note.note_id, back };
    setError(undefined);
    if (changed) setLeaving(to);
    else nav(to);
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
  // 초점이 칸 밖(팝오버를 닫은 뒤 창 상자 · body)이어도 저장 키를 받는다. 칸 안의 키는 아래 .edit 가 먼저 받아 막으므로 두 번 저장하지 않는다 (QA V6-05)
  useKey({ action: "save" }, () => void save(), !!note && !confirming && !leaving, { modal: true });
  // 닫은 뒤 연 곳(숨은 연필이면 그 가까이)으로 초점을 돌려주는 것은 공용 창(useOverlayScope)이다 (QA V4-08 ④)

  const front = note ? Math.max(0, note.roles.indexOf("front")) : 0;
  // M 그림 가리기: 카드 편집(E)은 가리기 편집기로 연다
  if (note?.kind === "image_occlusion") return <OcclusionEdit noteId={noteId} focus={card?.occlusion?.ordinal} onClose={onClose} onSaved={onSaved} />;
  const preview = note && [note.kind === "basic" ? plainText(note.values[1] ?? "") : "", here?.label ?? ""].filter(Boolean).join(" · ");
  // Q24 질문 · 추가 설명 칸 (카드 추가 Q0 처럼 툴바 없이)
  const recallField = (i: number, name: string) => (
    <div className="edit__item edit__item--wide">
      <span className="t-label-m">{name}</span>
      <RichField
        ref={(h) => {
          fields.current[i] = h;
        }}
        initial={note?.values[i]}
        minHeight={44}
        toolbar={false}
        onInput={() => {}}
      />
    </div>
  );

  return (
    <>
      {/* 확인 창이 떠 있으면 Esc 는 확인 창만 닫는다 (두 창의 Esc 중 어느 쪽이 먼저 받아도 같게). 하위 덱 드롭다운은 제 Esc 를 먼저 가로챈다 */}
      <Modal
        title="카드 편집"
        sub={
          <span className="edit__meta">
            <span>
              {`${
                wn?.native
                  ? `서술형 카드 · ${card ? `${pathLabel(card.deck_path)} · ` : ""}`
                  : note?.kind === "list" || note?.kind === "choice"
                    ? `${note.kind === "list" ? "목록 회상" : "선택형"} · `
                    : note?.kind === "other"
                      ? `필드 ${note.names.length}개 · `
                      : reversed
                        ? "양방향 · 카드 2장 · "
                        : ""
              }저장해도 복습 일정은 그대로예요`}
            </span>
            {note && typeLabel && (
              <span className="edit__type t-label-s c-secondary">
                <Icon name="layers" size={12} />
                노트 유형 · {typeLabel}
                <button className="edit__type-edit c-accent" onClick={toType}>
                  <Icon name="pencil" size={12} />
                  편집
                </button>
              </span>
            )}
          </span>
        }
        className={`modal--edit ${wn || list || choice ? "modal--written" : ""}`}
        onClose={() => (confirming ? setConfirming(false) : leaving ? setLeaving(null) : onClose())}
        dirty={changed && !confirming && !leaving}
      >
        <div
          className="edit"
          onKeyDown={(e) => {
            if (confirming || leaving) return;
            if (matchAction("save", e.nativeEvent) >= 0) {
              e.preventDefault();
              save();
            } else if (cloze && e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "c") {
              e.preventDefault();
              // 사용자 빈칸 유형은 지금 쓰는 칸
              makeCloze(((note?.own && fields.current.find((f) => f?.el()?.contains(document.activeElement))) || fields.current[0])?.el());
            }
          }}
        >
          {/* 칸에 친 글자만 고친 것으로 본다. 기출 팝오버에 치다 버린 이름 등 칸 밖 입력은 값으로 비교한다 (QA V4-09) */}
          <div className="edit__fields" onInput={() => setTouched(true)}>
            {note && (list || choice) ? (
              <>
                {recallField(0, "질문")}
                <div className="edit__item edit__item--wide">
                  {list ? <ListEditor value={list} onChange={setList} /> : choice && <ChoiceEditor value={choice} onChange={setChoice} question={() => fields.current[0]?.html() ?? ""} onUnplaced={(c) => (unplaced.current = c)} />}
                </div>
                {recallField(note.names.length - 1, "추가 설명")}
              </>
            ) : note?.names.map((_, i) =>
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
                      note.clozeAt[i] ? (
                        <Button
                          variant="secondary"
                          size="M"
                          label="빈칸 만들기"
                          shortcut="Ctrl Shift C"
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => makeCloze(fields.current[i]?.el())}
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
            {/* 떠날 때 묻는 창이 떠 있으면 오류는 그 창 안에만 */}
            <span className="edit__msg t-body-s c-danger">{leaving ? null : error}</span>
            <ModalCancel />
            <Button size="M" icon="check" label="저장" shortcut={actionKey("save")} onClick={() => save()} disabled={!note} busy={saving} busyLabel="저장 중…" />
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
      {leaving && (
        // 주 버튼은 [저장하고 이동]. 저장이 안 되면 이동하지 않고 이 창에 이유를 보인다
        <Modal title="고친 내용이 있어요" onClose={() => setLeaving(null)} className="edit__leave">
          <p className="t-body-m c-secondary">노트 유형 편집으로 가기 전에 고친 내용을 저장할까요?</p>
          {error && (
            <p className="edit__leave-error t-body-s c-danger" role="alert">
              저장하지 못했어요 · {error}
            </p>
          )}
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="계속 고치기" shortcut="Esc" onClick={() => setLeaving(null)} />
            <Button variant="secondary" size="M" label="버리고 이동" onClick={() => nav(leaving)} />
            <Button size="M" icon="check" label="저장하고 이동" onClick={() => save(() => nav(leaving))} busy={saving} busyLabel="저장 중…" />
          </div>
        </Modal>
      )}
    </>
  );
}
