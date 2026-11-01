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

/**
 * "하위 덱" 열의 최상위 덱 (T4-15). 비지 않은 칸이 모두 같은 첫 이름으로 시작하면 CSV 내보내기처럼 최상위 덱까지 든 전체 경로로 보고
 * 그 이름을 최상위로 쓴다 (full). 아니면 칸은 파일 이름 아래 경로다 (Day 12 → 토익 영단어::Day 12)
 */
export function csvDeckRoot(cells: string[], fileName: string): { root: string; full: boolean } {
  const heads = new Set(cells.filter(Boolean).map((c) => c.split("::")[0].trim()));
  const [only] = heads;
  return heads.size === 1 && only ? { root: only, full: true } : { root: fileName.replace(/\.[^.]+$/, ""), full: false };
}
