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
/// 동기화 중 카드 종류 번호의 기기 몫 (add_template)
const ORD_LANES: i64 = 1 << 16;

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
    /// 자리별 필드 id (앱에서 더한 필드만, "시각-기기". 처음부터 있던 자리는 ""). 두 기기가 같은 자리에 따로 더한 필드를 가린다 (Q2-06)
    pub ids: Vec<String>,
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
            "SELECT id, name, fields, field_order, native_kind, user_made, anki_model_id, anki_stock_kind, css, next_ord, field_ids FROM note_type WHERE id = ?1",
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
                    ids: r.get::<_, Option<String>>(10)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
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

/// 앞면이 빈 카드 판정 (Anki 의 "빈 카드"). 노트 유형마다 앞면 템플릿이 쓰는 자리를 한 번 읽어 둔다:
/// 일반 카드는 그 앞면이 쓰는 필드가 모두 비었으면(그림 · 소리는 비지 않았다), 빈칸 카드는 그 번호의 빈칸이 {{cloze:}} 필드에 없으면 빈 앞면.
/// 오늘의 학습 · 회독(압축 · 혼합)이 빼고 카드 목록이 "앞면 빔"을 붙인다 (상태 · 일정은 그대로, 필드를 채우면 다시 나온다).
/// 조건 구간이 숨기는 필드(`{{#A}}{{B}}{{/A}}` 에서 A 가 빈 경우)는 비지 않은 것으로 본다: 빼는 쪽으로는 틀리지 않는다
pub(crate) struct Fronts(HashMap<i64, (bool, HashMap<u32, Vec<usize>>)>);

impl Fronts {
    pub fn load(con: &Connection) -> Res<Fronts> {
        let ids: Vec<i64> = {
            let mut st = con.prepare("SELECT id FROM note_type").map_err(err)?;
            st.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
        };
        let mut out = HashMap::new();
        for id in ids {
            let nt = load(con, id)?;
            let slot: HashMap<String, usize> = nt.visible().into_iter().map(|(s, n)| (n, s)).collect();
            let cloze = nt.cloze();
            let by = nt
                .templates
                .iter()
                .map(|t| (t.ord, if cloze { cloze_refs(&t.qfmt) } else { refs(&t.qfmt) }.iter().filter_map(|n| slot.get(n).copied()).collect()))
                .collect();
            out.insert(id, (cloze, by));
        }
        Ok(Fronts(out))
    }

    /// fields: note.fields JSON (자리 순서)
    pub fn empty(&self, type_id: i64, ord: u32, fields: &str) -> bool {
        let Some((cloze, by)) = self.0.get(&type_id) else { return false };
        let Some(slots) = by.get(&if *cloze { 0 } else { ord }).filter(|s| !s.is_empty()) else { return false };
        let values: Vec<String> = serde_json::from_str(fields).unwrap_or_default();
        let value = |s: &usize| values.get(*s).map_or("", String::as_str);
        if *cloze { !slots.iter().any(|s| render::cloze_numbers(value(s)).contains(&(ord + 1))) } else { slots.iter().all(|s| blank_field(value(s))) }
    }
}

/// 그 덱들(SQL id 목록)에서 앞면이 빈 카드 (id, 상태). 학습 큐가 빼므로(queue::Card::plays) 남은 카드 수 · 하루 몫 · 목표 안내 ·
/// 계획도 이 규칙 하나로 뺀다 (Q1-01). 일시 정지한 카드는 원래 나오지 않아 넣지 않는다
pub(crate) fn front_empty_cards(con: &Connection, decks: &str) -> Res<Vec<(i64, String)>> {
    let fronts = Fronts::load(con)?;
    let mut st = con
        .prepare(&format!(
            "SELECT c.id, c.state, n.note_type_id, c.template_ord, n.fields FROM card c JOIN note n ON n.id = c.note_id
             WHERE c.deck_id IN ({decks}) AND c.state <> 'suspended'"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, u32>(3)?, r.get::<_, String>(4)?)))
        .map_err(err)?;
    let mut out = vec![];
    for row in rows {
        let (id, state, ty, ord, fields) = row.map_err(err)?;
        if fronts.empty(ty, ord, &fields) {
            out.push((id, state));
        }
    }
    Ok(out)
}

/// 그 덱들의 아직 보지 않은 카드 중 학습에 나오는 것 (앞면이 빈 카드는 뺀다). 남은 신규 수는 모두 이것으로 센다 (Q1-01)
pub(crate) fn new_left(con: &Connection, decks: &str) -> Res<i64> {
    let all: i64 = con.query_row(&format!("SELECT count(*) FROM card WHERE deck_id IN ({decks}) AND state = 'new'"), [], |r| r.get(0)).map_err(err)?;
    Ok(all - front_empty_cards(con, decks)?.iter().filter(|(_, s)| s == "new").count() as i64)
}

/// 일반 유형 노트의 빠진 카드를 만든다 (Anki 처럼 만들기만 하고 지우지 않는다). 덱 · 중요도는 그 노트의 첫 카드를 따른다 (C3-r). 만든 수
pub(crate) fn add_missing_cards(con: &Connection, nt: &Nt, note_id: i64, values: &[String]) -> Res<usize> {
    add_missing_cards_where(con, nt, note_id, values, |_| true)
}

/// add_missing_cards 를 고른 카드 종류(ord)에만. 동기화가 받은 노트 · 카드 종류에 쓴다 (Q2-04)
pub(crate) fn add_missing_cards_where(con: &Connection, nt: &Nt, note_id: i64, values: &[String], keep: impl Fn(u32) -> bool) -> Res<usize> {
    if nt.cloze() {
        return Ok(0);
    }
    let have: Vec<u32> = {
        let mut st = con.prepare("SELECT template_ord FROM card WHERE note_id = ?1").map_err(err)?;
        st.query_map([note_id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    let now = now_ms();
    let mut made = 0;
    for ord in card_ords(nt, values).into_iter().filter(|o| !have.contains(o) && keep(*o)) {
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

pub(crate) fn fill_cards(con: &Connection, id: i64) -> Res<usize> {
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
    // id 는 앱에서 더한 필드만 있다. 끝의 빈 id 는 떼고, 하나도 없으면 NULL (sync::engine::ids_value 와 같다)
    let n = nt.ids.iter().rposition(|x| !x.is_empty()).map_or(0, |i| i + 1);
    let ids = (n > 0).then(|| serde_json::to_string(&nt.ids[..n])).transpose().map_err(err)?;
    con.execute(
        "UPDATE note_type SET fields = ?2, field_ids = ?3, updated_at = ?4 WHERE id = ?1",
        params![nt.id, serde_json::to_string(&nt.slots).map_err(err)?, ids, now_ms()],
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

/// 앱 기본 유형의 화면 이름 (카드 추가 세그먼트와 같다). 복제 기본 이름 "내 질문과 답변"에 쓴다
const NATIVE_LABEL: [(&str, &str); 4] = [("basic", "질문과 답변"), ("reversed", "양방향"), ("cloze", "빈칸"), ("written", "서술형")];
/// 앱 유형을 복제하면 필드를 화면 이름으로 만든다 (C3 · E 의 칸 이름, NT54). 서술형은 이미 한글
const CLONE_FIELDS: [(&str, &[(&str, &str)]); 3] = [
    ("basic", &[("Front", "질문"), ("Back", "답변")]),
    ("reversed", &[("Front", "질문"), ("Back", "답변")]),
    ("cloze", &[("Text", "본문"), ("Back Extra", "추가 설명")]),
];

/// [복제해서 고치기]. 지운 자리 · 보이는 순서는 정리해서(보이는 순서대로 새 자리) 옮기고, 템플릿은 0 부터 다시 센다 (아직 카드가 없다).
/// 앱 유형은 필드를 화면 이름(질문 · 답변 등)으로 바꾸고 템플릿 참조도 같이 바꾼다. 원본과 그 카드는 그대로
pub(crate) fn clone_type(con: &Connection, id: i64, name: Option<&str>) -> Res<i64> {
    let src = load(con, id)?;
    if !src.cloneable() {
        return Err("이 유형은 학습 화면이 따로 그리는 앱 유형이라 복제할 수 없어요.".into());
    }
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => type_name(con, n, None)?,
        None => {
            // 앱 유형은 "내 질문과 답변" (세그먼트에서 앞부분으로 구분된다, NT55), 그 밖은 "X 복사본"
            let base = match NATIVE_LABEL.iter().find(|(k, _)| src.native_kind.as_deref() == Some(*k)) {
                Some((_, label)) => format!("내 {label}"),
                None => format!("{} 복사본", src.name),
            };
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
    let renames: &[(&str, &str)] = CLONE_FIELDS.iter().find(|(k, _)| src.native_kind.as_deref() == Some(*k)).map_or(&[], |(_, r)| *r);
    let shown = |n: String| renames.iter().find(|(o, _)| *o == n).map_or(n.clone(), |(_, x)| x.to_string());
    let fields: Vec<String> = src.visible().into_iter().map(|(_, n)| shown(n)).collect();
    let refs_renamed = |t: &str| renames.iter().fold(t.to_string(), |t, (o, x)| rename_refs(&t, o, x));
    // 앱 유형은 쉬운 편집 모양으로 새로 쓴다: 처음부터 모든 줄이 줄 상자라 미리보기 · 학습 답 면이 줄 설정대로다 (NT57).
    // 쉬운 편집이 못 읽는 앱 템플릿(서술형 뒷면의 조건 구간)은 그대로. 사용자 · 가져온 유형은 그대로 복사한다
    let templates: Vec<(String, String)> = src
        .templates
        .iter()
        .map(|t| {
            let (q, a) = (refs_renamed(&t.qfmt), refs_renamed(&t.afmt));
            match layout_of(&q, &a) {
                Ok(l) if src.native_kind.is_some() => build_layout(&l),
                _ => (q, a),
            }
        })
        .collect();
    let css = with_layout_css(&src.css, templates.iter().any(|(q, a)| q.contains(BOX) || a.contains(BOX)));
    let now = now_ms();
    con.execute(
        "INSERT INTO note_type (name, fields, css, created_at, updated_at, anki_model_id, anki_stock_kind, user_made, next_ord)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, 1, ?7)",
        params![name, serde_json::to_string(&fields).map_err(err)?, css, now, new_model_id(con)?, stock, src.templates.len() as i64],
    )
    .map_err(err)?;
    let new = con.last_insert_rowid();
    for (ord, (t, (q, a))) in src.templates.iter().zip(templates).enumerate() {
        con.execute(
            "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![new, ord as i64, t.name, q, a, now],
        )
        .map_err(err)?;
    }
    Ok(new)
}

/// 시험용: 앱 유형을 복제하고 필드를 Anki 이름(Front · Back · Text · Back Extra)으로 되돌린다.
/// 필드 · 템플릿 규칙 테스트가 Anki 이름으로 쓴다 (복제할 때 이름을 바꾸는 것은 app_types_are_read_only_and_clone_scope 가 본다)
#[cfg(test)]
pub(crate) fn anki_clone(con: &Connection, kind: &str, name: Option<&str>) -> i64 {
    let id = clone_type(con, crate::db::native_type_id(con, kind).unwrap(), name).unwrap();
    for (old, new) in CLONE_FIELDS.iter().find(|(k, _)| *k == kind).map_or(&[][..], |(_, r)| *r) {
        let slot = load(con, id).unwrap().visible().into_iter().find(|(_, n)| n == new).unwrap().0;
        rename_field(con, id, slot, old).unwrap();
    }
    id
}

pub(crate) fn rename_type(con: &Connection, id: i64, name: &str) -> Res<()> {
    let nt = load(con, id)?;
    nt.editable()?;
    // 이름을 그대로 두고 저장하면 할 일이 없다 (같은 이름 유형이 동기화로 둘이어도 막지 않는다, Q2-11)
    if name.trim() == nt.name {
        return Ok(());
    }
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
    // 이 필드의 id: 더한 시각 · 기기 (동기화 HLC 와 같은 꼴). 다른 기기가 같은 자리에 더한 필드와 가리고, 그 뒤 이 기기가 쓴 노트 값을 알아본다
    let dev: String = con.query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get(0)).map_err(err)?;
    nt.ids.resize(nt.slots.len(), String::new());
    nt.ids.push(format!("{:013}-00000000-{dev}", now_ms()));
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
    let base = nt.next_ord.unwrap_or(0).max(max);
    // 동기화를 켰으면 기기마다 다른 번호: ord 를 ORD_LANES 로 나눈 나머지가 이 기기 몫이다. 두 기기가 같은 유형에 따로 더해도
    // 같은 ord 로 합쳐져 한쪽 정의가 사라지지 않는다 (Q2-05). 화면 · 내보내기는 ord 값을 보이지 않는다 (내보내기는 0 부터 다시 센다)
    let lane: Option<i64> = con
        .query_row(
            "SELECT m.value FROM schema_meta m, sync_flags f WHERE m.key = 'device_uid' AND f.id = 1 AND f.enabled = 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(err)?
        .map(|dev| i64::from_str_radix(&dev[dev.len().saturating_sub(4)..], 16).unwrap_or(0) % ORD_LANES);
    let ord = match lane {
        Some(l) => base + (l + ORD_LANES - base % ORD_LANES) % ORD_LANES,
        None => base,
    } as u32;
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
/// (빈칸으로 쓰는 필드는 "{{c1::이름}}"). names 를 주면 편집 중인 필드 이름 (이름을 바꾸거나 더한 초안): values 는 그 순서, 없으면 이름 샘플
pub(crate) fn preview(con: &Connection, id: i64, qfmt: &str, afmt: &str, ord: u32, values: Option<&[String]>, names: Option<&[String]>) -> Res<Preview> {
    let nt = load(con, id)?;
    let cz = cloze_refs(qfmt);
    let sample = |n: &String| if cz.contains(n) { format!("{{{{c1::{n}}}}}") } else { format!("({n})") };
    let map = match (names, values) {
        (Some(names), Some(v)) if v.len() != names.len() => return Err("필드 수가 노트 유형과 달라요.".into()),
        (Some(names), Some(v)) => names.iter().cloned().zip(v.iter().cloned()).collect(),
        (Some(names), None) => names.iter().map(|n| (n.clone(), sample(n))).collect(),
        (None, Some(v)) => nt.field_map(&nt.to_slots(v)?),
        (None, None) => {
            let latest: Option<String> =
                con.query_row("SELECT fields FROM note WHERE note_type_id = ?1 ORDER BY id DESC LIMIT 1", [id], |r| r.get(0)).optional().map_err(err)?;
            let slots = match latest {
                Some(f) => serde_json::from_str(&f).unwrap_or_default(),
                None => nt.slots.iter().map(sample).collect::<Vec<_>>(),
            };
            nt.field_map(&slots)
        }
    };
    let r = render::render_card(qfmt, afmt, &map, ord);
    Ok(Preview { front: r.front, back: r.back })
}

#[derive(Serialize)]
pub struct Sample {
    note_id: i64,
    /// 그 노트의 첫 카드 덱
    deck: String,
    /// 자리 순서 값 (편집 중인 필드는 자리로 찾는다)
    slots: Vec<String>,
}

/// 미리보기 카드 고르기 (디자인 NT5 "지금 덱의 카드"): 이 유형의 최근 노트
pub(crate) fn samples(con: &Connection, id: i64, limit: i64) -> Res<Vec<Sample>> {
    let mut st = con
        .prepare(
            "SELECT n.id, n.fields, coalesce((SELECT d.name FROM card c JOIN deck d ON d.id = c.deck_id WHERE c.note_id = n.id ORDER BY c.id LIMIT 1), '')
             FROM note n WHERE n.note_type_id = ?1 ORDER BY n.id DESC LIMIT ?2",
        )
        .map_err(err)?;
    let rows = st
        .query_map(params![id, limit], |r| {
            Ok(Sample { note_id: r.get(0)?, slots: serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default(), deck: r.get(2)? })
        })
        .map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

// ------------------------------------------------------------------ 편집기 저장 (한 트랜잭션, 먼저 dry-run 으로 확인 창의 수를 센다)

#[derive(Deserialize)]
pub struct FieldEdit {
    /// None = 새 필드
    pub(crate) slot: Option<usize>,
    pub(crate) name: String,
}

#[derive(Deserialize)]
pub struct TemplateEdit {
    /// None = 새 카드 종류
    pub(crate) ord: Option<u32>,
    /// None 또는 빈 이름 = 지금 이름 (새것은 "Card N")
    pub(crate) name: Option<String>,
    pub(crate) qfmt: String,
    pub(crate) afmt: String,
}

/// 편집기 초안. fields 는 보이는 순서, 빠진 자리는 지운다. templates 에서 빠진 카드 종류도 지운다
#[derive(Deserialize)]
pub struct TypeEdit {
    pub(crate) fields: Vec<FieldEdit>,
    pub(crate) templates: Vec<TemplateEdit>,
    pub(crate) css: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RemovedTemplate {
    name: String,
    #[serde(flatten)]
    impact: TemplateImpact,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct HiddenField {
    name: String,
    /// 그 필드에 내용이 있는 노트 (지우지 않고 숨긴다, NT10)
    notes: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct SaveImpact {
    /// 지금은 앞면이 있는데 저장하면 비는 카드 (NT26 · 디자인 NT8 ①)
    empty_fronts: i64,
    /// 새로 생기는 카드 (카드 종류를 더했거나 앞면이 새로 생겼다)
    made: usize,
    templates: Vec<RemovedTemplate>,
    fields: Vec<HiddenField>,
}

/// 앞면이 비지 않은 카드 id (빈칸 카드는 그 번호가 {{cloze:}} 필드에 있는지)
fn live_fronts(con: &Connection, nt: &Nt) -> Res<BTreeSet<i64>> {
    let mut st = con
        .prepare("SELECT c.id, c.template_ord, n.fields FROM card c JOIN note n ON n.id = c.note_id WHERE n.note_type_id = ?1")
        .map_err(err)?;
    let rows = st.query_map([nt.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?, r.get::<_, String>(2)?))).map_err(err)?;
    let cloze = nt.cloze();
    let mut out = BTreeSet::new();
    for row in rows {
        let (cid, ord, fields) = row.map_err(err)?;
        let map = nt.field_map(&serde_json::from_str::<Vec<String>>(&fields).unwrap_or_default());
        let Ok(t) = nt.template(if cloze { 0 } else { ord }) else { continue };
        if if cloze { cloze_front(&t.qfmt, &map, ord) } else { has_front(&t.qfmt, &map) } {
            out.insert(cid);
        }
    }
    Ok(out)
}

/// 초안을 한 번에 저장한다 (부르는 쪽이 트랜잭션). 순서: 필드 이름 · 추가 → 카드 종류 저장 · 추가 → 카드 종류 지우기 → 필드 지우기 → 순서 → CSS.
/// 바꾸지 않은 템플릿은 다시 쓰지 않는다 (Anki 가 만든 템플릿의 공백을 그대로 둔다)
/// ponytail: 지운 필드와 같은 이름을 새로 더하거나 두 이름을 맞바꾸면 "이미 있어요"로 거부한다. 두 번 나눠 저장하면 된다
pub(crate) fn apply_edit(con: &Connection, id: i64, edit: &TypeEdit) -> Res<SaveImpact> {
    let nt = load(con, id)?;
    nt.editable()?;
    let before = live_fronts(con, &nt)?;
    for f in &edit.fields {
        if let Some(s) = f.slot
            && nt.slots.get(s).is_some_and(|n| n != f.name.trim())
        {
            rename_field(con, id, s, &f.name)?;
        }
    }
    let mut order = vec![];
    for f in &edit.fields {
        order.push(match f.slot {
            Some(s) => s,
            None => add_field(con, id, &f.name)?,
        });
    }
    let cur = load(con, id)?;
    let mut made = 0;
    for t in &edit.templates {
        let name = t.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        match t.ord {
            Some(ord) => {
                let old = cur.template(ord)?;
                let rename = name.filter(|n| *n != old.name);
                if rename.is_some() || t.qfmt != old.qfmt || t.afmt != old.afmt {
                    made += save_template(con, id, ord, rename, &t.qfmt, &t.afmt)?;
                }
            }
            None => made += add_template(con, id, name, &t.qfmt, &t.afmt)?.cards,
        }
    }
    let mut templates = vec![];
    for t in nt.templates.iter().filter(|t| !edit.templates.iter().any(|e| e.ord == Some(t.ord))) {
        templates.push(RemovedTemplate { name: t.name.clone(), impact: delete_template(con, id, t.ord, false)? });
    }
    let mut fields = vec![];
    for (slot, name) in nt.visible().into_iter().filter(|(s, _)| !order.contains(s)) {
        let notes = notes_of(con, id)?.iter().filter(|(_, v)| v.get(slot).is_some_and(|x| !blank_field(x))).count() as i64;
        delete_field(con, id, slot)?;
        fields.push(HiddenField { name, notes });
    }
    let now = load(con, id)?;
    if now.visible().iter().map(|(s, _)| *s).ne(order.iter().copied()) {
        reorder_fields(con, id, &order)?;
    }
    let css = with_layout_css(&edit.css, now.templates.iter().any(uses_box));
    if css != now.css {
        save_css(con, id, &css)?;
    }
    let after = live_fronts(con, &load(con, id)?)?;
    let alive: BTreeSet<i64> = {
        let mut st = con.prepare("SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.note_type_id = ?1").map_err(err)?;
        st.query_map([id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
    };
    let empty_fronts = before.iter().filter(|c| alive.contains(c) && !after.contains(c)).count() as i64;
    Ok(SaveImpact { empty_fronts, made, templates, fields })
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

/// 이미 있는 유형에 들어오는 Anki 노트의 필드 i -> 우리 자리. 이름이 같은 보이는 필드, 없으면 이 앱에서 지운(숨긴) 같은 자리
/// (가져온 유형의 자리 = 처음 가져온 Anki 순서. 지운 필드의 값도 숨긴 자리에 남긴다, NT10 · Q2-13), 없으면 같은 위치의 보이는 필드
/// (이름으로 이미 잡힌 자리는 빼고). 내보낼 때 보이는 순서로 썼으므로, Anki 에서 고치지 않았으면 그대로 돌아온다
/// ponytail: 이름이 하나도 안 맞고 위치도 겹치면 뒤 값이 진다. 필드 이름을 양쪽에서 다르게 바꾼 드문 경우
pub fn incoming_slots(nt: &Nt, names: &[String]) -> Vec<Option<usize>> {
    let vis = nt.visible();
    let by_name: Vec<Option<usize>> = names.iter().map(|n| vis.iter().find(|(_, v)| v == n).map(|(s, _)| *s)).collect();
    let free = |s: usize| !by_name.contains(&Some(s));
    by_name
        .iter()
        .enumerate()
        .map(|(i, m)| m.or_else(|| (nt.slots.get(i).is_some_and(String::is_empty) && free(i)).then_some(i)).or_else(|| vis.get(i).map(|(s, _)| *s).filter(|s| free(*s))))
        .collect()
}

/// 들어오는 Anki 템플릿 (ord, 이름, 앞면) -> 우리 ord. 이름이 같은 카드 종류, 없으면 앞면이 같은 것, 없으면 같은 ord
/// (가져온 유형의 ord = 처음 가져온 Anki ord, 이름만 이 앱에서 바꿨다). 어느 것도 아니면 빠진다: 이 앱에서 지운 카드 종류라 그 카드는 넣지 않는다
/// (같은 순번으로 대신하면 다른 카드 종류에 붙어 앞면이 빈 카드가 된다, Q2-08). 빈칸 유형은 비어 있다 (ord = 빈칸 번호)
pub fn incoming_ords(nt: &Nt, tmpls: &[(u32, String, String)]) -> HashMap<u32, u32> {
    if nt.cloze() {
        return HashMap::new();
    }
    let mut out: HashMap<u32, u32> = HashMap::new();
    let ways: [&dyn Fn(&Tpl, u32, &str, &str) -> bool; 3] =
        [&|t, _, name, _| t.name == name, &|t, _, _, qfmt| t.qfmt == qfmt, &|t, ord, _, _| t.ord == ord];
    for way in ways {
        for (ord, name, qfmt) in tmpls {
            if out.contains_key(ord) {
                continue;
            }
            let taken: Vec<u32> = out.values().copied().collect();
            if let Some(t) = nt.templates.iter().find(|t| !taken.contains(&t.ord) && way(t, *ord, name, qfmt)) {
                out.insert(*ord, t.ord);
            }
        }
    }
    out
}

// ------------------------------------------------------------------ 쉬운 편집기 (필드를 앞 · 뒷면에 놓기)

/// 줄 글자 크기 (쉬운 편집 S/M/L). 학습 화면 글자와 같은 px: L 30 질문 · M 20 답 · S 13 보조 글
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    S,
    M,
    L,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    Center,
}

/// 줄 모양 (디자인 NT5). 쉬운 편집이 쓰는 줄은 기본 모양이어도 늘 `<div class="nt-크기 nt-정렬">` 로 감싸고, 클래스의 CSS(LAYOUT_CSS)는
/// 노트 유형 CSS 에 넣는다. 그래서 학습 화면(.study__q 가운데) · Anki(.card 가운데) · 덱 CSS 어디서든 줄 설정이 이겨 세 곳이 같다.
/// 감싸지 않은 줄(앱 기본 유형 · 예전 템플릿)은 그 면의 기본(앞면 L · 가운데, 뒷면 M · 왼쪽 = 학습 화면의 질문 · 답)으로 읽는다
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub struct Look {
    pub size: Size,
    pub align: Align,
}

impl Look {
    pub const fn base(back: bool) -> Look {
        if back { Look { size: Size::M, align: Align::Left } } else { Look { size: Size::L, align: Align::Center } }
    }

    fn class(self) -> String {
        let s = match self.size {
            Size::S => "s",
            Size::M => "m",
            Size::L => "l",
        };
        let a = match self.align {
            Align::Left => "left",
            Align::Center => "center",
        };
        format!("nt-{s} nt-{a}")
    }

    /// "nt-크기 nt-정렬" 그대로일 때만
    fn parse(class: &str) -> Option<Look> {
        let (s, a) = class.strip_prefix("nt-")?.split_once(" nt-")?;
        let size = match s {
            "s" => Size::S,
            "m" => Size::M,
            "l" => Size::L,
            _ => return None,
        };
        let align = match a {
            "left" => Align::Left,
            "center" => Align::Center,
            _ => return None,
        };
        Some(Look { size, align })
    }
}

const LAYOUT_MARK: &str = "/* Neo-Anki 쉬운 편집 · 줄 크기와 정렬 */";
/// 줄 상자의 CSS. 템플릿이 줄 상자를 쓰면 저장할 때 노트 유형 CSS 끝에 붙인다 (첫 줄로 이미 있는지 본다).
/// 학습 화면은 노트 CSS 를 카드 안에 가두므로(cardCss.ts scopeCss) 앱 화면은 그대로다
pub const LAYOUT_CSS: &str = "/* Neo-Anki 쉬운 편집 · 줄 크기와 정렬 */\ndiv.nt-s { font-size: 13px; }\ndiv.nt-m { font-size: 20px; }\ndiv.nt-l { font-size: 30px; }\ndiv.nt-left { text-align: left; }\ndiv.nt-center { text-align: center; }\n";
const BOX: &str = "<div class=\"nt-";

/// 템플릿이 줄 상자를 쓰는데 CSS 에 없으면 붙인다
pub fn with_layout_css(css: &str, used: bool) -> String {
    if !used || css.contains(LAYOUT_MARK) {
        css.to_string()
    } else if css.trim().is_empty() {
        LAYOUT_CSS.into()
    } else {
        format!("{}\n\n{LAYOUT_CSS}", css.trim_end())
    }
}

/// 내보내기: 줄 상자 CSS 만 있는 유형은 빈 CSS 처럼 Anki 기본 CSS 를 쓴다 (그 뒤에 줄 상자 CSS)
pub fn without_layout_css(css: &str) -> String {
    css.replace(LAYOUT_CSS, "")
}

fn uses_box(t: &Tpl) -> bool {
    t.qfmt.contains(BOX) || t.afmt.contains(BOX)
}

/// 쉬운 편집기의 칸 하나. FrontSide 는 뒷면 첫 칸에만 (뒤에 <hr id=answer>). 필드 · 빈칸이 줄을 시작하고 look 은 그 줄의 모양 (없으면 면의 기본)
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Field {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        look: Option<Look>,
    },
    Cloze {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        look: Option<Look>,
    },
    /// 입력칸 {{type:X}}
    Type { name: String },
    /// 읽어주기 {{tts 언어 옵션:X}}
    Tts { opts: String, name: String },
    FrontSide,
}

impl Part {
    fn look(&self) -> Option<Look> {
        match self {
            Part::Field { look, .. } | Part::Cloze { look, .. } => *look,
            _ => None,
        }
    }

    /// 입력칸 · 읽어주기는 앞 줄에 붙는다
    fn attaches(&self) -> bool {
        matches!(self, Part::Type { .. } | Part::Tts { .. })
    }

    fn text(&self) -> String {
        match self {
            Part::Field { name, .. } => format!("{{{{{name}}}}}"),
            Part::Cloze { name, .. } => format!("{{{{cloze:{name}}}}}"),
            Part::Type { name } => format!("{{{{type:{name}}}}}"),
            Part::Tts { opts, name } => format!("{{{{tts {opts}:{name}}}}}"),
            Part::FrontSide => "{{FrontSide}}".into(),
        }
    }
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

/// {{…}} 안쪽 -> 칸. 쉬운 편집이 모르는 태그면 Err(이유: HTML 탭의 잠금 줄)
fn tag_part(tag: &str, side: &str) -> Result<Part, String> {
    let why = || format!("{side}에 {{{{{tag}}}}}");
    if tag == "FrontSide" {
        return Ok(Part::FrontSide);
    }
    if tag.starts_with(['#', '^', '/']) {
        return Err(format!("{side}에 조건문 {{{{{tag}}}}}"));
    }
    if SPECIAL.contains(&tag) {
        return Err(why());
    }
    Ok(match tag.rsplit_once(':') {
        None => Part::Field { name: tag.to_string(), look: None },
        Some((f, n)) => {
            let name = n.trim().to_string();
            match f.trim() {
                "cloze" => Part::Cloze { name, look: None },
                "type" => Part::Type { name },
                f => Part::Tts { opts: f.strip_prefix("tts ").ok_or_else(why)?.trim().to_string(), name },
            }
        }
    })
}

/// 칸이 아닌 것 (이유 줄): 태그는 "직접 쓴 <b> 태그", 글자는 "글자 「Q:」"
fn stray(rest: &str, side: &str) -> String {
    match rest.strip_prefix('<') {
        Some(t) => format!("{side}에 직접 쓴 <{}> 태그", t.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '/').collect::<String>()),
        None if rest.starts_with("{{") => format!("{side}에 닫히지 않은 {{{{"),
        None => format!("{side}에 글자 「{}」", rest.chars().take_while(|c| *c != '{' && *c != '<').take(12).collect::<String>().trim()),
    }
}

/// 맨 앞의 {{…}} 하나 -> (칸, 남은 글)
fn take_tag<'a>(rest: &'a str, side: &str) -> Result<(Part, &'a str), String> {
    let inner = rest.strip_prefix("{{").ok_or_else(|| stray(rest, side))?;
    let end = inner.find("}}").ok_or_else(|| stray(rest, side))?;
    Ok((tag_part(inner[..end].trim(), side)?, &inner[end + 2..]))
}

/// 줄 상자 <div class="nt-크기 nt-정렬">…</div> -> (모양, 안쪽, 남은 글)
fn take_box(rest: &str) -> Option<(Look, &str, &str)> {
    let (class, body) = rest.strip_prefix("<div class=\"")?.split_once("\">")?;
    let (inner, after) = body.split_once("</div>")?;
    Some((Look::parse(class)?, inner, after))
}

/// 한 면을 칸으로. build_side 가 다시 쓴 것이 같게 보일 때만 (공백 차이만 받는다), 아니면 Err(이유):
/// 필드 · 빈칸은 한 줄에 하나(<br> 하나로 나눔, 줄 상자 앞뒤에는 <br> 없음), 입력칸 · 읽어주기는 앞 줄에 붙음,
/// FrontSide 는 뒷면 첫 칸이고 뒤에 칸이 있으면 <hr id=answer>. 한 줄에 필드 여럿 · 서식 · 조건 구간은 HTML 전용 (디자인 NT 결정)
fn parse_side(tpl: &str, back: bool) -> Result<Vec<Part>, String> {
    let side = if back { "뒷면" } else { "앞면" };
    let html_div = || format!("{side}에 직접 쓴 <div> 태그");
    let mut parts: Vec<Part> = vec![];
    let (mut rest, mut brs) = gap(tpl);
    let mut hr = false;
    // 앞 줄을 줄 상자로 감쌌다
    let mut boxed = false;
    while !rest.is_empty() {
        let (line, after): (Vec<Part>, &str) = match take_box(rest) {
            Some((look, inner, after)) => {
                let (inner, lead) = gap(inner);
                let (mut first, mut r) = take_tag(inner, side)?;
                match &mut first {
                    Part::Field { look: l, .. } | Part::Cloze { look: l, .. } if lead == 0 => *l = Some(look),
                    _ => return Err(html_div()),
                }
                let mut line = vec![first];
                loop {
                    let (t, b) = gap(r);
                    if t.is_empty() {
                        break;
                    }
                    let (p, next) = take_tag(t, side)?;
                    if b > 0 || !p.attaches() {
                        return Err(html_div());
                    }
                    line.push(p);
                    r = next;
                }
                (line, after)
            }
            None if rest.starts_with(BOX) => return Err(html_div()),
            None => {
                let (p, after) = take_tag(rest, side)?;
                (vec![p], after)
            }
        };
        let part = &line[0];
        let is_box = part.look().is_some();
        // 이 칸 앞에 와야 하는 것 (build_side 와 같은 규칙)
        let why = match (parts.last(), part) {
            (_, Part::FrontSide) if !(back && parts.is_empty() && brs == 0) => Some(if back { "뒷면 중간의 {{FrontSide}}".into() } else { "앞면에 {{FrontSide}}".into() }),
            (_, Part::FrontSide) => None,
            (None, _) => (brs > 0).then(|| format!("{side} 맨 앞의 줄바꿈")),
            (Some(Part::FrontSide), _) if !hr => Some("{{FrontSide}} 뒤에 <hr id=answer> 없이 붙은 칸".into()),
            (Some(Part::FrontSide), _) => (brs > 0).then(|| format!("{side}에 빈 줄")),
            (Some(_), p) if p.attaches() => (brs > 0 || boxed).then(|| format!("{side}에 따로 놓인 {}", p.text())),
            (Some(_), _) => {
                let want = usize::from(!boxed && !is_box);
                (brs != want).then(|| if brs < want { format!("{side} 한 줄에 필드 여러 개") } else { format!("{side}에 빈 줄") })
            }
        };
        if let Some(w) = why {
            return Err(w);
        }
        rest = after;
        hr = false;
        if *part == Part::FrontSide {
            let t = rest.trim_start();
            if let Some(after) = ["<hr id=answer>", "<hr id=\"answer\">"].into_iter().find_map(|h| t.strip_prefix(h)) {
                rest = after;
                hr = true;
            }
        }
        if !part.attaches() || parts.last() == Some(&Part::FrontSide) {
            boxed = is_box;
        }
        parts.extend(line);
        (rest, brs) = gap(rest);
    }
    // 끝에 남은 <br> · 뒤에 칸이 없는 <hr> 는 다시 쓰면 사라진다
    if brs > 0 {
        return Err(format!("{side} 끝의 줄바꿈"));
    }
    if hr {
        return Err("뒷면 끝의 <hr id=answer>".into());
    }
    if !back && parts.is_empty() {
        return Err("빈 앞면".into());
    }
    Ok(parts)
}

/// 앞 · 뒷면이 칸 줄로만 되어 있으면 구조, 아니면 Err(이유) (= HTML 전용). 공백 · <br> 차이는 받는다
pub fn layout_of(qfmt: &str, afmt: &str) -> Result<Layout, String> {
    Ok(Layout { front: parse_side(qfmt, false)?, back: parse_side(afmt, true)? })
}

fn build_side(parts: &[Part], back: bool) -> String {
    let mut out = String::new();
    // 지금 줄을 줄 상자로 감쌌다
    let mut boxed = false;
    for (i, p) in parts.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &parts[j]);
        if !(p.attaches() && prev.is_some_and(|q| *q != Part::FrontSide)) {
            // 새 줄
            if boxed {
                out.push_str("</div>");
            }
            // 필드 · 빈칸 줄은 기본 모양이어도 감싼다 (NT41). 줄 머리가 입력칸 · 읽어주기면(FrontSide 바로 뒤) 상자 없이
            let look = matches!(p, Part::Field { .. } | Part::Cloze { .. }).then(|| p.look().unwrap_or(Look::base(back)));
            match prev {
                Some(Part::FrontSide) => out.push_str("<hr id=answer>"),
                Some(_) if !boxed && look.is_none() => out.push_str("<br>"),
                _ => {}
            }
            boxed = look.is_some();
            if let Some(l) = look {
                out.push_str(&format!("<div class=\"{}\">", l.class()));
            }
        }
        out.push_str(&p.text());
    }
    if boxed {
        out.push_str("</div>");
    }
    out
}

/// 구조 -> (qfmt, afmt). 필드 · 빈칸 줄은 줄 상자, 입력칸 · 읽어주기는 앞 줄(상자 안)에 붙인다, FrontSide 뒤 <hr id=answer>
pub fn build_layout(layout: &Layout) -> (String, String) {
    (build_side(&layout.front, false), build_side(&layout.back, true))
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

/// 붙여 넣은 글(HTML) -> 제안할 카드 유형 (AddCard KINDS: cloze · choice · list · occlusion) 또는 없음. 자동으로 바꾸지 않는다.
/// answer: 답 칸에 글이 있다. 괄호 빈칸 · ___ 은 그 답을 넣어 {{c1::답}} 으로 바꾸므로 답이 있을 때만 빈칸을 권한다 ({{c1::}} 은 늘)
pub fn suggest_kind(html: &str, answer: bool) -> Option<&'static str> {
    let lines = quality::html_lines(html);
    let text = lines.join("\n");
    if !render::cloze_numbers(html).is_empty() || (answer && blank_mark(&text)) {
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
    /// 보이는 필드 수 · 카드 종류 수. 상세(note_type_detail)는 같은 이름의 목록을 주므로 비운다 (JSON 키가 두 번 나오지 않게)
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    templates: Option<usize>,
    notes: i64,
    cards: i64,
    /// 이 유형 카드가 있는 덱 이름 (카드가 많은 덱 먼저)
    decks: Vec<String>,
    /// 만든 시각 · 이 기기에서 만들었나 (같은 이름 유형을 가린다, Q2-11). 다른 기기에서 받은 유형만 false
    created_at: i64,
    made_here: bool,
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
        fields: Some(nt.visible().len()),
        templates: Some(nt.templates.len()),
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
        created_at: count("SELECT created_at FROM note_type WHERE id = ?1")?,
        // 동기화 행 시계의 기기 = 그 행을 만든 기기 (받은 행은 보낸 기기). 동기화 표가 없으면 모두 이 기기 것
        made_here: con
            .query_row(
                "SELECT r.hlc NOT LIKE '%-' || (SELECT value FROM schema_meta WHERE key = 'device_uid') FROM sync_row r JOIN note_type nt ON nt.uid = r.uid
                 WHERE r.table_name = 'note_type' AND nt.id = ?1",
                [nt.id],
                |r| r.get::<_, bool>(0),
            )
            .optional()
            .ok()
            .flatten()
            .is_none_or(|elsewhere| !elsewhere),
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
    /// HTML 전용인 이유 ("뒷면에 조건문 {{#예문}}")
    reason: Option<String>,
}

#[derive(Serialize)]
pub struct Detail {
    #[serde(flatten)]
    row: TypeRow,
    css: String,
    /// 보이는 순서
    fields: Vec<FieldView>,
    templates: Vec<TemplateView>,
    /// 줄 상자 CSS (미리보기가 초안 CSS 에 없으면 붙인다. 저장은 apply_edit 가 붙인다)
    layout_css: &'static str,
    /// 연 때의 정의 (save_note_type 이 확인한다, Q2-07)
    version: String,
}

pub(crate) fn detail(con: &Connection, id: i64) -> Res<Detail> {
    let nt = load(con, id)?;
    Ok(Detail {
        row: TypeRow { fields: None, templates: None, ..row(con, &nt)? },
        css: nt.css.clone(),
        fields: nt.visible().into_iter().map(|(slot, name)| FieldView { slot, name }).collect(),
        templates: nt
            .templates
            .iter()
            .map(|t| {
                let (layout, reason) = match layout_of(&t.qfmt, &t.afmt) {
                    Ok(l) => (Some(l), None),
                    Err(e) => (None, Some(e)),
                };
                TemplateView { ord: t.ord, name: t.name.clone(), qfmt: t.qfmt.clone(), afmt: t.afmt.clone(), layout, reason }
            })
            .collect(),
        layout_css: LAYOUT_CSS,
        version: version(&nt),
    })
}

/// 정의(이름 · 필드 · 순서 · CSS · 카드 종류)의 지문. 편집기가 연 뒤 동기화가 그 유형을 바꿨는지 본다 (Q2-07).
/// updated_at 은 열 단위로 합쳐져 다른 기기의 카드 종류만 바뀌었을 때 그대로일 수 있어 쓰지 않는다
pub(crate) fn version(nt: &Nt) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&nt.name, &nt.slots, &nt.ids, &nt.order, &nt.css, nt.next_ord).hash(&mut h);
    for t in &nt.templates {
        (t.ord, &t.name, &t.qfmt, &t.afmt).hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// 편집기가 연 뒤 다른 기기에서 바뀐 유형은 저장하지 않는다: 낡은 초안에 없는 필드 · 카드 종류를 지우게 된다
pub(crate) const STALE: &str = "다른 기기에서 이 노트 유형이 바뀌었어요. 다시 불러온 뒤 고쳐 주세요.";

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
pub fn preview_template(state: State<AppState>, id: i64, qfmt: String, afmt: String, ord: u32, values: Option<Vec<String>>, names: Option<Vec<String>>) -> Res<Preview> {
    preview(&*state.db.lock().map_err(err)?, id, &qfmt, &afmt, ord, values.as_deref(), names.as_deref())
}

#[tauri::command]
pub fn note_type_samples(state: State<AppState>, id: i64) -> Res<Vec<Sample>> {
    samples(&*state.db.lock().map_err(err)?, id, 20)
}

/// 편집기 [저장]. dry_run 이면 확인 창의 수만 세고 되돌린다 (같은 트랜잭션을 버린다).
/// version: 편집기가 연 때의 정의 (note_type_detail.version). 다르면 저장하지 않는다 (Q2-07)
#[tauri::command]
pub fn save_note_type(state: State<AppState>, id: i64, edit: TypeEdit, dry_run: bool, version: Option<String>) -> Res<SaveImpact> {
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    if version.is_some_and(|v| load(&tx, id).is_ok_and(|nt| v != self::version(&nt))) {
        return Err(STALE.into());
    }
    let out = apply_edit(&tx, id, &edit)?;
    if !dry_run {
        tx.commit().map_err(err)?;
    }
    Ok(out)
}

/// 편집 중인 템플릿들의 필드 이름 바꾸기 (저장할 때 rename_field 와 같은 규칙)
#[tauri::command]
pub fn rename_field_refs(texts: Vec<String>, old: String, new: String) -> Vec<String> {
    texts.iter().map(|t| rename_refs(t, &old, &new)).collect()
}

#[tauri::command]
pub fn add_typed_note(state: State<AppState>, note: TypedNote) -> Res<usize> {
    write(&state, |c| add_note(c, &note))
}

/// 쉬운 편집 구조. HTML 전용이면 Err(이유: "뒷면에 조건문 {{#예문}}")
#[tauri::command]
pub fn template_layout(qfmt: String, afmt: String) -> Result<Layout, String> {
    layout_of(&qfmt, &afmt)
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
pub fn suggest_card_kind(text: String, answer: Option<String>) -> Option<String> {
    suggest_kind(&text, answer.as_deref().is_some_and(|a| !render::is_blank(a))).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn parse_layout(qfmt: &str, afmt: &str) -> Option<Layout> {
        layout_of(qfmt, afmt).ok()
    }

    fn f(name: &str, look: Option<(Size, Align)>) -> Part {
        Part::Field { name: name.into(), look: look.map(|(size, align)| Look { size, align }) }
    }

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
        use Align::*;
        use Size::*;
        // 앱 Basic · 앞뒤 · Cloze · 서술형 앞면은 감싸지 않은 줄이라 그 면의 기본 모양으로 읽는다 (앱 유형은 읽기 전용이라 다시 쓰지 않는다)
        for (q, a, front, back) in [
            ("{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}", vec![f("Front", None)], vec![Part::FrontSide, f("Back", None)]),
            ("{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}", vec![f("Back", None)], vec![Part::FrontSide, f("Front", None)]),
            ("{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}", vec![Part::Cloze { name: "Text".into(), look: None }], vec![Part::Cloze { name: "Text".into(), look: None }, f("Back Extra", None)]),
            ("{{질문}}{{type:모범 답안}}", "{{FrontSide}}", vec![f("질문", None), Part::Type { name: "모범 답안".into() }], vec![Part::FrontSide]),
            ("{{Front}}{{tts en_US:Front}}<br>{{Extra}}", "", vec![f("Front", None), Part::Tts { opts: "en_US".into(), name: "Front".into() }, f("Extra", None)], vec![]),
        ] {
            assert_eq!(parse_layout(q, a), Some(Layout { front, back }), "{q} | {a}");
        }
        // Anki 가 만든 Basic 은 공백만 다르다
        let l = parse_layout("{{Front}}", "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}").unwrap();
        assert_eq!(l.back, [Part::FrontSide, f("Back", None)]);
        // 다시 쓰면 줄마다 상자 (기본 모양 그대로): 앞면 L · 가운데, 뒷면 M · 왼쪽
        let (q, a) = build_layout(&l);
        assert_eq!((q.as_str(), a.as_str()), (r#"<div class="nt-l nt-center">{{Front}}</div>"#, r#"{{FrontSide}}<hr id=answer><div class="nt-m nt-left">{{Back}}</div>"#));
        assert_eq!(parse_layout(&q, &a), Some(Layout { front: vec![f("Front", Some((L, Center)))], back: vec![Part::FrontSide, f("Back", Some((M, Left)))] }));
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
        // 구조 -> 글 -> 구조. 입력칸 · 읽어주기는 상자 안
        let l = Layout {
            front: vec![f("단어", Some((L, Center))), Part::Tts { opts: "en_US speed=1.2".into(), name: "단어".into() }, f("품사", Some((S, Center)))],
            back: vec![Part::FrontSide, f("뜻", Some((M, Left))), Part::Type { name: "뜻".into() }, f("예문", Some((M, Left)))],
        };
        let (q, a) = build_layout(&l);
        assert_eq!(q, r#"<div class="nt-l nt-center">{{단어}}{{tts en_US speed=1.2:단어}}</div><div class="nt-s nt-center">{{품사}}</div>"#);
        assert_eq!(a, r#"{{FrontSide}}<hr id=answer><div class="nt-m nt-left">{{뜻}}{{type:뜻}}</div><div class="nt-m nt-left">{{예문}}</div>"#);
        assert_eq!(parse_layout(&q, &a), Some(l));
    }

    #[test]
    fn layout_line_looks_round_trip() {
        use Align::*;
        use Size::*;
        // 쉬운 편집이 쓰는 필드 · 빈칸 줄은 기본 모양이어도 늘 상자 (NT41). 모양을 정하지 않은 줄은 그 면의 기본으로 쓴다
        let l = Layout {
            front: vec![f("단어", None), Part::Tts { opts: "en_US".into(), name: "단어".into() }, f("소리", Some((S, Center)))],
            back: vec![Part::FrontSide, f("뜻", Some((M, Center))), Part::Type { name: "뜻".into() }, f("예문", Some((S, Left))), f("발음", None)],
        };
        let (q, a) = build_layout(&l);
        assert_eq!(q, r#"<div class="nt-l nt-center">{{단어}}{{tts en_US:단어}}</div><div class="nt-s nt-center">{{소리}}</div>"#);
        assert_eq!(a, r#"{{FrontSide}}<hr id=answer><div class="nt-m nt-center">{{뜻}}{{type:뜻}}</div><div class="nt-s nt-left">{{예문}}</div><div class="nt-m nt-left">{{발음}}</div>"#);
        let back_in = Layout {
            front: vec![f("단어", Some((L, Center))), Part::Tts { opts: "en_US".into(), name: "단어".into() }, f("소리", Some((S, Center)))],
            back: vec![Part::FrontSide, f("뜻", Some((M, Center))), Part::Type { name: "뜻".into() }, f("예문", Some((S, Left))), f("발음", Some((M, Left)))],
        };
        assert_eq!(parse_layout(&q, &a), Some(back_in.clone()));
        // 공백 · 줄바꿈 글자는 받는다
        assert_eq!(parse_layout(&q.replace("<div", "\n<div"), &a.replace("</div>", "</div>\n")), Some(back_in));
        // 예전 템플릿: 감싼 줄과 감싸지 않은 줄이 섞여도 읽는다 (감싸지 않은 줄은 기본 모양)
        let old = parse_layout("{{A}}", r#"{{FrontSide}}<hr id=answer><div class="nt-s nt-left">{{B}}</div>{{C}}<br>{{D}}"#).unwrap();
        assert_eq!(old.back, [Part::FrontSide, f("B", Some((S, Left))), f("C", None), f("D", None)]);
        // 빈칸 줄도 상자, FrontSide 바로 뒤의 입력칸 줄은 상자 없이
        let cz = Layout { front: vec![Part::Cloze { name: "본문".into(), look: None }], back: vec![Part::FrontSide, Part::Type { name: "답".into() }, f("추가 설명", Some((S, Left)))] };
        let (q, a) = build_layout(&cz);
        assert_eq!((q.as_str(), a.as_str()), (r#"<div class="nt-l nt-center">{{cloze:본문}}</div>"#, r#"{{FrontSide}}<hr id=answer>{{type:답}}<div class="nt-s nt-left">{{추가 설명}}</div>"#));
        assert!(parse_layout(&q, &a).is_some());
        // JSON: 모양이 없으면 look 키도 없다 (화면 Part 타입)
        assert_eq!(serde_json::to_string(&f("A", None)).unwrap(), r#"{"kind":"field","name":"A"}"#);
        assert_eq!(serde_json::to_string(&f("A", Some((S, Left)))).unwrap(), r#"{"kind":"field","name":"A","look":{"size":"s","align":"left"}}"#);
        // HTML 전용: 모르는 클래스 · 상자 뒤 <br> · 상자 밖에 붙은 입력칸 · 상자 안 필드 둘
        for (q, a) in [
            (r#"<div class="nt-x nt-left">{{A}}</div>"#, ""),
            (r#"<div class="nt-s nt-left">{{A}}</div><br>{{B}}"#, ""),
            (r#"<div class="nt-s nt-left">{{A}}</div>{{type:A}}"#, ""),
            (r#"<div class="nt-s nt-left">{{A}}<br>{{B}}</div>"#, ""),
            (r#"<div class="nt-s nt-left">{{A}}{{B}}</div>"#, ""),
        ] {
            assert_eq!(parse_layout(q, a), None, "{q} | {a}");
        }
    }

    #[test]
    fn html_only_reasons() {
        let why = |q: &str, a: &str| layout_of(q, a).unwrap_err();
        assert_eq!(why("{{단어}}", "{{FrontSide}}<hr id=answer>{{뜻}}<br>{{#예문}}{{예문}}{{/예문}}"), "뒷면에 조건문 {{#예문}}");
        assert_eq!(why("<b>{{Front}}</b>", ""), "앞면에 직접 쓴 <b> 태그");
        assert_eq!(why("{{Front}}", r#"{{FrontSide}}<hr id=answer><div class="grading-memo">{{Back}}</div>"#), "뒷면에 직접 쓴 <div> 태그");
        assert_eq!(why("Q: {{Front}}", ""), "앞면에 글자 「Q:」");
        assert_eq!(why("{{Front}} {{Back}}", ""), "앞면 한 줄에 필드 여러 개");
        assert_eq!(why("{{Front}}<br><br>{{Back}}", ""), "앞면에 빈 줄");
        assert_eq!(why("{{Tags}}", ""), "앞면에 {{Tags}}");
        assert_eq!(why("{{text:Front}}", ""), "앞면에 {{text:Front}}");
        assert_eq!(why("{{Front}}", "{{Back}}<br>{{FrontSide}}"), "뒷면 중간의 {{FrontSide}}");
        assert_eq!(why("{{Front}}<br>{{type:Back}}", ""), "앞면에 따로 놓인 {{type:Back}}");
        assert_eq!(why("{{Front", ""), "앞면에 닫히지 않은 {{");
    }

    #[test]
    fn apply_edit_in_one_go() {
        let (con, deck) = setup();
        let nt = anki_clone(&con, "reversed", Some("단어"));
        add_field(&con, nt, "메모").unwrap();
        note(&con, deck, nt, &["apple", "사과", "비밀"]);
        note(&con, deck, nt, &["pear", "배", ""]);
        let c2: i64 = con.query_row("SELECT min(id) FROM card WHERE template_ord = 1", [], |r| r.get(0)).unwrap();
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (?1, 1, 3)", [c2]).unwrap();
        let fe = |slot: Option<usize>, name: &str| FieldEdit { slot, name: name.into() };
        let te = |ord: Option<u32>, qfmt: &str, afmt: &str| TemplateEdit { ord, name: None, qfmt: qfmt.into(), afmt: afmt.into() };
        // 이름 바꾸기 + 새 필드 + 순서 + 메모 지우기 + 카드 2 지우기 + 카드 1 앞면을 새 필드로 (앞면이 빈다) + 줄 상자
        let edit = TypeEdit {
            fields: vec![fe(None, "예문"), fe(Some(1), "뜻"), fe(Some(0), "Front")],
            templates: vec![te(Some(0), "{{예문}}", r#"{{FrontSide}}<hr id=answer><div class="nt-s nt-center">{{뜻}}</div>"#)],
            css: ".card { color: red; }".into(),
        };
        let dry = {
            let tx = con.unchecked_transaction().unwrap();
            let out = apply_edit(&tx, nt, &edit).unwrap();
            drop(tx); // dry-run: 되돌린다
            out
        };
        assert_eq!(
            dry,
            SaveImpact {
                empty_fronts: 2,
                made: 0,
                templates: vec![RemovedTemplate { name: "Card 2".into(), impact: TemplateImpact { cards: 2, reviews: 1, notes: 0 } }],
                fields: vec![HiddenField { name: "메모".into(), notes: 1 }],
            }
        );
        assert_eq!(load(&con, nt).unwrap().slots, ["Front", "Back", "메모"], "dry-run 은 남기지 않는다");
        assert_eq!(apply_edit(&con, nt, &edit).unwrap(), dry);
        let t = load(&con, nt).unwrap();
        assert_eq!(t.slots, ["Front", "뜻", "", "예문"]);
        assert_eq!(t.visible().iter().map(|(s, _)| *s).collect::<Vec<_>>(), [3, 1, 0]);
        assert_eq!(t.templates.len(), 1);
        assert_eq!(t.css, format!(".card {{ color: red; }}\n\n{LAYOUT_CSS}"), "줄 상자를 쓰면 CSS 를 붙인다");
        assert_eq!(q(&con, "SELECT count(*) || '' FROM review_log WHERE card_id IS NULL"), ["1"]);
        // 바꾸지 않은 템플릿은 다시 쓰지 않는다 (Anki 공백 그대로). 카드 종류를 더하면 기존 노트에 카드가 생긴다
        let spaced = "{{FrontSide}}\n\n<hr id=answer>\n<div class=\"nt-s nt-center\">{{뜻}}</div>";
        con.execute("UPDATE card_template SET afmt = ?2 WHERE note_type_id = ?1", params![nt, spaced]).unwrap();
        let keep = TypeEdit {
            fields: vec![fe(Some(3), "예문"), fe(Some(1), "뜻"), fe(Some(0), "Front")],
            templates: vec![te(Some(0), "{{예문}}", spaced), te(None, "{{Front}}", "{{FrontSide}}<hr id=answer>{{뜻}}")],
            css: t.css.clone(),
        };
        let out = apply_edit(&con, nt, &keep).unwrap();
        assert_eq!((out.made, out.empty_fronts), (2, 0));
        assert_eq!(load(&con, nt).unwrap().templates[0].afmt, spaced);
        // 앱 유형은 거부
        assert_eq!(apply_edit(&con, db::native_type_id(&con, "basic").unwrap(), &keep), Err(APP_TYPE.to_string()));
    }

    #[test]
    fn empty_fronts_are_found_without_rendering() {
        let (con, deck) = setup();
        let nt = anki_clone(&con, "reversed", None);
        note(&con, deck, nt, &["q", "a"]);
        let cz = anki_clone(&con, "cloze", None);
        note(&con, deck, cz, &["{{c1::a}} {{c2::b}}", ""]);
        let fr = Fronts::load(&con).unwrap();
        let j = |v: &[&str]| serde_json::to_string(&s(v)).unwrap();
        // 카드 2(앞면 {{Back}}): Back 이 비면 빈 앞면. 그림 · 소리만 있으면 비지 않았다
        assert!(!fr.empty(nt, 1, &j(&["q", "a"])));
        assert!(fr.empty(nt, 1, &j(&["q", "&nbsp;<br>"])));
        assert!(!fr.empty(nt, 1, &j(&["q", "<img src=\"a.png\">"])) && !fr.empty(nt, 1, &j(&["q", "[sound:a.mp3]"])));
        assert!(!fr.empty(nt, 0, &j(&["q", ""])), "카드 1 은 Front");
        assert!(fr.empty(nt, 1, &j(&["q"])), "짧은 배열의 없는 칸은 빈 값");
        // 빈칸: 그 번호가 {{cloze:Text}} 에 없으면 빈 앞면 (빈칸을 지운 노트에 남은 카드)
        assert!(!fr.empty(cz, 1, &j(&["{{c1::a}} {{c2::b}}", ""])));
        assert!(fr.empty(cz, 1, &j(&["{{c1::a}} b", "{{c2::x}}"])), "추가 설명의 빈칸은 세지 않는다");
        // 모르는 유형 · 카드 종류는 빼지 않는다
        assert!(!fr.empty(999, 0, &j(&[""])) && !fr.empty(nt, 7, &j(&["", ""])));
        // 큐 (commands::load_cards) 가 표시한다
        crate::commands::save_note_fields(&con, q(&con, &format!("SELECT min(id) || '' FROM note WHERE note_type_id = {nt}"))[0].parse().unwrap(), &s(&["q", ""])).unwrap();
        let cards = crate::commands::load_cards(&con).unwrap();
        let empty: Vec<bool> = cards.iter().filter(|c| c.note_id == cards[0].note_id).map(|c| c.front_empty).collect();
        assert_eq!(empty.len(), 2);
        assert_eq!(empty.iter().filter(|e| **e).count(), 1, "카드 2 만");
    }

    #[test]
    fn preview_with_draft_names_and_samples() {
        let (con, deck) = setup();
        let nt = anki_clone(&con, "basic", None);
        let names = s(&["앞", "새 필드"]);
        let p = preview(&con, nt, "{{앞}}<br>{{새 필드}}", "", 0, None, Some(&names)).unwrap();
        assert_eq!(p.front, "(앞)<br>(새 필드)");
        let p = preview(&con, nt, "{{앞}}", "", 0, Some(&s(&["값", ""])), Some(&names)).unwrap();
        assert_eq!(p.front, "값");
        assert!(preview(&con, nt, "{{앞}}", "", 0, Some(&s(&["값"])), Some(&names)).is_err());
        note(&con, deck, nt, &["q1", "a1"]);
        note(&con, deck, nt, &["q2", "a2"]);
        let got = samples(&con, nt, 1).unwrap();
        assert_eq!((got.len(), got[0].deck.as_str(), got[0].slots.clone()), (1, "덱", s(&["q2", "a2"])));
        assert!(LAYOUT_CSS.starts_with(LAYOUT_MARK));
        assert_eq!(with_layout_css("", false), "");
        assert_eq!(with_layout_css(LAYOUT_CSS, true), LAYOUT_CSS, "두 번 붙이지 않는다");
        assert_eq!(without_layout_css(&with_layout_css(".a {}", true)).trim(), ".a {}");
    }

    #[test]
    fn suggest_kind_rules() {
        assert_eq!(suggest_kind("{{c1::서울}}은 수도", false), Some("cloze"), "빈칸 문법은 답 칸이 비어도");
        assert_eq!(suggest_kind("대한민국의 수도는 (   )이다", true), Some("cloze"));
        assert_eq!(suggest_kind("대한민국의 수도는 (   )이다", false), None, "괄호 빈칸은 넣을 답이 있어야");
        assert_eq!(suggest_kind("수도는 ___ 이다", true), Some("cloze"));
        assert_eq!(suggest_kind("수도는 ___ 이다", false), None);
        assert_eq!(suggest_kind("다음 중 옳은 것은?<br>① 가<br>② 나<br>③ 다<br>④ 라", false), Some("choice"));
        assert_eq!(suggest_kind("옳은 것은? 1) 가 2) 나 3) 다 4) 라", false), Some("choice"));
        assert_eq!(suggest_kind("<div>OSI 하위 계층</div><div>1. 물리</div><div>2. 데이터 링크</div><div>3. 네트워크</div>", false), Some("list"));
        assert_eq!(suggest_kind("- 가\n- 나\n- 다", false), Some("list"));
        assert_eq!(suggest_kind("( )를 고르면<br>- 가<br>- 나<br>- 다", false), Some("list"), "답이 없으면 괄호 빈칸은 건너뛰고 다음 규칙");
        assert_eq!(suggest_kind("<img src=\"heart.png\">", false), Some("occlusion"));
        assert_eq!(suggest_kind("결측값 처리 방법은?", true), None);
        assert_eq!(suggest_kind("- 가<br>- 나", false), None, "두 줄은 목록이 아니다");
        assert_eq!(suggest_kind("f(x) = (x)", true), None, "괄호 안에 글자가 있으면 빈칸이 아니다");
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
        assert_eq!((na.name.as_str(), nb.name.as_str()), ("내 질문과 답변", "내 질문과 답변 2"), "세그먼트에서 앞부분으로 구분되게 (NT55)");
        // 앱 유형 복제본은 필드를 화면 이름으로, 템플릿 참조도 같이 (NT54). 쉬운 편집 모양으로 새로 써 처음부터 모든 줄이 줄 상자이고
        // 노트 CSS 에 줄 상자 CSS 가 들어간다 (NT57). 원본 · 그 카드는 그대로
        let (lq, la) = (|x: &str| format!(r#"<div class="nt-l nt-center">{x}</div>"#), |x: &str| format!(r#"<div class="nt-m nt-left">{x}</div>"#));
        assert_eq!(na.slots, ["질문", "답변"]);
        assert_eq!((na.templates[0].qfmt.clone(), na.templates[0].afmt.clone()), (lq("{{질문}}"), format!("{{{{FrontSide}}}}<hr id=answer>{}", la("{{답변}}"))));
        assert_eq!(na.css, LAYOUT_CSS);
        assert!(parse_layout(&na.templates[0].qfmt, &na.templates[0].afmt).is_some(), "쉬운 편집으로 열린다");
        let basic = load(&con, id("basic")).unwrap();
        assert_eq!((basic.slots.clone(), basic.templates[0].qfmt.as_str(), basic.css.as_str()), (s(&["Front", "Back"]), "{{Front}}", ""));
        let r = load(&con, clone_type(&con, id("reversed"), None).unwrap()).unwrap();
        assert_eq!(r.name, "내 양방향");
        assert_eq!(
            r.templates.iter().map(|t| (t.qfmt.clone(), t.afmt.clone())).collect::<Vec<_>>(),
            [(lq("{{질문}}"), format!("{{{{FrontSide}}}}<hr id=answer>{}", la("{{답변}}"))), (lq("{{답변}}"), format!("{{{{FrontSide}}}}<hr id=answer>{}", la("{{질문}}")))]
        );
        let c = load(&con, clone_type(&con, id("cloze"), None).unwrap()).unwrap();
        assert_eq!(c.slots, ["본문", "추가 설명"]);
        assert_eq!((c.templates[0].qfmt.clone(), c.templates[0].afmt.clone()), (lq("{{cloze:본문}}"), format!("{}{}", la("{{cloze:본문}}"), la("{{추가 설명}}"))));
        assert!(c.cloze(), "빈칸 유형 그대로");
        assert_eq!(load(&con, id("cloze")).unwrap().templates[0].qfmt, "{{cloze:Text}}");
        // 쉬운 편집이 못 읽는 앱 템플릿(서술형 뒷면의 조건 구간)은 그대로 복사하고, 줄 상자가 없으니 CSS 도 그대로
        let wc = load(&con, clone_type(&con, id("written"), Some("서술형 복제 확인")).unwrap()).unwrap();
        let wo = load(&con, id("written")).unwrap();
        assert!(parse_layout(&wo.templates[0].qfmt, &wo.templates[0].afmt).is_none());
        assert_eq!((wc.name.as_str(), wc.templates[0].qfmt.as_str(), wc.templates[0].afmt.as_str(), wc.css.as_str()), ("서술형 복제 확인", wo.templates[0].qfmt.as_str(), wo.templates[0].afmt.as_str(), wo.css.as_str()));
        // 가져온 유형은 그대로 복사한다 (Anki 공백 그대로)
        con.execute("INSERT INTO note_type (name, fields, css, created_at, updated_at, anki_model_id) VALUES ('가져온 Basic', '[\"Front\",\"Back\"]', '.card {}', 0, 0, 42)", []).unwrap();
        let imp = con.last_insert_rowid();
        con.execute("INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, 0, 'Card 1', '{{Front}}', ?2)", params![imp, "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}"]).unwrap();
        let ic = load(&con, clone_type(&con, imp, None).unwrap()).unwrap();
        assert_eq!((ic.name.as_str(), ic.slots.clone(), ic.templates[0].afmt.as_str(), ic.css.as_str()), ("가져온 Basic 복사본", s(&["Front", "Back"]), "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}", ".card {}"));
        assert!(na.user_made && na.native_kind.is_none() && na.stock == Some(1));
        assert!(na.anki_model_id.is_some() && na.anki_model_id != nb.anki_model_id, "만들 때 모델 id 를 정한다");
        assert_eq!(clone_type(&con, a, Some("내 질문과 답변 2")), Err("'내 질문과 답변 2' 유형이 이미 있어요.".into()));
        let copy = load(&con, clone_type(&con, a, None).unwrap()).unwrap();
        assert_eq!((copy.name.as_str(), copy.slots.clone()), ("내 질문과 답변 복사본", s(&["질문", "답변"])), "사용자 유형 복제는 이름 · 필드 그대로");
        let rows = list(&con).unwrap();
        let ra = rows.iter().find(|r| r.id == a).unwrap();
        assert!(ra.editable && ra.cloneable && !ra.imported && ra.templates == Some(1) && ra.fields == Some(2));
        // 상세는 fields · templates 키가 한 번씩 (목록 배열)
        let json = serde_json::to_string(&detail(&con, a).unwrap()).unwrap();
        assert_eq!((json.matches("\"fields\":").count(), json.matches("\"templates\":").count()), (1, 1), "{json}");
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
        let nt = anki_clone(&con, "basic", Some("단어"));
        note(&con, deck, nt, &["apple", "사과"]);
        assert!(delete_type(&con, nt).unwrap_err().contains("노트가 1개"));
        // 추가: 끝에 새 자리, 노트는 그대로
        assert_eq!(add_field(&con, nt, "예문").unwrap(), 2);
        assert_eq!(q(&con, "SELECT fields FROM note"), [r#"["apple","사과"]"#]);
        assert!(add_field(&con, nt, "front").is_err(), "이름은 대소문자 없이 겹치지 않는다");
        assert!(add_field(&con, nt, "a:b").is_err() && add_field(&con, nt, "#x").is_err() && add_field(&con, nt, "Tags").is_err() && add_field(&con, nt, " ").is_err());
        // 이름 바꾸기: 템플릿 참조도
        rename_field(&con, nt, 1, "뜻").unwrap();
        assert_eq!(load(&con, nt).unwrap().templates[0].afmt, r#"{{FrontSide}}<hr id=answer><div class="nt-m nt-left">{{뜻}}</div>"#);
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
        let nt = anki_clone(&con, "basic", None);
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
        let cz = anki_clone(&con, "cloze", None);
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
        let nt = anki_clone(&con, "reversed", None);
        note(&con, deck, nt, &["q1", "a1"]);
        note(&con, deck, nt, &["q2", "a2"]);
        let d = |ord, qfmt: &str| Draft { ord, qfmt: qfmt.into() };
        assert_eq!(empty_front_count(&con, nt, &[], &[]).unwrap(), 0);
        assert_eq!(empty_front_count(&con, nt, &[d(1, "고정 글자")], &[]).unwrap(), 2, "Card 2 앞면에서 Back 을 뺐다");
        assert_eq!(empty_front_count(&con, nt, &[d(1, "{{Back}}")], &[1]).unwrap(), 2, "Back 을 지운다");
        assert_eq!(empty_front_count(&con, nt, &[d(0, "{{Front}}{{Back}}"), d(1, "{{Front}}{{Back}}")], &[1]).unwrap(), 0);
        let cz = anki_clone(&con, "cloze", None);
        note(&con, deck, cz, &["{{c1::a}}", "{{c2::b}}"]);
        assert_eq!(empty_front_count(&con, cz, &[d(0, "{{cloze:Back Extra}}")], &[]).unwrap(), 1, "c1 은 Back Extra 에 없다");
    }

    #[test]
    fn preview_and_deck_type() {
        let (con, deck) = setup();
        let nt = anki_clone(&con, "cloze", None);
        let p = preview(&con, nt, "{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}", 0, None, None).unwrap();
        assert!(p.front.contains("class=\"cloze\"") && p.back.contains("(Back Extra)"), "{p:?}");
        let p = preview(&con, nt, "{{Back Extra}}", "", 0, Some(&s(&["t", "덧붙임"])), None).unwrap();
        assert_eq!(p.front, "덧붙임");
        assert!(preview(&con, nt, "", "", 0, Some(&s(&["t"])), None).is_err());
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
        let p = preview(&con, nt, "{{cloze:Text}}", "", 0, None, None).unwrap();
        assert!(p.front.starts_with("<span class=\"cloze\">["), "{p:?}");
    }

    #[test]
    fn reimport_maps_fields_and_ords_by_name() {
        let (con, deck) = setup();
        let nt = anki_clone(&con, "basic", None);
        add_field(&con, nt, "Extra").unwrap();
        reorder_fields(&con, nt, &[2, 0, 1]).unwrap();
        add_template(&con, nt, Some("거꾸로"), "{{Back}}", "{{FrontSide}}").unwrap();
        add_template(&con, nt, Some("덧붙임"), "{{Extra}}", "{{FrontSide}}").unwrap();
        delete_template(&con, nt, 1, false).unwrap();
        let t = load(&con, nt).unwrap();
        // 내보낸 순서(보이는 순서) 그대로 돌아오면 자리로, Anki 에서 이름을 바꾼 필드는 같은 위치로
        assert_eq!(incoming_slots(&t, &s(&["Extra", "Front", "Back"])), [Some(2), Some(0), Some(1)]);
        assert_eq!(incoming_slots(&t, &s(&["덧", "Front", "Back", "새 필드"])), [Some(2), Some(0), Some(1), None]);
        // Anki 는 템플릿을 0 부터 빈틈없이 센다: 0 Card 1 · 1 덧붙임 (우리 ord 2). Anki 에서 이름만 바꾼 것은 앞면으로
        let q = |o: u32, n: &str, q: &str| (o, n.to_string(), q.to_string());
        let m = incoming_ords(&t, &[q(0, "Card 1", "x"), q(1, "덧붙임", "x")]);
        assert_eq!((m[&0], m[&1]), (0, 2));
        assert_eq!(incoming_ords(&t, &[q(0, "앞", &t.templates[0].qfmt), q(1, "뒤", "{{Extra}}")]), HashMap::from([(0, 0), (1, 2)]));
        let _ = deck;
    }

    #[test]
    fn new_left_skips_empty_front_cards() {
        // Q1-01: 덱 상세 · 목표 안내 · 계획의 남은 신규는 큐와 같이 앞면이 빈 카드를 빼고 센다
        let (con, deck) = setup();
        let nt = anki_clone(&con, "basic", None);
        note(&con, deck, nt, &["a", "b"]);
        note(&con, deck, nt, &["c", "d"]);
        add_field(&con, nt, "힌트").unwrap();
        save_template(&con, nt, 0, None, "{{힌트}}", "{{FrontSide}}").unwrap();
        con.execute("UPDATE note SET fields = json_set(fields, '$[2]', '힌트 있음') WHERE id = (SELECT min(id) FROM note)", []).unwrap();
        let ids = deck.to_string();
        assert_eq!(new_left(&con, &ids).unwrap(), 1);
        assert_eq!(front_empty_cards(&con, &ids).unwrap().len(), 1);
    }

    #[test]
    fn version_changes_with_the_definition_and_same_name_rename_is_a_no_op() {
        let (con, _) = setup();
        let a = anki_clone(&con, "basic", Some("내 단어"));
        let v = version(&load(&con, a).unwrap());
        touch(&con, a).unwrap();
        assert_eq!(version(&load(&con, a).unwrap()), v, "고친 시각만 바뀌면 같다");
        add_template(&con, a, Some("거꾸로"), "{{Back}}", "{{FrontSide}}").unwrap();
        assert_ne!(version(&load(&con, a).unwrap()), v, "카드 종류가 바뀌면 다르다 (Q2-07)");
        // Q2-11: 동기화로 같은 이름 유형이 둘이어도 이름을 그대로 두고 저장하면 막지 않는다
        con.execute("INSERT INTO note_type (name, fields, css, created_at, updated_at, anki_model_id, user_made) VALUES ('내 단어', '[\"Front\"]', '', 5, 5, 99, 1)", []).unwrap();
        rename_type(&con, a, " 내 단어 ").unwrap();
        assert!(rename_type(&con, a, "Basic").is_err(), "다른 유형 이름과는 여전히 겹칠 수 없다");
        let rows = list(&con).unwrap();
        assert!(rows.iter().any(|r| r.created_at == 5 && r.made_here), "만든 시각 · 이 기기에서 만듦 (동기화 전)");
    }

    #[test]
    fn reimport_skips_deleted_card_types_and_keeps_hidden_fields() {
        // 원래 Anki 컬렉션을 다시 가져온다 (가져온 유형: 우리 자리 · ord = 처음 가져온 Anki 순서)
        let (con, _) = setup();
        con.execute(
            "INSERT INTO note_type (name, fields, css, created_at, updated_at, anki_model_id) VALUES ('단어장', '[\"단어\",\"뜻\",\"메모\"]', '', 0, 0, 77)",
            [],
        )
        .unwrap();
        let nt = con.last_insert_rowid();
        for (ord, name, q) in [(0, "단어 → 뜻", "{{단어}}"), (1, "뜻 → 단어", "{{뜻}}")] {
            con.execute("INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, ?2, ?3, ?4, '')", params![nt, ord, name, q]).unwrap();
        }
        add_template(&con, nt, Some("예문 → 단어"), "{{뜻}}!", "").unwrap();
        delete_template(&con, nt, 1, false).unwrap();
        delete_field(&con, nt, 2).unwrap();
        let t = load(&con, nt).unwrap();
        // Q2-08: 이 앱에서 지운 "뜻 → 단어"(Anki ord 1)는 같은 순번의 "예문 → 단어"에 붙지 않고 빠진다
        let q = |o: u32, n: &str, q: &str| (o, n.to_string(), q.to_string());
        let m = incoming_ords(&t, &[q(0, "단어 → 뜻", "{{단어}}"), q(1, "뜻 → 단어", "{{뜻}}")]);
        assert_eq!(m, HashMap::from([(0, 0)]));
        // 이 앱에서 이름만 바꾼 카드 종류는 같은 ord 로
        rename_template_for_test(&con, nt, 0, "앞 → 뒤");
        let t = load(&con, nt).unwrap();
        assert_eq!(incoming_ords(&t, &[q(0, "Card 1", "{{단어}}!?")]), HashMap::from([(0, 0)]));
        // Q2-13: 이 앱에서 지운(숨긴) 필드 "메모"(자리 2)의 값은 숨긴 자리로
        assert_eq!(incoming_slots(&t, &s(&["단어", "뜻", "메모"])), [Some(0), Some(1), Some(2)]);
    }

    fn rename_template_for_test(con: &Connection, id: i64, ord: u32, name: &str) {
        let t = load(con, id).unwrap();
        let tpl = t.template(ord).unwrap();
        save_template(con, id, ord, Some(name), &tpl.qfmt, &tpl.afmt).unwrap();
    }
}
