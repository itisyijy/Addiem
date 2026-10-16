// CSV 읽기 (B2-c). 화면 파일과 떼어 둔다

/**
 * RFC 4180. 따옴표로 감싼 칸 안의 구분자 · 줄바꿈 · "" 를 지원한다. 따옴표는 칸 맨 앞에서만 특별하다.
 * 빈 줄은 뺀다 (행 번호는 빈 줄을 뺀 뒤 1부터).
 * CSV 내보내기가 수식 실행을 막으려고 = + - @ 탭 CR 앞에 붙인 ' 는 뗀다 (export.rs csv_text)
 */
export function parseCsv(text: string, delim: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  const push = () => row.push(cell.replace(/^'(?=[=+\-@\t\r])/, ""));
  const end = () => {
    push();
    if (row.some((c) => c.trim() !== "")) rows.push(row);
    row = [];
    cell = "";
  };
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c !== '"') cell += c;
      else if (text[i + 1] === '"') {
        cell += '"';
        i++;
      } else quoted = false;
    } else if (c === '"' && cell === "") quoted = true;
    else if (c === delim) {
      push();
      cell = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      end();
    } else cell += c;
  }
  if (cell !== "" || row.length > 0) end();
  return rows;
}
