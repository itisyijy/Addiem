//! 8.4 표 → 카드 (DECISIONS IT4 · IT5 · IT6). 표 격자 하나에서 노트 초안을 만든다 (저장하지 않는다).
//! 저장은 source::source_region_cards 가 초안 그대로 한다 (화면이 미리보기로 본 것과 같다).
//!   - cloze: 묻는 열의 칸마다 빈칸 노트 하나 (IT5). 노트마다 표 전체 HTML, 묻는 칸만 {{c1::…}}, "같은 열 가리기"면 그 열 다른 칸은 "…"
//!   - rows: 행마다 기본 카드 한 장. 앞면 "행 이름의 열 이름은?" (열이 여럿이면 "행 이름 — 열1? 열2?"), 뒷면 그 칸들
//!   - list: 묻는 열마다 목록 회상 카드 한 장 (Q, 순서대로). 항목은 그 열 칸을 위에서 아래로
//!
//! 행 이름은 첫 열, 열 이름은 머리글 행. 합친 칸(PPTX gridSpan · rowSpan)은 왼쪽 위 칸이 글자를 갖고 나머지는 merged

use serde::{Deserialize, Serialize};

/// 표 한 칸. merged: 다른 칸에 합쳐진 자리 (PPTX hMerge · vMerge). 글자는 합친 칸의 왼쪽 위에만 있다
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    #[serde(default = "one")]
    pub row_span: usize,
    #[serde(default = "one")]
    pub col_span: usize,
    #[serde(default)]
    pub merged: bool,
}

fn one() -> usize {
    1
}

#[derive(Deserialize)]
pub struct TableSpec {
    pub rows: Vec<Vec<Cell>>,
    /// 첫 행은 머리글 (묻지 않고 열 이름으로 쓴다)
    pub header: bool,
    /// cloze | rows | list
    pub method: String,
    /// 묻는 열 (0부터)
    pub cols: Vec<usize>,
    /// cloze: 같은 열의 다른 칸도 "…"로 가린다 (IT5 기본 켬)
    pub hide_same_col: bool,
    /// 빈칸 노트의 뒷면 추가 설명 ("강의.pptx · 슬라이드 3")
    pub source: String,
}

/// 만들 노트 하나. kind: cloze | basic | list. 필드 HTML 그대로 (list 의 back 은 <br> 로 나눈 항목)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Draft {
    pub kind: String,
    pub front: String,
    pub back: String,
}

/// HTML 글자. 칸 글자의 { } · :: 는 빈칸 문법({{c2::)으로 읽히지 않게 바꾼다
pub(crate) fn esc(s: &str) -> String {
    s.trim()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('{', "&#123;")
        .replace('}', "&#125;")
        .replace("::", ":&#58;")
        .replace('\n', "<br>")
}

/// 받침이 있는가 (I57 nav.tsx jong 과 같은 규칙: 영문 · 숫자는 읽는 소리로)
fn final_consonant(word: &str) -> bool {
    let last = word.trim_end_matches(|c: char| c.is_whitespace() || ")]}\"'.·…-".contains(c)).chars().last();
    match last {
        Some(c @ '가'..='힣') => !(c as u32 - '가' as u32).is_multiple_of(28),
        Some(c) => matches!(c.to_ascii_lowercase(), 'l' | 'r' | '1' | '7' | '8' | 'm' | 'n' | '0' | '3' | '6'),
        None => false,
    }
}

fn eun(word: &str) -> &'static str {
    if final_consonant(word) { "은" } else { "는" }
}

/// 한 줄로 (칸 안 줄바꿈 · 겹친 빈칸)
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Grid<'a> {
    rows: &'a [Vec<Cell>],
    header: bool,
}

impl Grid<'_> {
    fn width(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    fn cell(&self, r: usize, c: usize) -> Option<&Cell> {
        self.rows.get(r)?.get(c)
    }

    /// (r, c) 를 덮는 칸의 왼쪽 위 (합쳐진 자리면 위 · 왼쪽에서 찾는다)
    fn anchor(&self, r: usize, c: usize) -> Option<(usize, usize)> {
        if !self.cell(r, c)?.merged {
            return Some((r, c));
        }
        (0..=r).rev().flat_map(|r0| (0..=c).rev().map(move |c0| (r0, c0))).find(|&(r0, c0)| {
            self.cell(r0, c0).is_some_and(|x| !x.merged && r0 + x.row_span > r && c0 + x.col_span > c)
        })
    }

    fn text(&self, r: usize, c: usize) -> String {
        self.anchor(r, c).and_then(|(r0, c0)| self.cell(r0, c0)).map(|x| flat(&x.text)).unwrap_or_default()
    }

    fn body(&self) -> std::ops::Range<usize> {
        usize::from(self.header)..self.rows.len()
    }

    fn col_name(&self, c: usize) -> String {
        Some(self.text(0, c)).filter(|t| self.header && !t.is_empty()).unwrap_or_else(|| format!("{}열", c + 1))
    }

    /// 행 이름의 열 이름은? (행 이름이 없거나 묻는 칸이 첫 열이면 열 이름만)
    fn question(&self, r: usize, c: usize) -> String {
        let (row, col) = (self.text(r, 0), self.col_name(c));
        if c == 0 || row.is_empty() { format!("{col}{}?", eun(&col)) } else { format!("{row}의 {col}{}?", eun(&col)) }
    }

    /// 표 HTML. ask 칸은 {{c1::…}}, hide 가 참인 칸은 "…"
    fn html(&self, ask: Option<(usize, usize)>, hide: impl Fn(usize, usize) -> bool) -> String {
        let (h, w) = (self.rows.len(), self.width());
        let mut s = String::from("<table>");
        for (r, row) in self.rows.iter().enumerate() {
            s.push_str("<tr>");
            for (c, cell) in row.iter().enumerate().filter(|(_, x)| !x.merged) {
                let tag = if self.header && r == 0 { "th" } else { "td" };
                let mut attrs = String::new();
                // 표 밖으로 넘는 합치기는 잘라 둔다
                for (name, span) in [("rowspan", cell.row_span.min(h - r)), ("colspan", cell.col_span.min(w - c))] {
                    if span > 1 {
                        attrs.push_str(&format!(" {name}=\"{span}\""));
                    }
                }
                let body = if ask == Some((r, c)) {
                    format!("{{{{c1::{}}}}}", esc(&cell.text))
                } else if hide(r, c) {
                    "…".to_string()
                } else {
                    esc(&cell.text)
                };
                s.push_str(&format!("<{tag}{attrs}>{body}</{tag}>"));
            }
            s.push_str("</tr>");
        }
        s.push_str("</table>");
        s
    }
}

/// 미리보기 · 저장에 같이 쓰는 초안. 빈 칸 · 합쳐진 자리는 묻지 않는다
pub fn drafts(t: &TableSpec) -> Vec<Draft> {
    let g = Grid { rows: &t.rows, header: t.header };
    let mut cols: Vec<usize> = t.cols.iter().copied().filter(|&c| c < g.width()).collect();
    cols.sort_unstable();
    cols.dedup();
    let asked = |r: usize, c: usize| g.cell(r, c).filter(|x| !x.merged && !x.text.trim().is_empty());
    let mut out = vec![];
    match t.method.as_str() {
        "cloze" => {
            for r in g.body() {
                for &c in &cols {
                    if asked(r, c).is_none() {
                        continue;
                    }
                    // 같은 열: 이 열을 덮는 본문 칸 (합친 칸 포함)
                    let hide = |r2: usize, c2: usize| {
                        t.hide_same_col && r2 != r && g.body().contains(&r2) && g.cell(r2, c2).is_some_and(|x| c2 <= c && c < c2 + x.col_span)
                    };
                    let front = format!("<div>{}</div>{}", esc(&g.question(r, c)), g.html(Some((r, c)), hide));
                    out.push(Draft { kind: "cloze".into(), front, back: esc(&t.source) });
                }
            }
        }
        "rows" => {
            // 첫 열은 행 이름이라 뒷면에 넣지 않는다
            let cols: Vec<usize> = cols.into_iter().filter(|&c| c > 0).collect();
            for r in g.body() {
                let values: Vec<(String, String)> = cols.iter().map(|&c| (g.col_name(c), g.text(r, c))).collect();
                if cols.is_empty() || values.iter().all(|v| v.1.is_empty()) {
                    continue;
                }
                let (front, back) = if let [(_, v)] = values.as_slice() {
                    (g.question(r, cols[0]), esc(v))
                } else {
                    let row = g.text(r, 0);
                    let asks = values.iter().map(|(col, _)| format!("{col}?")).collect::<Vec<_>>().join(" ");
                    let back = values.iter().map(|(col, v)| format!("{}: {}", esc(col), esc(v))).collect::<Vec<_>>().join("<br>");
                    (if row.is_empty() { asks } else { format!("{row} — {asks}") }, back)
                };
                out.push(Draft { kind: "basic".into(), front: esc(&front), back });
            }
        }
        "list" => {
            for &c in &cols {
                let items: Vec<String> = g.body().filter_map(|r| asked(r, c)).map(|x| esc(&flat(&x.text))).collect();
                // Q4: 항목이 2개 미만이면 목록 카드가 아니다
                if items.len() < 2 {
                    continue;
                }
                let front = format!("{} {}가지를 표 순서대로", g.col_name(c), items.len());
                out.push(Draft { kind: "list".into(), front: esc(&front), back: items.join("<br>") });
            }
        }
        _ => {}
    }
    out
}

/// IT3 미리보기 · 카드 수. 아무것도 저장하지 않는다
#[tauri::command]
pub fn source_table_cards(table: TableSpec) -> Vec<Draft> {
    drafts(&table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(text: &str) -> Cell {
        Cell { text: text.into(), row_span: 1, col_span: 1, merged: false }
    }
    fn m() -> Cell {
        Cell { text: String::new(), row_span: 1, col_span: 1, merged: true }
    }
    fn table(rows: &[&[&str]]) -> Vec<Vec<Cell>> {
        rows.iter().map(|r| r.iter().map(|t| c(t)).collect()).collect()
    }
    fn spec(rows: Vec<Vec<Cell>>, method: &str, cols: &[usize], hide: bool) -> TableSpec {
        TableSpec { rows, header: true, method: method.into(), cols: cols.to_vec(), hide_same_col: hide, source: "a.pptx · 슬라이드 3".into() }
    }

    const HYP: &[&[&str]] = &[&["검정", "귀무가설", "쓰는 곳"], &["t-검정", "두 집단 평균이 같다", "두 집단 비교"], &["카이제곱 검정", "두 변수는 독립이다", "범주형 연관"]];

    #[test]
    fn cloze_per_cell() {
        let d = drafts(&spec(table(HYP), "cloze", &[1], true));
        // 머리글은 묻지 않는다: 본문 2행 × 1열 = 노트 2개, 노트마다 빈칸 하나
        assert_eq!(d.len(), 2);
        assert_eq!(
            d[0].front,
            "<div>t-검정의 귀무가설은?</div><table><tr><th>검정</th><th>귀무가설</th><th>쓰는 곳</th></tr>\
             <tr><td>t-검정</td><td>{{c1::두 집단 평균이 같다}}</td><td>두 집단 비교</td></tr>\
             <tr><td>카이제곱 검정</td><td>…</td><td>범주형 연관</td></tr></table>"
        );
        assert_eq!((d[0].kind.as_str(), d[0].back.as_str()), ("cloze", "a.pptx · 슬라이드 3"));
        assert!(d[1].front.contains("<td>…</td><td>두 집단 비교</td>") && d[1].front.contains("{{c1::두 변수는 독립이다}}"));
        assert_eq!(crate::render::cloze_numbers(&d[1].front).len(), 1);
        // 가리지 않으면 다른 칸은 그대로
        let d = drafts(&spec(table(HYP), "cloze", &[1], false));
        assert!(d[0].front.contains("<td>두 변수는 독립이다</td>"));
        // 머리글이 없으면 첫 행도 묻고, 열 이름은 "N열"
        let mut s = spec(table(HYP), "cloze", &[2], true);
        s.header = false;
        let d = drafts(&s);
        assert_eq!(d.len(), 3);
        assert!(d[0].front.starts_with("<div>검정의 3열은?</div><table><tr><td>검정</td>"));
        // 빈 칸 · 표 밖 열은 묻지 않는다. 칸 글자의 { } 와 < 는 빈칸 · 태그가 되지 않는다
        let mut rows = table(HYP);
        rows[1][1].text = "{{c2::x}} <b>".into();
        rows[2][1].text = " ".into();
        let d = drafts(&spec(rows, "cloze", &[1, 9], true));
        assert_eq!(d.len(), 1);
        assert!(d[0].front.contains("{{c1::&#123;&#123;c2:&#58;x&#125;&#125; &lt;b&gt;}}"));
        assert_eq!(crate::render::cloze_numbers(&d[0].front).into_iter().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn cloze_merged_cells() {
        // 머리글 "구분"이 두 열을 합치고, 본문 첫 열 "회귀"가 두 행을 합친다
        let mut rows = table(&[&["구분", "", "설명"], &["회귀", "선형", "직선"], &["", "로지스틱", "확률"]]);
        rows[0][0].col_span = 2;
        rows[0][1] = m();
        rows[1][0].row_span = 2;
        rows[2][0] = m();
        let d = drafts(&spec(rows.clone(), "cloze", &[0, 1, 2], true));
        // 합쳐진 자리(2행 1열)는 묻지 않는다: 회귀 · 선형 · 직선 · 로지스틱 · 확률
        assert_eq!(d.len(), 5);
        assert!(d[0].front.starts_with("<div>구분은?</div><table><tr><th colspan=\"2\">구분</th><th>설명</th></tr><tr><td rowspan=\"2\">{{c1::회귀}}</td>"));
        // 둘째 열 이름은 합친 머리글 "구분", 행 이름은 위에서 합친 "회귀"
        assert!(d[3].front.starts_with("<div>회귀의 구분은?</div>"), "{}", d[3].front);
        assert!(d[3].front.contains("<td>{{c1::로지스틱}}</td>"));
        // 같은 열 가리기: "로지스틱"을 물을 때 같은 열의 "선형"은 가리고, 0열을 덮는 "회귀"는 그대로
        assert!(d[3].front.contains("<tr><td rowspan=\"2\">회귀</td><td>…</td><td>직선</td></tr>"));
        // 표 밖으로 넘는 합치기는 자른다
        rows[1][0].row_span = 5;
        assert!(drafts(&spec(rows, "cloze", &[2], false))[0].front.contains("<td rowspan=\"2\">회귀</td>"));
    }

    #[test]
    fn row_and_list_cards() {
        let d = drafts(&spec(table(HYP), "rows", &[1], true));
        assert_eq!(
            d,
            vec![
                Draft { kind: "basic".into(), front: "t-검정의 귀무가설은?".into(), back: "두 집단 평균이 같다".into() },
                Draft { kind: "basic".into(), front: "카이제곱 검정의 귀무가설은?".into(), back: "두 변수는 독립이다".into() },
            ]
        );
        // 열이 여럿이면 "행 이름 — 열1? 열2?". 첫 열(행 이름)은 뒷면에 넣지 않는다. 받침 있는 열 이름 → 은
        let d = drafts(&spec(table(HYP), "rows", &[0, 2, 1], true));
        assert_eq!(d[0].front, "t-검정 — 귀무가설? 쓰는 곳?");
        assert_eq!(d[0].back, "귀무가설: 두 집단 평균이 같다<br>쓰는 곳: 두 집단 비교");
        assert_eq!(drafts(&spec(table(&[&["이름", "p-value"], &["A", "0.05"]]), "rows", &[1], true))[0].front, "A의 p-value는?");
        assert_eq!(drafts(&spec(table(&[&["이름", "Level"], &["A", "3"]]), "rows", &[1], true))[0].front, "A의 Level은?");
        // 목록: 열마다 한 장, 위에서 아래로. 항목이 2개 미만인 열은 만들지 않는다
        let d = drafts(&spec(table(HYP), "list", &[0], true));
        assert_eq!(d, vec![Draft { kind: "list".into(), front: "검정 2가지를 표 순서대로".into(), back: "t-검정<br>카이제곱 검정".into() }]);
        assert!(drafts(&spec(table(&[&["a"], &["b"]]), "list", &[0], true)).is_empty());
    }
}
