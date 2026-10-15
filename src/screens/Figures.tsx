// 8.4 그림 · 표로 카드 만들기 (Figma Row IT: IT1-p 176:14681 · IT3 168:13918, 노트 178:14484 · 170:14084, DECISIONS IT).
// PPTX 는 앱이 슬라이드를 그림으로 그리지 않아 끌어서 잡기 없이, 그림 · 표가 있는 슬라이드 목록에서 고른다 (IT1-p).
// 그림 → 그림 가리기(M2 편집기) · 그림을 앞면에 둔 카드, 표 → 표로 카드 만들기(IT3). EMF · WMF 는 만들지 않는다 (IT11).
// 만든 카드는 그 슬라이드 · 영역 구간에 이어진다 (IT9). TableCards 와 영역 잇기는 PDF(2단계)도 그대로 쓴다
import "../styles/figures.css";
import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText, qualityApi } from "../api";
import { sanitize } from "../cardHtml";
import { Badge, Button, Checkbox, Icon, Input, Segment as SegmentControl } from "../ui";
import { n, useKey, useOverlayScope } from "../nav";
import { useToast } from "../toast";
import { Modal } from "./NewDeck";
import { DeckPicker } from "./DeckPicker";
import { menuKeys } from "./Deck";
import { OcclusionEditor } from "./Occlusion";
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
const sameRect = (a: Rect | undefined, b: Rect) => !!a && a.every((v, i) => Math.abs(v - b[i]) < 1e-6);
/** 이 영역 구간에 이어진 카드 수 (목록 · 타일의 "카드 N장") */
export const linkedCards = (segs: Segment[], page: number, kind: "figure" | "table", rect: Rect) =>
  segs.filter((s) => s.page === page && s.signals.includes(kind) && sameRect(s.rects?.[0], rect)).reduce((t, s) => t + s.links.length, 0);

const width = (rows: Cell[][]) => Math.max(0, ...rows.map((r) => r.length));
const merges = (rows: Cell[][]) => rows.flat().filter((c) => !c.merged && (c.row_span > 1 || c.col_span > 1)).length;
/** "4행 × 3열 · 합친 칸 1" */
export const sizeLabel = (rows: Cell[][]) => `${rows.length}행 × ${width(rows)}열${merges(rows) ? ` · 합친 칸 ${merges(rows)}` : ""}`;
/** 구간 글자 (I2 패널 · 검색): 행마다 한 줄, 칸은 " · " */
const tableText = (rows: Cell[][]) =>
  rows
    .map((r) => r.filter((c) => !c.merged && c.text.trim()).map((c) => c.text.replace(/\s+/g, " ").trim()).join(" · "))
    .filter(Boolean)
    .join("\n");
const imageLabel = (img: SlideImage) => img.descr.trim() || img.name.trim() || `그림 ${img.index + 1}`;

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
  return invoke<string>("save_media", { name: `slide.${img.ext}`, dataBase64: data.split(",")[1] ?? "" });
}

// ---------------------------------------------------------------- IT1-p 슬라이드별 목록

type Item = { o: SlideObjects; key: string; k: number; img?: SlideImage; table?: SlideTable };
const itemsOf = (o: SlideObjects): Item[] => [
  ...o.images.map((img, k) => ({ o, key: `${o.page}:i${k}`, k, img })),
  ...o.tables.map((table, k) => ({ o, key: `${o.page}:t${k}`, k, table })),
];
const usable = (it: Item) => !it.img || it.img.supported;

/** 가운데 열(슬라이드 카드)과 오른쪽 목록. head: 목록 위 탭 줄 (Sources 가 준다) */
export function Figures({ m, objs, error, segs, deckId, head, onChanged }: {
  m: Material;
  objs: SlideObjects[] | null;
  error?: string;
  segs: Segment[];
  /** 자료 화면의 덱 (넣을 곳 고르기의 뿌리) */
  deckId: number;
  head: ReactNode;
  onChanged: () => void;
}) {
  const toast = useToast();
  const [menu, setMenu] = useState<string | null>(null);
  const [occl, setOccl] = useState<{ it: Item; pic: Picture } | null>(null);
  const [front, setFront] = useState<Item | null>(null);
  const [table, setTable] = useState<Item | null>(null);
  useOverlayScope(menu !== null);
  useKey("Escape", () => setMenu(null), menu !== null, { modal: true });
  useEffect(() => dropThumbs, [m.id]);

  const items = (objs ?? []).flatMap(itemsOf);
  const linked = (it: Item) => linkedCards(segs, it.o.page, it.img ? "figure" : "table", (it.img ?? it.table)!.rect);
  const region = (it: Item): Omit<Region, "kind" | "text"> => ({ material_id: m.id, page: it.o.page, rects: [(it.img ?? it.table)!.rect] });
  const place = (it: Item) => `${m.file_name} · 슬라이드 ${it.o.page}`;
  const open = (it: Item, where: string) => (it.table ? setTable(it) : setMenu(`${where}:${it.key}`));

  const startOcclusion = async (it: Item) => {
    setMenu(null);
    try {
      const { url } = await thumb(m.id, it.o.page, it.img!);
      setOccl({ it, pic: { name: "", url, fileName: place(it), ...(await naturalSize(url)) } });
    } catch (e) {
      toast({ title: "그림을 열지 못했어요", text: errorText(e), tone: "error" });
    }
  };
  const saveOcclusion = async (it: Item, shapes: Shape[], hideAll: boolean) => {
    try {
      const image = await storeImage(m.id, it.o.page, it.img!);
      const note = await occlusionApi.save({ note_id: null, deck_id: it.o.deck_id, image, header: "", back_extra: esc(place(it)), shapes, hide_all: hideAll, importance: null });
      await sourceApi.regionLink({ ...region(it), kind: "figure", text: it.img!.descr.trim() }, [note]);
      toast({ title: `그림 가리기 카드 ${n(ordinalsOf(shapes).length)}장을 만들었어요`, text: "슬라이드 그림과 이어 두었어요." });
      setOccl(null);
      onChanged();
    } catch (e) {
      toast({ title: "카드를 만들지 못했어요", text: errorText(e), tone: "error" });
    }
  };

  const imageMenu = (it: Item, where: string) =>
    menu === `${where}:${it.key}` && (
      <div
        className="menu__list menu--panel fig-menu"
        role="menu"
        aria-label="그림 메뉴"
        onKeyDown={menuKeys}
        onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setMenu(null)}
      >
        <button role="menuitem" autoFocus className="menu__item menu__item--top" disabled={it.img?.ext === "gif"} onClick={() => startOcclusion(it)}>
          <Icon name="image" size={18} />
          <span className="menu__text">
            <span className="t-label-m">그림 가리기 카드로</span>
            <span className="t-caption c-tertiary">{it.img?.ext === "gif" ? "움직이는 GIF는 가릴 수 없어요" : "이 그림으로 가리기 편집기를 열어요"}</span>
          </span>
        </button>
        <button role="menuitem" className="menu__item menu__item--top" onClick={() => (setMenu(null), setFront(it))}>
          <Icon name="layers" size={18} />
          <span className="menu__text">
            <span className="t-label-m">그림을 앞면에 둔 카드</span>
            <span className="t-caption c-tertiary">앞면에 그림, 뒷면에 답을 써요</span>
          </span>
        </button>
      </div>
    );

  return (
    <>
      <div className="fig-slides" aria-label="그림·표가 있는 슬라이드">
        <span className="t-label-m c-secondary">
          그림·표가 있는 슬라이드 {n(objs?.length ?? 0)} / {n(m.pages)}
        </span>
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
                <Tile key={it.key} m={m} it={it} linked={linked(it)} onOpen={() => open(it, "tile")} menu={imageMenu(it, "tile")} />
              ))}
            </div>
          </div>
        ))}
        <p className="t-caption c-tertiary">그림·표가 없는 슬라이드는 빼고 보여요 · 그림을 누르면 메뉴, 표를 누르면 표 카드 창이 열려요</p>
      </div>

      <section className="panel src-list" aria-label="그림과 표">
        {head}
        {objs === null && !error && <p className="src-list__note t-body-s c-secondary">슬라이드를 읽고 있어요…</p>}
        {error && <p className="src-list__note t-body-s c-secondary">{error}</p>}
        {objs?.length === 0 && !error && <p className="src-list__note t-body-s c-secondary">이 자료에서 그림이나 표를 찾지 못했어요.</p>}
        {items.map((it) => {
          const count = linked(it);
          return (
            <div key={it.key} className="fig-row">
              <span className="fig-row__thumb">
                <Icon name={it.img ? "image" : "list"} size={18} />
              </span>
              <span className="fig-row__text">
                <span className="fig-row__top">
                  <Badge>{it.img ? "그림" : "표"}</Badge>
                  <span className="t-caption c-tertiary">슬라이드 {it.o.page}</span>
                </span>
                <span className={`t-body-s ${usable(it) ? "c-primary" : "c-tertiary"}`}>
                  {it.img
                    ? `${imageLabel(it.img)}${it.img.supported ? "" : " · 미리 볼 수 없는 형식"}`
                    : `${it.table!.name || `표 ${it.k + 1}`} · ${it.table!.rows.length}행 × ${width(it.table!.rows)}열`}
                </span>
              </span>
              {!usable(it) ? (
                <span className="t-caption c-tertiary">만들 수 없어요</span>
              ) : count ? (
                <button className="badge badge--accent t-label-s fig-row__count" onClick={() => open(it, "row")}>
                  카드 {n(count)}장
                </button>
              ) : (
                <Button variant="ghost" size="M" icon="plus" label="만들기" onClick={() => open(it, "row")} />
              )}
              {imageMenu(it, "row")}
            </div>
          );
        })}
        {items.length > 0 && <p className="src-list__note t-caption c-tertiary">이미 만든 것도 다시 만들 수 있어요. 같은 그림 · 표에 이어져요.</p>}
      </section>

      {occl && (
        <OcclusionEditor
          picture={occl.pic}
          shapes={[]}
          hideAll
          onCancel={() => setOccl(null)}
          onDone={(shapes, hideAll) => saveOcclusion(occl.it, shapes, hideAll)}
        />
      )}
      {front && (
        <ImageCard
          m={m}
          it={front}
          rootDeckId={deckId}
          onClose={() => setFront(null)}
          onDone={() => {
            setFront(null);
            onChanged();
          }}
          onUndone={onChanged}
        />
      )}
      {table && (
        <TableCards
          sub={`표 ${table.k + 1} · ${place(table)}`}
          rows={table.table!.rows}
          region={region(table)}
          source={place(table)}
          rootDeckId={deckId}
          deckId={table.o.deck_id}
          onClose={() => setTable(null)}
          onDone={onChanged}
        />
      )}
    </>
  );
}

function Tile({ m, it, linked, onOpen, menu }: { m: Material; it: Item; linked: number; onOpen: () => void; menu: ReactNode }) {
  const ok = usable(it);
  const label = it.img
    ? `그림 ${it.k + 1} · ${imageLabel(it.img)}${it.img.supported ? "" : ` · ${it.img.ext.toUpperCase()}`}`
    : `표 ${it.k + 1} · ${sizeLabel(it.table!.rows)}`;
  return (
    <div className="fig-tile">
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

// ---------------------------------------------------------------- IT3 그림을 앞면에 둔 카드

function ImageCard({ m, it, rootDeckId, onClose, onDone, onUndone }: {
  m: Material;
  it: Item;
  rootDeckId: number;
  onClose: () => void;
  onDone: () => void;
  onUndone: () => void;
}) {
  const toast = useToast();
  const img = it.img!;
  const url = useThumb(m.id, it.o.page, img);
  const [question, setQuestion] = useState("");
  const [answer, setAnswer] = useState("");
  const [deck, setDeck] = useState(it.o.deck_id);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const create = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const name = await storeImage(m.id, it.o.page, img);
      const front = `${question.trim() ? `<div>${esc(question.trim())}</div>` : ""}<img src="${esc(name)}">`;
      const change = await sourceApi.regionCards(deck, { material_id: m.id, page: it.o.page, rects: [img.rect], kind: "figure", text: img.descr.trim() }, [
        { kind: "basic", front, back: esc(answer.trim()).replace(/\n/g, "<br>") },
      ]);
      toast({
        title: "카드 1장을 만들었어요",
        text: "슬라이드 그림과 이어 두었어요.",
        tone: "undo",
        action: { label: "되돌리기", onClick: () => qualityApi.undo(change).then(() => (toast({ title: "카드 만들기를 되돌렸어요" }), onUndone()), (e) => toast({ title: "되돌리지 못했어요", text: errorText(e), tone: "error" })) },
      });
      onDone();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal title="그림을 앞면에 둔 카드" sub={`그림 ${it.k + 1} · ${m.file_name} · 슬라이드 ${it.o.page}`} onClose={onClose} className="fig-front">
      <div className="modal__body">
        {url ? <img className="fig-front__img" src={url} alt={imageLabel(img)} /> : <span className="fig-front__img fig-front__img--wait" />}
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
const blank = (): Cell => ({ text: "", row_span: 1, col_span: 1, merged: false });
/** 행 · 열을 넣거나 빼면 합친 칸은 푼다 (글자는 왼쪽 위 칸에 남는다) */
const unmerged = (g: Cell[][]) => g.map((row) => row.map((x) => ({ text: x.merged ? "" : x.text, row_span: 1, col_span: 1, merged: false })));

/** {{c1::답}} → 묻는 칸 표시 (미리보기만) */
function DraftView({ d }: { d: RegionCard }) {
  if (d.kind === "cloze") {
    const answer = /\{\{c1::([\s\S]*?)\}\}/.exec(d.front)?.[1] ?? "";
    return (
      <>
        <div className="card-html tg-card__front" dangerouslySetInnerHTML={{ __html: sanitize(d.front.replace(/\{\{c1::[\s\S]*?\}\}/, '<span class="tg-ask">?</span>')) }} />
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

export function TableCards({ title = "표로 카드 만들기", sub, rows, region, source, rootDeckId, deckId, onClose, onDone }: {
  title?: string;
  /** "표 1 · 강의.pptx · 슬라이드 7" */
  sub: string;
  rows: Cell[][];
  /** 카드를 이을 곳 (자료 · 쪽 · 영역) */
  region: Omit<Region, "kind" | "text">;
  /** 빈칸 노트의 추가 설명 ("강의.pptx · 슬라이드 7") */
  source: string;
  rootDeckId: number;
  /** 넣을 곳 기본값 (R8) */
  deckId: number;
  onClose: () => void;
  /** 만들거나 되돌린 뒤 (연결 수를 다시 센다) */
  onDone: () => void;
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
    sourceApi.tableCards({ rows: cells, header, method, cols, hide_same_col: hide, source }).then(
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
  }, [cells, header, method, cols, hide, source]);

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
  // 합친 머리글 칸은 덮는 열마다 하나씩
  const pick = (c: number, span = 1) =>
    Array.from({ length: span }, (_, k) => c + k).map((col) => (
      <Checkbox
        key={col}
        quiet
        label={`${colName(col)}${span > 1 ? ` ${col + 1}열` : ""} 묻기`}
        checked={cols.includes(col)}
        onChange={(v) => setCols((cs) => (v ? [...cs, col].sort((a, b) => a - b) : cs.filter((x) => x !== col)))}
      />
    ));
  const asked = cols.map(colName).join(" · ");
  const hint =
    method === "cloze"
      ? `머리글에서 가릴 열을 체크해요${cols.length ? ` · ${asked} 칸 ${n(drafts.length)}개 → 노트 ${n(drafts.length)}개` : ""}`
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
            <span className="t-caption c-secondary">표 구조 그대로 가져왔어요 · 칸을 눌러 고쳐요</span>
            <Checkbox label="첫 행은 머리글" checked={header} onChange={setHeader} />
          </div>
          <div className="tg__grid">
            <table className="tg">
              <tbody>
                {!header && (
                  <tr className="tg__pick">
                    {Array.from({ length: w }, (_, c) => (
                      <td key={c}>
                        <span className="tg__cell">
                          {pick(c)}
                          <span className="t-label-s c-secondary">{c + 1}열</span>
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
                            {header && r === 0 && pick(c, Math.min(x.col_span, w - c))}
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
            <Button variant="ghost" size="M" icon="x" label={`${r0 + 1}행 빼기`} disabled={cells.length <= 1} onClick={delRow} />
            <Button variant="ghost" size="M" icon="x" label={`${c0 + 1}열 빼기`} disabled={w <= 1} onClick={delCol} />
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
          {method !== "list" && <p className="t-caption c-tertiary">질문은 "행 이름의 열 이름은?"으로 만들어요. 만든 뒤 카드 편집에서 고칠 수 있어요.</p>}
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
