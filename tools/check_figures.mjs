// 8.4 PDF 그림 · 표 찾기 검사 (src/figures.ts). 작은 PDF 를 손으로 만들어 pdf.js(node 판)로 읽고 그림 상자 · 표 제안 · 칸 나누기를 본다.
// 2단 본문 · 글머리표 목록은 표로 제안하지 않아야 한다 (DECISIONS IT2 오탐).
//   node tools/check_figures.mjs            검사만
//   node tools/check_figures.mjs a.pdf ...  덧붙여 그 PDF 의 쪽마다 제안 · 뺀 까닭을 적는다 (측정)
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { CAPTION, captionOf, captionTag, figuresOf, gridIn, layoutOf, lineGapOf, splitsRun, suggestTables, tableCandidates } from "../src/figures.ts";

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

// 7 · 8쪽 머리글 + 4행 괘선 표 (마지막 행 바로 아래가 표 아래 테두리). 7쪽은 칸마다 re S (닫는 변도 괘선),
// 8쪽은 가는 칠한 사각형(re f, 0.6pt)으로 그린 괘선 (Word · PowerPoint 내보내기). 마지막 행까지 5행이어야 한다
const Z = [
  ["Test", "Null hypothesis", "Used for"],
  ["t-test", "Means are equal", "Two groups"],
  ["Chi-square", "Independent", "Categorical"],
  ["F-test", "Variances equal", "ANOVA"],
  ["Z-test", "Mean equals mu0", "Large sample"],
];
const zx = [72, 172, 312, 452];
const zText = Z.map((row, r) => row.map((s, c) => text(zx[c] + 6, 690 - r * 22, s)).join("")).join("");
let page7 = "0.6 w\n";
Z.forEach((_, r) => zx.slice(0, -1).forEach((x, c) => (page7 += `${x} ${684 - r * 22} ${zx[c + 1] - x} 22 re S\n`)));
page7 += zText;
let page8 = "";
for (let r = 0; r <= Z.length; r++) page8 += `72 ${706 - r * 22} 380 0.6 re f\n`;
for (const x of zx) page8 += `${x} ${596} 0.6 110.6 re f\n`;
page8 += zText;

const bytes = pdf([page1, ruled, page3, page4, page5, page6, page7, page8]);

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

// 7 · 8쪽: 마지막 행(Z-test)까지 머리글 + 4행
for (const i of [6, 7]) {
  const s = suggestTables(L[i]);
  assert.equal(s.length, 1, `${i + 1}쪽 괘선 표 제안 1 (${JSON.stringify(tableCandidates(L[i]))})`);
  assert.deepEqual([s[0].rows, s[0].cols, s[0].ruled], [5, 3, true], `${i + 1}쪽 5행 × 3열`);
  assert.deepEqual(gridIn(L[i], s[0].box), Z, `${i + 1}쪽 마지막 행까지`);
}

// 사용자가 잡은 영역: 표 일부만 (2~3행 · 앞 두 열)
assert.deepEqual(gridIn(L[2], [80, 150, 300, 185]), [["R2", "Explained variance"], ["RMSE", "Root mean squared error"]]);
// 글자 없는 영역(그림) → 표로 만들 수 없다
assert.deepEqual(gridIn(L[0], figs[0]), []);
// 이름표 번호 (QA T6-12): 낱말 뒤 숫자 · 영문 한 자+숫자만, 끝 "." · "-" 는 뺀다
for (const [t, tag] of [["Figure 1. Regression", "Figure 1"], ["Fig. 2 Residuals", "Fig. 2"], ["그림 3-6 회귀", "그림 3-6"], ["그림 A-1", "그림 A-1"], ["Figure S1", "Figure S1"], ["표 2- 결과", "표 2"]])
  assert.equal(captionTag(t), tag, t);
for (const t of ["Tablet computers are common", "Figures show the trend", "Table of contents", "표준 편차"]) assert.ok(!CAPTION.test(t), t);
// 강조 구간 묶기 (QA T6-15, lecture.pdf 의 굵은 줄): 기준선 y · 크기로 pdf.ts 의 줄 상자를 만든다
const ln = (text, y, size = 11) => ({ text, size, box: [72, y - size * 0.25, 300, y + size * 0.85] });
assert.ok(splitsRun(ln("Network Lecture 3: TCP and UDP", 740, 20), ln("1. Transport Layer", 712, 14)), "제목 + H1 (크기)");
assert.ok(splitsRun(ln("Table 1. Protocol comparison", 440), ln("Protocol Connection Reliable", 420)), "이름표 + 표 머리글 (간격)");
assert.ok(splitsRun(ln("is the key idea.", 500), ln("Figure 2 shows it", 486)), "다음 줄이 이름표");
assert.ok(splitsRun(ln("Transport Layer", 500), ln("2.1 Fast Retransmit", 486)), "다음 줄이 제목 번호");
assert.ok(!splitsRun(ln("A two-line highlight that wraps", 500), ln("onto the next line of the paragraph.", 486)), "두 줄짜리 하이라이트는 한 구간");
// 줄 간격 (QA G4-02, G4 의 spacing.pdf): 10 · 11pt 에 1.2 · 1.5 · 1.6 · 1.65 · 1.8 · 2.0배로 굵은 두 줄 + 보통 한 줄(16pt 아래), 묶음 사이 40.
// 쪽의 보통 줄 간격과 견주어 두 줄짜리 굵은 글씨는 한 구간
{
  const lines = [];
  const pairs = [];
  let y = 750;
  for (const size of [10, 11])
    for (const k of [1.2, 1.5, 1.6, 1.65, 1.8, 2.0]) {
      pairs.push([`${size}pt ${k}x`, lines.length]);
      lines.push(ln(`[${size}pt ${k}x] first`, y, size), ln("second", y - size * k, size), ln("plain", y - size * k - 16, size));
      y -= size * k + 40;
    }
  const gap = lineGapOf(lines);
  for (const [name, i] of pairs) assert.ok(!splitsRun(lines[i], lines[i + 1], gap), `spacing.pdf ${name} 는 한 구간 (보통 줄 간격 ${gap})`);
}
// 고른 줄 간격 쪽: 두 줄은 한 구간, 빈 줄 하나 띄운 문단은 나눈다. 1.2배 + 문단 뒤 6pt 도 나눈다 (전과 같다)
for (const size of [10, 11])
  for (const k of [1.0, 1.2, 1.5, 1.6, 1.65, 2.0]) {
    const page = Array.from({ length: 12 }, (_, i) => ln(`line ${i}`, 700 - i * size * k, size));
    const gap = lineGapOf(page);
    assert.ok(Math.abs(gap - size * k) < 0.01, `보통 줄 간격 ${size}pt ${k}x`);
    assert.ok(!splitsRun(page[4], page[5], gap), `${size}pt ${k}x 두 줄은 한 구간`);
    assert.ok(splitsRun(page[4], ln("next paragraph", 700 - 4 * size * k - 2 * size * k, size), gap), `${size}pt ${k}x 빈 줄 띈 문단은 나눈다`);
  }
{
  const page = Array.from({ length: 12 }, (_, i) => ln(`line ${i}`, 700 - i * 13.2));
  assert.ok(splitsRun(page[4], ln("next paragraph", 700 - 4 * 13.2 - 19.2), lineGapOf(page)), "1.2배 + 문단 뒤 6pt");
}
// 이름표 다음 표 머리글이 붙어 있어도 나눈다 (QA G4-03)
assert.ok(splitsRun(ln("Table 1. Protocol comparison", 440), ln("Protocol Connection Reliable", 427)), "이름표 + 붙은 표 머리글");
assert.ok(splitsRun(ln("그림 3-4 회귀선", 440), ln("다음 문단의 굵은 글씨", 427)), "이름표 다음 줄");
// 숫자로 시작하는 둘째 줄 (QA G4-04): 제목 번호("1." · "2.1 제목") · 이름표("그림 3 …" · "표 2: …")만 나눈다
for (const t of ["2019년이고 그 뒤로 두 번 고쳐졌다.", "2019 년이고 그 뒤로 두 번 고쳐졌다.", "2 배씩 늘어난다.", "2배씩 늘어난다.", "1500 바이트이다.", "3.5 ms 정도이다.", "그림 3에 정리되어 있다.", "표 2와 같다."])
  assert.ok(!splitsRun(ln("첫 줄은 굵은 글씨", 500), ln(t, 485)), `"${t}" 는 이어진 줄`);
for (const t of ["1. 전송 계층", "2.1 Fast Retransmit", "3.2.1 혼잡 제어", "2.1. 개요", "그림 3 회귀선", "표 2: 결과", "Figure 2 shows it", "Table 1."])
  assert.ok(splitsRun(ln("첫 줄은 굵은 글씨", 500), ln(t, 485)), `"${t}" 에서 나눈다`);
assert.ok(!splitsRun(ln("그림 3에 정리되어 있다.", 500), ln("이어지는 둘째 줄", 485)), "본문 속 \"그림 3에\" 다음 줄은 이어진다");
// 성긴 쪽 (QA V2-02): 보통 줄 간격이 문단 간격이 되어도 문장이 끝난 줄 뒤 1.6배 넘는 간격은 나눈다.
// [기준선, 글, 같은 신호인 앞 줄과 한 구간인가 (없으면 신호가 달라 보지 않는다)]
for (const [name, rows] of [
  // V2 의 lecture-ko.pdf 3쪽: 굵은 줄 + "3.1 …" · 하이라이트 주석 둘 · 굵은 문단 둘 (1.5배 · 1.7배)
  ["lecture-ko p.3", [[712, "재전송 타이머가 끝나기 전에 확인 응답이 오지 않으면"], [697, "3.1 재전송 규칙", false], [670, "송신 측은 보낸 세그먼트마다 순서 번호를 붙이고 수신 측은"], [655, "받은 순서 번호에 대한 확인 응답을 돌려준다.", true], [630, "확인 응답이 세 번 겹쳐 오면 타이머를 기다리지 않고 바로", false], [615, "2019 년 표준에서도 빠른 재전송을 쓴다.", true], [590, "빠른 재전송은 지연을 줄이고, 느린 시작은 손실 뒤 윈도를"], [573.5, "처음부터 다시 키운다.", true], [548, "혼잡 윈도는 손실이 나면 절반으로 줄고, 손실이 없으면", false], [529.3, "다시 천천히 늘어난다.", true]]],
  // V2 의 para.pdf 2쪽: 보통 한 줄 셋 (25 간격) · 굵은 두 줄 문단 둘 · 보통 한 줄 둘
  ["para p.2", [[710, "Plain single line one."], [685, "Plain single line two."], [660, "Plain single line three."], [635, "First bold paragraph starts on this line"], [620, "and ends here.", true], [595, "Second bold paragraph is a different idea", false], [580, "and ends here too.", true], [555, "Plain single line four."], [530, "Plain single line five."]]],
  // 2줄 간격(2.0배) 문서의 두 줄짜리 하이라이트 둘
  ["2.0x", [[700, "A two-line highlight that wraps"], [678, "onto the next line of the paragraph.", true], [656, "Another highlight starts on this line", false], [634, "and ends on the next one.", true]]],
]) {
  const page = [ln("3. 신뢰성 있는 전송", 740, 16), ...rows.map(([y, t]) => ln(t, y))];
  const gap = lineGapOf(page);
  rows.forEach(([, t, same], i) => same === undefined || assert.equal(!splitsRun(page[i], page[i + 1], gap), same, `${name}: "${t}" ${same ? "는 앞 줄과 한 구간" : "에서 나눈다"} (보통 줄 간격 ${gap})`));
}
console.log("check_figures: 그림 · 괘선 표(선 · 칸 사각형 · 칠한 괘선) · 글자 표 · 2단 본문 · 목록 · 북탭 표 모두 통과");

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
