// C3 카드 추가 / C3-b 빈칸 카드 / C3-r 앞뒤 둘 다 / C3-n 넣을 곳 / J3 서술형 / M1 그림 가리기 / Q0 목록 · 선택형
// NT6 다른 노트 유형 고르기 · NT7 유형 추천 칩 (8.1 2단계)
import { forwardRef, Fragment, useEffect, useImperativeHandle, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { api, errorText, type CardKindHint, type DeckDetail, type Level, type NoteTypeDetail, type NoteTypeRow } from "../api";
import { Button, Checkbox, Icon, Importance, LEVEL_LABEL, Badge, Toggle } from "../ui";
import { n, ro, useKey, useLeaveGuard, useNav, useOverlayScope, type Route } from "../nav";
import { actionKey, chordOf, getKeys, matchAction, useKeymap } from "../keys";
import { imageTag, plainText, renderCardHtml, sanitize, toStoredHtml } from "../cardHtml";
import { deckFace, scopeCss } from "../cardCss";
import { checkMath, findMath, withMath } from "../math";
import { useToast } from "../toast";
import { DiscardModal } from "./NewDeck";
import { menuKeys } from "./Deck";
import { DeckPicker, rootDeckIdOf, type DeckChoice } from "./DeckPicker";
import { KeywordEditor } from "./StudyParts";
import { written, type Keyword } from "../written";
import { OcclusionEditor, OcclusionPicture } from "./Occlusion";
import { boxCount, labelOf, naturalSize, occlusionApi, ordinalsOf, storeImage, type Picture, type Shape } from "../occlusion";
import { ChoiceEditor, ChoicePreview, ListEditor, ListPreview } from "./Recall";
import { EMPTY_CHOICE, EMPTY_LIST, recall } from "../recall";
import "../styles/study.css";

/** typed: 사용자 · 가져온 노트 유형 (NT6). 필드 순서대로 칸을 만들고 add_typed_note 로 저장한다 */
type Kind = "basic" | "reversed" | "cloze" | "written" | "occlusion" | "list" | "choice" | "typed";
type Native = Exclude<Kind, "typed">;
const KINDS: [Native, string][] = [
  ["basic", "질문과 답변"],
  ["reversed", "양방향"],
  ["cloze", "빈칸"],
  ["written", "서술형"],
  ["occlusion", "그림 가리기"],
  ["list", "목록"],
  ["choice", "선택형"],
];
/** 칸 이름 (q · a · 채점 메모 순서). 유형을 바꿀 때 같은 이름 칸으로 내용을 옮긴다 */
const LABELS: Record<Native, string[]> = {
  basic: ["질문", "답변"],
  reversed: ["앞면", "뒷면"],
  cloze: ["본문", "추가 설명"],
  written: ["질문", "모범 답안", "채점 메모"],
  occlusion: ["질문", "추가 설명"],
  list: ["질문", "추가 설명"],
  choice: ["질문", "추가 설명"],
};
/** 앱 기본 유형의 칸 이름 (노트 유형 편집기가 DB 필드 이름 대신 보인다, NT51). native_kind 로 */
export const nativeLabels = (nativeKind: string): string[] | undefined => LABELS[(nativeKind === "io" ? "occlusion" : nativeKind) as Native];
/** 앱 유형의 native_kind (그림 가리기만 io) */
const nativeKey = (k: Kind) => (k === "occlusion" ? "io" : k);
const kindOf = (nativeKind: string) => (nativeKind === "io" ? "occlusion" : nativeKind) as Native;
/** 앱 기본 유형의 화면 이름 (native_kind -> 세그먼트 이름). 카드 편집 머리(NT13)도 쓴다 */
export const TYPE_LABEL: Record<string, string> = Object.fromEntries(KINDS.map(([k, label]) => [nativeKey(k), label]));
/** Q 목록 · 선택형 */
const isRecall = (k: Kind) => k === "list" || k === "choice";

// ---------------------------------------------------------------- NT12 덱마다 마지막에 쓴 유형 (이 기기에만)

export const LAST_TYPE = "neoanki.addCard.lastType"; // { [덱 id]: 노트 유형 id }. 모든 데이터 삭제가 같이 지운다
function lastType(deckId: number): number | undefined {
  try {
    return JSON.parse(localStorage.getItem(LAST_TYPE) ?? "{}")[deckId];
  } catch {
    return undefined; // 못 읽으면 그 덱 노트가 가장 많이 쓰는 유형으로 연다
  }
}
export function rememberType(deckId: number, typeId: number) {
  try {
    const all = JSON.parse(localStorage.getItem(LAST_TYPE) ?? "{}");
    localStorage.setItem(LAST_TYPE, JSON.stringify({ ...all, [deckId]: typeId }));
  } catch {
    /* 저장하지 못해도 카드 추가는 된다 */
  }
}

// ---------------------------------------------------------------- 유형을 바꿀 때 내용 옮기기

type Field = { name: string; html: string };
/** 내용이 있는 칸 (빈칸 뒤 공백 문자는 내용이 아니다) */
const filled = (html: string) => plainText(html).replace(/​/g, "") !== "" || html.includes("<img");
/** 빈칸 문법을 글자로 ({{c1::답::힌트}} -> 답). 빈칸이 아닌 유형으로 옮길 때 */
const unCloze = (html: string) => html.replace(/\{\{c\d+::([\s\S]*?)(?:::[\s\S]*?)?\}\}/g, "$1");
const ALIAS = [
  ["질문", "앞면", "본문"],
  ["답변", "뒷면", "모범 답안"],
];
const sameField = (x: string, y: string) => x.toLowerCase() === y.toLowerCase() || ALIAS.some((g) => g.includes(x) && g.includes(y));
/**
 * 첫 칸은 첫 칸으로, 나머지는 같은 이름 칸으로 (질문 · 앞면 · 본문, 답변 · 뒷면 · 모범 답안은 같은 칸으로 본다).
 * 갈 곳이 없는데 내용이 있는 칸은 lost (바꾸기 전에 묻는다)
 */
function placeFields(from: Field[], to: string[]) {
  const used = new Set<number>();
  const values = to.map((name, j) => {
    const i = j === 0 ? 0 : from.findIndex((f, k) => !used.has(k) && sameField(f.name, name));
    if (i < 0 || !from[i] || used.has(i)) return "";
    used.add(i);
    return from[i].html;
  });
  return { values, lost: from.filter((f, k) => !used.has(k) && filled(f.html)).map((f) => f.name) };
}

/** 템플릿이 쓰는 필드 이름 ({{X}} {{#X}} {{필터:X}}). cloze: {{cloze:X}} 로 쓴다 */
export function tagRefs(tpl: string) {
  return [...tpl.matchAll(/\{\{([^}]+)\}\}/g)].map((m) => {
    const t = m[1].trim().replace(/^[#^/]/, "");
    const at = t.lastIndexOf(":");
    return { name: t.slice(at + 1).trim(), cloze: t.slice(0, Math.max(0, at)).split(":").some((f) => f.trim().toLowerCase() === "cloze") };
  });
}
/** 값들의 빈칸 번호 (작은 것부터) */
const clozeNumbers = (values: string[]) => [...new Set(values.flatMap((v) => [...v.matchAll(/\{\{c(\d+)::/g)].map((m) => Number(m[1]))))].sort((x, y) => x - y);

// ---------------------------------------------------------------- NT14 유형 추천 칩: 내용을 그 유형 칸으로

/** 목록 표시 (quality.rs item_marker 와 같은 표시: ①… · 불릿 · 1. 1) (1)) */
const ITEM_MARK = /^(?:[①-⑳]|[-•·*▪○●◦■□]|\d+[.)]|\(\d+\))\s*/;
/** 편집기 HTML -> 글자 줄 (quality.rs html_lines 와 같은 블록 규칙) */
function textLines(html: string) {
  const tpl = document.createElement("template");
  tpl.innerHTML = html.replace(/<\/?(?:br|div|p|li|tr|td|th|hr|h[1-6])\b[^>]*>/gi, "\n");
  return (tpl.content.textContent ?? "")
    .replace(/​/g, "")
    .split("\n")
    .map((l) => l.replace(/\s+/g, " ").trim())
    .filter(Boolean);
}
/** 목록으로: 표시 없는 줄 = 질문, 표시 붙은 줄 = 항목 (표시는 뗀다). 번호만 있으면 순서 있는 목록 */
export function toListDraft(html: string) {
  const lines = textLines(html);
  const marked = lines.filter((l) => ITEM_MARK.test(l));
  return {
    question: lines.filter((l) => !ITEM_MARK.test(l)).map(escapeText).join("<br>"),
    items: marked.map((l) => l.replace(ITEM_MARK, "").trim()),
    ordered: marked.every((l) => /^(?:[①-⑳]|\d|\()/.test(l)),
  };
}
/** 선택형으로: 첫 보기 표시 앞 = 질문, ①… 또는 " 1)" 로 나눈 조각 = 보기 (한 줄에 이어 써도 된다) */
export function toChoiceDraft(html: string) {
  const text = textLines(html).join("\n");
  const at = [...text.matchAll(/[①-⑳]|(?<=^|\s)\d+\)/gm)].map((m) => ({ i: m.index ?? 0, len: m[0].length }));
  const head = text.slice(0, at[0]?.i ?? text.length);
  return {
    question: head.split("\n").map((l) => l.trim()).filter(Boolean).map(escapeText).join("<br>"),
    options: at.map((m, j) => text.slice(m.i + m.len, at[j + 1]?.i ?? text.length).replace(/\s+/g, " ").trim()).filter(Boolean),
  };
}
/** 칩의 이유 한 줄. 센 사실만 쓴다 (copy-guide 유형 추천) */
function hintReason(k: CardKindHint, html: string) {
  if (k === "list") {
    const marked = textLines(html).filter((l) => ITEM_MARK.test(l));
    return `${marked.every((l) => /^(?:[①-⑳]|\d|\()/.test(l)) ? "번호 붙은" : "글머리표"} 줄이 ${n(marked.length)}개예요`;
  }
  if (k === "choice") return `보기 ${n(toChoiceDraft(html).options.length)}개가 있어요`;
  return k === "occlusion" ? "그림만 있어요" : "빈칸 표시가 있어요";
}
const stripImages = (html: string) => html.replace(/<img\b[^>]*>/gi, "");
/** 괄호 빈칸 "( )" "[ ]" "（ ）" · 밑줄 "___" (notetype.rs blank_mark 와 같은 표시). 편집기 HTML 의 공백은 &nbsp; 일 수 있다 */
const BLANK = /\((?:\s|&nbsp;)+\)|\[(?:\s|&nbsp;)+\]|（(?:\s|&nbsp;)+）|_{3,}/;

/** cdp 로 돌리는 검사: (await import("/src/screens/AddCard.tsx")).hintSelfCheck() */
export function hintSelfCheck() {
  const list = toListDraft("OSI 7계층을 위에서부터<br>1. 응용<br>2. 표현<div>3) 세션</div>");
  const bullets = toListDraft("<ul><li>- 사과</li><li>• 배</li><li>* 귤</li></ul>");
  const inline = toChoiceDraft("다음 중 전송 계층은? ① IP ② TCP ③ ARP ④ ICMP");
  const paren = toChoiceDraft("포트 80은?<br>1) HTTP<br>2) FTP<br>3) SSH<br>4) DNS");
  const from = [
    { name: "본문", html: "A" },
    { name: "추가 설명", html: "B" },
  ];
  return {
    listQuestion: list.question === "OSI 7계층을 위에서부터",
    listItems: list.items.join("|") === "응용|표현|세션",
    listOrdered: list.ordered && !bullets.ordered,
    bulletItems: bullets.items.join("|") === "사과|배|귤",
    choiceInline: inline.question === "다음 중 전송 계층은?" && inline.options.join("|") === "IP|TCP|ARP|ICMP",
    choiceParen: paren.question === "포트 80은?" && paren.options.join("|") === "HTTP|FTP|SSH|DNS",
    reason: hintReason("list", "1. a<br>2. b<br>3. c") === "번호 붙은 줄이 3개예요",
    unCloze: unCloze("{{c1::TCP::프로토콜}}와 {{c2::UDP}}") === "TCP와 UDP",
    blank: "IPv6 주소는 ( &nbsp; )비트다".replace(BLANK, "{{c1::128}}") === "IPv6 주소는 {{c1::128}}비트다" && "수도는 ___ 이다".replace(BLANK, "X") === "수도는 X 이다",
    blankNot: !BLANK.test("f(x) = (x)"),
    placeAlias: JSON.stringify(placeFields([{ name: "질문", html: "Q" }, { name: "답변", html: "A" }], LABELS.reversed)) === '{"values":["Q","A"],"lost":[]}',
    placeLost: placeFields(from, LABELS.basic).lost.join() === "추가 설명",
    placeTyped: placeFields(from, ["Text", "추가 설명"]).values.join() === "A,B",
    refs: JSON.stringify(tagRefs("{{#뜻}}{{cloze:본문}}{{/뜻}} {{type:단어}}")) ===
      '[{"name":"뜻","cloze":false},{"name":"본문","cloze":true},{"name":"뜻","cloze":false},{"name":"단어","cloze":false}]',
  };
}

export function AddCard({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [deck, setDeck] = useState<DeckDetail | null>(null);
  const [rootId, setRootId] = useState<number>();
  const [target, setTarget] = useState(deckId); // 넣을 곳. 기본은 들어온 덱, 연속 입력 중에는 마지막 선택을 유지
  const [targetDeck, setTargetDeck] = useState<DeckChoice | null>(null);
  const [kind, setKind] = useState<Kind>("basic");
  const [importance, setImportance] = useState<Level | null>(null);
  const [keepGoing, setKeepGoing] = useState(true);
  const [added, setAdded] = useState<string[]>([]);
  const [addedCount, setAddedCount] = useState(0);
  const [error, setError] = useState<string>();
  const [leaving, setLeaving] = useState<Route | null>(null); // 작성 중인 내용을 버릴지 묻는 중
  const [, setTick] = useState(0); // 편집기 내용이 바뀌면 미리보기를 다시 그린다
  const q = useRef<FieldHandle>(null);
  const a = useRef<FieldHandle>(null);
  const memo = useRef<FieldHandle>(null); // J3 채점 메모
  // J3 서술형: 필수 키워드 · 제안 · 공부할 때 직접 써서 답하기
  const [kws, setKws] = useState<Keyword[]>([]);
  const [dismissed, setDismissed] = useState<string[]>([]);
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [write, setWrite] = useState(true);
  // M1 그림 가리기: 그림 · 칸 · 묻는 방식(기본 모두 가림, M3) · 편집기(M2)
  const [pic, setPic] = useState<Picture | null>(null);
  const [shapes, setShapes] = useState<Shape[]>([]);
  const [hideAll, setHideAll] = useState(true);
  const [editing, setEditing] = useState(false);
  const [over, setOver] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  // Q0 목록 · 선택형: 항목 · 순서, 보기 · 정답 · 섞기. 추가 설명은 누르면 펼친다
  const [list, setList] = useState(EMPTY_LIST);
  const [choice, setChoice] = useState(EMPTY_CHOICE);
  const [extraOpen, setExtraOpen] = useState(false);
  // NT6 노트 유형: 목록 · 이 덱이 쓰는 유형(많이 쓰는 것 먼저) · 고른 사용자 · 가져온 유형. origin: 유형 정보 줄 아래 안내
  const [types, setTypes] = useState<NoteTypeRow[]>([]);
  const [deckTypes, setDeckTypes] = useState<number[]>([]);
  const [typed, setTyped] = useState<NoteTypeDetail | null>(null);
  const [origin, setOrigin] = useState<"remembered" | "common" | "picked">();
  const [ready, setReady] = useState(false); // 처음 열 유형을 정하기 전에는 칸을 그리지 않는다 (쓰기 시작한 칸이 바뀌지 않게)
  const tf = useRef<(FieldHandle | null)[]>([]); // typed 의 칸 (필드 순서)
  // 유형을 바꿀 때 옮겨 온 칸 내용 (q · a · 채점 메모, typed 는 필드 순서). gen 이 바뀌면 칸을 새로 그려 init 으로 채운다
  const [init, setInit] = useState<string[]>([]);
  const [gen, setGen] = useState(0);
  const [moving, setMoving] = useState<{ lost: string[]; to: string; go: () => void } | null>(null); // 옮기지 못하는 칸을 버릴지 묻는 중
  const [menuOpen, setMenuOpen] = useState(false);
  // NT14 유형 추천 칩. hintOff: ✕ 로 닫았다 (이 노트에서는 다시 띄우지 않는다, 저장하면 풀린다)
  const [hint, setHint] = useState<CardKindHint | null>(null);
  const [hintOff, setHintOff] = useState(false);
  const toast = useToast();
  const takeImage = async (files: FileList | null | undefined) => {
    const f = Array.from(files ?? []).find((x) => x.type.startsWith("image/"));
    if (!f) return;
    try {
      setPic(await storeImage(f));
      setShapes([]);
      setEditing(true);
      setError(undefined);
    } catch (e) {
      setError(errorText(e));
    }
  };

  useEffect(() => {
    api.deckDetail(deckId).then(async (d) => {
      setDeck(d);
      // 트리는 최상위 덱부터. 하위 덱 상세에서 들어왔으면 그 하위 덱이 기본으로 골라져 있다
      setRootId(d.parent_id === null ? d.id : await rootDeckIdOf(d.path));
    });
  }, [deckId]);
  // NT12: 이 덱에서 마지막에 쓴 유형, 처음이면 그 덱 노트가 가장 많이 쓰는 유형으로 연다 (가져온 덱이 앱 "질문과 답변"으로 열리지 않게)
  useEffect(() => {
    Promise.all([api.noteTypes(), api.deckNoteTypes(deckId)])
      .then(async ([rows, used]) => {
        setTypes(rows);
        setDeckTypes(used);
        const saved = lastType(deckId);
        const row = rows.find((r) => r.id === saved) ?? rows.find((r) => r.id === used[0]);
        if (!row) return;
        if (row.native_kind) setKind(kindOf(row.native_kind));
        else {
          setTyped(await api.noteTypeDetail(row.id));
          setKind("typed");
        }
        setOrigin(row.id === saved ? "remembered" : "common");
      })
      .catch(() => {}) // 못 읽으면 질문과 답변으로 연다
      .finally(() => setReady(true));
  }, [deckId]);
  useEffect(() => {
    (kind === "typed" ? tf.current[0] : q.current)?.focus();
  }, [kind, gen, ready]);

  const refresh = () => setTick((t) => t + 1);
  const isCloze = (k: Kind, t: NoteTypeDetail | null) => k === "cloze" || (k === "typed" && !!t?.cloze);
  const typeName = (k: Kind, t: NoteTypeDetail | null) => (k === "typed" ? (t?.name ?? "") : (KINDS.find(([x]) => x === k)?.[1] ?? ""));
  /** 지금 유형의 노트 유형 id (앱 기본 유형도). 이 유형 편집 · NT12 */
  const typeIdOf = (k: Kind, t: NoteTypeDetail | null) => (k === "typed" ? t?.id : types.find((r) => r.native_kind === nativeKey(k))?.id);
  const currentTypeId = typeIdOf(kind, typed);
  /** 지금 칸들 (저장할 HTML. 빈칸 유형은 {{c1::}} 문법) */
  const fieldsNow = (): Field[] => {
    const names = kind === "typed" ? (typed?.fields.map((f) => f.name) ?? []) : LABELS[kind];
    const refs = kind === "typed" ? tf.current : [q.current, a.current, memo.current];
    return names.map((name, i) => {
      const html = refs[i]?.html() ?? "";
      return { name, html: isCloze(kind, typed) ? toCloze(html) : toStoredHtml(html) };
    });
  };
  /** 유형을 바꾸고 칸을 values 로 새로 채운다 */
  const apply = (k: Kind, t: NoteTypeDetail | null, values: string[]) => {
    setInit(values.map((v) => (isCloze(k, t) ? fromCloze(v) : v)));
    setGen((g) => g + 1);
    setKind(k);
    setTyped(k === "typed" ? t : null);
    if (isRecall(k) && filled(values[1] ?? "")) setExtraOpen(true);
    const id = typeIdOf(k, t);
    if (id !== undefined) rememberType(deckId, id);
    setOrigin("picked");
    refresh();
  };
  /**
   * 유형 바꾸기 (세그먼트 · [⋯] · 추천 칩 · 그림 가리기 링크). 내용은 같은 이름 칸으로 옮기고(placeFields), 옮기지 못하는 칸에 내용이 있으면 먼저 묻는다.
   * force: 묻지 않는다 (추천 칩은 되돌리기 토스트가 있다). fix: 옮기기 전에 칸을 고친다 (칩이 질문 · 항목을 나눈 것)
   */
  const switchTo = (k: Kind, t: NoteTypeDetail | null = null, opts: { force?: boolean; fix?: (from: Field[]) => void; then?: () => void } = {}) => {
    setMenuOpen(false);
    const from = fieldsNow().map((f) => ({ ...f, html: isCloze(kind, typed) && !isCloze(k, t) ? unCloze(f.html) : f.html }));
    opts.fix?.(from);
    const { values, lost } = placeFields(from, k === "typed" ? (t?.fields.map((f) => f.name) ?? []) : LABELS[k]);
    const go = () => {
      setMoving(null);
      apply(k, t, values);
      opts.then?.();
    };
    if (lost.length && !opts.force) setMoving({ lost, to: typeName(k, t), go });
    else go();
  };
  const pickType = (row: NoteTypeRow) => {
    setMenuOpen(false);
    if (row.native_kind) return kindOf(row.native_kind) !== kind && switchTo(kindOf(row.native_kind));
    if (row.id === typed?.id) return;
    api.noteTypeDetail(row.id).then((d) => switchTo("typed", d), (e) => setError(errorText(e)));
  };
  // 노트 유형 편집 · 관리는 설정 화면(NT1 · NT2). 쓰던 내용이 있으면 이탈 확인(useLeaveGuard)을 거친다. 돌아오면 이 덱의 카드 추가
  const editType = () => currentTypeId !== undefined && nav({ name: "noteTypeEdit", id: currentTypeId, back: { name: "addCard", deckId } });
  const manageTypes = () => nav({ name: "noteTypes", back: { name: "addCard", deckId } });

  /** M1: 다른 유형에서 붙인 그림으로 그림 가리기 카드를 만든다. 그 그림을 넘기고 편집기(M2)를 연다. field: 그림이 있던 칸 (그 칸의 그림은 칸에서 뺀다) */
  const toOcclusion = (img: { name: string; url: string }, field: number, force = false) =>
    switchTo("occlusion", null, {
      force,
      fix: (from) => {
        if (from[field]) from[field].html = stripImages(from[field].html);
      },
      then: async () => {
        setPic({ name: img.name, url: img.url, fileName: img.name, ...(await naturalSize(img.url)) });
        setShapes([]);
        setEditing(true);
        setError(undefined);
      },
    });
  // 첫 칸에 "그림 가리기로 바꾸기" 칩이 떠 있으면 같은 일을 하는 링크는 숨긴다 (✕ 로 닫으면 다시 보인다)
  const ioLink = (field: React.RefObject<FieldHandle | null>, i: number) =>
    i === 0 && shownHint === "occlusion" ? null : <OcclusionLink html={field.current?.html() ?? ""} onPick={(img) => toOcclusion(img, i)} />;

  // ---- NT14 유형 추천 칩: 붙여 넣거나 입력한 글(첫 칸)을 규칙으로 본다. 그림 data URL 은 보내지 않는다
  const hintSource = (kind === "typed" ? tf.current[0] : q.current)?.html() ?? "";
  const hintText = hintSource.replace(/src="data:[^"]*"/g, 'src="x"');
  // 답 칸 (질문과 답변 · 양방향 · 서술형의 둘째 칸, 사용자 유형의 둘째 필드). 괄호 빈칸은 여기 글이 있을 때만 빈칸으로 권한다 ({{c1::답}})
  const answerField = kind === "typed" ? tf.current[1] : kind === "basic" || kind === "reversed" || kind === "written" ? a.current : null;
  const hintAnswer = answerField?.html() ?? "";
  const answerText = hintAnswer.replace(/src="data:[^"]*"/g, 'src="x"');
  useEffect(() => {
    if (hintOff || !ready) return;
    const t = setTimeout(() => api.suggestCardKind(hintText, answerText).then(setHint, () => setHint(null)), 300);
    return () => clearTimeout(t);
  }, [hintText, answerText, hintOff, ready]);
  // 이미 그 유형이면 띄우지 않는다 (빈칸으로 쓰는 사용자 유형은 빈칸이다)
  const shownHint = !hintOff && hint && hint !== kind && !(hint === "cloze" && isCloze(kind, typed)) ? hint : null;
  const takeHint = (k: CardKindHint) => {
    const before = { kind, typed, values: fieldsNow().map((f) => f.html), list, choice, extraOpen, pic, shapes };
    const html = hintSource;
    const label = typeName(k, null);
    if (k === "list") {
      const d = toListDraft(html);
      switchTo("list", null, { force: true, fix: (from) => void (from[0].html = d.question) });
      setList({ items: d.items.length ? d.items : [""], ordered: d.ordered });
    } else if (k === "choice") {
      const d = toChoiceDraft(html);
      switchTo("choice", null, { force: true, fix: (from) => void (from[0].html = d.question) });
      setChoice((c) => ({ options: d.options.length >= 2 ? d.options : [...d.options, "", ""].slice(0, 2), correct: [], shuffle: c.shuffle }));
    } else if (k === "occlusion") {
      const img = lastImage(html);
      if (img) toOcclusion(img, 0, true);
      else switchTo("occlusion", null, { force: true, fix: (from) => void (from[0].html = stripImages(from[0].html)) });
    } else if (/\{\{c\d+::/.test(html)) switchTo("cloze", null, { force: true });
    else {
      // 괄호 빈칸 · ___ 에 답 칸의 글을 넣는다 ({{c1::답}}). 답 칸은 비운다 (추가 설명으로 옮기지 않는다)
      const answer = escapeText(plainText(hintAnswer));
      switchTo("cloze", null, {
        force: true,
        fix: (from) => {
          from[0].html = from[0].html.replace(BLANK, `{{c1::${answer}}}`);
          if (from[1]) from[1].html = "";
        },
      });
    }
    toast({
      title: `${label}${ro(label)} 바꿨어요`,
      tone: "undo",
      action: {
        label: "되돌리기",
        onClick: () => {
          apply(before.kind, before.typed, before.values);
          setList(before.list);
          setChoice(before.choice);
          setExtraOpen(before.extraOpen);
          setPic(before.pic);
          setShapes(before.shapes);
          setEditing(false);
          setHintOff(true); // 되돌렸으면 이 노트에서는 다시 권하지 않는다
        },
      },
    });
  };
  /** 칩은 그 글이 있는 칸(첫 칸) 바로 아래 한 줄. 쓰고 있는 칸은 움직이지 않고 그 아래만 내려간다 */
  const chip = shownHint && (
    <div className="addcard__hint" role="status">
      <Icon name="info" size={14} className="c-accent" />
      <span className="t-label-s">{hintReason(shownHint, hintSource)}</span>
      <button className="addcard__hint-act t-label-s" onClick={() => takeHint(shownHint)}>
        {typeName(shownHint, null)}
        {ro(typeName(shownHint, null))} 바꾸기
      </button>
      <button className="addcard__hint-x" aria-label="추천 닫기 · 이 카드에서는 다시 띄우지 않아요" title="닫기" onClick={() => setHintOff(true)}>
        <Icon name="x" size={14} />
      </button>
    </div>
  );
  // 비워두면 넣을 하위 덱의 중요도를 따른다
  const inherited = targetDeck?.importance ?? deck?.own_importance ?? 1;
  const shownLevel = importance ?? inherited;

  const save = async () => {
    const qHtml = q.current?.html() ?? "";
    const aHtml = a.current?.html() ?? "";
    const front = kind === "cloze" ? toCloze(qHtml) : toStoredHtml(qHtml);
    try {
      let made = 1;
      let text = "";
      if (kind === "typed") {
        // NT6 사용자 · 가져온 유형: 필드 순서대로. 빈칸 유형은 칸마다 번호를 이어 매긴다 (두 칸이 같은 c1 이 되지 않게)
        if (!typed) return;
        let next = 1;
        const values = typed.fields.map((_, i) => {
          const html = tf.current[i]?.html() ?? "";
          if (!typed.cloze) return toStoredHtml(html);
          const v = toCloze(html, next);
          next = Math.max(next, ...clozeNumbers([v]).map((c) => c + 1));
          return v;
        });
        made = await api.addTypedNote({ deck_id: target, note_type_id: typed.id, values, importance });
        text = plainText(unCloze(values.find(filled) ?? ""));
      } else if (kind === "occlusion") {
        if (!pic) throw new Error("그림을 넣어 주세요.");
        const note = { note_id: null, deck_id: target, image: pic.name, header: toStoredHtml(qHtml), back_extra: toStoredHtml(aHtml), shapes, hide_all: hideAll, importance };
        await occlusionApi.save(note);
        made = ordinalsOf(shapes).length;
        setPic(null);
        setShapes([]);
      } else if (kind === "written") {
        const memoHtml = memo.current?.html() ?? "";
        await written.add({ deck_id: target, question: front, answer: toStoredHtml(aHtml), memo: toStoredHtml(memoHtml), importance, write, keywords: kws });
      } else if (kind === "list") {
        await recall.addList({ deck_id: target, question: front, extra: toStoredHtml(aHtml), importance, ...list });
        setList((l) => ({ ...EMPTY_LIST, ordered: l.ordered }));
      } else if (kind === "choice") {
        await recall.addChoice({ deck_id: target, question: front, extra: toStoredHtml(aHtml), importance, ...choice });
        setChoice((c) => ({ ...EMPTY_CHOICE, shuffle: c.shuffle }));
      } else made = await api.addCard({ deck_id: target, kind, front, back: toStoredHtml(aHtml), importance });
      if (kind !== "typed")
        text = kind === "occlusion" ? plainText(qHtml) || "그림 가리기 · " + (pic?.fileName ?? "") : plainText(kind === "cloze" ? clozePlain(qHtml) : qHtml);
      setAdded((list) => [...list, text].slice(-5));
      setAddedCount((c) => c + made);
      setError(undefined);
      setKws([]);
      setDismissed([]);
      setHintOff(false); // 새 노트에서는 다시 권한다
      setHint(null);
      if (currentTypeId !== undefined) rememberType(deckId, currentTypeId);
      if (keepGoing) {
        q.current?.clear();
        a.current?.clear();
        memo.current?.clear();
        tf.current.forEach((f) => f?.clear());
        (kind === "typed" ? tf.current[0] : q.current)?.focus();
        refresh();
      } else {
        nav({ name: "deck", deckId }, true); // 방금 저장했다 (칸은 비우지 않았지만 버릴 것은 없다)
      }
    } catch (e) {
      setError(errorText(e));
    }
  };

  // 쓰던 내용이 있으면 말없이 버리지 않고 먼저 묻는다. 뒤로 · Esc 뿐 아니라 상단 메뉴 · 동기화 팝오버 등 이 화면을 떠나는 모든 이동 (QA T6-04)
  useLeaveGuard((r: Route) => {
    const wrote = [q.current, a.current, memo.current, ...tf.current].some((f) => filled(f?.html() ?? ""));
    const drafted = (kind === "list" && list.items.some((t) => t.trim())) || (kind === "choice" && choice.options.some((t) => t.trim()));
    if (!(wrote || pic || drafted)) return false;
    setLeaving(r);
    return true;
  });

  const onKeyDown = (e: KeyboardEvent) => {
    // 확인 창의 Esc 는 창이 처리한다 (넣을 곳 드롭다운은 제 키를 스스로 멈춘다). 편집기 · [⋯] 메뉴는 제 키를 먼저 받는다
    if (leaving || editing || moving || menuOpen) return;
    if (matchAction("save", e.nativeEvent) >= 0) {
      e.preventDefault();
      save();
    } else if (isCloze(kind, typed) && e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "c") {
      e.preventDefault();
      // 사용자 빈칸 유형은 지금 쓰는 칸에
      makeCloze(kind === "typed" ? tf.current.find((f) => f?.el()?.contains(document.activeElement))?.el() : q.current?.el());
      refresh();
    } else if (e.key === "Escape") {
      e.preventDefault(); // 여기서 띄운 확인 창의 Esc 가 같은 키를 또 받아 바로 닫히지 않게 (QA T6-03)
      nav({ name: "deck", deckId });
    }
  };

  const qHtml = q.current?.html() ?? "";
  const marks = kind === "cloze" ? clozeMarks(qHtml) : [];
  // NT6 사용자 · 가져온 유형의 칸: 앞면 템플릿이 쓰는 필드는 툴바 있는 편집기 (I44), {{cloze:X}} 칸에는 [빈칸 만들기]
  const refs = useMemo(() => (typed ? typed.templates.flatMap((t) => tagRefs(t.qfmt)) : []), [typed]);
  const typedValues = typed && kind === "typed" ? typed.fields.map((_, i) => {
    const html = tf.current[i]?.html() ?? "";
    return typed.cloze ? toCloze(html) : toStoredHtml(html);
  }) : [];
  // C3-m: 읽지 못한 수식. 저장은 막지 않는다
  const names = kind === "typed" ? (typed?.fields.map((f) => f.name) ?? []) : LABELS[kind];
  const mathHtml = kind === "typed" ? typed?.fields.map((_, i) => tf.current[i]?.html() ?? "") ?? [] : [qHtml, a.current?.html() ?? ""];
  // J3 제안 칩: 모범 답안에서 뽑는다. 이미 정한 키워드와 뺀 말은 빼고
  const answerHtml = kind === "written" ? (a.current?.html() ?? "") : "";
  useEffect(() => {
    if (kind !== "written") return;
    const t = setTimeout(
      () => written.suggest(answerHtml, [...kws.map((k) => k.word), ...dismissed]).then(setSuggestions, () => setSuggestions([])),
      250,
    );
    return () => clearTimeout(t);
  }, [kind, answerHtml, kws, dismissed]);
  const mathErrors = mathHtml.flatMap((h, i) => checkMath(h).map((e) => ({ ...e, field: names[i] })));
  const typedCards = typed ? (typed.cloze ? clozeNumbers(typedValues).length : typed.templates.length) : 0;

  return (
    <main className="addcard" onKeyDown={onKeyDown}>
      <button className="backlink t-label-m c-secondary" onClick={() => nav({ name: "deck", deckId })}>
        <Icon name="chevron-left" size={18} />
        {deck?.name ?? ""}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">카드 추가</h1>
          <p className="t-body-s c-secondary">
            {deck?.name}
            {addedCount > 0 && ` · 이번에 ${n(addedCount)}장 추가함`}
          </p>
        </div>
        <Button
          variant="secondary"
          size="M"
          icon="arrow-right"
          label="추가 끝내고 학습 시작"
          onClick={() => nav({ name: "summary", deckIds: [deckId] })}
        />
      </div>

      {ready && (
      <div className="addcard__cols">
        <div className="addcard__form">
          <div className="segment segment--boxed addcard__seg">
            {KINDS.map(([k, name]) => (
              <button key={k} className={`segment__item t-label-m ${kind === k ? "is-selected" : ""}`} onClick={() => k !== kind && switchTo(k)}>
                {name}
              </button>
            ))}
            {kind === "typed" && typed && (
              <button className="segment__item t-label-m is-selected addcard__typed" title={typed.name} aria-current="true">
                {typed.name}
              </button>
            )}
            <TypeMenu
              open={menuOpen}
              setOpen={setMenuOpen}
              types={types}
              deckTypes={deckTypes}
              current={currentTypeId}
              onPick={pickType}
              onEdit={editType}
              onManage={manageTypes}
            />
          </div>
          {kind === "typed" && typed && (
            <>
              <div className="addcard__type">
                <Icon name="layers" size={14} className="c-secondary" />
                <span className="t-label-m c-secondary">
                  노트 유형 · {typed.name} · {typed.cloze ? "빈칸마다 카드 1장" : `노트 하나에 카드 ${n(typed.templates.length)}장`}
                </span>
                <button className="addcard__type-edit t-label-s c-accent" onClick={editType}>
                  <Icon name="pencil" size={12} />이 유형 편집
                </button>
              </div>
              <p className="t-caption c-tertiary">
                {origin === "remembered"
                  ? "이 덱에서 마지막으로 쓴 유형이에요."
                  : origin === "common"
                    ? "이 덱의 노트가 가장 많이 쓰는 유형이에요."
                    : "다음에도 이 덱은 이 유형으로 열려요."}
              </p>
            </>
          )}
          <div className="addcard__place">
            <span className="t-label-m">넣을 곳</span>
            {rootId !== undefined && (
              <DeckPicker
                rootDeckId={rootId}
                value={target}
                onChange={(d) => {
                  setTarget(d.id);
                  setTargetDeck(d);
                }}
              />
            )}
          </div>

          {kind === "typed" && typed ? (
            <>
              {typed.fields.map((f, i) => (
                <Fragment key={`${typed.id}-${f.slot}-${gen}`}>
                  <span className="t-label-m">{f.name}</span>
                  <RichField
                    ref={(h) => {
                      tf.current[i] = h;
                    }}
                    initial={init[i]}
                    minHeight={44}
                    toolbar={refs.some((r) => r.name === f.name)}
                    onInput={refresh}
                    extraTool={
                      refs.some((r) => r.cloze && r.name === f.name) ? (
                        <Button
                          variant="secondary"
                          size="M"
                          label="빈칸 만들기"
                          shortcut="Ctrl Shift C"
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => {
                            makeCloze(tf.current[i]?.el());
                            refresh();
                          }}
                        />
                      ) : undefined
                    }
                  />
                  {i === 0 && chip}
                </Fragment>
              ))}
              <p className="t-caption c-tertiary">칸은 노트 유형의 필드 순서대로예요. 비운 칸은 그 칸을 쓰는 카드에서만 빠져요.</p>
            </>
          ) : kind === "occlusion" ? (
            <div className="addcard__io">
              <input ref={picker} type="file" accept="image/png,image/jpeg,image/webp,image/bmp" hidden onChange={(e) => takeImage(e.target.files)} />
              {!pic ? (
                <div
                  className={`addcard__io-drop ${over ? "is-over" : ""}`}
                  tabIndex={0}
                  onPaste={(e) => takeImage(e.clipboardData.files)}
                  onDragOver={(e) => {
                    e.preventDefault();
                    setOver(true);
                  }}
                  onDragLeave={() => setOver(false)}
                  onDrop={(e) => {
                    e.preventDefault();
                    setOver(false);
                    takeImage(e.dataTransfer.files);
                  }}
                >
                  <Icon name="image" size={32} className="c-tertiary" />
                  <span className="t-body-m c-secondary">그림을 끌어놓거나 붙여넣어요 (Ctrl V)</span>
                  <Button variant="secondary" size="M" label="그림 고르기" onClick={() => picker.current?.click()} />
                  <span className="t-caption c-tertiary">PNG · JPG · WEBP. 그림은 원본 그대로 이 기기에 복사해요.</span>
                </div>
              ) : (
                <>
                  <span className="addcard__io-head">
                    <span className="t-label-m">그림</span>
                    <span className="t-caption c-tertiary">
                      {pic.fileName} · {pic.width}×{pic.height}
                    </span>
                  </span>
                  <div className="addcard__io-pic">
                    <OcclusionPicture url={pic.url} shapes={shapes} hideAll={hideAll} />
                  </div>
                  <div className="addcard__io-sum">
                    <span>
                      <span className="t-body-m-strong">
                        칸 {n(boxCount(shapes))}개 · 카드 {n(ordinalsOf(shapes).length)}장
                      </span>{" "}
                      <span className="t-caption c-secondary">{hideAll ? "모두 가리고 하나씩 묻기" : "하나만 가리기"}</span>
                    </span>
                    <Button variant="secondary" size="M" icon="pencil" label="가릴 곳 편집" onClick={() => setEditing(true)} />
                  </div>
                  <span className="t-caption c-tertiary">질문·추가 설명 칸은 선택이에요. 비워 두면 "가린 곳에 들어갈 말은?"으로 물어요.</span>
                </>
              )}
              <span className="t-label-m">질문</span>
              <RichField key={`q-io-${gen}`} ref={q} initial={init[0]} minHeight={44} toolbar={false} onInput={refresh} />
              {chip}
              <span className="addcard__label">
                <span className="t-label-m">추가 설명</span>
                <span className="t-caption c-tertiary">선택 · 답을 본 뒤에만 보여요</span>
              </span>
              <RichField key={`a-io-${gen}`} ref={a} initial={init[1]} minHeight={44} toolbar={false} onInput={refresh} />
            </div>
          ) : kind === "written" ? (
            <>
              <span className="t-label-m">질문</span>
              <RichField key={`q-written-${gen}`} ref={q} initial={init[0]} minHeight={64} onInput={refresh} />
              {chip}
              {ioLink(q, 0)}
              <span className="t-label-m">모범 답안</span>
              <RichField key={`a-written-${gen}`} ref={a} initial={init[1]} minHeight={84} toolbar={false} onInput={refresh} />
              <span className="addcard__label">
                <span className="t-label-m">채점 메모</span>
                <span className="t-caption c-tertiary">선택 · 답을 본 뒤에 함께 보여요</span>
              </span>
              <RichField key={`m-written-${gen}`} ref={memo} initial={init[2]} minHeight={44} toolbar={false} onInput={refresh} />
              <KeywordEditor
                keywords={kws}
                suggestions={suggestions}
                onChange={setKws}
                onDismiss={(w) => setDismissed((d) => (d.includes(w) ? d : [...d, w]))}
              />
              <label className="edit__write">
                <Toggle on={write} onChange={setWrite} label="공부할 때 직접 써서 답하기" />
                <span className="t-body-s c-secondary">공부할 때 직접 써서 답하기</span>
              </label>
            </>
          ) : isRecall(kind) ? (
            <>
              <span className="t-label-m">질문</span>
              <RichField key={`q-${kind}-${gen}`} ref={q} initial={init[0]} minHeight={44} toolbar={false} onInput={refresh} />
              {chip}
              {kind === "list" ? <ListEditor value={list} onChange={setList} /> : <ChoiceEditor value={choice} onChange={setChoice} />}
              {extraOpen ? (
                <>
                  <span className="addcard__label">
                    <span className="t-label-m">추가 설명</span>
                    <span className="t-caption c-tertiary">선택 · 답을 본 뒤에만 보여요</span>
                  </span>
                  <RichField key={`a-${kind}-${gen}`} ref={a} initial={init[1]} minHeight={44} toolbar={false} onInput={refresh} />
                </>
              ) : (
                <button className="lc__more t-label-m c-secondary" onClick={() => setExtraOpen(true)}>
                  <Icon name="plus" size={14} />
                  추가 설명 · 답을 본 뒤에만 보여요
                </button>
              )}
            </>
          ) : kind !== "cloze" ? (
            <>
              {/* 앞뒤 둘 다는 어느 쪽도 질문이 되므로 앞면 · 뒷면. 질문과 답변을 오가도 쓴 내용은 남는다 */}
              <span className="t-label-m">{kind === "reversed" ? "앞면" : "질문"}</span>
              <RichField key={`q-basic-${gen}`} ref={q} initial={init[0]} minHeight={64} onInput={refresh} />
              {chip}
              {ioLink(q, 0)}
              <span className="t-label-m">{kind === "reversed" ? "뒷면" : "답변"}</span>
              <RichField key={`a-basic-${gen}`} ref={a} initial={init[1]} minHeight={84} onInput={refresh} />
              {ioLink(a, 1)}
              {kind === "reversed" && (
                <span className="addcard__chips">
                  <span className="t-caption c-tertiary">앞→뒤, 뒤→앞으로 묻는 카드가 각각 생겨요</span>
                  <Badge tone="accent">1 · 앞면 → 뒷면</Badge>
                  <Badge tone="accent">2 · 뒷면 → 앞면</Badge>
                </span>
              )}
              <DropZone onImage={(tag) => a.current?.insert(tag)} />
            </>
          ) : (
            <>
              <span className="addcard__label">
                <span className="t-label-m">본문</span>
                <span className="t-caption c-tertiary">단어를 선택하고 빈칸으로 만드세요. 빈칸 번호마다 카드 1장이 생기고, 같은 번호는 한 카드에서 함께 물어요.</span>
              </span>
              <RichField
                key={`q-cloze-${gen}`}
                ref={q}
                initial={init[0]}
                minHeight={96}
                onInput={refresh}
                extraTool={
                  <Button
                    variant="secondary"
                    size="M"
                    label="빈칸 만들기"
                    shortcut="Ctrl Shift C"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      makeCloze(q.current?.el());
                      refresh();
                    }}
                  />
                }
              />
              {chip}
              {ioLink(q, 0)}
              {marks.length > 0 && (
                <span className="addcard__chips">
                  <span className="t-caption c-tertiary">만들어질 카드</span>
                  {marks.map((m, i) => (
                    <Badge key={i} tone="accent">
                      {i + 1} · {m}
                    </Badge>
                  ))}
                </span>
              )}
              <span className="addcard__label">
                <span className="t-label-m">추가 설명</span>
                <span className="t-caption c-tertiary">선택 · 답을 본 뒤에만 보여요</span>
              </span>
              <RichField key={`a-cloze-${gen}`} ref={a} initial={init[1]} minHeight={44} toolbar={false} onInput={refresh} />
            </>
          )}

          <div className="addcard__importance">
            <span className="addcard__importance-label">
              <span className="t-label-m">중요도</span>
              <span className="t-caption c-tertiary">
                {importance === null
                  ? `하위 덱 설정을 따르는 중 · ${LEVEL_LABEL[inherited]}`
                  : `직접 지정 · 같은 칸을 다시 누르면 하위 덱 설정(${LEVEL_LABEL[inherited]})으로`}
              </span>
            </span>
            <div className="segment segment--boxed">
              {([2, 1, 0] as const).map((lvl) => (
                <button
                  key={lvl}
                  className={`segment__item t-label-m ${shownLevel === lvl ? "is-selected" : ""}`}
                  onClick={() => setImportance(importance === lvl ? null : lvl)}
                >
                  {LEVEL_LABEL[lvl]}
                </button>
              ))}
            </div>
          </div>

          <div className="addcard__submit">
            <Checkbox checked={keepGoing} label="저장 후 바로 다음 카드" onChange={setKeepGoing} />
            <span className="addcard__submit-right">
              {error && <span className="t-body-s c-danger">{error}</span>}
              <Button icon="check" label="저장" shortcut={actionKey("save")} onClick={save} />
            </span>
          </div>
        </div>

        <aside className="addcard__side">
          <p className="t-caption c-tertiary">
            학습할 때 이렇게 보여요
            {kind === "typed"
              ? typedCards > 0 && ` · 카드 ${n(typedCards)}장`
              : kind === "cloze" && marks.length > 0
              ? ` · 카드 1 / ${marks.length}`
              : kind === "reversed"
                ? " · 카드 2장"
                : kind === "occlusion" && shapes.length > 0
                  ? ` · 카드 ${ordinalsOf(shapes).length}장`
                  : kind === "list"
                    ? " · 카드 1장"
                    : kind === "choice"
                      ? " · 질문 면"
                      : ""}
          </p>
          {kind === "typed" && typed ? (
            <TypedPreview type={typed} values={typedValues} />
          ) : kind === "occlusion" ? (
            pic &&
            ordinalsOf(shapes)
              .slice(0, 3)
              .map((o) => (
                <div key={o} className="addcard__io-card">
                  <p className="t-caption c-secondary">
                    카드 {o}
                    {labelOf(shapes, o) && ` · ${labelOf(shapes, o)}`}
                  </p>
                  <OcclusionPicture url={pic.url} shapes={shapes} hideAll={hideAll} ask={o} />
                </div>
              ))
          ) : isRecall(kind) ? (
            kind === "list" ? (
              <ListPreview question={withMath(sanitize(qHtml))} draft={list} openKey={actionKey("reveal")} />
            ) : (
              <ChoicePreview question={withMath(sanitize(qHtml))} draft={choice} openKey={actionKey("reveal")} />
            )
          ) : kind === "reversed" ? (
            <>
              <Preview kind="basic" qHtml={qHtml} aHtml={a.current?.html() ?? ""} level={shownLevel} head="카드 1 · 앞 → 뒤" />
              <Preview kind="basic" qHtml={a.current?.html() ?? ""} aHtml={qHtml} level={shownLevel} head="카드 2 · 뒤 → 앞" />
              <p className="addcard__note t-caption c-secondary">
                <Icon name="info" size={14} />
                두 장은 같은 날 연달아 나오지 않아요. 하나를 보면 나머지는 다음 날로 미뤄요.
              </p>
            </>
          ) : (
            <Preview kind={kind} qHtml={qHtml} aHtml={a.current?.html() ?? ""} level={shownLevel} />
          )}
          {mathErrors.length > 0 && (
            <div className="addcard__matherr" role="status">
              <Icon name="alert" size={16} className="c-danger" />
              <span className="addcard__matherr-text">
                <span className="t-body-m-strong c-danger">수식 {n(mathErrors.length)}개를 읽지 못했어요</span>
                <span className="t-body-s c-secondary">
                  {mathErrors.map((e) => `${e.field} ${e.line}번째 줄 · ${e.message}`).join(" / ")}. 고치지 않아도 저장되고, 학습할 때는 원문
                  그대로 보여요.
                </span>
              </span>
            </div>
          )}
          {added.length > 0 && (
            <div className="addcard__recent">
              <p className="t-caption c-tertiary">방금 추가한 카드</p>
              {added.map((t, i) => (
                <p key={i} className="addcard__recent-item t-body-s c-secondary">
                  <Icon name="check" size={14} className="c-success" />
                  <span>{t || "(내용 없음)"}</span>
                </p>
              ))}
            </div>
          )}
        </aside>
      </div>
      )}

      {editing && pic && (
        <OcclusionEditor
          picture={pic}
          shapes={shapes}
          hideAll={hideAll}
          onCancel={() => {
            setEditing(false);
            if (!shapes.length) setPic(null); // 처음 그리다 버리면 그림 고르기로 돌아간다
          }}
          onDone={(s, h) => {
            setShapes(s);
            setHideAll(h);
            setEditing(false);
          }}
        />
      )}
      {leaving && (
        <DiscardModal
          title="작성 중인 카드가 있어요"
          text="저장하지 않고 나갈까요?"
          keep="계속 쓰기"
          discard="버리고 나가기"
          onKeep={() => setLeaving(null)}
          onDiscard={() => nav(leaving, true)}
        />
      )}
      {moving && (
        <DiscardModal
          title="옮기지 못하는 칸이 있어요"
          text={`${moving.lost.join(" · ")} 칸은 ${moving.to}에 없어요. 바꾸면 그 칸에 쓴 내용은 지워져요.`}
          keep="그대로 두기"
          discard="바꾸기"
          onKeep={() => setMoving(null)}
          onDiscard={moving.go}
        />
      )}
    </main>
  );
}

// ---------------------------------------------------------------- NT6 [⋯] 노트 유형 메뉴

function TypeMenu({ open, setOpen, types, deckTypes, current, onPick, onEdit, onManage }: {
  open: boolean;
  setOpen: (v: boolean) => void;
  types: NoteTypeRow[];
  /** 이 덱(하위 덱 포함) 노트가 쓰는 유형, 많이 쓰는 것 먼저 */
  deckTypes: number[];
  current: number | undefined;
  onPick: (r: NoteTypeRow) => void;
  onEdit: () => void;
  onManage: () => void;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  // 떠 있는 동안 화면 단축키는 꺼지고 Esc 는 메뉴만 닫는다 (C1-m 과 같다)
  useOverlayScope(open);
  useKey(
    "Escape",
    () => {
      setOpen(false);
      trigger.current?.focus();
    },
    open,
    { modal: true },
  );
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open, setOpen]);

  const name = (r: NoteTypeRow) => (r.native_kind ? (TYPE_LABEL[r.native_kind] ?? r.name) : r.name);
  const mine = deckTypes.map((id) => types.find((r) => r.id === id)).filter((r): r is NoteTypeRow => !!r);
  // 앱 기본 유형은 세그먼트에 있다
  const others = types.filter((r) => !r.native_kind && !deckTypes.includes(r.id));
  const item = (r: NoteTypeRow, caption: string, first: boolean) => (
    <button
      key={r.id}
      role="menuitem"
      className={`menu__item t-body-m ${r.id === current ? "is-active" : ""}`}
      autoFocus={first}
      onClick={() => onPick(r)}
    >
      <span className="menu__text">
        <span className="t-label-m">{name(r)}</span>
        <span className="t-caption c-tertiary">{caption}</span>
      </span>
      {r.id === current && <Icon name="check" size={16} className="c-accent" />}
    </button>
  );
  const shape = (r: NoteTypeRow) => (r.cloze ? "빈칸" : `필드 ${n(r.fields)}개`);
  return (
    <span className="menu addcard__more">
      <button
        ref={trigger}
        className="addcard__more-btn"
        aria-label="다른 노트 유형"
        title="다른 노트 유형"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => {
          e.stopPropagation();
          setOpen(!open);
        }}
      >
        <Icon name="more" size={18} />
      </button>
      {open && (
        <div className="menu__list menu--panel addcard__types" role="menu" aria-label="노트 유형" onKeyDown={menuKeys} onClick={(e) => e.stopPropagation()}>
          {mine.length > 0 && <p className="menu__cap t-label-s c-tertiary">이 덱이 쓰는 노트 유형</p>}
          {mine.map((r, i) => item(r, `${shape(r)} · ${r.cloze ? "빈칸마다 카드 1장" : `카드 ${n(r.templates)}장씩`}`, i === 0))}
          {others.length > 0 && <p className="menu__cap t-label-s c-tertiary">다른 노트 유형</p>}
          {others.map((r, i) =>
            item(r, `${shape(r)} · ${r.imported ? "Anki에서 가져옴" : "내가 만든 것"} · ${r.decks[0] ? `${r.decks[0]}에서 씀` : "아직 쓰는 덱이 없어요"}`, !mine.length && i === 0),
          )}
          {(mine.length > 0 || others.length > 0) && <div className="menu__sep" />}
          <button role="menuitem" className="menu__item t-body-m" disabled={current === undefined} autoFocus={!mine.length && !others.length} onClick={onEdit}>
            <Icon name="pencil" size={16} scaled />
            <span className="menu__text t-label-m">이 유형 편집</span>
          </button>
          <button role="menuitem" className="menu__item t-body-m menu__item--top" onClick={onManage}>
            <Icon name="settings" size={16} scaled />
            <span className="menu__text">
              <span className="t-label-m">노트 유형 관리</span>
              <span className="t-caption c-tertiary">설정 › 노트 유형</span>
            </span>
          </button>
        </div>
      )}
    </span>
  );
}

// ---------------------------------------------------------------- NT6 사용자 · 가져온 유형 미리보기 (카드 종류마다 앞면)

/**
 * 학습 화면과 같은 길: render.rs(preview_template) → 살균 · 소리 칩 · 그림(renderCardHtml) → 수식(withMath), 노트 CSS 는 미리보기 안에 가둔다 (scopeCss).
 * 다크 테마의 덱 카드 면도 학습 화면과 같다 (deckFace). values 는 저장할 HTML (보이는 필드 순서)
 */
function TypedPreview({ type, values }: { type: NoteTypeDetail; values: string[] }) {
  const [cards, setCards] = useState<{ label: string; html: string }[]>([]);
  const key = JSON.stringify(values);
  useEffect(() => {
    let live = true;
    const t = setTimeout(async () => {
      // 빈칸 유형은 빈칸 번호마다 (템플릿 하나), 일반 유형은 카드 종류마다
      const shown = type.cloze
        ? clozeNumbers(values).map((c, i) => ({ tpl: type.templates[0], ord: c - 1, label: `카드 ${i + 1} · 빈칸 ${c}` }))
        : type.templates.map((tpl, i) => ({ tpl, ord: tpl.ord, label: /^(card|카드)\s*\d/i.test(tpl.name) ? tpl.name : `카드 ${i + 1} · ${tpl.name}` }));
      try {
        const out = await Promise.all(
          shown.slice(0, 4).map(async ({ tpl, ord, label }) => {
            const p = await api.previewTemplate(type.id, tpl.qfmt, tpl.afmt, ord, values);
            return { label, html: withMath(await renderCardHtml(p.front)) };
          }),
        );
        if (live) setCards(out);
      } catch {
        if (live) setCards([]);
      }
    }, 150);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [type, key]); // values 는 key(JSON) 로 본다
  const dark = document.documentElement.dataset.theme === "dark";
  const face = dark && type.css.trim() ? deckFace(type.css) : null;
  const css = useMemo(() => scopeCss(type.css, face === "other", ".addcard__nt-card"), [type.css, face]);
  const empty = !values.some(filled);
  return (
    <div className="preview addcard__nt" data-theme={face === "light" ? "light" : undefined}>
      {css && <style>{css}</style>}
      {empty || !cards.length ? (
        <p className="addcard__nt-empty t-title-m c-tertiary">{empty ? "입력하면 여기에 보여요" : type.cloze ? "빈칸을 만들면 카드가 보여요" : ""}</p>
      ) : (
        cards.map((c, i) => (
          <Fragment key={i}>
            {i > 0 && <div className="preview__rule addcard__nt-rule" />}
            <p className="t-caption c-tertiary">{c.label} · 앞면</p>
            <div className={`addcard__nt-card card-html ${i > 0 ? "is-sub" : ""}`}>
              <div className={face === "night" ? "nightMode night_mode" : undefined} dangerouslySetInnerHTML={{ __html: c.html }} />
            </div>
          </Fragment>
        ))
      )}
    </div>
  );
}

// ---------------------------------------------------------------- M1 "이 그림으로 가리기 카드 만들기"

/** 편집기에 붙인 마지막 그림 (미디어 폴더에 저장된 것). 움직이는 GIF · SVG · HEIC 는 가리기에 쓰지 않는다 (M7) */
function lastImage(html: string) {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  const img = [...tpl.content.querySelectorAll<HTMLImageElement>("img[data-media]")].pop();
  const name = img?.dataset.media ?? "";
  return img && name && !/\.(gif|svg|heic|heif)$/i.test(name) ? { name, url: img.getAttribute("src") ?? "" } : null;
}

/** 그림 아래 링크 (M1 노트). 그림이 없으면 그리지 않는다 */
function OcclusionLink({ html, onPick }: { html: string; onPick: (img: { name: string; url: string }) => void }) {
  const img = lastImage(html);
  if (!img) return null;
  return (
    <button className="addcard__io-link t-label-m c-accent" onClick={() => onPick(img)}>
      <Icon name="image" size={14} />
      이 그림으로 가리기 카드 만들기
    </button>
  );
}

// ---------------------------------------------------------------- 미리보기

/** head 를 주면 C3-r 의 작은 미리보기 (카드 1 · 앞 → 뒤) */
function Preview({ kind, qHtml, aHtml, level, head }: { kind: Kind; qHtml: string; aHtml: string; level: Level; head?: string }) {
  let question: string;
  let answer: string;
  if (kind === "cloze") {
    const marks = clozeMarks(qHtml);
    question = previewCloze(qHtml);
    answer = marks.length ? `정답 · ${marks[0]}` : "";
    if (plainText(aHtml)) answer += `<br>${sanitize(aHtml)}`;
  } else {
    question = sanitize(qHtml);
    answer = sanitize(aHtml);
  }
  // 수식은 살균한 뒤에 그린다 (math.ts)
  question = withMath(question);
  answer = withMath(answer);
  const empty = !plainText(question) && !question.includes("<img");
  return (
    <div className={`preview ${head ? "preview--small" : ""}`}>
      <div className="preview__head">
        <span className="t-label-s c-tertiary">{head ?? "미리보기"}</span>
        <Importance level={level} />
      </div>
      <div
        className={`preview__q t-title-m card-html ${empty ? "c-tertiary" : ""}`}
        dangerouslySetInnerHTML={{ __html: empty ? (head ? "입력하면 여기에 보여요" : "질문을 입력하면 여기에 보여요") : question }}
      />
      <div className="preview__rule" />
      <div className="preview__a t-body-m c-secondary card-html" dangerouslySetInnerHTML={{ __html: answer }} />
    </div>
  );
}

// ---------------------------------------------------------------- 서식 입력 칸

/**
 * 편집기 안 수식 표시 (C3-m). DOM 을 바꾸지 않는 CSS Custom Highlight 로 칠한다.
 * 그래서 표시용 요소가 저장 HTML 에 섞일 일이 없고, 입력 중 커서 · 실행 취소도 그대로다.
 * 한계: ::highlight 는 글꼴을 바꿀 수 없어 고정폭 글꼴은 못 쓴다 (배경 · 글자색 · 밑줄만)
 */
const mathRanges = new Map<HTMLElement, { ok: Range[]; bad: Range[] }>();
function paintMath() {
  if (!("highlights" in CSS)) return; // 지원하지 않으면 표시만 없다
  const all = [...mathRanges.values()];
  CSS.highlights.set("math", new Highlight(...all.flatMap((r) => r.ok)));
  CSS.highlights.set("math-error", new Highlight(...all.flatMap((r) => r.bad)));
}
const escapeText = (t: string) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
/** 글자 노드 하나 안의 수식만 (그리기와 같은 규칙: 서식이 끼어 노드가 나뉜 수식은 수식이 아니다) */
function markMath(el: HTMLElement | null) {
  if (!el) return;
  const ok: Range[] = [];
  const bad: Range[] = [];
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    for (const part of findMath(n.textContent ?? "")) {
      const r = document.createRange();
      r.setStart(n, part.start);
      r.setEnd(n, part.end);
      (checkMath(escapeText(part.source)).length ? bad : ok).push(r);
    }
  }
  mathRanges.set(el, { ok, bad });
  paintMath();
}

export type FieldHandle = { html: () => string; clear: () => void; focus: () => void; insert: (html: string) => void; el: () => HTMLDivElement | null };

export const RichField = forwardRef<
  FieldHandle,
  { minHeight: number; onInput: () => void; toolbar?: boolean; extraTool?: React.ReactNode; initial?: string }
>(
  function RichField({ minHeight, onInput, toolbar = true, extraTool, initial }, ref) {
    const box = useRef<HTMLDivElement>(null);
    const file = useRef<HTMLInputElement>(null);
    useKeymap();
    const mathKey = getKeys("math")[0] ?? "";
    const mathTip = mathKey
      ? `수식 넣기 ${actionKey("math")} · 블록 수식 ${actionKey("math").replace(/^((?:Ctrl )?(?:Alt )?)/, "$1Shift ")}`
      : "수식 넣기";

    // 내용이 바뀔 때마다 수식 표시를 다시 칠하고 부모에 알린다
    const changed = () => {
      markMath(box.current);
      onInput();
    };
    useEffect(() => {
      const el = box.current;
      return () => {
        if (el) mathRanges.delete(el);
        paintMath();
      };
    }, []);

    // 저장된 HTML 로 시작한다 (카드 편집). 그림 파일 이름을 화면용 주소로 바꾼다
    useEffect(() => {
      if (!initial) return;
      renderCardHtml(initial).then((html) => {
        if (box.current) box.current.innerHTML = html;
        changed(); // 미리보기 · 유형 추천도 옮겨 온 내용으로
      });
    }, [initial]);

    const insert = (html: string) => {
      const el = box.current;
      if (!el) return;
      el.focus();
      const sel = window.getSelection();
      if (!sel || !el.contains(sel.anchorNode)) {
        const range = document.createRange();
        range.selectNodeContents(el);
        range.collapse(false);
        sel?.removeAllRanges();
        sel?.addRange(range);
      }
      document.execCommand("insertHTML", false, html);
      changed();
    };

    /** C3-m: 고른 글자를 \( \) (블록은 \[ \]) 로 감싼다. 고른 게 없으면 빈 괄호 안에 커서 */
    const wrapMath = (block: boolean) => {
      const el = box.current;
      if (!el) return;
      const sel = window.getSelection();
      if (!sel || !el.contains(sel.anchorNode)) insert("");
      const text = window.getSelection()?.toString() ?? "";
      const [open, close] = block ? ["\\[", "\\]"] : ["\\(", "\\)"];
      document.execCommand("insertText", false, open + text + close);
      if (!text) for (let i = 0; i < close.length; i++) window.getSelection()?.modify("move", "backward", "character");
      changed();
    };

    useImperativeHandle(ref, () => ({
      html: () => box.current?.innerHTML ?? "",
      clear: () => {
        if (box.current) box.current.innerHTML = "";
        markMath(box.current);
      },
      focus: () => box.current?.focus(),
      insert,
      el: () => box.current,
    }));

    const cmd = (c: string) => {
      box.current?.focus();
      document.execCommand(c);
      changed();
    };

    const addFiles = async (files: FileList | File[]) => {
      for (const f of Array.from(files).filter((f) => f.type.startsWith("image/"))) insert(await imageTag(f));
    };

    return (
      <div className="rich">
        {toolbar && (
          <div className="rich__tools" onMouseDown={(e) => e.target !== file.current && e.preventDefault()}>
            <button className="rich__tool t-label-m" aria-label="굵게" onClick={() => cmd("bold")}>
              B
            </button>
            <button className="rich__tool t-label-m" aria-label="기울임" onClick={() => cmd("italic")}>
              I
            </button>
            <button className="rich__tool t-label-m" aria-label="밑줄" onClick={() => cmd("underline")}>
              U
            </button>
            <button className="rich__tool" aria-label="목록" onClick={() => cmd("insertUnorderedList")}>
              <Icon name="list" size={16} scaled />
            </button>
            <button className="rich__tool" aria-label="이미지" onClick={() => file.current?.click()}>
              <Icon name="image" size={16} scaled />
            </button>
            <button className="rich__tool rich__math" aria-label={mathTip} title={mathTip} onClick={() => wrapMath(false)}>
              <span className="rich__sum">∑</span>
              <span className="t-label-s">수식</span>
            </button>
            <input ref={file} type="file" accept="image/*" multiple hidden onChange={(e) => e.target.files && addFiles(e.target.files)} />
            {extraTool && <span className="rich__extra">{extraTool}</span>}
          </div>
        )}
        <div
          ref={box}
          className="rich__body t-body-l card-html"
          style={{ minHeight }}
          contentEditable
          suppressContentEditableWarning
          onInput={changed}
          onKeyDown={(e) => {
            // 수식 넣기 (기본 Ctrl M), 같은 키 + Shift 는 블록 수식. 키는 설정 > 단축키에서 바꿀 수 있다
            const c = chordOf(e.nativeEvent);
            const m = getKeys("math")[0];
            if (!c || !m) return;
            const block = !m.includes("Shift+") && c === m.replace(/^((?:Ctrl\+)?(?:Alt\+)?)/, "$1Shift+");
            if (c === m || block) {
              e.preventDefault();
              wrapMath(block);
            }
          }}
          onPaste={(e) => {
            const imgs = Array.from(e.clipboardData.files).filter((f) => f.type.startsWith("image/"));
            if (imgs.length) {
              e.preventDefault();
              addFiles(imgs);
            }
          }}
        />
      </div>
    );
  },
);

function DropZone({ onImage }: { onImage: (tag: string) => void }) {
  const [over, setOver] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const take = async (files: FileList) => {
    for (const f of Array.from(files).filter((f) => f.type.startsWith("image/"))) onImage(await imageTag(f));
  };
  return (
    <button
      className={`dropzone ${over ? "is-over" : ""}`}
      onClick={() => input.current?.click()}
      onDragOver={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        take(e.dataTransfer.files);
      }}
    >
      <Icon name="image" size={18} className="c-tertiary" />
      <span className="t-body-s c-tertiary">
        이미지를 끌어오거나 붙여넣기 (Ctrl V) · 여러 장 가능
      </span>
      <input ref={input} type="file" accept="image/*" multiple hidden onChange={(e) => e.target.files && take(e.target.files)} />
    </button>
  );
}

// ---------------------------------------------------------------- 빈칸

/** 선택한 글자를 빈칸 표시로 감싼다. Cloze 문법은 화면에 드러내지 않는다 (브리프 3.5) */
export function makeCloze(el: HTMLDivElement | null | undefined) {
  const sel = window.getSelection();
  if (!el || !sel || sel.isCollapsed || !el.contains(sel.anchorNode)) return;
  const text = sel.toString();
  if (!text.trim()) return;
  const esc = text.replace(/&/g, "&amp;").replace(/</g, "&lt;");
  document.execCommand("insertHTML", false, `<span class="cloze-mark">${esc}</span>&#8203;`);
}

function marksOf(html: string) {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  return { tpl, marks: Array.from(tpl.content.querySelectorAll("span.cloze-mark")) };
}

function clozeMarks(html: string): string[] {
  return marksOf(html).marks.map((m) => (m.textContent ?? "").trim());
}

/**
 * 편집기 HTML -> Anki Cloze 문법 ({{c1::...}}).
 * 편집할 때 받은 번호(data-c)는 지킨다. 번호가 바뀌면 다른 카드의 학습 기록이 엉뚱한 빈칸에 붙는다
 */
export function toCloze(html: string, firstFree = 1): string {
  const { tpl, marks } = marksOf(toStoredHtml(html).replace(/​/g, ""));
  const used = new Set(marks.map((m) => Number(m.getAttribute("data-c"))).filter((c) => c > 0));
  let next = firstFree; // 편집할 때는 원래 노트의 가장 큰 번호 다음부터 (EditNote)
  marks.forEach((m) => {
    let c = Number(m.getAttribute("data-c"));
    if (!(c > 0)) {
      while (used.has(next)) next++;
      c = next;
      used.add(c);
    }
    const hint = m.getAttribute("data-hint");
    m.replaceWith(`{{c${c}::${m.textContent ?? ""}${hint ? `::${hint}` : ""}}}`);
  });
  return tpl.innerHTML;
}

/** Anki Cloze 문법 -> 편집기 표시. 번호와 힌트를 속성으로 남긴다 */
export function fromCloze(html: string): string {
  const attr = (v: string) => v.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
  return html.replace(
    /\{\{c(\d+)::([\s\S]*?)(?:::([\s\S]*?))?\}\}/g,
    (_, c: string, body: string, hint?: string) =>
      `<span class="cloze-mark" data-c="${c}"${hint ? ` data-hint="${attr(hint)}"` : ""}>${body}</span>`,
  );
}

function clozePlain(html: string): string {
  const { tpl, marks } = marksOf(html.replace(/​/g, ""));
  marks.forEach((m) => m.replaceWith(`[${m.textContent}]`));
  return tpl.innerHTML;
}

function previewCloze(html: string): string {
  const { tpl, marks } = marksOf(sanitize(html));
  marks.forEach((m, i) => {
    if (i === 0) {
      const span = document.createElement("span");
      span.className = "cloze";
      span.textContent = "[      ]";
      m.replaceWith(span);
    } else m.replaceWith(m.textContent ?? "");
  });
  return tpl.innerHTML;
}
