// 수식 렌더링 (Figma C3-m 43:4963 · D3-m 43:5198). 문법은 Anki 와 같다: \( … \) 인라인, \[ … \] 블록.
// 옛 Anki 태그 [$]…[/$] 인라인, [$$]…[/$$] 블록, [latex]…[/latex] (수식만 들었으면 블록). $ 하나는 수식으로 보지 않는다 (가격 표기).
//
// 보안: 카드 HTML 을 살균한 "뒤" 텍스트 노드에서만 구분자를 찾는다. 덱 HTML 이 KaTeX 결과물을 흉내 내지 못하고,
// KaTeX 에는 글자만 들어간다. trust: false 로 \href · \url · \includegraphics 를 끄고 크기·매크로 전개를 제한한다
import katex from "katex";
import "katex/dist/katex.min.css";
import "./styles/math.css";

const OPTIONS = { throwOnError: true, trust: false, strict: "ignore", maxSize: 20, maxExpand: 200, output: "html" } as const;

/** 한 번에 찾는 구분자들. 그룹 순서: \( \) · \[ \] · [$] · [$$] · [latex] */
const MATH = /\\\(([\s\S]+?)\\\)|\\\[([\s\S]+?)\\\]|\[\$\]([\s\S]+?)\[\/\$\]|\[\$\$\]([\s\S]+?)\[\/\$\$\]|\[latex\]([\s\S]+?)\[\/latex\]/g;

/** 수식이 있을 수도 있는가 (빠른 거르기). MATH 는 g 플래그라 test 로 쓰면 lastIndex 가 남아 matchAll 이 첫 수식을 건너뛴다 */
const HINT = /\\\(|\\\[|\[\$|\[latex\]/;

/** 수식이 아니라 LaTeX 문서 전체 ([latex] 안 \documentclass 등). 렌더링하지 않고 원문을 둔다 */
const isDocument = (tex: string) => /\\documentclass|\\begin\{document\}|\\usepackage/.test(tex);

export type MathPart = { source: string; tex: string; block: boolean; start: number; end: number };

/** 글에서 수식 구간을 찾는다 (편집기의 칩 표시와 오류 줄 번호용). 문서 전체 [latex] 는 빠진다 */
export function findMath(text: string): MathPart[] {
  const out: MathPart[] = [];
  for (const m of text.matchAll(MATH)) {
    const [source, inl, blk, old, oldBlk, latex] = m;
    const tex = inl ?? blk ?? old ?? oldBlk ?? latex;
    if (latex !== undefined && isDocument(latex)) continue;
    out.push({ source, tex, block: inl === undefined && old === undefined, start: m.index, end: m.index + source.length });
  }
  return out;
}

/** KaTeX 오류 문구를 짧은 한국어로. 원래 문구는 화면의 title 로 남긴다 */
function explain(message: string) {
  if (/Expected '\}'|Missing '\}'|Unexpected end of input|got 'EOF'/.test(message)) return "{ 가 닫히지 않았어요";
  const undef = message.match(/Undefined control sequence: (\\\S+)/);
  if (undef) return `${undef[1]} 는 모르는 명령이에요`;
  if (/Double (superscript|subscript)/.test(message)) return "^ 나 _ 가 겹쳤어요";
  if (/Too many expansions|maxExpand/.test(message)) return "너무 긴 수식이에요";
  return "수식 문법이 맞지 않아요";
}

/** 수식 하나를 HTML 로. 실패하면 null 과 이유 */
function render(tex: string, block: boolean): { html: string } | { error: string; raw: string } {
  try {
    return { html: katex.renderToString(tex, { ...OPTIONS, displayMode: block }) };
  } catch (e) {
    const raw = e instanceof Error ? e.message : String(e);
    return { error: explain(raw), raw };
  }
}

const SKIP = new Set(["CODE", "PRE", "SCRIPT", "STYLE", "TEXTAREA"]);

/**
 * 살균한 HTML 이 들어 있는 요소 안의 수식을 그린다. 렌더링한 수 와 읽지 못한 수식 목록을 돌려준다.
 * 블록 수식은 줄바꿈하지 않고 그 수식만 가로로 스크롤한다 (.math--block)
 */
export function renderMathIn(root: HTMLElement): { rendered: number; errors: { source: string; message: string }[] } {
  const nodes: Text[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => {
      for (let p = n.parentElement; p && p !== root; p = p.parentElement) {
        if (SKIP.has(p.tagName) || p.classList.contains("katex")) return NodeFilter.FILTER_REJECT;
      }
      return HINT.test(n.textContent ?? "") ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
    },
  });
  for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n as Text);

  let rendered = 0;
  const errors: { source: string; message: string }[] = [];
  for (const node of nodes) {
    const text = node.textContent ?? "";
    const frag = document.createDocumentFragment();
    let at = 0;
    for (const part of findMath(text)) {
      frag.append(text.slice(at, part.start));
      const r = render(part.tex, part.block);
      const el = document.createElement(part.block ? "div" : "span");
      if ("html" in r) {
        el.className = part.block ? "math math--block" : "math";
        el.innerHTML = r.html; // KaTeX 가 글자에서 만든 것만 들어간다 (trust: false)
        rendered++;
      } else {
        // 원문을 빨간 고정폭 글자로 (C3-m 과 같은 모양). 학습할 때도 원문 그대로
        el.className = "math-error";
        el.textContent = part.source;
        el.title = r.raw;
        errors.push({ source: part.source, message: r.error });
      }
      frag.append(el);
      at = part.end;
    }
    frag.append(text.slice(at));
    node.replaceWith(frag);
  }
  return { rendered, errors };
}

/** 살균한 HTML 문자열에 수식을 그려 돌려준다 (React 의 dangerouslySetInnerHTML 용) */
export function withMath(html: string): string {
  if (!HINT.test(html)) return html;
  const box = document.createElement("div");
  box.innerHTML = html;
  renderMathIn(box);
  return box.innerHTML;
}

/**
 * 필드 HTML 안의 읽지 못한 수식 (C3-m 미리보기 아래 안내 "답변 3번째 줄 · { 가 닫히지 않았어요").
 * 줄은 편집기에서 보이는 줄: <br> 과 블록 요소마다 한 줄
 */
export function checkMath(html: string): { source: string; message: string; line: number }[] {
  if (!HINT.test(html)) return [];
  // Chromium 편집기는 첫 줄을 감싸지 않고 둘째 줄부터 <div>…</div> 로 감싼다. 여는 블록 태그마다 줄을 나누고,
  // 블록 끝의 자리표시 <br>(빈 줄의 <div><br></div>)은 줄로 세지 않는다
  const lines = html
    .replace(/<br\s*\/?>\s*(?=<\/(div|p|li|h\d)>)/gi, "")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<(div|p|li|h\d)\b[^>]*>/gi, "\n")
    .replace(/^\n/, "");
  const box = document.createElement("div");
  box.innerHTML = lines;
  const text = box.textContent ?? "";
  return findMath(text).flatMap((part) => {
    const r = render(part.tex, part.block);
    return "html" in r ? [] : [{ source: part.source, message: r.error, line: text.slice(0, part.start).split("\n").length }];
  });
}
