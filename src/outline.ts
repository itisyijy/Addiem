// 8.4 목차 (DECISIONS OT). PDF 목차(getOutline)를 쪽으로 풀고, 목차가 없으면 큰 글자 줄을 제목으로 본다.
// pdf.js 문서와 줄만 받는 순수 함수라 node 로 바로 검사한다 (tools/check_outline.mjs). 구간 · 핵심 범위는 source.rs outline 이 정한다
import type { PDFDocumentProxy } from "pdfjs-dist";
import type { NewSegment, Rect } from "./sourceApi";

/** 목차 항목 하나. page 는 1부터, level 은 1부터 (6까지) */
export type OutlineItem = { title: string; page: number; level: number };
/** 쪽 하나의 줄 (pdf.ts linesOf): 글 · 글자 크기 pt · 쪽 위 위치 */
export type PageLine = { text: string; size: number; rect?: Rect };
/** 찾은 제목. line: 그 쪽의 몇째 줄인지 (목차 제목을 줄에서 못 찾으면 0). rect: 큰 글자 줄로 찾았을 때만 */
export type Heading = OutlineItem & { line: number; rect?: Rect };

/** 본문 글자 크기(글자 수로 가중한 중앙값)의 이만큼 이상이면 큰 글자 줄 */
const BIG = 1.3;
/** 큰 글자 줄이 이보다 많으면 제목이 아니라 본문 서식으로 본다 */
const MAX_SHARE = 0.4;

/** 제목 구간의 글: 단계만큼 "#" + 띄어쓰기 + 제목. source.rs heading_text 와 같다 (OT2) */
export const headingText = (level: number, title: string) => `${"#".repeat(Math.min(6, Math.max(1, level)))} ${title.replace(/\s+/g, " ").trim()}`;

const squash = (s: string) => s.replace(/\s+/g, "").toLowerCase();

type Node = { title: string; dest: unknown; items?: Node[] };
type Doc = Pick<PDFDocumentProxy, "getOutline" | "getDestination" | "getPageIndex">;

/** PDF 목차를 쪽 번호로 푼다. 쪽을 모르는 항목(바깥 링크 · 깨진 대상)은 빼고 그 아래 항목은 남긴다 */
export async function outlineOf(doc: Doc): Promise<OutlineItem[]> {
  const out: OutlineItem[] = [];
  const pageOf = async (dest: unknown): Promise<number | null> => {
    const d = typeof dest === "string" ? await doc.getDestination(dest) : dest;
    if (!Array.isArray(d) || d[0] == null) return null;
    return typeof d[0] === "number" ? d[0] + 1 : (await doc.getPageIndex(d[0])) + 1;
  };
  const walk = async (items: Node[] | null | undefined, level: number) => {
    for (const it of items ?? []) {
      const page = await pageOf(it.dest).catch(() => null);
      const title = (it.title ?? "").replace(/\s+/g, " ").trim();
      if (page && title) out.push({ title, page, level: Math.min(level, 6) });
      await walk(it.items, level + 1);
    }
  };
  await walk((await doc.getOutline()) as Node[] | null, 1);
  return out;
}

/** 짧은 쪽이 긴 쪽 안에 들고 그 절반 이상 (본문 문장 속 같은 낱말은 제목 줄이 아니다). source.rs core_reason 과 같은 맞추기 */
const same = (a: string, b: string) => {
  const [x, y] = [squash(a), squash(b)];
  const [short, long] = x.length <= y.length ? [x, y] : [y, x];
  return short.length >= 2 && long.includes(short) && short.length * 2 >= long.length;
};

/** 글자 수로 가중한 글자 크기 중앙값 */
function bodySize(lines: PageLine[]) {
  const w = lines.map((l) => [l.size, squash(l.text).length] as const).filter(([s, c]) => s > 0 && c > 0).sort((a, b) => a[0] - b[0]);
  const total = w.reduce((t, x) => t + x[1], 0);
  let acc = 0;
  for (const [s, c] of w) if ((acc += c) * 2 >= total) return s;
  return 0;
}

const union = (a: Rect, b: Rect): Rect => {
  const [x0, y0] = [Math.min(a[0], b[0]), Math.min(a[1], b[1])];
  return [x0, y0, Math.max(a[0] + a[2], b[0] + b[2]) - x0, Math.max(a[1] + a[3], b[1] + b[3]) - y0];
};

/** 목차가 없는 PDF: 큰 글자 줄. 크기가 큰 순서로 1 · 2 · 3단계, 같은 크기로 이어진 두 줄은 한 제목. 숫자뿐인 줄 · 60자 넘는 줄은 뺀다 */
function bigLines(pages: PageLine[][]): Heading[] {
  const all = pages.flat();
  const body = bodySize(all);
  const big = (l: PageLine) => body > 0 && l.size >= body * BIG && /\p{L}/u.test(l.text) && squash(l.text).length >= 2 && l.text.length <= 60;
  const count = all.filter(big).length;
  if (!count || count > all.length * MAX_SHARE) return [];
  const round = (s: number) => Math.round(s * 2) / 2;
  const sizes = [...new Set(all.filter(big).map((l) => round(l.size)))].sort((a, b) => b - a);
  const out: Heading[] = [];
  pages.forEach((lines, p) => {
    let last = -2;
    lines.forEach((l, i) => {
      if (!big(l)) return;
      const level = Math.min(3, sizes.indexOf(round(l.size)) + 1);
      const prev = out[out.length - 1];
      if (last === i - 1 && prev?.level === level) {
        prev.title = `${prev.title} ${l.text.trim()}`;
        if (prev.rect && l.rect) prev.rect = union(prev.rect, l.rect);
      } else out.push({ title: l.text.trim(), page: p + 1, level, line: i, rect: l.rect });
      last = i;
    });
  });
  return out;
}

/** 제목 찾기 (OT1). PDF 목차가 있으면 그것을 쓰고 제목 줄을 그 쪽에서 찾는다. 없으면 큰 글자 줄 */
export function headingsOf(outline: OutlineItem[], pages: PageLine[][]): Heading[] {
  if (!outline.length) return bigLines(pages);
  return outline
    .filter((o) => o.page >= 1 && o.page <= pages.length)
    .map((o) => ({ ...o, line: Math.max(0, pages[o.page - 1].findIndex((l) => same(l.text, o.title))) }));
}

/** 저장할 제목 구간. 쪽 안 순서는 그 줄 앞에서 시작한 강조 구간 수라 강조 구간의 순서 · id 는 그대로다
 * (같은 자리면 제목이 먼저, source.rs outline). starts: 쪽마다 강조 구간이 시작한 줄 */
export const headingSegments = (headings: Heading[], starts: number[][]): NewSegment[] =>
  headings.map((h) => ({
    page: h.page,
    ord: (starts[h.page - 1] ?? []).filter((x) => x < h.line).length,
    text: headingText(h.level, h.title),
    signals: ["heading"],
    rects: h.rect ? [h.rect] : null,
    before: null,
    after: null,
  }));
