//! 8.1 노트 유형 · 카드 템플릿 (docs/engineering/note-types.md). 1단계: 엔진과 명령만, 화면은 2단계.
//!
//! - 앱 유형은 `native_kind` 로 알아보고 읽기 전용이다. 고치려면 복제한다 (list · choice · io 는 복제하지 않는다)
//! - 필드는 자리(slot) 고정: note.fields 의 위치를 다시 매기지 않는다. 지운 자리는 이름 "", 보이는 순서는 `field_order`
//! - 템플릿 ord 는 정체성: 다시 매기지 않고, 지운 번호도 다시 쓰지 않는다 (`next_ord`)

use crate::commands::AppState;
use crate::db::now_ms;
use crate::{quality, render};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 카드 -> 그 카드를 그리는 템플릿 (별칭 c = 카드, t = card_template). 빈칸 유형은 템플릿 하나(ord 0)로 모든 빈칸 카드를 그린다.
/// 앱 Cloze 도 앞면이 {{cloze: 라 이름을 보지 않는다
pub const TEMPLATE_OF_CARD: &str = "t.ord = (CASE WHEN t.qfmt LIKE '%{{cloze:%' THEN 0 ELSE c.template_ord END)";
/// 노트 유형(별칭 nt)이 빈칸 유형인가. 템플릿 저장 검사(check_template)가 빈칸 여부를 바꾸지 못하게 지킨다
pub const IS_CLOZE: &str = "EXISTS (SELECT 1 FROM card_template ct WHERE ct.note_type_id = nt.id AND ct.qfmt LIKE '%{{cloze:%')";

const APP_TYPE: &str = "앱 기본 유형은 고칠 수 없어요. 복제해서 고쳐 주세요.";
/// Anki 가 템플릿에서 따로 쓰는 이름 (필드 이름으로 쓸 수 없다. 아직 그리지 않는 것도 있다: 다음 단계)
const SPECIAL: [&str; 8] = ["FrontSide", "Card", "CardFlag", "CardID", "Deck", "Subdeck", "Tags", "Type"];
/// 복제할 수 있는 앱 유형. 목록 · 선택형 · 그림 가리기는 학습 화면이 필드로 따로 그려 복제본이 다르게 보인다
const CLONEABLE: [&str; 4] = ["basic", "reversed", "cloze", "written"];

// ------------------------------------------------------------------ 읽기

#[derive(Clone)]
pub(crate) struct Tpl {
    pub ord: u32,
    pub name: String,
    pub qfmt: String,
    pub afmt: String,
}

#[derive(Clone)]
pub(crate) struct Nt {
    pub id: i64,
    pub name: String,
    /// 자리별 이름. 지운 자리는 ""
    pub slots: Vec<String>,
    pub order: Option<String>,
    pub native_kind: Option<String>,
    pub user_made: bool,
    pub anki_model_id: Option<i64>,
    pub stock: Option<i64>,
    pub css: String,
    pub next_ord: Option<i64>,
    pub templates: Vec<Tpl>,
}

impl Nt {
    pub fn visible(&self) -> Vec<(usize, String)> {
        visible(&self.slots, self.order.as_deref())
    }

    pub fn cloze(&self) -> bool {
        self.templates.iter().any(|t| cloze_like(&t.qfmt))
    }

    fn editable(&self) -> Res<()> {
        if self.native_kind.is_some() {
            return Err(APP_TYPE.into());
        }
        Ok(())
    }

    fn cloneable(&self) -> bool {
        self.native_kind.as_deref().is_none_or(|k| CLONEABLE.contains(&k))
    }

    fn template(&self, ord: u32) -> Res<&Tpl> {
        self.templates.iter().find(|t| t.ord == ord).ok_or_else(|| "카드 종류를 찾을 수 없어요.".to_string())
    }

    /// 노트 값(자리 순서) -> 템플릿에 넣을 이름 -> 값. 지운 자리는 넣지 않는다. 짧은 배열의 없는 칸은 빈 값
    pub fn field_map(&self, values: &[String]) -> HashMap<String, String> {
        self.visible().into_iter().map(|(s, n)| (n, values.get(s).cloned().unwrap_or_default())).collect()
    }

    /// 보이는 순서의 값 -> 자리 순서 배열 (지운 자리는 빈 값)
    fn to_slots(&self, values: &[String]) -> Res<Vec<String>> {
        let vis = self.visible();
        if values.len() != vis.len() {
            return Err("필드 수가 노트 유형과 달라요.".into());
        }
        let mut out = vec![String::new(); self.slots.len()];
        for ((s, _), v) in vis.iter().zip(values) {
            out[*s] = v.clone();
        }
        Ok(out)
    }
}

/// 보이는 필드 (자리, 이름): field_order 의 살아 있는 자리, 그다음 거기 없는 살아 있는 자리를 자리 순서로.
/// fields 와 field_order 가 열 단위 LWW 로 서로 다른 기기 것이어도 필드를 잃지 않고, 지운 필드가 살아나지 않는다
pub fn visible(names: &[String], order: Option<&str>) -> Vec<(usize, String)> {
    let listed: Vec<usize> = order.and_then(|o| serde_json::from_str(o).ok()).unwrap_or_default();
    let mut out: Vec<usize> = vec![];
    for s in listed.into_iter().chain(0..names.len()) {
        if names.get(s).is_some_and(|n| !n.is_empty()) && !out.contains(&s) {
            out.push(s);
        }
    }
    out.into_iter().map(|s| (s, names[s].clone())).collect()
}

pub(crate) fn load(con: &Connection, id: i64) -> Res<Nt> {
    let mut nt = con
        .query_row(
            "SELECT id, name, fields, field_order, native_kind, user_made, anki_model_id, anki_stock_kind, css, next_ord FROM note_type WHERE id = ?1",
            [id],
            |r| {
                Ok(Nt {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    slots: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                    order: r.get(3)?,
                    native_kind: r.get(4)?,
                    user_made: r.get::<_, i64>(5)? == 1,
                    anki_model_id: r.get(6)?,
                    stock: r.get(7)?,
                    css: r.get(8)?,
                    next_ord: r.get(9)?,
                    templates: vec![],
                })
            },
        )
        .optional()
        .map_err(err)?
        .ok_or("노트 유형을 찾을 수 없어요.")?;
    let mut st = con.prepare("SELECT ord, name, qfmt, afmt FROM card_template WHERE note_type_id = ?1 ORDER BY ord").map_err(err)?;
    nt.templates = st
        .query_map([id], |r| Ok(Tpl { ord: r.get(0)?, name: r.get(1)?, qfmt: r.get(2)?, afmt: r.get(3)? }))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    Ok(nt)
}

// ------------------------------------------------------------------ 템플릿 글자 (순수 함수)

/// 템플릿 앞면의 빈칸 표시 (SQL 의 LIKE '%{{cloze:%' 와 같게 대소문자를 보지 않는다)
pub fn cloze_like(tpl: &str) -> bool {
    tpl.to_ascii_lowercase().contains("{{cloze:")
}

/// {{…}} 의 안쪽들 (render.rs 와 같은 읽기: "{{" 다음 처음 나오는 "}}")
fn tag_inners(tpl: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut rest = tpl;
    while let Some(i) = rest.find("{{") {
        let Some(len) = rest[i + 2..].find("}}") else { break };
        out.push(&rest[i + 2..i + 2 + len]);
        rest = &rest[i + 4 + len..];
    }
    out
}

/// 태그마다 바꾼다. f 가 None 이면 그대로
fn map_tags(tpl: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(i) = rest.find("{{") {
        let Some(len) = rest[i + 2..].find("}}") else { break };
        out.push_str(&rest[..i]);
        match f(&rest[i + 2..i + 2 + len]) {
            Some(new) => out.push_str(&format!("{{{{{new}}}}}")),
            None => out.push_str(&rest[i..i + 4 + len]),
        }
        rest = &rest[i + 4 + len..];
    }
    out.push_str(rest);
    out
}

/// 태그 안에서 필드 이름의 위치. {{X}} {{#X}} {{^X}} {{/X}} {{필터:필터:X}}
fn name_range(inner: &str) -> (usize, usize) {
    let lead = inner.len() - inner.trim_start().len();
    let t = inner.trim();
    let off = if t.starts_with(['#', '^', '/']) { 1 } else { t.rfind(':').map_or(0, |i| i + 1) };
    let name = &t[off..];
    let s = lead + off + (name.len() - name.trim_start().len());
    (s, s + name.trim().len())
}

/// 템플릿이 쓰는 필드 이름 (FrontSide · 특수 이름은 뺀다)
pub fn refs(tpl: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for inner in tag_inners(tpl) {
        let (s, e) = name_range(inner);
        let n = &inner[s..e];
        if !n.is_empty() && !SPECIAL.contains(&n) && !out.iter().any(|x| x == n) {
            out.push(n.to_string());
        }
    }
    out
}

/// {{cloze:X}} 로 쓰는 필드 (빈칸 카드의 앞면이 비는지 볼 때)
fn cloze_refs(tpl: &str) -> Vec<String> {
    tag_inners(tpl)
        .into_iter()
        .filter_map(|inner| {
            let (s, e) = name_range(inner);
            inner[..s].split(':').any(|f| f.trim() == "cloze").then(|| inner[s..e].to_string())
        })
        .collect()
}

/// 필드 이름을 바꾼 템플릿
pub fn rename_refs(tpl: &str, old: &str, new: &str) -> String {
    map_tags(tpl, |inner| {
        let (s, e) = name_range(inner);
        (&inner[s..e] == old).then(|| format!("{}{new}{}", &inner[..s], &inner[e..]))
    })
}

/// {{#X}} · {{^X}} 와 {{/X}} 의 짝
fn sections_ok(tpl: &str) -> bool {
    let mut open: Vec<&str> = vec![];
    for inner in tag_inners(tpl) {
        let t = inner.trim();
        if let Some(n) = t.strip_prefix(['#', '^']) {
            open.push(n.trim());
        } else if let Some(n) = t.strip_prefix('/')
            && open.pop() != Some(n.trim())
        {
            return false;
        }
    }
    open.is_empty()
}

/// 템플릿 저장 검사. 빈칸 여부는 템플릿으로 바꾸지 못한다 (카드 ord 의 뜻이 통째로 바뀐다)
fn check_template(nt: &Nt, qfmt: &str, afmt: &str) -> Res<()> {
    let names: Vec<String> = nt.visible().into_iter().map(|(_, n)| n).collect();
    if nt.cloze() && !cloze_like(qfmt) {
        return Err("빈칸 유형의 앞면에는 {{cloze:필드}} 가 있어야 해요.".into());
    }
    if !nt.cloze() && (cloze_like(qfmt) || cloze_like(afmt)) {
        return Err("일반 유형에는 {{cloze:…}} 를 쓸 수 없어요. 빈칸 유형을 복제해서 만들어 주세요.".into());
    }
    for (side, t) in [("앞면", qfmt), ("뒷면", afmt)] {
        if !sections_ok(t) {
            return Err(format!("{side}의 {{{{#…}}}} 와 {{{{/…}}}} 짝이 맞지 않아요."));
        }
        if let Some(n) = refs(t).into_iter().find(|n| !names.contains(n)) {
            return Err(format!("{side}에 쓴 '{n}' 필드가 없어요."));
        }
    }
    if refs(qfmt).is_empty() {
        return Err("앞면에 필드를 하나 이상 넣어 주세요.".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ 카드 만들기 규칙

/// 그림 · 소리 없이 비어 보이는 칸
fn blank_field(v: &str) -> bool {
    render::is_blank(v) && !v.contains("[sound:")
}

/// Anki 의 "앞면이 비지 않으면 카드": 비지 않은 필드만 넣어 그린 앞면이 모든 필드를 비운 앞면과 다르다.
/// 고정 글자만으로는 카드가 생기지 않는다
pub fn has_front(qfmt: &str, map: &HashMap<String, String>) -> bool {
    let live = map.iter().map(|(k, v)| (k.clone(), if blank_field(v) { String::new() } else { v.clone() })).collect();
    let empty = map.keys().map(|k| (k.clone(), String::new())).collect();
    render::render_card(qfmt, "", &live, 0).front != render::render_card(qfmt, "", &empty, 0).front
}

/// 빈칸 카드(ord = 번호 - 1)의 앞면에 그 빈칸이 있는가
fn cloze_front(qfmt: &str, map: &HashMap<String, String>, ord: u32) -> bool {
    cloze_refs(qfmt).iter().filter_map(|f| map.get(f)).any(|v| render::cloze_numbers(v).contains(&(ord + 1)))
}

/// 노트 값(자리 순서)으로 생길 카드 번호. 빈칸 유형은 보이는 필드의 빈칸 번호 - 1, 아니면 앞면이 비지 않는 템플릿
pub(crate) fn card_ords(nt: &Nt, values: &[String]) -> Vec<u32> {
    if nt.cloze() {
        let nums: BTreeSet<u32> = nt.visible().iter().filter_map(|(s, _)| values.get(*s)).flat_map(|v| render::cloze_numbers(v)).collect();
        return nums.into_iter().map(|n| n - 1).collect();
    }
    let map = nt.field_map(values);
    nt.templates.iter().filter(|t| has_front(&t.qfmt, &map)).map(|t| t.ord).collect()
}

/// 일반 유형 노트의 빠진 카드를 만든다 (Anki 처럼 만들기만 하고 지우지 않는다). 덱 · 중요도는 그 노트의 첫 카드를 따른다 (C3-r). 만든 수
pub(crate) fn add_missing_cards(con: &Connection, nt: &Nt, note_id: i64, values: &[String]) -> Res<usize> {
    if nt.cloze() {
        return Ok(0);
    }
    let have: Vec<u32> = {
        let mut st = con.prepare("SELECT template_ord FROM card WHERE note_id = ?1").map_err(err)?;
        st.query_map([note_id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    let now = now_ms();
    let mut made = 0;
    for ord in card_ords(nt, values).into_iter().filter(|o| !have.contains(o)) {
        made += con
            .execute(
                "INSERT INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
                 SELECT note_id, deck_id, ?2, importance, importance_source, ?3, ?3 FROM card WHERE note_id = ?1 ORDER BY id LIMIT 1",
                params![note_id, ord, now],
            )
            .map_err(err)?;
    }
    Ok(made)
}

/// 이 유형의 모든 노트 (id, 자리 순서 값)
fn notes_of(con: &Connection, id: i64) -> Res<Vec<(i64, Vec<String>)>> {
    let mut st = con.prepare("SELECT id, fields FROM note WHERE note_type_id = ?1 ORDER BY id").map_err(err)?;
    let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(err)?;
    rows.map(|r| r.map(|(id, f)| (id, serde_json::from_str(&f).unwrap_or_default()))).collect::<Result<_, _>>().map_err(err)
}

fn fill_cards(con: &Connection, id: i64) -> Res<usize> {
    let nt = load(con, id)?;
    let mut made = 0;
    for (nid, values) in notes_of(con, id)? {
        made += add_missing_cards(con, &nt, nid, &values)?;
    }
    Ok(made)
}

// ------------------------------------------------------------------ 바꾸기 (명령의 본문. 모두 한 트랜잭션 안에서 부른다)

fn touch(con: &Connection, id: i64) -> Res<()> {
    con.execute("UPDATE note_type SET updated_at = ?2 WHERE id = ?1", params![id, now_ms()]).map_err(err)?;
    Ok(())
}

fn save_slots(con: &Connection, nt: &Nt) -> Res<()> {
    con.execute(
        "UPDATE note_type SET fields = ?2, updated_at = ?3 WHERE id = ?1",
        params![nt.id, serde_json::to_string(&nt.slots).map_err(err)?, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

fn name_taken(con: &Connection, name: &str, except: Option<i64>) -> Res<bool> {
    con.query_row("SELECT EXISTS (SELECT 1 FROM note_type WHERE name = ?1 AND id IS NOT ?2)", params![name, except], |r| r.get(0)).map_err(err)
}

fn type_name(con: &Connection, name: &str, except: Option<i64>) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("유형 이름을 입력해 주세요.".into());
    }
    if name_taken(con, n, except)? {
        return Err(format!("'{n}' 유형이 이미 있어요."));
    }
    Ok(n.to_string())
}

fn field_name(nt: &Nt, name: &str, except: Option<usize>) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("필드 이름을 입력해 주세요.".into());
    }
    if n.contains([':', '{', '}', '"']) || n.starts_with(['#', '/', '^']) {
        return Err("필드 이름에는 : { } \" 를 쓸 수 없고, # / ^ 로 시작할 수 없어요.".into());
    }
    if SPECIAL.contains(&n) {
        return Err(format!("'{n}'은 Anki 가 따로 쓰는 이름이에요."));
    }
    if nt.visible().iter().any(|(s, x)| Some(*s) != except && x.to_lowercase() == n.to_lowercase()) {
        return Err(format!("'{n}' 필드가 이미 있어요."));
    }
    Ok(n.to_string())
}

fn template_name(nt: &Nt, name: &str, except: Option<u32>) -> Res<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("카드 종류 이름을 입력해 주세요.".into());
    }
    if nt.templates.iter().any(|t| Some(t.ord) != except && t.name == n) {
        return Err(format!("'{n}' 카드 종류가 이미 있어요."));
    }
    Ok(n.to_string())
}

/// Anki 식 모델 id: 지금 ms, 이미 있으면 하나씩 올린다. 동기화 자연 키라 만들 때 정해 둔다
fn new_model_id(con: &Connection) -> Res<i64> {
    let mut id = now_ms();
    while con.query_row("SELECT EXISTS (SELECT 1 FROM note_type WHERE anki_model_id = ?1)", [id], |r| r.get::<_, bool>(0)).map_err(err)? {
        id += 1;
    }
    Ok(id)
}

/// [복제해서 고치기]. 지운 자리 · 보이는 순서는 정리해서(보이는 순서대로 새 자리) 옮기고, 템플릿은 0 부터 다시 센다 (아직 카드가 없다)
pub(crate) fn clone_type(con: &Connection, id: i64, name: Option<&str>) -> Res<i64> {
    let src = load(con, id)?;
    if !src.cloneable() {
        return Err("이 유형은 학습 화면이 따로 그리는 앱 유형이라 복제할 수 없어요.".into());
    }
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => type_name(con, n, None)?,
        None => {
            let base = format!("{} 복사본", src.name);
            let mut n = base.clone();
            let mut i = 2;
            while name_taken(con, &n, None)? {
                n = format!("{base} {i}");
                i += 1;
            }
            n
        }
    };
    // Anki 복제처럼 원래 기본 유형 표시를 물려받는다 (I78)
    let stock = src.stock.or(match src.native_kind.as_deref() {
        Some("basic") => Some(1),
        Some("reversed") => Some(2),
        Some("cloze") => Some(5),
        _ => None,
    });
    let fields: Vec<String> = src.visible().into_iter().map(|(_, n)| n).collect();
    let now = now_ms();
    con.execute(
        "INSERT INTO note_type (name, fields, css, created_at, updated_at, anki_model_id, anki_stock_kind, user_made, next_ord)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, 1, ?7)",
        params![name, serde_json::to_string(&fields).map_err(err)?, src.css, now, new_model_id(con)?, stock, src.templates.len() as i64],
    )
    .map_err(err)?;
    let new = con.last_insert_rowid();
    for (ord, t) in src.templates.iter().enumerate() {
        con.execute(
            "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![new, ord as i64, t.name, t.qfmt, t.afmt, now],
        )
        .map_err(err)?;
    }
    Ok(new)
}

pub(crate) fn rename_type(con: &Connection, id: i64, name: &str) -> Res<()> {
    load(con, id)?.editable()?;
    let name = type_name(con, name, Some(id))?;
    con.execute("UPDATE note_type SET name = ?2, updated_at = ?3 WHERE id = ?1", params![id, name, now_ms()]).map_err(err)?;
    Ok(())
}

/// 쓰는 노트가 없을 때만 (K29: 남은 노트가 가리키는 유형은 지우지 않는다)
pub(crate) fn delete_type(con: &Connection, id: i64) -> Res<()> {
    load(con, id)?.editable()?;
    let notes: i64 = con.query_row("SELECT count(*) FROM note WHERE note_type_id = ?1", [id], |r| r.get(0)).map_err(err)?;
    if notes > 0 {
        return Err(format!("이 유형을 쓰는 노트가 {notes}개 있어요. 노트를 먼저 지워 주세요."));
    }
    con.execute("DELETE FROM note_type WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
}

/// 끝에 새 자리. 노트는 고치지 않는다 (짧은 배열의 없는 칸 = 빈 값). 새 자리 번호
pub(crate) fn add_field(con: &Connection, id: i64, name: &str) -> Res<usize> {
    let mut nt = load(con, id)?;
    nt.editable()?;
    let name = field_name(&nt, name, None)?;
    nt.slots.push(name);
    save_slots(con, &nt)?;
    Ok(nt.slots.len() - 1)
}

fn visible_slot(nt: &Nt, slot: usize) -> Res<()> {
    if nt.visible().iter().any(|(s, _)| *s == slot) { Ok(()) } else { Err("필드를 찾을 수 없어요.".into()) }
}

/// 이름 + 템플릿 참조를 같이 바꾼다
pub(crate) fn rename_field(con: &Connection, id: i64, slot: usize, name: &str) -> Res<()> {
    let mut nt = load(con, id)?;
    nt.editable()?;
    visible_slot(&nt, slot)?;
    let name = field_name(&nt, name, Some(slot))?;
    let old = std::mem::replace(&mut nt.slots[slot], name.clone());
    save_slots(con, &nt)?;
    let now = now_ms();
    for t in &nt.templates {
        let (q, a) = (rename_refs(&t.qfmt, &old, &name), rename_refs(&t.afmt, &old, &name));
        if q != t.qfmt || a != t.afmt {
            con.execute(
                "UPDATE card_template SET qfmt = ?3, afmt = ?4, updated_at = ?5 WHERE note_type_id = ?1 AND ord = ?2",
                params![id, t.ord, q, a, now],
            )
            .map_err(err)?;
        }
    }
    Ok(())
}

/// 보이는 순서만 바꾼다. 자리는 그대로
pub(crate) fn reorder_fields(con: &Connection, id: i64, slots: &[usize]) -> Res<()> {
    let nt = load(con, id)?;
    nt.editable()?;
    let mut now: Vec<usize> = nt.visible().into_iter().map(|(s, _)| s).collect();
    let mut want = slots.to_vec();
    now.sort_unstable();
    want.sort_unstable();
    if now != want {
        return Err("필드 순서가 노트 유형과 맞지 않아요.".into());
    }
    con.execute(
        "UPDATE note_type SET field_order = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, serde_json::to_string(slots).map_err(err)?, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// 자리를 "지움"으로 (이름 ""). 값은 노트에 남고, 화면 · 템플릿 · 내보내기에서 빠진다. 템플릿이 쓰면 거부
pub(crate) fn delete_field(con: &Connection, id: i64, slot: usize) -> Res<()> {
    let mut nt = load(con, id)?;
    nt.editable()?;
    visible_slot(&nt, slot)?;
    if nt.visible().len() < 2 {
        return Err("필드가 하나는 있어야 해요.".into());
    }
    let name = nt.slots[slot].clone();
    if let Some(t) = nt.templates.iter().find(|t| refs(&t.qfmt).contains(&name) || refs(&t.afmt).contains(&name)) {
        return Err(format!("'{}' 카드 종류가 이 필드를 써요. 템플릿에서 먼저 빼 주세요.", t.name));
    }
    nt.slots[slot] = String::new();
    save_slots(con, &nt)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TemplateAdded {
    pub(crate) ord: u32,
    /// 기존 노트에 새로 생긴 카드 수
    pub(crate) cards: usize,
}

/// 카드 종류 더하기. ord 는 지운 번호도 다시 쓰지 않는다. 기존 노트마다 앞면이 비지 않으면 카드를 만든다
pub(crate) fn add_template(con: &Connection, id: i64, name: Option<&str>, qfmt: &str, afmt: &str) -> Res<TemplateAdded> {
    let nt = load(con, id)?;
    nt.editable()?;
    if nt.cloze() {
        return Err("빈칸 유형은 카드 종류를 더할 수 없어요.".into());
    }
    check_template(&nt, qfmt, afmt)?;
    let max = nt.templates.iter().map(|t| t.ord as i64 + 1).max().unwrap_or(0);
    let ord = nt.next_ord.unwrap_or(0).max(max) as u32;
    let name = match name {
        Some(n) => template_name(&nt, n, None)?,
        None => (nt.templates.len() + 1..).map(|i| format!("Card {i}")).find(|n| nt.templates.iter().all(|t| t.name != *n)).unwrap_or_default(),
    };
    let now = now_ms();
    con.execute(
        "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, ord, name, qfmt, afmt, now],
    )
    .map_err(err)?;
    con.execute("UPDATE note_type SET next_ord = ?2, updated_at = ?3 WHERE id = ?1", params![id, ord + 1, now]).map_err(err)?;
    Ok(TemplateAdded { ord, cards: fill_cards(con, id)? })
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TemplateImpact {
    pub(crate) cards: i64,
    /// 그 카드들의 복습 기록 수. 기록은 다른 카드 삭제처럼 card_id 없이 남는다 (A4, 히트맵)
    pub(crate) reviews: i64,
    /// 카드가 하나도 남지 않아 같이 지워지는 노트
    pub(crate) notes: i64,
}

/// 카드 종류 지우기. dry_run 이면 세기만 한다. 마지막 하나는 지울 수 없다
pub(crate) fn delete_template(con: &Connection, id: i64, ord: u32, dry_run: bool) -> Res<TemplateImpact> {
    let nt = load(con, id)?;
    nt.editable()?;
    nt.template(ord)?;
    if nt.templates.len() < 2 {
        return Err("카드 종류가 하나는 있어야 해요.".into());
    }
    let cards = "SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.note_type_id = ?1 AND c.template_ord = ?2";
    let count = |sql: &str| con.query_row(sql, params![id, ord], |r| r.get::<_, i64>(0)).map_err(err);
    let impact = TemplateImpact {
        cards: count(&format!("SELECT count(*) FROM ({cards})"))?,
        reviews: count(&format!("SELECT count(*) FROM review_log WHERE card_id IN ({cards})"))?,
        notes: count(
            "SELECT count(*) FROM note n WHERE n.note_type_id = ?1
               AND EXISTS (SELECT 1 FROM card WHERE note_id = n.id AND template_ord = ?2)
               AND NOT EXISTS (SELECT 1 FROM card WHERE note_id = n.id AND template_ord <> ?2)",
        )?,
    };
    if dry_run {
        return Ok(impact);
    }
    con.execute(&format!("DELETE FROM card WHERE id IN ({cards})"), params![id, ord]).map_err(err)?;
    con.execute("DELETE FROM note WHERE note_type_id = ?1 AND NOT EXISTS (SELECT 1 FROM card WHERE note_id = note.id)", [id]).map_err(err)?;
    con.execute("DELETE FROM card_template WHERE note_type_id = ?1 AND ord = ?2", params![id, ord]).map_err(err)?;
    // 지운 번호를 다시 쓰지 않게 (다른 기기에 남은 옛 카드가 새 템플릿에 붙지 않는다)
    let next = nt.next_ord.unwrap_or(0).max(nt.templates.iter().map(|t| t.ord as i64 + 1).max().unwrap_or(0));
    con.execute("UPDATE note_type SET next_ord = ?2, updated_at = ?3 WHERE id = ?1", params![id, next, now_ms()]).map_err(err)?;
    Ok(impact)
}

/// 앞 · 뒷면 저장. 앞면이 새로 비지 않게 된 노트는 카드를 만든다 (Anki 와 같다). 만든 카드 수
pub(crate) fn save_template(con: &Connection, id: i64, ord: u32, name: Option<&str>, qfmt: &str, afmt: &str) -> Res<usize> {
    let nt = load(con, id)?;
    nt.editable()?;
    let name = match name {
        Some(n) => template_name(&nt, n, Some(ord))?,
        None => nt.template(ord)?.name.clone(),
    };
    check_template(&nt, qfmt, afmt)?;
    con.execute(
        "UPDATE card_template SET name = ?3, qfmt = ?4, afmt = ?5, updated_at = ?6 WHERE note_type_id = ?1 AND ord = ?2",
        params![id, ord, name, qfmt, afmt, now_ms()],
    )
    .map_err(err)?;
    touch(con, id)?;
    fill_cards(con, id)
}

pub(crate) fn save_css(con: &Connection, id: i64, css: &str) -> Res<()> {
    load(con, id)?.editable()?;
    con.execute("UPDATE note_type SET css = ?2, updated_at = ?3 WHERE id = ?1", params![id, css, now_ms()]).map_err(err)?;
    Ok(())
}

#[derive(Deserialize)]
pub struct Draft {
    pub(crate) ord: u32,
    pub(crate) qfmt: String,
}

/// 저장 전 확인 "카드 N장의 앞면이 비어요": 초안 앞면(drafts)과 지울 필드(removed)로 지금은 앞면이 있는데 비게 되는 카드 수
pub(crate) fn empty_front_count(con: &Connection, id: i64, drafts: &[Draft], removed: &[usize]) -> Res<i64> {
    let nt = load(con, id)?;
    let cloze = nt.cloze();
    let qfmt = |ord: u32, draft: bool| -> Option<&str> {
        let ord = if cloze { 0 } else { ord };
        draft.then(|| drafts.iter().find(|d| d.ord == ord).map(|d| d.qfmt.as_str())).flatten().or_else(|| nt.template(ord).ok().map(|t| t.qfmt.as_str()))
    };
    let gone: Vec<&str> = removed.iter().filter_map(|s| nt.slots.get(*s)).map(String::as_str).collect();
    let mut cards: HashMap<i64, Vec<u32>> = HashMap::new();
    {
        let mut st = con
            .prepare("SELECT c.note_id, c.template_ord FROM card c JOIN note n ON n.id = c.note_id WHERE n.note_type_id = ?1")
            .map_err(err)?;
        for r in st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?))).map_err(err)? {
            let (nid, ord) = r.map_err(err)?;
            cards.entry(nid).or_default().push(ord);
        }
    }
    let mut n = 0;
    for (nid, values) in notes_of(con, id)? {
        let before = nt.field_map(&values);
        let mut after = before.clone();
        after.retain(|k, _| !gone.contains(&k.as_str()));
        for &ord in cards.get(&nid).into_iter().flatten() {
            let (Some(q0), Some(q1)) = (qfmt(ord, false), qfmt(ord, true)) else { continue };
            let lost = if cloze { cloze_front(q0, &before, ord) && !cloze_front(q1, &after, ord) } else { has_front(q0, &before) && !has_front(q1, &after) };
            n += i64::from(lost);
        }
    }
    Ok(n)
}

#[derive(Serialize, Debug)]
pub struct Preview {
    front: String,
    back: String,
}

/// 초안 템플릿 미리보기 (render.rs 그대로). values 는 보이는 순서. 없으면 이 유형의 가장 최근 노트, 그것도 없으면 필드 이름
/// (빈칸으로 쓰는 필드는 "{{c1::이름}}")
pub(crate) fn preview(con: &Connection, id: i64, qfmt: &str, afmt: &str, ord: u32, values: Option<&[String]>) -> Res<Preview> {
    let nt = load(con, id)?;
    let slots = match values {
        Some(v) => nt.to_slots(v)?,
        None => {
            let latest: Option<String> =
                con.query_row("SELECT fields FROM note WHERE note_type_id = ?1 ORDER BY id DESC LIMIT 1", [id], |r| r.get(0)).optional().map_err(err)?;
            match latest {
                Some(f) => serde_json::from_str(&f).unwrap_or_default(),
                None => {
                    let cz = cloze_refs(qfmt);
                    nt.slots.iter().map(|n| if cz.contains(n) { format!("{{{{c1::{n}}}}}") } else { format!("({n})") }).collect()
                }
            }
        }
    };
    let r = render::render_card(qfmt, afmt, &nt.field_map(&slots), ord);
    Ok(Preview { front: r.front, back: r.back })
}

#[derive(Deserialize)]
pub struct TypedNote {
    pub(crate) deck_id: i64,
    pub(crate) note_type_id: i64,
    /// 보이는 순서 (note_type_detail 의 fields 와 같다)
    pub(crate) values: Vec<String>,
    pub(crate) importance: Option<i64>,
}

/// 사용자 유형(또는 가져온 유형)으로 노트 추가. 템플릿마다 앞면이 비지 않으면 카드, 빈칸 유형은 번호마다. 만든 카드 수
pub(crate) fn add_note(con: &Connection, input: &TypedNote) -> Res<usize> {
    let nt = load(con, input.note_type_id)?;
    let values = nt.to_slots(&input.values)?;
    if nt.visible().first().is_none_or(|(s, _)| render::is_blank(&values[*s])) {
        return Err("첫 칸을 비울 수 없어요.".into());
    }
    crate::recall::check_fields(&nt.name, &nt.slots, &values)?;
    let ords = card_ords(&nt, &values);
    if ords.is_empty() {
        return Err(if nt.cloze() { "빈칸을 하나 이상 만들어 주세요." } else { "앞면이 모두 비어 카드를 만들 수 없어요." }.into());
    }
    let now = now_ms();
    con.execute(
        "INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![nt.id, serde_json::to_string(&values).map_err(err)?, now],
    )
    .map_err(err)?;
    let nid = con.last_insert_rowid();
    for ord in &ords {
        con.execute(
            "INSERT INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, CASE WHEN ?4 IS NULL THEN NULL ELSE 'user' END, ?5, ?5)",
            params![nid, input.deck_id, ord, input.importance, now],
        )
        .map_err(err)?;
    }
    Ok(ords.len())
}

// ------------------------------------------------------------------ 다시 가져오기 (같은 모델 id 의 유형은 덮지 않는다)

/// 이미 있는 유형에 들어오는 Anki 노트의 필드 i -> 우리 자리. 이름이 같은 보이는 필드, 없으면 같은 위치의 보이는 필드 (이름으로 이미 잡힌 자리는 빼고).
/// 내보낼 때 보이는 순서로 썼으므로, Anki 에서 고치지 않았으면 그대로 돌아온다
/// ponytail: 이름이 하나도 안 맞고 위치도 겹치면 뒤 값이 진다. 필드 이름을 양쪽에서 다르게 바꾼 드문 경우
pub fn incoming_slots(nt: &Nt, names: &[String]) -> Vec<Option<usize>> {
    let vis = nt.visible();
    let by_name: Vec<Option<usize>> = names.iter().map(|n| vis.iter().find(|(_, v)| v == n).map(|(s, _)| *s)).collect();
    by_name.iter().enumerate().map(|(i, m)| m.or_else(|| vis.get(i).map(|(s, _)| *s).filter(|s| !by_name.contains(&Some(*s))))).collect()
}

/// 들어오는 Anki 템플릿 ord -> 우리 ord. 이름이 같은 템플릿, 없으면 같은 순번 (Anki 는 빈틈없이 센다). 빈칸 유형은 비어 있다 (ord = 빈칸 번호)
pub fn incoming_ords(nt: &Nt, tmpls: &[(u32, String)]) -> HashMap<u32, u32> {
    if nt.cloze() {
        return HashMap::new();
    }
    tmpls
        .iter()
        .enumerate()
        .filter_map(|(i, (ord, name))| {
            let ours = nt.templates.iter().find(|t| t.name == *name).or_else(|| nt.templates.get(i))?;
            Some((*ord, ours.ord))
        })
        .collect()
}

// ------------------------------------------------------------------ 쉬운 편집기 (필드를 앞 · 뒷면에 놓기)

/// 쉬운 편집기의 칸 하나. FrontSide 는 뒷면 첫 칸에만 (뒤에 <hr id=answer>)
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Field { name: String },
    Cloze { name: String },
    /// 입력칸 {{type:X}}
    Type { name: String },
    /// 읽어주기 {{tts 언어 옵션:X}}
    Tts { opts: String, name: String },
    FrontSide,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Layout {
    pub front: Vec<Part>,
    pub back: Vec<Part>,
}

/// 칸 사이: 공백 · 줄바꿈 글자 · <br>. (남은 글, <br> 수). HTML 에서 줄바꿈 글자는 줄을 바꾸지 않는다
fn gap(mut s: &str) -> (&str, usize) {
    let mut brs = 0;
    loop {
        let t = s.trim_start();
        let lower = t.get(..6).unwrap_or(t).to_ascii_lowercase();
        match ["<br />", "<br/>", "<br>"].into_iter().find(|b| lower.starts_with(b)) {
            Some(b) => {
                s = &t[b.len()..];
                brs += 1;
            }
            None => return (t, brs),
        }
    }
}

/// 한 면을 칸으로. build_side 가 다시 쓴 것이 같게 보일 때만 (공백 차이만 받는다):
/// 필드 · 빈칸은 한 줄에 하나(<br> 하나로 나눔), 입력칸 · 읽어주기는 앞 칸에 붙음, FrontSide 는 뒷면 첫 칸이고 뒤에 칸이 있으면 <hr id=answer>.
/// 한 줄에 필드 여럿 · 서식 · 조건 구간은 None (HTML 전용, 디자인 NT 결정)
fn parse_side(tpl: &str, back: bool) -> Option<Vec<Part>> {
    let mut parts: Vec<Part> = vec![];
    let (mut rest, lead) = gap(tpl);
    let mut brs = lead;
    let mut hr = false;
    while !rest.is_empty() {
        let inner = rest.strip_prefix("{{")?;
        let end = inner.find("}}")?;
        let tag = inner[..end].trim();
        let part = if tag == "FrontSide" {
            Part::FrontSide
        } else if tag.starts_with(['#', '^', '/']) || SPECIAL.contains(&tag) {
            return None;
        } else {
            match tag.rsplit_once(':') {
                None => Part::Field { name: tag.to_string() },
                Some((f, n)) => {
                    let name = n.trim().to_string();
                    match f.trim() {
                        "cloze" => Part::Cloze { name },
                        "type" => Part::Type { name },
                        f => Part::Tts { opts: f.strip_prefix("tts ")?.trim().to_string(), name },
                    }
                }
            }
        };
        // 이 칸 앞에 와야 하는 것 (build_side 와 같은 규칙)
        let ok = match (parts.last(), &part) {
            (_, Part::FrontSide) => back && parts.is_empty() && brs == 0,
            (None, _) => brs == 0,
            (Some(Part::FrontSide), _) => hr && brs == 0,
            (Some(_), Part::Type { .. } | Part::Tts { .. }) => brs == 0,
            (Some(_), _) => brs == 1,
        };
        if !ok {
            return None;
        }
        rest = &inner[end + 2..];
        hr = false;
        if part == Part::FrontSide {
            let t = rest.trim_start();
            if let Some(after) = ["<hr id=answer>", "<hr id=\"answer\">"].into_iter().find_map(|h| t.strip_prefix(h)) {
                rest = after;
                hr = true;
            }
        }
        parts.push(part);
        (rest, brs) = gap(rest);
    }
    // 끝에 남은 <br> · 뒤에 칸이 없는 <hr> 는 다시 쓰면 사라진다
    if brs > 0 || hr {
        return None;
    }
    (back || !parts.is_empty()).then_some(parts)
}

/// 앞 · 뒷면이 칸 줄로만 되어 있으면 구조, 아니면 None (= HTML 전용). 공백 · <br> 차이는 받는다
pub fn parse_layout(qfmt: &str, afmt: &str) -> Option<Layout> {
    Some(Layout { front: parse_side(qfmt, false)?, back: parse_side(afmt, true)? })
}

fn build_side(parts: &[Part]) -> String {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            match (&parts[i - 1], p) {
                (Part::FrontSide, _) => out.push_str("<hr id=answer>"),
                (_, Part::Type { .. } | Part::Tts { .. }) => {}
                _ => out.push_str("<br>"),
            }
        }
        out.push_str(&match p {
            Part::Field { name } => format!("{{{{{name}}}}}"),
            Part::Cloze { name } => format!("{{{{cloze:{name}}}}}"),
            Part::Type { name } => format!("{{{{type:{name}}}}}"),
            Part::Tts { opts, name } => format!("{{{{tts {opts}:{name}}}}}"),
            Part::FrontSide => "{{FrontSide}}".into(),
        });
    }
    out
}

/// 구조 -> (qfmt, afmt). 칸 사이 <br>, 입력칸 · 읽어주기는 앞 칸에 붙인다. 앱 Basic · 앞뒤 · Cloze 템플릿과 같은 모양
pub fn build_layout(layout: &Layout) -> (String, String) {
    (build_side(&layout.front), build_side(&layout.back))
}

// ------------------------------------------------------------------ 카드 유형 추천 (규칙만, 2단계 제안 칩)

/// 괄호 빈칸 "( )" "[ ]" "（ ）" 또는 밑줄 "___"
fn blank_mark(text: &str) -> bool {
    if text.contains("___") {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(i, c)| {
        let close = match c {
            '(' => ')',
            '[' => ']',
            '（' => '）',
            _ => return false,
        };
        let gap = chars[i + 1..].iter().take_while(|c| c.is_whitespace()).count();
        gap > 0 && chars.get(i + 1 + gap) == Some(&close)
    })
}

/// 붙여 넣은 글(HTML) -> 제안할 카드 유형 (AddCard KINDS: cloze · choice · list · occlusion) 또는 없음. 자동으로 바꾸지 않는다
pub fn suggest_kind(html: &str) -> Option<&'static str> {
    let lines = quality::html_lines(html);
    let text = lines.join("\n");
    if !render::cloze_numbers(html).is_empty() || blank_mark(&text) {
        return Some("cloze");
    }
    let circled = ['①', '②', '③', '④'].iter().all(|c| text.contains(*c));
    let paren = ["1)", "2)", "3)", "4)"].iter().all(|m| lines.iter().any(|l| l.starts_with(m)) || text.contains(&format!(" {m}")));
    if circled || paren {
        return Some("choice");
    }
    if lines.iter().filter(|l| quality::item_marker(l).is_some()).count() >= 3 {
        return Some("list");
    }
    if text.trim().is_empty() && html.to_ascii_lowercase().contains("<img") {
        return Some("occlusion");
    }
    None
}

// ------------------------------------------------------------------ 명령

#[derive(Serialize)]
pub struct TypeRow {
    id: i64,
    name: String,
    /// 앱 기본 유형의 종류. 있으면 읽기 전용 ([복제해서 고치기])
    native_kind: Option<String>,
    editable: bool,
    /// 목록 · 선택형 · 그림 가리기는 복제 버튼이 없다
    cloneable: bool,
    /// Anki 에서 가져온 유형 (앱에서 만든 사용자 유형은 아니다)
    imported: bool,
    /// 앱에서 만든 유형 (디자인 NT1 묶음: 앱 기본 · Anki 에서 가져옴 · 내가 만든 것)
    user_made: bool,
    cloze: bool,
    fields: usize,
    templates: usize,
    notes: i64,
    cards: i64,
    /// 이 유형 카드가 있는 덱 이름 (카드가 많은 덱 먼저)
    decks: Vec<String>,
}

fn row(con: &Connection, nt: &Nt) -> Res<TypeRow> {
    let count = |sql: &str| con.query_row(sql, [nt.id], |r| r.get::<_, i64>(0)).map_err(err);
    Ok(TypeRow {
        id: nt.id,
        name: nt.name.clone(),
        native_kind: nt.native_kind.clone(),
        editable: nt.native_kind.is_none(),
        cloneable: nt.cloneable(),
        imported: nt.native_kind.is_none() && !nt.user_made && nt.anki_model_id.is_some(),
        cloze: nt.cloze(),
        fields: nt.visible().len(),
        templates: nt.templates.len(),
        user_made: nt.user_made,
        notes: count("SELECT count(*) FROM note WHERE note_type_id = ?1")?,
        cards: count("SELECT count(*) FROM card c JOIN note n ON n.id = c.note_id WHERE n.note_type_id = ?1")?,
        decks: {
            let mut st = con
                .prepare(
                    "SELECT d.name FROM card c JOIN note n ON n.id = c.note_id JOIN deck d ON d.id = c.deck_id
                     WHERE n.note_type_id = ?1 GROUP BY d.id ORDER BY count(*) DESC, d.name",
                )
                .map_err(err)?;
            st.query_map([nt.id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
        },
    })
}

pub(crate) fn list(con: &Connection) -> Res<Vec<TypeRow>> {
    let ids: Vec<i64> = {
        let mut st = con.prepare("SELECT id FROM note_type ORDER BY native_kind IS NULL, id").map_err(err)?;
        st.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    ids.into_iter().map(|id| row(con, &load(con, id)?)).collect()
}

#[derive(Serialize)]
pub struct FieldView {
    slot: usize,
    name: String,
}

#[derive(Serialize)]
pub struct TemplateView {
    ord: u32,
    name: String,
    qfmt: String,
    afmt: String,
    /// 쉬운 편집기로 열 수 있으면 구조, 아니면 None (HTML 탭만)
    layout: Option<Layout>,
}

#[derive(Serialize)]
pub struct Detail {
    #[serde(flatten)]
    row: TypeRow,
    css: String,
    /// 보이는 순서
    fields: Vec<FieldView>,
    templates: Vec<TemplateView>,
}

pub(crate) fn detail(con: &Connection, id: i64) -> Res<Detail> {
    let nt = load(con, id)?;
    Ok(Detail {
        row: row(con, &nt)?,
        css: nt.css.clone(),
        fields: nt.visible().into_iter().map(|(slot, name)| FieldView { slot, name }).collect(),
        templates: nt
            .templates
            .iter()
            .map(|t| TemplateView { ord: t.ord, name: t.name.clone(), qfmt: t.qfmt.clone(), afmt: t.afmt.clone(), layout: parse_layout(&t.qfmt, &t.afmt) })
            .collect(),
    })
}

/// 그 덱(하위 덱 포함) 노트가 쓰는 유형, 많이 쓰는 것 먼저 (같으면 최근에 쓴 쪽). 디자인 NT11 [⋯] "이 덱이 쓰는 노트 유형 먼저",
/// NT12 처음 열 유형 = 첫째. "마지막에 쓴 유형"은 화면이 이 기기에만 기억한다 (2단계)
pub(crate) fn deck_types(con: &Connection, deck_id: i64) -> Res<Vec<i64>> {
    let mut st = con
        .prepare(
            "WITH RECURSIVE d(id) AS (SELECT ?1 UNION ALL SELECT deck.id FROM deck JOIN d ON deck.parent_id = d.id)
             SELECT n.note_type_id FROM card c JOIN note n ON n.id = c.note_id WHERE c.deck_id IN (SELECT id FROM d)
             GROUP BY n.note_type_id ORDER BY count(DISTINCT n.id) DESC, max(n.id) DESC",
        )
        .map_err(err)?;
    st.query_map([deck_id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)
}

/// 바꾸는 명령은 모두 한 트랜잭션
fn write<T>(state: &State<AppState>, f: impl FnOnce(&Connection) -> Res<T>) -> Res<T> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let out = f(&tx)?;
    tx.commit().map_err(err)?;
    Ok(out)
}

#[tauri::command]
pub fn note_types(state: State<AppState>) -> Res<Vec<TypeRow>> {
    list(&*state.db.lock().map_err(err)?)
}

#[tauri::command]
pub fn note_type_detail(state: State<AppState>, id: i64) -> Res<Detail> {
    detail(&*state.db.lock().map_err(err)?, id)
}

#[tauri::command]
pub fn deck_note_types(state: State<AppState>, deck_id: i64) -> Res<Vec<i64>> {
    deck_types(&*state.db.lock().map_err(err)?, deck_id)
}

#[tauri::command]
pub fn clone_note_type(state: State<AppState>, id: i64, name: Option<String>) -> Res<i64> {
    write(&state, |c| clone_type(c, id, name.as_deref()))
}

#[tauri::command]
pub fn rename_note_type(state: State<AppState>, id: i64, name: String) -> Res<()> {
    write(&state, |c| rename_type(c, id, &name))
}

#[tauri::command]
pub fn delete_note_type(state: State<AppState>, id: i64) -> Res<()> {
    write(&state, |c| delete_type(c, id))
}

#[tauri::command]
pub fn add_note_field(state: State<AppState>, id: i64, name: String) -> Res<usize> {
    write(&state, |c| add_field(c, id, &name))
}

#[tauri::command]
pub fn rename_note_field(state: State<AppState>, id: i64, slot: usize, name: String) -> Res<()> {
    write(&state, |c| rename_field(c, id, slot, &name))
}

#[tauri::command]
pub fn reorder_note_fields(state: State<AppState>, id: i64, slots: Vec<usize>) -> Res<()> {
    write(&state, |c| reorder_fields(c, id, &slots))
}

#[tauri::command]
pub fn delete_note_field(state: State<AppState>, id: i64, slot: usize) -> Res<()> {
    write(&state, |c| delete_field(c, id, slot))
}

#[tauri::command]
pub fn add_card_template(state: State<AppState>, id: i64, name: Option<String>, qfmt: String, afmt: String) -> Res<TemplateAdded> {
    write(&state, |c| add_template(c, id, name.as_deref(), &qfmt, &afmt))
}

#[tauri::command]
pub fn delete_card_template(state: State<AppState>, id: i64, ord: u32, dry_run: bool) -> Res<TemplateImpact> {
    write(&state, |c| delete_template(c, id, ord, dry_run))
}

#[tauri::command]
pub fn save_card_template(state: State<AppState>, id: i64, ord: u32, name: Option<String>, qfmt: String, afmt: String) -> Res<usize> {
    write(&state, |c| save_template(c, id, ord, name.as_deref(), &qfmt, &afmt))
}

#[tauri::command]
pub fn save_note_type_css(state: State<AppState>, id: i64, css: String) -> Res<()> {
    write(&state, |c| save_css(c, id, &css))
}

#[tauri::command]
pub fn note_type_empty_fronts(state: State<AppState>, id: i64, drafts: Vec<Draft>, removed: Vec<usize>) -> Res<i64> {
    empty_front_count(&*state.db.lock().map_err(err)?, id, &drafts, &removed)
}

#[tauri::command]
pub fn preview_template(state: State<AppState>, id: i64, qfmt: String, afmt: String, ord: u32, values: Option<Vec<String>>) -> Res<Preview> {
    preview(&*state.db.lock().map_err(err)?, id, &qfmt, &afmt, ord, values.as_deref())
}

#[tauri::command]
pub fn add_typed_note(state: State<AppState>, note: TypedNote) -> Res<usize> {
    write(&state, |c| add_note(c, &note))
}

#[tauri::command]
pub fn template_layout(qfmt: String, afmt: String) -> Option<Layout> {
    parse_layout(&qfmt, &afmt)
}

#[derive(Serialize)]
pub struct Built {
    qfmt: String,
    afmt: String,
}

#[tauri::command]
pub fn build_template_layout(layout: Layout) -> Built {
    let (qfmt, afmt) = build_layout(&layout);
    Built { qfmt, afmt }
}

#[tauri::command]
pub fn suggest_card_kind(text: String) -> Option<String> {
    suggest_kind(&text).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn setup() -> (Connection, i64) {
        let con = db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('덱', 0, 0)", []).unwrap();
        let deck = con.last_insert_rowid();
        (con, deck)
    }

    fn note(con: &Connection, deck: i64, nt: i64, values: &[&str]) -> usize {
        add_note(con, &TypedNote { deck_id: deck, note_type_id: nt, values: s(values), importance: None }).unwrap()
    }

    fn q(con: &Connection, sql: &str) -> Vec<String> {
        con.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn visible_skips_deleted_and_keeps_unlisted() {
        let names = s(&["A", "", "C", "D"]);
        assert_eq!(visible(&names, None), [(0, "A".into()), (2, "C".into()), (3, "D".into())]);
        // 다른 기기의 순서(D 가 없던 때 · 지운 자리 1 을 가리킴)와 섞여도 잃지 않고 살아나지 않는다
        assert_eq!(visible(&names, Some("[2,1,0]")), [(2, "C".into()), (0, "A".into()), (3, "D".into())]);
        assert_eq!(visible(&names, Some("[9,3,3]")), [(3, "D".into()), (0, "A".into()), (2, "C".into())], "없는 자리 · 겹침은 버린다");
        assert_eq!(visible(&names, Some("깨짐")), visible(&names, None));
    }

    #[test]
    fn template_refs_rename_and_sections() {
        let t = "{{Front}} {{ #Extra }}<i>{{Extra}}</i>{{/Extra}}{{^Extra}}-{{/Extra}}{{type:Back}}{{tts en_US speed=1.2:Front}}{{FrontSide}}{{Tags}}";
        assert_eq!(refs(t), ["Front", "Extra", "Back"]);
        assert!(sections_ok(t));
        assert!(!sections_ok("{{#A}}x") && !sections_ok("{{#A}}{{/B}}") && !sections_ok("{{/A}}"));
        let r = rename_refs(t, "Extra", "덧붙임");
        assert_eq!(r, "{{Front}} {{ #덧붙임 }}<i>{{덧붙임}}</i>{{/덧붙임}}{{^덧붙임}}-{{/덧붙임}}{{type:Back}}{{tts en_US speed=1.2:Front}}{{FrontSide}}{{Tags}}");
        assert_eq!(rename_refs("{{Front}}{{FrontX}}{{cloze:Front}}", "Front", "앞"), "{{앞}}{{FrontX}}{{cloze:앞}}", "이름이 정확히 같을 때만");
        assert_eq!(cloze_refs("{{cloze:Text}}{{tts ko:cloze:Extra}}{{Back}}"), ["Text", "Extra"]);
    }

    #[test]
    fn has_front_follows_anki_rule() {
        let m = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        assert!(has_front("Q: {{Front}}", &m(&[("Front", "x")])));
        assert!(!has_front("Q: {{Front}}", &m(&[("Front", "&nbsp;<br>")])), "고정 글자만으로는 카드가 생기지 않는다");
        assert!(has_front("{{Front}}", &m(&[("Front", "[sound:a.mp3]")])), "소리만 있는 앞면도 카드");
        assert!(has_front("{{Front}}", &m(&[("Front", "<img src=\"a.png\">")])));
        assert!(!has_front("{{#Extra}}{{Front}}{{/Extra}}", &m(&[("Front", "x"), ("Extra", "")])));
        assert!(has_front("{{#Extra}}{{Front}}{{/Extra}}", &m(&[("Front", "x"), ("Extra", "y")])));
    }

    #[test]
    fn layout_round_trips_and_html_only() {
        // 앱 Basic · 앞뒤 · Cloze · 서술형 앞면은 바이트 그대로 돌아온다
        for (q, a) in [
            ("{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}"),
            ("{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}"),
            ("{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}"),
            ("{{질문}}{{type:모범 답안}}", "{{FrontSide}}"),
            ("{{Front}}{{tts en_US:Front}}<br>{{Extra}}", ""),
        ] {
            let l = parse_layout(q, a).unwrap_or_else(|| panic!("{q} | {a}"));
            assert_eq!(build_layout(&l), (q.to_string(), a.to_string()));
        }
        // Anki 가 만든 Basic 은 공백만 다르다: 구조로 열고 다시 쓰면 정해진 모양
        let l = parse_layout("{{Front}}", "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}").unwrap();
        assert_eq!(l.back, [Part::FrontSide, Part::Field { name: "Back".into() }]);
        // HTML 전용: 조건 구간 · 서식 · 한 줄에 필드 여럿 · 빈 줄 · FrontSide 위치 · 특수 필드
        for (q, a) in [
            ("{{Front}}", "{{FrontSide}}<hr id=answer>{{모범 답안}}{{#채점 메모}}<div>{{채점 메모}}</div>{{/채점 메모}}"),
            ("<b>{{Front}}</b>", ""),
            ("{{Front}} {{Back}}", ""),
            ("{{Front}}{{Back}}", ""),
            ("{{Front}}<br><br>{{Back}}", ""),
            ("{{Front}}<br>", ""),
            ("{{Front}}<br>{{type:Back}}", ""),
            ("{{FrontSide}}", ""),
            ("{{Front}}", "{{Back}}<br>{{FrontSide}}"),
            ("{{Front}}", "{{FrontSide}}{{Back}}"),
            ("{{Front}}", "{{FrontSide}}<hr id=answer>"),
            ("{{text:Front}}", ""),
            ("{{Tags}}", ""),
            ("", ""),
        ] {
            assert_eq!(parse_layout(q, a), None, "{q} | {a}");
        }
        // 구조 -> 글 -> 구조
        let l = Layout {
            front: vec![Part::Field { name: "단어".into() }, Part::Tts { opts: "en_US speed=1.2".into(), name: "단어".into() }, Part::Field { name: "품사".into() }],
            back: vec![Part::FrontSide, Part::Field { name: "뜻".into() }, Part::Type { name: "뜻".into() }, Part::Field { name: "예문".into() }],
        };
        let (q, a) = build_layout(&l);
        assert_eq!(a, "{{FrontSide}}<hr id=answer>{{뜻}}{{type:뜻}}<br>{{예문}}");
        assert_eq!(parse_layout(&q, &a), Some(l));
    }

    #[test]
    fn suggest_kind_rules() {
        assert_eq!(suggest_kind("{{c1::서울}}은 수도"), Some("cloze"));
        assert_eq!(suggest_kind("대한민국의 수도는 (   )이다"), Some("cloze"));
        assert_eq!(suggest_kind("수도는 ___ 이다"), Some("cloze"));
        assert_eq!(suggest_kind("다음 중 옳은 것은?<br>① 가<br>② 나<br>③ 다<br>④ 라"), Some("choice"));
        assert_eq!(suggest_kind("옳은 것은? 1) 가 2) 나 3) 다 4) 라"), Some("choice"));
        assert_eq!(suggest_kind("<div>OSI 하위 계층</div><div>1. 물리</div><div>2. 데이터 링크</div><div>3. 네트워크</div>"), Some("list"));
        assert_eq!(suggest_kind("- 가\n- 나\n- 다"), Some("list"));
        assert_eq!(suggest_kind("<img src=\"heart.png\">"), Some("occlusion"));
        assert_eq!(suggest_kind("결측값 처리 방법은?"), None);
        assert_eq!(suggest_kind("- 가<br>- 나"), None, "두 줄은 목록이 아니다");
        assert_eq!(suggest_kind("f(x) = (x)"), None, "괄호 안에 글자가 있으면 빈칸이 아니다");
    }

    #[test]
    fn native_kinds_seed_once_and_fill_old_databases() {
        let (con, _) = setup();
        let kinds = q(&con, "SELECT native_kind FROM note_type ORDER BY native_kind");
        assert_eq!(kinds, ["basic", "choice", "cloze", "io", "list", "reversed", "written"]);
        // 이 열이 생기기 전 DB (또는 옛 앱 스냅샷): 이름으로 채우고 새로 넣지 않는다. 같은 이름의 가져온 유형은 건드리지 않는다
        con.execute("INSERT INTO note_type (name, fields, created_at, updated_at, anki_model_id) VALUES ('Basic', '[\"F\",\"B\"]', 0, 0, 77)", []).unwrap();
        con.execute("UPDATE note_type SET native_kind = NULL", []).unwrap();
        db::seed_note_types(&con).unwrap();
        db::seed_note_types(&con).unwrap();
        assert_eq!(q(&con, "SELECT count(*) || '' FROM note_type"), ["8"]);
        assert_eq!(q(&con, "SELECT coalesce(native_kind, '-') FROM note_type WHERE anki_model_id = 77"), ["-"]);
        // 앱 유형 이름을 바꿔도 (다른 기기 · 옛 데이터) 다시 시드하지 않는다
        con.execute("UPDATE note_type SET name = 'Basic!' WHERE native_kind = 'basic'", []).unwrap();
        db::seed_note_types(&con).unwrap();
        assert_eq!(q(&con, "SELECT count(*) || '' FROM note_type"), ["8"]);
    }

    #[test]
    fn note_field_names_are_keyed_by_type_id() {
        // 전에는 이름 키라 가져온 "Basic"(필드 셋)이 앱 Basic 을 덮어 앱 노트가 엉뚱한 필드로 그려졌다
        let (con, _) = setup();
        con.execute("INSERT INTO note_type (name, fields, created_at, updated_at, anki_model_id) VALUES ('Basic', '[\"X\",\"Front\",\"Back\"]', 0, 0, 5)", [])
            .unwrap();
        let names = crate::commands::note_field_names(&con).unwrap();
        let basic = db::native_type_id(&con, "basic").unwrap();
        let r = crate::commands::render_fields(&names, basic, r#"["앞","뒤"]"#, Some("{{Front}}"), Some("{{FrontSide}}<hr id=answer>{{Back}}"), 0);
        assert_eq!((r.front.as_str(), r.back.as_str()), ("앞", "앞<hr id=answer>뒤"));
    }

    #[test]
    fn app_types_are_read_only_and_clone_scope() {
        let (con, _) = setup();
        let id = |k: &str| db::native_type_id(&con, k).unwrap();
        for e in [rename_type(&con, id("basic"), "x"), add_field(&con, id("cloze"), "x").map(|_| ()), save_css(&con, id("written"), "x"), delete_type(&con, id("reversed"))] {
            assert_eq!(e, Err(APP_TYPE.to_string()));
        }
        for k in ["list", "choice", "io"] {
            assert!(clone_type(&con, id(k), None).is_err(), "{k} 는 복제하지 않는다");
        }
        let a = clone_type(&con, id("basic"), None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = clone_type(&con, id("basic"), None).unwrap();
        let (na, nb) = (load(&con, a).unwrap(), load(&con, b).unwrap());
        assert_eq!((na.name.as_str(), nb.name.as_str()), ("Basic 복사본", "Basic 복사본 2"));
        assert!(na.user_made && na.native_kind.is_none() && na.stock == Some(1));
        assert!(na.anki_model_id.is_some() && na.anki_model_id != nb.anki_model_id, "만들 때 모델 id 를 정한다");
        assert_eq!(clone_type(&con, a, Some("Basic 복사본 2")), Err("'Basic 복사본 2' 유형이 이미 있어요.".into()));
        let rows = list(&con).unwrap();
        let ra = rows.iter().find(|r| r.id == a).unwrap();
        assert!(ra.editable && ra.cloneable && !ra.imported && ra.templates == 1 && ra.fields == 2);
        assert!(rows.iter().filter(|r| r.native_kind.is_some()).all(|r| !r.editable));
        let w = clone_type(&con, id("written"), Some("내 서술형")).unwrap();
        assert_eq!(load(&con, w).unwrap().templates[0].qfmt, "{{질문}}{{type:모범 답안}}");
        rename_type(&con, w, "  서술형 2 ").unwrap();
        assert_eq!(load(&con, w).unwrap().name, "서술형 2");
        assert!(rename_type(&con, w, "Basic").is_err(), "앱 유형과 같은 이름은 안 된다");
        // 쓰는 노트가 없을 때만 지운다
        delete_type(&con, w).unwrap();
        assert!(load(&con, w).is_err());
    }

    #[test]
    fn fields_keep_their_slots() {
        let (con, deck) = setup();
        let nt = clone_type(&con, db::native_type_id(&con, "basic").unwrap(), Some("단어")).unwrap();
        note(&con, deck, nt, &["apple", "사과"]);
        assert!(delete_type(&con, nt).unwrap_err().contains("노트가 1개"));
        // 추가: 끝에 새 자리, 노트는 그대로
        assert_eq!(add_field(&con, nt, "예문").unwrap(), 2);
        assert_eq!(q(&con, "SELECT fields FROM note"), [r#"["apple","사과"]"#]);
        assert!(add_field(&con, nt, "front").is_err(), "이름은 대소문자 없이 겹치지 않는다");
        assert!(add_field(&con, nt, "a:b").is_err() && add_field(&con, nt, "#x").is_err() && add_field(&con, nt, "Tags").is_err() && add_field(&con, nt, " ").is_err());
        // 이름 바꾸기: 템플릿 참조도
        rename_field(&con, nt, 1, "뜻").unwrap();
        assert_eq!(load(&con, nt).unwrap().templates[0].afmt, "{{FrontSide}}<hr id=answer>{{뜻}}");
        // 순서: 보이는 순서만
        reorder_fields(&con, nt, &[2, 0, 1]).unwrap();
        assert!(reorder_fields(&con, nt, &[0, 1]).is_err());
        let t = load(&con, nt).unwrap();
        assert_eq!(t.slots, ["Front", "뜻", "예문"]);
        assert_eq!(t.visible(), [(2, "예문".into()), (0, "Front".into()), (1, "뜻".into())]);
        // 지우기: 템플릿이 쓰면 거부, 빼면 지운다. 값은 노트에 남고 템플릿에서는 안 보인다
        assert!(delete_field(&con, nt, 1).unwrap_err().contains("Card 1"));
        save_template(&con, nt, 0, None, "{{Front}}", "{{FrontSide}}<hr id=answer>{{예문}}").unwrap();
        delete_field(&con, nt, 1).unwrap();
        let t = load(&con, nt).unwrap();
        assert_eq!(t.slots, ["Front", "", "예문"]);
        assert_eq!(t.field_map(&s(&["apple", "사과"])).len(), 2);
        assert_eq!(q(&con, "SELECT fields FROM note"), [r#"["apple","사과"]"#], "지운 필드 값은 남는다");
        assert!(save_template(&con, nt, 0, None, "{{Front}}", "{{뜻}}").unwrap_err().contains("'뜻' 필드가 없어요"));
        // 같은 이름을 다시 더해도 지운 자리와 겹치지 않는다
        assert_eq!(add_field(&con, nt, "뜻").unwrap(), 3);
        // 보이는 순서로 넣으면 자리에 들어간다 (지운 자리는 빈 값)
        note(&con, deck, nt, &["예문 b", "banana", "바나나"]);
        assert_eq!(q(&con, "SELECT fields FROM note ORDER BY id DESC LIMIT 1"), [r#"["banana","","예문 b","바나나"]"#]);
        assert!(delete_field(&con, nt, 0).is_err(), "앞면이 쓰는 필드");
        // 첫 칸 검사는 보이는 첫 필드 (자리 0 이 아니다)
        assert_eq!(add_note(&con, &TypedNote { deck_id: deck, note_type_id: nt, values: s(&["", "x", ""]), importance: None }), Err("첫 칸을 비울 수 없어요.".into()));
    }

    #[test]
    fn templates_keep_ords_and_make_cards() {
        let (con, deck) = setup();
        let nt = clone_type(&con, db::native_type_id(&con, "basic").unwrap(), None).unwrap();
        add_field(&con, nt, "Extra").unwrap();
        note(&con, deck, nt, &["q1", "a1", ""]);
        note(&con, deck, nt, &["q2", "a2", "e2"]);
        // 앞면이 비지 않는 노트에만 카드 (Anki 규칙). 새 카드는 형제의 덱 · 중요도를 따른다
        con.execute("UPDATE card SET importance = 2, importance_source = 'user'", []).unwrap();
        let added = add_template(&con, nt, None, "{{Extra}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        assert_eq!(added, TemplateAdded { ord: 1, cards: 1 });
        assert_eq!(q(&con, "SELECT importance || ' ' || deck_id FROM card WHERE template_ord = 1"), [format!("2 {deck}")]);
        assert_eq!(load(&con, nt).unwrap().templates[1].name, "Card 2");
        // 노트를 고쳐 앞면이 생기면 카드를 만든다 (지우지는 않는다)
        let first: i64 = con.query_row("SELECT min(id) FROM note", [], |r| r.get(0)).unwrap();
        crate::commands::save_note_fields(&con, first, &s(&["q1", "a1", "e1"])).unwrap();
        crate::commands::save_note_fields(&con, first, &s(&["q1", "a1", ""])).unwrap();
        assert_eq!(q(&con, "SELECT count(*) || '' FROM card WHERE template_ord = 1"), ["2"]);
        // 지우기 dry-run: 카드 · 기록 수. 기록은 card_id 없이 남는다 (A4)
        let c1: i64 = con.query_row("SELECT min(id) FROM card WHERE template_ord = 1", [], |r| r.get(0)).unwrap();
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (?1, 1, 3)", [c1]).unwrap();
        let impact = delete_template(&con, nt, 1, true).unwrap();
        assert_eq!(impact, TemplateImpact { cards: 2, reviews: 1, notes: 0 });
        assert_eq!(q(&con, "SELECT count(*) || '' FROM card"), ["4"], "dry-run 은 지우지 않는다");
        assert_eq!(delete_template(&con, nt, 1, false).unwrap(), impact);
        assert_eq!(q(&con, "SELECT count(*) || '' FROM card"), ["2"]);
        assert_eq!(q(&con, "SELECT count(*) || '' FROM review_log WHERE card_id IS NULL"), ["1"]);
        // 지운 번호는 다시 쓰지 않는다
        assert_eq!(add_template(&con, nt, Some("뒤집기"), "{{Back}}", "{{FrontSide}}").unwrap().ord, 2);
        assert_eq!(q(&con, &format!("SELECT group_concat(ord) FROM card_template WHERE note_type_id = {nt}")), ["0,2"]);
        assert!(add_template(&con, nt, Some("뒤집기"), "{{Back}}", "").is_err(), "이름이 겹친다");
        // 지우면 카드가 없어지는 노트도 같이 지운다 (지금은 둘 다 ord 2 카드가 있다)
        assert_eq!(delete_template(&con, nt, 0, false).unwrap().notes, 0);
        assert!(delete_template(&con, nt, 2, true).is_err(), "마지막 하나");
        assert_eq!(q(&con, "SELECT count(*) || '' FROM note"), ["2"]);
        // 저장 검사
        for (qf, af, want) in [
            ("{{cloze:Front}}", "", "일반 유형에는"),
            ("고정 글자", "", "앞면에 필드를"),
            ("{{Nope}}", "", "'Nope' 필드가 없어요"),
            ("{{Front}}", "{{#Back}}x", "짝이 맞지"),
        ] {
            assert!(add_template(&con, nt, None, qf, af).unwrap_err().contains(want), "{qf}");
        }
        // 빈칸 유형: 템플릿을 더할 수 없고, 앞면의 {{cloze: 를 뺄 수 없다
        let cz = clone_type(&con, db::native_type_id(&con, "cloze").unwrap(), None).unwrap();
        assert!(add_template(&con, cz, None, "{{cloze:Text}}", "").is_err());
        assert!(save_template(&con, cz, 0, None, "{{Text}}", "").unwrap_err().contains("빈칸 유형"));
        assert_eq!(note(&con, deck, cz, &["{{c1::a}} {{c3::b}}", ""]), 2);
        assert_eq!(q(&con, "SELECT group_concat(template_ord) FROM card WHERE note_id = (SELECT max(id) FROM note)"), ["0,2"]);
        assert!(add_note(&con, &TypedNote { deck_id: deck, note_type_id: cz, values: s(&["빈칸 없음", ""]), importance: None }).is_err());
        // 바꿀 때마다 updated_at (Anki mod · 동기화)
        con.execute("UPDATE note_type SET updated_at = 0", []).unwrap();
        con.execute("UPDATE card_template SET updated_at = 0", []).unwrap();
        save_template(&con, nt, 2, Some("뒤"), "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        assert_eq!(q(&con, &format!("SELECT (updated_at > 0) || (SELECT updated_at > 0 FROM card_template WHERE note_type_id = {nt}) FROM note_type WHERE id = {nt}")), ["11"]);
    }

    #[test]
    fn empty_front_count_before_saving() {
        let (con, deck) = setup();
        let nt = clone_type(&con, db::native_type_id(&con, "reversed").unwrap(), None).unwrap();
        note(&con, deck, nt, &["q1", "a1"]);
        note(&con, deck, nt, &["q2", "a2"]);
        let d = |ord, qfmt: &str| Draft { ord, qfmt: qfmt.into() };
        assert_eq!(empty_front_count(&con, nt, &[], &[]).unwrap(), 0);
        assert_eq!(empty_front_count(&con, nt, &[d(1, "고정 글자")], &[]).unwrap(), 2, "Card 2 앞면에서 Back 을 뺐다");
        assert_eq!(empty_front_count(&con, nt, &[d(1, "{{Back}}")], &[1]).unwrap(), 2, "Back 을 지운다");
        assert_eq!(empty_front_count(&con, nt, &[d(0, "{{Front}}{{Back}}"), d(1, "{{Front}}{{Back}}")], &[1]).unwrap(), 0);
        let cz = clone_type(&con, db::native_type_id(&con, "cloze").unwrap(), None).unwrap();
        note(&con, deck, cz, &["{{c1::a}}", "{{c2::b}}"]);
        assert_eq!(empty_front_count(&con, cz, &[d(0, "{{cloze:Back Extra}}")], &[]).unwrap(), 1, "c1 은 Back Extra 에 없다");
    }

    #[test]
    fn preview_and_deck_type() {
        let (con, deck) = setup();
        let nt = clone_type(&con, db::native_type_id(&con, "cloze").unwrap(), None).unwrap();
        let p = preview(&con, nt, "{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}", 0, None).unwrap();
        assert!(p.front.contains("class=\"cloze\"") && p.back.contains("(Back Extra)"), "{p:?}");
        let p = preview(&con, nt, "{{Back Extra}}", "", 0, Some(&s(&["t", "덧붙임"]))).unwrap();
        assert_eq!(p.front, "덧붙임");
        assert!(preview(&con, nt, "", "", 0, Some(&s(&["t"]))).is_err());
        assert!(deck_types(&con, deck).unwrap().is_empty());
        note(&con, deck, nt, &["{{c1::a}}", ""]);
        con.execute("INSERT INTO deck (name, parent_id, created_at, updated_at) VALUES ('하위', ?1, 0, 0)", [deck]).unwrap();
        let sub = con.last_insert_rowid();
        let basic = db::native_type_id(&con, "basic").unwrap();
        for i in 0..2 {
            let card = crate::commands::NewCard { deck_id: sub, kind: "basic".into(), front: format!("q{i}"), back: "a".into(), importance: None };
            crate::commands::insert_note(&con, &card, "", None).unwrap();
        }
        assert_eq!(deck_types(&con, deck).unwrap(), [basic, nt], "하위 덱 노트도 센다. 많이 쓰는 것 먼저");
        assert_eq!(deck_types(&con, sub).unwrap(), [basic]);
        let rows = list(&con).unwrap();
        assert_eq!(rows.iter().find(|r| r.id == basic).unwrap().decks, ["하위"]);
        assert!(rows.iter().find(|r| r.id == nt).is_some_and(|r| r.user_made && r.decks == ["덱"]));
        // 미리보기 기본값: 가장 최근 노트 (이 유형의)
        let p = preview(&con, nt, "{{cloze:Text}}", "", 0, None).unwrap();
        assert!(p.front.starts_with("<span class=\"cloze\">["), "{p:?}");
    }

    #[test]
    fn reimport_maps_fields_and_ords_by_name() {
        let (con, deck) = setup();
        let nt = clone_type(&con, db::native_type_id(&con, "basic").unwrap(), None).unwrap();
        add_field(&con, nt, "Extra").unwrap();
        reorder_fields(&con, nt, &[2, 0, 1]).unwrap();
        add_template(&con, nt, Some("거꾸로"), "{{Back}}", "{{FrontSide}}").unwrap();
        add_template(&con, nt, Some("덧붙임"), "{{Extra}}", "{{FrontSide}}").unwrap();
        delete_template(&con, nt, 1, false).unwrap();
        let t = load(&con, nt).unwrap();
        // 내보낸 순서(보이는 순서) 그대로 돌아오면 자리로, Anki 에서 이름을 바꾼 필드는 같은 위치로
        assert_eq!(incoming_slots(&t, &s(&["Extra", "Front", "Back"])), [Some(2), Some(0), Some(1)]);
        assert_eq!(incoming_slots(&t, &s(&["덧", "Front", "Back", "새 필드"])), [Some(2), Some(0), Some(1), None]);
        // Anki 는 템플릿을 0 부터 빈틈없이 센다: 0 Card 1 · 1 덧붙임 (우리 ord 2)
        let m = incoming_ords(&t, &[(0, "Card 1".into()), (1, "덧붙임".into())]);
        assert_eq!((m[&0], m[&1]), (0, 2));
        let _ = deck;
    }
}
