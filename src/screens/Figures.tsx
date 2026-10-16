// 8.4 그림 · 표로 카드 만들기 (Figma Row IT: IT1 157:13787 · IT1-p 176:14681 · IT3 168:13918, 노트 170:14075 · 178:14484 · 170:14084, DECISIONS IT).
// PPTX 는 앱이 슬라이드를 그림으로 그리지 않아 끌어서 잡기 없이, 그림 · 표가 있는 슬라이드 목록에서 고른다 (IT1-p). PDF 는 PdfFigures.tsx.
// 그림 → 그림 가리기(M2 편집기) · 그림을 앞면에 둔 카드, 표 → 표로 카드 만들기(IT3). EMF · WMF 는 만들지 않는다 (IT11).
// 만든 카드는 그 쪽 · 영역 구간에 이어진다 (IT9). 흐름(useFlows) · 메뉴 · 오른쪽 목록 · 차례로 만들기(IT8)는 PDF 도 같이 쓴다
import "../styles/figures.css";
import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { api, errorText, qualityApi } from "../api";
import { sanitize } from "../cardHtml";
import { Badge, Button, Checkbox, Icon, Input, Segment as SegmentControl, type IconName } from "../ui";
import { n, useKey, useOverlayScope } from "../nav";
import { useToast } from "../toast";
import { Modal } from "./NewDeck";
import { DeckPicker } from "./DeckPicker";
import { menuKeys } from "./Deck";
import { OcclusionEditor } from "./Occlusion";
import { cropPage, pageLayout } from "../pdf";
import { boxOf, gridIn } from "../figures";
import { naturalSize, occlusionApi, ordinalsOf, type Picture, type Shape } from "../occlusion";
import {
  sourceApi,
  type Cell,
  type Material,
  type Rect,
  type Region,
  type RegionCard,
  type Segment,
  type SlideImage,
  type SlideObjects,
  type SlideTable,
  type TableMethod,
} from "../sourceApi";

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
export const sameRect = (a: Rect | undefined, b: Rect) => !!a && a.every((v, i) => Math.abs(v - b[i]) < 1e-6);
/** 이 영역의 구간 (목록 · 타일의 "카드 N장", 연결 끊기) */
const regionSeg = (segs: Segment[], page: number, kind: "figure" | "table", rect: Rect) =>
  segs.find((s) => s.page === page && s.signals.includes(kind) && sameRect(s.rects?.[0], rect));

const width = (rows: Cell[][]) => Math.max(0, ...rows.map((r) => r.length));
const merges = (rows: Cell[][]) => rows.flat().filter((c) => !c.merged && (c.row_span > 1 || c.col_span > 1)).length;
/** "4행 × 3열 · 합친 칸 1" */
export const sizeLabel = (rows: Cell[][]) => `${rows.length}행 × ${width(rows)}열${merges(rows) ? ` · 합친 칸 ${merges(rows)}` : ""}`;
/** PDF 칸 나누기(글자만) → 표 창의 칸 */
export const cellsOf = (g: string[][]): Cell[][] => g.map((r) => r.map((text) => ({ text, row_span: 1, col_span: 1, merged: false })));
/** 구간 글자 (I2 패널 · 검색): 행마다 한 줄, 칸은 " · " */
const tableText = (rows: Cell[][]) =>
  rows
    .map((r) => r.filter((c) => !c.merged && c.text.trim()).map((c) => c.text.replace(/\s+/g, " ").trim()).join(" · "))
    .filter(Boolean)
    .join("\n");
const imageLabel = (img: SlideImage) => img.descr.trim() || img.name.trim() || `그림 ${img.index + 1}`;

/** data URL 그림(PDF 에서 자른 것)을 미디어 폴더에. 파일 이름은 save_media 가 내용으로 정한다 */
export const storeDataUrl = (url: string, name = "figure.png") => invoke<string>("save_media", { name, dataBase64: url.split(",")[1] ?? "" });

// ---------------------------------------------------------------- 썸네일 (원본 바이트 → Blob URL)

// 자료마다 한 번만 받는다. 카드를 만들 때 같은 Blob 을 미디어 폴더에 넣는다 (CSP 가 blob: fetch 를 막아 Blob 을 들고 있는다)
const thumbs = new Map<string, Promise<{ url: string; blob: Blob }>>();
function thumb(materialId: number, page: number, img: SlideImage) {
  const key = `${materialId}:${page}:${img.index}`;
  let p = thumbs.get(key);
  if (!p) {
    p = sourceApi.slideImage(materialId, page, img.index).then((b) => {
      const blob = new Blob([b], { type: `image/${img.ext === "jpg" ? "jpeg" : img.ext}` });
      return { url: URL.createObjectURL(blob), blob };
    });
    p.catch(() => thumbs.delete(key));
    thumbs.set(key, p);
  }
  return p;
}
function dropThumbs() {
  thumbs.forEach((p) => p.then((t) => URL.revokeObjectURL(t.url), () => {}));
  thumbs.clear();
}
function useThumb(materialId: number, page: number, img: SlideImage) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    if (!img.supported) return;
    let live = true;
    thumb(materialId, page, img).then((t) => live && setUrl(t.url), () => {});
    return () => {
      live = false;
    };
  }, [materialId, page, img]);
  return url;
}

/** 카드를 만들 때만 미디어 폴더에 넣는다. 파일 이름은 save_media 가 내용으로 정한다 (자료 안 이름은 믿지 않는다) */
async function storeImage(materialId: number, page: number, img: SlideImage) {
  const { blob } = await thumb(materialId, page, img);
  const data = await new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(blob);
  });
  return storeDataUrl(data, `slide.${img.ext}`);
}

// ---------------------------------------------------------------- 흐름 (PPTX · PDF 공통)

/** 그림 카드가 받는 것. url: 보여 줄 그림 · store: 미디어 폴더에 넣고 이름 (카드를 만들 때만) */
export type FigSource = {
  region: Region;
  /** "그림 1 · 강의.pptx · 슬라이드 3" */
  sub: string;
  /** 가리기 편집기 머리 */
  fileName: string;
  alt: string;
  /** 넣을 곳 기본값 (R8) */
  deckId: number;
  url: () => Promise<string>;
  store: () => Promise<string>;
};
export type TableFlow = { sub: string; how?: string; rows: Cell[][]; region: Omit<Region, "kind" | "text">; deckId: number };

/** 그림 가리기 · 그림을 앞면에 둔 카드 · 표 창. onMade: 카드를 만들었을 때 (차례로 만들기가 다음 것을 연다, 닫기만 하면 부르지 않는다).
 * step: 차례로 만드는 중이면 창 머리 둘째 줄 ("차례로 만들기 1 / 3", 창이 아래 막대를 가린다) */
export function useFlows(rootDeckId: number, onChanged: () => void, onMade: () => void, step?: string) {
  const toast = useToast();
  const [occl, setOccl] = useState<{ src: FigSource; pic: Picture } | null>(null);
  const [front, setFront] = useState<FigSource | null>(null);
  const [table, setTable] = useState<TableFlow | null>(null);
  const occlusion = async (src: FigSource) => {
    try {
      const url = await src.url();
      setOccl({ src, pic: { name: "", url, fileName: src.fileName, ...(await naturalSize(url)) } });
    } catch (e) {
      toast({ title: "그림을 열지 못했어요", text: errorText(e), tone: "error" });
    }
  };
  const saveOcclusion = async (src: FigSource, shapes: Shape[], hideAll: boolean) => {
    try {
      const image = await src.store();
      const note = await occlusionApi.save({ note_id: null, deck_id: src.deckId, image, header: "", back_extra: "", shapes, hide_all: hideAll, importance: null });
      await sourceApi.regionLink(src.region, [note]);
      toast({ title: `그림 가리기 카드 ${n(ordinalsOf(shapes).length)}장을 만들었어요`, text: "자료의 그림과 이어 두었어요." });
      setOccl(null);
      onChanged();
      onMade();
    } catch (e) {
      toast({ title: "카드를 만들지 못했어요", text: errorText(e), tone: "error" });
    }
  };
  const sub = (s: string) => (step ? `${s}\n${step}` : s);
  const ui = (
    <>
      {occl && (
        <OcclusionEditor
          picture={step ? { ...occl.pic, fileName: `${occl.pic.fileName} · ${step}` } : occl.pic}
          shapes={[]}
          hideAll
          direct
          onCancel={() => setOccl(null)}
          onDone={(shapes, hideAll) => saveOcclusion(occl.src, shapes, hideAll)}
        />
      )}
      {front && <ImageCard src={front} step={step} rootDeckId={rootDeckId} onClose={() => setFront(null)} onDone={() => (onChanged(), onMade())} onUndone={onChanged} />}
      {table && (
        <TableCards sub={sub(table.sub)} how={table.how} rows={table.rows} region={table.region} rootDeckId={rootDeckId} deckId={table.deckId} onClose={() => setTable(null)} onDone={onChanged} onMade={onMade} />
      )}
    </>
  );
  return { occlusion, front: setFront, table: setTable, ui };
}

// ---------------------------------------------------------------- 그림 메뉴 (IT2)

export type MenuEntry = { icon: IconName; title: string; hint: string; disabled?: boolean; onClick: () => void };

/** 그림 · 잡은 영역 메뉴. tail: 아래 줄 (목록에서 빼기 · 영역 다시 잡기) */
export function FigMenu({ entries, tail, className = "", style, onClose }: {
  entries: MenuEntry[];
  tail?: { label: string; onClick: () => void };
  className?: string;
  style?: React.CSSProperties;
  onClose: () => void;
}) {
  return (
    <div
      className={`menu__list menu--panel fig-menu ${className}`}
      style={style}
      role="menu"
      aria-label="그림 메뉴"
      onKeyDown={menuKeys}
      onPointerDown={(e) => e.stopPropagation()}
      onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && onClose()}
    >
      {entries.map((x, i) => (
        <button key={x.title} role="menuitem" autoFocus={i === 0} className="menu__item menu__item--top" disabled={x.disabled} onClick={x.onClick}>
          <Icon name={x.icon} size={18} />
          <span className="menu__text">
            <span className="t-label-m">{x.title}</span>
            <span className="t-caption c-tertiary">{x.hint}</span>
          </span>
        </button>
      ))}
      {tail && (
        <>
          <div className="menu__sep" role="separator" />
          <button role="menuitem" className="menu__item" onClick={tail.onClick}>
            <Icon name="x" size={18} />
            <span className="t-label-m c-secondary">{tail.label}</span>
          </button>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- 오른쪽 목록 · 차례로 만들기 (IT4 · IT8)

export type FigRow = {
  key: string;
  kind: "figure" | "table";
  /** "슬라이드 7" · "p.12" */
  where: string;
  text: string;
  /** "표일 수 있어요 · 확인 전" (warn) */
  note?: string;
  seg?: Segment;
  /** 만들 수 없음 (IT11) */
  off?: boolean;
  /** 버튼 글: "만들기" · "확인" */
  act?: string;
};

/** 고르기 · 차례로 만들기. open: 그 줄의 흐름을 연다 (표는 IT3 창, 그림은 메뉴). 창이 닫히면 next() */
export function useBulk(open: (key: string) => void) {
  const [checked, setChecked] = useState<string[]>([]);
  const [run, setRun] = useState<{ keys: string[]; at: number } | null>(null);
  const go = (keys: string[], at: number) => {
    if (at >= keys.length) return setRun(null);
    setRun({ keys, at });
    open(keys[at]);
  };
  return {
    checked,
    toggle: (k: string, on: boolean) => setChecked((c) => (on ? [...c.filter((x) => x !== k), k] : c.filter((x) => x !== k))),
    run,
    /** 목록 순서대로 */
    start: (order: string[]) => {
      setChecked([]);
      go(order.filter((k) => checked.includes(k)), 0);
    },
    next: () => run && go(run.keys, run.at + 1),
    stop: () => setRun(null),
  };
}
export type Bulk = ReturnType<typeof useBulk>;

export function FigList({ rows, head, notes, linking, onLinking, links, onOpen, onPick, menu, bulk, picked }: {
  rows: FigRow[];
  head: ReactNode;
  /** 목록 위 안내 (읽는 중 · 오류 · 비었음) */
  notes?: ReactNode;
  linking?: number;
  onLinking: (segmentId: number | undefined) => void;
  links: (seg: Segment) => ReactNode;
  onOpen: (key: string) => void;
  /** 줄을 누르면 (PDF: 그 쪽으로) */
  onPick?: (key: string) => void;
  /** 그 줄의 메뉴 (PPTX 그림) */
  menu?: (key: string) => ReactNode;
  bulk: Bulk;
  /** 쪽 보기에서 고른 것 */
  picked?: string;
}) {
  const usable = rows.filter((r) => !r.off);
  const sel = rows.filter((r) => bulk.checked.includes(r.key));
  const tables = sel.filter((r) => r.kind === "table").length;
  const label = (r?: FigRow) => (r ? `${r.where} · ${r.text}` : "");
  const run = bulk.run;
  const current = picked ?? run?.keys[run.at];
  return (
    <section className="panel src-list" aria-label="그림과 표">
      {head}
      {notes}
      {rows.map((r) => {
        const count = r.seg?.links.length ?? 0;
        const on = bulk.checked.includes(r.key);
        return (
          <div key={r.key} id={`fig-${r.key}`} className={`fig-item ${on ? "is-picked" : ""} ${current === r.key ? "is-active" : ""}`}>
            <div className="fig-row" onClick={onPick && (() => onPick(r.key))}>
              <span className="src-seg__check">
                <Checkbox quiet label={`${r.text} 고르기`} checked={on} disabled={r.off} onChange={(v) => bulk.toggle(r.key, v)} />
              </span>
              <span className="fig-row__thumb">
                <Icon name={r.kind === "figure" ? "image" : "list"} size={18} />
              </span>
              <span className="fig-row__text">
                <span className="fig-row__top">
                  <Badge>{r.kind === "figure" ? "그림" : "표"}</Badge>
                  <span className="t-caption c-tertiary">{r.where}</span>
                  {r.note && <span className="t-caption c-warn">{r.note}</span>}
                </span>
                <span className={`t-body-s ${r.off ? "c-tertiary" : "c-primary"}`}>{r.text}</span>
              </span>
              {r.off ? (
                <span className="t-caption c-tertiary">만들 수 없어요</span>
              ) : r.seg && count ? (
                <button
                  className="badge badge--accent t-label-s fig-row__count"
                  aria-expanded={linking === r.seg.id}
                  onClick={(e) => {
                    e.stopPropagation();
                    onLinking(linking === r.seg!.id ? undefined : r.seg!.id);
                  }}
                >
                  카드 {n(count)}장
                </button>
              ) : (
                <Button
                  variant="ghost"
                  size="M"
                  icon="plus"
                  label={r.act ?? "만들기"}
                  onClick={(e) => {
                    e.stopPropagation();
                    onOpen(r.key);
                  }}
                />
              )}
              {menu?.(r.key)}
            </div>
            {r.seg && linking === r.seg.id && links(r.seg)}
          </div>
        );
      })}
      {usable.length > 0 &&
        (run ? (
          <div className="src-pick" role="status">
            <span className="src-pick__text">
              <span className="t-label-m">
                차례로 만들기 {n(run.at + 1)} / {n(run.keys.length)} · {label(rows.find((r) => r.key === run.keys[run.at]))}
              </span>
              <span className="t-caption c-tertiary">{run.at + 1 < run.keys.length ? "카드를 만들면 다음 것을 열어요 · 건너뛰려면 [다음]" : "마지막이에요 · 카드를 만들면 끝나요"}</span>
            </span>
            <span className="fig-run__btns">
              <Button variant="ghost" size="M" label="그만하기" onClick={bulk.stop} />
              <Button variant="secondary" size="M" icon="arrow-right" label={run.at + 1 < run.keys.length ? "다음" : "끝내기"} onClick={bulk.next} />
            </span>
          </div>
        ) : (
          <div className="src-pick">
            <span className="src-pick__text">
              <span className="t-label-m">{sel.length ? `${n(sel.length)}개 고름 · 표 ${n(tables)} · 그림 ${n(sel.length - tables)}` : "차례로 만들 그림·표를 골라 주세요"}</span>
              <span className="t-caption c-tertiary">하나씩 차례로 열어요 · 이미 만든 것은 다시 고를 수 있어요</span>
            </span>
            <Button size="M" icon="arrow-right" label="차례로 만들기" disabled={!sel.length} onClick={() => bulk.start(rows.map((r) => r.key))} />
          </div>
        ))}
    </section>
  );
}

// ---------------------------------------------------------------- IT1-p 슬라이드별 목록

type Item = { o: SlideObjects; key: string; k: number; img?: SlideImage; table?: SlideTable };
const itemsOf = (o: SlideObjects): Item[] => [
  ...o.images.map((img, k) => ({ o, key: `${o.page}:i${k}`, k, img })),
  ...o.tables.map((table, k) => ({ o, key: `${o.page}:t${k}`, k, table })),
];
const usable = (it: Item) => !it.img || it.img.supported;

/** 가운데 열(슬라이드 카드)과 오른쪽 목록. head: 목록 위 탭 줄, pill: 파일 알약, links: 카드 잇기 칸 (Sources 가 준다) */
export function Figures({ m, objs, error, segs, deckId, head, pill, linking, onLinking, links, onChanged }: {
  m: Material;
  objs: SlideObjects[] | null;
  error?: string;
  segs: Segment[];
  /** 자료 화면의 덱 (넣을 곳 고르기의 뿌리) */
  deckId: number;
  head: ReactNode;
  pill: ReactNode;
  /** 카드 잇기 칸을 연 구간 ("카드 N장"을 누르면 이어진 카드 · 끊기) */
  linking?: number;
  onLinking: (segmentId: number | undefined) => void;
  links: (seg: Segment) => ReactNode;
  onChanged: () => void;
}) {
  const [menu, setMenu] = useState<string | null>(null);
  useOverlayScope(menu !== null);
  useKey("Escape", () => setMenu(null), menu !== null, { modal: true });
  useEffect(() => dropThumbs, [m.id]);

  const items = (objs ?? []).flatMap(itemsOf);
  const byKey = (key: string) => items.find((it) => it.key === key);
  const segOf = (it: Item) => regionSeg(segs, it.o.page, it.img ? "figure" : "table", (it.img ?? it.table)!.rect);
  const linked = (it: Item) => segOf(it)?.links.length ?? 0;
  const region = (it: Item): Omit<Region, "kind" | "text"> => ({ material_id: m.id, page: it.o.page, rects: [(it.img ?? it.table)!.rect] });
  const place = (it: Item) => `${m.file_name} · 슬라이드 ${it.o.page}`;
  const bulk = useBulk((key) => {
    const it = byKey(key);
    if (!it) return bulk.next();
    if (it.table) return openTable(it);
    setMenu(`row:${key}`);
    document.getElementById(`fig-${key}`)?.scrollIntoView({ block: "nearest" });
  });
  const flows = useFlows(deckId, onChanged, () => bulk.run && bulk.next(), bulk.run ? `차례로 만들기 ${n(bulk.run.at + 1)} / ${n(bulk.run.keys.length)}` : undefined);
  const openTable = (it: Item) => flows.table({ sub: `표 ${it.k + 1} · ${place(it)}`, rows: it.table!.rows, region: region(it), deckId: it.o.deck_id });
  const open = (it: Item, where: string) => (it.table ? openTable(it) : setMenu(`${where}:${it.key}`));
  const src = (it: Item): FigSource => ({
    region: { ...region(it), kind: "figure", text: it.img!.descr.trim() },
    sub: `그림 ${it.k + 1} · ${place(it)}`,
    fileName: place(it),
    alt: imageLabel(it.img!),
    deckId: it.o.deck_id,
    url: () => thumb(m.id, it.o.page, it.img!).then((t) => t.url),
    store: () => storeImage(m.id, it.o.page, it.img!),
  });

  const imageMenu = (it: Item, where: string) =>
    menu === `${where}:${it.key}` && (
      <FigMenu
        onClose={() => setMenu(null)}
        entries={[
          {
            icon: "image",
            title: "그림 가리기 카드로",
            hint: it.img?.ext === "gif" ? "움직이는 GIF는 가릴 수 없어요" : "이 그림으로 가리기 편집기를 열어요",
            disabled: it.img?.ext === "gif",
            onClick: () => (setMenu(null), flows.occlusion(src(it))),
          },
          { icon: "layers", title: "그림을 앞면에 둔 카드", hint: "앞면에 그림, 뒷면에 답을 써요", onClick: () => (setMenu(null), flows.front(src(it))) },
        ]}
      />
    );

  const rows: FigRow[] = items.map((it) => ({
    key: it.key,
    kind: it.img ? "figure" : "table",
    where: `슬라이드 ${it.o.page}`,
    text: it.img
      ? `${imageLabel(it.img)}${it.img.supported ? "" : " · 미리 볼 수 없는 형식"}`
      : `${it.table!.name || `표 ${it.k + 1}`} · ${it.table!.rows.length}행 × ${width(it.table!.rows)}열`,
    seg: segOf(it),
    off: !usable(it),
  }));

  return (
    <>
      <div className="fig-slides" aria-label="그림·표가 있는 슬라이드">
        <div className="fig-bar">
          {pill}
          <span className="t-label-m c-secondary">
            그림·표가 있는 슬라이드 {n(objs?.length ?? 0)} / {n(m.pages)}
          </span>
        </div>
        {objs?.map((o) => (
          <div key={o.page} className="fig-slide">
            <div className="fig-slide__head">
              <span className="t-label-m">
                슬라이드 {o.page}
                {o.title ? ` · ${o.title}` : ""}
              </span>
              <span className="t-caption c-tertiary">
                {[o.images.length && `그림 ${n(o.images.length)}`, o.tables.length && `표 ${n(o.tables.length)}`].filter(Boolean).join(" · ")}
              </span>
            </div>
            <div className="fig-tiles">
              {itemsOf(o).map((it) => (
                <Tile
                  key={it.key}
                  m={m}
                  it={it}
                  linked={linked(it)}
                  checked={bulk.checked.includes(it.key)}
                  onCheck={(v) => bulk.toggle(it.key, v)}
                  onOpen={() => open(it, "tile")}
                  menu={imageMenu(it, "tile")}
                />
              ))}
            </div>
          </div>
        ))}
        <p className="t-caption c-tertiary">그림·표가 없는 슬라이드는 빼고 보여요 · 그림을 누르면 메뉴, 표를 누르면 표 카드 창이 열려요</p>
      </div>

      <FigList
        rows={rows}
        head={head}
        notes={
          <>
            {objs === null && !error && <p className="src-list__note t-body-s c-secondary">슬라이드를 읽고 있어요…</p>}
            {error && <p className="src-list__note t-body-s c-secondary">{error}</p>}
            {objs?.length === 0 && !error && <p className="src-list__note t-body-s c-secondary">이 자료에서 그림이나 표를 찾지 못했어요.</p>}
          </>
        }
        linking={linking}
        onLinking={onLinking}
        links={links}
        onOpen={(key) => byKey(key) && open(byKey(key)!, "row")}
        menu={(key) => byKey(key) && imageMenu(byKey(key)!, "row")}
        bulk={bulk}
      />
      {flows.ui}
    </>
  );
}

function Tile({ m, it, linked, checked, onCheck, onOpen, menu }: { m: Material; it: Item; linked: number; checked: boolean; onCheck: (v: boolean) => void; onOpen: () => void; menu: ReactNode }) {
  const ok = usable(it);
  const label = it.img
    ? `그림 ${it.k + 1} · ${imageLabel(it.img)}${it.img.supported ? "" : ` · ${it.img.ext.toUpperCase()}`}`
    : `표 ${it.k + 1} · ${sizeLabel(it.table!.rows)}`;
  return (
    <div className={`fig-tile ${checked ? "is-checked" : ""}`}>
      <button className={`fig-thumb ${ok ? "" : "is-off"}`} disabled={!ok} aria-label={`${label} ${it.table ? "표로 카드 만들기" : "카드 만들기"}`} aria-haspopup={it.img ? "menu" : "dialog"} onClick={onOpen}>
        {it.img ? (
          ok ? (
            <ThumbImage m={m} page={it.o.page} img={it.img} />
          ) : (
            <>
              <Icon name="image" size={18} />
              <span className="t-caption c-tertiary">미리 볼 수 없는 형식</span>
            </>
          )
        ) : (
          <MiniGrid rows={it.table!.rows} />
        )}
        {linked > 0 && <span className="badge badge--accent t-label-s fig-thumb__count">카드 {n(linked)}장</span>}
      </button>
      {ok && (
        <button className="fig-tile__check" role="checkbox" aria-checked={checked} aria-label={`${label} 고르기`} onClick={() => onCheck(!checked)}>
          {checked && <Icon name="check" size={12} />}
        </button>
      )}
      <span className={`t-caption ${ok ? "c-secondary" : "c-tertiary"}`} title={label}>
        {label}
      </span>
      {menu}
    </div>
  );
}

function ThumbImage({ m, page, img }: { m: Material; page: number; img: SlideImage }) {
  const url = useThumb(m.id, page, img);
  return url ? <img src={url} alt={imageLabel(img)} draggable={false} /> : <Icon name="image" size={18} className="c-tertiary" />;
}

/** 표 모양만 (글자 없이). 머리글 줄은 칠한다 */
function MiniGrid({ rows }: { rows: Cell[][] }) {
  const shown = rows.slice(0, 8);
  return (
    <table className="fig-mini" aria-hidden>
      <tbody>
        {shown.map((r, i) => (
          <tr key={i}>{r.map((c, j) => (c.merged ? null : <td key={j} rowSpan={Math.min(c.row_span, shown.length - i)} colSpan={c.col_span} />))}</tr>
        ))}
      </tbody>
    </table>
  );
}

// ---------------------------------------------------------------- I2 패널의 그림 · 표 구간

const flat = (t: string) => t.replace(/\s+/g, " ").trim();
/** 이 카드가 묻는 칸 ("r,c"): 빈칸 카드는 답 칸, 행 카드는 질문이 시작하는 행 이름의 행, 목록 카드는 그 열 */
function hitsOf(rows: Cell[][], cardHtml: string) {
  const tpl = document.createElement("template");
  tpl.innerHTML = cardHtml;
  const answers = [...tpl.content.querySelectorAll(".cloze")].map((e) => flat(e.textContent ?? "")).filter(Boolean);
  const text = flat(tpl.content.textContent ?? "");
  const hits = new Set<string>();
  rows.forEach((row, r) => row.forEach((c, j) => !c.merged && answers.includes(flat(c.text)) && hits.add(`${r},${j}`)));
  if (hits.size || !text) return hits;
  const longest = (list: [string, string[]][]) => list.filter(([name]) => name && text.startsWith(name)).sort((a, b) => b[0].length - a[0].length)[0]?.[1] ?? [];
  const byRow = longest(rows.slice(1).map((row, i): [string, string[]] => [flat(row[0]?.text ?? ""), row.map((_, j) => `${i + 1},${j}`)]));
  const byCol = byRow.length ? [] : longest((rows[0] ?? []).map((c, j): [string, string[]] => [flat(c.text), rows.map((_, r) => `${r},${j}`)]));
  [...byRow, ...byCol].forEach((k) => hits.add(k));
  return hits;
}

/** 표는 카드와 같은 HTML 표로 (첫 행 머리글), 그림은 썸네일. PDF 는 그 영역을 다시 나누거나(표) 잘라 그린다(그림 · 글자 없는 표) */
export function RegionView({ m, s, cardId }: { m: Material; s: Segment; cardId?: number }) {
  const [obj, setObj] = useState<{ img?: SlideImage; table?: SlideTable; url?: string } | null>();
  const [card, setCard] = useState("");
  useEffect(() => {
    let live = true;
    setObj(undefined);
    const r = s.rects?.[0];
    if (m.kind === "pdf" && r) {
      (async () => {
        if (s.signals.includes("table")) {
          const lay = await pageLayout(m.id, s.page);
          const g = gridIn(lay, boxOf(lay, r));
          if (g.length) return live && setObj({ table: { name: "", rect: r, rows: cellsOf(g) } });
        }
        const c = await cropPage(m.id, s.page, r, 600);
        if (live) setObj({ url: c.url });
      })().catch(() => live && setObj(null));
    } else if (m.kind === "pptx") {
      sourceApi.slideObjects(m.id, s.page).then(
        ([o]) => {
          const img = o?.images.find((x) => sameRect(r, x.rect));
          const table = o?.tables.find((x) => sameRect(r, x.rect));
          if (live) setObj(img || table ? { img, table } : null);
        },
        () => live && setObj(null),
      );
    } else setObj(null);
    if (cardId != null) api.cardView(cardId).then((v) => live && setCard(v.back), () => {});
    return () => {
      live = false;
    };
  }, [m.id, m.kind, s.page, s.rects, s.signals, cardId]);
  if (obj === undefined) return <div className="src-panel__region is-wait" />;
  if (obj?.img || obj?.url) {
    return (
      <figure className="src-panel__region">
        {obj.url ? (
          <img src={obj.url} alt={s.text} />
        ) : obj.img!.supported ? (
          <ThumbImage m={m} page={s.page} img={obj.img!} />
        ) : (
          <span className="t-caption c-tertiary">미리 볼 수 없는 형식</span>
        )}
        <figcaption className="t-caption c-tertiary">{s.text}</figcaption>
      </figure>
    );
  }
  if (obj?.table) {
    const hits = hitsOf(obj.table.rows, card);
    return (
      <div className="card-html src-panel__table">
        <table>
          <tbody>
            {obj.table.rows.map((row, r) => (
              <tr key={r}>
                {row.map((c, j) => {
                  if (c.merged) return null;
                  const Tag = r === 0 ? "th" : "td";
                  return (
                    <Tag key={j} rowSpan={c.row_span > 1 ? c.row_span : undefined} colSpan={c.col_span > 1 ? c.col_span : undefined} className={hits.has(`${r},${j}`) ? "is-hit" : undefined}>
                      {c.text}
                    </Tag>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }
  return <p className="src-panel__hit t-body-s c-primary">{s.text}</p>;
}

// ---------------------------------------------------------------- IT3 그림을 앞면에 둔 카드

function ImageCard({ src, step, rootDeckId, onClose, onDone, onUndone }: {
  src: FigSource;
  step?: string;
  rootDeckId: number;
  onClose: () => void;
  onDone: () => void;
  onUndone: () => void;
}) {
  const toast = useToast();
  const [url, setUrl] = useState<string>();
  const [question, setQuestion] = useState("");
  const [answer, setAnswer] = useState("");
  const [deck, setDeck] = useState(src.deckId);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    let live = true;
    src.url().then((u) => live && setUrl(u), (e) => live && setError(errorText(e)));
    return () => {
      live = false;
    };
  }, [src]);
  const create = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const name = await src.store();
      const front = `${question.trim() ? `<div>${esc(question.trim())}</div>` : ""}<img src="${esc(name)}">`;
      const change = await sourceApi.regionCards(deck, src.region, [{ kind: "basic", front, back: esc(answer.trim()).replace(/\n/g, "<br>") }]);
      toast({
        title: "카드 1장을 만들었어요",
        text: "자료의 그림과 이어 두었어요.",
        tone: "undo",
        action: { label: "되돌리기", onClick: () => qualityApi.undo(change).then(() => (toast({ title: "카드 만들기를 되돌렸어요" }), onUndone()), (e) => toast({ title: "되돌리지 못했어요", text: errorText(e), tone: "error" })) },
      });
      onDone();
      onClose();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal title="그림을 앞면에 둔 카드" sub={step ? `${src.sub}\n${step}` : src.sub} onClose={onClose} className="fig-front">
      <div className="modal__body">
        {url ? <img className="fig-front__img" src={url} alt={src.alt} /> : <span className="fig-front__img fig-front__img--wait" />}
        <Input label="질문 (앞면, 그림 위)" placeholder="비워 두면 그림만 보여요" value={question} onChange={(e) => setQuestion(e.target.value)} />
        <label className="field">
          <span className="t-label-m">답 (뒷면)</span>
          <textarea className="field__input t-body-m fig-front__answer" value={answer} autoFocus onChange={(e) => setAnswer(e.target.value)} />
        </label>
        {error && <p className="t-body-s c-danger">{error}</p>}
        <div className="fig-foot">
          <DeckPicker rootDeckId={rootDeckId} value={deck} onChange={(d) => setDeck(d.id)} relative prefix="넣을 곳" />
          <span className="modal__actions">
            <Button variant="ghost" size="M" label="취소" onClick={onClose} />
            <Button size="M" label="카드 만들기" disabled={!answer.trim()} busy={busy} busyLabel="만드는 중…" onClick={create} />
          </span>
        </div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- IT3 표로 카드 만들기 (PDF 2단계도 같이 쓴다)

const MODES: { value: TableMethod; label: string; hint: string }[] = [
  { value: "cloze", label: "칸 빈칸", hint: "고른 열의 칸을 하나씩 가려 물어요 · 칸 하나 = 노트 한 개" },
  { value: "rows", label: "행마다 한 장", hint: "앞면은 행 이름과 고른 열 이름, 뒷면은 그 칸들" },
  { value: "list", label: "열을 목록으로", hint: "고른 열 하나 = 목록 회상 카드 한 장 · 위에서 아래 순서대로" },
];
/** 미리보기 아래 한 줄: 방식마다 질문 모양 */
const QUESTION_HINT: Record<TableMethod, string> = {
  cloze: '질문은 "행 이름의 열 이름은?"으로 만들어요.',
  rows: '앞면은 "행 이름의 열 이름은?", 열을 여럿 고르면 "행 이름 — 열1? 열2?"로 만들어요.',
  list: '질문은 "열 이름 N가지를 표 순서대로"로 만들어요.',
};
const blank = (): Cell => ({ text: "", row_span: 1, col_span: 1, merged: false });
/** 행 · 열을 넣거나 빼면 합친 칸은 푼다 (글자는 왼쪽 위 칸에 남는다) */
const unmerged = (g: Cell[][]) => g.map((row) => row.map((x) => ({ text: x.merged ? "" : x.text, row_span: 1, col_span: 1, merged: false })));

/** 묻는 칸: 실제 카드 앞면과 같게 (render.rs cloze, 표 칸에는 힌트가 없다) */
const ASK = `<span class="cloze">[${"\u00a0".repeat(6)}]</span>`;
/** {{c1::답}} → 묻는 칸 표시 (미리보기만) */
function DraftView({ d }: { d: RegionCard }) {
  if (d.kind === "cloze") {
    const answer = /\{\{c1::([\s\S]*?)\}\}/.exec(d.front)?.[1] ?? "";
    return (
      <>
        <div className="card-html tg-card__front" dangerouslySetInnerHTML={{ __html: sanitize(d.front.replace(/\{\{c1::[\s\S]*?\}\}/, ASK)) }} />
        <p className="t-caption c-secondary" dangerouslySetInnerHTML={{ __html: sanitize(`답 · ${answer}`) }} />
      </>
    );
  }
  return (
    <>
      <div className="card-html tg-card__q" dangerouslySetInnerHTML={{ __html: sanitize(d.front) }} />
      <div className="card-html t-body-s c-secondary" dangerouslySetInnerHTML={{ __html: sanitize(d.back) }} />
    </>
  );
}

export function TableCards({ title = "표로 카드 만들기", sub, how = "표 구조 그대로 가져왔어요 · 칸을 눌러 고쳐요", rows, region, rootDeckId, deckId, onClose, onDone, onMade }: {
  title?: string;
  /** "표 1 · 강의.pptx · 슬라이드 7" */
  sub: string;
  /** 표 위 한 줄 (어떻게 칸을 나눴는지) */
  how?: string;
  rows: Cell[][];
  /** 카드를 이을 곳 (자료 · 쪽 · 영역) */
  region: Omit<Region, "kind" | "text">;
  rootDeckId: number;
  /** 넣을 곳 기본값 (R8) */
  deckId: number;
  onClose: () => void;
  /** 만들거나 되돌린 뒤 (연결 수를 다시 센다) */
  onDone: () => void;
  /** 만든 뒤에만 (되돌리기는 아니다) */
  onMade?: () => void;
}) {
  const toast = useToast();
  const [cells, setCells] = useState<Cell[][]>(() => rows.map((r) => r.map((c) => ({ ...c }))));
  const [header, setHeader] = useState(true);
  const [method, setMethod] = useState<TableMethod>("cloze");
  // 첫 열(행 이름)과 머리글은 기본으로 묻지 않는다
  const [cols, setCols] = useState<number[]>(() => [width(rows) > 1 ? 1 : 0]);
  const [hide, setHide] = useState(true);
  const [drafts, setDrafts] = useState<RegionCard[]>([]);
  const [at, setAt] = useState<[number, number] | null>(null);
  const [i, setI] = useState(0);
  const [deck, setDeck] = useState(deckId);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    let live = true;
    sourceApi.tableCards({ rows: cells, header, method, cols, hide_same_col: hide }).then(
      (d) => {
        if (!live) return;
        setDrafts(d);
        setI((x) => Math.min(x, Math.max(0, d.length - 1)));
      },
      (e) => live && setError(errorText(e)),
    );
    return () => {
      live = false;
    };
  }, [cells, header, method, cols, hide]);

  const w = width(cells);
  const colName = (c: number) => {
    if (header) for (let j = c; j >= 0; j--) if (cells[0]?.[j] && !cells[0][j].merged) return cells[0][j].text.trim() || `${c + 1}열`;
    return `${c + 1}열`;
  };
  const edit = (r: number, c: number, text: string) => setCells((g) => g.map((row, ri) => (ri !== r ? row : row.map((x, ci) => (ci !== c ? x : { ...x, text })))));
  const [r0, c0] = at ?? [cells.length - 1, w - 1];
  const addRow = () =>
    setCells((g) => {
      const u = unmerged(g);
      u.splice(r0 + 1, 0, Array.from({ length: w }, blank));
      return u;
    });
  const addCol = () => {
    setCells((g) => unmerged(g).map((row) => (row.splice(c0 + 1, 0, blank()), row)));
    setCols((cs) => cs.map((c) => (c > c0 ? c + 1 : c)));
  };
  const delRow = () => {
    setCells((g) => unmerged(g).filter((_, ri) => ri !== r0));
    setAt(null);
  };
  const delCol = () => {
    setCells((g) => unmerged(g).map((row) => row.filter((_, ci) => ci !== c0)));
    setCols((cs) => cs.filter((c) => c !== c0).map((c) => (c > c0 ? c - 1 : c)));
    setAt(null);
  };
  // 머리글이 없거나 합친 머리글 칸이 있으면 표 위에 열 고르기 줄을 따로 둔다 (열마다 하나, 이름은 질문에 쓰는 것)
  const pickRow = !header || (cells[0] ?? []).some((x) => !x.merged && x.col_span > 1);
  const pickName = (c: number) => (header ? `${c + 1}열 · ${colName(c)}` : `${c + 1}열`);
  const pick = (c: number) => (
    <Checkbox
      quiet
      label={`${pickRow ? pickName(c) : colName(c)} 묻기`}
      checked={cols.includes(c)}
      onChange={(v) => setCols((cs) => (v ? [...cs, c].sort((a, b) => a - b) : cs.filter((x) => x !== c)))}
    />
  );
  const asked = cols.map(colName).join(" · ");
  const hint =
    method === "cloze"
      ? `${pickRow ? "위 줄" : "머리글"}에서 가릴 열을 체크해요${cols.length ? ` · ${asked} 칸 ${n(drafts.length)}개 → 노트 ${n(drafts.length)}개` : ""}`
      : method === "rows"
        ? "뒷면에 넣을 열을 체크해요 · 첫 열은 행 이름이라 앞면에 둬요"
        : "목록으로 만들 열을 체크해요 · 칸이 2개 이상인 열만 카드가 돼요";

  const create = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const change = await sourceApi.regionCards(deck, { ...region, kind: "table", text: tableText(cells) }, drafts);
      toast({
        title: `카드 ${n(drafts.length)}장을 만들었어요`,
        text: "자료의 표와 이어 두었어요.",
        tone: "undo",
        action: {
          label: "되돌리기",
          onClick: () => qualityApi.undo(change).then(() => (toast({ title: "카드 만들기를 되돌렸어요" }), onDone()), (e) => toast({ title: "되돌리지 못했어요", text: errorText(e), tone: "error" })),
        },
      });
      onDone();
      onMade?.();
      onClose();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  const d = drafts[i];
  return (
    <Modal title={title} sub={sub} onClose={onClose} className="tg-modal">
      <div className="tg-mode">
        <SegmentControl boxed label="만드는 방식" value={method} onChange={setMethod} options={MODES} />
        <span className="t-body-s c-secondary">{MODES.find((x) => x.value === method)?.hint}</span>
      </div>
      <div className="tg-body">
        <div className="tg-editor">
          <div className="tg-editor__head">
            <span className="t-caption c-secondary">{how}</span>
            <Checkbox label="첫 행은 머리글" checked={header} onChange={setHeader} />
          </div>
          <div className="tg__grid">
            <table className="tg">
              <tbody>
                {pickRow && (
                  <tr className="tg__pick">
                    {Array.from({ length: w }, (_, c) => (
                      <td key={c}>
                        <span className="tg__cell">
                          {pick(c)}
                          <span className="t-label-s c-secondary">{pickName(c)}</span>
                        </span>
                      </td>
                    ))}
                  </tr>
                )}
                {cells.map((row, r) => (
                  <tr key={r} className={header && r === 0 ? "tg__headrow" : undefined}>
                    {row.map((x, c) =>
                      x.merged ? null : (
                        <td
                          key={c}
                          rowSpan={x.row_span > 1 ? Math.min(x.row_span, cells.length - r) : undefined}
                          colSpan={x.col_span > 1 ? x.col_span : undefined}
                          className={!(header && r === 0) && cols.includes(c) ? "is-asked" : undefined}
                        >
                          <span className="tg__cell">
                            {header && r === 0 && !pickRow && pick(c)}
                            <input className="tg__input t-body-s" value={x.text} aria-label={`${r + 1}행 ${c + 1}열`} onFocus={() => setAt([r, c])} onChange={(e) => edit(r, c, e.target.value)} />
                          </span>
                        </td>
                      ),
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="tg-tools">
            <Button variant="ghost" size="M" icon="plus" label="행" onClick={addRow} />
            <Button variant="ghost" size="M" icon="plus" label="열" onClick={addCol} />
            <Button variant="ghost" size="M" icon="x" label={`${at ? "고른" : "마지막"} 행 빼기`} disabled={cells.length <= 1} onClick={delRow} />
            <Button variant="ghost" size="M" icon="x" label={`${at ? "고른" : "마지막"} 열 빼기`} disabled={w <= 1} onClick={delCol} />
            {merges(cells) > 0 && <span className="t-caption c-tertiary">행 · 열을 넣거나 빼면 합친 칸은 풀려요</span>}
          </div>
          <p className="t-caption c-secondary">{hint}</p>
          {method === "cloze" && (
            <span className="tg-hide">
              <Checkbox label="같은 열의 다른 칸도 가리기" checked={hide} onChange={setHide} />
              <span className="t-caption c-tertiary">옆 줄에서 답이 보이지 않게 · Anki로 보내도 그대로예요</span>
            </span>
          )}
        </div>
        <div className="tg-preview">
          <div className="tg-preview__head">
            <span className="t-label-m">
              미리보기 · 카드 {n(d ? i + 1 : 0)} / {n(drafts.length)}
            </span>
            <span className="tg-preview__nav">
              <button className="src-icon-btn" aria-label="이전 카드" disabled={i <= 0} onClick={() => setI(i - 1)}>
                <Icon name="chevron-left" size={16} scaled />
              </button>
              <button className="src-icon-btn" aria-label="다음 카드" disabled={i >= drafts.length - 1} onClick={() => setI(i + 1)}>
                <Icon name="chevron-right" size={16} />
              </button>
            </span>
          </div>
          <div className="tg-card">{d ? <DraftView d={d} /> : <p className="t-body-s c-tertiary">만들 카드가 없어요. 물을 열을 골라 주세요.</p>}</div>
          <p className="t-caption c-tertiary">{QUESTION_HINT[method]} 만든 뒤 카드 편집에서 고칠 수 있어요.</p>
        </div>
      </div>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="fig-foot">
        <span className="fig-foot__where">
          <span className="t-label-m">
            노트 {n(drafts.length)}개 · 카드 {n(drafts.length)}장
          </span>
          <DeckPicker rootDeckId={rootDeckId} value={deck} onChange={(x) => setDeck(x.id)} relative prefix="넣을 곳" />
        </span>
        <span className="modal__actions">
          <Button variant="ghost" size="M" label="취소" onClick={onClose} />
          <Button size="M" label={`카드 ${n(drafts.length)}장 만들기`} disabled={!drafts.length} busy={busy} busyLabel="만드는 중…" onClick={create} />
        </span>
      </div>
    </Modal>
  );
}
