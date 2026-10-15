//! 그림 가리기 (MVP 8.1 · Figma Row M). Anki 23.10+ 내장 "Image Occlusion" 노트를 형식 그대로 쓴다.
//! 근거: ankitects/anki 26.09.3 소스와, 같은 버전 Anki(26.9.2)가 실제로 만든 .apkg (DECISIONS M9).
//!
//! 필드 (rslib/src/image_occlusion/notetype.rs): Occlusion · Image · Header · Back Extra · Comments.
//! Anki 도 필드 tag 가 없으면 이 순서로 찾는다 (imagedata.rs get_io_field_indexes).
//!
//! 칸 하나 = 클로즈 하나 (ts/routes/image-occlusion/shapes/to-cloze.ts):
//!   {{c3::image-occlusion:rect:left=.1:top=.2:width=.25:height=.1:oi=1}}<br>
//!   - 좌표는 그림 크기 대비 비율(0–1). 소수 4자리에서 앞뒤 0 을 뗀다 (shapes/lib.ts floatToDisplay)
//!   - cN 의 N 이 카드 번호이자 칸의 안정된 id. 같은 N 의 칸은 묶음(카드 한 장). c0 은 늘 보이는 글자 칸
//!   - oi=1 이면 묻지 않는 칸도 덮는다 (모두 가리고 하나씩). 없으면 묻는 칸만 (하나만 가리기)
//!   - label= 은 우리 칸 이름. Anki 는 속성을 보존하고(imageocclusion.rs parse_image_cloze)
//!     그릴 때는 모르는 속성을 버린다(get_image_cloze_data 의 `_ => {}`)
//!
//! 학습 화면은 캔버스·스크립트 없이 % 로 위치한 div 를 그림 위에 겹친다. 창 크기가 바뀌어도 CSS 만으로 맞는다

use crate::commands::{self, AppState};
use crate::db::now_ms;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 앱이 만드는 노트 유형 이름 (Anki 영문 기본 유형과 같다)
pub const NATIVE: &str = "Image Occlusion";
pub const FIELDS: &str = r#"["Occlusion","Image","Header","Back Extra","Comments"]"#;
/// Anki 26.9.2 가 만든 템플릿 그대로. 내보내면 Anki 의 IO 화면이 그대로 그린다
pub const QFMT: &str = "{{#Header}}<div>{{Header}}</div>{{/Header}}\n<div style=\"display: none\">{{cloze:Occlusion}}</div>\n<div id=\"err\"></div>\n<div id=\"image-occlusion-container\">\n    {{Image}}\n    <canvas id=\"image-occlusion-canvas\"></canvas>\n</div>\n<script>\ntry {\n    anki.imageOcclusion.setup();\n} catch (exc) {\n    document.getElementById(\"err\").innerHTML = `Error loading image occlusion. Is your Anki version up to date?<br><br>${exc}`;\n}\n</script>\n";
pub const AFMT_TAIL: &str = "\n<div><button id=\"toggle\">Toggle Masks</button></div>\n{{#Back Extra}}<div>{{Back Extra}}</div>{{/Back Extra}}\n";
pub const CSS: &str = "#image-occlusion-canvas {\n    --inactive-shape-color: #ffeba2;\n    --active-shape-color: #ff8e8e;\n    --inactive-shape-border: 1px #212121;\n    --active-shape-border: 1px #212121;\n    --highlight-shape-color: #ff8e8e00;\n    --highlight-shape-border: 1px #ff8e8e;\n}\n\n.card {\n    font-family: arial;\n    font-size: 20px;\n    text-align: center;\n    color: black;\n    background-color: white;\n}\n";
/// Anki NotetypeConfig.original_stock_kind (proto/anki/notetypes.proto ORIGINAL_STOCK_KIND_IMAGE_OCCLUSION)
pub const STOCK_KIND: i64 = 6;

/// 기본 노트 유형으로 넣는다 (db::seed_note_types). CSS 는 비워 앱 테마(다크 포함)를 따르고, 내보낼 때 Anki CSS 를 채운다
pub(crate) fn seed(con: &Connection) -> rusqlite::Result<()> {
    let exists: bool =
        con.query_row("SELECT EXISTS (SELECT 1 FROM note_type WHERE name = ?1 AND anki_model_id IS NULL)", [NATIVE], |r| r.get(0))?;
    if exists {
        return Ok(());
    }
    let now = now_ms();
    con.execute("INSERT INTO note_type (name, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", params![NATIVE, FIELDS, now])?;
    con.execute(
        "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, 0, ?2, ?3, ?4)",
        params![con.last_insert_rowid(), NATIVE, QFMT, format!("{QFMT}{AFMT_TAIL}")],
    )?;
    Ok(())
}

/// IO 템플릿인가. 컨테이너 id 는 Anki 가 번역하지 않는다 (가져온 한국어 Anki 노트도 같다)
pub fn is_template(qfmt: &str) -> bool {
    qfmt.contains("image-occlusion-container")
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Shape {
    /// cN. 카드 번호 = 칸의 안정된 id. 0 은 Anki 의 글자 칸 (카드가 없다)
    pub ordinal: u32,
    /// rect | ellipse | polygon | text (모르는 모양도 그대로 보존한다)
    pub kind: String,
    /// 외접 사각형 (그림 대비 비율). 타원은 2rx × 2ry
    pub left: f64,
    pub top: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    /// 다각형 꼭짓점 (비율)
    #[serde(default)]
    pub points: Vec<[f64; 2]>,
    /// 칸 이름 (M2). Anki 화면에는 보이지 않는다
    #[serde(default)]
    pub label: String,
    /// angle · fill · text · scale · fs 처럼 우리가 고치지 않는 속성. 순서대로 다시 쓴다
    #[serde(default)]
    pub extra: Vec<(String, String)>,
}

impl Shape {
    fn prop(&self, name: &str) -> Option<&str> {
        self.extra.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Occlusion 필드 -> 칸들, 모두 가리기(oi=1 인 칸이 하나라도 있으면)
pub fn parse(field: &str) -> (Vec<Shape>, bool) {
    let mut shapes = vec![];
    let mut hide_all = false;
    let mut rest = field;
    while let Some(i) = rest.find("{{c") {
        let after = &rest[i + 3..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let body = after[digits..].strip_prefix("::image-occlusion:").filter(|_| digits > 0);
        match body.and_then(|b| b.find("}}").map(|end| (b, end))) {
            Some((b, end)) => {
                if let Some((shape, oi)) = parse_shape(after[..digits].parse().unwrap_or(0), &b[..end]) {
                    hide_all |= oi;
                    shapes.push(shape);
                }
                rest = &b[end + 2..];
            }
            None => rest = after,
        }
    }
    (shapes, hide_all)
}

/// "rect:left=.1:top=.2:..." (클로즈 안의 image-occlusion: 다음). 값 안의 \ 와 : 는 \ 로 이스케이프돼 있다
fn props(body: &str) -> Option<(&str, Vec<(String, String)>)> {
    let (kind, rest) = body.split_once(':')?;
    let mut out = vec![];
    let mut chars = rest.chars().peekable();
    while chars.peek().is_some() {
        let name: String = chars.by_ref().take_while(|c| *c != '=').collect();
        let mut value = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => value.extend(chars.next()),
                ':' => break,
                c => value.push(c),
            }
        }
        out.push((name, value));
    }
    Some((kind, out))
}

fn parse_shape(ordinal: u32, body: &str) -> Option<(Shape, bool)> {
    let (kind, props) = props(body)?;
    let get = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let num = |k: &str| get(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
    let geometry: &[&str] = match kind {
        "rect" => &["width", "height"],
        "ellipse" => &["rx", "ry"],
        "polygon" => &["points"],
        _ => &[],
    };
    let mut s = Shape {
        ordinal,
        kind: kind.into(),
        left: num("left"),
        top: num("top"),
        width: 0.0,
        height: 0.0,
        points: vec![],
        label: get("label").unwrap_or_default().into(),
        extra: props.iter().filter(|(k, _)| !["left", "top", "oi", "label"].contains(&k.as_str()) && !geometry.contains(&k.as_str())).cloned().collect(),
    };
    match kind {
        "rect" => (s.width, s.height) = (num("width"), num("height")),
        "ellipse" => (s.width, s.height) = (num("rx") * 2.0, num("ry") * 2.0),
        "polygon" => {
            let pts: Vec<[f64; 2]> = get("points")
                .unwrap_or_default()
                .split(' ')
                .filter_map(|p| p.split_once(','))
                .filter_map(|(x, y)| Some([x.parse::<f64>().ok()?, y.parse::<f64>().ok()?]))
                .filter(|[x, y]| x.is_finite() && y.is_finite())
                .collect();
            if !pts.is_empty() {
                // Anki 는 꼭짓점을 left/top 으로 옮겨 그린다 (review.ts getPolygonOffset). 옮긴 위치로 저장한다
                let (min_x, min_y) = pts.iter().fold((f64::MAX, f64::MAX), |(a, b), [x, y]| (a.min(*x), b.min(*y)));
                let (dx, dy) = (s.left - min_x, s.top - min_y);
                s.points = pts.iter().map(|[x, y]| [x + dx, y + dy]).collect();
                let (max_x, max_y) = s.points.iter().fold((f64::MIN, f64::MIN), |(a, b), [x, y]| (a.max(*x), b.max(*y)));
                (s.width, s.height) = (max_x - s.left, max_y - s.top);
            }
        }
        _ => {}
    }
    Some((s, get("oi") == Some("1")))
}

/// Anki floatToDisplay: 소수 4자리에서 앞의 0 과 뒤의 0 을 뗀다 (.25, 1.). 0 은 ".0000"
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0');
    let s = s.strip_prefix('0').unwrap_or(s);
    if !v.is_finite() || s == "." || s == "-." || s == "-0." { ".0000".into() } else { s.into() }
}

fn push(out: &mut String, key: &str, value: &str) {
    out.push(':');
    out.push_str(key);
    out.push('=');
    out.push_str(&value.replace('\\', "\\\\").replace(':', "\\:"));
}

/// 칸 이름은 클로즈와 필드 HTML 을 깨지 않게 { } < > 와 줄바꿈을 뺀다
pub fn clean_label(label: &str) -> String {
    label.chars().filter(|c| !matches!(c, '{' | '}' | '<' | '>') && !c.is_control()).collect::<String>().trim().chars().take(40).collect()
}

/// 칸들 -> Occlusion 필드. 묻는 방식은 노트 단위라 모든 칸에 같은 oi 를 붙인다 (Anki 편집기도 같다)
pub fn to_field(shapes: &[Shape], hide_all: bool) -> String {
    let mut out = String::new();
    for s in shapes {
        out.push_str(&format!("{{{{c{}::image-occlusion:{}", s.ordinal, s.kind));
        let (left, top) = match s.kind.as_str() {
            // 다각형의 위치는 꼭짓점이 정한다
            "polygon" if !s.points.is_empty() => s.points.iter().fold((f64::MAX, f64::MAX), |(a, b), [x, y]| (a.min(*x), b.min(*y))),
            _ => (s.left, s.top),
        };
        push(&mut out, "left", &num(left));
        push(&mut out, "top", &num(top));
        match s.kind.as_str() {
            "rect" => {
                push(&mut out, "width", &num(s.width));
                push(&mut out, "height", &num(s.height));
            }
            "ellipse" => {
                push(&mut out, "rx", &num(s.width / 2.0));
                push(&mut out, "ry", &num(s.height / 2.0));
            }
            "polygon" => push(&mut out, "points", &s.points.iter().map(|[x, y]| format!("{},{}", num(*x), num(*y))).collect::<Vec<_>>().join(" ")),
            _ => {}
        }
        for (k, v) in &s.extra {
            push(&mut out, k, v);
        }
        let label = clean_label(&s.label);
        if !label.is_empty() {
            push(&mut out, "label", &label);
        }
        if hide_all {
            push(&mut out, "oi", "1");
        }
        out.push_str("}}<br>");
    }
    out
}

/// 카드를 만드는 번호 (c0 글자 칸은 카드가 없다)
pub fn ordinals(shapes: &[Shape]) -> BTreeSet<u32> {
    shapes.iter().map(|s| s.ordinal).filter(|n| *n > 0).collect()
}

// ------------------------------------------------------------------ 그리기

/// 우리가 만드는 겹침 요소의 머리. 칸을 그림 위로 옮길 때 이것으로 찾는다
const LAYER: &str = "<div class=\"io-";

fn pct(v: f64) -> String {
    format!("{:.2}%", v * 100.0)
}

pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 렌더러가 클로즈 자리에 두는 칸 하나 (rslib cloze.rs render_image_occlusion 과 같은 판정).
/// body 는 "image-occlusion:" 다음. active: 이 카드가 묻는 번호. style 에는 우리가 계산한 숫자만 들어간다
pub fn layer(ordinal: u32, body: &str, active: bool, is_back: bool) -> String {
    let Some((s, oi)) = parse_shape(ordinal, body) else { return String::new() };
    if s.kind == "text" || ordinal == 0 {
        let text = s.prop("text").unwrap_or_default();
        return format!("<div class=\"io-label\" style=\"left:{};top:{}\">{}</div>", pct(s.left), pct(s.top), escape(text));
    }
    let state = match (active, is_back) {
        (true, false) => " io-mask--ask",
        (true, true) => " io-mask--shown",
        (false, _) if oi => "",
        _ => return String::new(),
    };
    let (shape, style) = match s.kind.as_str() {
        "rect" | "ellipse" => {
            // angle 은 360° 를 10000 단계로 나눈 값 (shapes/lib.ts storedToAngle). 왼쪽 위를 축으로 돈다
            let turn = s.prop("angle").and_then(|a| a.parse::<f64>().ok()).filter(|a| a.is_finite() && *a != 0.0);
            let rotate = turn.map_or(String::new(), |a| format!(";transform:rotate({:.2}deg)", (a % 10000.0) / 10000.0 * 360.0));
            let shape = if s.kind == "ellipse" { " io-mask--ellipse" } else { "" };
            (shape, format!("left:{};top:{};width:{};height:{}{rotate}", pct(s.left), pct(s.top), pct(s.width), pct(s.height)))
        }
        "polygon" if s.points.len() >= 3 => {
            let pts: Vec<String> = s.points.iter().map(|[x, y]| format!("{} {}", pct(*x), pct(*y))).collect();
            (" io-mask--polygon", format!("clip-path:polygon({})", pts.join(",")))
        }
        _ => return String::new(),
    };
    format!("<div class=\"io-mask{shape}{state}\" style=\"{style}\"></div>")
}

/// IO 템플릿 결과에서 칸(숨긴 div 안)을 걷어 그림 위로 겹친다. Anki 의 스크립트·[Toggle Masks] 버튼은 지운다.
/// caption 은 답 면 그림 아래 "번호 · 이름" (M6, 번호는 배지). 그림이 없으면 칸은 버린다
pub fn place(html: &str, caption: Option<(u32, String)>) -> String {
    let mut rest = String::with_capacity(html.len());
    let mut layers = String::new();
    let mut s = html;
    while let Some(i) = s.find(LAYER) {
        rest.push_str(&s[..i]);
        let end = s[i..].find("</div>").map_or(s.len(), |j| i + j + "</div>".len());
        layers.push_str(&s[i..end]);
        s = &s[end..];
    }
    rest.push_str(s);
    for (open, close) in [("<script", "</script>"), ("<button id=\"toggle\"", "</button>")] {
        while let Some(i) = rest.find(open) {
            let end = rest[i..].find(close).map_or(rest.len(), |j| i + j + close.len());
            rest.replace_range(i..end, "");
        }
    }
    let from = rest.find("image-occlusion-container").unwrap_or(0);
    let Some(i) = rest[from..].find("<img").map(|i| from + i) else { return rest };
    let Some(j) = rest[i..].find('>').map(|j| i + j + 1) else { return rest };
    let caption = caption.map_or(String::new(), |(n, l)| format!("<div class=\"io-caption\"><span class=\"io-caption__num\">{n}</span>{}</div>", escape(&l)));
    format!("{}<div class=\"io\">{}{layers}</div>{caption}{}", &rest[..i], &rest[i..j], &rest[j..])
}

pub const DEFAULT_QUESTION: &str = "가린 곳에 들어갈 말은?";
/// 템플릿이 칸을 숨겨 두는 자리. 그 앞이 머리말이다
const HIDDEN: &str = "<div style=\"display: none\">";

/// 템플릿이 칸을 읽는 필드 ({{cloze:이름}}). 일반 빈칸 유형도 같다 (CSV 내보내기)
pub(crate) fn cloze_field(qfmt: &str) -> Option<&str> {
    let i = qfmt.find("{{cloze:")? + "{{cloze:".len();
    Some(qfmt[i..].split("}}").next()?.trim())
}

/// render_card 가 IO 템플릿이면 부른다. ord 는 카드의 template_ord (= 번호 - 1)
pub fn finish(front: String, back: String, qfmt: &str, fields: &HashMap<String, String>, ord: u32) -> crate::render::Rendered {
    let n = ord + 1;
    let shapes = cloze_field(qfmt).and_then(|f| fields.get(f)).map(|v| parse(v).0).unwrap_or_default();
    let label = shapes.iter().filter(|s| s.ordinal == n).map(|s| clean_label(&s.label)).find(|l| !l.is_empty());
    let front = place(&front, None);
    let back = place(&back, label.map(|l| (n, l)));
    // 질문(머리말)을 비우면 기본 질문 (M1 · M3-q). 답 면은 흐린 질문 줄 (M3-a): 머리말(숨긴 칸 앞)을 감싼다
    if crate::render::strip_tags(&front).is_empty() {
        let q = |dim: &str| format!("<div class=\"io-question{dim}\">{DEFAULT_QUESTION}</div>");
        return crate::render::Rendered { front: format!("{}{front}", q("")), back: format!("{}{back}", q(" io-question--back")) };
    }
    let back = match back.find(HIDDEN) {
        Some(i) => format!("<div class=\"io-question io-question--back\">{}</div>{}", &back[..i], &back[i..]),
        None => back,
    };
    crate::render::Rendered { front, back }
}

// ------------------------------------------------------------------ 편집 (M1 · M2)

/// 필드 HTML 의 첫 그림 파일 이름
fn image_name(html: &str) -> String {
    let mut names: Vec<String> = vec![];
    crate::export::media_refs(html, &mut names);
    names.into_iter().next().unwrap_or_default()
}

#[derive(Serialize)]
pub struct OcclusionCard {
    ordinal: u32,
    card_id: i64,
    /// 복습한 적이 있는가. 칸을 지울 때 확인 창을 띄운다 (M4 · C2-d)
    reviewed: bool,
}

#[derive(Serialize)]
pub struct OcclusionNote {
    note_id: i64,
    deck_id: i64,
    /// 미디어 파일 이름. 비었으면 M5 [그림 다시 넣기]
    image: String,
    header: String,
    back_extra: String,
    shapes: Vec<Shape>,
    hide_all: bool,
    cards: Vec<OcclusionCard>,
}

/// 노트가 IO 노트인가 (앱 유형이든 가져온 Anki 유형이든 템플릿으로 판정한다)
pub(crate) fn is_note(con: &Connection, note_id: i64) -> Res<bool> {
    con.query_row(
        "SELECT EXISTS (SELECT 1 FROM note n JOIN card_template t ON t.note_type_id = n.note_type_id
                        WHERE n.id = ?1 AND t.qfmt LIKE '%image-occlusion-container%')",
        [note_id],
        |r| r.get(0),
    )
    .map_err(err)
}

fn load(con: &Connection, note_id: i64) -> Res<OcclusionNote> {
    if !is_note(con, note_id)? {
        return Err("그림 가리기 노트가 아니에요.".into());
    }
    let fields: String = con.query_row("SELECT fields FROM note WHERE id = ?1", [note_id], |r| r.get(0)).map_err(err)?;
    let mut v: Vec<String> = serde_json::from_str(&fields).map_err(err)?;
    v.resize(4, String::new());
    let (shapes, hide_all) = parse(&v[0]);
    let mut st = con.prepare("SELECT id, template_ord, deck_id, reps > 0 OR lapses > 0 FROM card WHERE note_id = ?1 ORDER BY template_ord").map_err(err)?;
    let rows: Vec<(i64, u32, i64, bool)> =
        st.query_map([note_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    Ok(OcclusionNote {
        note_id,
        deck_id: rows.first().map_or(0, |r| r.2),
        image: image_name(&v[1]),
        header: v[2].clone(),
        back_extra: v[3].clone(),
        shapes,
        hide_all,
        cards: rows.into_iter().map(|(card_id, ord, _, reviewed)| OcclusionCard { ordinal: ord + 1, card_id, reviewed }).collect(),
    })
}

#[tauri::command]
pub fn occlusion_note(state: State<AppState>, note_id: i64) -> Res<OcclusionNote> {
    let con = state.db.lock().map_err(err)?;
    load(&con, note_id)
}

#[derive(Deserialize)]
pub struct OcclusionInput {
    /// 없으면 새 노트
    note_id: Option<i64>,
    deck_id: i64,
    image: String,
    header: String,
    back_extra: String,
    shapes: Vec<Shape>,
    hide_all: bool,
    importance: Option<i64>,
}

/// 노트 하나에 모든 칸을 저장한다. 새 번호는 카드를 만들고 빠진 번호는 카드를 지운다 (기록은 남는다, I14).
/// 노트 id 를 돌려준다
pub(crate) fn save(con: &Connection, input: &OcclusionInput) -> Res<i64> {
    if ordinals(&input.shapes).is_empty() {
        return Err("가릴 곳을 하나 이상 만들어주세요.".into());
    }
    let image = commands::safe_media_name(input.image.trim()).map_err(|_| "그림을 넣어주세요.".to_string())?;
    let occlusion = to_field(&input.shapes, input.hide_all);
    let img = format!("<img src=\"{}\">", escape(image));
    match input.note_id {
        Some(nid) => {
            if !is_note(con, nid)? {
                return Err("그림 가리기 노트가 아니에요.".into());
            }
            let fields: String = con.query_row("SELECT fields FROM note WHERE id = ?1", [nid], |r| r.get(0)).map_err(err)?;
            let mut v: Vec<String> = serde_json::from_str(&fields).map_err(err)?;
            v.resize(v.len().max(4), String::new());
            // 그림을 바꾸지 않았으면 원래 HTML(크기 속성 등)을 둔다
            if image_name(&v[1]) != image {
                v[1] = img;
            }
            (v[0], v[2], v[3]) = (occlusion, input.header.clone(), input.back_extra.clone());
            commands::save_note_fields(con, nid, &v)?;
            Ok(nid)
        }
        None => {
            let ntid: i64 = con
                .query_row("SELECT id FROM note_type WHERE name = ?1 AND anki_model_id IS NULL ORDER BY id LIMIT 1", [NATIVE], |r| r.get(0))
                .map_err(err)?;
            let now = now_ms();
            let fields = serde_json::to_string(&[occlusion.as_str(), &img, &input.header, &input.back_extra, ""]).map_err(err)?;
            con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", params![ntid, fields, now])
                .map_err(err)?;
            let nid = con.last_insert_rowid();
            for n in ordinals(&input.shapes) {
                con.execute(
                    "INSERT INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, CASE WHEN ?4 IS NULL THEN NULL ELSE 'user' END, ?5, ?5)",
                    params![nid, input.deck_id, n - 1, input.importance, now],
                )
                .map_err(err)?;
            }
            Ok(nid)
        }
    }
}

#[tauri::command]
pub fn save_occlusion(state: State<AppState>, note: OcclusionInput) -> Res<i64> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let id = save(&tx, &note)?;
    tx.commit().map_err(err)?;
    Ok(id)
}

/// 이름이 있는 칸 수 (C1-xr "칸 이름 N개는 Anki에서 보이지 않아요")
pub fn label_count(field: &str) -> usize {
    parse(field).0.iter().filter(|s| !clean_label(&s.label).is_empty()).count()
}

/// CSV 한 줄 (C1-xc): 앞면 "질문 (그림 파일)", 뒷면 칸마다 "번호 · 이름"(이름 없으면 번호). 겹침 HTML 은 표에서 쓸모가 없다
pub fn csv_cells(fields: &[String]) -> (String, String) {
    let get = |i: usize| fields.get(i).map_or("", String::as_str);
    let header = crate::render::strip_tags(get(2));
    let question = if header.is_empty() { DEFAULT_QUESTION.to_string() } else { header };
    let image = image_name(get(1));
    let front = if image.is_empty() { question } else { format!("{question} ({image})") };
    let shapes = parse(get(0)).0;
    let regions: Vec<String> = ordinals(&shapes)
        .into_iter()
        .map(|n| match shapes.iter().filter(|s| s.ordinal == n).map(|s| clean_label(&s.label)).find(|l| !l.is_empty()) {
            Some(l) => format!("{n} · {l}"),
            None => n.to_string(),
        })
        .collect();
    (front, regions.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render_card;

    /// Anki 26.9.2 가 저장한 그대로 (스크래치에서 Anki 로 만든 .apkg 에서 확인한 모양)
    const ANKI: &str = "{{c1::image-occlusion:rect:left=.1:top=.2:width=.25:height=.1:oi=1}}<br>\
        {{c2::image-occlusion:ellipse:left=.5:top=.5:rx=.1:ry=.05:oi=1}}<br>\
        {{c3::image-occlusion:rect:left=.6:top=.1:width=.2:height=.1:angle=833:oi=1}}<br>\
        {{c3::image-occlusion:polygon:left=.1:top=.6:points=.1,.6 .3,.6 .2,.9:oi=1}}<br>\
        {{c0::image-occlusion:text:left=.4:top=.05:text=a\\:b:scale=1.:fs=.0667:oi=1}}<br>";

    fn rect(ordinal: u32, left: f64, label: &str) -> Shape {
        Shape { ordinal, kind: "rect".into(), left, top: 0.1, width: 0.2, height: 0.1, points: vec![], label: label.into(), extra: vec![] }
    }

    #[test]
    fn numbers_match_anki_float_to_display() {
        let got = [num(0.25), num(1.0), num(0.0), num(-0.5), num(1.0 / 3.0), num(0.00001), num(f64::NAN)];
        assert_eq!(got, [".25", "1.", ".0000", "-0.5", ".3333", ".0000", ".0000"]);
    }

    #[test]
    fn parses_anki_shapes() {
        let (shapes, hide_all) = parse(ANKI);
        assert!(hide_all);
        let kinds: Vec<(u32, &str)> = shapes.iter().map(|s| (s.ordinal, s.kind.as_str())).collect();
        assert_eq!(kinds, [(1, "rect"), (2, "ellipse"), (3, "rect"), (3, "polygon"), (0, "text")]);
        let e = &shapes[1];
        assert_eq!((e.left, e.top, e.width, e.height), (0.5, 0.5, 0.2, 0.1), "타원은 외접 사각형으로");
        assert_eq!(shapes[2].prop("angle"), Some("833"), "모르는 속성은 그대로 둔다");
        assert_eq!(shapes[3].points, [[0.1, 0.6], [0.3, 0.6], [0.2, 0.9]]);
        assert!((shapes[3].width - 0.2).abs() < 1e-9 && (shapes[3].height - 0.3).abs() < 1e-9);
        assert_eq!(shapes[4].prop("text"), Some("a:b"), "\\: 이스케이프를 푼다");
        assert_eq!(ordinals(&shapes).into_iter().collect::<Vec<_>>(), [1, 2, 3], "c0 은 카드가 없다");
        assert_eq!(crate::render::cloze_numbers(ANKI).into_iter().collect::<Vec<_>>(), [1, 2, 3], "c0 이 template_ord 에서 넘치지 않는다");
        // 다각형은 Anki 처럼 꼭짓점을 left/top 으로 옮긴다 (review.ts getPolygonOffset)
        let (moved, _) = parse("{{c1::image-occlusion:polygon:left=.5:top=.5:points=.1,.1 .3,.1 .2,.4}}");
        let pts: Vec<String> = moved[0].points.iter().map(|[x, y]| format!("{},{}", num(*x), num(*y))).collect();
        assert_eq!(pts, [".5,.5", ".7,.5", ".6,.8"]);
        // 깨진 값은 0, 모양이 아닌 클로즈는 건너뛴다
        let (odd, hide) = parse("{{c1::서울}} {{c2::image-occlusion:rect:left=x:top=.2:width=.1:height=.1}} {{c3::image-occlusion}}");
        assert_eq!((odd.len(), odd[0].left, odd[0].top, hide), (1, 0.0, 0.2, false));
    }

    #[test]
    fn serialization_round_trip() {
        let (shapes, hide_all) = parse(ANKI);
        let again = to_field(&shapes, hide_all);
        assert_eq!(parse(&again), (shapes.clone(), true), "다시 읽으면 같다");
        // angle 이 없는 칸은 Anki 가 쓴 글자와 똑같다
        let anki_prefix = "{{c1::image-occlusion:rect:left=.1:top=.2:width=.25:height=.1:oi=1}}<br>\
                           {{c2::image-occlusion:ellipse:left=.5:top=.5:rx=.1:ry=.05:oi=1}}<br>";
        assert!(again.starts_with(anki_prefix), "{again}");
        assert!(again.contains("{{c3::image-occlusion:polygon:left=.1:top=.6:points=.1,.6 .3,.6 .2,.9:oi=1}}"));
        assert!(again.contains("text=a\\:b"), "값의 : 는 다시 이스케이프");
        // 하나만 가리기는 oi 를 뺀다. 칸 이름은 label= 로, 이스케이프해서
        let one = to_field(&[rect(4, 0.0, " a:b\\c{x}<y> ")], false);
        assert_eq!(one, "{{c4::image-occlusion:rect:left=.0000:top=.1:width=.2:height=.1:label=a\\:b\\\\cxy}}<br>");
        let (back, hide) = parse(&one);
        assert_eq!((back[0].label.as_str(), hide), ("a:b\\cxy", false));
        assert_eq!(label_count(&one), 1);
    }

    fn fields(occlusion: &str) -> HashMap<String, String> {
        [("Occlusion", occlusion), ("Image", "<img src=\"a.png\">"), ("Header", "머리말"), ("Back Extra", "덧붙임"), ("Comments", "")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn afmt() -> String {
        format!("{QFMT}{AFMT_TAIL}")
    }

    #[test]
    fn renders_masks_over_the_image() {
        let field = to_field(&[rect(1, 0.1, "인터넷 계층"), rect(2, 0.5, ""), rect(2, 0.7, "")], true);
        // 둘째 카드 (c2 묶음)
        let r = render_card(QFMT, &afmt(), &fields(&field), 1);
        let ask = "<div class=\"io-mask io-mask--ask\" style=\"left:50.00%;top:10.00%;width:20.00%;height:10.00%\"></div>";
        let cover = "<div class=\"io-mask\" style=\"left:10.00%;top:10.00%;width:20.00%;height:10.00%\"></div>";
        let inside = r.front.split("<div class=\"io\">").nth(1).unwrap().split("</div>\n").next().unwrap();
        assert_eq!(inside, format!("<img src=\"a.png\">{cover}{ask}{}", ask.replace("50.00", "70.00")), "모두 가림: 다른 칸은 덮개, 묻는 칸(묶음 둘)은 강조");
        assert!(r.front.contains("<div>머리말</div>") && !r.front.contains("덧붙임"));
        for side in [&r.front, &r.back] {
            assert!(!side.contains("<script") && !side.contains("Toggle Masks") && !side.contains("anki.imageOcclusion"), "Anki 스크립트·버튼은 지운다");
            assert!(side.contains("<div style=\"display: none\"><br><br><br></div>"), "숨긴 자리에는 줄바꿈만 남는다");
        }
        assert!(r.back.contains("io-mask--shown") && !r.back.contains("io-mask--ask") && r.back.contains("덧붙임"));
        assert!(!r.back.contains("io-caption"), "이름이 없으면 번호 · 이름 줄도 없다");
        // 첫 카드 답 면: "1 · 인터넷 계층"
        let r = render_card(QFMT, &afmt(), &fields(&field), 0);
        assert!(r.back.contains("</div><div class=\"io-caption\"><span class=\"io-caption__num\">1</span>인터넷 계층</div>"), "{}", r.back);
        // 답 면 머리말은 흐린 질문 줄로 감싼다. 머리말이 비면 기본 질문이 양면에
        assert!(r.back.starts_with("<div class=\"io-question io-question--back\"><div>머리말</div>"), "{}", r.back);
        // 하나만 가림: 묻지 않는 칸은 그리지 않는다
        let r = render_card(QFMT, &afmt(), &fields(&to_field(&[rect(1, 0.1, ""), rect(2, 0.5, "")], false)), 0);
        assert_eq!(r.front.matches("class=\"io-mask").count(), 1, "묻는 칸 하나만: {}", r.front);
        // 글자 칸(c0)은 늘 보이고 글자는 이스케이프한다. 그림이 없으면 칸은 버린다
        let text = "{{c0::image-occlusion:text:left=.4:top=.05:text=<b>\\:</b>}}{{c1::image-occlusion:rect:left=.1:top=.1:width=.1:height=.1}}";
        let r = render_card(QFMT, &afmt(), &fields(text), 0);
        assert!(r.front.contains("<div class=\"io-label\" style=\"left:40.00%;top:5.00%\">&lt;b&gt;:&lt;/b&gt;</div>"), "{}", r.front);
        let mut no_image = fields(&field);
        no_image.insert("Image".into(), String::new());
        assert!(!render_card(QFMT, &afmt(), &no_image, 0).front.contains("io-mask"));
        // 목록 글자 (strip_tags) 에 스크립트 글자가 새지 않는다
        assert_eq!(crate::render::strip_tags(&render_card(QFMT, &afmt(), &fields(&field), 0).front), "머리말");
        let mut no_header = fields(&field);
        no_header.insert("Header".into(), "<br>".into());
        assert_eq!(crate::render::strip_tags(&render_card(QFMT, &afmt(), &no_header, 0).front), DEFAULT_QUESTION, "머리말이 비면 기본 질문");
        assert!(render_card(QFMT, &afmt(), &no_header, 0).back.starts_with(&format!("<div class=\"io-question io-question--back\">{DEFAULT_QUESTION}</div>")));
    }

    /// 살균(src/cardHtml.ts) 은 class · style 을 남긴다. 그래서 style 에는 우리가 계산한 숫자만 들어가야 한다
    #[test]
    fn layers_only_carry_computed_numbers() {
        let evil = "{{c1::image-occlusion:rect:left=1);background\\:url(x):top=.1\" onmouseover=\"x:width=.1:height=.1:angle=1;color\\:red}}\
                    {{c2::image-occlusion:polygon:left=0:top=0:points=0,0 1,0 x\" y,1:oi=1}}\
                    {{c0::image-occlusion:text:left=.1:top=.1:text=\"><img src=x onerror=alert(1)>}}";
        let html = render_card(QFMT, &afmt(), &fields(evil), 0).front;
        let mut rest = html.as_str();
        let mut seen = 0;
        while let Some(i) = rest.find("<div class=\"io") {
            let tag = &rest[i..i + rest[i..].find('>').unwrap()];
            if let Some(style) = tag.split("style=\"").nth(1) {
                assert!(style.trim_end_matches('"').chars().all(|c| c.is_ascii_alphanumeric() || "-:;.%(), ".contains(c)), "{tag}");
                assert_eq!(tag.matches('"').count(), 4, "class · style 두 속성뿐: {tag}");
            }
            rest = &rest[i + 1..];
            seen += 1;
        }
        assert_eq!(seen, 3, "감싸개 · 묻는 칸 · 글자 칸 (깨진 다각형은 그리지 않는다): {html}");
        assert!(html.contains("&quot;&gt;&lt;img src=x onerror=alert(1)&gt;"), "글자는 이스케이프: {html}");
        // IO 가 아닌 노트에 같은 모양의 HTML 이 있어도 옮기거나 감싸지 않는다
        let user = "<div class=\"io-mask\" style=\"left:0\"></div><img src=\"a.png\">";
        let basic = render_card("{{Front}}", "{{FrontSide}}", &HashMap::from([("Front".to_string(), user.to_string())]), 0);
        assert_eq!(basic.front, user);
    }

    fn cards(con: &Connection, nid: i64) -> Vec<(u32, i64)> {
        con.prepare("SELECT template_ord + 1, id FROM card WHERE note_id = ?1 ORDER BY template_ord")
            .unwrap()
            .query_map([nid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn card_per_number_and_stable_ids() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('해부학', 0, 0)", []).unwrap();
        let did = con.last_insert_rowid();
        let input = |note_id, shapes: Vec<Shape>| OcclusionInput {
            note_id,
            deck_id: did,
            image: "neo-1.png".into(),
            header: String::new(),
            back_extra: String::new(),
            shapes,
            hide_all: true,
            importance: Some(2),
        };
        // c1, c2(묶음 둘), c3 -> 카드 세 장
        let nid = save(&con, &input(None, vec![rect(1, 0.0, "a"), rect(2, 0.2, ""), rect(2, 0.3, ""), rect(3, 0.5, "c")])).unwrap();
        let before = cards(&con, nid);
        assert_eq!(before.iter().map(|c| c.0).collect::<Vec<_>>(), [1, 2, 3]);
        let imp: i64 = con.query_row("SELECT count(*) FROM card WHERE note_id = ?1 AND importance = 2", [nid], |r| r.get(0)).unwrap();
        assert_eq!(imp, 3);
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating, elapsed_ms, prev_state) VALUES (?1, 1, 3, 1000, 'new')", [before[1].1])
            .unwrap();
        con.execute("UPDATE card SET reps = 1 WHERE id = ?1", [before[1].1]).unwrap();

        // c1 을 지우고 순서를 바꾸고(c3 이 먼저) 새 칸 c4 를 더한다: c2 · c3 카드와 기록은 그대로
        let nid2 = save(&con, &input(Some(nid), vec![rect(3, 0.5, "c"), rect(2, 0.2, ""), rect(2, 0.3, ""), rect(4, 0.8, "")])).unwrap();
        assert_eq!(nid2, nid);
        let after = cards(&con, nid);
        assert_eq!(after.iter().map(|c| c.0).collect::<Vec<_>>(), [2, 3, 4]);
        assert_eq!((after[0].1, after[1].1), (before[1].1, before[2].1), "번호가 카드 id 를 지킨다");
        let logged: i64 = con.query_row("SELECT card_id FROM review_log", [], |r| r.get(0)).unwrap();
        assert_eq!(logged, before[1].1, "기록이 다른 칸으로 가지 않는다");

        // c3 을 c2 에 묶으면 c2 카드만 남는다 (기록이 있는 쪽)
        save(&con, &input(Some(nid), vec![rect(2, 0.5, ""), rect(2, 0.2, ""), rect(4, 0.8, "")])).unwrap();
        assert_eq!(cards(&con, nid).iter().map(|c| c.0).collect::<Vec<_>>(), [2, 4]);
        let note = load(&con, nid).unwrap();
        assert_eq!((note.image.as_str(), note.hide_all, note.shapes.len()), ("neo-1.png", true, 3));
        assert!(note.cards.iter().any(|c| c.ordinal == 2 && c.reviewed) && note.cards.iter().any(|c| c.ordinal == 4 && !c.reviewed));

        // 막는 오류: 칸 0개 (글자 칸만 있어도), 그림 없음
        let text = Shape { kind: "text".into(), ..rect(0, 0.0, "") };
        assert!(save(&con, &input(None, vec![text])).is_err());
        assert!(save(&con, &OcclusionInput { image: "../x.png".into(), ..input(None, vec![rect(1, 0.0, "")]) }).is_err());
        // 일반 빈칸 노트를 그림 가리기로 덮어쓰지 않는다
        let card = crate::commands::NewCard { deck_id: did, kind: "cloze".into(), front: "{{c1::x}}".into(), back: String::new(), importance: None };
        crate::commands::insert_note(&con, &card, "", None).unwrap();
        let cloze: i64 = con.query_row("SELECT max(id) FROM note", [], |r| r.get(0)).unwrap();
        assert!(!is_note(&con, cloze).unwrap() && save(&con, &input(Some(cloze), vec![rect(1, 0.0, "")])).is_err());
    }
}
