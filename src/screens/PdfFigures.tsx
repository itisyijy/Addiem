// 8.4 PDF 그림 · 표 고르기 (Figma IT1 157:13787 · 노트 170:14075, DECISIONS IT1 · IT2 · IT4 · IT7 · IT8 · IT10).
// 왼쪽은 큰 쪽 보기: 찾은 그림 · 확정한 표는 실선 + 이름표, 표 제안 · 끌어서 잡은 영역은 점선 + 모서리 손잡이.
// 오른쪽은 문서 전체의 그림 · 표 · 표 제안 목록 (고르기 · 차례로 만들기). 제안만으로는 아무것도 만들지 않는다 (IT2).
// 스캔본(글자 없음)도 쪽을 그려 끌어서 잡은 영역으로 그림 카드만 만든다 (IT10)
import { useEffect, useRef, useState, type ReactNode } from "react";
import { errorText } from "../api";
import { n, useKey, useOverlayScope } from "../nav";
import { useToast } from "../toast";
import { Button, Icon } from "../ui";
import { cropPage, cropSize, pageLayout } from "../pdf";
import { boxOf, captionOf, figuresOf, gridIn, rectOf, suggestTables, type Layout } from "../figures";
import { PdfPage } from "./Sources";
import { cellsOf, FigList, FigMenu, sameRect, storeDataUrl, useBulk, useFlows, type FigRow, type FigSource, type MenuEntry } from "./Figures";
import { isRegion, sourceApi, type Material, type Rect, type Segment } from "../sourceApi";

// ---------------------------------------------------------------- 문서 전체 읽기 · 찾은 것

export type Scan = { id: number; list: (Layout | null)[] };

/** 문서를 한 쪽씩 읽는다 (그림 상자 · 글자 · 괘선, pdf.ts pageLayout 이 쪽마다 들고 있다). 자료를 고르면 시작한다.
 * ponytail: 쪽 수만큼 연산자 목록을 받는다. 수백 쪽이면 몇 초 걸린다 (목록 머리에 "살피는 중 N/M"), 느리면 보이는 쪽부터 */
export function usePdfScan(m: Material | undefined): Scan | null {
  const id = m && m.kind === "pdf" && m.status !== "pending" && m.pages > 0 ? m.id : undefined;
  const pages = m?.pages ?? 0;
  const [scan, setScan] = useState<Scan | null>(null);
  useEffect(() => {
    setScan(null);
    if (!id) return;
    let live = true;
    (async () => {
      const list: (Layout | null)[] = [];
      for (let p = 1; p <= pages && live; p++) {
        list.push(await pageLayout(id, p).catch(() => null));
        if (live) setScan({ id, list: [...list] });
      }
    })();
    return () => {
      live = false;
    };
  }, [id, pages]);
  return scan?.id === id ? scan : null;
}

type Finds = { figs: { rect: Rect; caption: string }[]; suggs: { rect: Rect; rows: number; cols: number; caption: string }[] };
const finds = new WeakMap<Layout, Finds>();
function findsOf(lay: Layout): Finds {
  let f = finds.get(lay);
  if (!f) {
    f = {
      figs: figuresOf(lay).map((b) => ({ rect: rectOf(lay, b), caption: captionOf(lay, b) })),
      suggs: suggestTables(lay).map((c) => ({ rect: rectOf(lay, c.box), rows: c.rows, cols: c.cols, caption: captionOf(lay, c.box) })),
    };
    finds.set(lay, f);
  }
  return f;
}

export type PdfItem = {
  /** f…: 찾은 그림 · s…: 표 제안 · g{구간 id}: 카드가 이어진 영역 */
  key: string;
  page: number;
  kind: "figure" | "table";
  rect: Rect;
  /** 이름표 글 ("그림 3-4 · 회귀선과 잔차"), 없으면 "" */
  name: string;
  seg?: Segment;
  /** 표 제안 (확인 전) */
  sugg?: { rows: number; cols: number };
  /** 찾은 그림 (목록에서 뺄 수 있다) */
  auto?: boolean;
  /** 표 크기 "5행 × 3열" */
  size?: string;
};
const keyOf = (kind: "f" | "s", page: number, r: Rect) => `${kind}${page}:${r.join(",")}`;
/** 작은 쪽 넓이 대비 겹친 비율 */
const overlap = (a: Rect, b: Rect) => {
  const w = Math.min(a[0] + a[2], b[0] + b[2]) - Math.max(a[0], b[0]);
  const h = Math.min(a[1] + a[3], b[1] + b[3]) - Math.max(a[1], b[1]);
  return w > 0 && h > 0 ? (w * h) / Math.min(a[2] * a[3], b[2] * b[3]) : 0;
};
const DEFAULT_TEXT = /^(그림|표) · p\.\d+$/;

/** 오른쪽 목록 · 쪽 보기가 같이 쓰는 것: 쪽 순서, 쪽 안은 위에서 아래 (IT4) */
export function pdfItems(scan: Scan | null, segs: Segment[], hidden: string[]): PdfItem[] {
  const regions = segs.filter((s) => isRegion(s) && s.rects?.length);
  const used = new Set<number>();
  const out: PdfItem[] = [];
  const sizeOf = (lay: Layout | null | undefined, r: Rect) => {
    const g = lay ? gridIn(lay, boxOf(lay, r)) : [];
    return g.length ? `${n(g.length)}행 × ${n(Math.max(...g.map((x) => x.length)))}열` : undefined;
  };
  scan?.list.forEach((lay, i) => {
    if (!lay) return;
    const page = i + 1;
    const f = findsOf(lay);
    const here = regions.filter((s) => s.page === page);
    for (const x of f.figs) {
      const seg = here.find((s) => s.signals.includes("figure") && sameRect(s.rects![0], x.rect));
      const key = keyOf("f", page, x.rect);
      if (!seg && hidden.includes(key)) continue;
      if (seg) used.add(seg.id);
      out.push({ key: seg ? `g${seg.id}` : key, page, kind: "figure", rect: x.rect, name: x.caption, seg, auto: true });
    }
    for (const x of f.suggs) {
      const key = keyOf("s", page, x.rect);
      if (hidden.includes(key) || here.some((s) => s.signals.includes("table") && overlap(s.rects![0], x.rect) >= 0.5)) continue;
      out.push({ key, page, kind: "table", rect: x.rect, name: x.caption, sugg: { rows: x.rows, cols: x.cols }, size: `${n(x.rows)}행 × ${n(x.cols)}열` });
    }
  });
  for (const s of regions) {
    if (used.has(s.id)) continue;
    const lay = scan?.list[s.page - 1];
    const kind = s.signals.includes("table") ? "table" : "figure";
    const name = kind === "figure" && !DEFAULT_TEXT.test(s.text) ? s.text : lay ? captionOf(lay, boxOf(lay, s.rects![0])) : "";
    out.push({ key: `g${s.id}`, page: s.page, kind, rect: s.rects![0], name, seg: s, size: kind === "table" ? sizeOf(lay, s.rects![0]) : undefined });
  }
  return out.sort((a, b) => a.page - b.page || a.rect[1] - b.rect[1] || a.rect[0] - b.rect[0]);
}

/** 쪽 안 번호 ("그림 2" · "표 1"). 이름표가 없을 때만 쓴다 */
const numberOf = (items: PdfItem[], it: PdfItem) => items.filter((x) => x.page === it.page && x.kind === it.kind).indexOf(it) + 1;
/** 이름표 앞 번호 ("그림 3-6") */
const CAPTION_NO = /^(그림|표|도표|사진|Fig\.?|Figure|Table)\s*[\dA-Z][\w.-]*/i;
/** 쪽 위 이름표 · 목록 · 창 머리가 같은 이름을 쓴다: 이름표가 있으면 그 번호, 없으면 "그림 n" */
const labelOf = (items: PdfItem[], it: PdfItem) => it.name.match(CAPTION_NO)?.[0] ?? `${it.kind === "figure" ? "그림" : "표"} ${numberOf(items, it)}`;

// 무시한 제안 · 목록에서 뺀 그림은 이 기기에만 둔다 (동기화하지 않는 작은 기록)
const HIDDEN = "neoanki.figures.hidden";
const readHidden = (): Record<string, string[]> => {
  try {
    return JSON.parse(localStorage.getItem(HIDDEN) ?? "{}");
  } catch {
    return {};
  }
};
export function useHidden(materialId: number | undefined) {
  const [all, setAll] = useState(readHidden);
  const list = (materialId && all[materialId]) || [];
  const hide = (key: string) => {
    if (!materialId) return;
    const next = { ...all, [materialId]: [...list, key] };
    setAll(next);
    try {
      localStorage.setItem(HIDDEN, JSON.stringify(next));
    } catch {
      // 저장이 막혀도 이번에는 숨긴다
    }
  };
  return [list, hide] as const;
}

// ---------------------------------------------------------------- 큰 쪽 보기 + 목록

const PAGE_W = 540;
/** 표 창 위 한 줄 (IT7: PDF 는 글자 위치로 나눈다) */
const HOW = "글자 위치로 칸을 나눴어요 · 틀린 칸은 눌러 고쳐요";
const round = (r: Rect) => r.map((v) => Math.round(v * 1e4) / 1e4) as Rect;
const pos = (r: Rect) => ({ left: `${r[0] * 100}%`, top: `${r[1] * 100}%`, width: `${r[2] * 100}%`, height: `${r[3] * 100}%` });
const clamp = (v: number) => Math.min(1, Math.max(0, v));

type Sel = { key: string; page: number; menu: boolean };

export function PdfFigures({ m, items, scanning, scanned, deckId, head, pill, page, onPage, linking, onLinking, links, onChanged, hide }: {
  m: Material;
  items: PdfItem[];
  /** 아직 다 읽지 않았다 */
  scanning: boolean;
  /** 스캔본 (글자 없음): 끌어서 잡은 영역 → 그림 카드만 */
  scanned: boolean;
  deckId: number;
  head: ReactNode;
  pill: ReactNode;
  page: number;
  onPage: (p: number) => void;
  linking?: number;
  onLinking: (segmentId: number | undefined) => void;
  links: (seg: Segment) => ReactNode;
  onChanged: () => void;
  hide: (key: string) => void;
}) {
  const toast = useToast();
  const [lay, setLay] = useState<Layout | null>(null);
  const [sel, setSel] = useState<Sel | null>(null);
  const [drag, setDrag] = useState<{ page: number; rect: Rect } | null>(null);
  const [edit, setEdit] = useState<{ key: string; rect: Rect } | null>(null);
  const over = useRef<HTMLDivElement>(null);
  const moving = useRef<{ anchor: [number, number]; what: "drag" | "edit" } | null>(null);
  useEffect(() => {
    let live = true;
    setLay(null);
    pageLayout(m.id, page).then((l) => live && setLay(l), () => {});
    return () => {
      live = false;
    };
  }, [m.id, page]);

  const on = sel?.page === page ? sel : null;
  useOverlayScope(on !== null);
  useKey("Escape", () => (on?.menu ? setSel({ ...on, menu: false }) : (setSel(null), setDrag(null), setEdit(null))), on !== null, { modal: true });

  const here = items.filter((x) => x.page === page);
  const dragHere = drag?.page === page ? drag.rect : null;
  const byKey = (key: string) => items.find((x) => x.key === key);
  const label = (it: PdfItem) => it.name || labelOf(items, it);
  const where = (p: number) => `${m.file_name} · ${p}쪽`;

  const changed = () => {
    setDrag(null);
    setEdit(null);
    setSel(null);
    onChanged();
  };
  const bulk = useBulk((key) => {
    const it = byKey(key);
    if (!it) return bulk.next();
    onPage(it.page);
    if (it.kind === "table") return openTable(it.page, it.rect, label(it));
    setSel({ key, page: it.page, menu: true });
  });
  // 차례로 만들기는 카드를 만들었을 때만 다음 것으로 (닫거나 못 열면 막대의 [다음]으로 넘긴다)
  const flows = useFlows(deckId, changed, () => bulk.run && bulk.next(), bulk.run ? `차례로 만들기 ${n(bulk.run.at + 1)} / ${n(bulk.run.keys.length)}` : undefined);

  const src = async (p: number, r: Rect, name: string, title: string): Promise<FigSource> => {
    let crop: Promise<string> | undefined;
    const url = () => (crop ??= cropPage(m.id, p, r).then((c) => c.url));
    return {
      region: { material_id: m.id, page: p, rects: [r], kind: "figure", text: name },
      sub: `${title} · ${where(p)}`,
      fileName: where(p),
      alt: name || title,
      deckId: await sourceApi.regionDeck(m.id, p),
      url,
      store: async () => storeDataUrl(await url()),
    };
  };
  const fail = (e: unknown) => {
    toast({ title: "열지 못했어요", text: errorText(e), tone: "error" });
  };
  const openTable = async (p: number, r: Rect, title: string) => {
    try {
      const l = await pageLayout(m.id, p);
      const g = gridIn(l, boxOf(l, r));
      if (!g.length) {
        return toast({ title: "이 영역에서 글자를 찾지 못했어요", text: "그림 카드로 만들 수 있어요." });
      }
      flows.table({ sub: `${title} · ${where(p)}`, how: HOW, rows: cellsOf(g), region: { material_id: m.id, page: p, rects: [r] }, deckId: await sourceApi.regionDeck(m.id, p) });
    } catch (e) {
      fail(e);
    }
  };
  const openItem = (it: PdfItem) => {
    if (it.sugg) {
      onPage(it.page);
      return setSel({ key: it.key, page: it.page, menu: false });
    }
    if (it.kind === "table") return openTable(it.page, it.rect, label(it));
    onPage(it.page);
    setSel({ key: it.key, page: it.page, menu: true });
  };

  // ---- 끌어서 잡기 · 손잡이
  const point = (e: React.PointerEvent): [number, number] => {
    const b = over.current!.getBoundingClientRect();
    return [clamp((e.clientX - b.left) / b.width), clamp((e.clientY - b.top) / b.height)];
  };
  const down = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const t = e.target as HTMLElement;
    const corner = t.dataset.corner;
    if (corner !== undefined) {
      const r = on?.key === "drag" ? dragHere : edit && edit.key === on?.key ? edit.rect : byKey(on?.key ?? "")?.rect;
      if (!r) return;
      const c = Number(corner);
      // 반대쪽 모서리를 붙잡고 늘린다 (0 왼쪽 위 · 1 오른쪽 위 · 2 왼쪽 아래 · 3 오른쪽 아래)
      moving.current = { anchor: [c % 2 ? r[0] : r[0] + r[2], c < 2 ? r[1] + r[3] : r[1]], what: on!.key === "drag" ? "drag" : "edit" };
      if (on!.menu) setSel({ ...on!, menu: false });
    } else if (t === e.currentTarget) {
      const p = point(e);
      moving.current = { anchor: p, what: "drag" };
      setDrag({ page, rect: [p[0], p[1], 0, 0] });
      setSel({ key: "drag", page, menu: false });
      setEdit(null);
    } else return;
    e.currentTarget.setPointerCapture(e.pointerId);
    e.preventDefault();
  };
  const move = (e: React.PointerEvent) => {
    const mv = moving.current;
    if (!mv) return;
    const [x, y] = point(e);
    const r: Rect = [Math.min(x, mv.anchor[0]), Math.min(y, mv.anchor[1]), Math.abs(x - mv.anchor[0]), Math.abs(y - mv.anchor[1])];
    if (mv.what === "drag") setDrag({ page, rect: r });
    else if (on) setEdit({ key: on.key, rect: r });
  };
  const up = () => {
    const mv = moving.current;
    if (!mv) return;
    moving.current = null;
    if (mv.what === "drag") {
      if (!dragHere || dragHere[2] < 0.015 || dragHere[3] < 0.015) {
        // 빈 곳을 누르기만 했으면 고른 것을 푼다
        setDrag(null);
        setSel(null);
        return;
      }
      setDrag({ page, rect: round(dragHere) });
      setSel({ key: "drag", page, menu: true });
    } else if (edit) setEdit({ ...edit, rect: round(edit.rect) });
  };

  const handles = (r: Rect) =>
    [0, 1, 2, 3].map((c) => (
      <span key={c} className="pf-handle" data-corner={c} style={{ left: `${(r[0] + (c % 2 ? r[2] : 0)) * 100}%`, top: `${(r[1] + (c < 2 ? 0 : r[3])) * 100}%` }} aria-hidden />
    ));
  const menuAt = (r: Rect) => ({ left: `min(${(r[0] + r[2] * 0.45) * 100}%, calc(100% - 276px))`, top: `calc(${r[1] * 100}% + 16px)` });
  const barAt = (r: Rect) => ({ left: `min(${r[0] * 100}%, calc(100% - 360px))`, top: r[1] + r[3] > 0.86 ? `calc(${r[1] * 100}% - 58px)` : `calc(${(r[1] + r[3]) * 100}% + 10px)` });

  // 메뉴: 찾은 그림은 두 갈래 + 목록에서 빼기, 끌어서 잡은 영역은 표로도 + 영역 다시 잡기 (Figma 노트 170:14075)
  const figEntries = (p: number, r: Rect, name: string, title: string): MenuEntry[] => [
    { icon: "image", title: "그림 가리기 카드로", hint: "이 영역으로 가리기 편집기를 열어요", onClick: () => (setSel(null), src(p, r, name, title).then(flows.occlusion, fail)) },
    { icon: "layers", title: "그림을 앞면에 둔 카드", hint: "앞면에 그림, 뒷면에 답을 써요", onClick: () => (setSel(null), src(p, r, name, title).then(flows.front, fail)) },
  ];
  const onItem = on && on.key !== "drag" ? byKey(on.key) : undefined;
  const dragText = dragHere && lay ? gridIn(lay, boxOf(lay, dragHere)).length > 0 : false;
  let menu: ReactNode = null;
  if (on?.menu && on.key === "drag" && dragHere) {
    menu = (
      <FigMenu
        style={menuAt(dragHere)}
        className="pf-menu"
        onClose={() => setSel({ ...on, menu: false })}
        entries={[
          ...figEntries(page, dragHere, "", "잡은 영역"),
          {
            icon: "list",
            title: "표로 카드 만들기",
            hint: dragText ? "영역 안 글자로 칸을 나눠요" : scanned ? "스캔본이라 글자가 없어요 · 그림 카드만 돼요" : "영역 안에 글자가 없어 표로 만들 수 없어요",
            disabled: !dragText,
            onClick: () => (setSel(null), openTable(page, dragHere, "잡은 영역")),
          },
        ]}
        tail={{ label: "영역 다시 잡기", onClick: () => (setDrag(null), setSel(null)) }}
      />
    );
  } else if (on?.menu && onItem?.kind === "figure") {
    menu = (
      <FigMenu
        style={menuAt(onItem.rect)}
        className="pf-menu"
        onClose={() => setSel({ ...on, menu: false })}
        entries={figEntries(page, onItem.rect, onItem.name, label(onItem))}
        tail={onItem.auto && !onItem.seg ? { label: "목록에서 빼기 (그림이 아니에요)", onClick: () => (hide(onItem.key), setSel(null)) } : undefined}
      />
    );
  }
  const suggRect = onItem?.sugg ? (edit?.key === onItem.key ? edit.rect : onItem.rect) : null;

  const rows: FigRow[] = items.map((it) => ({
    key: it.key,
    kind: it.kind,
    where: `${it.page}쪽`,
    text: [label(it), it.size ?? ""].filter(Boolean).join(" · "),
    note: it.sugg ? "표일 수 있어요 · 확인 전" : undefined,
    seg: it.seg,
    act: it.sugg ? "확인" : "만들기",
  }));

  return (
    <>
      <div className="pf-viewer">
        <div className="fig-bar">
          {pill}
          <span className="src-preview__nav">
            <button className="src-icon-btn" aria-label="이전 쪽" disabled={page <= 1} onClick={() => onPage(page - 1)}>
              <Icon name="chevron-left" size={16} scaled />
            </button>
            <span className="t-label-m c-secondary">
              {n(page)}쪽 / {n(m.pages)}
            </span>
            <button className="src-icon-btn" aria-label="다음 쪽" disabled={page >= m.pages} onClick={() => onPage(page + 1)}>
              <Icon name="chevron-right" size={16} />
            </button>
          </span>
        </div>
        <div className="pf-page">
          <PdfPage materialId={m.id} page={page} width={PAGE_W} segs={[]} />
          <div className="pf-over" ref={over} onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={up}>
            {here.map((it) => {
              const r = it.sugg && edit?.key === it.key ? edit.rect : it.rect;
              const picked = on?.key === it.key;
              const count = it.seg?.links.length ?? 0;
              const name = labelOf(items, it);
              return (
                <span key={it.key} className="pf-item">
                  <button
                    id={`pf-${it.key}`}
                    className={`pf-box ${it.sugg ? "is-sugg" : "is-found"} ${picked ? "is-on" : ""}`}
                    style={pos(r)}
                    aria-label={it.sugg ? `표일 수 있어요 · ${it.size} 고르기` : `${name}${count ? ` · 카드 ${n(count)}장` : ""} ${it.kind === "table" ? "표로 카드 만들기" : "메뉴"}`}
                    aria-haspopup={it.kind === "figure" ? "menu" : undefined}
                    onClick={() => openItem(it)}
                  />
                  {it.sugg ? (
                    <span className="pf-tag is-sugg t-label-s" style={{ left: `calc(${(r[0] + r[2]) * 100}% - 4px)`, top: `${r[1] * 100}%` }}>
                      표일 수 있어요 · {it.size}
                    </span>
                  ) : (
                    <span className="pf-tag is-found t-label-s" style={{ left: `calc(${r[0] * 100}% + 4px)`, top: `calc(${r[1] * 100}% + 4px)` }}>
                      <Icon name="check" size={12} />
                      {name}
                      {count ? ` · 카드 ${n(count)}장` : ""}
                    </span>
                  )}
                  {picked && it.sugg && handles(r)}
                </span>
              );
            })}
            {dragHere && (
              <span className="pf-item">
                <button className={`pf-box is-drag ${on?.key === "drag" ? "is-on" : ""}`} style={pos(dragHere)} aria-label="끌어서 잡은 영역 메뉴" aria-haspopup="menu" onClick={() => setSel({ key: "drag", page, menu: true })} />
                {lay && dragHere[2] > 0 && (
                  <span className="pf-tag is-sugg t-label-s" style={{ left: `calc(${(dragHere[0] + dragHere[2]) * 100}% - 4px)`, top: `${dragHere[1] * 100}%` }}>
                    끌어서 잡음 · {cropSize(lay, dragHere).width}×{cropSize(lay, dragHere).height}
                  </span>
                )}
                {on?.key === "drag" && handles(dragHere)}
              </span>
            )}
            {menu}
            {suggRect && onItem && (
              <div className="pf-bar" style={barAt(suggRect)} onPointerDown={(e) => e.stopPropagation()}>
                <Button size="M" label="이 영역으로 표 만들기" autoFocus onClick={() => (setSel(null), setEdit(null), openTable(page, suggRect, label(onItem)))} />
                <Button variant="ghost" size="M" label="무시" onClick={() => (hide(onItem.key), setSel(null), setEdit(null))} />
                <span className="t-caption c-tertiary">모서리를 끌어 고쳐요</span>
              </div>
            )}
          </div>
        </div>
        <div className="pf-legend">
          <span className="t-caption c-secondary">
            <span className="pf-swatch is-found" />
            찾은 그림 · 확정한 표
          </span>
          <span className="t-caption c-secondary">
            <span className="pf-swatch is-sugg" />
            표일 수 있어요 · 끌어서 잡은 영역
          </span>
          <span className="t-caption c-tertiary">빈 곳을 끌면 언제든 직접 잡아요</span>
        </div>
        {scanned && <p className="t-caption c-warn">스캔본이라 글자를 읽을 수 없어요. 끌어서 잡은 영역으로 그림 카드만 만들 수 있어요 (표 제안 · 표 만들기는 없어요).</p>}
      </div>

      <FigList
        rows={rows}
        head={head}
        notes={
          items.length === 0 && (
            <p className="src-list__note t-body-s c-secondary">
              {scanned
                ? "스캔본이라 그림과 표를 자동으로 찾지 않아요. 쪽 보기에서 끌어서 영역을 잡아 주세요."
                : scanning
                  ? "쪽을 살피고 있어요…"
                  : "이 자료에서 그림이나 표를 찾지 못했어요. 쪽 보기에서 끌어서 영역을 잡을 수 있어요."}
            </p>
          )
        }
        linking={linking}
        onLinking={onLinking}
        links={links}
        onOpen={(key) => byKey(key) && openItem(byKey(key)!)}
        onPick={(key) => {
          const it = byKey(key);
          if (!it) return;
          onPage(it.page);
          setSel({ key, page: it.page, menu: false });
          requestAnimationFrame(() => document.getElementById(`pf-${key}`)?.scrollIntoView({ block: "center", behavior: "smooth" }));
        }}
        bulk={bulk}
        picked={on?.key}
      />
      {flows.ui}
    </>
  );
}
