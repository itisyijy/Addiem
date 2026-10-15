// 8.4 PDF 그림 · 표 찾기 (DECISIONS IT1 · IT2 · IT7). pdf.js 의 연산자 목록 · 글자 항목만 받는 순수 함수라
// node 로 바로 검사한다 (tools/check_figures.mjs). 좌표는 쪽 왼쪽 위 기준 pt (scale 1 viewport), 화면에 줄 때만 0~1 로 바꾼다
import type { Rect } from "./sourceApi";

/** [x0, y0, x1, y1] pt */
export type Box = [number, number, number, number];
/** 글자 항목 하나 (size: 글자 높이 pt) */
export type Item = { box: Box; text: string; size: number };
/** 쪽 하나. hl: 가로 괘선 [x0, x1, y] · vl: 세로 괘선 [y0, y1, x] (같은 줄 위 조각은 이어 붙였다) */
export type Layout = { w: number; h: number; images: Box[]; items: Item[]; hl: [number, number, number][]; vl: [number, number, number][] };

type Ops = { fnArray: ArrayLike<number>; argsArray: ArrayLike<unknown> };
type TextLike = { str: string; transform: number[]; width: number; height: number };
type Viewport = { width: number; height: number; convertToViewportPoint: (x: number, y: number) => number[] };
type M = [number, number, number, number, number, number];

const mul = (m: M, n: ArrayLike<number>): M => [
  m[0] * n[0] + m[2] * n[1],
  m[1] * n[0] + m[3] * n[1],
  m[0] * n[2] + m[2] * n[3],
  m[1] * n[2] + m[3] * n[3],
  m[0] * n[4] + m[2] * n[5] + m[4],
  m[1] * n[4] + m[3] * n[5] + m[5],
];
const bounds = (pts: number[][]): Box => [
  Math.min(...pts.map((p) => p[0])),
  Math.min(...pts.map((p) => p[1])),
  Math.max(...pts.map((p) => p[0])),
  Math.max(...pts.map((p) => p[1])),
];
const area = (b: Box) => Math.max(0, b[2] - b[0]) * Math.max(0, b[3] - b[1]);
const inter = (a: Box, b: Box) => area([Math.max(a[0], b[0]), Math.max(a[1], b[1]), Math.min(a[2], b[2]), Math.min(a[3], b[3])]);
const center = (b: Box) => [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2];
const inside = (b: Box, r: Box) => {
  const [x, y] = center(b);
  return x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3];
};
const median = (v: number[]) => {
  const s = [...v].sort((a, b) => a - b);
  return s.length ? s[Math.floor(s.length / 2)] : 0;
};

/** 같은 줄(가로면 y, 세로면 x)의 조각을 이어 붙인다. 2pt 안이면 같은 줄, 2pt 안 틈은 잇는다 */
function joinLines(segs: [number, number, number][]): [number, number, number][] {
  const out: [number, number, number][] = [];
  for (const s of [...segs].sort((a, b) => a[2] - b[2] || a[0] - b[0])) {
    const same = out.filter((o) => Math.abs(o[2] - s[2]) <= 2 && s[0] <= o[1] + 2 && s[1] >= o[0] - 2);
    if (!same.length) {
      out.push([...s]);
      continue;
    }
    const o = same[0];
    o[0] = Math.min(o[0], s[0]);
    o[1] = Math.max(o[1], s[1]);
  }
  return out;
}

/**
 * 연산자 목록 → 그림 상자 · 괘선, 글자 항목 → 글자 상자. 변환 행렬(cm · 양식 XObject)을 따라간다.
 * 그림 위치는 page.imageCoordinates 대신 여기서 센다 (그건 그 쪽을 처음 그릴 때만 남아, I1 미리보기가 먼저 그리면 비어 있다)
 */
export function layoutOf(ops: Ops, OPS: Record<string, number>, text: TextLike[], vp: Viewport): Layout {
  const pt = (m: M, x: number, y: number) => vp.convertToViewportPoint(m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
  const unit = (m: M) => bounds([pt(m, 0, 0), pt(m, 1, 0), pt(m, 0, 1), pt(m, 1, 1)]);
  const images: Box[] = [];
  const hl: [number, number, number][] = [];
  const vl: [number, number, number][] = [];
  const line = (a: number[], b: number[]) => {
    if (Math.abs(a[1] - b[1]) < 0.5 && Math.abs(a[0] - b[0]) > 3) hl.push([Math.min(a[0], b[0]), Math.max(a[0], b[0]), (a[1] + b[1]) / 2]);
    else if (Math.abs(a[0] - b[0]) < 0.5 && Math.abs(a[1] - b[1]) > 3) vl.push([Math.min(a[1], b[1]), Math.max(a[1], b[1]), (a[0] + b[0]) / 2]);
  };
  const stroked = new Set([OPS.stroke, OPS.closeStroke, OPS.fillStroke, OPS.eoFillStroke, OPS.closeFillStroke, OPS.closeEOFillStroke]);
  const filled = new Set([OPS.fill, OPS.eoFill]);
  const stack: M[] = [];
  let ctm: M = [1, 0, 0, 1, 0, 0];
  for (let i = 0; i < ops.fnArray.length; i++) {
    const fn = ops.fnArray[i];
    const args = ops.argsArray[i] as unknown[];
    if (fn === OPS.save) stack.push(ctm);
    else if (fn === OPS.restore) ctm = stack.pop() ?? ctm;
    else if (fn === OPS.transform) ctm = mul(ctm, args as number[]);
    else if (fn === OPS.paintFormXObjectBegin) {
      stack.push(ctm);
      if (args[0]) ctm = mul(ctm, args[0] as number[]);
    } else if (fn === OPS.paintFormXObjectEnd) ctm = stack.pop() ?? ctm;
    else if (fn === OPS.paintImageXObject || fn === OPS.paintInlineImageXObject || fn === OPS.paintImageMaskXObject) images.push(unit(ctm));
    else if (fn === OPS.paintImageXObjectRepeat) {
      // 같은 그림을 여러 자리에 (조각난 그림). 붙은 상자는 figuresOf 가 합친다
      const [, sx, sy, pos] = args as [unknown, number, number, ArrayLike<number>];
      for (let q = 0; q + 1 < pos.length; q += 2) images.push(unit(mul(ctm, [sx, 0, 0, sy, pos[q], pos[q + 1]])));
    } else if (fn === OPS.constructPath) {
      // 6.x: [그리기 연산, [Float32Array 경로], minMax]. 경로: 0 moveTo x y · 1 lineTo x y · 2 curveTo ×6 · 3 quadratic ×4 · 4 closePath
      const [op, [path], minMax] = args as [number, [ArrayLike<number> | null], ArrayLike<number> | null];
      if (stroked.has(op) && path && typeof path.length === "number") {
        let start: number[] = [0, 0];
        let cur: number[] = [0, 0];
        for (let k = 0; k < path.length; ) {
          const c = path[k];
          if (c === 0) {
            start = cur = pt(ctm, path[k + 1], path[k + 2]);
            k += 3;
          } else if (c === 1) {
            const p = pt(ctm, path[k + 1], path[k + 2]);
            line(cur, p);
            cur = p;
            k += 3;
          } else if (c === 2) {
            cur = pt(ctm, path[k + 5], path[k + 6]);
            k += 7;
          } else if (c === 3) {
            cur = pt(ctm, path[k + 3], path[k + 4]);
            k += 5;
          } else {
            line(cur, start);
            cur = start;
            k += 1;
          }
        }
      } else if (filled.has(op) && minMax) {
        // 칠한 가는 사각형(2.5pt 미만)은 괘선으로 (Word · PowerPoint 표 테두리)
        const b = bounds([pt(ctm, minMax[0], minMax[1]), pt(ctm, minMax[2], minMax[1]), pt(ctm, minMax[0], minMax[3]), pt(ctm, minMax[2], minMax[3])]);
        const [bw, bh] = [b[2] - b[0], b[3] - b[1]];
        if (bh < 2.5 && bw > 3) hl.push([b[0], b[2], (b[1] + b[3]) / 2]);
        else if (bw < 2.5 && bh > 3) vl.push([b[1], b[3], (b[0] + b[2]) / 2]);
      }
    }
  }
  // 글자 상자: readPdf 의 linesOf 와 같은 셈 (기준선 아래 0.25 · 위 0.85)
  const items: Item[] = [];
  for (const it of text) {
    if (!it.str.trim()) continue;
    const [, , c, d, e, f] = it.transform;
    const size = Math.hypot(c, d) || it.height || 10;
    const b = bounds([vp.convertToViewportPoint(e, f - size * 0.25), vp.convertToViewportPoint(e + it.width, f + size * 0.85)]);
    items.push({ box: b, text: it.str, size: b[3] - b[1] });
  }
  return { w: vp.width, h: vp.height, images, items, hl: joinLines(hl), vl: joinLines(vl) };
}

// ---------------------------------------------------------------- 그림

/** 그림 상자: 겹치거나 붙은 것(조각난 그림)은 합치고, 쪽의 85% 넘는 것(배경 · 스캔)과 2% 안 되는 것(로고)은 뺀다. 읽는 순서 */
export function figuresOf(lay: Layout): Box[] {
  const page = lay.w * lay.h;
  const big = (b: Box) => area(b) / page > 0.85;
  // 배경은 합치기 전에 뺀다 (다른 그림을 다 덮어 하나로 합쳐 버린다)
  let boxes = lay.images.map((b): Box => [Math.max(0, b[0]), Math.max(0, b[1]), Math.min(lay.w, b[2]), Math.min(lay.h, b[3])]).filter((b) => area(b) > 0 && !big(b));
  for (let merged = true; merged; ) {
    merged = false;
    for (let i = 0; i < boxes.length && !merged; i++)
      for (let j = i + 1; j < boxes.length && !merged; j++) {
        const [a, b] = [boxes[i], boxes[j]];
        if (a[0] <= b[2] + 1 && b[0] <= a[2] + 1 && a[1] <= b[3] + 1 && b[1] <= a[3] + 1) {
          boxes = boxes.filter((_, k) => k !== i && k !== j).concat([[Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])]]);
          merged = true;
        }
      }
  }
  return boxes.filter((b) => area(b) / page >= 0.02 && !big(b)).sort((a, b) => a[1] - b[1] || a[0] - b[0]);
}

// ---------------------------------------------------------------- 줄 · 칸

type Chunk = Item;
const cy = (b: Box) => (b[1] + b[3]) / 2;

/** 글자 항목 → 줄. 줄마다 가까운 항목(글자 높이 0.6 안 틈)은 한 조각으로 */
export function rowsOf(items: Item[]): Chunk[][] {
  const rows: Item[][] = [];
  for (const it of [...items].sort((a, b) => cy(a.box) - cy(b.box) || a.box[0] - b.box[0])) {
    const r = rows[rows.length - 1];
    if (r && Math.abs(cy(it.box) - cy(r[0].box)) < Math.min(it.size, r[0].size) * 0.5) r.push(it);
    else rows.push([it]);
  }
  return rows.map((r) => {
    const out: Chunk[] = [];
    for (const it of r.sort((a, b) => a.box[0] - b.box[0])) {
      const last = out[out.length - 1];
      const gap = last ? it.box[0] - last.box[2] : Infinity;
      if (last && gap <= Math.max(last.size, it.size) * 0.6) {
        const space = gap > Math.min(last.size, it.size) * 0.15 && !/\s$/.test(last.text) && !/^\s/.test(it.text);
        out[out.length - 1] = { box: [last.box[0], Math.min(last.box[1], it.box[1]), Math.max(last.box[2], it.box[2]), Math.max(last.box[3], it.box[3])], text: last.text + (space ? " " : "") + it.text, size: Math.max(last.size, it.size) };
      } else out.push({ ...it, box: [...it.box] });
    }
    return out;
  });
}

/** 글자로 나눈 열 경계: 가장 흔한 조각 수의 줄들에서 i 열 오른쪽 끝과 i+1 열 왼쪽 끝 사이 (오른쪽 정렬 숫자 열도 된다) */
function textCols(rows: Chunk[][]): number[] {
  const multi = rows.filter((r) => r.length >= 2);
  if (!multi.length) return [];
  const count = new Map<number, number>();
  multi.forEach((r) => count.set(r.length, (count.get(r.length) ?? 0) + 1));
  const k = [...count].sort((a, b) => b[1] - a[1] || b[0] - a[0])[0][0];
  const model = multi.filter((r) => r.length === k);
  return Array.from({ length: k - 1 }, (_, i) => {
    const right = Math.max(...model.map((r) => r[i].box[2]));
    const left = Math.min(...model.map((r) => r[i + 1].box[0]));
    return right <= left ? (right + left) / 2 : median(model.map((r) => (r[i].box[2] + r[i + 1].box[0]) / 2));
  });
}

/**
 * 영역 안 표 (IT7): 괘선이 있으면 괘선으로, 없으면 글자 위치로 나눈다. 칸은 글자만 (합친 칸은 찾지 않는다, 표 창에서 고친다).
 * 빈 행 · 빈 열은 뺀다. 글자가 없으면 [] (스캔 · 그림 속 표)
 */
export function gridIn(lay: Layout, box: Box): string[][] {
  const its = lay.items.filter((it) => inside(it.box, box));
  if (!its.length) return [];
  const ext = bounds(its.flatMap((i) => [[i.box[0], i.box[1]], [i.box[2], i.box[3]]]));
  // 글자 범위 안쪽을 가르는 괘선만 (바깥 테두리는 칸을 나누지 않는다)
  const vs = lay.vl.filter(([y0, y1, x]) => x > ext[0] + 1 && x < ext[2] - 1 && Math.min(y1, box[3]) - Math.max(y0, box[1]) >= (box[3] - box[1]) * 0.4).map((v) => v[2]);
  const hs = lay.hl.filter(([x0, x1, y]) => y > ext[1] + 1 && y < ext[3] - 1 && Math.min(x1, box[2]) - Math.max(x0, box[0]) >= (box[2] - box[0]) * 0.5).map((h) => h[2]);
  const rows = rowsOf(its);
  const colEdges = (vs.length ? vs : textCols(rows)).sort((a, b) => a - b);
  const colOf = (b: Box) => colEdges.filter((e) => e < center(b)[0]).length;
  const width = colEdges.length + 1;
  let grid: string[][];
  if (vs.length && hs.length) {
    // 칸마다 괘선이 있는 표: 괘선 사이가 한 행 (칸 안 여러 줄은 한 칸으로)
    const rowEdges = [...hs].sort((a, b) => a - b);
    grid = Array.from({ length: rowEdges.length + 1 }, () => Array<string>(width).fill(""));
    for (const r of rows)
      for (const c of r) {
        const ri = rowEdges.filter((e) => e < cy(c.box)).length;
        const ci = colOf(c.box);
        grid[ri][ci] = grid[ri][ci] ? `${grid[ri][ci]} ${c.text.trim()}` : c.text.trim();
      }
  } else {
    grid = rows.map((r) => {
      const row = Array<string>(width).fill("");
      for (const c of r) row[colOf(c.box)] = row[colOf(c.box)] ? `${row[colOf(c.box)]} ${c.text.trim()}` : c.text.trim();
      return row;
    });
  }
  grid = grid.filter((r) => r.some((t) => t));
  const used = Array.from({ length: width }, (_, c) => grid.some((r) => r[c]));
  return grid.map((r) => r.filter((_, c) => used[c]));
}

// ---------------------------------------------------------------- 표 제안 (IT2: 제안만, 확정은 사용자)

export type Candidate = { box: Box; rows: number; cols: number; ruled: boolean; reason: string | null };

const BULLET = /^([•·∙▪■□◦‣○●\-–—*]|\(?\d{1,2}[.)]|[a-zA-Z가-힣][.)])$/;

/** 가로 괘선이 2개 이상 지나가는 사각형인가 (북탭 표: 위 · 머리글 아래 · 아래만 줄) */
const ruledBox = (lay: Layout, b: Box) =>
  lay.hl.filter(([x0, x1, y]) => y >= b[1] - 6 && y <= b[3] + 6 && Math.min(x1, b[2]) - Math.max(x0, b[0]) >= (b[2] - b[0]) * 0.6).length >= 2;

/**
 * 쪽 전체에서 표일 수 있는 곳. reason 이 null 이면 제안, 아니면 뺀 까닭 (검사 · 측정용).
 * 글자 덩어리: 조각 2개 이상인 줄이 3줄 넘게 이어지고 열 경계를 가로지르는 조각이 적을 것.
 * 뺀다: 모든 열이 긴 글(2단 본문), 글머리표 · 번호 열(목록). 괘선이 있으면 이 둘은 보지 않는다 (확신이 높다).
 * 괘선 격자(가로 3 · 세로 1 이상)는 글자 덩어리와 따로 찾고, 겹치면 격자를 남긴다
 */
export function tableCandidates(lay: Layout): Candidate[] {
  const rows = rowsOf(lay.items);
  const out: Candidate[] = [];
  const judge = (box: Box, why: (g: string[][], ruled: boolean) => string | null) => {
    const pad: Box = [Math.max(0, box[0] - 6), Math.max(0, box[1] - 4), Math.min(lay.w, box[2] + 6), Math.min(lay.h, box[3] + 4)];
    const g = gridIn(lay, pad);
    const ruled = ruledBox(lay, pad);
    const cols = Math.max(0, ...g.map((r) => r.length));
    const reason = g.length < 3 || cols < 2 ? "3행 × 2열보다 작다" : why(g, ruled);
    out.push({ box: pad, rows: g.length, cols, ruled, reason });
  };

  // 괘선 격자: 서로 닿는 괘선을 묶는다
  const lines = [...lay.hl.map(([x0, x1, y]): Box => [x0, y, x1, y]), ...lay.vl.map(([y0, y1, x]): Box => [x, y0, x, y1])];
  const group = lines.map((_, i) => i);
  const root = (i: number): number => (group[i] === i ? i : (group[i] = root(group[i])));
  lines.forEach((a, i) =>
    lines.forEach((b, j) => {
      if (j > i && a[0] <= b[2] + 3 && b[0] <= a[2] + 3 && a[1] <= b[3] + 3 && b[1] <= a[3] + 3) group[root(i)] = root(j);
    }),
  );
  const grids = new Map<number, Box[]>();
  lines.forEach((l, i) => grids.set(root(i), [...(grids.get(root(i)) ?? []), l]));
  for (const ls of grids.values()) {
    const h = ls.filter((l) => l[1] === l[3]).length;
    const v = ls.length - h;
    if (h < 3 || v < 1) continue;
    const box = bounds(ls.flatMap((l) => [[l[0], l[1]], [l[2], l[3]]]));
    if (!lay.items.some((it) => inside(it.box, box))) continue;
    judge(box, () => null);
  }
  const gridBoxes = out.map((c) => c.box);

  // 글자 덩어리: 조각 2개 이상인 줄이 이어지는 곳 (칸 안 줄바꿈처럼 조각 하나인 줄은 2줄까지 끼워 준다)
  const blocks: Chunk[][][] = [];
  let cur: Chunk[][] = [];
  let singles: Chunk[][] = [];
  const close = () => {
    if (cur.filter((r) => r.length >= 2).length >= 3) blocks.push(cur);
    cur = [];
    singles = [];
  };
  for (const r of rows) {
    const prev = (singles.length ? singles[singles.length - 1] : cur[cur.length - 1]) ?? null;
    const lh = Math.max(...r.map((c) => c.size));
    const near = prev && Math.min(...r.map((c) => c.box[1])) - Math.max(...prev.map((c) => c.box[3])) <= lh * 1.6;
    if (r.length >= 2) {
      if (!near) close();
      cur.push(...singles, r);
      singles = [];
    } else if (cur.length && near && singles.length < 2) singles.push(r);
    else close();
  }
  close();
  for (const b of blocks) {
    const box = bounds(b.flat().flatMap((c) => [[c.box[0], c.box[1]], [c.box[2], c.box[3]]]));
    if (gridBoxes.some((g) => inter(g, box) >= Math.min(area(g), area(box)) * 0.5)) continue;
    judge(box, (g, ruled) => {
      // 열 경계를 가로지르는 조각 (열이 들쭉날쭉하다)
      const multi = b.filter((r) => r.length >= 2);
      const edges = textCols(b);
      const crossing = multi.filter((r) => r.some((c) => edges.some((e) => c.box[0] < e - 1 && c.box[2] > e + 1))).length;
      if (crossing > multi.length * 0.2) return "열이 들쭉날쭉하다";
      if (ruled) return null;
      const cols = Math.max(...g.map((r) => r.length));
      const med = Array.from({ length: cols }, (_, c) => median(g.map((r) => r[c] ?? "").filter(Boolean).map((t) => t.length)));
      if (med.every((m) => m >= 18)) return "모든 열이 긴 글 (2단 본문)";
      for (let c = 0; c < cols; c++) {
        const col = g.map((r) => r[c] ?? "").filter(Boolean);
        if (col.length && col.filter((t) => BULLET.test(t.trim())).length >= col.length * 0.8) return "글머리표 · 번호 열 (목록)";
      }
      return null;
    });
  }
  // 그림 위 제안은 뺀다
  const figs = figuresOf(lay);
  for (const c of out) if (!c.reason && figs.some((f) => inter(f, c.box) >= area(c.box) * 0.5)) c.reason = "그림 위";
  return out;
}

export const suggestTables = (lay: Layout) => tableCandidates(lay).filter((c) => !c.reason);

/** 그림 · 표 이름표: 바로 아래(없으면 위) 줄이 "그림 3-4 …" · "표 2 …" · "Figure 1 …" 처럼 시작하면 그 글 (40자) */
export function captionOf(lay: Layout, box: Box): string {
  const near = rowsOf(lay.items.filter((it) => Math.min(it.box[2], box[2]) - Math.max(it.box[0], box[0]) > 0));
  const text = (r: Chunk[]) => r.map((c) => c.text.trim()).join(" ");
  const below = near.find((r) => r[0].box[1] >= box[3] - 1 && r[0].box[1] <= box[3] + r[0].size * 2.5);
  const above = [...near].reverse().find((r) => r[0].box[3] <= box[1] + 1 && r[0].box[3] >= box[1] - r[0].size * 2.5);
  const hit = [below, above].find((r) => r && /^(그림|표|도표|사진|Fig\.?|Figure|Table)\s*[\dA-Z]/i.test(text(r)));
  return hit ? text(hit).slice(0, 40) : "";
}

/** pt 상자 ↔ 0~1 Rect (소수 4자리, 같은 영역을 다시 찾을 때 같은 값이 되게) */
export const rectOf = (lay: { w: number; h: number }, b: Box): Rect => {
  const r = (v: number) => Math.round(v * 1e4) / 1e4;
  return [r(b[0] / lay.w), r(b[1] / lay.h), r((b[2] - b[0]) / lay.w), r((b[3] - b[1]) / lay.h)];
};
export const boxOf = (lay: { w: number; h: number }, r: Rect): Box => [r[0] * lay.w, r[1] * lay.h, (r[0] + r[2]) * lay.w, (r[1] + r[3]) * lay.h];
