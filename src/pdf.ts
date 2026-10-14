// I1 PDF 읽기와 그리기 (pdf.js, Apache-2.0). 워커와 CMap 은 앱에 들어 있다 (CDN 없음, CSP script-src 'self').
// 강조 신호 (post-mvp 5장): 하이라이트·밑줄 주석, 붉은 글씨(채우기 색), 볼드(글꼴), 타이핑한 ★ · *
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from "pdfjs-dist";
import type { TextItem } from "pdfjs-dist/types/src/display/api";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { sourceApi, type NewSegment, type Rect, type Signal } from "./sourceApi";

// 한글 PDF 는 내장 CMap(KSCms-UHC-H 등)을 쓰는 일이 많다. connect-src 가 ipc 만 열려 있어 fetch 대신
// 작은 JS 조각(data URL)으로 묶어 필요할 때만 불러온다. ?inline 만으로는 모르는 확장자를 자산으로 보지 않아 ?url&inline
const cmaps = import.meta.glob("../node_modules/pdfjs-dist/cmaps/*.bcmap", { query: "?url&inline", import: "default" }) as Record<
  string,
  () => Promise<string>
>;

class BundledData {
  constructor(_: unknown) {}
  async fetch({ kind, filename }: { kind: string; filename: string }) {
    const load = kind === "cMapUrl" ? Object.entries(cmaps).find(([k]) => k.endsWith(`/${filename}`))?.[1] : undefined;
    // 표준 글꼴은 시스템 글꼴로 대신하고(useSystemFonts), wasm 디코더는 CSP 가 막으므로 JS 로 돌아간다
    if (!load) throw new Error(`${kind} ${filename} 없음`);
    const url = await load();
    return Uint8Array.from(atob(url.slice(url.indexOf(",") + 1)), (c) => c.charCodeAt(0));
  }
}

let lib: Promise<typeof import("pdfjs-dist")> | null = null;
const pdfjs = () =>
  (lib ??= import("pdfjs-dist").then((m) => {
    m.GlobalWorkerOptions.workerSrc = workerUrl;
    return m;
  }));

const docs = new Map<number, Promise<PDFDocumentProxy>>();
/** 자료마다 한 번 연다 (I1 미리보기와 I2 패널이 같이 쓴다) */
export function openPdf(materialId: number): Promise<PDFDocumentProxy> {
  let d = docs.get(materialId);
  if (!d) {
    d = Promise.all([pdfjs(), sourceApi.bytes(materialId)]).then(
      ([m, bytes]) =>
        m.getDocument({
          data: new Uint8Array(bytes),
          useWorkerFetch: false,
          cMapUrl: "cmaps/",
          cMapPacked: true,
          BinaryDataFactory: BundledData,
        }).promise,
    );
    d.catch(() => docs.delete(materialId));
    docs.set(materialId, d);
  }
  return d;
}

/** 쪽 하나를 width(px) 폭으로 그린다. 하이라이트 주석도 함께 그려진다. 화면을 떠나면 cancel() */
export async function renderPage(materialId: number, pageNo: number, canvas: HTMLCanvasElement, width: number): Promise<RenderTask> {
  const page = await (await openPdf(materialId)).getPage(pageNo);
  const base = page.getViewport({ scale: 1 });
  const scale = width / base.width;
  const vp = page.getViewport({ scale: scale * (window.devicePixelRatio || 1) });
  canvas.width = Math.floor(vp.width);
  canvas.height = Math.floor(vp.height);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${Math.round(base.height * scale)}px`;
  return page.render({ canvas, viewport: vp });
}

type Line = { text: string; box: [number, number, number, number]; bold: boolean; size: number };

const squash = (s: string) => s.replace(/\s+/g, "");
const isRed = (hex: string) => {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return r >= 150 && g <= 90 && b <= 90 && r - Math.max(g, b) >= 100;
};
const hasStar = (t: string) => /[★☆]/.test(t) || /^\s*\*/.test(t) || /\*\s*$/.test(t);

/** 채우기 색이 붉은 동안 그린 글자들 (연산자 목록을 따라가며 save/restore 를 지킨다) */
async function redRuns(page: PDFPageProxy, OPS: Record<string, number>): Promise<string[]> {
  const ops = await page.getOperatorList();
  const runs: string[] = [];
  const stack: string[] = [];
  let color = "#000000";
  let run = "";
  const flush = () => {
    if (squash(run).length >= 2) runs.push(squash(run));
    run = "";
  };
  ops.fnArray.forEach((fn, i) => {
    const args = ops.argsArray[i];
    if (fn === OPS.save) stack.push(color);
    else if (fn === OPS.restore) color = stack.pop() ?? color;
    else if (fn === OPS.setFillRGBColor) {
      if (isRed(color) && !isRed(args[0])) flush();
      color = args[0];
    } else if (fn === OPS.showText && isRed(color)) {
      for (const g of args[0] as ({ unicode?: string } | number)[]) if (typeof g === "object" && g?.unicode) run += g.unicode;
    }
  });
  flush();
  return runs;
}

/** 텍스트 항목을 줄로 묶는다. hasEOL 이나 기준선이 바뀌면 새 줄 */
function linesOf(items: TextItem[], page: PDFPageProxy): Line[] {
  const lines: Line[] = [];
  let cur: Line | null = null;
  let base = NaN;
  const boldFont = (name: string) => {
    if (!page.commonObjs.has(name)) return false;
    const f = page.commonObjs.get(name) as { bold?: boolean; black?: boolean; name?: string };
    return !!(f.bold || f.black || /bold|black|heavy|semibold|extrabold/i.test(f.name ?? ""));
  };
  for (const it of items) {
    const [, , c, d, e, f] = it.transform;
    const size = Math.hypot(c, d) || it.height || 10;
    if (cur && Math.abs(f - base) > size * 0.5) {
      lines.push(cur);
      cur = null;
    }
    if (it.str.trim()) {
      const box: Line["box"] = [e, f - size * 0.25, e + it.width, f + size * 0.85];
      if (!cur) {
        cur = { text: "", box, bold: false, size };
        base = f;
      }
      cur.text += it.str;
      cur.box = [Math.min(cur.box[0], box[0]), Math.min(cur.box[1], box[1]), Math.max(cur.box[2], box[2]), Math.max(cur.box[3], box[3])];
      if (squash(it.str).length >= 2 && boldFont(it.fontName)) cur.bold = true;
    } else if (cur && it.str) cur.text += " ";
    if (it.hasEOL && cur) {
      lines.push(cur);
      cur = null;
    }
  }
  if (cur) lines.push(cur);
  return lines.map((l) => ({ ...l, text: l.text.replace(/\s+/g, " ").trim() })).filter((l) => l.text);
}

/** PDF 전체를 읽어 강조 구간을 만든다. 글자가 거의 없으면(스캔본) readable=false */
export async function readPdf(materialId: number): Promise<{ pages: number; segments: NewSegment[]; readable: boolean }> {
  const { OPS } = await pdfjs();
  const doc = await openPdf(materialId);
  const segments: NewSegment[] = [];
  let chars = 0;
  for (let p = 1; p <= doc.numPages; p++) {
    const page = await doc.getPage(p);
    const vp = page.getViewport({ scale: 1 });
    // 연산자 목록을 먼저 받아야 글꼴(commonObjs)이 풀려 볼드를 알 수 있다
    const red = await redRuns(page, OPS);
    const [content, annots] = await Promise.all([page.getTextContent(), page.getAnnotations()]);
    const lines = linesOf(content.items.filter((i): i is TextItem => "str" in i), page);
    chars += lines.reduce((s, l) => s + squash(l.text).length, 0);
    const marks = (annots as { subtype?: string; quadPoints?: ArrayLike<number> | null; rect: number[] }[])
      .filter((a) => a.subtype === "Highlight" || a.subtype === "Underline" || a.subtype === "Squiggly")
      .flatMap((a) => {
        const q = a.quadPoints;
        const boxes: number[][] = [];
        if (q && q.length >= 8) for (let i = 0; i + 7 < q.length; i += 8) boxes.push([q[i + 4], q[i + 5], q[i + 2], q[i + 3]]);
        else boxes.push(a.rect);
        return boxes.map((b) => ({ sig: (a.subtype === "Highlight" ? "highlight" : "underline") as Signal, box: b }));
      });
    const sigs = lines.map((l) => {
      const out = new Set<Signal>();
      const h = l.box[3] - l.box[1];
      for (const m of marks) {
        const [x0, y0, x1, y1] = [Math.min(m.box[0], m.box[2]), Math.min(m.box[1], m.box[3]), Math.max(m.box[0], m.box[2]), Math.max(m.box[1], m.box[3])];
        const oy = Math.min(y1, l.box[3]) - Math.max(y0, l.box[1]);
        const ox = Math.min(x1, l.box[2]) - Math.max(x0, l.box[0]);
        if (ox > 0 && oy >= h * 0.4) out.add(m.sig);
      }
      const t = squash(l.text);
      if (red.some((r) => t.includes(r) || (t.length >= 2 && r.includes(t)))) out.add("red");
      if (hasStar(l.text)) out.add("star");
      if (l.bold) out.add("bold");
      return [...out];
    });
    // 같은 신호가 이어지는 줄(두 줄짜리 하이라이트)은 한 구간으로
    let ord = 0;
    for (let i = 0; i < lines.length; i++) {
      if (!sigs[i].length) continue;
      let j = i;
      while (j + 1 < lines.length && sigs[j + 1].join() === sigs[i].join() && !hasStar(lines[j + 1].text)) j++;
      const rects = lines.slice(i, j + 1).map((l) => {
        const [a, b] = vp.convertToViewportPoint(l.box[0], l.box[1]);
        const [c, d] = vp.convertToViewportPoint(l.box[2], l.box[3]);
        const [x0, x1, y0, y1] = [Math.min(a, c), Math.max(a, c), Math.min(b, d), Math.max(b, d)];
        return [x0 / vp.width, y0 / vp.height, (x1 - x0) / vp.width, (y1 - y0) / vp.height] as Rect;
      });
      segments.push({
        page: p,
        ord: ord++,
        text: lines
          .slice(i, j + 1)
          .map((l) => l.text)
          .join(" "),
        signals: sigs[i],
        rects,
        before: lines[i - 1]?.text ?? null,
        after: lines[j + 1]?.text ?? null,
      });
      i = j;
    }
    page.cleanup();
  }
  return { pages: doc.numPages, segments, readable: chars >= 20 };
}
