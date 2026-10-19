// 8.4 목차 검사 (src/outline.ts). 작은 PDF 를 손으로 만들어 pdf.js(node 판)로 읽고 목차 풀기 · 큰 글자 제목 · 쪽 안 순서를 본다.
//   node tools/check_outline.mjs
import assert from "node:assert/strict";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { headingSegments, headingsOf, headingText, outlineOf } from "../src/outline.ts";

// ---------------------------------------------------------------- PDF 만들기 (Helvetica). outline(pages) 는 목차 객체를 더한다
function pdf(pages, outline) {
  const objs = [];
  const add = (s) => (objs.push(s), objs.length);
  add(""); // Catalog (아래에서 채운다)
  add(""); // Pages
  const font = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
  const kids = pages.map((content) => {
    const c = add({ dict: "<<", data: content });
    return add(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${c} 0 R >>`);
  });
  objs[1] = `<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`;
  const extra = outline ? outline(kids, add, objs) : "";
  objs[0] = `<< /Type /Catalog /Pages 2 0 R ${extra} >>`;
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
/** 한글 목차 제목: UTF-16BE (BOM) 16진 문자열 */
const u16 = (s) => `<FEFF${[...s].map((c) => c.charCodeAt(0).toString(16).padStart(4, "0")).join("")}>`;
const body = (y0, n) => Array.from({ length: n }, (_, i) => text(72, y0 - i * 14, `body line ${i} with ordinary words in it`)).join("");

const open = (data) => pdfjs.getDocument({ data, useSystemFonts: true }).promise;
/** pdf.ts linesOf 처럼 쪽마다 줄 (여기서는 글자 항목 하나가 한 줄) · 글자 크기 = 변환 행렬의 세로 크기 */
async function pageLines(doc) {
  const out = [];
  for (let p = 1; p <= doc.numPages; p++) {
    const page = await doc.getPage(p);
    const items = (await page.getTextContent()).items.filter((i) => "str" in i && i.str.trim());
    out.push(items.map((i) => ({ text: i.str, size: Math.hypot(i.transform[2], i.transform[3]), rect: [0, 0, 1, 0.02] })));
  }
  return out;
}

// ---------------------------------------------------------------- 1. PDF 목차: 이름 붙은 대상 · 직접 대상 · 아래 단계 · 바깥 링크
{
  const pages = [
    text(72, 780, "Chapter 1 Basics", 18) + body(740, 6),
    text(72, 780, "running header", 8) + body(760, 3) + text(72, 600, "1.1 Detail", 14) + body(580, 3),
    text(72, 780, "Summary", 18) + body(740, 4),
  ];
  const data = pdf(pages, (kids, add, objs) => {
    const root = add("");
    const [a, a1, b, link] = [add(""), add(""), add(""), add("")];
    objs[root - 1] = `<< /Type /Outlines /First ${a} 0 R /Last ${link} 0 R /Count 4 >>`;
    objs[a - 1] = `<< /Title (Chapter 1 Basics) /Parent ${root} 0 R /Next ${b} 0 R /First ${a1} 0 R /Last ${a1} 0 R /Count 1 /Dest [${kids[0]} 0 R /XYZ 0 800 0] >>`;
    objs[a1 - 1] = `<< /Title (1.1 Detail) /Parent ${a} 0 R /Dest [${kids[1]} 0 R /Fit] >>`;
    objs[b - 1] = `<< /Title ${u16("3장 요약")} /Parent ${root} 0 R /Prev ${a} 0 R /Next ${link} 0 R /Dest (sum) >>`;
    objs[link - 1] = `<< /Title (Website) /Parent ${root} 0 R /Prev ${b} 0 R /A << /S /URI /URI (https://example.com) >> >>`;
    return `/Outlines ${root} 0 R /Names << /Dests << /Names [(sum) [${kids[2]} 0 R /XYZ 0 800 0]] >> >>`;
  });
  const doc = await open(data);
  const outline = await outlineOf(doc);
  assert.deepEqual(outline, [
    { title: "Chapter 1 Basics", page: 1, level: 1 },
    { title: "1.1 Detail", page: 2, level: 2 },
    { title: "3장 요약", page: 3, level: 1 },
  ]);
  const lines = await pageLines(doc);
  const heads = headingsOf(outline, lines);
  // 목차 제목을 그 쪽 줄에서 찾는다 (2쪽은 머리글 · 본문 3줄 다음 5째 줄). 못 찾으면 쪽 맨 앞. 목차로 읽은 제목은 위치를 남기지 않는다
  assert.deepEqual(
    heads.map((h) => [h.page, h.line, h.level, h.rect]),
    [
      [1, 0, 1, undefined],
      [2, 4, 2, undefined],
      [3, 0, 1, undefined],
    ],
  );
  // 쪽 안 순서: 그 줄 앞에서 시작한 강조 구간 수. 2쪽 강조가 1 · 6째 줄에서 시작하면 1.1 은 둘 사이 (ord 1)
  const segs = headingSegments(heads, [[0], [1, 6], []]);
  assert.deepEqual(
    segs.map((s) => [s.page, s.ord, s.text, s.signals.join(), s.rects]),
    [
      [1, 0, "# Chapter 1 Basics", "heading", null],
      [2, 1, "## 1.1 Detail", "heading", null],
      [3, 0, "# 3장 요약", "heading", null],
    ],
  );
}

// ---------------------------------------------------------------- 2. 목차 없는 PDF: 큰 글자 줄
{
  const pages = [
    text(72, 780, "Regression Basics", 24) + body(740, 10) + text(72, 560, "Key Points", 16) + body(540, 6) + text(500, 40, "12", 24),
    text(72, 780, "Model", 24) + text(72, 750, "Summary", 24) + body(720, 12),
  ];
  const doc = await open(pdf(pages));
  assert.deepEqual(await outlineOf(doc), []);
  const heads = headingsOf([], await pageLines(doc));
  // 본문 10pt 의 1.3배 이상: 24pt 1단계 · 16pt 2단계. 숫자뿐인 줄(쪽 번호)은 빼고, 같은 크기로 이어진 두 줄은 한 제목
  assert.deepEqual(
    heads.map((h) => [h.page, h.line, h.level, h.title]),
    [
      [1, 0, 1, "Regression Basics"],
      [1, 11, 2, "Key Points"],
      [2, 0, 1, "Model Summary"],
    ],
  );
  // 큰 글자 줄로 찾은 제목은 줄 위치를 남긴다 (출처 표시 "큰 글자 줄로 읽은 제목", source.rs outline_of)
  assert.ok(headingSegments(heads, [[], []]).every((s) => s.rects?.length === 1));
}

// ---------------------------------------------------------------- 3. 순수 규칙
{
  const L = (text, size) => ({ text, size });
  // 큰 글자 줄이 40% 를 넘으면 (포스터 · 표지) 제목으로 보지 않는다
  assert.deepEqual(headingsOf([], [[L("Big one", 30), L("Big two", 30), L("small body text here", 10)]]), []);
  // 본문 문장 속 같은 낱말은 제목 줄이 아니다 (짧은 쪽이 긴 쪽의 절반 이상일 때만)
  const page = [L("logistic regression is used when the response is categorical", 10), L("Regression", 18)];
  assert.equal(headingsOf([{ title: "Regression", page: 1, level: 1 }], [page])[0].line, 1);
  // 쪽을 넘는 목차 항목은 뺀다
  assert.deepEqual(headingsOf([{ title: "x", page: 3, level: 1 }], [page]), []);
  // source.rs heading_text 와 같은 모양
  assert.equal(headingText(2, " 3.2  요약 "), "## 3.2 요약");
  assert.equal(headingText(9, "깊음"), "###### 깊음");
}

console.log("check_outline: ok");
