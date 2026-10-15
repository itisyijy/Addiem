// M 그림 가리기: M2 가리기 편집기 · M1 그림 미리보기 · 카드 편집(E)에서 여는 편집기
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api, errorText } from "../api";
import { Badge, Button, Icon } from "../ui";
import { plainText } from "../cardHtml";
import { n, useOverlayScope } from "../nav";
import { Modal } from "./NewDeck";
import { boxCount, labelOf, naturalSize, occlusionApi, ordinalsOf, storeImage, type OcclusionNote, type Picture, type Shape } from "../occlusion";
import "../styles/occlusion.css";

const pct = (v: number) => `${v * 100}%`;

/** 칸 하나의 위치 (그림 대비 %). 다각형은 외접 사각형 안에서 clip-path 로 자른다 */
function boxStyle(s: Shape): React.CSSProperties {
  const style: React.CSSProperties = { left: pct(s.left), top: pct(s.top), width: pct(s.width), height: pct(s.height) };
  if (s.kind === "polygon" && s.points.length >= 3 && s.width > 0 && s.height > 0) {
    style.clipPath = `polygon(${s.points.map(([x, y]) => `${((x - s.left) / s.width) * 100}% ${((y - s.top) / s.height) * 100}%`).join(",")})`;
  }
  const angle = Number(s.extra.find(([k]) => k === "angle")?.[1] ?? 0);
  if (angle) style.transform = `rotate(${((angle % 10000) / 10000) * 360}deg)`;
  return style;
}

/**
 * 그림 + 칸. ask 가 없으면 번호를 보인다 (M1). ask 가 있으면 학습 면 모양 (묻는 칸 강조, 나머지는 모두 가림일 때만 덮개)
 */
export function OcclusionPicture({ url, shapes, hideAll, ask, className = "" }: {
  url: string;
  shapes: Shape[];
  hideAll: boolean;
  ask?: number;
  className?: string;
}) {
  return (
    <div className={`io ${className}`}>
      <img src={url} alt="" draggable={false} />
      {shapes.map((s, i) => {
        if (s.kind === "text" || s.ordinal === 0) return null;
        if (ask === undefined) {
          return (
            <div key={i} className={`io-mask io-mask--num ${s.kind === "ellipse" ? "io-mask--ellipse" : ""}`} style={boxStyle(s)}>
              <span>{s.ordinal}</span>
            </div>
          );
        }
        if (s.ordinal !== ask && !hideAll) return null;
        return <div key={i} className={`io-mask ${s.kind === "ellipse" ? "io-mask--ellipse" : ""} ${s.ordinal === ask ? "io-mask--ask" : ""}`} style={boxStyle(s)} />;
      })}
    </div>
  );
}

// ---------------------------------------------------------------- M4 카드 목록 행

type Thumb = { url: string; shapes: Shape[] };
const thumbs = new Map<number, Promise<Thumb>>(); // 노트 id → 그림 · 칸. 목록에 같은 노트 카드가 여러 장이라 한 번만 읽는다
const thumbOf = (noteId: number) => {
  if (!thumbs.has(noteId))
    thumbs.set(
      noteId,
      occlusionApi.note(noteId).then(async (o) => ({ url: o.image ? await api.readMedia(o.image).catch(() => "") : "", shapes: o.shapes })),
    );
  return thumbs.get(noteId)!;
};

/** M4 행의 질문 칸: 120px 축소 그림(묻는 칸만 파랑) · "그림 가리기 · n/N" · 칸 이름 · 질문 */
export function OcclusionRow({ noteId, pos, question }: { noteId: number; pos: { ordinal: number; n: number; count: number }; question: string }) {
  const [t, setT] = useState<Thumb | null>(null);
  useEffect(() => {
    let live = true;
    thumbOf(noteId).then((x) => live && setT(x), () => thumbs.delete(noteId));
    return () => void (live = false);
  }, [noteId]);
  const label = t ? labelOf(t.shapes, pos.ordinal) : "";
  return (
    <span className="io-row">
      <span className="io-row__thumb">{t?.url && <OcclusionPicture url={t.url} shapes={t.shapes} hideAll={false} ask={pos.ordinal} className="io--thumb" />}</span>
      <span className="io-row__text">
        <span className="io-row__head">
          <Badge tone="accent">
            그림 가리기 · {n(pos.n)}/{n(pos.count)}
          </Badge>
          <span className="t-body-m">{label || `${n(pos.ordinal)}번 칸`}</span>
        </span>
        <span className="t-caption c-tertiary">{question}</span>
      </span>
    </span>
  );
}

// ---------------------------------------------------------------- M2 편집기

type EShape = Shape & { key: number };
type Tool = "select" | "rect" | "ellipse";
type Drag =
  | { kind: "move"; start: [number, number]; base: EShape[]; moved: boolean }
  | { kind: "resize"; key: number; anchor: [number, number]; base: EShape[]; moved: boolean }
  | { kind: "pan"; x: number; y: number; left: number; top: number };

const strip = (list: EShape[]): Shape[] => list.map(({ key: _key, ...s }) => s);
const clamp = (v: number) => Math.min(1, Math.max(0, v));

/** 카드 편집(E)에서 고치는 질문(머리말) · 추가 설명. 글자만 고친다 (고치지 않으면 원래 HTML 그대로) */
export type IoTexts = { header: string; backExtra: string };

export function OcclusionEditor({ picture, shapes: initial, hideAll: initialHideAll, knownMax = 0, reviewed = [], focus, texts, onDone, onCancel, onReplaceImage }: {
  /** 비었으면(url "") 가져온 노트에 그림이 없는 것 (M5) */
  picture: Picture;
  shapes: Shape[];
  hideAll: boolean;
  /** 이 노트에서 이미 쓴 가장 큰 번호. 지운 번호를 다시 쓰지 않는다 (기록이 엉뚱한 칸에 붙지 않게) */
  knownMax?: number;
  /** 복습 기록이 있는 카드 번호. 지우면 완료할 때 묻는다 (C2-d) */
  reviewed?: number[];
  /** 이 번호의 칸을 고른 채로 연다 (M4) */
  focus?: number;
  /** 있으면 패널에 질문 · 추가 설명 칸 (이미 있는 노트를 E 로 열 때) */
  texts?: IoTexts;
  onDone: (shapes: Shape[], hideAll: boolean, texts?: IoTexts) => void;
  onCancel: () => void;
  onReplaceImage?: () => void;
}) {
  useOverlayScope();
  const seq = useRef(0);
  const [shapes, setShapes] = useState<EShape[]>(() => initial.map((s) => ({ ...s, key: ++seq.current })));
  const [hideAll, setHideAll] = useState(initialHideAll);
  const [sel, setSel] = useState<number[]>(() => (focus === undefined ? [] : shapes.filter((s) => s.ordinal === focus).map((s) => s.key)));
  const [tool, setTool] = useState<Tool>("rect");
  const [zoom, setZoom] = useState(1);
  const [naming, setNaming] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [ask, setAsk] = useState<null | "leave" | "reviewed">(null);
  const [header, setHeader] = useState(() => plainText(texts?.header ?? ""));
  const [extra, setExtra] = useState(() => plainText(texts?.backExtra ?? ""));
  const past = useRef<EShape[][]>([]);
  const future = useRef<EShape[][]>([]);
  const maxSeen = useRef(Math.max(knownMax, 0, ...initial.map((s) => s.ordinal)));
  const box = useRef<HTMLDivElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const space = useRef(false);
  const latest = useRef({ shapes, sel });
  latest.current = { shapes, sel };
  const nat = { w: picture.width || 1, h: picture.height || 1 };

  // 처음에는 화면에 맞춘다 (100% 넘게 키우지 않는다)
  useEffect(() => {
    const el = stage.current;
    if (el && picture.width) setZoom(Math.min(1, (el.clientWidth - 96) / picture.width, (el.clientHeight - 96) / picture.height));
  }, [picture.width, picture.height]);

  const record = (prev: EShape[]) => {
    past.current = [...past.current.slice(-49), prev];
    future.current = [];
  };
  const commit = (next: EShape[]) => {
    record(latest.current.shapes);
    setShapes(next);
  };
  const newOrdinal = () => ++maxSeen.current;
  const groupOf = (key: number, list = shapes) => {
    const o = list.find((s) => s.key === key)?.ordinal;
    return list.filter((s) => s.ordinal === o).map((s) => s.key);
  };
  const groups = ordinalsOf(shapes);
  const selectedOrdinals = [...new Set(shapes.filter((s) => sel.includes(s.key)).map((s) => s.ordinal))];

  const point = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = box.current!.getBoundingClientRect();
    return [clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)];
  };

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0 || !picture.url) return;
    if (space.current || e.button === 0 && e.altKey) {
      const el = stage.current!;
      drag.current = { kind: "pan", x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
      return;
    }
    const target = e.target as HTMLElement;
    const handle = target.closest<HTMLElement>("[data-handle]");
    const hit = target.closest<HTMLElement>("[data-key]");
    const at = point(e);
    if (handle && hit) {
      const s = shapes.find((x) => x.key === Number(hit.dataset.key))!;
      const [h, v] = [handle.dataset.handle!.includes("w"), handle.dataset.handle!.includes("n")];
      drag.current = { kind: "resize", key: s.key, anchor: [h ? s.left + s.width : s.left, v ? s.top + s.height : s.top], base: shapes, moved: false };
      return;
    }
    if (hit) {
      const g = groupOf(Number(hit.dataset.key));
      const next = e.shiftKey ? (g.every((k) => sel.includes(k)) ? sel.filter((k) => !g.includes(k)) : [...sel, ...g]) : g.some((k) => sel.includes(k)) ? sel : g;
      setSel(next);
      drag.current = { kind: "move", start: at, base: shapes, moved: false };
      return;
    }
    if (tool === "select") {
      setSel([]);
      return;
    }
    // 그리기: 연속 그리기라 도구는 그대로 둔다
    const s: EShape = { key: ++seq.current, ordinal: newOrdinal(), kind: tool, left: at[0], top: at[1], width: 0, height: 0, points: [], label: "", extra: [] };
    const base = [...shapes, s];
    setShapes(base);
    setSel([s.key]);
    drag.current = { kind: "resize", key: s.key, anchor: at, base: shapes, moved: false };
  };

  useEffect(() => {
    const move = (e: PointerEvent) => {
      const d = drag.current;
      if (!d) return;
      if (d.kind === "pan") {
        stage.current!.scrollLeft = d.left - (e.clientX - d.x);
        stage.current!.scrollTop = d.top - (e.clientY - d.y);
        return;
      }
      const at = point(e);
      d.moved = true;
      if (d.kind === "move") {
        const { sel } = latest.current;
        const moving = d.base.filter((s) => sel.includes(s.key));
        // 그림 밖으로 나가지 않게 묶음 전체의 이동량을 자른다
        const dx = Math.min(Math.max(at[0] - d.start[0], -Math.min(...moving.map((s) => s.left))), 1 - Math.max(...moving.map((s) => s.left + s.width)));
        const dy = Math.min(Math.max(at[1] - d.start[1], -Math.min(...moving.map((s) => s.top))), 1 - Math.max(...moving.map((s) => s.top + s.height)));
        setShapes(d.base.map((s) => (sel.includes(s.key) ? shift(s, dx, dy) : s)));
      } else {
        const [ax, ay] = d.anchor;
        setShapes((list) =>
          list.map((s) =>
            s.key === d.key
              ? { ...s, left: Math.min(ax, at[0]), top: Math.min(ay, at[1]), width: Math.abs(at[0] - ax), height: Math.abs(at[1] - ay) }
              : s,
          ),
        );
      }
    };
    const up = () => {
      const d = drag.current;
      drag.current = null;
      if (!d || d.kind === "pan") return;
      const { shapes } = latest.current;
      if (d.kind === "resize") {
        const s = shapes.find((x) => x.key === d.key);
        // 5px 보다 작으면 실수로 누른 것 (Anki 도 버린다)
        if (s && (s.width * nat.w < 5 || s.height * nat.h < 5)) {
          if (!d.base.some((x) => x.key === d.key)) {
            setShapes(d.base);
            setSel([]);
            maxSeen.current = Math.max(knownMax, ...d.base.map((x) => x.ordinal), s.ordinal - 1);
          } else setShapes(d.base);
          return;
        }
      }
      if (d.moved || (d.kind === "resize" && !d.base.some((x) => x.key === d.key))) record(d.base);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  });

  const removeSelected = () => {
    if (!sel.length) return;
    commit(shapes.filter((s) => !sel.includes(s.key)));
    setSel([]);
  };
  const nudge = (dx: number, dy: number) => commit(shapes.map((s) => (sel.includes(s.key) ? shift(s, dx / nat.w, dy / nat.h) : s)));
  const duplicate = () => {
    const fresh = new Map<number, number>();
    const copies = shapes
      .filter((s) => sel.includes(s.key))
      .map((s) => {
        if (!fresh.has(s.ordinal)) fresh.set(s.ordinal, newOrdinal());
        return { ...shift(s, 10 / nat.w, 10 / nat.h), key: ++seq.current, ordinal: fresh.get(s.ordinal)!, label: "" };
      });
    commit([...shapes, ...copies]);
    setSel(copies.map((c) => c.key));
  };
  // 묶으면 가장 작은 번호(그 카드와 기록)가 남는다. 풀면 떨어진 칸이 새 번호를 받는다 (DECISIONS M11)
  const group = () => {
    if (selectedOrdinals.length < 2) return;
    const keep = Math.min(...selectedOrdinals);
    commit(shapes.map((s) => (selectedOrdinals.includes(s.ordinal) ? { ...s, ordinal: keep, label: s.ordinal === keep ? s.label : "" } : s)));
  };
  const ungroup = () => {
    const seen = new Set<number>();
    commit(
      shapes.map((s) => {
        if (!sel.includes(s.key)) return s;
        if (!seen.has(s.ordinal)) {
          seen.add(s.ordinal);
          return s;
        }
        return { ...s, ordinal: newOrdinal(), label: "" };
      }),
    );
  };
  const undo = () => {
    const prev = past.current.pop();
    if (!prev) return;
    future.current.push(shapes);
    setShapes(prev);
    setSel((s) => s.filter((k) => prev.some((x) => x.key === k)));
  };
  const redo = () => {
    const next = future.current.pop();
    if (!next) return;
    past.current.push(shapes);
    setShapes(next);
  };
  const cycle = (dir: number) => {
    if (!groups.length) return;
    const cur = groups.indexOf(selectedOrdinals[0]);
    const o = groups[(cur + dir + groups.length) % groups.length];
    setSel(shapes.filter((s) => s.ordinal === o).map((s) => s.key));
  };
  const rename = (o: number, label: string) => {
    let first = true;
    commit(
      shapes.map((s) => {
        if (s.ordinal !== o) return s;
        const l = first ? label.trim() : "";
        first = false;
        return { ...s, label: l };
      }),
    );
    setNaming(null);
  };
  const startNaming = (o: number) => {
    setDraft(labelOf(shapes, o));
    setNaming(o);
  };
  const removed = () => {
    const now = new Set(ordinalsOf(shapes));
    return reviewed.filter((o) => !now.has(o));
  };
  const finish = (force = false) => {
    if (!groups.length) return;
    if (!force && removed().length) return setAsk("reviewed");
    // 글자를 고치지 않았으면 원래 HTML(서식 · 그림)을 그대로 둔다
    const keep = (text: string, html: string) => (text === plainText(html) ? html : escapeLines(text));
    onDone(strip(shapes), hideAll, texts && { header: keep(header, texts.header), backExtra: keep(extra, texts.backExtra) });
  };
  const zoomBy = (f: number) => setZoom((z) => Math.min(4, Math.max(0.1, z * f)));

  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      const typing = e.target instanceof Element && e.target.closest("input, textarea, [contenteditable]");
      if (typing || ask) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const run = (f: () => void) => {
        e.preventDefault();
        e.stopPropagation();
        f();
      };
      if (e.code === "Space") return run(() => (space.current = true));
      if (ctrl && e.key === "Enter") return run(() => finish());
      if (ctrl && e.code === "KeyZ") return run(e.shiftKey ? redo : undo);
      if (ctrl && e.code === "KeyY") return run(redo);
      if (ctrl && e.code === "KeyD") return run(duplicate);
      if (ctrl && e.code === "KeyG") return run(e.shiftKey ? ungroup : group);
      if (ctrl && (e.code === "Equal" || e.code === "NumpadAdd")) return run(() => zoomBy(1.25));
      if (ctrl && (e.code === "Minus" || e.code === "NumpadSubtract")) return run(() => zoomBy(0.8));
      if (ctrl || e.altKey) return;
      const step = e.shiftKey ? 10 : 1;
      const arrows: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
      if (arrows[e.key] && sel.length) return run(() => nudge(...arrows[e.key]));
      if (e.key === "Delete" || e.key === "Backspace") return run(removeSelected);
      if (e.key === "Tab") return run(() => cycle(e.shiftKey ? -1 : 1));
      if (e.key === "Enter" && selectedOrdinals.length) return run(() => startNaming(selectedOrdinals[0]));
      if (e.key === "Escape") return run(() => (sel.length ? setSel([]) : setAsk("leave")));
      const tools: Record<string, Tool> = { KeyV: "select", KeyR: "rect", KeyO: "ellipse" };
      if (tools[e.code]) return run(() => setTool(tools[e.code]));
    };
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space") space.current = false;
    };
    // 화면 단축키(카드 추가 · 학습)보다 먼저 받는다
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
    };
  });

  const boxes = boxCount(shapes);
  const single = sel.length === 1 ? shapes.find((s) => s.key === sel[0]) : undefined;
  const small = picture.width > 0 && Math.min(picture.width, picture.height) < 240;

  return (
    <div className="ioe" role="dialog" aria-label="가릴 곳 그리기">
      <header className="ioe__bar">
        <button className="ioe__back" aria-label="닫기" onClick={() => setAsk("leave")}>
          <Icon name="chevron-left" size={20} />
        </button>
        <span className="t-title-s">가릴 곳 그리기</span>
        <span className="t-caption c-tertiary ioe__file">{picture.fileName}</span>
        <div className="segment segment--boxed ioe__tools" role="toolbar" aria-label="도구">
          {(
            [
              ["select", "선택", "V"],
              ["rect", "사각형", "R"],
              ["ellipse", "타원", "O"],
            ] as const
          ).map(([t, label, key]) => (
            <button key={t} className={`segment__item t-label-m ${tool === t ? "is-selected" : ""}`} aria-pressed={tool === t} onClick={() => setTool(t)}>
              {label} <kbd className="kbd kbd--default">{key}</kbd>
            </button>
          ))}
        </div>
        <span className="ioe__right">
          <Button variant="ghost" size="M" icon="rotate" label="실행 취소" onClick={undo} disabled={!past.current.length} />
          <span className="t-label-m c-secondary">{Math.round(zoom * 100)}%</span>
          <Button icon="check" size="M" label="완료" shortcut="Ctrl Enter" disabled={!groups.length} onClick={() => finish()} />
        </span>
      </header>

      <div
        ref={stage}
        className="ioe__stage"
        onWheel={(e) => {
          if (e.ctrlKey) zoomBy(e.deltaY < 0 ? 1.1 : 0.9);
        }}
      >
        {picture.url ? (
          <div
            ref={box}
            className={`ioe__canvas ioe__canvas--${tool}`}
            style={{ width: picture.width * zoom, height: picture.height * zoom }}
            onPointerDown={onPointerDown}
          >
            <img src={picture.url} alt="" draggable={false} />
            {shapes.map((s) =>
              s.kind === "text" || s.ordinal === 0 ? (
                <div key={s.key} className="io-label" style={{ left: pct(s.left), top: pct(s.top) }}>
                  {s.extra.find(([k]) => k === "text")?.[1]}
                </div>
              ) : (
                <div
                  key={s.key}
                  data-key={s.key}
                  className={`ioe__shape ${s.kind === "ellipse" ? "io-mask--ellipse" : ""} ${sel.includes(s.key) ? "is-selected" : ""}`}
                  style={boxStyle(s)}
                >
                  <span className="ioe__num">{s.ordinal}</span>
                  {single?.key === s.key &&
                    s.kind !== "polygon" &&
                    ["nw", "ne", "sw", "se"].map((h) => <i key={h} data-handle={h} className={`ioe__handle ioe__handle--${h}`} />)}
                </div>
              ),
            )}
            <svg className="ioe__links" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden>
              {groups.flatMap((o) => {
                const g = shapes.filter((s) => s.ordinal === o);
                return g.slice(1).map((s, i) => {
                  const a = g[i];
                  return (
                    <line
                      key={`${o}-${i}`}
                      x1={(a.left + a.width / 2) * 100}
                      y1={(a.top + a.height / 2) * 100}
                      x2={(s.left + s.width / 2) * 100}
                      y2={(s.top + s.height / 2) * 100}
                    />
                  );
                });
              })}
            </svg>
          </div>
        ) : (
          <div className="ioe__missing">
            <p className="t-body-m c-secondary">그림을 찾을 수 없어요. 칸 {n(boxes)}개는 그대로 있어요.</p>
            {onReplaceImage && <Button variant="secondary" size="M" icon="image" label="그림 다시 넣기" onClick={onReplaceImage} />}
          </div>
        )}
        <p className="ioe__hint t-caption c-secondary">끌어서 그리기 · Space+끌기 화면 이동 · Ctrl+휠 확대</p>
      </div>

      <aside className="ioe__panel">
        <p className="t-title-s">
          칸 {n(boxes)}개 · 카드 {n(groups.length)}장
        </p>
        {small && <p className="t-caption c-secondary ioe__warn">그림이 작아요 (짧은 변 {Math.min(picture.width, picture.height)}px). 글자가 흐릴 수 있어요.</p>}
        {boxes > 40 && <p className="t-caption c-secondary ioe__warn">칸이 {n(boxes)}개예요. 한 그림에 너무 많으면 복습이 길어져요.</p>}
        <span className="t-label-s c-secondary">묻는 방식</span>
        <div className="segment segment--boxed">
          {(
            [
              [false, "하나만 가리기"],
              [true, "모두 가리고 하나씩"],
            ] as const
          ).map(([v, label]) => (
            <button key={label} className={`segment__item t-label-m ${hideAll === v ? "is-selected" : ""}`} onClick={() => setHideAll(v)}>
              {label}
            </button>
          ))}
        </div>
        <p className="t-caption c-tertiary">
          모두 가리고 하나씩: 나머지 칸도 회색으로 가린 채 한 칸만 물어요. 하나만 가리기: 묻는 칸만 가리고 나머지는 보여요.
        </p>
        {groups.length > 0 ? (
          <ul className="ioe__list">
            {groups.map((o) => {
              const count = shapes.filter((s) => s.ordinal === o).length;
              const on = selectedOrdinals.includes(o);
              return (
                <li key={o} className={on ? "is-selected" : ""} onClick={() => setSel(shapes.filter((s) => s.ordinal === o).map((s) => s.key))}>
                  <span className="ioe__badge">{o}</span>
                  {naming === o ? (
                    <input
                      className="ioe__name t-body-s"
                      autoFocus
                      value={draft}
                      maxLength={40}
                      placeholder="칸 이름"
                      aria-label={`${o}번 칸 이름`}
                      onChange={(e) => setDraft(e.target.value)}
                      onBlur={() => rename(o, draft)}
                      onKeyDown={(e) => {
                        e.stopPropagation();
                        if (e.key === "Enter") rename(o, draft);
                        if (e.key === "Escape") setNaming(null);
                      }}
                    />
                  ) : (
                    <button className={`ioe__name t-body-s ${labelOf(shapes, o) ? "" : "c-tertiary"}`} onDoubleClick={() => startNaming(o)} onClick={() => on && startNaming(o)}>
                      {labelOf(shapes, o) || "이름 없음"}
                    </button>
                  )}
                  {count > 1 && <span className="ioe__chip t-caption">{count}칸 묶음</span>}
                  <button
                    className="ioe__del"
                    aria-label={`${o}번 칸 지우기`}
                    onClick={(e) => {
                      e.stopPropagation();
                      commit(shapes.filter((s) => s.ordinal !== o));
                      setSel([]);
                    }}
                  >
                    <Icon name="trash" size={16} />
                  </button>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="t-body-s c-tertiary ioe__empty">그림 위를 끌어서 가릴 곳을 그려요. 칸마다 카드가 한 장 생겨요.</p>
        )}
        <div className="ioe__group">
          <Button variant="secondary" size="M" icon="layers" label="묶기" shortcut="Ctrl G" disabled={selectedOrdinals.length < 2} onClick={group} />
          <Button variant="ghost" size="M" label="풀기" disabled={!sel.length} onClick={ungroup} />
        </div>
        {texts && (
          <>
            <label className="ioe__text">
              <span className="t-label-s c-secondary">질문</span>
              <textarea className="t-body-s" rows={2} value={header} placeholder="비워 두면 “가린 곳에 들어갈 말은?”" onChange={(e) => setHeader(e.target.value)} />
            </label>
            <label className="ioe__text">
              <span className="t-label-s c-secondary">추가 설명 · 답을 본 뒤에만 보여요</span>
              <textarea className="t-body-s" rows={2} value={extra} onChange={(e) => setExtra(e.target.value)} />
            </label>
          </>
        )}
        <span className="t-label-s c-secondary">단축키</span>
        <dl className="ioe__keys t-caption c-secondary">
          {[
            ["Delete", "선택한 칸 지우기"],
            ["방향키", "1px 이동 · Shift 10px"],
            ["Ctrl D", "복제"],
            ["Tab", "다음 칸 · Shift Tab 이전"],
            ["Enter", "칸 이름 쓰기"],
            ["Ctrl Z", "실행 취소 · Ctrl Shift Z 다시"],
          ].map(([k, v]) => (
            <div key={k}>
              <dt>
                <kbd className="kbd kbd--default">{k}</kbd>
              </dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
      </aside>

      {ask === "leave" && (
        <Modal title="편집을 마칠까요?" onClose={() => setAsk(null)}>
          <div className="modal__body">
            <p className="t-body-m c-secondary">완료를 누르면 지금 칸으로 카드를 만들어요.</p>
            <div className="modal__actions">
              <Button variant="ghost" size="M" label="계속 편집" shortcut="Esc" onClick={() => setAsk(null)} />
              <Button variant="danger" size="M" label="바꾼 것 버리기" onClick={onCancel} />
              <Button size="M" label="완료" disabled={!groups.length} onClick={() => (setAsk(null), finish())} />
            </div>
          </div>
        </Modal>
      )}
      {ask === "reviewed" && (
        <Modal title={`복습한 카드 ${n(removed().length)}장이 지워져요`} onClose={() => setAsk(null)}>
          <div className="modal__body">
            <p className="t-body-m c-secondary">
              {removed().map((o) => `${o}번 칸`).join(", ")}을 지웠어요. 그 카드와 복습 기록이 함께 지워져요.
            </p>
            <div className="modal__actions">
              <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setAsk(null)} />
              <Button variant="danger" size="M" label="지우고 완료" onClick={() => (setAsk(null), finish(true))} />
            </div>
          </div>
        </Modal>
      )}
    </div>
  );
}

const escapeLines = (s: string) =>
  s.trim().replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/\n/g, "<br>");

function shift<T extends Shape>(s: T, dx: number, dy: number): T {
  return { ...s, left: s.left + dx, top: s.top + dy, points: s.points.map(([x, y]) => [x + dx, y + dy] as [number, number]) };
}

// ---------------------------------------------------------------- 카드 편집 (E) · 카드 목록에서 여는 편집기

/** 가져온 노트처럼 이미 있는 그림 가리기 노트. 칸 · 질문(머리말) · 추가 설명(덧붙임)을 고친다 */
export function OcclusionEdit({ noteId, focus, onClose, onSaved }: { noteId: number; focus?: number; onClose: () => void; onSaved: () => void }) {
  const [note, setNote] = useState<OcclusionNote | null>(null);
  const [picture, setPicture] = useState<Picture | null>(null);
  const [error, setError] = useState<string>();
  const file = useRef<HTMLInputElement>(null);

  useEffect(() => {
    occlusionApi.note(noteId).then(async (o) => {
      setNote(o);
      const url = o.image ? await api.readMedia(o.image).catch(() => "") : "";
      setPicture({ name: o.image, url, fileName: o.image, ...(await naturalSize(url)) });
    }, (e) => setError(errorText(e)));
  }, [noteId]);

  if (error) {
    return (
      <Modal title="그림 가리기를 열지 못했어요" onClose={onClose}>
        <div className="modal__body">
          <p className="t-body-m c-danger">{error}</p>
        </div>
      </Modal>
    );
  }
  if (!note || !picture) return null;
  return (
    <>
      <OcclusionEditor
        picture={picture}
        shapes={note.shapes}
        hideAll={note.hide_all}
        knownMax={Math.max(0, ...note.cards.map((c) => c.ordinal))}
        reviewed={note.cards.filter((c) => c.reviewed).map((c) => c.ordinal)}
        focus={focus}
        texts={{ header: note.header, backExtra: note.back_extra }}
        onCancel={onClose}
        onReplaceImage={() => file.current?.click()}
        onDone={(shapes, hideAll, t) =>
          occlusionApi
            .save({
              note_id: noteId,
              deck_id: note.deck_id,
              image: picture.name,
              header: t?.header ?? note.header,
              back_extra: t?.backExtra ?? note.back_extra,
              shapes,
              hide_all: hideAll,
              importance: null,
            })
            .then(() => {
              thumbs.delete(noteId); // M4 축소 그림을 새로 읽게
              onSaved();
            }, (e) => setError(errorText(e)))
        }
      />
      <input
        ref={file}
        type="file"
        accept="image/png,image/jpeg,image/webp,image/bmp"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) storeImage(f).then(setPicture, (err) => setError(errorText(err)));
        }}
      />
    </>
  );
}
