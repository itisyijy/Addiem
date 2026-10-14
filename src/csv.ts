// CSV 읽기 (B2-c). 화면 파일과 떼어 둔다

/**
 * RFC 4180. 따옴표로 감싼 칸 안의 구분자 · 줄바꿈 · "" 를 지원한다. 따옴표는 칸 맨 앞에서만 특별하다.
 * 빈 줄은 뺀다 (행 번호는 빈 줄을 뺀 뒤 1부터)
 */
export function parseCsv(text: string, delim: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  const end = () => {
    row.push(cell);
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
      row.push(cell);
      cell = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      end();
    } else cell += c;
  }
  if (cell !== "" || row.length > 0) end();
  return rows;
}
