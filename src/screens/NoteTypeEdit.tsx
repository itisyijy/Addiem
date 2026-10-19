// NT2 쉬운 편집 · NT3 HTML (쉬운 편집 잠김) · NT4 앱 기본 유형 보기 · NT5 저장할 때 확인과 오류.
// 초안을 화면에 들고 있다가 [저장]에서 한 트랜잭션으로 쓴다 (save_note_type). 먼저 dry-run 으로 확인 창의 수를 센다
import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type KeyboardEvent, type ReactNode } from "react";
import {
  api, errorText,
  type Layout, type Look, type NoteTypeConflict, type NoteTypeDetail as Detail, type NoteTypeEdit as TypeEdit, type NoteTypeImpact as SaveImpact,
  type NoteTypeSample as Sample, type Part,
} from "../api";
import { useSyncStatus, when } from "../sync";
import { Badge, Button, Checkbox, Icon, type IconName } from "../ui";
import { iga, n, useKey, useLeaveGuard, useNav, useOverlayScope, type Route } from "../nav";
import { actionKey, matchAction } from "../keys";
import { useToast } from "../toast";
import { plainText, renderCardHtml } from "../cardHtml";
import { withMath } from "../math";
import { deckFace, scopeCss } from "../cardCss";
import { splitBack } from "./Study";
import { DiscardModal, Modal } from "./NewDeck";
import { CloneDialog, typeLabel } from "./NoteTypes";
import { nativeLabels } from "./AddCard";
import { menuKeys } from "./Deck";
import "../styles/notetypes.css";
import "../styles/study.css";

type Side = "front" | "back";
type DField = { key: number; slot: number | null; name: string };
type DTpl = { key: number; ord: number | null; name: string; qfmt: string; afmt: string; layout: Layout | null; reason: string | null };
type Draft = { fields: DField[]; tpls: DTpl[]; css: string };

const SIDE_LABEL: Record<Side, string> = { front: "앞면", back: "뒷면" };
/** 면의 기본 줄 모양 (notetype.rs Look::base: 학습 화면의 질문 · 답 글자) */
const BASE_LOOK: Record<Side, Look> = { front: { size: "l", align: "center" }, back: { size: "m", align: "left" } };
const SPECIAL = ["FrontSide", "Card", "CardFlag", "CardID", "Deck", "Subdeck", "Tags", "Type"];
/** 읽어주기 언어 (Anki {{tts 언어:필드}}) */
const TTS_LANGS: [string, string][] = [
  ["en_US", "영어"], ["ko_KR", "한국어"], ["ja_JP", "일본어"], ["zh_CN", "중국어"], ["de_DE", "독일어"], ["fr_FR", "프랑스어"], ["es_ES", "스페인어"],
];

/** 템플릿이 쓰는 필드 이름 (notetype.rs refs 와 같은 읽기) */
function refsOf(tpl: string): string[] {
  const out: string[] = [];
  for (const m of tpl.matchAll(/\{\{(.*?)\}\}/gs)) {
    const t = m[1].trim();
    const name = (/^[#^/]/.test(t) ? t.slice(1) : t.slice(t.lastIndexOf(":") + 1)).trim();
    if (name && !SPECIAL.includes(name) && !out.includes(name)) out.push(name);
  }
  return out;
}
const isCloze = (s: string) => /\{\{[^}]*cloze:/i.test(s);
const usesBox = (t: DTpl) => (t.qfmt + t.afmt).includes('<div class="nt-');

/** 필드 이름 검사 (notetype.rs field_name 과 같은 규칙) */
function fieldNameError(name: string, others: string[]) {
  const v = name.trim();
  if (!v) return "필드 이름을 입력해 주세요.";
  if (/[:{}"]/.test(v) || /^[#/^]/.test(v)) return '필드 이름에는 : { } " 를 쓸 수 없고, # / ^ 로 시작할 수 없어요.';
  if (SPECIAL.includes(v)) return `'${v}'은 Anki 가 따로 쓰는 이름이에요.`;
  if (others.some((o) => o.toLowerCase() === v.toLowerCase())) return `'${v}' 필드가 이미 있어요.`;
  return null;
}

/** 카드 종류 탭 이름 "카드 1 · 단어 → 뜻". Anki 기본 이름(Card 1 · Cloze)이면 앞면 첫 필드 → 뒷면 첫 필드로 */
/** show: 화면 필드 이름 (앱 기본 유형은 "질문 · 답변" 처럼, NT51) */
type Show = (name: string) => string;
const same: Show = (x) => x;
function tplTitle(t: DTpl, i: number, cloze: boolean, show: Show = same) {
  return `카드 ${i + 1} · ${tplName(t, cloze, show)}`;
}
function tplName(t: DTpl, cloze: boolean, show: Show = same) {
  if (t.name && !/^(Card \d+|Cloze)$/.test(t.name)) return t.name;
  return autoName(t, cloze, show);
}
function autoName(t: DTpl, cloze: boolean, show: Show = same) {
  const f = refsOf(t.qfmt);
  if (cloze) return `빈칸${f[0] ? ` · ${show(f[0])}` : ""}`;
  const b = refsOf(t.afmt).find((x) => !f.includes(x));
  return f[0] ? `${show(f[0])}${b ? ` → ${show(b)}` : ""}` : "앞면 없음";
}

// ---------------------------------------------------------------- 쉬운 편집 줄 (칸 묶음)

/** 칸 -> 줄. 줄은 필드 · 빈칸으로 시작하고 입력칸 · 읽어주기가 붙는다. FrontSide 는 한 줄 */
function linesOf(parts: Part[]): Part[][] {
  const out: Part[][] = [];
  for (const p of parts) {
    const last = out[out.length - 1];
    const attach = (p.kind === "type" || p.kind === "tts") && last && last[0].kind !== "front_side";
    if (attach) last.push(p);
    else out.push([p]);
  }
  return out;
}
const lookOf = (p: Part): Look | undefined => (p.kind === "field" || p.kind === "cloze" ? p.look : undefined);
const nameOf = (p: Part) => (p.kind === "front_side" ? "" : p.name);

/** 줄을 빼면 거기 붙은 입력칸 · 읽어주기는 앞 줄(없으면 다음 줄)로 옮긴다 (옵션이 저절로 꺼지지 않게) */
function dropLine(lines: Part[][], i: number) {
  const out = lines.filter((_, j) => j !== i);
  const extra = lines[i].slice(1);
  const j = i - 1 >= 0 && out[i - 1][0].kind !== "front_side" ? i - 1 : i < out.length ? i : -1;
  if (extra.length && j >= 0) out[j] = [...out[j], ...extra];
  return out;
}

// ---------------------------------------------------------------- 화면

export function NoteTypeEdit({ id, ord, note, back }: { id: number; ord?: number; note?: number; back?: Route }) {
  const [d, setD] = useState<Detail | null>(null);
  const [error, setError] = useState<string>();
  const load = useCallback(() => api.noteTypeDetail(id).then(setD, (e) => setError(errorText(e))), [id]);
  useEffect(() => {
    load();
  }, [load]);
  if (!d) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;
  // 다시 불러오면 초안을 새 정의로 처음부터 (key)
  return <Editor key={d.version} d={d} ord={ord} note={note} back={back ?? { name: "noteTypes" }} reload={load} />;
}

/** 저장 거부 문구 (notetype.rs STALE) */
const STALE = "다른 기기에서 이 노트 유형이 바뀌었어요";
/** 같은 자리에 두 기기가 따로 더한 필드 (sync engine KEPT_BOTH). 잃은 것이 없어 진 쪽으로 부르지 않는다. 앞부분으로 옛 기록도 알아본다 */
const KEPT_BOTH = "같은 자리에 두 기기가 필드를 더해 둘 다 남겼어요";
const keptBoth = (c: NoteTypeConflict) => c.what.startsWith("같은 자리에");

const initDraft = (d: Detail): Draft => ({
  fields: d.fields.map((f) => ({ key: f.slot, slot: f.slot, name: f.name })),
  tpls: d.templates.map((t) => ({ key: t.ord, ord: t.ord, name: t.name, qfmt: t.qfmt, afmt: t.afmt, layout: t.layout, reason: t.reason })),
  css: d.css,
});
const draftKey = (x: Draft) =>
  JSON.stringify([x.fields.map((f) => [f.slot, f.name]), x.tpls.map((t) => [t.ord, t.name, t.qfmt, t.afmt]), x.css]);

function Editor({ d, ord, note, back, reload }: { d: Detail; ord?: number; note?: number; back: Route; reload: () => void }) {
  const nav = useNav();
  const toast = useToast();
  const readOnly = !d.editable;
  // Q2-07: 연 뒤 동기화가 이 유형을 바꾸면 낡은 초안으로 저장하지 못하게 (다른 기기가 더한 필드 · 카드 종류를 지우게 된다)
  const [stale, setStale] = useState(false);
  const sync = useSyncStatus();
  const seen = useRef(sync?.changed_seq);
  useEffect(() => {
    const seq = sync?.changed_seq;
    if (seq === undefined || seq === seen.current) return;
    seen.current = seq;
    api.noteTypeDetail(d.id).then((now) => now.version !== d.version && setStale(true), () => {});
  }, [sync?.changed_seq, d.id, d.version]);
  // K39 다른 기기와 동시에 고쳐 진 쪽 정의
  const [conflicts, setConflicts] = useState<NoteTypeConflict[]>([]);
  const [showConflicts, setShowConflicts] = useState(false);
  useEffect(() => {
    if (!readOnly) api.noteTypeConflicts(d.id).then(setConflicts, () => {});
  }, [d.id, readOnly]);
  const unseen = conflicts.filter((c) => !c.seen);
  const lost = unseen.filter((c) => !keptBoth(c)).length;
  const drawn = !d.cloneable && !!d.native_kind; // 목록 · 선택형 · 그림 가리기
  const initial = useMemo(() => initDraft(d), [d]);
  const [draft, setDraft] = useState<Draft>(initial);
  const [curKey, setCurKey] = useState(() => d.templates.find((t) => t.ord === ord)?.ord ?? d.templates[0]?.ord ?? 0);
  const [mode, setMode] = useState<"easy" | "html">("easy");
  const [renaming, setRenaming] = useState<{ what: "field" | "tpl"; key: number; value: string } | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const [leaving, setLeaving] = useState<Route | null>(null);
  const [ask, setAsk] = useState<SaveImpact | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string>();
  const [cloning, setCloning] = useState(false);
  const seq = useRef(-1); // 새 필드 · 카드 종류의 key (음수)
  const area = useRef<{ el: HTMLTextAreaElement; which: "qfmt" | "afmt" } | null>(null); // HTML 탭에서 마지막으로 고친 칸
  const latest = useRef(draft);
  latest.current = draft;
  const dirty = draftKey(draft) !== draftKey(initial);
  const cur = draft.tpls.find((t) => t.key === curKey) ?? draft.tpls[0];
  const curIndex = draft.tpls.indexOf(cur);
  const effMode = cur?.layout ? mode : "html";
  const names = draft.fields.map((f) => f.name);
  // 앱 기본 유형은 카드 추가 · 편집과 같은 칸 이름을 보인다 (DB 이름 Front · Back 은 필드 행 오른쪽 태그 · HTML 탭 그대로, NT51)
  const shown = useMemo(() => {
    const labels = d.native_kind ? nativeLabels(d.native_kind) : undefined;
    return new Map(labels ? d.fields.map((f, i) => [f.name, labels[i] ?? f.name]) : []);
  }, [d]);
  const show: Show = (x) => shown.get(x) ?? x;

  useLeaveGuard((to) => {
    if (!dirty) return false;
    setLeaving(to);
    return true;
  });
  const goBack = useCallback(() => nav(back), [nav, back]);
  const overlay = !!(leaving || ask || cloning || showConflicts);
  useKey("Escape", goBack, !overlay && !renaming && !menu);
  useOverlayScope(menu !== null);
  useKey("Escape", () => setMenu(null), menu !== null, { modal: true });

  const setTpl = useCallback((key: number, patch: Partial<DTpl>) => setDraft((x) => ({ ...x, tpls: x.tpls.map((t) => (t.key === key ? { ...t, ...patch } : t)) })), []);

  // 쉬운 편집: 구조를 바꾸면 Rust 가 HTML 로 쓴다 (build_template_layout). 늦게 온 답은 버린다
  const builds = useRef(new Map<number, number>());
  const setLayout = (key: number, layout: Layout) => {
    setTpl(key, { layout, reason: null });
    const n = (builds.current.get(key) ?? 0) + 1;
    builds.current.set(key, n);
    api.buildTemplateLayout(layout).then((b) => builds.current.get(key) === n && setTpl(key, b));
  };
  // HTML: 고칠 때마다 쉬운 편집이 읽을 수 있는지 다시 본다 (잠금 · 이유 줄)
  const parses = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const setHtml = (key: number, patch: { qfmt?: string; afmt?: string }) => {
    setTpl(key, patch);
    clearTimeout(parses.current.get(key));
    parses.current.set(
      key,
      setTimeout(() => {
        const t = latest.current.tpls.find((y) => y.key === key);
        if (t)
          api.templateLayout(t.qfmt, t.afmt).then(
            (layout) => setTpl(key, { layout, reason: null }),
            (why) => setTpl(key, { layout: null, reason: String(why) }),
          );
      }, 250),
    );
  };

  // ---------------------------------------------------------------- 필드

  /** 이 필드를 쓰는 곳 "카드 1 뒷면 · 카드 2 앞면" (없으면 빈 목록) */
  const usedWhere = (name: string) =>
    draft.tpls.flatMap((t, i) =>
      (["front", "back"] as Side[]).filter((side) => refsOf(side === "front" ? t.qfmt : t.afmt).includes(name)).map((side) => `카드 ${i + 1} ${SIDE_LABEL[side]}`),
    );
  const renameField = async (key: number, value: string) => {
    const f = draft.fields.find((x) => x.key === key);
    if (!f) return;
    const name = value.trim();
    if (!name && !f.name) {
      setDraft((x) => ({ ...x, fields: x.fields.filter((y) => y.key !== key) })); // 새 필드를 비운 채 닫았다
      return setRenaming(null);
    }
    if (fieldNameError(name, names.filter((_, i) => draft.fields[i].key !== key))) return; // 입력칸 아래 오류를 보인 채 둔다
    setRenaming(null);
    if (name === f.name) return;
    const old = f.name;
    setDraft((x) => ({ ...x, fields: x.fields.map((y) => (y.key === key ? { ...y, name } : y)) }));
    if (!old) return;
    // 템플릿 참조도 같이 (저장할 때 rename_field 와 같은 규칙)
    for (const t of draft.tpls) {
      const [qfmt, afmt] = await api.renameFieldRefs([t.qfmt, t.afmt], old, name);
      if (qfmt === t.qfmt && afmt === t.afmt) continue;
      const layout = await api.templateLayout(qfmt, afmt).catch(() => null);
      setTpl(t.key, { qfmt, afmt, layout, reason: layout ? null : t.reason });
    }
  };
  const addField = () => {
    const key = seq.current--;
    setDraft((x) => ({ ...x, fields: [...x.fields, { key, slot: null, name: "" }] }));
    setRenaming({ what: "field", key, value: "" });
  };
  const moveField = (from: number, to: number) =>
    setDraft((x) => {
      if (to < 0 || to >= x.fields.length || from === to) return x;
      const fields = [...x.fields];
      const [f] = fields.splice(from, 1);
      fields.splice(to, 0, f);
      return { ...x, fields };
    });
  const removeField = (key: number) => setDraft((x) => ({ ...x, fields: x.fields.filter((f) => f.key !== key) }));

  // ---------------------------------------------------------------- 쉬운 편집 줄

  const edit = (side: Side, f: (lines: Part[][]) => Part[][]) => {
    if (!cur?.layout) return;
    setLayout(cur.key, { ...cur.layout, [side]: f(linesOf(cur.layout[side])).flat() });
  };
  const addLine = (side: Side, name: string, at?: number) =>
    edit(side, (lines) => {
      // 빈칸 유형: 앞면에 빈칸 줄이 없으면 빈칸으로
      const cloze = d.cloze && side === "front" && !lines.some((l) => l[0].kind === "cloze");
      const line: Part[] = [cloze ? { kind: "cloze", name } : { kind: "field", name }];
      const out = [...lines];
      out.splice(at ?? out.length, 0, line);
      return out;
    });
  const setLook = (side: Side, i: number, look: Look) =>
    edit(side, (lines) =>
      lines.map((l, j) => {
        if (j !== i) return l;
        const p = l[0];
        if (p.kind !== "field" && p.kind !== "cloze") return l;
        const base = BASE_LOOK[side];
        return [{ ...p, look: look.size === base.size && look.align === base.align ? undefined : look }, ...l.slice(1)];
      }),
    );
  // 끌기: 필드 행 → 줄 만들기, 줄 → 옮기기
  const drag = useRef<{ kind: "field"; key: number } | { kind: "line"; side: Side; i: number } | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const dropOn = (side: Side, at: number) => {
    const g = drag.current;
    drag.current = null;
    setOver(null);
    if (!g || !cur?.layout) return;
    if (g.kind === "field") {
      const f = draft.fields.find((x) => x.key === g.key);
      if (f?.name) addLine(side, f.name, at);
      return;
    }
    const front = linesOf(cur.layout.front);
    const backs = linesOf(cur.layout.back);
    const src = g.side === "front" ? front : backs;
    const line = src[g.i];
    if (!line || line[0].kind === "front_side") return;
    const nextSrc = src.filter((_, j) => j !== g.i);
    const dst = side === g.side ? nextSrc : side === "front" ? front : backs;
    const pos = side === g.side && at > g.i ? at - 1 : at;
    const nextDst = [...dst];
    nextDst.splice(Math.max(side === "back" && dst[0]?.[0].kind === "front_side" ? 1 : 0, pos), 0, line);
    const layout = side === g.side ? { ...cur.layout, [side]: nextDst.flat() } : { ...cur.layout, [g.side]: nextSrc.flat(), [side]: nextDst.flat() };
    setLayout(cur.key, layout);
  };
  const zone = (id: string) => ({
    onDragOver: (e: DragEvent) => {
      if (!drag.current || readOnly) return;
      e.preventDefault();
      setOver(id);
    },
    onDragLeave: () => setOver((o) => (o === id ? null : o)),
  });

  // 옵션 3개
  const all = cur?.layout ? [...cur.layout.front, ...cur.layout.back] : [];
  const hasFrontSide = !!cur?.layout?.back.some((p) => p.kind === "front_side");
  const typePart = all.find((p) => p.kind === "type") as Extract<Part, { kind: "type" }> | undefined;
  const ttsPart = all.find((p) => p.kind === "tts") as Extract<Part, { kind: "tts" }> | undefined;
  const without = (l: Layout, kind: Part["kind"]): Layout => ({ front: l.front.filter((p) => p.kind !== kind), back: l.back.filter((p) => p.kind !== kind) });
  const setFrontSide = (on: boolean) => {
    if (!cur?.layout) return;
    const l = without(cur.layout, "front_side");
    setLayout(cur.key, on ? { ...l, back: [{ kind: "front_side" }, ...l.back] } : l);
  };
  const setType = (name: string | null) => {
    if (!cur?.layout) return;
    const l = without(cur.layout, "type");
    setLayout(cur.key, name ? { ...l, front: [...l.front, { kind: "type", name }] } : l);
  };
  const setTts = (t: { name: string; opts: string } | null) => {
    if (!cur?.layout) return;
    const l = without(cur.layout, "tts");
    if (!t) return setLayout(cur.key, l);
    // 그 필드의 줄 뒤에 (앞면 먼저), 없으면 앞면 끝 줄에
    for (const side of ["front", "back"] as Side[]) {
      const lines = linesOf(l[side]);
      const i = lines.findIndex((x) => nameOf(x[0]) === t.name);
      if (i >= 0) {
        lines[i] = [...lines[i], { kind: "tts", ...t }];
        return setLayout(cur.key, { ...l, [side]: lines.flat() });
      }
    }
    setLayout(cur.key, { ...l, front: [...l.front, { kind: "tts", ...t }] });
  };

  // ---------------------------------------------------------------- 카드 종류

  const addTpl = async () => {
    const f0 = refsOf(cur?.qfmt ?? "")[0] ?? names[0];
    const other = names.find((x) => x && x !== f0) ?? f0;
    const layout: Layout = { front: [{ kind: "field", name: other }], back: [{ kind: "front_side" }, { kind: "field", name: f0 }] };
    const b = await api.buildTemplateLayout(layout);
    const key = seq.current--;
    setDraft((x) => ({ ...x, tpls: [...x.tpls, { key, ord: null, name: "", ...b, layout, reason: null }] }));
    setCurKey(key);
  };
  const removeTpl = (key: number) => {
    const i = draft.tpls.findIndex((t) => t.key === key);
    const rest = draft.tpls.filter((t) => t.key !== key);
    setDraft((x) => ({ ...x, tpls: x.tpls.filter((t) => t.key !== key) }));
    setCurKey(rest[Math.max(0, i - 1)].key);
  };
  const tplNameError = (key: number, value: string) => {
    const v = value.trim();
    if (!v) return null; // 비우면 자동 이름
    return draft.tpls.some((t) => t.key !== key && t.name === v) ? `'${v}' 카드 종류가 이미 있어요.` : null;
  };
  const renameTpl = (key: number, value: string) => {
    const t = draft.tpls.find((x) => x.key === key);
    if (!t || tplNameError(key, value)) return;
    setRenaming(null);
    const v = value.trim();
    // 자동 이름 그대로면 이름을 바꾸지 않는다
    if (!v || v === autoName(t, d.cloze, show)) return;
    setTpl(key, { name: v });
  };

  // ---------------------------------------------------------------- 그 자리에서 막는 것 (NT9)

  const issues = useMemo(() => {
    const out: { key: number; side: Side | null; text: string }[] = [];
    const clozeField = draft.fields[0]?.name ?? "본문";
    for (const t of draft.tpls) {
      if (d.cloze && !isCloze(t.qfmt)) out.push({ key: t.key, side: "front", text: `빈칸 유형은 앞면에 {{cloze:필드}}가 있어야 해요. 예: {{cloze:${clozeField}}}` });
      if (!d.cloze && isCloze(t.qfmt + t.afmt)) out.push({ key: t.key, side: null, text: "일반 유형에는 {{cloze:…}}를 쓸 수 없어요. 빈칸 유형을 복제해서 만들어 주세요." });
      if (!refsOf(t.qfmt).length) out.push({ key: t.key, side: "front", text: "앞면에 필드를 하나 이상 넣어 주세요." });
      for (const side of ["front", "back"] as Side[]) {
        const miss = refsOf(side === "front" ? t.qfmt : t.afmt).find((r) => !names.includes(r));
        if (miss) out.push({ key: t.key, side, text: `${SIDE_LABEL[side]}에 쓴 '${miss}' 필드가 없어요.` });
      }
    }
    return out;
  }, [draft, d.cloze, names]);
  const renameError =
    renaming?.what === "field"
      ? fieldNameError(renaming.value, names.filter((_, i) => draft.fields[i].key !== renaming.key))
      : renaming?.what === "tpl"
        ? tplNameError(renaming.key, renaming.value)
        : null;
  const fixes = issues.length + (renameError && renaming?.value.trim() ? 1 : 0);
  const curIssues = issues.filter((x) => x.key === cur?.key);

  // ---------------------------------------------------------------- 저장 (NT5)

  const editOf = (): TypeEdit => ({
    fields: draft.fields.filter((f) => f.name).map((f) => ({ slot: f.slot, name: f.name })),
    templates: draft.tpls.map((t) => ({ ord: t.ord, name: t.name.trim() || null, qfmt: t.qfmt, afmt: t.afmt })),
    css: draft.css,
  });
  const commit = async (impact: SaveImpact) => {
    setSaving(true);
    try {
      // 지우는 저장은 이 기기 백업을 먼저 (디자인 NT8)
      if (impact.templates.length || impact.fields.length) await api.createBackup();
      await api.saveNoteType(d.id, editOf(), false, d.version);
      toast({ title: "노트 유형을 저장했어요", text: impact.made ? `카드 ${n(impact.made)}장을 새로 만들었어요` : undefined });
      nav(back, true);
    } catch (e) {
      setAsk(null);
      failed(e);
    }
  };
  const failed = (e: unknown) => {
    const text = errorText(e);
    if (text.startsWith(STALE)) setStale(true);
    else setSaveError(text);
    setSaving(false);
  };
  const save = async () => {
    if (readOnly || saving || fixes > 0 || renaming || stale) return;
    if (!dirty) return nav(back, true);
    setSaving(true);
    setSaveError(undefined);
    try {
      const impact = await api.saveNoteType(d.id, editOf(), true, d.version);
      setSaving(false);
      if (impact.empty_fronts || impact.templates.length || impact.fields.length || impact.made) setAsk(impact);
      else await commit(impact);
    } catch (e) {
      failed(e);
    }
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (overlay) return;
    if (matchAction("save", e.nativeEvent) >= 0) {
      e.preventDefault();
      save();
    }
  };
  // 초점이 입력칸 밖(빈 곳 · 버튼 · 이름 입력을 마친 뒤의 body)이어도 저장 (Q1-05). 입력칸 안은 위 onKeyDown 이 먼저 받고 막는다
  useKey({ action: "save" }, () => void save(), !overlay && !readOnly);

  // ---------------------------------------------------------------- 그리기

  const badge = d.native_kind ? "앱 기본" : d.user_made ? "내가 만든 것" : "Anki에서 가져옴";
  const top = (
    <header className="nte__bar">
      <div className="nte__bar-left">
        <button className="nte__back" aria-label="뒤로" onClick={goBack}>
          <Icon name="chevron-left" size={20} />
        </button>
        <span className="t-title-s">노트 유형 편집</span>
        <span className="nte__type t-body-m c-secondary" title={typeLabel(d)}>{typeLabel(d)}</span>
        <Badge>{badge}</Badge>
      </div>
      {!drawn && (
        <div className="segment segment--boxed nte__modes" role="tablist" aria-label="편집 방식">
          {!cur?.layout && <Icon name="lock" size={14} className="c-tertiary nte__lock" />}
          <button
            role="tab"
            aria-selected={effMode === "easy"}
            disabled={!cur?.layout}
            title={cur?.layout ? undefined : "이 템플릿은 HTML로만 고칠 수 있어요"}
            className={`segment__item t-label-m ${effMode === "easy" ? "is-selected" : ""}`}
            onClick={() => setMode("easy")}
          >
            쉬운 편집
          </button>
          <button role="tab" aria-selected={effMode === "html"} className={`segment__item t-label-m ${effMode === "html" ? "is-selected" : ""}`} onClick={() => setMode("html")}>
            HTML
          </button>
        </div>
      )}
      <div className="nte__actions">
        {readOnly ? (
          <>
            <Button variant="ghost" size="M" label="닫기" onClick={goBack} />
            {d.cloneable && <Button size="M" icon="layers" label="복제해서 고치기" onClick={() => setCloning(true)} />}
          </>
        ) : (
          <>
            {fixes > 0 && <span className="t-label-s c-danger">고칠 곳 {n(fixes)}개</span>}
            <Button variant="ghost" size="M" label="취소" onClick={goBack} />
            <Button size="M" label="저장" shortcut={actionKey("save")} disabled={fixes > 0 || stale} busy={saving} busyLabel="저장하는 중" onClick={save} />
          </>
        )}
      </div>
    </header>
  );

  const decks = d.decks.length ? ` · ${d.decks.slice(0, 2).join(" · ")}${d.decks.length > 2 ? ` 외 ${n(d.decks.length - 2)}곳` : ""}` : "";
  const notice = readOnly ? (
    <p className="nte__notice nte__notice--lock t-label-m">
      <Icon name="lock" size={16} />
      앱 기본 유형은 볼 수만 있어요.{" "}
      {d.cloneable
        ? `복제하면 내 유형으로 마음대로 고칠 수 있어요${d.cards ? ` (이 유형을 쓰는 카드 ${n(d.cards)}장은 그대로예요)` : ""}.`
        : "학습 화면이 직접 그려서 복제할 수 없어요."}
    </p>
  ) : (
    <p className={`nte__notice t-label-m ${d.cards ? "nte__notice--warn" : ""}`}>
      <Icon name="info" size={16} />
      {d.cards ? `이 유형을 쓰는 카드 ${n(d.cards)}장(노트 ${n(d.notes)}개)이 함께 바뀌어요${decks}` : "아직 이 유형을 쓰는 카드가 없어요"}
    </p>
  );

  return (
    <main className="nte" onKeyDown={onKeyDown} onClick={() => setMenu(null)}>
      {top}
      <div className="nte__body">
        {notice}
        {drawn ? (
          <div className="nte__drawn">
            <span className="nte__drawn-icon">
              <Icon name="image" size={18} className="c-secondary" />
            </span>
            <span className="nte__drawn-text">
              <span className="t-caption c-tertiary">그림 가리기 · 목록 · 선택형</span>
              <span className="t-label-m">앱이 직접 그리는 유형이에요</span>
              <span className="t-body-s c-secondary">
                필드와 앞·뒷면 배치가 정해져 있어 이 편집기로 고치거나 복제할 수 없어요. 카드는 카드 추가 화면의 [{typeLabel(d)}]에서 만들어요.
              </span>
            </span>
          </div>
        ) : (
          <>
            {stale && (
              <p className="nte__notice nte__notice--warn t-label-m" role="alert">
                <Icon name="cloud" size={16} />
                다른 기기에서 이 노트 유형이 바뀌었어요. 지금 화면으로는 저장할 수 없어요 · 다시 불러오면 고친 내용은 사라져요
                {/* 저장이 막힌 동안 나갈 길이라 글자 링크가 아니라 버튼 (디자인 검토 b) */}
                <Button variant="secondary" size="M" label="다시 불러오기" onClick={reload} />
              </p>
            )}
            {unseen.length > 0 ? (
              <p className="nte__notice t-label-m">
                <Icon name="info" size={16} />
                {lost > 0 ? `다른 기기와 동시에 고친 내용 ${n(lost)}건은 나중에 고친 쪽으로 맞췄어요` : KEPT_BOTH}
                <Button variant="ghost" size="M" label={lost > 0 ? "진 쪽 보기" : "자세히"} onClick={() => setShowConflicts(true)} />
              </p>
            ) : (
              conflicts.length > 0 && (
                // 본 뒤에도 30일 동안 다시 열 수 있게 (진 쪽 내용을 복사해 되돌린다)
                <p className="nte__seen t-caption c-tertiary">
                  다른 기기와 동시에 고친 기록 {n(conflicts.length)}건 · 30일 동안 남아요
                  <button type="button" className="nte__seen-btn t-caption" onClick={() => setShowConflicts(true)}>
                    보기
                  </button>
                </p>
              )
            )}
            {saveError && (
              <p className="nte__notice nte__notice--danger t-label-m" role="alert">
                <Icon name="alert" size={16} />
                저장하지 못했어요 · {saveError}
              </p>
            )}
            {effMode === "html" && !cur?.layout && cur?.reason && (
              <p className="nte__notice nte__notice--lock">
                <Icon name="lock" size={16} />
                <span className="t-label-m">이 템플릿은 HTML로만 고칠 수 있어요</span>
                <span className="t-caption c-secondary">
                  {cur.reason}
                  {iga(cur.reason)} 있어 쉬운 편집이 그대로 옮기지 못해요. 고쳐도 HTML은 그대로 남아요.
                </span>
              </p>
            )}
            {kinds()}
            <div className="nte__cols">
              {fields()}
              <div className="nte__mid">{effMode === "easy" ? easy() : htmlPanes()}</div>
              <Preview d={d} draft={draft} tpl={cur} note={note} />
            </div>
          </>
        )}
      </div>

      {leaving && (
        <DiscardModal
          title="고친 내용이 있어요"
          text="저장하지 않고 나갈까요?"
          keep="계속 고치기"
          discard="버리고 나가기"
          onKeep={() => setLeaving(null)}
          onDiscard={() => nav(leaving, true)}
        />
      )}
      {ask && <SaveConfirm impact={ask} labels={labelsOf(d)} busy={saving} onBack={() => setAsk(null)} onSave={() => commit(ask)} />}
      {cloning && <CloneDialog from={d} back={back} onClose={() => setCloning(false)} />}
      {showConflicts && (
        <ConflictList
          list={conflicts}
          onClose={() => {
            setShowConflicts(false);
            setConflicts((c) => c.map((x) => ({ ...x, seen: true })));
            api.noteTypeConflictsSeen(d.id).catch(() => {});
          }}
        />
      )}
    </main>
  );

  // ---------------------------------------------------------------- 부분들 (같은 상태를 쓰는 안쪽 함수. 컴포넌트로 쓰면 그릴 때마다 새로 붙어 입력 초점을 잃는다)

  function kinds() {
    return (
      <div className="nte__kinds" role="tablist" aria-label="카드 종류">
        {draft.tpls.map((t, i) =>
          renaming?.what === "tpl" && renaming.key === t.key ? (
            <span key={t.key} className="nte__kind-edit">
              <input
                className="nte__kind-input t-label-m"
                autoFocus
                aria-label="카드 종류 이름"
                value={renaming.value}
                onChange={(e) => setRenaming({ ...renaming, value: e.target.value })}
                onBlur={() => renameTpl(t.key, renaming.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") renameTpl(t.key, renaming.value);
                  if (e.key === "Escape") (e.preventDefault(), setRenaming(null));
                }}
              />
              {renameError && <span className="t-caption c-danger">{renameError}</span>}
            </span>
          ) : (
            <span key={t.key} className={`nte__kind ${t.key === cur?.key ? "is-on" : ""} ${issues.some((x) => x.key === t.key) ? "is-error" : ""}`}>
              <button
                role="tab"
                aria-selected={t.key === cur?.key}
                className="t-label-m"
                title={readOnly ? undefined : "두 번 눌러 이름 바꾸기"}
                onClick={() => setCurKey(t.key)}
                onDoubleClick={() => !readOnly && setRenaming({ what: "tpl", key: t.key, value: tplName(t, d.cloze, show) })}
                onKeyDown={(e) => e.key === "F2" && !readOnly && setRenaming({ what: "tpl", key: t.key, value: tplName(t, d.cloze, show) })}
              >
                {tplTitle(t, i, d.cloze, show)}
              </button>
              {!readOnly && t.key === cur?.key && draft.tpls.length > 1 && (
                <button className="nte__kind-x" aria-label={`${tplTitle(t, i, d.cloze, show)} 지우기`} title="카드 종류 지우기" onClick={() => removeTpl(t.key)}>
                  <Icon name="x" size={14} />
                </button>
              )}
            </span>
          ),
        )}
        {!readOnly && !d.cloze && (
          <button className="nte__kind-add t-label-m c-secondary" onClick={addTpl}>
            <Icon name="plus" size={14} />
            카드 종류
          </button>
        )}
        <span className="t-caption c-tertiary">
          {readOnly ? "앱 기본 유형에는 카드 종류를 더할 수 없어요" : d.cloze ? "빈칸 유형은 빈칸 번호마다 카드가 한 장씩 생겨요" : "카드 종류마다 노트 하나에서 카드가 한 장씩 생겨요"}
        </span>
      </div>
    );
  }

  function fields() {
    const html = effMode === "html";
    return (
      <section className="nte__panel nte__fields" aria-label="필드">
        <div className="nte__panel-head">
          <span className="t-label-m">필드 {n(names.filter(Boolean).length)}</span>
          {!readOnly && (
            <button className="nte__add t-label-m c-accent" onClick={addField}>
              <Icon name="plus" size={14} />
              필드
            </button>
          )}
        </div>
        {draft.fields.map((f, i) => {
          const use = cur ? [refsOf(cur.qfmt).includes(f.name) && "앞면", refsOf(cur.afmt).includes(f.name) && "뒷면"].filter(Boolean).join("·") : "";
          const where = usedWhere(f.name);
          const editing = renaming?.what === "field" && renaming.key === f.key;
          const tipId = `nte-tip-${f.key}`;
          return (
            <div
              key={f.key}
              className={`nte__field ${over === `f${i}` ? "is-over" : ""}`}
              tabIndex={readOnly || editing ? -1 : 0}
              draggable={!readOnly && !editing}
              aria-label={`${f.name} 필드${readOnly ? "" : " · Alt+위아래로 순서 바꾸기"}`}
              onDragStart={() => (drag.current = { kind: "field", key: f.key })}
              onDragEnd={() => ((drag.current = null), setOver(null))}
              onDragOver={(e) => {
                if (drag.current?.kind !== "field" || readOnly) return;
                e.preventDefault();
                setOver(`f${i}`);
              }}
              onDrop={() => {
                const g = drag.current;
                if (g?.kind === "field") moveField(draft.fields.findIndex((x) => x.key === g.key), i);
                drag.current = null;
                setOver(null);
              }}
              onKeyDown={(e) => {
                if (e.target !== e.currentTarget || readOnly) return;
                if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
                  e.preventDefault();
                  const to = i + (e.key === "ArrowUp" ? -1 : 1);
                  const list = e.currentTarget.parentElement;
                  moveField(i, to);
                  requestAnimationFrame(() => list?.querySelectorAll<HTMLElement>(".nte__field")[to]?.focus());
                } else if (e.key === "F2" || e.key === "Enter") {
                  e.preventDefault();
                  setRenaming({ what: "field", key: f.key, value: f.name });
                }
              }}
            >
              <Icon name="grip" size={14} className="c-tertiary" />
              {editing ? (
                <span className="nte__fname-edit">
                  <input
                    className={`nte__fname-input t-body-m ${renameError ? "is-error" : ""}`}
                    autoFocus
                    aria-label="필드 이름"
                    value={renaming.value}
                    onChange={(e) => setRenaming({ ...renaming, value: e.target.value })}
                    onBlur={() => renameField(f.key, renaming.value)}
                    onKeyDown={(e) => {
                      e.stopPropagation();
                      if (e.key === "Enter") renameField(f.key, renaming.value);
                      if (e.key === "Escape") {
                        e.preventDefault();
                        if (!f.name) removeField(f.key);
                        setRenaming(null);
                      }
                    }}
                  />
                  {renameError && renaming.value.trim() && <span className="t-caption c-danger">{renameError}</span>}
                </span>
              ) : (
                <button
                  className="nte__fname t-body-m"
                  tabIndex={-1}
                  title={html && !readOnly ? "누르면 커서 자리에 넣어요 · 두 번 눌러 이름 바꾸기" : readOnly ? undefined : "두 번 눌러 이름 바꾸기"}
                  onClick={() => html && !readOnly && insertRef(f.name)}
                  onDoubleClick={() => !readOnly && setRenaming({ what: "field", key: f.key, value: f.name })}
                >
                  {show(f.name)}
                </button>
              )}
              {!editing && <span className="t-caption c-tertiary">{use}</span>}
              {!editing && show(f.name) !== f.name && <span className="nte__dbname t-mono-key c-tertiary" title="템플릿 · Anki 에 쓰는 필드 이름">{f.name}</span>}
              {!readOnly && !editing && (
                <span className="nte__trash-wrap">
                  <button
                    className="nte__trash"
                    aria-label={`${f.name} 필드 지우기`}
                    aria-disabled={where.length > 0 || draft.fields.length < 2}
                    aria-describedby={where.length ? tipId : undefined}
                    onClick={() => !where.length && draft.fields.length > 1 && removeField(f.key)}
                  >
                    <Icon name="trash" size={14} />
                  </button>
                  {where.length > 0 && (
                    <span id={tipId} role="tooltip" className="nte__tip">
                      <span className="t-label-s">카드에서 쓰고 있어 지울 수 없어요</span>
                      <span className="t-caption c-on-inverse">{where.join(" · ")}에서 먼저 빼면 지울 수 있어요</span>
                    </span>
                  )}
                </span>
              )}
            </div>
          );
        })}
        <p className="t-caption c-tertiary">
          {readOnly
            ? "필드 이름과 순서도 복제한 뒤에 바꿀 수 있어요."
            : html
              ? "필드를 누르면 커서 자리에 {{필드}}를 넣어요. 끌거나 Alt+↑↓로 순서를 바꿔요."
              : "끌거나 Alt+↑↓로 순서를 바꿔요. 카드 추가 화면의 칸 순서예요. 카드에 쓰는 필드는 지울 수 없어요."}
        </p>
      </section>
    );
  }

  function easy() {
    if (!cur?.layout) return null;
    const layout = cur.layout;
    const fieldsUsed = [...layout.front, ...layout.back].map(nameOf).filter(Boolean);
    const sideBox = (side: Side) => {
      const lines = linesOf(layout[side]);
      const err = curIssues.find((x) => x.side === side);
      return (
        <section className={`nte__panel nte__side ${err ? "is-error" : ""}`} aria-label={SIDE_LABEL[side]}>
          <div className="nte__side-head">
            <span className="t-label-m">{SIDE_LABEL[side]}</span>
            <span className="t-caption c-tertiary">{side === "front" ? "질문으로 보이는 면" : "답을 볼 때 보이는 면"}</span>
          </div>
          {lines.map((line, i) => {
            const p = line[0];
            if (p.kind === "front_side")
              return (
                <div key="fs" className="nte__fs t-caption c-secondary">
                  <Icon name="layers" size={14} />
                  앞면 내용 · 아래 옵션에서 끌 수 있어요
                </div>
              );
            const look = lookOf(p) ?? BASE_LOOK[side];
            const label = p.kind === "cloze" ? `빈칸 · ${show(p.name)}` : p.kind === "type" ? `써서 답하기 · ${show(p.name)}` : p.kind === "tts" ? `읽어주기 · ${show(p.name)}` : show(p.name);
            const styled = p.kind === "field" || p.kind === "cloze";
            return (
              <div
                key={i}
                className={`nte__line ${over === `${side}${i}` ? "is-over" : ""}`}
                {...zone(`${side}${i}`)}
                onDrop={() => dropOn(side, i)}
                tabIndex={readOnly ? -1 : 0}
                aria-label={`${label} 줄${readOnly ? "" : " · Alt+위아래로 옮기기 · Delete 로 빼기"}`}
                onKeyDown={(e) => {
                  if (e.target !== e.currentTarget || readOnly) return;
                  if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
                    e.preventDefault();
                    const to = i + (e.key === "ArrowUp" ? -1 : 1);
                    if (to < 0 || to >= lines.length || lines[to][0].kind === "front_side") return;
                    edit(side, (ls) => {
                      const out = [...ls];
                      [out[i], out[to]] = [out[to], out[i]];
                      return out;
                    });
                  } else if (e.key === "Delete" && !(side === "front" && lines.length < 2)) edit(side, (ls) => dropLine(ls, i));
                }}
              >
                <span
                  className="fieldchip"
                  draggable={!readOnly}
                  onDragStart={() => (drag.current = { kind: "line", side, i })}
                  onDragEnd={() => ((drag.current = null), setOver(null))}
                >
                  <Icon name="grip" size={14} />
                  <span className="t-label-m">{label}</span>
                </span>
                <span className="nte__spacer" />
                {styled && (
                  <>
                    <Mini
                      label={`${p.name} 크기`}
                      value={look.size}
                      disabled={readOnly}
                      options={[["s", "S"], ["m", "M"], ["l", "L"]]}
                      onChange={(size) => setLook(side, i, { ...look, size })}
                    />
                    <Mini
                      label={`${p.name} 정렬`}
                      value={look.align}
                      disabled={readOnly}
                      options={[["left", "왼쪽"], ["center", "가운데"]]}
                      onChange={(align) => setLook(side, i, { ...look, align })}
                    />
                  </>
                )}
                {!readOnly && (
                  <button
                    className="nte__x"
                    aria-label={`${label} 줄 빼기`}
                    disabled={side === "front" && lines.filter((l) => l[0].kind !== "front_side").length < 2}
                    onClick={() => edit(side, (ls) => dropLine(ls, i))}
                  >
                    <Icon name="x" size={14} />
                  </button>
                )}
              </div>
            );
          })}
          {!readOnly && (
            <span className="menu nte__zone-wrap">
              <button
                className={`nte__zone t-caption c-tertiary ${over === `${side}z` ? "is-over" : ""}`}
                aria-haspopup="menu"
                aria-expanded={menu === `add-${side}`}
                {...zone(`${side}z`)}
                onDrop={() => dropOn(side, lines.length)}
                onClick={(e) => {
                  e.stopPropagation();
                  setMenu(menu === `add-${side}` ? null : `add-${side}`);
                }}
              >
                <Icon name="plus" size={14} />
                왼쪽 필드를 끌어 놓으면 줄이 생겨요
              </button>
              {menu === `add-${side}` && (
                <FieldMenu
                  label={`${SIDE_LABEL[side]}에 넣을 필드`}
                  items={names.filter(Boolean).map((x) => ({ label: x, hint: fieldsUsed.includes(x) ? "이미 쓰고 있어요" : undefined, onPick: () => addLine(side, x) }))}
                  onClose={() => setMenu(null)}
                />
              )}
            </span>
          )}
          {err && (
            <p className="t-caption c-danger nte__err">
              <Icon name="alert" size={12} />
              {err.text}
            </p>
          )}
        </section>
      );
    };
    const ttsLang = ttsPart ? TTS_LANGS.find(([k]) => ttsPart.opts.startsWith(k))?.[1] ?? ttsPart.opts : "";
    return (
      <>
        {sideBox("front")}
        {sideBox("back")}
        <section className="nte__panel nte__options" aria-label="옵션">
          <div className="nte__option">
            <Checkbox checked={hasFrontSide} label="뒷면에 앞면 포함" disabled={readOnly} onChange={setFrontSide} />
            <span className="t-caption c-tertiary">답을 볼 때 질문도 위에 남겨요</span>
          </div>
          <div className="nte__option">
            <Checkbox checked={!!typePart} label="써서 답하기 칸" disabled={readOnly} onChange={(on) => setType(on ? (refsOf(cur.afmt).find((x) => !refsOf(cur.qfmt).includes(x)) ?? names[0]) : null)} />
            {typePart ? (
              pill(
                "type",
                `정답 · ${show(typePart.name)}`,
                names.filter(Boolean).map((x) => ({ label: x, onPick: () => setType(x) })),
                readOnly,
              )
            ) : (
              <span className="t-caption c-tertiary">고르면 필드 하나를 정답으로 써서 비교해요</span>
            )}
          </div>
          <div className="nte__option">
            <Checkbox
              checked={!!ttsPart}
              label="읽어주기"
              disabled={readOnly}
              onChange={(on) => setTts(on ? { name: nameOf(linesOf(layout.front)[0]?.[0] ?? { kind: "front_side" }) || names[0], opts: "en_US" } : null)}
            />
            {ttsPart ? (
              pill(
                "tts",
                `${show(ttsPart.name)} · ${ttsLang}`,
                [
                  ...names.filter(Boolean).map((x) => ({ label: x, hint: "필드", onPick: () => setTts({ name: x, opts: ttsPart.opts }) })),
                  ...TTS_LANGS.map(([k, v]) => ({ label: v, hint: k, onPick: () => setTts({ name: ttsPart.name, opts: k }) })),
                ],
                readOnly,
              )
            ) : (
              <span className="t-caption c-tertiary">{readOnly ? "복제하면 켤 수 있어요" : "필드를 소리 내어 읽어 줘요"}</span>
            )}
          </div>
        </section>
      </>
    );
  }

  function pill(id: string, text: string, items: MenuPick[], disabled?: boolean) {
    return (
      <span className="menu">
        <button
          className="nte__pill t-label-s"
          aria-haspopup="menu"
          aria-expanded={menu === id}
          disabled={disabled}
          onClick={(e) => {
            e.stopPropagation();
            setMenu(menu === id ? null : id);
          }}
        >
          {text}
          <Icon name="chevron-down" size={12} scaled />
        </button>
        {menu === id && <FieldMenu label={text} items={items} onClose={() => setMenu(null)} />}
      </span>
    );
  }

  function htmlPanes() {
    if (!cur) return null;
    const pane = (which: "qfmt" | "afmt" | "css") => {
      const side: Side | null = which === "qfmt" ? "front" : which === "afmt" ? "back" : null;
      const err = side ? curIssues.find((x) => x.side === side) : undefined;
      const value = which === "css" ? draft.css : cur[which];
      return (
        <section className={`nte__panel nte__pane ${err ? "is-error" : ""}`} aria-label={side ? SIDE_LABEL[side] : "CSS"}>
          <div className="nte__side-head">
            <span className="t-label-m">{side ? SIDE_LABEL[side] : "CSS"}</span>
            <span className="t-caption c-tertiary">{side ? tplTitle(cur, curIndex, d.cloze, show) : "이 유형의 모든 카드 종류에 써요"}</span>
          </div>
          <CodeArea
            label={side ? `${SIDE_LABEL[side]} HTML` : "CSS"}
            value={value}
            readOnly={readOnly}
            invalid={!!err}
            onFocus={(el) => which !== "css" && (area.current = { el, which })}
            onChange={(v) => (which === "css" ? setDraft((x) => ({ ...x, css: v })) : setHtml(cur.key, { [which]: v }))}
          />
          {err && (
            <p className="t-caption c-danger nte__err">
              <Icon name="alert" size={12} />
              {err.text}
            </p>
          )}
        </section>
      );
    };
    return (
      <>
        {pane("qfmt")}
        {pane("afmt")}
        {pane("css")}
        <p className="t-caption c-tertiary">Anki와 같은 문법이에요 · 스크립트(&lt;script&gt;)는 학습 화면에서 실행하지 않아요</p>
      </>
    );
  }

  /** HTML 탭: 필드를 누르면 마지막으로 고친 칸(없으면 뒷면)의 커서 자리에 {{필드}} */
  function insertRef(name: string) {
    if (!cur) return;
    const at = area.current;
    const which = at?.which ?? "afmt";
    const text = cur[which];
    const el = at?.el.isConnected ? at.el : null;
    const [s, e] = el ? [el.selectionStart, el.selectionEnd] : [text.length, text.length];
    const tag = `{{${name}}}`;
    setHtml(cur.key, { [which]: text.slice(0, s) + tag + text.slice(e) });
    requestAnimationFrame(() => {
      const box = document.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${which === "qfmt" ? "앞면" : "뒷면"} HTML"]`);
      box?.focus();
      box?.setSelectionRange(s + tag.length, s + tag.length);
    });
  }
}

type MenuPick = { label: string; hint?: string; onPick: () => void };

function FieldMenu({ label, items, onClose }: { label: string; items: MenuPick[]; onClose: () => void }) {
  return (
    <div
      className="menu__list nte__menu"
      role="menu"
      aria-label={label}
      onKeyDown={menuKeys}
      onClick={(e) => e.stopPropagation()}
      onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && onClose()}
    >
      {items.map((x, i) => (
        <button key={`${x.label}${i}`} role="menuitem" autoFocus={i === 0} className="menu__item t-label-m" onClick={() => (onClose(), x.onPick())}>
          <span className="menu__text">{x.label}</span>
          {x.hint && <span className="t-caption c-tertiary">{x.hint}</span>}
        </button>
      ))}
    </div>
  );
}

/** 작은 세그먼트 (S/M/L · 왼쪽/가운데 · 앞면/뒷면 · 라이트/다크) */
function Mini<T extends string>({ label, value, options, onChange, disabled }: { label: string; value: T; options: [T, string][]; onChange: (v: T) => void; disabled?: boolean }) {
  return (
    <span className="nte__mini" role="radiogroup" aria-label={label}>
      {options.map(([v, text]) => (
        <button key={v} role="radio" aria-checked={v === value} disabled={disabled} className={`t-label-s ${v === value ? "is-on" : ""}`} onClick={() => onChange(v)}>
          {text}
        </button>
      ))}
    </span>
  );
}

/** 고정폭 칸. {{필드}} 는 accent 로 칠한다 (같은 글자를 뒤에 그린 pre 위에 투명한 textarea) */
function CodeArea({ label, value, readOnly, invalid, onChange, onFocus }: {
  label: string;
  value: string;
  readOnly: boolean;
  invalid: boolean;
  onChange: (v: string) => void;
  onFocus: (el: HTMLTextAreaElement) => void;
}) {
  const parts = value.split(/(\{\{.*?\}\})/s);
  return (
    <div className={`nte-code ${invalid ? "is-error" : ""}`}>
      <pre className="nte-code__hl" aria-hidden>
        {parts.map((p, i) => (i % 2 ? <span key={i} className="nte-code__tag">{p}</span> : p))}
        {"\n"}
      </pre>
      <textarea
        aria-label={label}
        aria-invalid={invalid}
        spellCheck={false}
        readOnly={readOnly}
        value={value}
        onFocus={(e) => onFocus(e.currentTarget)}
        onSelect={(e) => onFocus(e.currentTarget)}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

// ---------------------------------------------------------------- 미리보기 (학습 화면과 같은 렌더: 살균 · 소리 칩 · 수식 · CSS 가두기)

function Preview({ d, draft, tpl, note }: { d: Detail; draft: Draft; tpl: DTpl | undefined; note?: number }) {
  const [side, setSide] = useState<Side>("back");
  const [theme, setTheme] = useState<"light" | "dark">(() => (document.documentElement.dataset.theme === "dark" ? "dark" : "light"));
  const [samples, setSamples] = useState<Sample[] | null>(null);
  const [pick, setPick] = useState<number | "names">(0);
  const [open, setOpen] = useState(false);
  const [html, setHtml] = useState<{ front: string; question: string; answer: string } | null>(null);
  const [error, setError] = useState<string>();
  useOverlayScope(open);
  useKey("Escape", () => setOpen(false), open, { modal: true });
  useEffect(() => {
    api.noteTypeSamples(d.id, note).then(setSamples, () => setSamples([]));
  }, [d.id, note]);
  const sample = samples && pick !== "names" ? samples[pick] : undefined;
  const names = draft.fields.filter((f) => f.name).map((f) => f.name);
  const values = sample ? draft.fields.filter((f) => f.name).map((f) => (f.slot !== null ? (sample.slots[f.slot] ?? "") : `(${f.name})`)) : null;
  const key = JSON.stringify([tpl?.qfmt, tpl?.afmt, names, values]);
  useEffect(() => {
    if (!tpl || samples === null) return;
    let live = true;
    api
      .previewTemplate(d.id, tpl.qfmt, tpl.afmt, tpl.ord ?? 0, values, names, sample?.note_id)
      .then(async (p) => {
        const parts = splitBack(p.back, p.front);
        const [front, question, answer] = (await Promise.all([renderCardHtml(p.front), renderCardHtml(parts.question), renderCardHtml(parts.answer)])).map(withMath);
        if (live) (setHtml({ front, question, answer }), setError(undefined));
      })
      .catch((e) => live && setError(errorText(e)));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- key 가 템플릿 · 필드 · 고른 카드를 모두 담는다
  }, [key, d.id, samples]);

  // 줄 상자를 쓰는데 CSS 에 없으면 저장할 때처럼 붙여 그린다
  const mark = d.layout_css.split("\n")[0];
  const css = tpl && usesBox(tpl) && !draft.css.includes(mark) ? `${draft.css}\n${d.layout_css}` : draft.css;
  const face = theme === "dark" && css.trim() ? deckFace(css) : null;
  // 학습 화면과 같은 가두기 · 같은 구조 (.study › .study__card-content › 질문 면 .study__q / 답 면 .study__a). 그래서 면마다 학습 화면의 정렬 · 줄 높이 · 그림 크기가 그대로다
  const scoped = useMemo(() => scopeCss(css, face === "other"), [css, face]);
  // 카드 고르기 이름: 보이는 순서로 처음 비지 않은 필드 글자 (빈칸 표시는 답만) · 덱. 맨 앞 필드가 빈 노트도 알아볼 수 있게
  const label = (s: Sample) => {
    const text = (slot: number | null) => (slot === null ? "" : plainText(s.slots[slot] ?? "").replace(/\{\{c\d+::(.*?)(::.*?)?\}\}/g, "$1").trim());
    const first = draft.fields.map((f) => text(f.slot)).find(Boolean) ?? "";
    return `${first.slice(0, 30) || "(빈 칸)"}${s.deck ? ` · ${s.deck}` : ""}`;
  };
  // 고른 노트로 그린 앞면이 비었다: 그 카드는 학습에 나오지 않는다 (NT49). 앞 · 뒷면 보기 모두 카드 칸 위에 한 줄
  const blankFront = !!sample && !!html && !plainText(html.front).trim() && !/<img|class="snd/.test(html.front);

  return (
    <section className="nte__panel nte__preview" aria-label="미리보기">
      <div className="nte__panel-head">
        <span className="t-label-m">미리보기</span>
        <Mini label="미리볼 면" value={side} options={[["front", "앞면"], ["back", "뒷면"]]} onChange={setSide} />
      </div>
      <div className="nte__panel-head">
        <span className="menu">
          <button
            className="nte__pill t-label-s"
            aria-haspopup="menu"
            aria-expanded={open}
            onClick={(e) => {
              e.stopPropagation();
              setOpen(!open);
            }}
          >
            <span className="nte__pill-text">{sample ? label(sample) : "필드 이름으로 보기"}</span>
            <Icon name="chevron-down" size={12} scaled />
          </button>
          {open && (
            <FieldMenu
              label="미리볼 카드"
              items={[
                ...(samples ?? []).map((s, i) => ({ label: label(s), onPick: () => setPick(i) })),
                { label: "필드 이름으로 보기", hint: samples?.length ? undefined : "이 유형의 노트가 아직 없어요", onPick: () => setPick("names") },
              ]}
              onClose={() => setOpen(false)}
            />
          )}
        </span>
        <Mini label="미리볼 테마" value={theme} options={[["light", "라이트"], ["dark", "다크"]]} onChange={setTheme} />
      </div>
      <div className="nte-card study" data-theme={face === "light" ? "light" : theme}>
        {scoped && <style>{scoped}</style>}
        {blankFront && <p className="nte-card__blank t-body-s c-tertiary">앞면이 비어 학습에 나오지 않아요</p>}
        <div className="study__card-content">
          <div className={face === "night" ? "nightMode night_mode" : undefined}>
            {error ? (
              <p className="t-body-s c-danger">{error}</p>
            ) : !html ? null : side === "front" ? (
              <div className="study__q card-html" dangerouslySetInnerHTML={{ __html: html.front }} />
            ) : (
              <div className="study__a">
                {plainText(html.question) || html.question.includes("<img") || html.question.includes('class="snd') ? (
                  <>
                    <div className="study__a-q t-title-m c-tertiary card-html" dangerouslySetInnerHTML={{ __html: html.question }} />
                    <div className="study__rule" />
                  </>
                ) : null}
                <div className="study__a-body card-html" dangerouslySetInnerHTML={{ __html: html.answer }} />
              </div>
            )}
          </div>
        </div>
      </div>
      <p className="t-caption c-tertiary">학습 화면과 같은 방식으로 그려요. 덱 스타일(CSS)도 그대로예요.</p>
    </section>
  );
}

// ---------------------------------------------------------------- NT5 저장 전 확인 (여러 개면 한 창에 목록)

const labelsOf = (d: Detail) => {
  const t = initDraft(d).tpls;
  return new Map(t.map((x, i) => [x.name, tplTitle(x, i, d.cloze)]));
};

function SaveConfirm({ impact, labels, busy, onBack, onSave }: {
  impact: SaveImpact;
  labels: Map<string, string>;
  busy: boolean;
  onBack: () => void;
  onSave: () => void;
}) {
  type Item = { icon: IconName; tone: "warn" | "danger" | "info"; title: string; text: ReactNode };
  const items: Item[] = [];
  const removes = impact.templates.length + impact.fields.length > 0;
  if (impact.empty_fronts)
    items.push({
      icon: "alert",
      tone: "warn",
      title: `카드 ${n(impact.empty_fronts)}장의 앞면이 비어요`,
      text: "앞면이 빈 카드는 학습에 나오지 않고, 필드를 채우면 다시 나와요. 카드와 복습 기록은 그대로예요.",
    });
  for (const t of impact.templates) {
    const label = labels.get(t.name) ?? t.name;
    items.push({
      icon: "trash",
      tone: "danger",
      title: t.cards ? `「${label}」의 카드 ${n(t.cards)}장을 지워요` : `「${label}」 카드 종류를 지워요`,
      text: t.cards
        ? `되돌릴 수 없어요. 이 카드들의 복습 기록은 학습 통계에만 남아요.${t.notes ? ` 카드가 하나도 남지 않는 노트 ${n(t.notes)}개도 함께 지워요.` : ""}`
        : "이 카드 종류로 만든 카드는 없어요.",
    });
  }
  for (const f of impact.fields)
    items.push({
      icon: "trash",
      tone: "warn",
      title: `"${f.name}" 필드를 지워요`,
      text: `${f.notes ? `노트 ${n(f.notes)}개에 든 "${f.name}" 내용은 지우지 않고 숨겨요.` : `"${f.name}"에 내용이 든 노트는 없어요.`} 카드 추가 · 편집 칸과 내보내기에서 빠져요.`,
    });
  if (impact.made)
    items.push({ icon: "plus", tone: "info", title: `카드 ${n(impact.made)}장이 새로 생겨요`, text: "앞면이 비지 않는 노트마다 새 카드 종류의 카드가 한 장씩 생겨요." });
  const backup = removes ? "저장하기 전에 이 기기 백업을 먼저 만들어요." : null;
  const one = items.length === 1 ? items[0] : null;
  if (one && impact.fields.length) one.title = `"${impact.fields[0].name}" 필드를 지울까요?`; // 창 하나면 묻는 꼴 (Figma NT5 ③)
  const saveLabel = one
    ? impact.templates[0]?.cards
      ? `카드 ${n(impact.templates[0].cards)}장 지우기`
      : impact.fields.length
        ? "필드 지우기"
        : impact.empty_fronts
          ? "그래도 저장"
          : "저장"
    : "확인하고 저장";
  const danger = impact.templates.some((t) => t.cards > 0);
  return (
    <Modal
      title={one ? one.title : "저장하기 전에 확인해 주세요"}
      onClose={onBack}
      variant="confirm"
      icon={one ? one.icon : "alert"}
      className={`nte-ask nte-ask--${one ? one.tone : danger ? "danger" : "warn"}`}
    >
      {one ? (
        <p className="t-body-s c-secondary">{one.text}</p>
      ) : (
        <ul className="nte-ask__list">
          {items.map((x) => (
            <li key={x.title} className={`nte-ask__item nte-ask__item--${x.tone}`}>
              <span className="nte-ask__icon">
                <Icon name={x.icon} size={16} />
              </span>
              <span className="nte-ask__text">
                <span className="t-label-m">{x.title}</span>
                <span className="t-body-s c-secondary">{x.text}</span>
              </span>
            </li>
          ))}
        </ul>
      )}
      {backup && <p className="t-caption c-tertiary">{backup}</p>}
      <div className="modal__actions">
        <Button variant="ghost" size="M" label={one?.tone === "danger" || one?.icon === "trash" ? "취소" : "돌아가기"} shortcut="Esc" onClick={onBack} />
        <Button variant={danger ? "danger" : "primary"} size="M" icon={danger ? "trash" : undefined} label={saveLabel} busy={busy} busyLabel="저장하는 중" autoFocus onClick={onSave} />
      </div>
    </Modal>
  );
}

/** K39 다른 기기와 동시에 고쳐 진 쪽 정의. 진 쪽 HTML 을 복사해 다시 넣을 수 있게 그대로 보인다 (지난 30일) */
function ConflictList({ list, onClose }: { list: NoteTypeConflict[]; onClose: () => void }) {
  useKey("Escape", onClose, true, { modal: true });
  return (
    <Modal title="동시에 고친 내용" sub="두 기기에서 같은 곳을 고친 기록이에요. 30일 동안 여기 남아요" wide onClose={onClose}>
      <ul className="nte-conflicts">
        {list.map((c, i) =>
          keptBoth(c) ? (
            // 잃은 것이 없다: 진 쪽 · 이긴 쪽이 아니라 두 필드가 놓인 자리 (디자인 검토 a)
            <li key={i} className="nte-conflicts__item">
              <span className="t-label-m">{KEPT_BOTH}</span>
              <span className="t-caption c-tertiary">{when(c.at)}</span>
              <span className="t-body-s">
                이 자리 · {c.winner} ({c.winner_device})
              </span>
              <span className="t-body-s">
                다음 자리로 옮김 · {c.loser} ({c.loser_device})
              </span>
            </li>
          ) : (
            <li key={i} className="nte-conflicts__item">
              <span className="t-label-m">{c.what}</span>
              <span className="t-caption c-tertiary">
                지금 내용 · {c.winner_device} · {when(c.at)}
              </span>
              <pre className="nte-conflicts__text t-body-s">{c.winner || "(비어 있음)"}</pre>
              <span className="t-caption c-tertiary">진 쪽 · {c.loser_device}</span>
              <pre className="nte-conflicts__text t-body-s">{c.loser || "(비어 있음)"}</pre>
            </li>
          ),
        )}
      </ul>
      <div className="modal__actions">
        <Button size="M" label="닫기" onClick={onClose} />
      </div>
    </Modal>
  );
}
