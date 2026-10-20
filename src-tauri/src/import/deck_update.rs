//! 8.7 덱 업데이트 가져오기 (docs/engineering/deck-update.md). 엔진 1단계: 원본 저장 · 판정 · 적용. 화면은 2단계.
//!
//! - 원본(B) = 마지막으로 받은 버전. `note.origin` = 자리별 해시 + 태그, `note_type.origin` = Anki 필드 → 우리 자리 ·
//!   카드 종류(Anki → 우리 ord, 앞 · 뒷면 해시) · CSS 해시 (DU1). 동기화 열이라 다른 기기도 같은 기준으로 판정한다 (DU2)
//! - 자리마다 원본(B) · 내 것(M) · 새 버전(N) 해시를 비교한다 (판정표 `pick`). 다른 자리는 묻지 않고 합치고(DU5), 충돌의 기본은 내 것,
//!   고른 것은 충돌한 자리에만 적용한다 (DU6). 카드 모양이 충돌하면 내 모양을 지킨다 (DU9)
//! - 사전 검사(inspect_update)와 적용(apply_update)은 같은 `run` 이다. 사전 검사는 한 트랜잭션에서 다 해 보고 되돌린다:
//!   요약이 곧 적용 결과다 (미디어 파일만 쓰지 않는다)
//! - 이미 있는 카드의 일정 · 상태 · 덱 · 중요도와 복습 기록은 쓰지 않는다. 새 .apkg 의 복습 기록도 옮기지 않는다 (DU12)

use super::*;
use crate::notetype::{self, Nt};
use serde::Deserialize;

// ------------------------------------------------------------------ 원본 (B)

/// 해시 전 가벼운 정규화: 편집기가 바꾼 모양(공백 · 줄바꿈 접기, `<br/>` · `<br />`, 끝의 `<br>`, `&nbsp;`)을 내 수정으로 보지 않는다.
/// ponytail: 모양만 다른 HTML 을 다 거르지는 못한다 (속성 순서 · div 감싸기). 못 거른 차이는 "내 것"으로 지켜진다 (위험 ①)
fn norm(s: &str) -> String {
    let s = s.replace("&nbsp;", " ").replace("<br/>", "<br>").replace("<br />", "<br>");
    let mut out = s.split_whitespace().collect::<Vec<_>>().join(" ");
    while let Some(t) = out.strip_suffix("<br>") {
        out = t.trim_end().to_string();
    }
    out
}

/// 정규화한 글의 SHA-256 앞 16자
pub(crate) fn h(s: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(norm(s).as_bytes()))[..16].to_string()
}

fn words(tags: &str) -> String {
    tags.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// note.origin: 자리마다 넣은 값의 해시 ("" = 원본 없음, 빈 값으로 본다), 태그 글 (출처 태그는 뺀 것),
/// 자리마다 넣은 값 그대로 (DU 2단계 "원본 보기", DU53. null = 모른다: 이 기능 전에 받은 원본 · 옛 앱이 다시 쓴 원본).
/// 옛 앱은 모르는 "v" 를 건너뛴다 (serde 기본). 판정은 여전히 해시 "f" 로 한다
#[derive(Serialize, Deserialize, Default)]
struct NoteOrigin {
    f: Vec<String>,
    t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v: Option<Vec<Option<String>>>,
}

impl NoteOrigin {
    /// 그 자리의 원본 글. 해시가 맞을 때만 (다른 길로 f 만 바뀌었으면 믿지 않는다)
    fn value(&self, slot: usize) -> Option<&str> {
        let v = self.v.as_ref()?.get(slot)?.as_deref()?;
        (self.f.get(slot).is_some_and(|x| *x == h(v))).then_some(v)
    }
}

/// 넣은 값(자리 맞추기 · 미디어 이름 바꾸기 뒤)의 원본. 처음 가져올 때와 업데이트를 적용할 때 쓴다
pub(crate) fn note_origin(fields: &[String], tags: &str) -> String {
    serde_json::to_string(&NoteOrigin { f: fields.iter().map(|f| h(f)).collect(), t: words(tags), v: Some(fields.iter().cloned().map(Some).collect()) })
        .unwrap_or_default()
}

/// note_type.origin 의 카드 종류 하나: Anki ord · 이름, 우리 ord (None = 이 앱에서 지웠거나 넣지 못했다), 앞 · 뒷면 해시 (Anki 필드 이름 그대로)
#[derive(Serialize, Deserialize, Clone)]
struct TplOrigin {
    o: u32,
    n: String,
    our: Option<u32>,
    q: String,
    a: String,
}

/// note_type.origin: Anki 필드 (이름, 우리 자리) 를 Anki 순서로, 카드 종류, CSS 해시 (줄 상자 CSS 는 빼고 센다)
#[derive(Serialize, Deserialize)]
struct TypeOrigin {
    f: Vec<(String, Option<usize>)>,
    t: Vec<TplOrigin>,
    c: String,
    /// 이 유형으로 내보낸 Anki 모델 id (anki_model_id 가 없거나 다른 id 로 나갔을 때, DU55). 다시 가져올 때 이것으로 찾는다
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    m: Vec<i64>,
}

/// note_type.origin 의 "m" (f 가 없는 원본에서도 읽는다)
fn exported_ids(origin: Option<&str>) -> Vec<i64> {
    origin.and_then(|o| serde_json::from_str::<serde_json::Value>(o).ok()).and_then(|v| serde_json::from_value(v["m"].clone()).ok()).unwrap_or_default()
}

/// 처음 가져온 유형: 우리 자리 = Anki 순서, 우리 ord = Anki ord
pub(super) fn new_type_origin(t: &AType) -> String {
    let origin = TypeOrigin {
        f: t.fields.iter().enumerate().map(|(i, n)| (n.clone(), Some(i))).collect(),
        t: t.templates.iter().map(|(o, n, q, a)| TplOrigin { o: *o, n: n.clone(), our: Some(*o), q: h(q), a: h(a) }).collect(),
        c: h(&t.css),
        m: vec![],
    };
    serde_json::to_string(&origin).unwrap_or_default()
}

// ------------------------------------------------------------------ 왕복 기준 (내보낼 때, DU55)

/// 앱 기본 유형이 내보낸 모델 id 는 이만큼만 기억한다 (기기마다 다르다: created_at + rowid)
const EXPORTED_IDS: usize = 8;

/// 내보낸 유형의 원본 (DU55, 코디네이터 결정 (A)). 만든 사람이 없는 유형만: 사용자 유형(user_made)은 원본이 없을 때 내보낸 정의로,
/// 앱 기본 유형은 고칠 수 없어 내보낸 모델 id 만 `"m"` 에 (다시 가져올 때 이것으로 찾는다, anki_model_id 는 동기화 자연 키라 건드리지 않는다).
/// 가져온 유형(만든 사람의 원본)은 어떤 경우에도 덮지 않는다
pub(crate) fn exported_type_origin(con: &Connection, ntid: i64, model: &serde_json::Value, slots: &[usize], our_ord: impl Fn(u32) -> u32) -> Res<()> {
    let (native, user_made, model_id, origin): (bool, bool, Option<i64>, Option<String>) = con
        .query_row("SELECT native_kind IS NOT NULL, user_made = 1, anki_model_id, origin FROM note_type WHERE id = ?1", [ntid], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(err)?;
    let mid = model["id"].as_i64().unwrap_or_default();
    let mut v: serde_json::Value = origin.as_deref().and_then(|o| serde_json::from_str(o).ok()).filter(serde_json::Value::is_object).unwrap_or_else(|| serde_json::json!({}));
    let mut changed = false;
    // 사용자 유형은 만든 사람이 없다: 원본이 없으면 내보낸 정의 (앱 기본 유형은 고칠 수 없어 정의를 두지 않는다)
    if !native && user_made && origin.is_none() {
        let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
        let names: Vec<String> = model["flds"].as_array().into_iter().flatten().map(|f| s(&f["name"])).collect();
        let o = TypeOrigin {
            f: names.into_iter().enumerate().map(|(j, n)| (n, slots.get(j).copied())).collect(),
            t: model["tmpls"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| {
                    let ord = x["ord"].as_u64().unwrap_or_default() as u32;
                    TplOrigin { o: ord, n: s(&x["name"]), our: Some(our_ord(ord)), q: h(&s(&x["qfmt"])), a: h(&s(&x["afmt"])) }
                })
                .collect(),
            c: h(&s(&model["css"])),
            m: vec![],
        };
        v = serde_json::to_value(&o).map_err(err)?;
        changed = true;
    }
    // 모델 id 가 이 유형에 남지 않으면(앱 기본 유형 · 모델 id 없는 예전 유형 · 겹쳐 다른 id 로 나감) 내보낸 id 를 "m" 에 (코디네이터 QA: 다시 가져오면 새 유형이 생겼다)
    if model_id != Some(mid) {
        let mut m = exported_ids(origin.as_deref());
        if !m.contains(&mid) {
            m.push(mid);
            let cut = m.len().saturating_sub(EXPORTED_IDS);
            m.drain(..cut);
            v["m"] = serde_json::json!(m);
            changed = true;
        }
    }
    if changed {
        con.execute("UPDATE note_type SET origin = ?2 WHERE id = ?1", params![ntid, v.to_string()]).map_err(err)?;
    }
    Ok(())
}

/// 내보낸 노트의 원본 (DU55): 앱에서 만든 노트(원본 · Anki 노트 id 가 없다)만 처음 내보낼 때. 다시 내보내도 쓰지 않는다
/// (예전에 내보낸 사본을 다시 가져올 때 그 뒤의 앱 수정이 "새 버전"에 덮이지 않게). 가져온 노트의 원본(만든 사람 기준)은 건드리지 않는다
pub(crate) fn exported_note_origin(con: &Connection, note_id: i64, fields: &[String], tags: &str) -> Res<()> {
    con.execute(
        "UPDATE note SET origin = ?2 WHERE id = ?1 AND origin IS NULL AND anki_note_id IS NULL",
        params![note_id, note_origin(fields, &crate::export::without_source_tags(tags))],
    )
    .map_err(err)?;
    Ok(())
}

/// Anki 모델 id 로 우리 유형: 같은 모델 id, 없으면 그 id 로 내보낸 유형 (DU55 "m"). (유형 id, 앱 기본 유형인가)
pub(crate) fn type_of_model(con: &Connection, aid: i64) -> Res<Option<(i64, bool)>> {
    let found = con.query_row("SELECT id FROM note_type WHERE anki_model_id = ?1", [aid], |r| r.get(0)).optional().map_err(err)?;
    if let Some(id) = found {
        return Ok(Some((id, false)));
    }
    con.query_row(
        "SELECT nt.id, nt.native_kind IS NOT NULL FROM note_type nt, json_each(CASE WHEN json_valid(nt.origin) THEN nt.origin ELSE '{}' END, '$.m') m
         WHERE m.value = ?1 ORDER BY nt.id LIMIT 1",
        [aid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(err)
}

// ------------------------------------------------------------------ 판정 (순수 함수)

#[derive(Debug, Clone, Copy, PartialEq)]
enum Pick {
    /// 그대로 (B = M = N · 둘이 같게 고침 · 원본 없이 같음): 원본만 새로 쓴다
    Same,
    /// 새 버전 (만든 사람만 고침)
    New,
    /// 내 것 (나만 고침)
    Mine,
    /// 둘 다 다르게 고침
    Conflict,
}

/// 판정표 (deck-update.md 2장 3). b: 원본 해시 (None = 원본이 없다), m: 내 것, n: 새 버전.
/// 원본이 없으면 다를 때 가져온 뒤 고친 흔적(touched)이 없으면 새 버전, 있으면 충돌 (DU18)
fn pick(b: Option<&str>, m: &str, n: &str, touched: bool) -> Pick {
    match b {
        _ if m == n => Pick::Same,
        Some(b) if b == m => Pick::New,
        Some(b) if b == n => Pick::Mine,
        Some(_) => Pick::Conflict,
        None if touched => Pick::Conflict,
        None => Pick::New,
    }
}

/// 태그 집합 3-way: (M − (B − N)) ∪ (N − B). 원본이 없으면 B = M 으로 본다 (앱에 태그 편집이 없다): 결과는 N
fn merge_tags(b: Option<&str>, m: &str, n: &str) -> String {
    let b: Vec<&str> = b.unwrap_or(m).split_whitespace().collect();
    let n: Vec<&str> = n.split_whitespace().collect();
    let mut out: Vec<&str> = m.split_whitespace().filter(|t| !b.contains(t) || n.contains(t)).collect();
    for t in &n {
        if !b.contains(t) && !out.contains(t) {
            out.push(t);
        }
    }
    out.join(" ")
}

/// 태그가 기출 표시로 읽히는 값 (exam::fill_from_tags 와 같다)
fn exam_of(tags: &str) -> (Option<i64>, Option<String>) {
    crate::exam::from_tags(tags).map_or((None, None), |(hits, names)| (Some(hits), crate::exam::names_json(&names)))
}

// ------------------------------------------------------------------ 요청 · 요약

/// B2-k · B2-kt 에서 고른 것. 기본은 모두 내 것 (DU6 · DU9)
#[derive(Deserialize, Default)]
pub struct Choices {
    /// 충돌한 필드를 모두 새 버전으로 할 노트 (우리 노트 id). 같은 노트의 충돌 없는 변경은 고르기와 상관없이 합친다
    #[serde(default)]
    pub take_new: Vec<i64>,
    /// 필드마다 고르기 (DU2-k, DU54): 새 버전으로 할 충돌 필드 [노트 id, 자리]. take_new 와 합친다 (어느 쪽에 있어도 새 버전)
    #[serde(default)]
    pub take_new_fields: Vec<(i64, usize)>,
    /// 새 모양으로 할 카드 종류 [노트 유형 id, ord]. ord 가 null 이면 CSS
    #[serde(default)]
    pub new_looks: Vec<(i64, Option<u32>)>,
}

/// B2-u0 업데이트로 알아본 근거
#[derive(Serialize, Default)]
pub struct UpdateHint {
    /// 이 파일(고른 덱)의 노트
    notes: usize,
    /// 그중 이미 있는 노트 (같은 guid)
    existing_notes: usize,
    /// 같은 Anki 덱 id 로 가져온 우리 덱의 최상위 덱 이름
    known_decks: Vec<String>,
    /// 절반 이상이 이미 있거나 같은 덱이 있으면 업데이트가 기본 (DU15)
    suggested: bool,
}

#[derive(Serialize)]
pub struct ConflictField {
    slot: usize,
    name: String,
    mine: String,
    new: String,
    /// 원본 글 (지난번에 받은 값, DU53). None = "원본 글이 없어요" (예전에 가져온 노트)
    base: Option<String>,
    /// 지금 고른 것 (기본 false = 내 것)
    take_new: bool,
}

/// B2-k 충돌 노트 한 줄
#[derive(Serialize)]
pub struct ConflictNote {
    note_id: i64,
    /// 첫 필드 글 (지금 값)
    title: String,
    /// 첫 카드의 덱 경로 ("정보처리기사 › 데이터베이스")
    deck: String,
    fields: Vec<ConflictField>,
    /// 같은 노트에서 충돌 없이 새 버전으로 바뀌는 필드 · 내 것으로 지키는 필드 (태그 포함)
    auto_new: usize,
    auto_mine: usize,
    /// 그 새 버전으로 바뀌는 필드 이름 ("태그" 포함)
    auto_new_fields: Vec<String>,
    /// 내가 마지막으로 고친 시각 (note.updated_at, ms)
    edited_at: i64,
    /// 충돌 필드 하나라도 새 버전으로 골랐다 (기본 false = 내 것). 필드마다는 fields[].take_new
    take_new: bool,
    /// 원본이 없어 어림으로 충돌로 본 노트 (DU18)
    no_base: bool,
    /// 앱 기본 유형이면 그 종류와 자리 이름: 화면이 카드 추가 · 편집과 같은 칸 이름으로 보인다 (NT51, Back → 답변)
    native_kind: Option<String>,
    slot_names: Vec<String>,
}

/// B2-kt 카드 모양 충돌 하나 (둘 다 바꿨다)
#[derive(Serialize)]
pub struct LookConflict {
    /// 카드 종류 ord. None 이면 CSS (front 에 CSS, back 은 비어 있다)
    ord: Option<u32>,
    name: String,
    mine_front: String,
    mine_back: String,
    new_front: String,
    new_back: String,
    take_new: bool,
    /// 원본과 달라진 면: 내 것 · 새 버전 ("앞면" · "뒷면" · "CSS")
    mine_changed: Vec<&'static str>,
    new_changed: Vec<&'static str>,
    /// 이 앱에서 그 카드 종류를 마지막으로 고친 시각 (CSS 는 없다)
    edited_at: Option<i64>,
}

#[derive(Serialize, Default)]
pub struct TypeChange {
    note_type_id: i64,
    name: String,
    /// 이 유형의 노트 수 (B2-kt "이 유형 노트 n개")
    notes: usize,
    /// B2-kt 미리보기: 지금 CSS · 새 버전 CSS (쉬운 편집 줄 상자 CSS 를 붙인 것)
    css_mine: String,
    css_new: String,
    /// 이번에 새로 만든 노트 유형
    created: bool,
    /// 원본이 없는 예전 유형: 새 필드 · 카드 종류를 내가 지운 것과 구별하지 못해 넣지 않는다 (NT29 · Q2-08 · Q2-13)
    no_base: bool,
    new_fields: Vec<String>,
    /// (원래 이름, 새 이름)
    renamed_fields: Vec<(String, String)>,
    /// 새 버전에서 없어진 필드 (값은 남긴다)
    removed_fields: Vec<String>,
    new_templates: Vec<String>,
    /// 새 카드 종류로 생기는 카드
    new_template_cards: usize,
    /// 새 버전에서 없어진 카드 종류 (카드 · 기록 그대로)
    removed_templates: Vec<String>,
    /// 원본이 없는 예전 유형: 새 버전의 필드 · 카드 종류 중 내가 지운 것과 구별하지 못해 넣지 않는 것 (DU22 알림)
    skipped_fields: Vec<String>,
    skipped_templates: Vec<String>,
    /// 만든 사람만 바꿔 새 모양으로 바뀌는 카드 종류 이름 (CSS 는 "CSS")
    updated_looks: Vec<String>,
    /// 둘 다 바꾼 모양 (기본 내 모양, DU9)
    looks: Vec<LookConflict>,
}

impl TypeChange {
    fn is_empty(&self) -> bool {
        !self.created
            && self.new_fields.is_empty()
            && self.renamed_fields.is_empty()
            && self.removed_fields.is_empty()
            && self.new_templates.is_empty()
            && self.removed_templates.is_empty()
            && self.skipped_fields.is_empty()
            && self.skipped_templates.is_empty()
            && self.updated_looks.is_empty()
            && self.looks.is_empty()
    }
}

#[derive(Serialize, Default)]
pub struct ClozeChange {
    /// 빈칸 글 묶음은 같은데 번호만 바뀐 것 같은 노트
    renumbered_notes: usize,
    /// 새 번호로 생기는 신규 카드
    new_cards: usize,
    /// 없어진 번호라 학습에서 빠지는 카드 (기록 · 일정 보존, DU13)
    emptied_cards: usize,
}

/// B2-u 요약 = B3-u 결과. 적용 전 영향을 모두 보인다 (사용자 요청 2026-10-18)
#[derive(Serialize, Default)]
pub struct UpdateSummary {
    pub(crate) file_name: String,
    pub(crate) hint: UpdateHint,
    /// 새 버전으로 바뀌는 노트 (만든 사람만 고쳤다)
    pub(crate) changed: usize,
    /// 합친 노트 (만든 사람이 고친 필드와 내가 고친 필드가 다르다, DU5)
    pub(crate) merged: usize,
    /// 충돌 노트 (같은 필드를 둘 다 고쳤다)
    pub(crate) conflicts: Vec<ConflictNote>,
    /// 내가 고친 것만 있어 그대로 두는 노트
    pub(crate) mine_kept: usize,
    /// 바뀐 것이 없는 노트
    pub(crate) same: usize,
    /// 비교할 원본이 없어 어림으로 판정한 노트 (DU18)
    pub(crate) no_base: usize,
    /// 노트 유형이 달라 고치지 못한 노트 (만든 사람이 바꿨거나 내가 다른 유형으로 옮겼다. 누가 바꿨는지는 모른다)
    pub(crate) other_type: usize,
    pub(crate) new_notes: usize,
    pub(crate) new_note_cards: usize,
    /// 새 버전에 없는 노트 (그대로 두고 학습에도 나온다, DU11). [카드 목록에서 보기] 의 카드
    pub(crate) missing_notes: usize,
    pub(crate) missing_card_ids: Vec<i64>,
    /// 내가 지운 노트 (다시 넣지 않는다, DU3)
    pub(crate) gone: usize,
    /// 이미 있는 노트에 새로 생기는 카드 (새 카드 종류 · 빈칸 새 번호 · 앞면이 새로 생김). 신규로 시작한다
    pub(crate) new_cards: usize,
    /// 앞면이 비어 학습에서 빠지는 카드 (지우지 않는다. 기록 · 일정 보존, NT49)
    pub(crate) emptied_cards: usize,
    pub(crate) cloze: ClozeChange,
    pub(crate) types: Vec<TypeChange>,
    /// 내 모양을 지킨 노트 유형 (DU9)
    pub(crate) kept_looks: usize,
    /// 새로 저장할 미디어 파일, 그중 이름은 같은데 내용이 달라 새 이름으로 저장하는 것 (DU10)
    pub(crate) media_new: usize,
    pub(crate) media_renamed: usize,
    /// 이미 같은 이름의 파일이 있어 내용을 비교한 미디어 (B2-u0 "이름이 같은 n개는 내용을 비교해요")
    pub(crate) media_compared: usize,
    /// 같은 guid 의 노트가 두 벌 이상 있는 노트 (두 기기가 받지 않은 채 같은 파일을 적용했다, DU17). 첫 번째 것만 고친다. [카드 목록에서 보기]
    pub(crate) duplicate_notes: usize,
    pub(crate) duplicate_card_ids: Vec<i64>,
    /// 새 노트를 넣으려고 새로 만드는 덱 (이 기기에 없는 Anki 덱)
    pub(crate) new_decks: usize,
    /// 사전 검사만: 충돌 필드의 새 버전이 쓰는데 아직 저장하지 않은 그림 (이름 -> data URL, 합쳐 8MB 까지). B2-k 가 나란히 보인다
    pub(crate) media_preview: HashMap<String, String>,
    /// 옮기지 않는 새 .apkg 의 복습 기록 (DU12)
    pub(crate) reviews_skipped: usize,
    pub(crate) issues: Vec<Issue>,
    /// 적용했으면 그 전에 만든 백업 (DU16)
    pub(crate) backup: Option<String>,
    /// 업데이트한 덱 · 새 노트가 들어간 덱의 최상위 덱 (결과의 [덱 열기])
    pub(crate) roots: Vec<i64>,
    /// 적용했으면 그 업데이트 (B3-u [이 업데이트 되돌리기], undo_update)
    pub(crate) batch: Option<i64>,
}

// ------------------------------------------------------------------ 읽기

struct PNote {
    id: i64,
    guid: String,
    mid: i64,
    modified: i64,
    tags: String,
    flds: String,
}

struct PCard {
    id: i64,
    did: i64,
    odid: i64,
    ord: u32,
    queue: i64,
}

/// 이미 있는 노트 (같은 guid)
struct Ours {
    id: i64,
    ntid: i64,
    fields_raw: String,
    fields: Vec<String>,
    tags: String,
    origin: Option<String>,
    updated_at: i64,
    exam: (Option<i64>, Option<String>),
}

/// Anki 노트타입 하나가 들어갈 곳
struct TypeMap {
    ntid: i64,
    /// Anki 필드 j -> 우리 자리 (None = 넣을 자리가 없다)
    slots: Vec<Option<usize>>,
    /// Anki 카드 ord -> 우리 ord (빈칸 유형은 쓰지 않는다: ord = 빈칸 번호 - 1). 없으면 이 앱에서 지운 카드 종류
    ords: HashMap<u32, u32>,
    cloze: bool,
    /// 이번에 더한 카드 종류 (우리 ord)
    new_ords: Vec<u32>,
}

/// 같은 guid 비율 · 같은 Anki 덱 id (DU15)
fn hint(con: &Connection, guids: &[&str], decks: &BTreeSet<i64>) -> Res<UpdateHint> {
    let mut st = con.prepare_cached("SELECT (SELECT c.deck_id FROM card c WHERE c.note_id = n.id ORDER BY c.id LIMIT 1) FROM note n WHERE n.anki_guid = ?1 LIMIT 1").map_err(err)?;
    let mut existing = 0;
    let mut their_decks: HashSet<i64> = HashSet::new();
    for g in guids {
        if let Some(d) = st.query_row([g], |r| r.get::<_, Option<i64>>(0)).optional().map_err(err)? {
            existing += 1;
            their_decks.extend(d);
        }
    }
    let same_deck = known_decks(con, decks)?;
    let suggested = existing > 0 && (existing * 2 >= guids.len() || !same_deck.is_empty());
    // 그 덱들의 최상위 덱 이름 (요약 · 결과의 덱 "…"). 같은 Anki 덱 id 가 없으면(내 덱을 내보냈다 다시 가져옴, Q4 L4) 이미 있는 노트의 덱
    let mut known: Vec<String> = vec![];
    for id in roots_of(con, if same_deck.is_empty() { &their_decks } else { &same_deck })? {
        known.push(con.query_row("SELECT name FROM deck WHERE id = ?1", [id], |r| r.get(0)).map_err(err)?);
    }
    Ok(UpdateHint { notes: guids.len(), existing_notes: existing, suggested, known_decks: known })
}

/// 이번 패키지 덱(Anki 덱 id)으로 가져온 우리 덱
fn known_decks(con: &Connection, decks: &BTreeSet<i64>) -> Res<HashSet<i64>> {
    let list = decks.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    con.prepare(&format!("SELECT id FROM deck WHERE anki_deck_id IN ({list})")).and_then(|mut s| s.query_map([], |r| r.get(0))?.collect()).map_err(err)
}

/// inspect_anki (B2): 이미 있는 노트가 하나라도 있으면 [업데이트로 가져오기] 를 보일 근거
pub(super) fn file_hint(con: &Connection, pkg: &Package) -> Res<Option<UpdateHint>> {
    let guids: Vec<String> = pkg.col.prepare("SELECT guid FROM notes").and_then(|mut s| s.query_map([], |r| r.get(0))?.collect()).map_err(err)?;
    let refs: Vec<&str> = guids.iter().map(String::as_str).collect();
    let decks: BTreeSet<i64> = card_deck_ids(pkg, |_| true)?;
    let hint = hint(con, &refs, &decks)?;
    Ok((hint.existing_notes > 0).then_some(hint))
}

/// 카드가 실제로 있는 Anki 덱 (필터 덱은 원래 덱)
fn card_deck_ids(pkg: &Package, picked: impl Fn(i64) -> bool) -> Res<BTreeSet<i64>> {
    let mut st = pkg.col.prepare("SELECT id, did, odid FROM cards").map_err(err)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).map_err(err)?;
    let mut out = BTreeSet::new();
    for row in rows {
        let (id, did, odid) = row.map_err(err)?;
        if picked(id) && target_deck(pkg, did, odid).is_some() {
            out.insert(real_deck(did, odid));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ 덱 (DU14)

#[derive(Default)]
struct DeckFinder {
    decks: Decks,
    cache: HashMap<i64, Option<i64>>,
}

impl DeckFinder {
    /// 새 노트의 덱: 그 Anki 덱 id 의 우리 덱 → 위 덱 중 우리 덱이 있는 가장 가까운 것 아래에 새로 → 경로대로.
    /// 내가 이름을 바꾸거나 옮긴 덱도 따라간다. 덱 이름 · 위치는 바꾸지 않는다
    fn find(&mut self, tx: &Connection, decks: &HashMap<i64, ADeck>, real: i64, now: i64) -> Res<Option<i64>> {
        if let Some(id) = self.cache.get(&real) {
            return Ok(*id);
        }
        let Some(d) = decks.get(&real).filter(|d| !d.filtered) else {
            self.cache.insert(real, None);
            return Ok(None);
        };
        let by_anki = |aid: i64| {
            tx.query_row("SELECT id FROM deck WHERE anki_deck_id = ?1 ORDER BY archived_at IS NOT NULL, id LIMIT 1", [aid], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(err)
        };
        let trimmed = |p: &[String]| p.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>();
        let id = match by_anki(real)? {
            Some(id) => {
                tx.execute("UPDATE deck SET archived_at = NULL, updated_at = ?2 WHERE id = ?1 AND archived_at IS NOT NULL", params![id, now]).map_err(err)?;
                id
            }
            None => {
                let mut under = None;
                for k in (1..d.path.len()).rev() {
                    let up = decks.iter().find(|(_, x)| !x.filtered && x.path == d.path[..k]).map(|(id, _)| *id);
                    if let Some(p) = up.map(by_anki).transpose()?.flatten() {
                        under = Some((p, k));
                        break;
                    }
                }
                match under {
                    Some((p, k)) if !trimmed(&d.path[k..]).is_empty() => self.decks.ensure(tx, Some(p), &trimmed(&d.path[k..]), now)?,
                    _ => self.decks.ensure(tx, None, &deck_parts(d), now)?,
                }
            }
        };
        tx.execute("UPDATE deck SET anki_deck_id = ?2 WHERE id = ?1 AND anki_deck_id IS NULL", params![id, real]).map_err(err)?;
        self.cache.insert(real, Some(id));
        Ok(Some(id))
    }
}

// ------------------------------------------------------------------ 미디어 (I85 · DU10)

/// 미디어를 둘 곳 (I85 · DU10). 이번 노트 · 유형이 쓰는 것만. 같은 이름 · 같은 내용이면 그 파일, 다른 내용이면 `이름-<sha8>` 로 새로 저장하고
/// 들어온 값의 참조를 고친다. 파일은 쓰지 않는다 (적용의 마지막 단계 `write_media`): 이름이 겹치는 파일만 풀어서 비교한다.
/// ponytail: 신형 패키지의 미디어 목록 sha1 로 풀지 않고 비교하는 것은 하지 않았다 (위험 ⑨)
struct MediaPlan {
    /// 패키지 이름 -> 저장할 이름 (바뀐 것만)
    renamed: HashMap<String, String>,
    /// (zip 항목, 저장할 이름) 전부. 아직 없는 파일은 write_media 가 쓴다
    files: Vec<(String, String)>,
}

fn plan_media(pkg: &mut Package, dir: &Path, wanted: impl Fn(&str) -> bool, s: &mut UpdateSummary) -> Res<MediaPlan> {
    let mut names = media_names(pkg)?;
    names.retain(|(_, n)| wanted(n));
    let mut plan = MediaPlan { renamed: HashMap::new(), files: vec![] };
    let mut unpacked = 0u64;
    let mut new: HashSet<String> = HashSet::new();
    for (key, name) in &names {
        if safe_media_name(name).is_err() {
            s.issues.push(issue("skipped", "media_name", format!("미디어 '{name}' 는 이름이 올바르지 않아 건너뛰어요")));
            continue;
        }
        let storable = storable_name(name).into_owned();
        let mut target = storable.clone();
        if dir.join(&target).exists() {
            s.media_compared += 1;
            let mut b = vec![];
            if let Err(e) = unpack(&mut pkg.zip, key, &format!("미디어 '{name}'"), MAX_MEDIA_FILE, &mut b) {
                s.issues.push(issue("warning", "media", format!("미디어 '{name}' 를 읽지 못했어요: {e}")));
                continue;
            }
            unpacked += b.len() as u64;
            if unpacked > MAX_TOTAL {
                return Err(format!("패키지의 미디어를 모두 풀면 {} MB를 넘어서 멈췄어요. 손상됐거나 안전하지 않은 파일일 수 있어요.", mb(MAX_TOTAL)));
            }
            if !same_or_absent(&dir.join(&target), &b) {
                use sha2::{Digest, Sha256};
                target = fit_name(&storable, &format!("-{}", &format!("{:x}", Sha256::digest(&b))[..8]));
                if !same_or_absent(&dir.join(&target), &b) {
                    s.issues.push(issue("warning", "media", format!("미디어 '{name}' 를 저장할 이름을 찾지 못해 건너뛰어요")));
                    continue;
                }
            }
        }
        if target != *name {
            plan.renamed.insert(name.clone(), target.clone());
        }
        if !dir.join(&target).exists() && new.insert(target.clone()) {
            s.media_new += 1;
            s.media_renamed += (target != storable) as usize;
        }
        plan.files.push((key.clone(), target));
    }
    Ok(plan)
}

/// 아직 없는 미디어 파일을 쓰고 미디어 행을 남긴다. 새로 쓴 파일은 written 에 남겨, 실패하거나 멈추면 부르는 쪽이 지운다
fn write_media(tx: &Connection, pkg: &mut Package, dir: &Path, plan: &MediaPlan, written: &mut Vec<PathBuf>, s: &mut UpdateSummary) -> Res<()> {
    if !plan.files.is_empty() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    for (key, target) in &plan.files {
        let file = dir.join(target);
        if !file.exists() {
            let mut b = vec![];
            if let Err(e) = unpack(&mut pkg.zip, key, &format!("미디어 '{target}'"), MAX_MEDIA_FILE, &mut b) {
                s.issues.push(issue("warning", "media", format!("미디어 '{target}' 를 읽지 못했어요: {e}")));
                continue;
            }
            written.push(file.clone()); // 쓰다가 실패해도 지우게 먼저 남긴다
            std::fs::write(&file, &b).map_err(err)?;
        }
        let size = std::fs::metadata(&file).map_or(0, |m| m.len() as i64);
        tx.execute(
            "INSERT OR IGNORE INTO media (filename, rel_path, sha256, byte_size, mime_type, created_at, anki_media_key) VALUES (?1, ?2, '', ?3, ?4, ?5, ?6)",
            params![target, format!("media/{target}"), size, mime_of(target), now_ms(), key],
        )
        .map_err(err)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ 노트 유형 (DU8 · DU9)

/// (Anki 이름, 우리 자리) -> Anki 이름과 다른 우리 이름 (rev 면 우리 이름 -> Anki 이름). 템플릿 참조를 옮길 때 쓴다
fn name_map<'a>(pairs: impl Iterator<Item = (&'a String, Option<usize>)>, nt: &Nt, rev: bool) -> HashMap<String, String> {
    pairs
        .filter_map(|(a, s)| {
            let o = nt.slots.get(s?)?;
            (!o.is_empty() && o != a).then(|| if rev { (o.clone(), a.clone()) } else { (a.clone(), o.clone()) })
        })
        .collect()
}

/// 필드 3-way (원본이 있을 때). Anki 필드 j -> 우리 자리. 이름이 같으면 원본의 자리, 같은 위치의 원본 이름이 새 버전에 없으면 이름이 바뀐 것
/// (내가 이름을 바꾸지 않았으면 새 이름으로, 템플릿 참조도), 원본에 없으면 새 필드 (끝 자리, field_ids K39). 원본에만 있으면 없어진 필드라 남긴다
fn fields_3way(tx: &Connection, before: &Nt, b: &TypeOrigin, t: &AType, c: &mut TypeChange, issues: &mut Vec<Issue>) -> Res<Vec<Option<usize>>> {
    let id = before.id;
    let n = &t.fields;
    let mut slots: Vec<Option<usize>> = vec![None; n.len()];
    let mut done = vec![false; n.len()];
    let mut used = vec![false; b.f.len()];
    for (j, y) in n.iter().enumerate() {
        if let Some(k) = (0..b.f.len()).find(|k| !used[*k] && b.f[*k].0 == *y) {
            (used[k], done[j], slots[j]) = (true, true, b.f[k].1);
        }
    }
    for (j, y) in n.iter().enumerate() {
        if done[j] || used.get(j) != Some(&false) || n.contains(&b.f[j].0) {
            continue;
        }
        (used[j], done[j], slots[j]) = (true, true, b.f[j].1);
        if let Some(s) = b.f[j].1
            && before.slots.get(s) == Some(&b.f[j].0)
        {
            match notetype::rename_field(tx, id, s, y) {
                Ok(()) => c.renamed_fields.push((b.f[j].0.clone(), y.clone())),
                Err(e) => issues.push(issue("warning", "field_rename", format!("노트 유형 '{}'의 필드 '{}' 이름을 '{y}'(으)로 바꾸지 못했어요: {e}", before.name, b.f[j].0))),
            }
        }
    }
    for (j, y) in n.iter().enumerate() {
        if done[j] {
            continue;
        }
        // 내가 같은 이름으로 더한 필드가 있으면 그 자리
        let cur = notetype::load(tx, id)?;
        slots[j] = match cur.visible().into_iter().find(|(s, x)| x == y && !slots.contains(&Some(*s))) {
            Some((s, _)) => Some(s),
            None => match notetype::add_field(tx, id, y) {
                Ok(s) => {
                    c.new_fields.push(y.clone());
                    Some(s)
                }
                Err(e) => {
                    issues.push(issue("warning", "new_field", format!("노트 유형 '{}'에 새 필드 '{y}'를 더하지 못했어요: {e}", before.name)));
                    None
                }
            },
        };
    }
    for (k, (_, s)) in b.f.iter().enumerate() {
        // 이름을 바꾸고 순서를 바꿔 내보낸 내 덱처럼, 그 자리가 다른 이름으로 이미 맞춰졌으면 없어진 것이 아니다
        if slots.contains(s) {
            continue;
        }
        if let (false, Some(name)) = (used[k], s.and_then(|s| before.slots.get(s)).filter(|x| !x.is_empty())) {
            c.removed_fields.push(name.clone());
        }
    }
    // 보이는 순서: 내가 순서를 바꾸거나 필드를 더하지 않았으면 새 버전 순서로. 바꿨으면 새 필드는 끝에 (visible 이 그렇게 읽는다)
    let vis: Vec<usize> = before.visible().into_iter().map(|(s, _)| s).collect();
    let base_order: Vec<usize> = b.f.iter().filter_map(|(_, s)| *s).filter(|s| vis.contains(s)).collect();
    if vis == base_order {
        let now: Vec<usize> = notetype::load(tx, id)?.visible().into_iter().map(|(s, _)| s).collect();
        let mut want: Vec<usize> = vec![];
        for s in slots.iter().flatten().chain(&now) {
            if now.contains(s) && !want.contains(s) {
                want.push(*s);
            }
        }
        if want != now {
            notetype::reorder_fields(tx, id, &want)?;
        }
    }
    Ok(slots)
}

/// 이미 있는 유형(같은 Anki 모델 id)을 새 버전에 맞춘다. 원본이 없으면 지금처럼 덮지 않는다 (NT29): 필드 · 카드 종류는 지금 규칙으로 찾고,
/// 다른 모양은 충돌로 본다 (기본 내 모양)
fn update_type(tx: &Connection, id: i64, t: &AType, choices: &Choices, issues: &mut Vec<Issue>) -> Res<(TypeMap, TypeChange)> {
    let before = notetype::load(tx, id)?;
    let raw: Option<String> = tx.query_row("SELECT origin FROM note_type WHERE id = ?1", [id], |r| r.get(0)).map_err(err)?;
    let base: Option<TypeOrigin> = raw.as_deref().and_then(|j| serde_json::from_str(j).ok());
    let exported = exported_ids(raw.as_deref());
    let mut c = TypeChange { note_type_id: id, name: before.name.clone(), no_base: base.is_none(), ..Default::default() };
    let slots = match &base {
        None => {
            let slots = notetype::incoming_slots(&before, &t.fields);
            c.skipped_fields = t.fields.iter().zip(&slots).filter(|(_, s)| s.is_none()).map(|(f, _)| f.clone()).collect();
            slots
        }
        Some(b) => fields_3way(tx, &before, b, t, &mut c, issues)?,
    };
    let now_nt = notetype::load(tx, id)?;
    // 들어온 템플릿은 우리 이름으로 넣고, 내 템플릿은 원본의 Anki 이름으로 바꿔 비교한다 (필드 이름만 바꾼 것은 모양을 고친 것이 아니다)
    let to_ours = name_map(t.fields.iter().zip(slots.iter().copied()), &now_nt, false);
    let to_anki = match &base {
        Some(b) => name_map(b.f.iter().map(|(a, s)| (a, *s)), &before, true),
        None => name_map(t.fields.iter().zip(slots.iter().copied()), &before, true),
    };
    let tr = |x: &str| notetype::rename_all_refs(x, &to_ours);
    let untr = |x: &str| notetype::rename_all_refs(x, &to_anki);
    let cloze = now_nt.cloze();
    let has = |ord: u32| before.templates.iter().any(|y| y.ord == ord);
    // 들어온 카드 종류 i -> 우리 ord, 그 원본
    let mut ours_of: Vec<Option<u32>> = vec![None; t.templates.len()];
    let mut base_of: Vec<Option<&TplOrigin>> = vec![None; t.templates.len()];
    match &base {
        None => {
            // 지금 규칙 (이름 · 앞면 · 같은 ord). 맞지 않는 것은 이 앱에서 지운 카드 종류로 본다 (Q2-08). 빈칸 유형은 같은 ord
            let tmpls: Vec<(u32, String, String)> = t.templates.iter().map(|(o, n, q, _)| (*o, n.clone(), q.clone())).collect();
            let map: HashMap<u32, u32> = if cloze { t.templates.iter().filter(|x| has(x.0)).map(|x| (x.0, x.0)).collect() } else { notetype::incoming_ords(&before, &tmpls) };
            for (i, x) in t.templates.iter().enumerate() {
                ours_of[i] = map.get(&x.0).copied();
                if ours_of[i].is_none() && !cloze {
                    c.skipped_templates.push(x.1.clone());
                }
            }
        }
        Some(b) => {
            // 원본의 카드 종류를 이름으로, 다음에 같은 Anki ord 로 찾는다. 원본에 있고 우리에 없으면 내가 지운 것이라 넣지 않는다 (Q2-08)
            let mut used = vec![false; b.t.len()];
            for pass in 0..2 {
                for (i, (o, name, _, _)) in t.templates.iter().enumerate() {
                    if base_of[i].is_some() {
                        continue;
                    }
                    if let Some(k) = (0..b.t.len()).find(|k| !used[*k] && if pass == 0 { b.t[*k].n == *name } else { b.t[*k].o == *o }) {
                        used[k] = true;
                        base_of[i] = Some(&b.t[k]);
                        ours_of[i] = b.t[k].our.filter(|x| has(*x));
                    }
                }
            }
            for (k, e) in b.t.iter().enumerate() {
                if let (false, Some(y)) = (used[k], e.our.and_then(|x| before.templates.iter().find(|y| y.ord == x))) {
                    c.removed_templates.push(y.name.clone());
                }
            }
        }
    }
    // 새 카드 종류 (원본에 없다): 더한다 (기기 몫 ord, K39). 카드는 노트 값을 고친 뒤에 만든다. 빈칸 유형은 카드 종류가 하나다
    let mut new_ords = vec![];
    if base.is_some() && !cloze {
        for (i, (_, name, q, a)) in t.templates.iter().enumerate() {
            if base_of[i].is_some() {
                continue;
            }
            let cur = notetype::load(tx, id)?;
            // 내가 같은 이름으로 더한 카드 종류면 그것 (모양은 아래에서 원본 없이 비교한다)
            if let Some(y) = cur.templates.iter().find(|y| y.name == *name && !ours_of.contains(&Some(y.ord))) {
                ours_of[i] = Some(y.ord);
                continue;
            }
            match notetype::insert_template(tx, &cur, Some(name), &tr(q), &tr(a)) {
                Ok(ord) => {
                    ours_of[i] = Some(ord);
                    new_ords.push(ord);
                    c.new_templates.push(name.clone());
                }
                Err(e) => issues.push(issue("warning", "new_template", format!("노트 유형 '{}'에 새 카드 종류 '{name}'를 더하지 못했어요: {e}", before.name))),
            }
        }
    }
    // 모양 3-way: 만든 사람만 바꿨으면 새 모양, 둘 다 바꿨으면 내 모양 (고르면 새 모양)
    let cur = notetype::load(tx, id)?;
    for (i, (_, _, q, a)) in t.templates.iter().enumerate() {
        let Some(x) = ours_of[i].filter(|x| !new_ords.contains(x)) else { continue };
        let (Some(mine), Some(now)) = (before.templates.iter().find(|y| y.ord == x), cur.templates.iter().find(|y| y.ord == x)) else { continue };
        let (bq, ba) = base_of[i].map(|e| (e.q.as_str(), e.a.as_str())).unzip();
        let pq = pick(bq, &h(&untr(&mine.qfmt)), &h(q), true);
        let pa = pick(ba, &h(&untr(&mine.afmt)), &h(a), true);
        let (nq, na) = (tr(q), tr(a));
        let conflict = pq == Pick::Conflict || pa == Pick::Conflict;
        let write = if conflict {
            let take = choices.new_looks.contains(&(id, Some(x)));
            let sides = |who: Pick| [("앞면", pq), ("뒷면", pa)].into_iter().filter(|(_, p)| *p == who || *p == Pick::Conflict).map(|(n, _)| n).collect();
            c.looks.push(LookConflict {
                ord: Some(x),
                name: now.name.clone(),
                mine_front: now.qfmt.clone(),
                mine_back: now.afmt.clone(),
                new_front: nq.clone(),
                new_back: na.clone(),
                take_new: take,
                mine_changed: sides(Pick::Mine),
                new_changed: sides(Pick::New),
                edited_at: tx
                    .query_row("SELECT updated_at FROM card_template WHERE note_type_id = ?1 AND ord = ?2", params![id, x], |r| r.get(0))
                    .map_err(err)?,
            });
            take.then_some((nq, na))
        } else if pq == Pick::New || pa == Pick::New {
            Some((if pq == Pick::New { nq } else { now.qfmt.clone() }, if pa == Pick::New { na } else { now.afmt.clone() }))
        } else {
            None
        };
        let Some((q2, a2)) = write.filter(|(q2, a2)| *q2 != now.qfmt || *a2 != now.afmt) else { continue };
        match notetype::check_template(&cur, &q2, &a2) {
            Ok(()) => {
                tx.execute(
                    "UPDATE card_template SET qfmt = ?3, afmt = ?4, updated_at = ?5 WHERE note_type_id = ?1 AND ord = ?2",
                    params![id, x, q2, a2, now_ms()],
                )
                .map_err(err)?;
                if !conflict {
                    c.updated_looks.push(now.name.clone());
                }
            }
            Err(e) => issues.push(issue("warning", "look", format!("노트 유형 '{}'의 카드 종류 '{}'에 새 모양을 넣지 못했어요: {e}", before.name, now.name))),
        }
    }
    // CSS 3-way. 쉬운 편집이 붙인 줄 상자 CSS 는 내 수정으로 세지 않고, 새 CSS 에도 다시 붙인다
    let cur = notetype::load(tx, id)?;
    let take_css = match pick(base.as_ref().map(|b| b.c.as_str()), &h(&notetype::without_layout_css(&cur.css)), &h(&t.css), true) {
        Pick::Conflict => {
            let take = choices.new_looks.contains(&(id, None));
            c.looks.push(LookConflict {
                ord: None,
                name: "CSS".into(),
                mine_front: cur.css.clone(),
                mine_back: String::new(),
                new_front: t.css.clone(),
                new_back: String::new(),
                take_new: take,
                mine_changed: vec!["CSS"],
                new_changed: vec!["CSS"],
                edited_at: None,
            });
            take
        }
        Pick::New => {
            c.updated_looks.push("CSS".into());
            true
        }
        _ => false,
    };
    let css = notetype::with_layout_css(&t.css, cur.templates.iter().any(notetype::uses_box));
    (c.css_mine, c.css_new) = (cur.css.clone(), css.clone());
    c.notes = tx.query_row("SELECT count(*) FROM note WHERE note_type_id = ?1", [id], |r| r.get::<_, i64>(0)).map_err(err)? as usize;
    if take_css && css != cur.css {
        tx.execute("UPDATE note_type SET css = ?2, updated_at = ?3 WHERE id = ?1", params![id, css, now_ms()]).map_err(err)?;
    }
    // 원본 = 새 버전 (고른 결과와 상관없이, 내 것을 지킨 곳은 다음에도 "내가 고침"으로 남는다)
    let origin = TypeOrigin {
        f: t.fields.iter().cloned().zip(slots.iter().copied()).collect(),
        t: t.templates.iter().zip(&ours_of).map(|((o, n, q, a), our)| TplOrigin { o: *o, n: n.clone(), our: *our, q: h(q), a: h(a) }).collect(),
        c: h(&t.css),
        m: exported,
    };
    tx.execute("UPDATE note_type SET origin = ?2 WHERE id = ?1 AND origin IS NOT ?2", params![id, serde_json::to_string(&origin).map_err(err)?]).map_err(err)?;
    let ords = if cloze { HashMap::new() } else { t.templates.iter().zip(&ours_of).filter_map(|(x, o)| o.map(|o| (x.0, o))).collect() };
    Ok((TypeMap { ntid: id, slots, ords, cloze, new_ords }, c))
}

/// 앱 기본 유형 (내보낸 내 덱이 돌아왔다, DU55): 읽기 전용이라 필드 · 카드 종류 · 모양 · 원본은 그대로 두고, 들어온 필드 · 카드 종류를 이름으로 찾기만 한다
fn native_map(tx: &Connection, id: i64, t: &AType) -> Res<TypeMap> {
    let nt = notetype::load(tx, id)?;
    let cloze = nt.cloze();
    let tmpls: Vec<(u32, String, String)> = t.templates.iter().map(|(o, n, q, _)| (*o, n.clone(), q.clone())).collect();
    let ords = if cloze { HashMap::new() } else { notetype::incoming_ords(&nt, &tmpls) };
    Ok(TypeMap { ntid: id, slots: notetype::incoming_slots(&nt, &t.fields), ords, cloze, new_ords: vec![] })
}

// ------------------------------------------------------------------ 실행

/// 사전 검사 · 적용 요청
pub(crate) struct Req<'a> {
    pub path: &'a Path,
    /// B2-s 고른 Anki 덱. None 이면 전부
    pub selected: Option<&'a HashSet<i64>>,
    pub choices: &'a Choices,
    pub voices: &'a [String],
    /// B2-p 진행 (단계, 끝난 수, 전체): backup → types → notes → cards → media → done
    pub emit: &'a dyn Fn(&'static str, usize, usize),
    /// B2-p [취소] (import_cancel). 사전 검사는 늘 false
    pub cancel: &'a dyn Fn() -> bool,
}

pub(crate) const STOPPED: &str = "업데이트를 멈췄어요. 아무것도 바뀌지 않았어요.";
/// 백업을 만들지 못하면 적용하지 않는다 (DU16). 화면이 이 머리로 "백업을 만들 수 없음"을 가린다
pub(crate) const NO_BACKUP: &str = "적용 전 백업을 만들지 못해 업데이트하지 않았어요.";

/// 덱 업데이트. dry 면 한 트랜잭션에서 다 해 보고 되돌린다 (미디어 파일은 쓰지 않는다). 적용은 부르는 쪽이 먼저 백업한다
pub(crate) fn run(con: &mut Connection, data_dir: &Path, req: &Req, dry: bool, written: &mut Vec<PathBuf>) -> Res<UpdateSummary> {
    let mut pkg = open_package(req.path, data_dir, req.voices)?;
    let chosen = req.selected.map(|s| choose(&pkg, s)).transpose()?;
    let picked_note = |id: i64| chosen.as_ref().is_none_or(|c| c.notes.contains(&id));
    let picked_card = |id: i64| chosen.as_ref().is_none_or(|c| c.cards.contains(&id));
    let notes: Vec<PNote> = {
        let mut st = pkg.col.prepare("SELECT id, guid, mid, mod, tags, flds FROM notes ORDER BY id").map_err(err)?;
        let rows = st
            .query_map([], |r| Ok(PNote { id: r.get(0)?, guid: r.get(1)?, mid: r.get(2)?, modified: r.get(3)?, tags: r.get(4)?, flds: r.get(5)? }))
            .map_err(err)?;
        let all: Vec<PNote> = rows.collect::<Result<_, _>>().map_err(err)?;
        all.into_iter().filter(|n| picked_note(n.id)).collect()
    };
    let mut cards: HashMap<i64, Vec<PCard>> = HashMap::new();
    {
        let mut st = pkg.col.prepare("SELECT id, nid, did, odid, ord, queue FROM cards ORDER BY due, id").map_err(err)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(1)?, PCard { id: r.get(0)?, did: r.get(2)?, odid: r.get(3)?, ord: r.get(4)?, queue: r.get(5)? })))
            .map_err(err)?;
        for row in rows {
            let (nid, c) = row.map_err(err)?;
            if picked_card(c.id) {
                cards.entry(nid).or_default().push(c);
            }
        }
    }
    let file_name = req.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut s = UpdateSummary { file_name: file_name.clone(), ..Default::default() };
    s.reviews_skipped = {
        let mut st = pkg.col.prepare("SELECT cid FROM revlog").map_err(err)?;
        let cids = st.query_map([], |r| r.get::<_, i64>(0)).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?;
        cids.into_iter().filter(|c| picked_card(*c)).count()
    };
    let deck_ids = card_deck_ids(&pkg, picked_card)?;
    let all_guids: HashSet<String> = pkg.col.prepare("SELECT guid FROM notes").and_then(|mut s| s.query_map([], |r| r.get(0))?.collect()).map_err(err)?;

    let tx = con.transaction().map_err(err)?;
    let now = now_ms();
    s.hint = hint(&tx, &notes.iter().map(|n| n.guid.as_str()).collect::<Vec<_>>(), &deck_ids)?;
    tx.execute("INSERT INTO import_batch (source_kind, source_name, started_at) VALUES ('apkg', ?1, ?2)", params![file_name, now]).map_err(err)?;
    let batch = tx.last_insert_rowid();
    // 되돌리기 기록 (DU57): 이보다 큰 id 는 이번에 만든 노트 · 카드다
    let (max_note, max_card): (i64, i64) = tx
        .query_row("SELECT (SELECT coalesce(max(id), 0) FROM note), (SELECT coalesce(max(id), 0) FROM card)", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?;

    // 미디어: 고른 노트가 쓰는 것과, 쓰는 노트타입의 템플릿 · CSS 가 쓰는 "_" 파일(글꼴 등). 이름 바꾸기가 들어온 값(N)을 정한다
    let used_types: BTreeSet<i64> = notes.iter().map(|n| n.mid).filter(|m| pkg.types.contains_key(m)).collect();
    let mut refs: HashSet<String> = HashSet::new();
    for n in &notes {
        crate::export::media_refs(&n.flds, &mut refs);
    }
    let type_text: String = used_types
        .iter()
        .map(|m| &pkg.types[m])
        .flat_map(|t| std::iter::once(t.css.as_str()).chain(t.templates.iter().flat_map(|x| [x.2.as_str(), x.3.as_str()])))
        .collect();
    let wanted = |n: &str| refs.contains(n) || (n.starts_with('_') && type_text.contains(n));
    let media = plan_media(&mut pkg, &media_dir(data_dir), wanted, &mut s)?;
    let renamed = &media.renamed;

    // 노트 유형
    (req.emit)("types", 0, used_types.len());
    let mut maps: HashMap<i64, TypeMap> = HashMap::new();
    let mut type_at: HashMap<i64, usize> = HashMap::new();
    for aid in &used_types {
        let t = &pkg.types[aid];
        let (map, change) = match type_of_model(&tx, *aid)? {
            Some((id, true)) => (native_map(&tx, id, t)?, TypeChange { note_type_id: id, ..Default::default() }),
            Some((id, false)) => {
                let before = type_row(&tx, id)?;
                let out = update_type(&tx, id, t, req.choices, &mut s.issues)?;
                if !dry {
                    journal(&tx, batch, "type", &before, &type_row(&tx, id)?)?;
                }
                out
            }
            None => {
                // 새 유형의 카드 종류 덱(NT76): 같은 Anki 덱 id 로 가져온 우리 덱이 이미 있을 때만
                let id = insert_type(&tx, *aid, t, now, &|did| {
                    let ours = tx.query_row("SELECT id FROM deck WHERE anki_deck_id = ?1 AND archived_at IS NULL ORDER BY id LIMIT 1", [did], |r| r.get(0)).optional().map_err(err)?;
                    Ok((ours, pkg.decks.get(&did).map(|d| d.path.join(" › "))))
                })?;
                let map = TypeMap {
                    ntid: id,
                    slots: (0..t.fields.len()).map(Some).collect(),
                    ords: t.templates.iter().map(|x| (x.0, x.0)).collect(),
                    cloze: t.templates.iter().any(|x| notetype::cloze_like(&x.2)),
                    new_ords: vec![],
                };
                (map, TypeChange { note_type_id: id, name: t.name.clone(), created: true, ..Default::default() })
            }
        };
        type_at.insert(map.ntid, s.types.len());
        maps.insert(*aid, map);
        s.types.push(change);
    }
    (req.emit)("types", used_types.len(), used_types.len());
    let nts: HashMap<i64, Nt> = maps.values().map(|m| Ok((m.ntid, notetype::load(&tx, m.ntid)?))).collect::<Res<_>>()?;
    let fronts = notetype::Fronts::load(&tx)?;

    // 같은 guid 가 두 벌인 노트 (DU-06): 알린다. 고치는 것은 첫 번째 것 (ORDER BY id)
    {
        let dups: Vec<String> = tx
            .prepare("SELECT anki_guid FROM note WHERE anki_guid IS NOT NULL GROUP BY anki_guid HAVING count(*) > 1")
            .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
            .map_err(err)?;
        let mut st = tx.prepare_cached("SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = ?1 ORDER BY c.id").map_err(err)?;
        for g in dups.iter().filter(|g| all_guids.contains(*g)) {
            s.duplicate_notes += 1;
            s.duplicate_card_ids.extend(st.query_map([g], |r| r.get::<_, i64>(0)).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?);
        }
    }

    // 노트
    let mut finder = DeckFinder::default();
    let mut used_decks: HashSet<i64> = HashSet::new();
    let mut card_tags: HashMap<i64, Vec<Option<(i64, String)>>> = HashMap::new();
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    let (mut no_tpl, mut no_deck) = (0, 0);
    for (i, n) in notes.iter().enumerate() {
        if i.is_multiple_of(50) {
            if (req.cancel)() {
                return Err(STOPPED.into());
            }
            (req.emit)("notes", i, notes.len());
        }
        let (Some(tm), Some(t)) = (maps.get(&n.mid), pkg.types.get(&n.mid)) else { continue };
        let nt = &nts[&tm.ntid];
        // 자리마다 보일 이름: 우리 이름, 이 앱에서 지운 자리는 "Anki 이름 (지운 필드)" (DU-12)
        let names: Vec<String> = (0..nt.slots.len())
            .map(|sl| match nt.slots[sl].as_str() {
                "" => tm.slots.iter().position(|x| *x == Some(sl)).and_then(|j| t.fields.get(j)).map_or_else(|| "(지운 필드)".into(), |f| format!("{f} (지운 필드)")),
                n => n.to_string(),
            })
            .collect();
        // N: 들어온 값을 우리 자리에 (미디어 이름 바꾸기 뒤). 넣을 자리가 없는 값은 알린다 (Q2-13)
        let mut inc: Vec<Option<String>> = vec![None; nt.slots.len()];
        let mut lost = vec![];
        for (j, v) in n.flds.split(SEP).enumerate() {
            match tm.slots.get(j).copied().flatten() {
                Some(sl) if sl < inc.len() => inc[sl] = Some(rename_refs(v, renamed)),
                _ if !v.is_empty() => lost.push(t.fields.get(j).cloned().unwrap_or_default()),
                _ => {}
            }
        }
        let tags = crate::export::without_source_tags(&n.tags);
        let ours: Option<Ours> = tx
            .prepare_cached("SELECT id, note_type_id, fields, tags, origin, updated_at, exam_hits, exam_names FROM note WHERE anki_guid = ?1 ORDER BY id LIMIT 1")
            .map_err(err)?
            .query_row([&n.guid], |r| {
                let raw: String = r.get(2)?;
                Ok(Ours {
                    id: r.get(0)?,
                    ntid: r.get(1)?,
                    fields: serde_json::from_str(&raw).unwrap_or_default(),
                    fields_raw: raw,
                    tags: r.get(3)?,
                    origin: r.get(4)?,
                    updated_at: r.get(5)?,
                    exam: (r.get(6)?, r.get(7)?),
                })
            })
            .optional()
            .map_err(err)?;
        match ours {
            Some(o) if o.ntid != tm.ntid => s.other_type += 1,
            Some(o) => {
                lost.into_iter().for_each(|f| *dropped.entry(f).or_default() += 1);
                let before = if dry { serde_json::Value::Null } else { note_row(&tx, o.id)? };
                let made = update_note(&tx, nt, &names, &fronts, &o, &inc, &tags, req.choices, now, &mut s)?;
                if !dry {
                    journal(&tx, batch, "note", &before, &note_row(&tx, o.id)?)?;
                }
                if let Some(c) = type_at.get(&tm.ntid).map(|i| &mut s.types[*i]) {
                    c.new_template_cards += made.iter().filter(|o| tm.new_ords.contains(o)).count();
                }
            }
            None => {
                let gone: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM origin_gone WHERE guid = ?1)", [&n.guid], |r| r.get(0)).map_err(err)?;
                if gone {
                    s.gone += 1;
                    continue;
                }
                // 새 노트: 지금 가져오기와 같다 (신규로 시작, 일시 정지만 지킨다). 카드가 하나도 들어가지 못하면 넣지 않는다
                let mut plan = vec![];
                for c in cards.get(&n.id).into_iter().flatten() {
                    let ord = if tm.cloze { Some(c.ord) } else { tm.ords.get(&c.ord).copied() };
                    let Some(ord) = ord else {
                        no_tpl += 1;
                        continue;
                    };
                    let Some(deck) = finder.find(&tx, &pkg.decks, real_deck(c.did, c.odid), now)? else {
                        no_deck += 1;
                        continue;
                    };
                    plan.push((c, ord, deck));
                }
                if plan.is_empty() {
                    continue;
                }
                lost.into_iter().for_each(|f| *dropped.entry(f).or_default() += 1);
                let values: Vec<String> = inc.into_iter().map(Option::unwrap_or_default).collect();
                tx.execute(
                    "INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, anki_note_id, anki_guid, import_batch_id, origin)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?4, ?6, ?7, ?8)",
                    params![tm.ntid, serde_json::to_string(&values).map_err(err)?, tags, n.id, n.modified * 1000, n.guid, batch, note_origin(&values, &tags)],
                )
                .map_err(err)?;
                let nid = tx.last_insert_rowid();
                for (c, ord, deck) in plan {
                    s.new_note_cards += tx
                        .execute(
                            "INSERT OR IGNORE INTO card (note_id, deck_id, template_ord, state, created_at, updated_at, anki_card_id) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                            params![nid, deck, ord, if c.queue == -1 { "suspended" } else { "new" }, now, c.id],
                        )
                        .map_err(err)?;
                    used_decks.insert(deck);
                    card_tags.entry(deck).or_default().push(tags_level(&tags));
                }
                s.new_notes += 1;
            }
        }
    }
    (req.emit)("cards", 0, 1);
    // 새 카드 종류: 이 유형의 다른 노트(이번 파일에 없는 노트 포함)도 앞면이 비지 않으면 카드를 만든다 (NT22, 덱 · 중요도는 노트의 첫 카드)
    for tm in maps.values().filter(|m| !m.new_ords.is_empty()) {
        let nt = &nts[&tm.ntid];
        let rows: Vec<(i64, String)> = tx
            .prepare("SELECT id, fields FROM note WHERE note_type_id = ?1 ORDER BY id")
            .and_then(|mut st| st.query_map([tm.ntid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .map_err(err)?;
        let mut made = 0;
        for (nid, fields) in rows {
            let values: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
            made += notetype::add_missing_cards_where(&tx, nt, nid, &values, |o| tm.new_ords.contains(&o))?;
        }
        s.new_cards += made;
        if let Some(i) = type_at.get(&tm.ntid) {
            s.types[*i].new_template_cards += made;
        }
    }

    // 새 버전에 없는 노트: 원본이 있고, 이번 패키지 덱의 우리 덱에 카드가 있는 노트 (DU11). 그대로 둔다
    if !deck_ids.is_empty() {
        let list = deck_ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let rows: Vec<(i64, String)> = tx
            .prepare(&format!(
                "SELECT DISTINCT n.id, n.anki_guid FROM note n JOIN card c ON c.note_id = n.id JOIN deck d ON d.id = c.deck_id
                 WHERE n.origin IS NOT NULL AND n.anki_guid IS NOT NULL AND d.anki_deck_id IN ({list}) ORDER BY n.id"
            ))
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .map_err(err)?;
        let missing: Vec<i64> = rows.into_iter().filter(|(_, g)| !all_guids.contains(g)).map(|(id, _)| id).collect();
        s.missing_notes = missing.len();
        let mut st = tx.prepare_cached("SELECT id FROM card WHERE note_id = ?1 ORDER BY id").map_err(err)?;
        for id in missing {
            s.missing_card_ids.extend(st.query_map([id], |r| r.get::<_, i64>(0)).map_err(err)?.collect::<Result<Vec<_>, _>>().map_err(err)?);
        }
    }

    // 새 노트의 기출 태그 · 덱 중요도 제안 (새로 넣은 카드만 센다, 사용자가 정한 덱은 건드리지 않는다, I1 · I24)
    crate::exam::fill_from_tags(&tx).map_err(err)?;
    suggest_from_tags(&tx, &card_tags)?;
    s.new_decks = finder.decks.created.len();
    // 결과의 [덱 열기] · [학습 시작]: 업데이트한 덱과 새 노트가 들어간 덱의 최상위 덱
    used_decks.extend(known_decks(&tx, &deck_ids)?);
    s.roots = roots_of(&tx, &used_decks)?.into_iter().collect();
    for (f, c) in &dropped {
        s.issues.push(issue("warning", "dropped_field", format!("이 노트 유형에 없는 필드 '{f}'의 값이 있는 노트 {}개는 그 값을 넣지 않아요", n(*c))));
    }
    if no_tpl > 0 {
        s.issues.push(issue("skipped", "deleted_card_type", format!("이 앱에서 지운 카드 종류의 새 카드 {}장은 넣지 않아요", n(no_tpl))));
    }
    if no_deck > 0 {
        s.issues.push(issue("skipped", "missing_deck", format!("덱을 찾지 못한 새 카드 {}장은 넣지 않아요", n(no_deck))));
    }
    (req.emit)("cards", 1, 1);
    if !dry {
        (req.emit)("media", 0, media.files.len());
        write_media(&tx, &mut pkg, &media_dir(data_dir), &media, written, &mut s)?;
        (req.emit)("media", media.files.len(), media.files.len());
    }
    if (req.cancel)() {
        return Err(STOPPED.into());
    }
    if dry {
        let mut want: HashSet<String> = HashSet::new();
        s.conflicts.iter().flat_map(|c| &c.fields).for_each(|f| crate::export::media_refs(&f.new, &mut want));
        let mut budget = 8usize << 20;
        for (key, target) in &media.files {
            if !want.contains(target) || media_dir(data_dir).join(target).exists() {
                continue;
            }
            let mut b = vec![];
            if unpack(&mut pkg.zip, key, target, MAX_MEDIA_FILE, &mut b).is_ok() && b.len() <= budget {
                use base64::Engine;
                budget -= b.len();
                s.media_preview.insert(target.clone(), format!("data:{};base64,{}", mime_of(target), base64::engine::general_purpose::STANDARD.encode(&b)));
            }
        }
    }
    s.types.retain(|t| !t.is_empty());
    s.kept_looks = s.types.iter().filter(|t| t.looks.iter().any(|l| !l.take_new)).count();
    if !dry {
        journal_created(&tx, batch, max_note, max_card, &s)?;
        s.batch = Some(batch);
    }
    tx.execute(
        "UPDATE import_batch SET finished_at = ?2, status = 'ok', note_count = ?3, card_count = ?4, media_count = ?5 WHERE id = ?1",
        params![batch, now_ms(), s.new_notes as i64, (s.new_note_cards + s.new_cards) as i64, s.media_new as i64],
    )
    .map_err(err)?;
    if dry {
        tx.rollback().map_err(err)?;
    } else {
        tx.commit().map_err(err)?;
        (req.emit)("done", 1, 1);
    }
    Ok(s)
}

/// 이미 있는 노트 하나. 자리마다 판정해 값 · 태그 · 기출 · 원본을 쓰고, 생기는 카드를 만든다 (빈칸 번호 · 앞면). 만든 카드의 ord
#[allow(clippy::too_many_arguments)]
fn update_note(tx: &Connection, nt: &Nt, names: &[String], fronts: &notetype::Fronts, o: &Ours, inc: &[Option<String>], tags: &str, choices: &Choices, now: i64, s: &mut UpdateSummary) -> Res<Vec<u32>> {
    let base: Option<NoteOrigin> = o.origin.as_deref().and_then(|j| serde_json::from_str(j).ok());
    let len = nt.slots.len().max(o.fields.len());
    let mut m = o.fields.clone();
    m.resize(len, String::new());
    let mut fin = m.clone();
    let mut of = base.as_ref().map_or_else(Vec::new, |b| b.f.clone());
    of.resize(len, String::new());
    // 원본 글: 들어온 자리는 새 버전 값, 나머지는 지난번 값 (해시가 맞을 때만, 모르면 null)
    let mut ov: Vec<Option<String>> = (0..len).map(|sl| base.as_ref().and_then(|b| b.value(sl)).map(String::from)).collect();
    let take = |sl: usize| choices.take_new.contains(&o.id) || choices.take_new_fields.contains(&(o.id, sl));
    // DU18: 원본이 없으면 updated_at 의 ms 자리로 가져온 뒤 고쳤는지 본다 (가져올 때는 Anki mod 초 × 1000).
    // ponytail: 앱에서 저장한 시각의 ms 가 000 이면 (1/1000) 고치지 않은 것으로 본다
    let touched = o.updated_at % 1000 != 0;
    let (mut new, mut mine) = (0, 0);
    let mut conflicts = vec![];
    let mut new_names: Vec<String> = vec![];
    for (sl, v) in inc.iter().enumerate() {
        let Some(v) = v else { continue };
        let b = base.as_ref().map(|_| if of[sl].is_empty() { h("") } else { of[sl].clone() });
        match pick(b.as_deref(), &h(&m[sl]), &h(v), touched) {
            Pick::Same => {}
            Pick::New => {
                fin[sl] = v.clone();
                new += 1;
                new_names.push(names.get(sl).cloned().unwrap_or_default());
            }
            Pick::Mine => mine += 1,
            Pick::Conflict => {
                let name = names.get(sl).cloned().unwrap_or_default();
                let base = base.as_ref().and_then(|b| b.value(sl)).map(String::from);
                conflicts.push(ConflictField { slot: sl, name, mine: m[sl].clone(), new: v.clone(), base, take_new: take(sl) });
                if take(sl) {
                    fin[sl] = v.clone();
                }
            }
        }
        of[sl] = h(v);
        ov[sl] = Some(v.clone());
    }
    let fin_tags = merge_tags(base.as_ref().map(|b| b.t.as_str()), &o.tags, tags);
    let set = |t: &str| t.split_whitespace().map(String::from).collect::<BTreeSet<_>>();
    let tags_changed = set(&fin_tags) != set(&o.tags);
    new += tags_changed as usize;
    if tags_changed {
        new_names.push("태그".into());
    }
    mine += base.as_ref().is_some_and(|b| set(&b.t) != set(&o.tags)) as usize;
    // 기출 표시: 지금 값이 지금 태그의 해석과 같으면(또는 아직 정하지 않았으면) 새 태그로 다시 채운다. 다르면 사용자가 고친 것이라 지킨다 (DU7)
    let exam = if o.exam == exam_of(&o.tags) || o.exam.0.is_none() { exam_of(&fin_tags) } else { o.exam.clone() };
    if base.is_none() {
        s.no_base += 1;
    }
    match (conflicts.is_empty(), new > 0, mine > 0) {
        (false, _, _) => {
            let title = nt.visible().first().map(|(sl, _)| crate::render::strip_tags(&m[*sl])).unwrap_or_default().chars().take(80).collect();
            let deck: String = tx
                .query_row(
                    "WITH RECURSIVE up(id, name, parent_id, depth) AS (
                       SELECT d.id, d.name, d.parent_id, 0 FROM deck d WHERE d.id = (SELECT deck_id FROM card WHERE note_id = ?1 ORDER BY id LIMIT 1)
                       UNION ALL SELECT d.id, d.name, d.parent_id, up.depth + 1 FROM deck d JOIN up ON d.id = up.parent_id)
                     SELECT coalesce(group_concat(name, ' › '), '') FROM (SELECT name FROM up ORDER BY depth DESC)",
                    [o.id],
                    |r| r.get(0),
                )
                .map_err(err)?;
            let take_new = conflicts.iter().any(|f| f.take_new);
            s.conflicts.push(ConflictNote {
                note_id: o.id,
                title,
                deck,
                fields: conflicts,
                auto_new: new,
                auto_mine: mine,
                auto_new_fields: new_names,
                edited_at: o.updated_at,
                take_new,
                no_base: base.is_none(),
                native_kind: nt.native_kind.clone(),
                slot_names: names.to_vec(),
            });
        }
        (true, true, true) => s.merged += 1,
        (true, true, false) => s.changed += 1,
        (true, false, true) => s.mine_kept += 1,
        (true, false, false) => s.same += 1,
    }

    let fields_changed = fin != m;
    let fields_json = if fields_changed { serde_json::to_string(&fin).map_err(err)? } else { o.fields_raw.clone() };
    let origin = serde_json::to_string(&NoteOrigin { f: of, t: words(tags), v: Some(ov) }).map_err(err)?;
    if fields_changed || tags_changed || exam != o.exam || o.origin.as_deref() != Some(origin.as_str()) {
        tx.execute(
            "UPDATE note SET fields = ?2, tags = ?3, exam_hits = ?4, exam_names = ?5, origin = ?6, updated_at = ?7 WHERE id = ?1",
            params![
                o.id,
                fields_json,
                if tags_changed { fin_tags.as_str() } else { o.tags.as_str() },
                exam.0,
                exam.1,
                origin,
                if fields_changed || tags_changed { now } else { o.updated_at }
            ],
        )
        .map_err(err)?;
    }
    if !fields_changed {
        return Ok(vec![]);
    }
    // 카드: 지우지 않는다 (save_note_fields 는 빈칸이 줄면 카드를 지운다, I14). 앞면이 비는 카드는 학습에서 빠지고 기록 · 일정은 남는다 (DU12 · DU13).
    // 새로 생기는 카드(빈칸 새 번호 · 앞면이 새로 생김)는 신규로, 덱 · 중요도는 노트의 첫 카드 (C3-r)
    let cloze = nt.cloze();
    let have: Vec<u32> = tx
        .prepare_cached("SELECT template_ord FROM card WHERE note_id = ?1")
        .map_err(err)?
        .query_map([o.id], |r| r.get(0))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    for ord in &have {
        if !fronts.empty(nt.id, *ord, &o.fields_raw) && fronts.empty(nt.id, *ord, &fields_json) {
            s.emptied_cards += 1;
            s.cloze.emptied_cards += cloze as usize;
        }
    }
    let mut made = vec![];
    for ord in notetype::card_ords(nt, &fin).into_iter().filter(|o| !have.contains(o)) {
        let n = tx
            .execute(
                "INSERT OR IGNORE INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
                 SELECT note_id, coalesce(?4, deck_id), ?2, importance, importance_source, ?3, ?3 FROM card WHERE note_id = ?1 ORDER BY id LIMIT 1",
                params![o.id, ord, now, notetype::template_deck(tx, nt.id, ord)?],
            )
            .map_err(err)?;
        if n > 0 {
            made.push(ord);
        }
    }
    s.new_cards += made.len();
    if cloze {
        s.cloze.new_cards += made.len();
        // 빈칸 글 묶음은 같은데 번호만 다르다 (기록 · 일정은 번호를 따른다: 새 번호 내용에 옛 번호 기록이 붙는다, DU13)
        let parts = |v: &[String]| {
            let mut p: Vec<(u32, String)> =
                nt.visible().iter().filter_map(|(sl, _)| v.get(*sl)).flat_map(|x| crate::render::cloze_parts(x).into_iter().map(|(n, a)| (n, a.to_string()))).collect();
            p.sort();
            p
        };
        let texts = |p: &[(u32, String)]| {
            let mut t: Vec<String> = p.iter().map(|x| x.1.clone()).collect();
            t.sort();
            t
        };
        let (pm, pf) = (parts(&m), parts(&fin));
        if !pm.is_empty() && pm != pf && texts(&pm) == texts(&pf) {
            s.cloze.renumbered_notes += 1;
        }
    }
    Ok(made)
}

// ------------------------------------------------------------------ 명령

/// B2-u 사전 검사: 쓰지 않는다. choices 를 주면 그 고르기를 반영한 요약 (충돌 검토 뒤 다시 부른다)
#[tauri::command]
pub async fn inspect_update(
    state: State<'_, AppState>,
    path: String,
    deck_ids: Option<Vec<i64>>,
    choices: Option<Choices>,
    voices: Option<Vec<String>>,
) -> Res<UpdateSummary> {
    let selected: Option<HashSet<i64>> = deck_ids.map(|v| v.into_iter().collect());
    let (choices, voices) = (choices.unwrap_or_default(), voices.unwrap_or_default());
    let req = Req { path: Path::new(&path), selected: selected.as_ref(), choices: &choices, voices: &voices, emit: &|_, _, _| {}, cancel: &|| false };
    let mut con = state.db.lock().map_err(err)?;
    run(&mut con, &state.data_dir, &req, true, &mut vec![])
}

/// [업데이트 적용]: 자동 백업(DU16) → 한 트랜잭션. 멈추면(cancel_import) 아무것도 바뀌지 않고 새로 쓴 미디어도 지운다 (I36).
/// 진행은 "update-progress" (B2-p). 동기화가 연결돼 있으면 화면이 먼저 받는다 (sync_now → sync_status, deck-update.md 9장)
#[tauri::command]
pub async fn apply_update(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    deck_ids: Option<Vec<i64>>,
    choices: Option<Choices>,
    voices: Option<Vec<String>>,
) -> Res<UpdateSummary> {
    state.import_cancel.store(false, Ordering::Relaxed);
    let selected: Option<HashSet<i64>> = deck_ids.map(|v| v.into_iter().collect());
    let (choices, voices) = (choices.unwrap_or_default(), voices.unwrap_or_default());
    let emit = |step: &'static str, done: usize, total: usize| {
        let _ = app.emit("update-progress", Progress { step, done, total });
    };
    let cancel = || state.import_cancel.load(Ordering::Relaxed);
    let req = Req { path: Path::new(&path), selected: selected.as_ref(), choices: &choices, voices: &voices, emit: &emit, cancel: &cancel };
    apply(&mut *state.db.lock().map_err(err)?, &state.data_dir, &req)
}

/// 자동 백업 → 적용. 실패하면 새로 쓴 미디어 파일을 지운다 (DB 는 트랜잭션이 되돌린다)
pub(crate) fn apply(con: &mut Connection, data_dir: &Path, req: &Req) -> Res<UpdateSummary> {
    (req.emit)("backup", 0, 1);
    let backup = crate::commands::write_backup_as(con, data_dir, "update").map_err(|e| format!("{NO_BACKUP} {e}"))?;
    (req.emit)("backup", 1, 1);
    if (req.cancel)() {
        return Err(STOPPED.into());
    }
    let mut written = vec![];
    let result = run(con, data_dir, req, false, &mut written);
    if result.is_err() {
        for f in written {
            let _ = std::fs::remove_file(f);
        }
    }
    let mut s = result?;
    s.backup = Some(backup.display().to_string());
    // 되돌리기 머리에 적용 전 백업 (설정 › 이 기기 백업의 그 줄에 [이 업데이트 되돌리기])
    let name = backup.file_name().map(|n| n.to_string_lossy().into_owned());
    con.execute("UPDATE update_journal SET after = json_set(after, '$.backup', ?2) WHERE batch_id = ?1 AND kind = 'meta'", params![s.batch, name]).map_err(err)?;
    Ok(s)
}

// ------------------------------------------------------------------ 새 버전에 없는 노트 정리 (DU59 · 초안, 사용자 결정 대기)

/// [정리하기] 확인 창 · 결과의 수
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct TidySummary {
    /// 정리할 노트 (원본이 있는 노트만, DU11 의 "새 버전에 없는 노트")
    notes: usize,
    cards: usize,
    /// 그 사이 공부한 카드 (기록이 있거나 학습을 시작했다, STUDIED)
    studied: usize,
    /// 이미 학습 중지인 카드
    suspended: usize,
    /// 지우기 전에 만든 백업 이름 (-vanish)
    backup: Option<String>,
}

/// DU59 (초안 · 사용자 결정 대기) 새 버전에 없는 노트를 한 번에 정리한다. 업데이트 적용과 따로 (적용 전 요약 · 적용 뒤 결과 어디서든)이고
/// 되돌리기 기록(update_journal)에는 들어가지 않는다. action: "suspend" 학습 중지 (기본, [학습 중지 풀기]로 되돌린다) · "delete" 지우기
/// (지우기 전 자동 백업 -vanish, 복습 기록은 통계에만 남는다 A4, 원본이 있어 다음 업데이트도 다시 넣지 않는다 DU3). 동기화는 보통 편집 · 삭제 (K29).
/// card_ids 는 요약의 missing_card_ids: 그 카드의 노트 중 원본이 있는 것만 본다
pub(crate) fn tidy_missing(con: &mut Connection, data_dir: &Path, card_ids: &[i64], action: &str, dry: bool) -> Res<TidySummary> {
    if !["suspend", "delete"].contains(&action) {
        return Err("정리할 방법을 골라 주세요.".into());
    }
    if card_ids.is_empty() {
        return Ok(TidySummary::default());
    }
    let ids = crate::commands::id_list(card_ids);
    let notes: Vec<i64> = con
        .prepare(&format!("SELECT DISTINCT n.id FROM note n JOIN card c ON c.note_id = n.id WHERE c.id IN ({ids}) AND n.origin IS NOT NULL ORDER BY n.id"))
        .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
        .map_err(err)?;
    let list = if notes.is_empty() { "-1".to_string() } else { crate::commands::id_list(&notes) };
    let n = |sql: &str| -> Res<usize> { con.query_row(&format!("SELECT count(*) FROM card c WHERE c.note_id IN ({list}) {sql}"), [], |r| r.get::<_, i64>(0)).map(|v| v as usize).map_err(err) };
    let mut s = TidySummary { notes: notes.len(), cards: n("")?, studied: n(&format!("AND {STUDIED}"))?, suspended: n("AND c.state = 'suspended'")?, backup: None };
    if dry || notes.is_empty() {
        return Ok(s);
    }
    let now = now_ms();
    if action == "delete" {
        let b = crate::commands::write_backup_as(con, data_dir, "vanish").map_err(|e| format!("지우기 전 백업을 만들지 못해 지우지 않았어요. {e}"))?;
        s.backup = b.file_name().map(|x| x.to_string_lossy().into_owned());
    }
    let tx = con.transaction().map_err(err)?;
    if action == "suspend" {
        tx.execute(&format!("UPDATE card SET state = 'suspended', updated_at = ?1 WHERE note_id IN ({list}) AND state <> 'suspended'"), [now]).map_err(err)?;
    } else {
        // 카드 목록 [삭제] 와 같다 (commands::delete_cards): 기록은 card_id 가 NULL 이 되어 통계에만 남는다
        tx.execute(&format!("DELETE FROM card WHERE note_id IN ({list})"), []).map_err(err)?;
        tx.execute(&format!("DELETE FROM note WHERE id IN ({list})"), []).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(s)
}

/// B2-u · B3-u [정리하기]. dry_run 이면 확인 창의 수만
#[tauri::command]
pub fn tidy_missing_notes(state: State<'_, AppState>, card_ids: Vec<i64>, action: String, dry_run: bool) -> Res<TidySummary> {
    tidy_missing(&mut *state.db.lock().map_err(err)?, &state.data_dir, &card_ids, &action, dry_run)
}

// ------------------------------------------------------------------ 업데이트 되돌리기 (DU57 · DU58)

/// 사용자 결정 2026-10-19 ①: 업데이트 뒤 공부한 새 카드(그 노트 포함)는 되돌리기가 건드리지 않는다 (기록 · 일정 보존).
/// 공부하지 않은 새 노트 · 카드는 지우지 않고 보관한다 (학습 중지 + ARCHIVE_TAG). 바꾸려면 이 두 곳
const KEEP_STUDIED: bool = true;
pub(crate) const ARCHIVE_TAG: &str = "되돌린_업데이트";
/// 공부한 카드 (별칭 c): 기록이 있거나 학습을 시작했다
const STUDIED: &str = "(EXISTS (SELECT 1 FROM review_log l WHERE l.card_id = c.id) OR c.reps > 0 OR c.state IN ('learning', 'review', 'relearning'))";

/// 되돌리기가 비교 · 되돌리는 노트 열
fn note_row(tx: &Connection, id: i64) -> Res<serde_json::Value> {
    tx.query_row("SELECT uid, fields, tags, exam_hits, exam_names, origin FROM note WHERE id = ?1", [id], |r| {
        Ok(serde_json::json!({
            "uid": r.get::<_, String>(0)?, "fields": r.get::<_, String>(1)?, "tags": r.get::<_, String>(2)?,
            "exam_hits": r.get::<_, Option<i64>>(3)?, "exam_names": r.get::<_, Option<String>>(4)?, "origin": r.get::<_, Option<String>>(5)?,
        }))
    })
    .map_err(err)
}

/// 되돌리기가 비교 · 되돌리는 노트 유형 열 (필드 이름은 템플릿 참조를 지금 이름으로 옮길 때만 쓴다)
fn type_row(tx: &Connection, id: i64) -> Res<serde_json::Value> {
    let (uid, fields, css, origin): (String, String, String, Option<String>) =
        tx.query_row("SELECT uid, fields, css, origin FROM note_type WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(err)?;
    let mut st = tx.prepare("SELECT ord, qfmt, afmt FROM card_template WHERE note_type_id = ?1 ORDER BY ord").map_err(err)?;
    let templates: Vec<(u32, String, String)> = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
    Ok(serde_json::json!({ "uid": uid, "fields": fields, "css": css, "origin": origin, "templates": templates }))
}

/// 바뀐 행의 전후를 남긴다 (같으면 남기지 않는다)
fn journal(tx: &Connection, batch: i64, kind: &str, before: &serde_json::Value, after: &serde_json::Value) -> Res<()> {
    if before == after {
        return Ok(());
    }
    tx.execute(
        "INSERT OR REPLACE INTO update_journal (batch_id, kind, uid, before, after) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![batch, kind, after["uid"].as_str().or(before["uid"].as_str()).unwrap_or_default(), before.to_string(), after.to_string()],
    )
    .map_err(err)?;
    Ok(())
}

/// 적용 끝에: 새 노트 · 이미 있던 노트의 새 카드, 머리 (요약 수 · 파일)
fn journal_created(tx: &Connection, batch: i64, max_note: i64, max_card: i64, s: &UpdateSummary) -> Res<()> {
    tx.execute("INSERT OR IGNORE INTO update_journal (batch_id, kind, uid) SELECT ?1, 'note_new', uid FROM note WHERE id > ?2", params![batch, max_note]).map_err(err)?;
    tx.execute(
        "INSERT OR IGNORE INTO update_journal (batch_id, kind, uid) SELECT ?1, 'card_new', uid FROM card WHERE id > ?2 AND note_id <= ?3",
        params![batch, max_card, max_note],
    )
    .map_err(err)?;
    let structure: usize = s.types.iter().map(|t| t.new_fields.len() + t.new_templates.len()).sum();
    let meta = serde_json::json!({ "file": s.file_name, "changed": s.changed, "merged": s.merged, "conflicts": s.conflicts.len(), "roots": s.roots, "structure": structure });
    tx.execute("INSERT OR REPLACE INTO update_journal (batch_id, kind, uid, after) VALUES (?1, 'meta', '', ?2)", params![batch, meta.to_string()]).map_err(err)?;
    Ok(())
}

/// DU2-u 확인 창 · DU2-u2 결과
#[derive(Serialize, Default)]
pub struct UndoSummary {
    batch: i64,
    file_name: String,
    /// 업데이트를 적용한 시각
    applied_at: i64,
    /// 적용 때의 수 ("바뀐 노트 137개(새 버전 128 · 합친 9)")
    changed: usize,
    merged: usize,
    /// 업데이트 전 내용으로 돌아가는 노트
    reverted_notes: usize,
    /// 업데이트 전 모양으로 돌아가는 노트 유형 (카드 종류 앞 · 뒷면 · CSS)
    reverted_types: usize,
    /// 공부하지 않아 보관하는 새 노트 (그 카드 전부) · 보관하는 카드 (새 노트의 카드 + 이미 있던 노트에 생긴 카드)
    archived_notes: usize,
    archived_cards: usize,
    /// 그중 보관한 새 노트의 카드 (나머지는 이미 있던 노트 · 공부한 새 노트에 생긴 새 카드)
    archived_note_cards: usize,
    archived_card_ids: Vec<i64>,
    /// 그 사이 공부해 남기는 새 카드 (기록 · 일정 그대로)
    kept_cards: usize,
    kept_card_ids: Vec<i64>,
    /// 그 뒤에 고친 (또는 지운) 노트라 그대로 두는 노트 · 그 카드
    edited_notes: usize,
    edited_card_ids: Vec<i64>,
    /// 그 뒤에 고친 노트 유형 (모양을 되돌리지 않는다)
    edited_types: usize,
    /// 되돌릴 수 없으면 이유 (이미 되돌림 · 그 뒤 같은 노트를 다시 업데이트함 · 기록 없음)
    blocked: Option<String>,
    /// 되돌리기 전에 만든 백업
    backup: Option<String>,
    /// 업데이트한 덱의 최상위 덱 ([덱 열기])
    roots: Vec<i64>,
    /// 업데이트로 더한 필드 · 카드 종류 수 (되돌리지 않고 남긴다)
    kept_structure: usize,
}

const NO_JOURNAL: &str = "이 업데이트는 되돌릴 기록이 없어요. 설정 › 이 기기 백업의 [백업으로 복구]를 써 주세요.";
const UNDONE: &str = "이미 되돌린 업데이트예요.";
const LATER: &str = "그 뒤에 같은 덱을 다시 업데이트해서 마지막 업데이트만 되돌릴 수 있어요. 이 업데이트는 [백업으로 복구]로 돌려요.";

/// 되돌리기. dry 면 같은 일을 해 보고 되돌린다 (확인 창의 수). 되돌림은 보통 편집이다 (동기화로 다른 기기에도 간다)
pub(crate) fn undo(con: &mut Connection, batch: i64, dry: bool) -> Res<UndoSummary> {
    let tx = con.transaction().map_err(err)?;
    let (file_name, applied_at, status): (String, i64, String) = tx
        .query_row("SELECT source_name, started_at, status FROM import_batch WHERE id = ?1", [batch], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(err)?
        .ok_or("업데이트 기록을 찾을 수 없어요.")?;
    let mut s = UndoSummary { batch, file_name, applied_at, ..Default::default() };
    let meta: Option<String> = tx.query_row("SELECT after FROM update_journal WHERE batch_id = ?1 AND kind = 'meta'", [batch], |r| r.get(0)).optional().map_err(err)?;
    let Some(meta) = meta.and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok()) else {
        s.blocked = Some(NO_JOURNAL.into());
        return Ok(s);
    };
    let num = |k: &str| meta[k].as_u64().unwrap_or_default() as usize;
    (s.changed, s.merged, s.kept_structure) = (num("changed"), num("merged"), num("structure"));
    s.roots = serde_json::from_value(meta["roots"].clone()).unwrap_or_default();
    let later: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM update_journal j JOIN import_batch b ON b.id = j.batch_id
             WHERE j.batch_id > ?1 AND b.status <> 'rolled_back' AND j.kind <> 'meta' AND j.uid IN (SELECT uid FROM update_journal WHERE batch_id = ?1 AND kind <> 'meta'))",
            [batch],
            |r| r.get(0),
        )
        .map_err(err)?;
    s.blocked = if status == "rolled_back" { Some(UNDONE.into()) } else if later { Some(LATER.into()) } else { None };
    if s.blocked.is_some() {
        return Ok(s);
    }
    let rows: Vec<(String, String, Option<String>, Option<String>)> = tx
        .prepare("SELECT kind, uid, before, after FROM update_journal WHERE batch_id = ?1 AND kind <> 'meta' ORDER BY kind, uid")
        .and_then(|mut st| st.query_map([batch], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
        .map_err(err)?;
    let now = now_ms();
    let json = |x: &Option<String>| x.as_deref().and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok()).unwrap_or_default();
    let id_of = |t: &str, uid: &str| tx.query_row(&format!("SELECT id FROM {t} WHERE uid = ?1"), [uid], |r| r.get::<_, i64>(0)).optional().map_err(err);
    let cards_of = |note: i64, studied: bool| -> Res<Vec<i64>> {
        tx.prepare_cached(&format!("SELECT c.id FROM card c WHERE c.note_id = ?1 AND {}{STUDIED} ORDER BY c.id", if studied { "" } else { "NOT " }))
            .and_then(|mut st| st.query_map([note], |r| r.get(0))?.collect())
            .map_err(err)
    };
    let studied = |card: i64| -> Res<bool> {
        tx.query_row(&format!("SELECT {STUDIED} FROM card c WHERE c.id = ?1"), [card], |r| r.get(0)).map_err(err)
    };
    // 보관: 학습 중지 (신규 카드만. 이미 학습 중지면 그대로)
    let archive = |card: i64| tx.execute("UPDATE card SET state = 'suspended', updated_at = ?2 WHERE id = ?1 AND state = 'new'", params![card, now]).map_err(err);
    for (kind, uid, before, after) in &rows {
        match kind.as_str() {
            "note" => {
                let Some(id) = id_of("note", uid)? else {
                    s.edited_notes += 1; // 그 뒤에 지웠다
                    continue;
                };
                if note_row(&tx, id)? != json(after) {
                    s.edited_notes += 1;
                    s.edited_card_ids.extend(cards_of(id, true)?.into_iter().chain(cards_of(id, false)?));
                    continue;
                }
                let (b, a) = (json(before), json(after));
                // 원본만 바뀐 노트(내용은 그대로)는 세지 않고 고친 시각도 그대로: "돌아간 노트"는 내용이 돌아간 노트
                let content = |v: &serde_json::Value| (v["fields"].clone(), v["tags"].clone(), v["exam_hits"].clone(), v["exam_names"].clone());
                let changed = content(&b) != content(&a);
                tx.execute(
                    "UPDATE note SET fields = ?2, tags = ?3, exam_hits = ?4, exam_names = ?5, origin = ?6, updated_at = CASE WHEN ?8 THEN ?7 ELSE updated_at END WHERE id = ?1",
                    params![id, b["fields"].as_str(), b["tags"].as_str(), b["exam_hits"].as_i64(), b["exam_names"].as_str(), b["origin"].as_str(), now, changed],
                )
                .map_err(err)?;
                s.reverted_notes += changed as usize;
            }
            "note_new" => {
                let Some(id) = id_of("note", uid)? else { continue };
                let (kept, rest) = (cards_of(id, true)?, cards_of(id, false)?);
                if KEEP_STUDIED && !kept.is_empty() {
                    s.kept_cards += kept.len();
                    s.kept_card_ids.extend(kept);
                } else {
                    s.archived_notes += 1;
                    s.archived_note_cards += rest.len() + if KEEP_STUDIED { 0 } else { kept.len() };
                    tx.execute(
                        "UPDATE note SET tags = trim(tags || ' ' || ?2), updated_at = ?3 WHERE id = ?1 AND (' ' || tags || ' ') NOT LIKE '% ' || ?2 || ' %'",
                        params![id, ARCHIVE_TAG, now],
                    )
                    .map_err(err)?;
                }
                // 공부한 카드가 있는 노트도 공부하지 않은 카드는 보관한다
                for c in if KEEP_STUDIED { rest } else { cards_of(id, true)?.into_iter().chain(rest).collect() } {
                    archive(c)?;
                    s.archived_cards += 1;
                    s.archived_card_ids.push(c);
                }
            }
            "card_new" => {
                let Some(id) = id_of("card", uid)? else { continue };
                if KEEP_STUDIED && studied(id)? {
                    s.kept_cards += 1;
                    s.kept_card_ids.push(id);
                } else {
                    archive(id)?;
                    s.archived_cards += 1;
                    s.archived_card_ids.push(id);
                }
            }
            "type" => {
                let Some(id) = id_of("note_type", uid)? else { continue };
                if type_row(&tx, id)? != json(after) {
                    s.edited_types += 1;
                    continue;
                }
                // 모양 · 원본만 되돌린다. 더한 필드 · 카드 종류 · 이름은 남긴다 (그 카드 · 값이 남아 있다). 옛 템플릿의 필드 이름은 지금 이름으로
                let b = json(before);
                let nt = notetype::load(&tx, id)?;
                let old: Vec<String> = serde_json::from_str(b["fields"].as_str().unwrap_or("[]")).unwrap_or_default();
                let names: HashMap<String, String> =
                    old.iter().zip(&nt.slots).filter(|(o, n)| !o.is_empty() && !n.is_empty() && o != n).map(|(o, n)| (o.clone(), n.clone())).collect();
                let tpls: Vec<(u32, String, String)> = serde_json::from_value(b["templates"].clone()).unwrap_or_default();
                for (ord, q, a) in tpls {
                    let (q, a) = (notetype::rename_all_refs(&q, &names), notetype::rename_all_refs(&a, &names));
                    tx.execute(
                        "UPDATE card_template SET qfmt = ?3, afmt = ?4, updated_at = ?5 WHERE note_type_id = ?1 AND ord = ?2 AND (qfmt <> ?3 OR afmt <> ?4)",
                        params![id, ord, q, a, now],
                    )
                    .map_err(err)?;
                }
                tx.execute(
                    "UPDATE note_type SET css = ?2, origin = ?3, updated_at = ?4 WHERE id = ?1",
                    params![id, b["css"].as_str().unwrap_or_default(), b["origin"].as_str(), now],
                )
                .map_err(err)?;
                s.reverted_types += 1;
            }
            _ => {}
        }
    }
    tx.execute("UPDATE import_batch SET status = 'rolled_back' WHERE id = ?1", [batch]).map_err(err)?;
    if dry {
        tx.rollback().map_err(err)?;
    } else {
        tx.commit().map_err(err)?;
    }
    Ok(s)
}

/// 설정 › 이 기기 백업의 [이 업데이트 되돌리기] (DU2-u): 그 백업을 만든 업데이트
#[derive(Serialize)]
pub struct Undoable {
    batch: i64,
    file_name: String,
    applied_at: i64,
    /// 적용 전 백업 파일 이름 (local_backups 의 name)
    backup: Option<String>,
    blocked: Option<String>,
}

#[tauri::command]
pub fn undoable_updates(state: State<'_, AppState>) -> Res<Vec<Undoable>> {
    let mut con = state.db.lock().map_err(err)?;
    let rows: Vec<(i64, Option<String>)> = con
        .prepare("SELECT b.id, json_extract(j.after, '$.backup') FROM import_batch b JOIN update_journal j ON j.batch_id = b.id AND j.kind = 'meta' ORDER BY b.id DESC LIMIT 50")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(err)?;
    let mut out = vec![];
    for (batch, backup) in rows {
        let s = undo(&mut con, batch, true)?;
        out.push(Undoable { batch, file_name: s.file_name, applied_at: s.applied_at, backup, blocked: s.blocked });
    }
    Ok(out)
}

/// [이 업데이트 되돌리기] (DU2-u). dry_run 이면 확인 창의 수만. 적용은 되돌리기 전 백업(-undo) → 한 트랜잭션
#[tauri::command]
pub fn undo_update(state: State<'_, AppState>, batch: i64, dry_run: bool) -> Res<UndoSummary> {
    let mut con = state.db.lock().map_err(err)?;
    if dry_run {
        return undo(&mut con, batch, true);
    }
    let first = undo(&mut con, batch, true)?;
    if let Some(why) = first.blocked {
        return Err(why);
    }
    let backup = crate::commands::write_backup_as(&con, &state.data_dir, "undo").map_err(|e| format!("되돌리기 전 백업을 만들지 못해 되돌리지 않았어요. {e}"))?;
    let mut s = undo(&mut con, batch, false)?;
    s.backup = Some(backup.display().to_string());
    Ok(s)
}

/// 시험용 .apkg (구버전 스키마 11): 노트타입 · 덱 · 노트 (카드마다 복습 기록 하나) · 미디어. 덱 업데이트 · 동기화 시험이 같이 쓴다
#[cfg(test)]
pub(crate) mod testpkg {
    use rusqlite::{Connection, params};
    use serde_json::{Map, Value, json};
    use std::io::Write;
    use std::path::Path;

    pub(crate) struct Model {
        pub id: i64,
        pub name: &'static str,
        pub fields: Vec<&'static str>,
        /// (이름, 앞면, 뒷면). ord 는 순서
        pub templates: Vec<(&'static str, &'static str, &'static str)>,
        pub css: &'static str,
    }

    pub(crate) struct Note {
        pub id: i64,
        pub guid: &'static str,
        pub mid: i64,
        pub tags: &'static str,
        pub fields: Vec<String>,
        /// (덱 id, ord). 카드 id 는 노트 id × 10 + ord
        pub cards: Vec<(i64, i64)>,
    }

    pub(crate) fn note(id: i64, guid: &'static str, mid: i64, fields: &[&str], cards: &[(i64, i64)]) -> Note {
        Note { id, guid, mid, tags: "", fields: fields.iter().map(|f| f.to_string()).collect(), cards: cards.to_vec() }
    }

    const TABLES: &str = "CREATE TABLE col (id integer primary key, crt integer, mod integer, scm integer, ver integer, dty integer, usn integer, ls integer,
                           conf text, models text, decks text, dconf text, tags text);
         CREATE TABLE notes (id integer primary key, guid text, mid integer, mod integer, usn integer, tags text, flds text, sfld text,
                             csum integer, flags integer, data text);
         CREATE TABLE cards (id integer primary key, nid integer, did integer, ord integer, mod integer, usn integer, type integer,
                             queue integer, due integer, ivl integer, factor integer, reps integer, lapses integer, left integer,
                             odue integer, odid integer, flags integer, data text);
         CREATE TABLE revlog (id integer primary key, cid integer, usn integer, ease integer, ivl integer, lastIvl integer,
                              factor integer, time integer, type integer);";

    pub(crate) fn write(path: &Path, models: &[Model], decks: &[(i64, &str)], notes: &[Note], media: &[(&str, &[u8])]) {
        let db = path.with_extension("anki2");
        let _ = std::fs::remove_file(&db);
        {
            let col = Connection::open(&db).unwrap();
            col.execute_batch(TABLES).unwrap();
            let models: Map<String, Value> = models
                .iter()
                .map(|m| {
                    let flds: Vec<Value> = m.fields.iter().map(|f| json!({ "name": f })).collect();
                    let tmpls: Vec<Value> = m.templates.iter().enumerate().map(|(i, (n, q, a))| json!({ "ord": i, "name": n, "qfmt": q, "afmt": a })).collect();
                    (m.id.to_string(), json!({ "name": m.name, "css": m.css, "flds": flds, "tmpls": tmpls }))
                })
                .collect();
            let decks: Map<String, Value> = decks.iter().map(|(id, n)| (id.to_string(), json!({ "name": n, "dyn": 0 }))).collect();
            col.execute("INSERT INTO col VALUES (1, 0, 0, 0, 11, 0, 0, 0, '{}', ?1, ?2, '{}', '{}')", params![Value::Object(models).to_string(), Value::Object(decks).to_string()])
                .unwrap();
            for n in notes {
                col.execute("INSERT INTO notes VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, '', 0, 0, '')", params![n.id, n.guid, n.mid, n.tags, n.fields.join("\x1f")]).unwrap();
                for (did, ord) in &n.cards {
                    let cid = n.id * 10 + ord;
                    col.execute("INSERT INTO cards VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0, ?1, 0, 0, 0, 0, 0, 0, 0, 0, '')", params![cid, n.id, did, ord]).unwrap();
                    col.execute("INSERT INTO revlog VALUES (?1, ?2, 0, 3, 1, 0, 2500, 1000, 1)", params![cid * 100, cid]).unwrap();
                }
            }
        }
        let list: Map<String, Value> = media.iter().enumerate().map(|(i, (n, _))| (i.to_string(), json!(n))).collect();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("collection.anki2", opts).unwrap();
        zip.write_all(&std::fs::read(&db).unwrap()).unwrap();
        zip.start_file("media", opts).unwrap();
        zip.write_all(Value::Object(list).to_string().as_bytes()).unwrap();
        for (i, (_, b)) in media.iter().enumerate() {
            zip.start_file(i.to_string(), opts).unwrap();
            zip.write_all(b).unwrap();
        }
        zip.finish().unwrap();
        let _ = std::fs::remove_file(db);
    }
}

#[cfg(test)]
mod tests {
    use super::testpkg::{self, Model, Note, note};
    use super::*;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(tag: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!("neoanki-du-{tag}-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(p.join("data/media")).unwrap();
        Tmp(p)
    }

    fn state(dir: &Path) -> AppState {
        std::fs::create_dir_all(dir.join("media")).unwrap();
        AppState {
            db: std::sync::Mutex::new(db::open(&dir.join("neoanki.sqlite")).unwrap()),
            fsrs: fsrs::FSRS::default(),
            data_dir: dir.to_path_buf(),
            undo: Default::default(),
            import_cancel: Default::default(),
            startup_error: Default::default(),
        }
    }

    fn import(s: &AppState, p: &Path) {
        run_import_anki(&|_, _, _| {}, s, p.to_str().unwrap(), false, &mut vec![]).unwrap();
    }

    fn update(s: &AppState, p: &Path, choices: &Choices, dry: bool) -> UpdateSummary {
        let req = Req { path: p, selected: None, choices, voices: &[], emit: &|_, _, _| {}, cancel: &|| false };
        let mut con = s.db.lock().unwrap();
        if dry { run(&mut con, &s.data_dir, &req, true, &mut vec![]).unwrap() } else { apply(&mut con, &s.data_dir, &req).unwrap() }
    }

    fn q<T: rusqlite::types::FromSql>(s: &AppState, sql: &str) -> T {
        s.db.lock().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn id_of(s: &AppState, guid: &str) -> i64 {
        q(s, &format!("SELECT id FROM note WHERE anki_guid = '{guid}'"))
    }

    fn fields(s: &AppState, guid: &str) -> Vec<String> {
        serde_json::from_str(&q::<String>(s, &format!("SELECT fields FROM note WHERE anki_guid = '{guid}'"))).unwrap()
    }

    /// 편집기 저장과 같은 길 (updated_at 이 지금 시각이 된다)
    fn edit(s: &AppState, guid: &str, slot: usize, value: &str) {
        let mut f = fields(s, guid);
        f[slot] = value.into();
        let id = id_of(s, guid);
        let mut con = s.db.lock().unwrap();
        let tx = con.transaction().unwrap();
        crate::commands::save_note_fields(&tx, id, &f).unwrap();
        tx.commit().unwrap();
    }

    /// 덱 업데이트가 건드릴 수 있는 표 전부 (import_batch 는 기록이라 뺀다)
    fn dump(s: &AppState) -> Vec<String> {
        let con = s.db.lock().unwrap();
        let mut out = vec![];
        for t in ["note", "card", "note_type", "card_template", "deck", "media", "review_log", "origin_gone"] {
            let mut st = con.prepare(&format!("SELECT * FROM {t} ORDER BY rowid")).unwrap();
            let n = st.column_count();
            let rows = st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))).unwrap();
            out.extend(rows.map(|r| format!("{t}: {}", r.unwrap())));
        }
        out
    }

    fn files(dir: &Path) -> BTreeSet<String> {
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect()
    }

    const AFMT: &str = "{{FrontSide}}<hr id=answer>{{Back}}";

    fn basic(v2: bool) -> Model {
        if v2 {
            Model {
                id: 7,
                name: "Basic",
                fields: vec!["Front", "Hint", "Back"],
                templates: vec![("Card 1", "{{Front}}<br>{{Hint}}", AFMT), ("Card 2", "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}")],
                css: ".card { color: black; }",
            }
        } else {
            Model { id: 7, name: "Basic", fields: vec!["Front", "Back"], templates: vec![("Card 1", "{{Front}}", AFMT)], css: ".card { color: black; }" }
        }
    }

    fn cloze() -> Model {
        Model { id: 8, name: "Cloze", fields: vec!["Text", "Back Extra"], templates: vec![("Cloze", "{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}")], css: "" }
    }

    #[test]
    fn judgement_table() {
        let (b, m, n, x) = ("b", "m", "n", "x");
        // 판정표 (deck-update.md 2장 3) 일곱 줄
        assert_eq!(pick(Some(b), b, b, true), Pick::Same, "B = M = N");
        assert_eq!(pick(Some(b), b, n, true), Pick::New, "B = M ≠ N: 만든 사람만 고침");
        assert_eq!(pick(Some(b), m, m, true), Pick::Same, "B ≠ M = N: 둘이 같게 고침 (원본만 새로)");
        assert_eq!(pick(Some(b), m, b, false), Pick::Mine, "B = N ≠ M: 나만 고침");
        assert_eq!(pick(Some(b), m, x, false), Pick::Conflict, "셋 다 다르다");
        assert_eq!(pick(None, m, m, true), Pick::Same, "원본 없음 · M = N");
        assert_eq!(pick(None, m, n, false), Pick::New, "원본 없음 · 고친 흔적 없음 (DU18)");
        assert_eq!(pick(None, m, n, true), Pick::Conflict, "원본 없음 · 고친 흔적 있음 (DU18)");
        // 편집기가 바꾼 모양은 내 수정이 아니다
        assert_eq!(h("a  b<br/>c&nbsp;d<br />\n<br>"), h("a b<br>c d"));
        assert_ne!(h("a b"), h("a c"));
        assert_eq!(h("").len(), 16);
        // 태그 집합 3-way: 내가 더한 c 는 남고, 만든 사람이 뺀 b 는 빠지고 더한 d 는 들어온다
        assert_eq!(merge_tags(Some("a b"), "a b c", "a d"), "a c d");
        assert_eq!(merge_tags(None, "a b", "b d"), "b d", "원본이 없으면 새 버전 태그");
        assert_eq!(merge_tags(Some("a"), "a", "a"), "a");
    }

    #[test]
    fn round_trip_v1_to_v2() {
        let t = tmp("rt");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let decks = [(1, "공유덱"), (2, "공유덱::2장")];
        let b1 = |id, g, f: &str, b: &str| note(id, g, 7, &[f, b], &[(2, 0)]);
        let b2 = |id, g, f: &str, b: &str| note(id, g, 7, &[f, "", b], &[(2, 0)]);
        let cz = |id, g, text: &str, ords: &[i64]| Note { cards: ords.iter().map(|o| (2, *o)).collect(), ..note(id, g, 8, &[text, ""], &[]) };
        let img = r#"<img src="pic.png">"#;
        let v1 = vec![
            b1(1, "g_fix", "TCP는?", "연결 지향"),
            b1(2, "g_mine", "UDP는?", "비연결"),
            b1(3, "g_merge", "IP는?", "주소"),
            b1(4, "g_conf", "ARP는?", "주소 변환"),
            b1(5, "g_conf2", "DNS는?", "이름"),
            b1(6, "g_gone", "지울 카드", "x"),
            b1(7, "g_missing", "빠질 카드", "y"),
            cz(8, "g_cloze", "{{c1::TCP}} 와 {{c2::UDP}}", &[0, 1]),
            b1(9, "g_img", "그림", img),
            Note { tags: "기출::2023", ..b1(10, "g_tag", "기출 문제", "답") },
            b1(11, "g_same", "그대로", "z"),
        ];
        testpkg::write(&p1, &[basic(false), cloze()], &decks, &v1, &[("pic.png", b"AAAA")]);
        // v2: 필드 수정 · 새 노트 · 사라진 노트(g_missing) · 빈칸 번호 변화(c2 → c3) · 템플릿 변경(Card 1 앞면, 새 필드 Hint, 새 카드 종류 Card 2) ·
        // 미디어 같은 이름 다른 내용
        let v2 = vec![
            b2(1, "g_fix", "TCP는?", "연결 지향 · 신뢰성"),
            b2(2, "g_mine", "UDP는?", "비연결"),
            b2(3, "g_merge", "IP 는?", "주소"),
            b2(4, "g_conf", "ARP는?", "IP→MAC"),
            b2(5, "g_conf2", "DNS는?", "이름 풀이"),
            b2(6, "g_gone", "지울 카드", "x"),
            cz(8, "g_cloze", "{{c1::TCP}} 와 {{c3::UDP}}", &[0, 2]),
            b2(9, "g_img", "그림", img),
            Note { tags: "기출::2023 기출::2024", ..b2(10, "g_tag", "기출 문제", "답") },
            b2(11, "g_same", "그대로", "z"),
            Note { cards: vec![(2, 0), (2, 1)], ..b2(12, "g_new", "새 노트", "새 답") },
        ];
        testpkg::write(&p2, &[basic(true), cloze()], &decks, &v2, &[("pic.png", b"BBBB")]);

        import(&s, &p1);
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM note WHERE origin IS NULL"), 0, "처음 가져올 때 원본을 남긴다");
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM note_type WHERE anki_model_id IS NOT NULL AND origin IS NOT NULL"), 2);
        let o: NoteOrigin = serde_json::from_str(&q::<String>(&s, "SELECT origin FROM note WHERE anki_guid = 'g_tag'")).unwrap();
        assert_eq!((o.f, o.t.as_str()), (vec![h("기출 문제"), h("답")], "기출::2023"));

        // 내 수정 · 학습 · 지우기
        edit(&s, "g_mine", 1, "비연결 (내 메모)");
        edit(&s, "g_merge", 1, "주소 체계");
        edit(&s, "g_conf", 1, "MAC 찾기");
        edit(&s, "g_conf2", 1, "도메인");
        {
            let con = s.db.lock().unwrap();
            con.execute(
                "UPDATE card SET state = 'review', due_at = 123456, stability = 5.5, difficulty = 4.5, reps = 3, importance = 2, importance_source = 'user'
                 WHERE note_id = (SELECT id FROM note WHERE anki_guid = 'g_fix') OR (note_id = (SELECT id FROM note WHERE anki_guid = 'g_cloze') AND template_ord = 1)",
                [],
            )
            .unwrap();
            con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) SELECT id, 1000, 3 FROM card WHERE state = 'review'", []).unwrap();
            con.execute("DELETE FROM card WHERE note_id = (SELECT id FROM note WHERE anki_guid = 'g_gone')", []).unwrap();
            con.execute("DELETE FROM note WHERE anki_guid = 'g_gone'", []).unwrap();
        }
        assert_eq!(q::<String>(&s, "SELECT group_concat(guid) FROM origin_gone"), "g_gone", "내가 지운 노트를 기억한다 (DU3)");
        let reviewed = || q::<String>(&s, "SELECT group_concat(state || due_at || stability || reps || importance, ',') FROM card WHERE state = 'review'");
        let reviewed_before = reviewed();

        // 사전 검사는 아무것도 쓰지 않는다 (DB · 미디어 파일 · 가져오기 기록)
        let media = s.data_dir.join("media");
        let (before, files_before) = (dump(&s), files(&media));
        let r = update(&s, &p2, &Choices::default(), true);
        assert_eq!(dump(&s), before);
        assert_eq!(files(&media), files_before);
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM import_batch"), 1);

        // 요약 (B2-u): 적용 전에 모든 영향
        assert_eq!(
            (r.changed, r.merged, r.conflicts.len(), r.mine_kept, r.same, r.no_base, r.other_type),
            (4, 1, 2, 1, 1, 0, 0),
            "g_fix · g_cloze · g_img · g_tag / g_merge / g_conf · g_conf2 / g_mine / g_same"
        );
        assert_eq!((r.new_notes, r.new_note_cards, r.gone, r.missing_notes, r.missing_card_ids.len()), (1, 2, 1, 1, 2), "g_missing 의 카드 + 새 카드 종류로 생긴 카드");
        assert_eq!((r.new_cards, r.emptied_cards), (10, 1), "Card 2 아홉 장 + c3 / c2");
        assert_eq!((r.cloze.renumbered_notes, r.cloze.new_cards, r.cloze.emptied_cards), (1, 1, 1));
        assert_eq!((r.media_new, r.media_renamed, r.media_compared, r.reviews_skipped, r.kept_looks), (1, 1, 1, 13, 0));
        assert_eq!((r.hint.notes, r.hint.existing_notes, r.hint.known_decks.clone(), r.hint.suggested), (11, 9, vec!["공유덱".to_string()], true));
        assert_eq!(r.types.len(), 1, "Cloze 는 바뀐 것이 없다");
        let ty = &r.types[0];
        assert_eq!(
            (ty.new_fields.clone(), ty.new_templates.clone(), ty.new_template_cards, ty.updated_looks.clone()),
            (vec!["Hint".to_string()], vec!["Card 2".to_string()], 9, vec!["Card 1".to_string()])
        );
        let conf = r.conflicts.iter().find(|c| c.note_id == id_of(&s, "g_conf")).unwrap();
        assert_eq!(
            (conf.title.as_str(), conf.deck.as_str(), conf.fields.len(), conf.fields[0].name.as_str(), conf.fields[0].mine.as_str(), conf.fields[0].new.as_str(), conf.take_new),
            ("ARP는?", "공유덱 › 2장", 1, "Back", "MAC 찾기", "IP→MAC", false)
        );
        // 고르기를 반영한 요약
        let choices = Choices { take_new: vec![id_of(&s, "g_conf2")], ..Default::default() };
        let r = update(&s, &p2, &choices, true);
        assert!(r.conflicts.iter().any(|c| c.note_id == id_of(&s, "g_conf2") && c.take_new));

        // 적용
        let r = update(&s, &p2, &choices, false);
        assert!(r.backup.as_ref().is_some_and(|b| Path::new(b).exists()), "적용 전 자동 백업 (DU16)");
        let sha8 = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(b"BBBB"))[..8].to_string()
        };
        assert_eq!(fields(&s, "g_fix"), ["TCP는?", "연결 지향 · 신뢰성", ""], "새 버전 (Hint 는 끝 자리)");
        assert_eq!(fields(&s, "g_mine"), ["UDP는?", "비연결 (내 메모)"], "내 수정 지킴 (고치지 않은 노트는 다시 쓰지 않는다: 짧은 배열 = 빈 값)");
        assert_eq!(fields(&s, "g_merge"), ["IP 는?", "주소 체계", ""], "합침 (DU5)");
        assert_eq!(fields(&s, "g_conf")[1], "MAC 찾기", "충돌 기본은 내 것 (DU6)");
        assert_eq!(fields(&s, "g_conf2")[1], "이름 풀이", "고른 노트는 새 버전");
        assert_eq!(fields(&s, "g_img")[1], format!(r#"<img src="pic-{sha8}.png">"#), "같은 이름 다른 내용은 새 이름 (DU10)");
        assert_eq!(std::fs::read(media.join("pic.png")).unwrap(), b"AAAA");
        assert_eq!(std::fs::read(media.join(format!("pic-{sha8}.png"))).unwrap(), b"BBBB");
        assert_eq!(
            q::<String>(&s, "SELECT tags || ' ' || exam_hits || ' ' || exam_names FROM note WHERE anki_guid = 'g_tag'"),
            r#"기출::2023 기출::2024 2 ["2023","2024"]"#,
            "기출 표시를 새 태그로 다시 (DU7)"
        );
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM note WHERE anki_guid IN ('g_gone')"), 0, "내가 지운 노트는 다시 넣지 않는다");
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM note WHERE anki_guid = 'g_missing'"), 1, "새 버전에 없는 노트는 남긴다 (DU11)");
        assert_eq!(
            q::<String>(&s, "SELECT group_concat(d.name || c.template_ord || c.state) FROM card c JOIN deck d ON d.id = c.deck_id JOIN note n ON n.id = c.note_id WHERE n.anki_guid = 'g_new'"),
            "2장0new,2장1new"
        );
        // 일정 · 상태 · 중요도 · 기록은 그대로. 빈칸 c2 카드는 남고 (앞면 빔), c3 는 신규
        assert_eq!(reviewed(), reviewed_before);
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM review_log"), 2);
        assert_eq!(
            q::<String>(&s, "SELECT group_concat(template_ord || state) FROM (SELECT * FROM card WHERE note_id = (SELECT id FROM note WHERE anki_guid = 'g_cloze') ORDER BY template_ord)"),
            "0new,1review,2new"
        );
        // 노트 유형: 새 필드는 끝 자리 · 보이는 순서는 새 버전대로, Card 1 새 모양, Card 2 더함
        assert_eq!(q::<String>(&s, "SELECT fields || field_order FROM note_type WHERE anki_model_id = 7"), r#"["Front","Back","Hint"][0,2,1]"#);
        assert_eq!(
            q::<String>(&s, "SELECT group_concat(ord || name || qfmt) FROM card_template WHERE note_type_id = (SELECT id FROM note_type WHERE anki_model_id = 7)"),
            "0Card 1{{Front}}<br>{{Hint}},1Card 2{{Back}}"
        );
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM card WHERE template_ord = 1 AND note_id IN (SELECT id FROM note WHERE note_type_id = (SELECT id FROM note_type WHERE anki_model_id = 7))"), 10);
        // 원본 = 새 버전 (내 것을 지킨 필드는 다음에도 "내가 고침")
        let o: NoteOrigin = serde_json::from_str(&q::<String>(&s, "SELECT origin FROM note WHERE anki_guid = 'g_conf'")).unwrap();
        assert_eq!(o.f, [h("ARP는?"), h("IP→MAC"), h("")]);

        // 같은 파일을 다시: 바뀌는 것이 없다 (멱등). 동기화 변경 기록도 쌓이지 않는다
        s.db.lock().unwrap().execute("UPDATE sync_flags SET enabled = 1", []).unwrap();
        let after = dump(&s);
        let r = update(&s, &p2, &choices, false);
        assert_eq!(dump(&s), after);
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM sync_outbox"), 0);
        assert_eq!((r.changed, r.merged, r.conflicts.len(), r.new_notes, r.new_cards, r.media_new, r.types.len()), (0, 0, 0, 0, 0, 0, 0));
        assert_eq!((r.mine_kept, r.gone, r.missing_notes), (3, 1, 1), "g_mine · g_merge · g_conf 는 내 것으로 남는다");
    }

    #[test]
    fn note_type_table() {
        let t = tmp("nt");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let v1 = Model {
            id: 9,
            name: "Vocab",
            fields: vec!["Front", "Back", "Extra", "Note"],
            templates: vec![("Card 1", "{{Front}}", AFMT), ("Card 2", "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}"), ("Card 3", "{{Extra}}", "{{Back}}")],
            css: ".card {}",
        };
        testpkg::write(&p1, &[v1], &[(1, "단어")], &[note(1, "g1", 9, &["사과", "apple", "과일", "메모1"], &[(1, 0), (1, 1), (1, 2)])], &[]);
        import(&s, &p1);
        let ntid: i64 = q(&s, "SELECT id FROM note_type WHERE anki_model_id = 9");
        {
            // 내가: Front 이름을 바꾸고, Note 필드를 지우고(숨김), Card 2 를 지우고, Card 1 뒷면을 고쳤다
            let con = s.db.lock().unwrap();
            notetype::rename_field(&con, ntid, 0, "질문").unwrap();
            notetype::delete_field(&con, ntid, 3).unwrap();
            notetype::delete_template(&con, ntid, 1, false).unwrap();
            notetype::save_template(&con, ntid, 0, None, "{{질문}}", "{{FrontSide}}<hr id=answer>{{Back}}<br>{{Extra}}").unwrap();
        }
        // 만든 사람이: Back → Answer, Extra 와 Card 3 를 없애고, Card 1 뒷면 · CSS 를 바꾸고, Note 값을 고쳤다
        let v2afmt = "{{FrontSide}}<hr id=answer>{{Answer}}<div>v2</div>";
        let v2 = Model {
            id: 9,
            name: "Vocab",
            fields: vec!["Front", "Answer", "Note"],
            templates: vec![("Card 1", "{{Front}}", v2afmt), ("Card 2", "{{Answer}}", "{{FrontSide}}<hr id=answer>{{Front}}")],
            css: ".card { color: red; }",
        };
        let notes = [note(1, "g1", 9, &["사과", "apple", "메모2"], &[(1, 0)]), note(2, "g2", 9, &["배", "pear", ""], &[(1, 0), (1, 1)])];
        testpkg::write(&p2, &[v2], &[(1, "단어")], &notes, &[]);

        let r = update(&s, &p2, &Choices::default(), true);
        let ty = &r.types[0];
        assert_eq!(ty.renamed_fields, [("Back".to_string(), "Answer".to_string())], "내가 이름을 바꾸지 않은 필드만 새 이름으로");
        assert_eq!((ty.removed_fields.clone(), ty.removed_templates.clone()), (vec!["Extra".to_string()], vec!["Card 3".to_string()]), "없어진 필드 · 카드 종류는 남긴다");
        assert!(ty.new_fields.is_empty() && ty.new_templates.is_empty(), "내가 지운 Card 2 는 다시 넣지 않는다 (Q2-08)");
        assert_eq!(ty.updated_looks, ["CSS"], "만든 사람만 바꾼 CSS");
        assert_eq!(
            (ty.looks.len(), ty.looks[0].ord, ty.looks[0].take_new, r.kept_looks),
            (1, Some(0), false, 1),
            "둘 다 바꾼 Card 1 은 내 모양 (DU9). 앞면은 필드 이름만 바꿔 내 수정이 아니다"
        );
        assert_eq!(ty.looks[0].new_back, v2afmt, "새 모양은 우리 필드 이름으로");
        assert_eq!((r.changed, r.new_notes, r.new_note_cards), (1, 1, 1), "g1 의 숨긴 필드 값 · 새 노트 g2 (지운 Card 2 의 카드는 넣지 않는다)");
        assert!(r.issues.iter().any(|i| i.kind == "deleted_card_type"));

        let choices = Choices { new_looks: vec![(ntid, Some(0))], ..Default::default() };
        let r = update(&s, &p2, &choices, false);
        assert_eq!(r.kept_looks, 0);
        let nt = notetype::load(&s.db.lock().unwrap(), ntid).unwrap();
        assert_eq!(nt.slots, ["질문", "Answer", "Extra", ""]);
        assert_eq!(nt.templates.iter().map(|t| (t.ord, t.qfmt.as_str(), t.afmt.as_str())).collect::<Vec<_>>(), [(0, "{{질문}}", v2afmt), (2, "{{Extra}}", "{{Answer}}")]);
        assert_eq!(nt.css, ".card { color: red; }");
        assert_eq!(fields(&s, "g1"), ["사과", "apple", "과일", "메모2"], "숨긴 자리에 새 값 (Q2-13), 없어진 필드 값은 그대로");
        assert_eq!(fields(&s, "g2"), ["배", "pear", "", ""]);
        assert_eq!(q::<String>(&s, "SELECT group_concat(template_ord) FROM card WHERE note_id = (SELECT id FROM note WHERE anki_guid = 'g1')"), "0,2");
    }

    #[test]
    fn legacy_notes_and_types_without_origin() {
        let t = tmp("legacy");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let v1 = [note(1, "ga", 7, &["A", "a"], &[(1, 0)]), note(2, "gb", 7, &["B", "b"], &[(1, 0)]), note(3, "gc", 7, &["C", "c"], &[(1, 0)])];
        testpkg::write(&p1, &[basic(false)], &[(1, "D")], &v1, &[]);
        import(&s, &p1);
        // 이 기능보다 먼저 가져온 데이터
        s.db.lock().unwrap().execute_batch("UPDATE note SET origin = NULL; UPDATE note_type SET origin = NULL;").unwrap();
        edit(&s, "ga", 1, "a (내 것)");
        let v2 = [note(1, "ga", 7, &["A", "힌트", "a2"], &[(1, 0)]), note(2, "gb", 7, &["B", "", "b2"], &[(1, 0)]), note(3, "gc", 7, &["C", "", "c"], &[(1, 0)])];
        testpkg::write(&p2, &[basic(true)], &[(1, "D")], &v2, &[]);

        let r = update(&s, &p2, &Choices::default(), false);
        assert_eq!((r.changed, r.conflicts.len(), r.same, r.no_base), (1, 1, 1, 3), "gb 는 고친 흔적이 없어 새 버전, ga 는 충돌, gc 는 그대로 (DU18)");
        assert!(r.conflicts[0].no_base);
        assert_eq!(fields(&s, "ga")[1], "a (내 것)");
        assert_eq!(fields(&s, "gb")[1], "b2");
        let ty = &r.types[0];
        assert!(ty.no_base && ty.new_fields.is_empty() && ty.new_templates.is_empty(), "원본 없는 유형: 새 필드 · 카드 종류를 내가 지운 것과 구별하지 못한다 (NT29)");
        assert_eq!((ty.skipped_fields.clone(), ty.skipped_templates.clone()), (vec!["Hint".to_string()], vec!["Card 2".to_string()]), "넣지 않는 것을 알린다 (DU22)");
        assert_eq!((ty.looks.len(), r.kept_looks, r.new_cards), (1, 1, 0), "다른 모양은 내 모양 (덮지 않는다)");
        assert!(r.issues.iter().any(|i| i.kind == "dropped_field" && i.detail.contains("'Hint'")));
        assert_eq!(
            q::<i64>(&s, "SELECT count(*) FROM note WHERE origin IS NULL") + q::<i64>(&s, "SELECT count(*) FROM note_type WHERE anki_model_id = 7 AND origin IS NULL"),
            0,
            "원본을 채운다"
        );
        // 다음 업데이트부터는 원본으로 판정한다: 같은 파일이면 내 것 · 그대로, 모양도 묻지 않는다
        let r = update(&s, &p2, &Choices::default(), false);
        assert_eq!((r.changed, r.conflicts.len(), r.mine_kept, r.same, r.no_base, r.kept_looks), (0, 0, 1, 2, 0, 0));
    }

    #[test]
    fn tags_and_exam_marks() {
        let t = tmp("tags");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let v1 = [Note { tags: "a b 기출::2023", ..note(1, "g1", 7, &["1", "x"], &[(1, 0)]) }, Note { tags: "기출::2023", ..note(2, "g2", 7, &["2", "y"], &[(1, 0)]) }];
        testpkg::write(&p1, &[basic(false)], &[(1, "D")], &v1, &[]);
        import(&s, &p1);
        {
            let con = s.db.lock().unwrap();
            con.execute("UPDATE note SET tags = 'a b 기출::2023 c' WHERE anki_guid = 'g1'", []).unwrap();
            con.execute("UPDATE note SET exam_hits = 0, exam_names = NULL WHERE anki_guid = 'g2'", []).unwrap(); // 사용자가 기출 표시를 지웠다
        }
        let v2 = [
            Note { tags: "a d 기출::2023 기출::2024 Neo-Anki::출처::x", ..note(1, "g1", 7, &["1", "x"], &[(1, 0)]) },
            Note { tags: "기출::2023 기출::2024", ..note(2, "g2", 7, &["2", "y"], &[(1, 0)]) },
        ];
        testpkg::write(&p2, &[basic(false)], &[(1, "D")], &v2, &[]);
        // B2-p [취소]: 멈추면 아무것도 바뀌지 않는다 (백업은 남는다)
        let before = dump(&s);
        let req = Req { path: &p2, selected: None, choices: &Choices::default(), voices: &[], emit: &|_, _, _| {}, cancel: &|| true };
        assert_eq!(apply(&mut s.db.lock().unwrap(), &s.data_dir, &req).err().as_deref(), Some(STOPPED));
        assert_eq!(dump(&s), before);
        let r = update(&s, &p2, &Choices::default(), false);
        assert_eq!(r.changed, 1);
        assert_eq!(r.merged, 1, "g1: 내가 더한 태그 c 와 만든 사람의 태그 변경을 합친다");
        assert_eq!(q::<String>(&s, "SELECT tags || ' ' || exam_hits FROM note WHERE anki_guid = 'g1'"), "a 기출::2023 c d 기출::2024 2", "출처 태그는 들이지 않는다");
        assert_eq!(q::<String>(&s, "SELECT tags || ' ' || exam_hits FROM note WHERE anki_guid = 'g2'"), "기출::2023 기출::2024 0", "사용자가 고친 기출 표시는 지킨다");
    }

    #[test]
    fn new_cards_of_an_update_follow_the_card_type_deck() {
        // NT76: 업데이트로 이미 있는 노트에 생기는 카드(앞면이 채워진 카드 종류)는 그 카드 종류에 정한 덱, 없으면 노트의 첫 카드 덱
        let t = tmp("tdeck");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let m = || Model { id: 20, name: "Two", fields: vec!["Front", "Back", "Extra"], templates: vec![("Card 1", "{{Front}}", AFMT), ("Card 2", "{{Extra}}", AFMT)], css: "" };
        testpkg::write(&p1, &[m()], &[(1, "D")], &[note(1, "g1", 20, &["Q", "A", ""], &[(1, 0)]), note(2, "g2", 20, &["Q2", "A2", ""], &[(1, 0)])], &[]);
        import(&s, &p1);
        let (ty, deck): (i64, i64) = (q(&s, "SELECT id FROM note_type WHERE name = 'Two'"), q(&s, "SELECT id FROM deck WHERE name = 'D'"));
        let other = {
            let con = s.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('예문', 0, 0)", []).unwrap();
            let id = con.last_insert_rowid();
            notetype::set_template_deck(&con, ty, 1, Some(id)).unwrap();
            id
        };
        testpkg::write(&p2, &[m()], &[(1, "D")], &[note(1, "g1", 20, &["Q", "A", "예문 있음"], &[(1, 0), (1, 1)]), note(2, "g2", 20, &["Q2", "A2", ""], &[(1, 0)])], &[]);
        update(&s, &p2, &Choices::default(), false);
        let rows: Vec<String> = {
            let con = s.db.lock().unwrap();
            con.prepare("SELECT n.anki_guid || c.template_ord || ':' || c.deck_id FROM card c JOIN note n ON n.id = c.note_id ORDER BY 1").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(rows, [format!("g10:{deck}"), format!("g11:{other}"), format!("g20:{deck}")]);
    }

    #[test]
    fn missing_notes_are_tidied_apart_from_the_update() {
        // DU59 (초안): 새 버전에 없는 노트를 한 번에 학습 중지 · 지우기. 원본이 있는 노트만, 업데이트 되돌리기 기록에는 들어가지 않는다,
        // 지우면 백업(-vanish) 뒤 지우고 기록은 통계에 남고 다음 업데이트도 다시 넣지 않는다
        let t = tmp("tidy");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let m = || Model { id: 20, name: "Basic", fields: vec!["Front", "Back"], templates: vec![("Card 1", "{{Front}}", AFMT)], css: "" };
        testpkg::write(&p1, &[m()], &[(1, "D")], &[note(1, "g1", 20, &["Q1", "A1"], &[(1, 0)]), note(2, "g2", 20, &["Q2", "A2"], &[(1, 0)]), note(3, "g3", 20, &["Q3", "A3"], &[(1, 0)])], &[]);
        import(&s, &p1);
        testpkg::write(&p2, &[m()], &[(1, "D")], &[note(1, "g1", 20, &["Q1", "A1 v2"], &[(1, 0)])], &[]);
        let u = update(&s, &p2, &Choices::default(), false);
        assert_eq!(u.missing_notes, 2);
        // 그 사이 g2 를 공부했다. 앱에서 만든 노트의 카드는 넘겨도 정리하지 않는다
        let app_card: i64 = {
            let con = s.db.lock().unwrap();
            con.execute("UPDATE card SET reps = 1, state = 'review' WHERE note_id = (SELECT id FROM note WHERE anki_guid = 'g2')", []).unwrap();
            let deck: i64 = con.query_row("SELECT id FROM deck WHERE name = 'D'", [], |r| r.get(0)).unwrap();
            crate::commands::insert_note(&con, &crate::commands::NewCard { deck_id: deck, kind: "basic".into(), front: "앱".into(), back: "노트".into(), importance: None }, "", None).unwrap();
            con.query_row("SELECT max(id) FROM card", [], |r| r.get(0)).unwrap()
        };
        let mut ids = u.missing_card_ids.clone();
        ids.push(app_card);
        let journal = || q::<i64>(&s, "SELECT count(*) FROM update_journal");
        let j0 = journal();
        let tidy = |action: &str, dry: bool| tidy_missing(&mut s.db.lock().unwrap(), &s.data_dir, &ids, action, dry).unwrap();
        assert_eq!(tidy("suspend", true), TidySummary { notes: 2, cards: 2, studied: 1, suspended: 0, backup: None });
        assert!(tidy_missing(&mut s.db.lock().unwrap(), &s.data_dir, &ids, "keep", true).is_err());
        tidy("suspend", false);
        let states: String = q(&s, "SELECT group_concat(x, ',') FROM (SELECT coalesce(n.anki_guid, 'app') || ':' || c.state AS x FROM card c JOIN note n ON n.id = c.note_id ORDER BY 1)");
        assert_eq!(states, "app:new,g1:new,g2:suspended,g3:suspended");
        assert_eq!(journal(), j0, "되돌리기 기록에 들어가지 않는다");
        assert_eq!(tidy("delete", true).suspended, 2);
        // 지우기: 백업 뒤 노트 · 카드를 지우고 기록은 남는다. 다음 업데이트는 "내가 지운 노트"로 다시 넣지 않는다
        let logs = q::<i64>(&s, "SELECT count(*) FROM review_log");
        let r = tidy("delete", false);
        assert!(r.backup.as_deref().is_some_and(|b| b.ends_with("-vanish.zip") && s.data_dir.join("backups").join(b).exists()), "{r:?}");
        assert_eq!((q::<i64>(&s, "SELECT count(*) FROM note WHERE anki_guid IN ('g2', 'g3')"), q::<i64>(&s, "SELECT count(*) FROM review_log")), (0, logs));
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM note WHERE anki_guid IS NULL"), 1, "앱에서 만든 노트는 그대로");
        assert_eq!(update(&s, &p2, &Choices::default(), true).missing_notes, 0);
        let back = update(&s, &p1, &Choices::default(), true); // 그 노트가 다시 있는 버전이 와도
        assert_eq!((back.gone, back.new_notes), (2, 0), "지운 노트는 다시 넣지 않는다 (DU3)");
    }

    #[test]
    fn field_picks_and_original_values() {
        // DU53 · DU54: 원본 글("v")을 남기고, 충돌 필드마다 고른다. 노트 단위 고르기(DU6)도 그대로
        let t = tmp("pick");
        let s = state(&t.0.join("data"));
        let (p1, p2) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"));
        let m = || Model { id: 20, name: "Three", fields: vec!["Front", "Back", "Extra"], templates: vec![("Card 1", "{{Front}}", AFMT)], css: "" };
        let v1 = [note(1, "g1", 20, &["Q", "A", "X"], &[(1, 0)]), note(2, "g2", 20, &["Q2", "A2", ""], &[(1, 0)]), note(3, "g3", 20, &["Q3", "A3", ""], &[(1, 0)])];
        testpkg::write(&p1, &[m()], &[(1, "D")], &v1, &[]);
        import(&s, &p1);
        let o: NoteOrigin = serde_json::from_str(&q::<String>(&s, "SELECT origin FROM note WHERE anki_guid = 'g1'")).unwrap();
        assert_eq!(o.v, Some(vec![Some("Q".to_string()), Some("A".into()), Some("X".into())]), "처음 가져올 때 넣은 값");
        // 옛 앱(구조체에 v 가 없다)도 읽는다
        #[derive(Deserialize)]
        struct Old {
            f: Vec<String>,
            #[allow(dead_code)]
            t: String,
        }
        let old: Old = serde_json::from_str(&q::<String>(&s, "SELECT origin FROM note WHERE anki_guid = 'g1'")).unwrap();
        assert_eq!(old.f, o.f);
        for (g, v) in [("g1", ["Q 내 것", "A 내 것"]), ("g2", ["Q2 내 것", "A2"]), ("g3", ["Q3", "A3 내 것"])] {
            edit(&s, g, 0, v[0]);
            edit(&s, g, 1, v[1]);
        }
        // 옛 앱이 g3 의 원본을 다시 썼다 (v 가 없다): 원본 글이 없어요
        s.db.lock().unwrap().execute("UPDATE note SET origin = json_remove(origin, '$.v') WHERE anki_guid = 'g3'", []).unwrap();
        let v2 = [note(1, "g1", 20, &["Q 새", "A 새", "X 새"], &[(1, 0)]), note(2, "g2", 20, &["Q2 새", "A2", ""], &[(1, 0)]), note(3, "g3", 20, &["Q3", "A3 새", ""], &[(1, 0)])];
        testpkg::write(&p2, &[m()], &[(1, "D")], &v2, &[]);
        let r = update(&s, &p2, &Choices::default(), true);
        let c1 = r.conflicts.iter().find(|c| c.note_id == id_of(&s, "g1")).unwrap();
        assert_eq!(c1.fields.iter().map(|f| (f.slot, f.base.as_deref(), f.take_new)).collect::<Vec<_>>(), [(0, Some("Q"), false), (1, Some("A"), false)]);
        let c3 = r.conflicts.iter().find(|c| c.note_id == id_of(&s, "g3")).unwrap();
        assert_eq!((c3.fields[0].base.as_deref(), c3.no_base), (None, false), "해시는 있어 판정은 되지만 원본 글은 없다");
        // g1 은 Back 만 새 버전, g2 는 노트 단위로 새 버전
        let choices = Choices { take_new: vec![id_of(&s, "g2")], take_new_fields: vec![(id_of(&s, "g1"), 1)], ..Default::default() };
        let r = update(&s, &p2, &choices, true);
        let c1 = r.conflicts.iter().find(|c| c.note_id == id_of(&s, "g1")).unwrap();
        assert_eq!((c1.fields[0].take_new, c1.fields[1].take_new, c1.take_new), (false, true, true));
        let applied = update(&s, &p2, &choices, false);
        // 결과 화면도 고른 것 그대로 센다 (요약 = 결과: 새 버전 g2 · 일부 g1 · 내 것 g3)
        let picks = |r: &UpdateSummary| r.conflicts.iter().map(|c| (c.note_id, c.fields.iter().map(|f| f.take_new).collect::<Vec<_>>())).collect::<Vec<_>>();
        assert_eq!(picks(&applied), picks(&r));
        assert_eq!(picks(&applied).iter().map(|(_, f)| (f.iter().all(|x| *x), f.iter().any(|x| *x))).collect::<Vec<_>>(), [(false, true), (true, true), (false, false)]);
        assert_eq!(fields(&s, "g1"), ["Q 내 것", "A 새", "X 새"], "고른 필드만 새 버전, 충돌 없는 Extra 는 합친다");
        assert_eq!(fields(&s, "g2"), ["Q2 새", "A2", ""]);
        assert_eq!(fields(&s, "g3")[1], "A3 내 것");
        // 원본 = 새 버전 값 (글도), 옛 앱이 지웠던 g3 도 다시 채운다
        for g in ["g1", "g3"] {
            let o: NoteOrigin = serde_json::from_str(&q::<String>(&s, &format!("SELECT origin FROM note WHERE anki_guid = '{g}'"))).unwrap();
            let want: Vec<&str> = if g == "g1" { vec!["Q 새", "A 새", "X 새"] } else { vec!["Q3", "A3 새", ""] };
            assert_eq!((0..3).map(|i| o.value(i)).collect::<Vec<_>>(), want.into_iter().map(Some).collect::<Vec<_>>());
        }
    }

    #[test]
    fn update_undo_reverts_and_keeps_what_was_studied_or_edited() {
        // DU57 · DU58 (사용자 결정 2026-10-19 ①): 이 업데이트 되돌리기. 업데이트가 바꾼 노트 · 모양은 되돌리고, 그 뒤에 고친 노트와 공부한 새 카드는 그대로,
        // 공부하지 않은 새 노트 · 카드는 지우지 않고 보관 (학습 중지 + 태그)
        let t = tmp("undo");
        let s = state(&t.0.join("data"));
        let (p1, p2, p3) = (t.0.join("v1.apkg"), t.0.join("v2.apkg"), t.0.join("v3.apkg"));
        let b1 = |id, g, f: &str, b: &str| note(id, g, 7, &[f, b], &[(1, 0)]);
        let b2 = |id, g, f: &str, b: &str| note(id, g, 7, &[f, "", b], &[(1, 0), (1, 1)]);
        testpkg::write(&p1, &[basic(false)], &[(1, "D")], &[b1(1, "g1", "Q1", "A1"), b1(2, "g2", "Q2", "A2"), b1(3, "g3", "Q3", "A3")], &[]);
        import(&s, &p1);
        let v2 = [b2(1, "g1", "Q1", "A1 v2"), b2(2, "g2", "Q2", "A2 v2"), b2(3, "g3", "Q3", "A3"), b2(4, "g4", "Q4", "A4"), b2(5, "g5", "Q5", "A5")];
        testpkg::write(&p2, &[basic(true)], &[(1, "D")], &v2, &[]);
        let r = update(&s, &p2, &Choices::default(), false);
        let batch = r.batch.expect("적용한 업데이트");
        assert_eq!((r.changed, r.new_notes), (2, 2));
        // 그 뒤: g2 를 내가 고치고, 새 노트 g4 의 Card 1 과 이미 있던 g3 의 새 Card 2 를 공부했다
        edit(&s, "g2", 1, "A2 (내 수정)");
        {
            let con = s.db.lock().unwrap();
            for (g, ord) in [("g4", 0), ("g3", 1)] {
                con.execute(
                    "UPDATE card SET state = 'review', reps = 1, due_at = 99, stability = 3.0 WHERE template_ord = ?2 AND note_id = (SELECT id FROM note WHERE anki_guid = ?1)",
                    params![g, ord],
                )
                .unwrap();
                con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) SELECT id, 5, 3 FROM card WHERE state = 'review' AND template_ord = ?2 AND note_id = (SELECT id FROM note WHERE anki_guid = ?1)", params![g, ord]).unwrap();
            }
        }
        let studied = || q::<String>(&s, "SELECT group_concat(id || state || reps || due_at, ',') FROM card WHERE state = 'review'");
        let studied_before = studied();
        let u = |dry: bool| undo(&mut s.db.lock().unwrap(), batch, dry).unwrap();
        let untouched = dump(&s);
        let dry = u(true);
        assert_eq!(dump(&s), untouched, "확인 창의 수는 쓰지 않는다");
        assert_eq!((dry.changed, dry.reverted_notes, dry.edited_notes, dry.reverted_types, dry.blocked.as_deref()), (2, 1, 1, 1, None), "g1 이 돌아가고 g2 는 내가 고쳐 그대로");
        assert_eq!((dry.kept_cards, dry.archived_notes, dry.archived_cards), (2, 1, 5), "공부한 g4 Card 1 · g3 Card 2 를 남기고, g5 두 장 · g4 Card 2 · g1 · g2 의 새 Card 2 를 보관");
        assert_eq!(dry.edited_card_ids.len(), 2);
        let r = u(false);
        assert_eq!((r.reverted_notes, r.archived_cards), (1, 5));
        assert_eq!(r.archived_card_ids.len(), 5, "[보관한 카드 보기]는 이미 있던 노트에 생긴 새 카드(태그 없음, 학습 중지)도 보여 준다");
        assert_eq!(fields(&s, "g1"), ["Q1", "A1"], "업데이트 전 내용");
        assert_eq!(fields(&s, "g2")[1], "A2 (내 수정)", "그 뒤에 고친 노트는 그대로");
        assert_eq!(studied(), studied_before, "공부한 카드의 일정 · 상태 그대로");
        assert_eq!(q::<i64>(&s, "SELECT count(*) FROM review_log"), 2, "기록은 지우지 않는다");
        assert_eq!(
            q::<String>(&s, "SELECT group_concat(x, ',') FROM (SELECT n.anki_guid || c.template_ord || c.state AS x FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid IN ('g4', 'g5') ORDER BY 1)"),
            "g40review,g41suspended,g50suspended,g51suspended"
        );
        assert_eq!(q::<String>(&s, "SELECT group_concat(anki_guid) FROM note WHERE tags LIKE '%되돌린_업데이트%'"), "g5", "다 보관한 새 노트만 태그");
        // 모양은 되돌리고 더한 필드 · 카드 종류는 남긴다 (공부한 카드가 쓴다)
        let ntid: i64 = q(&s, "SELECT id FROM note_type WHERE anki_model_id = 7");
        let nt = notetype::load(&s.db.lock().unwrap(), ntid).unwrap();
        assert_eq!((nt.templates[0].qfmt.as_str(), nt.templates.len(), nt.slots.len()), ("{{Front}}", 2, 3));
        assert_eq!(nt.css, ".card { color: black; }");
        // 원본도 업데이트 전으로: 같은 파일을 다시 적용하면 다시 바뀐다
        let o: NoteOrigin = serde_json::from_str(&q::<String>(&s, "SELECT origin FROM note WHERE anki_guid = 'g1'")).unwrap();
        assert_eq!(o.f, [h("Q1"), h("A1")]);
        assert_eq!(u(true).blocked.as_deref(), Some(UNDONE));
        let again = update(&s, &p2, &Choices::default(), false);
        assert_eq!(fields(&s, "g1"), ["Q1", "A1 v2", ""]);
        assert_eq!(again.changed, 1);
        // 그 뒤에 같은 노트를 다시 업데이트했으면 앞의 것은 되돌리지 않는다 (마지막 것만)
        testpkg::write(&p3, &[basic(true)], &[(1, "D")], &[b2(1, "g1", "Q1", "A1 v3")], &[]);
        let v3 = update(&s, &p3, &Choices::default(), false);
        assert_eq!(undo(&mut s.db.lock().unwrap(), again.batch.unwrap(), true).unwrap().blocked.as_deref(), Some(LATER));
        assert!(undo(&mut s.db.lock().unwrap(), v3.batch.unwrap(), true).unwrap().blocked.is_none());
        // 기록이 없는 (이 기능 전) 업데이트
        let old: i64 = q(&s, "SELECT min(id) FROM import_batch");
        assert_eq!(undo(&mut s.db.lock().unwrap(), old, true).unwrap().blocked.as_deref(), Some(NO_JOURNAL));
    }

    #[test]
    fn duplicate_guid_notes_are_reported() {
        // DU-06: 두 기기가 받지 않은 채 같은 파일을 적용하면 같은 guid 노트가 두 벌. 다음 업데이트가 알린다
        let t = tmp("dup");
        let s = state(&t.0.join("data"));
        let p = t.0.join("v1.apkg");
        testpkg::write(&p, &[basic(false)], &[(1, "D")], &[note(1, "g1", 7, &["Q", "A"], &[(1, 0)]), note(2, "g2", 7, &["Q2", "A2"], &[(1, 0)])], &[]);
        import(&s, &p);
        {
            let con = s.db.lock().unwrap();
            con.execute("INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, anki_guid, origin) SELECT note_type_id, fields, tags, created_at + 1, updated_at, anki_guid, origin FROM note WHERE anki_guid = 'g1'", []).unwrap();
            con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) SELECT last_insert_rowid(), deck_id, 0, 0, 0 FROM card LIMIT 1", []).unwrap();
        }
        let r = update(&s, &p, &Choices::default(), true);
        assert_eq!((r.duplicate_notes, r.duplicate_card_ids.len(), r.same), (1, 2, 2));
    }

    #[test]
    fn first_import_origin_matches_update_insert_and_gone_trigger() {
        let t = tmp("same");
        let p = t.0.join("v1.apkg");
        let notes = [note(1, "g1", 7, &["<img src=\"a.png\">", "b"], &[(1, 0)]), note(2, "g2", 8, &["{{c1::x}}", ""], &[(1, 0)])];
        testpkg::write(&p, &[basic(false), cloze()], &[(1, "D")], &notes, &[("a.png", b"A")]);
        let (a, b) = (state(&t.0.join("a")), state(&t.0.join("b")));
        for d in ["a", "b"] {
            std::fs::write(t.0.join(d).join("media/a.png"), b"other").unwrap();
        }
        import(&a, &p);
        let r = update(&b, &p, &Choices::default(), false);
        assert_eq!((r.new_notes, r.new_note_cards, r.media_new, r.media_renamed), (2, 2, 1, 1), "빈 DB 의 업데이트 = 가져오기");
        let origins = |s: &AppState| -> Vec<String> {
            let con = s.db.lock().unwrap();
            let mut st = con.prepare("SELECT origin FROM (SELECT origin, anki_guid AS k FROM note UNION ALL SELECT origin, anki_model_id FROM note_type WHERE anki_model_id IS NOT NULL) ORDER BY k").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(origins(&a), origins(&b), "같은 기준 (미디어 이름 바꾸기 뒤의 값)");
        assert_eq!(fields(&a, "g1"), fields(&b, "g1"));
        // 지운 노트 기억: 원본이 있는 노트만. 열 · 표 · 트리거는 다시 열어도 하나 (migrate 멱등)
        let con = a.db.lock().unwrap();
        con.execute("DELETE FROM note WHERE anki_guid = 'g1'", []).unwrap();
        con.execute("UPDATE note SET origin = NULL WHERE anki_guid = 'g2'", []).unwrap();
        con.execute("DELETE FROM note WHERE anki_guid = 'g2'", []).unwrap();
        assert_eq!(con.query_row("SELECT group_concat(guid) FROM origin_gone", [], |r| r.get::<_, String>(0)).unwrap(), "g1");
        drop(con);
        let again = db::open(&t.0.join("a/neoanki.sqlite")).unwrap();
        let n = |sql: &str| again.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(n("SELECT count(*) FROM pragma_table_info('note') WHERE name = 'origin'") + n("SELECT count(*) FROM pragma_table_info('note_type') WHERE name = 'origin'"), 2);
        assert_eq!(n("SELECT count(*) FROM sqlite_master WHERE name IN ('origin_gone', 'note_origin_gone')"), 2);
        assert_eq!(n("SELECT count(*) FROM origin_gone"), 1);
    }

    /// 화면 확인(B2-u0 ~ B3-u)용 데이터: NEOANKI_DU_OUT/data 에 v1 을 가져와 몇 장 고치고 지운 DB, NEOANKI_DU_OUT/v2.apkg.
    /// NEOANKI_DU_OUT=<폴더> cargo test --lib deck_update_sample -- --ignored
    #[test]
    #[ignore]
    fn deck_update_sample() {
        let out = PathBuf::from(std::env::var("NEOANKI_DU_OUT").expect("NEOANKI_DU_OUT"));
        let s = state(&out.join("data"));
        let decks = [(1, "정보처리기사 실기"), (2, "정보처리기사 실기::데이터베이스"), (3, "정보처리기사 실기::네트워크")];
        let afmt = "{{FrontSide}}<hr id=answer>{{뒷면}}";
        let basic = |v2: bool| Model {
            id: 100,
            name: "기본 (정보처리)",
            fields: if v2 { vec!["앞면", "뒷면", "보충 설명"] } else { vec!["앞면", "뒷면"] },
            templates: if v2 {
                vec![
                    ("카드 1", "<span class=\"tag\">{{보충 설명}}</span><div>{{앞면}}</div>", afmt),
                    ("용어 → 정의", "{{뒷면}}", "{{FrontSide}}<hr id=answer>{{앞면}}"),
                ]
            } else {
                vec![("카드 1", "{{앞면}}", afmt)]
            },
            css: if v2 { ".card { font-size: 20px; text-align: left; } .tag { font-size: 12px; padding: 2px 8px; border-radius: 99px; background: #16161d; color: #fff; }" } else { ".card { font-size: 24px; text-align: center; }" },
        };
        let cloze = || Model { id: 101, name: "빈칸 (정보처리)", fields: vec!["본문", "추가 설명"], templates: vec![("빈칸", "{{cloze:본문}}", "{{cloze:본문}}<br>{{추가 설명}}")], css: "" };
        let acid1 = "원자성(Atomicity): 모두 반영되거나 전혀 반영되지 않음<br>일관성(Consistency): 실행 전후 DB가 일관된 상태<br>독립성(Isolation): 동시에 실행돼도 서로 간섭하지 않음<br>영속성(Durability): 완료된 결과는 영구히 반영";
        let acid2 = "원자성(Atomicity): All or Nothing<br>일관성(Consistency): 실행 전후 DB가 일관된 상태<br>격리성(Isolation): 실행 중인 트랜잭션에 다른 트랜잭션이 끼어들 수 없음<br>영속성(Durability): 완료된 결과는 영구히 반영";
        let svg = |fill: &str, text: &str| format!("<svg xmlns='http://www.w3.org/2000/svg' width='240' height='90'><rect width='240' height='90' rx='12' fill='{fill}'/><text x='20' y='55' font-size='22' fill='white'>{text}</text></svg>");
        let b = |id, g, f: &str, k: &str, d| note(id, g, 100, &[f, k], &[(d, 0)]);
        let b2 = |id, g, f: &str, k: &str, x: &str, d| note(id, g, 100, &[f, k, x], &[(d, 0)]);
        let cz = |id, g, t: &str, ords: &[i64]| Note { cards: ords.iter().map(|o| (2, *o)).collect(), ..note(id, g, 101, &[t, ""], &[]) };
        let v1 = vec![
            b(1, "acid", "트랜잭션의 특성 4가지(ACID)를 쓰시오.", acid1, 2),
            b(2, "nf", "정규화 단계: 1NF → 2NF 조건은?", "부분 함수 종속 제거", 2),
            b(3, "osi", "OSI 7계층 그림에서 3계층의 이름은?", "<img src=\"osi.svg\"><br>네트워크 계층", 3),
            cz(4, "http", "HTTP는 {{c1::80}}번 포트를 써요", &[0]),
            b(5, "tcp", "TCP와 UDP의 차이는?", "TCP는 연결 지향", 3),
            b(6, "dns", "DNS의 역할은?", "도메인 이름을 IP로", 3),
            b(7, "pk", "기본키의 조건 두 가지는?", "유일성", 2),
            b(8, "fk", "외래키란?", "다른 릴레이션의 기본키를 참조", 2),
            b(9, "view", "뷰(View)의 장점은?", "논리적 독립성", 2),
            cz(10, "sql", "{{c1::SELECT}} 열 {{c2::FROM}} 테이블", &[0, 1]),
            b(11, "old", "Gopher 프로토콜의 포트는?", "70", 3),
            b(12, "dup", "중복으로 만든 카드", "지울 것", 3),
            b(13, "arp", "ARP의 역할은?", "IP 주소로 MAC 주소 찾기", 3),
        ];
        testpkg::write(&out.join("v1.apkg"), &[basic(false), cloze()], &decks, &v1, &[("osi.svg", svg("#4433ff", "OSI v1").as_bytes())]);
        import(&s, &out.join("v1.apkg"));
        edit(&s, "acid", 1, &acid1.replace("간섭하지 않음", "간섭하지 않음 ← 시험에 자주 나옴"));
        edit(&s, "nf", 0, "정규화: 1NF에서 2NF로 가려면?");
        edit(&s, "nf", 1, "부분 함수 종속을 없앤다");
        edit(&s, "osi", 1, "<img src=\"osi.svg\"><br>네트워크 계층 (라우터)");
        edit(&s, "http", 0, "HTTP는 {{c1::80}}번, HTTPS는 {{c2::443}}번 포트를 써요");
        edit(&s, "fk", 1, "다른 릴레이션의 기본키를 참조 (참조 무결성)");
        edit(&s, "view", 1, "논리적 독립성 · 보안 (내 메모)");
        {
            let con = s.db.lock().unwrap();
            let nt: i64 = con.query_row("SELECT id FROM note_type WHERE anki_model_id = 100", [], |r| r.get(0)).unwrap();
            notetype::save_template(&con, nt, 0, None, "{{앞면}}<div class=\"hint\">힌트: 원 · 일 · 독 · 영</div>", afmt).unwrap();
            con.execute("DELETE FROM card WHERE note_id = (SELECT id FROM note WHERE anki_guid = 'dup')", []).unwrap();
            con.execute("DELETE FROM note WHERE anki_guid = 'dup'", []).unwrap();
        }
        let v2 = vec![
            b2(1, "acid", "트랜잭션의 특성 4가지(ACID)를 쓰시오.", acid2, "데이터베이스", 2),
            b2(2, "nf", "정규화 단계: 1NF에서 2NF로 가는 조건은?", "부분 함수 종속을 제거한다", "데이터베이스", 2),
            b2(3, "osi", "OSI 7계층 그림에서 3계층의 이름은?", "<img src=\"osi.svg\"><br>네트워크 계층 (Network)", "네트워크", 3),
            cz(4, "http", "HTTP는 {{c1::80}}번, HTTPS는 {{c2::443}}번", &[0, 1]),
            b2(5, "tcp", "TCP와 UDP의 차이는?", "TCP는 연결 지향, UDP는 비연결", "네트워크", 3),
            b2(6, "dns", "DNS의 역할은?", "도메인 이름을 IP 주소로 바꾼다", "네트워크", 3),
            b2(7, "pk", "기본키의 조건 두 가지는?", "유일성 · 최소성", "데이터베이스", 2),
            b2(8, "fk", "외래키(Foreign Key)란?", "다른 릴레이션의 기본키를 참조", "데이터베이스", 2),
            b2(9, "view", "뷰(View)의 장점은?", "논리적 독립성", "데이터베이스", 2),
            cz(10, "sql", "{{c1::SELECT}} 열 {{c3::FROM}} 테이블", &[0, 2]),
            b2(12, "dup", "중복으로 만든 카드", "지울 것", "", 3),
            b2(13, "arp", "ARP의 역할은?", "IP 주소로 MAC 주소 찾기", "네트워크", 3),
            Note { cards: vec![(3, 0), (3, 1)], ..b2(14, "rarp", "RARP의 역할은?", "MAC 주소로 IP 주소 찾기", "네트워크", 3) },
            Note { cards: vec![(2, 0), (2, 1)], ..b2(15, "idx", "인덱스를 쓰는 까닭은?", "검색 속도를 높인다", "데이터베이스", 2) },
        ];
        testpkg::write(&out.join("v2.apkg"), &[basic(true), cloze()], &decks, &v2, &[("osi.svg", svg("#1f7a4a", "OSI v2").as_bytes())]);
        // B2-p 진행을 볼 만큼 큰 다음 버전: v2 + 새 노트 6천 개
        let mut big = v2;
        for i in 0..6000 {
            let guid: &'static str = Box::leak(format!("bulk{i}").into_boxed_str());
            big.push(b2(10_000 + i, guid, &format!("대량 노트 {i}"), "답", "", 2));
        }
        testpkg::write(&out.join("big.apkg"), &[basic(true), cloze()], &decks, &big, &[("osi.svg", svg("#1f7a4a", "OSI v2").as_bytes())]);
    }
}
