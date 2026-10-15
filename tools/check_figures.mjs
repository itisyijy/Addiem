// 8.4 PDF 그림 · 표 찾기 검사 (src/figures.ts). 작은 PDF 를 손으로 만들어 pdf.js(node 판)로 읽고 그림 상자 · 표 제안 · 칸 나누기를 본다.
// 2단 본문 · 글머리표 목록은 표로 제안하지 않아야 한다 (DECISIONS IT2 오탐).
//   node tools/check_figures.mjs            검사만
//   node tools/check_figures.mjs a.pdf ...  덧붙여 그 PDF 의 쪽마다 제안 · 뺀 까닭을 적는다 (측정)
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { captionOf, figuresOf, gridIn, layoutOf, suggestTables, tableCandidates } from "../src/figures.ts";

// ---------------------------------------------------------------- PDF 만들기 (Helvetica · 2×2 RGB 그림 하나)
function pdf(pages) {
  const objs = [];
  const add = (s) => (objs.push(s), objs.length);
  add("<< /Type /Catalog /Pages 2 0 R >>");
  add(""); // Pages (아래에서 채운다)
  const font = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
  const img = add({ dict: "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8", data: "\xff\x00\x00\x00\xff\x00\x00\x00\xff\xff\xff\x00" });
  // 양식 XObject 안 그림 (행렬 따라가기)
  const form = add({ dict: `<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Matrix [200 0 0 120 0 0] /Resources << /XObject << /Im1 ${img} 0 R >> >>`, data: "q 1 0 0 1 0 0 cm /Im1 Do Q" });
  const kids = pages.map((content) => {
    const c = add({ dict: "<<", data: content });
    return add(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 ${font} 0 R >> /XObject << /Im1 ${img} 0 R /Fm1 ${form} 0 R >> >> /Contents ${c} 0 R >>`);
  });
  objs[1] = `<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`;
  let out = "%PDF-1.4\n";
  const offs = [];
  objs.forEach((o, i) => {
    offs.push(out.length);
    out += `${i + 1} 0 obj\n`;
    out += typeof o === "string" ? o : `${o.dict} /Length ${o.data.length} >>\nstream\n${o.data}\nendstream`;
    out += "\nendobj\n";
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offs.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Uint8Array.from(out, (ch) => ch.charCodeAt(0));
}
const esc = (s) => s.replace(/[()\\]/g, "\\$&");
const text = (x, y, s, size = 10) => `BT /F1 ${size} Tf ${x} ${y} Td (${esc(s)}) Tj ET\n`;
const rtext = (xRight, y, s, size = 10) => text(xRight - s.length * size * 0.55, y, s, size); // 오른쪽 정렬 어림

const T = [
  ["Test", "Null hypothesis", "Used for"],
  ["t-test", "Means are equal", "Two groups"],
  ["Chi-square", "Variables independent", "Categorical"],
  ["F-test", "Variances equal", "ANOVA"],
];
// 1쪽 그림: 큰 그림 1 (양식 안, 300 × 180pt) + 로고(작음) + 쪽 전체 배경 → 그림 1개, 이름표
const page1 =
  "q 595 0 0 842 0 0 cm /Im1 Do Q\n" + // 배경 (85% 초과)
  "q 20 0 0 20 540 800 cm /Im1 Do Q\n" + // 로고 (2% 미만)
  "q 1.5 0 0 1.5 100 450 cm /Fm1 Do Q\n" + // 양식: 300 × 180pt 그림
  text(100, 432, "Figure 3-4 Regression and residuals", 9) +
  text(72, 760, "Chapter 3. Regression", 16);
// 2쪽 괘선 표 (전체를 0.5 배로 줄이는 cm 안에서 좌표 두 배): 4행 × 3열
const cellW = [120, 180, 160];
let ruled = "q 0.5 0 0 0.5 0 0 cm 1 w\n";
{
  const x0 = 144, top = 1400, rh = 48;
  const xs = [x0];
  cellW.forEach((w) => xs.push(xs[xs.length - 1] + w * 2));
  for (let r = 0; r <= T.length; r++) ruled += `${x0} ${top - r * rh} m ${xs[xs.length - 1]} ${top - r * rh} l S\n`;
  for (const x of xs) ruled += `${x} ${top} m ${x} ${top - T.length * rh} l S\n`;
  ruled += "Q\n";
  T.forEach((row, r) => row.forEach((s, c) => (ruled += text(xs[c] / 2 + 6, (top - r * rh - 32) / 2, s))));
}
// 3쪽 괘선 없는 표: 5행 × 3열, 셋째 열은 오른쪽 정렬 숫자
const U = [
  ["Metric", "Meaning", "Value"],
  ["R2", "Explained variance", "0.82"],
  ["RMSE", "Root mean squared error", "12.5"],
  ["MAE", "Mean absolute error", "9.1"],
  ["AIC", "Information criterion", "1024.3"],
];
const page3 = text(72, 760, "Model diagnostics", 14) + U.map((row, r) => text(90, 700 - r * 18, row[0]) + text(190, 700 - r * 18, row[1]) + rtext(460, 700 - r * 18, row[2])).join("");
// 4쪽 2단 본문: 줄마다 왼쪽 단 · 오른쪽 단 (기준선이 같다, 오탐 측정)
const words = "the regression model explains variance in the response using predictors and residual analysis checks the fit".split(" ");
const sentence = (i) => Array.from({ length: 6 }, (_, k) => words[(i * 3 + k) % words.length]).join(" ");
const page4 = Array.from({ length: 24 }, (_, i) => text(56, 780 - i * 14, sentence(i)) + text(316, 780 - i * 14, sentence(i + 7))).join("");
// 5쪽 글머리표 목록 (글머리표는 따로 그린다)
const page5 = Array.from({ length: 6 }, (_, i) => text(80, 700 - i * 18, "-") + text(100, 700 - i * 18, `Point ${i + 1}: ${sentence(i)}`)).join("");
// 6쪽 북탭 표: 가로 줄 3개만 (위 · 머리글 아래 · 아래)
const B = [
  ["Model", "Accuracy", "F1"],
  ["Logistic", "0.81", "0.78"],
  ["Tree", "0.77", "0.74"],
  ["Forest", "0.86", "0.83"],
];
const page6 =
  "0.8 w 80 712 m 400 712 l S 80 694 m 400 694 l S 80 630 m 400 630 l S\n" + B.map((row, r) => row.map((s, c) => text(90 + c * 110, 700 - r * 18 - (r ? 4 : 0), s)).join("")).join("");

const bytes = pdf([page1, ruled, page3, page4, page5, page6]);

async function layouts(data) {
  const task = pdfjs.getDocument({ data, useSystemFonts: false, verbosity: 0 });
  const doc = await task.promise;
  const out = [];
  for (let p = 1; p <= doc.numPages; p++) {
    const page = await doc.getPage(p);
    const vp = page.getViewport({ scale: 1 });
    const [ops, tc] = await Promise.all([page.getOperatorList(), page.getTextContent()]);
    out.push(layoutOf(ops, pdfjs.OPS, tc.items.filter((i) => "str" in i), vp));
    page.cleanup();
  }
  await task.destroy();
  return out;
}

const L = await layouts(bytes);

// 1쪽: 배경 · 로고는 빼고 양식 안 그림 하나, 아래 이름표
const figs = figuresOf(L[0]);
assert.equal(figs.length, 1, `그림 1개 (${JSON.stringify(figs)})`);
assert.deepEqual(figs[0].map(Math.round), [100, 212, 400, 392], "양식 · cm 를 따라간 자리");
assert.equal(captionOf(L[0], figs[0]), "Figure 3-4 Regression and residuals");
assert.equal(suggestTables(L[0]).length, 0, "그림 쪽에는 표 제안 없음");

// 2쪽: 괘선 표 → 제안 하나, 칸 그대로
const s2 = suggestTables(L[1]);
assert.equal(s2.length, 1, `괘선 표 제안 1 (${JSON.stringify(tableCandidates(L[1]))})`);
assert.deepEqual([s2[0].rows, s2[0].cols, s2[0].ruled], [4, 3, true]);
assert.deepEqual(gridIn(L[1], s2[0].box), T, "괘선으로 나눈 칸");

// 3쪽: 괘선 없는 표 → 5 × 3, 오른쪽 정렬 숫자 열도 한 열
const s3 = suggestTables(L[2]);
assert.equal(s3.length, 1, `글자 표 제안 1 (${JSON.stringify(tableCandidates(L[2]))})`);
assert.deepEqual([s3[0].rows, s3[0].cols, s3[0].ruled], [5, 3, false]);
assert.deepEqual(gridIn(L[2], s3[0].box), U, "글자 위치로 나눈 칸");

// 4쪽: 2단 본문 → 후보는 24행 × 2열로 잡히지만 제안하지 않는다
const c4 = tableCandidates(L[3]);
assert.equal(suggestTables(L[3]).length, 0, `2단 본문은 제안 없음 (${JSON.stringify(c4)})`);
assert.ok(c4.some((c) => c.rows >= 10 && c.cols === 2 && c.reason === "모든 열이 긴 글 (2단 본문)"), "규칙이 없었다면 오탐이었다");

// 5쪽: 글머리표 목록 → 제안 없음
assert.equal(suggestTables(L[4]).length, 0, `목록은 제안 없음 (${JSON.stringify(tableCandidates(L[4]))})`);

// 6쪽: 북탭 표 (가로 줄만) → 괘선 있음 · 4 × 3
const s6 = suggestTables(L[5]);
assert.equal(s6.length, 1, `북탭 표 제안 1 (${JSON.stringify(tableCandidates(L[5]))})`);
assert.deepEqual([s6[0].rows, s6[0].cols, s6[0].ruled], [4, 3, true]);
assert.deepEqual(gridIn(L[5], s6[0].box), B);

// 사용자가 잡은 영역: 표 일부만 (2~3행 · 앞 두 열)
assert.deepEqual(gridIn(L[2], [80, 150, 300, 185]), [["R2", "Explained variance"], ["RMSE", "Root mean squared error"]]);
// 글자 없는 영역(그림) → 표로 만들 수 없다
assert.deepEqual(gridIn(L[0], figs[0]), []);
console.log("check_figures: 그림 · 괘선 표 · 글자 표 · 2단 본문 · 목록 · 북탭 표 모두 통과");

// ---------------------------------------------------------------- 측정: 덧붙인 PDF
for (const file of process.argv.slice(2)) {
  const ls = await layouts(new Uint8Array(readFileSync(file)));
  console.log(`\n${file} · ${ls.length}쪽`);
  ls.forEach((l, i) => {
    const cands = tableCandidates(l);
    const shown = cands.filter((c) => !c.reason);
    console.log(
      `  p.${i + 1}: 그림 ${figuresOf(l).length} · 표 제안 ${shown.length}${shown.map((c) => ` (${c.rows}×${c.cols}${c.ruled ? " 괘선" : ""})`).join("")}` +
        cands.filter((c) => c.reason).map((c) => ` · 뺌 ${c.rows}×${c.cols}: ${c.reason}`).join(""),
    );
  });
}
