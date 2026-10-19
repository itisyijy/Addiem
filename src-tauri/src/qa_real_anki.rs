//! 실제 Anki(pylib 26.9.3) 검증 하네스의 Neo-Anki 쪽. 혼자 돌리지 않는다: `tools/qa-real-anki/real_anki.py all` 이
//! pylib 로 만든 패키지를 NA_QR_WORK 에 두고 phase1 · phase2 를 부른다 (#[ignore], README 참고).
//! 앱 함수를 그대로 쓴다: 가져오기 `run_import_anki`, 덱 업데이트 `deck_update::run · apply`, 내 편집 `save_note_fields` ·
//! `notetype::save_css`, 평가 `answer_for_qa`(= answer_at), 내보내기 `export_for_qa`(= export).
//! 판정은 <work>/checks_rust1.json · checks_rust2.json, Anki 와 비교할 Neo 상태는 neo_*.json (비교는 real_anki.py).
//! 렌더는 `neo_render` 한 곳 (C 의 특수 필드 작업으로 render_fields 모양이 바뀌면 여기만 고친다).

use crate::commands::{self, AppState};
use crate::db::{self, now_ms, study_day};
use crate::import::{deck_update, run_import_anki};
use crate::notetype;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const ROOT: &str = "QA원본";
const A1: &str = "1 가져오기";
const A2: &str = "2 덱 업데이트";
const A3: &str = "3 내보내기 → Anki";
const A4: &str = "4 왕복";

fn work() -> PathBuf {
    PathBuf::from(std::env::var("NA_QR_WORK").expect("NA_QR_WORK 가 없어요. real_anki.py all 로 돌리세요"))
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn write(p: &Path, v: &Value) {
    std::fs::write(p, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

#[derive(Default)]
struct Checks(Vec<Value>);

impl Checks {
    fn add(&mut self, id: &str, area: &str, name: &str, ok: bool, ev: impl std::fmt::Display) {
        println!("[{}] {id} {name}: {ev}", if ok { "PASS" } else { "FAIL" });
        self.0.push(json!({ "id": id, "area": area, "name": name, "status": if ok { "PASS" } else { "FAIL" }, "evidence": ev.to_string() }));
    }
    /// 이 브랜치에 아직 없는 기능 (C 작업). 통과하면 PASS, 아니면 '다음 회차'
    fn next(&mut self, id: &str, area: &str, name: &str, ok: bool, ev: impl std::fmt::Display) {
        self.add(id, area, name, ok, ev);
        if !ok {
            self.0.last_mut().unwrap()["status"] = json!("NEXT");
        }
    }
    fn save(&self, phase: &str) {
        write(&work().join(format!("checks_{phase}.json")), &Value::Array(self.0.clone()));
    }
}

fn state(dir: &Path, fresh: bool) -> AppState {
    if fresh {
        let _ = std::fs::remove_dir_all(dir);
    }
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

/// 다른 폴더로 DB(VACUUM INTO) · 미디어를 복사해 연다 (왕복 두 방식을 같은 시작점에서)
fn copy_state(src: &AppState, dir: &Path) -> AppState {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("media")).unwrap();
    src.db.lock().unwrap().execute("VACUUM INTO ?1", [dir.join("neoanki.sqlite").to_str().unwrap()]).unwrap();
    for e in std::fs::read_dir(src.data_dir.join("media")).unwrap().flatten() {
        std::fs::copy(e.path(), dir.join("media").join(e.file_name())).unwrap();
    }
    state(dir, false)
}

fn q<T: rusqlite::types::FromSql>(s: &AppState, sql: &str) -> T {
    s.db.lock().unwrap().query_row(sql, [], |r| r.get(0)).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn cell(v: rusqlite::types::ValueRef) -> String {
    use rusqlite::types::ValueRef::*;
    match v {
        Null => "NULL".into(),
        Integer(i) => i.to_string(),
        Real(f) => f.to_string(),
        Text(t) => String::from_utf8_lossy(t).into_owned(),
        Blob(b) => format!("<blob {}>", b.len()),
    }
}

fn rows(s: &AppState, sql: &str) -> Vec<String> {
    let con = s.db.lock().unwrap();
    let mut st = con.prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| Ok((0..n).map(|i| cell(r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))).unwrap().map(Result::unwrap).collect()
}

/// 이미 있는 카드의 일정 · 덱 · 중요도 (덱 업데이트 · 다시 가져오기가 건드리면 안 되는 것). id -> 값
fn card_rows(s: &AppState) -> BTreeMap<i64, String> {
    let con = s.db.lock().unwrap();
    let mut st = con
        .prepare(
            "SELECT id, note_id, deck_id, template_ord, state, due_at, interval_days, stability, difficulty, srs_step, reps, lapses,
                    last_reviewed_at, buried_until, importance, importance_source, anki_card_id FROM card",
        )
        .unwrap();
    st.query_map([], |r| Ok((r.get::<_, i64>(0)?, (1..17).map(|i| cell(r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn changed_cards(before: &BTreeMap<i64, String>, after: &BTreeMap<i64, String>) -> Vec<String> {
    before.iter().filter(|(id, v)| after.get(id) != Some(v)).map(|(id, v)| format!("card {id}: {v} -> {:?}", after.get(id))).collect()
}

fn guid_of(roles: &Value, role: &str) -> String {
    roles[role].as_str().unwrap_or_else(|| panic!("role {role}")).to_string()
}

/// 노트 값을 보이는 필드 이름으로 ([[이름, 값]])
fn named_fields(con: &Connection, note: i64) -> Vec<(String, String)> {
    let (ntid, fields): (i64, String) = con.query_row("SELECT note_type_id, fields FROM note WHERE id = ?1", [note], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let values: Vec<String> = serde_json::from_str(&fields).unwrap();
    notetype::load(con, ntid).unwrap().visible().into_iter().map(|(sl, n)| (n, values.get(sl).cloned().unwrap_or_default())).collect()
}

fn field(s: &AppState, guid: &str, name: &str) -> String {
    let con = s.db.lock().unwrap();
    named_fields(&con, note_id_in(&con, guid)).into_iter().find(|(n, _)| n == name).map(|x| x.1).unwrap_or_else(|| format!("<필드 {name} 없음>"))
}

fn note_id_in(con: &Connection, guid: &str) -> i64 {
    con.query_row("SELECT id FROM note WHERE anki_guid = ?1 ORDER BY id LIMIT 1", [guid], |r| r.get(0)).unwrap_or_else(|e| panic!("{guid}: {e}"))
}

/// 편집기 [저장] 과 같은 길 (commands::update_note → save_note_fields)
fn edit(s: &AppState, guid: &str, name: &str, value: &str) {
    let mut con = s.db.lock().unwrap();
    let id = note_id_in(&con, guid);
    let (ntid, fields): (i64, String) = con.query_row("SELECT note_type_id, fields FROM note WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let mut values: Vec<String> = serde_json::from_str(&fields).unwrap();
    let slot = notetype::load(&con, ntid).unwrap().visible().into_iter().find(|(_, n)| n == name).unwrap().0;
    values.resize(values.len().max(slot + 1), String::new());
    values[slot] = value.into();
    let tx = con.transaction().unwrap();
    commands::save_note_fields(&tx, id, &values).unwrap();
    tx.commit().unwrap();
}

/// 카드 한 장 그리기. 앱의 card_view 와 같은 템플릿 고르기(TEMPLATE_OF_CARD) · render_fields
#[allow(clippy::too_many_arguments)]
fn neo_render(names: &HashMap<i64, (String, Vec<String>)>, ntid: i64, fields: &str, q: Option<&str>, a: Option<&str>, ord: u32, special: &[(String, String)]) -> (String, String) {
    let r = commands::render_fields_with(names, ntid, fields, q, a, ord, special);
    (r.front, r.back)
}

/// real_anki.py snapshot() 과 같은 꼴. 미디어는 sha1 대신 파일 경로 (파이썬이 해시를 센다)
fn snapshot(s: &AppState, root: &str) -> Value {
    let con = s.db.lock().unwrap();
    let rid: i64 = con.query_row("SELECT id FROM deck WHERE name = ?1 AND parent_id IS NULL", [root], |r| r.get(0)).unwrap();
    let names = commands::note_field_names(&con).unwrap();
    let today = study_day(now_ms());
    let sql = format!(
        "WITH RECURSIVE dt(id, path) AS (SELECT id, name FROM deck WHERE id = ?1
                                         UNION ALL SELECT d.id, dt.path || '::' || d.name FROM deck d JOIN dt ON d.parent_id = dt.id)
         SELECT c.id, c.template_ord, c.state, c.due_at, c.interval_days, c.stability, c.difficulty, c.reps, c.lapses, dt.path,
                n.id, n.fields, n.tags, n.anki_guid, n.note_type_id, t.name, t.qfmt, t.afmt,
                (SELECT count(*) FROM review_log l WHERE l.card_id = c.id AND l.kind IS NULL)
         FROM card c JOIN dt ON dt.id = c.deck_id JOIN note n ON n.id = c.note_id
         LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {}
         ORDER BY c.id",
        notetype::TEMPLATE_OF_CARD
    );
    let mut st = con.prepare(&sql).unwrap();
    let mut out_rows = st.query([rid]).unwrap();
    let (mut notes, mut cards, mut types, mut renders) = (serde_json::Map::new(), vec![], serde_json::Map::new(), serde_json::Map::new());
    let (mut media, mut decks, mut total) = (BTreeSet::new(), BTreeSet::new(), 0i64);
    let mut nts: HashMap<i64, notetype::Nt> = HashMap::new();
    while let Some(r) = out_rows.next().unwrap() {
        let g = |i: usize| r.get_ref(i).unwrap();
        let (cid, ord, st_, due): (i64, i64, String, Option<i64>) = (r.get(0).unwrap(), r.get(1).unwrap(), r.get(2).unwrap(), r.get(3).unwrap());
        let ivl: f64 = r.get(4).unwrap();
        let (stab, diff): (Option<f64>, Option<f64>) = (r.get(5).unwrap(), r.get(6).unwrap());
        let path: String = r.get(9).unwrap();
        let (nid, fields, tags, guid, ntid): (i64, String, String, Option<String>, i64) = (r.get(10).unwrap(), r.get(11).unwrap(), r.get(12).unwrap(), r.get(13).unwrap(), r.get(14).unwrap());
        let (tname, qf, af): (Option<String>, Option<String>, Option<String>) = (r.get(15).unwrap(), r.get(16).unwrap(), r.get(17).unwrap());
        let revs: i64 = r.get(18).unwrap();
        let nt = nts.entry(ntid).or_insert_with(|| notetype::load(&con, ntid).unwrap());
        let cloze = nt.cloze();
        let guid = guid.unwrap_or_else(|| format!("neo-note-{nid}"));
        let key = format!("{guid}|{}|{}", tname.clone().unwrap_or_default(), if cloze { ord.to_string() } else { String::new() });
        let vals: Vec<String> = serde_json::from_str(&fields).unwrap();
        notes.insert(
            guid.clone(),
            json!({
                "type": nt.name,
                "fields": nt.visible().into_iter().map(|(sl, n)| json!([n, vals.get(sl).cloned().unwrap_or_default()])).collect::<Vec<_>>(),
                "tags": tags.split_whitespace().collect::<BTreeSet<_>>(),
            }),
        );
        // 유형은 Anki 모델 id 로 (내보내기 model() 과 같은 id: 가져온 유형은 그 id, 앱 유형은 만든 시각 + 행 번호). 이름은 겹칠 수 있다
        let mid: i64 = con.query_row("SELECT coalesce(anki_model_id, created_at + id) FROM note_type WHERE id = ?1", [ntid], |r| r.get(0)).unwrap();
        if !types.contains_key(&mid.to_string()) {
            types.insert(
                mid.to_string(),
                json!({
                    "name": nt.name,
                    "fields": nt.visible().into_iter().map(|x| x.1).collect::<Vec<_>>(),
                    "templates": nt.templates.iter().map(|t| json!([t.name, t.qfmt, t.afmt])).collect::<Vec<_>>(),
                    "css": nt.css,
                    "cloze": cloze,
                }),
            );
        }
        for v in &vals {
            crate::export::media_refs(v, &mut media);
        }
        decks.insert(path.clone());
        total += revs;
        let review_like = st_ == "review" || (st_ == "suspended" && ivl >= 1.0);
        cards.push(json!({
            "key": key, "guid": guid, "ord": ord, "deck": path, "state": st_,
            "due_days": due.filter(|_| review_like).map(|d| study_day(d) - today),
            "due_secs": due.filter(|_| st_ == "learning" || st_ == "relearning").map(|d| d / 1000),
            "ivl": ivl, "reps": g(7).as_i64().unwrap(), "lapses": g(8).as_i64().unwrap(),
            "s": stab, "d": diff, "revlog": revs, "id": cid,
        }));
        // 학습 화면(card_view)과 같은 특수 필드 (C: render_fields_with)
        let deck_path: Vec<String> = path.split("::").map(String::from).collect();
        let special = crate::render::special_fields(&tags, &deck_path, &nt.name, tname.as_deref().unwrap_or_default());
        let (front, back) = neo_render(&names, ntid, &fields, qf.as_deref(), af.as_deref(), ord as u32, &special);
        renders.insert(key, json!({ "q": front, "a": back }));
    }
    let media: serde_json::Map<String, Value> = media
        .into_iter()
        .map(|m| {
            let rel: String = con.query_row("SELECT rel_path FROM media WHERE filename = ?1", [&m], |r| r.get(0)).unwrap_or_else(|_| format!("media/{m}"));
            (m, json!(s.data_dir.join(rel).to_string_lossy()))
        })
        .collect();
    json!({ "root": root, "notes": notes, "cards": cards, "types": types, "renders": renders, "media": media,
            "decks": decks, "review_total": total, "today": today })
}

/// (노트, 카드, 복습 기록, 같은 guid 두 벌, 같은 카드 · 같은 시각 기록 두 벌)
fn counts(s: &AppState) -> (i64, i64, i64, i64, i64) {
    (
        q(s, "SELECT count(*) FROM note"),
        q(s, "SELECT count(*) FROM card"),
        q(s, "SELECT count(*) FROM review_log"),
        q(s, "SELECT count(*) FROM (SELECT anki_guid FROM note WHERE anki_guid IS NOT NULL GROUP BY anki_guid HAVING count(*) > 1)"),
        q(s, "SELECT count(*) FROM (SELECT 1 FROM review_log WHERE card_id IS NOT NULL GROUP BY card_id, reviewed_at HAVING count(*) > 1)"),
    )
}

/// 노트가 하나도 없는 가져온 노트 유형 (다시 가져오기가 남긴 빈 유형)
fn empty_types(s: &AppState) -> Vec<String> {
    rows(s, "SELECT id, name, anki_model_id FROM note_type nt WHERE anki_model_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM note n WHERE n.note_type_id = nt.id)")
}

fn import(s: &AppState, p: &Path, history: bool) -> Result<Value, String> {
    run_import_anki(&|_, _, _| {}, s, p.to_str().unwrap(), history, &mut vec![]).map(|r| serde_json::to_value(&r).unwrap())
}

fn update(s: &AppState, p: &Path, choices: &deck_update::Choices, dry: bool) -> Result<Value, String> {
    let req = deck_update::Req { path: p, selected: None, choices, voices: &[], emit: &|_, _, _| {}, cancel: &|| false };
    let mut con = s.db.lock().unwrap();
    let r = if dry { deck_update::run(&mut con, &s.data_dir, &req, true, &mut vec![]) } else { deck_update::apply(&mut con, &s.data_dir, &req) };
    r.map(|x| serde_json::to_value(&x).unwrap())
}

/// 요약에서 판정에 쓰는 수만 (dry 와 apply 비교 · 근거)
fn brief(u: &Value) -> Value {
    json!({
        "changed": u["changed"], "merged": u["merged"], "conflicts": u["conflicts"].as_array().map_or(0, Vec::len),
        "mine_kept": u["mine_kept"], "same": u["same"], "no_base": u["no_base"], "other_type": u["other_type"],
        "new_notes": u["new_notes"], "new_note_cards": u["new_note_cards"], "missing_notes": u["missing_notes"], "gone": u["gone"],
        "new_cards": u["new_cards"], "emptied_cards": u["emptied_cards"], "cloze": u["cloze"], "kept_looks": u["kept_looks"],
        "media_new": u["media_new"], "reviews_skipped": u["reviews_skipped"], "duplicate_notes": u["duplicate_notes"],
    })
}

fn type_change<'a>(u: &'a Value, name: &str) -> &'a Value {
    u["types"].as_array().and_then(|a| a.iter().find(|t| t["name"] == name)).unwrap_or(&Value::Null)
}

fn ntid_by_name(s: &AppState, name: &str) -> i64 {
    s.db.lock().unwrap().query_row("SELECT id FROM note_type WHERE name = ?1 AND anki_model_id IS NOT NULL", [name], |r| r.get(0)).unwrap()
}

#[test]
#[ignore = "real_anki.py all 이 부른다 (pylib 로 만든 패키지가 필요)"]
fn phase1() {
    let w = work();
    let mut ck = Checks::default();
    let ref1 = read(&w.join("ref_v1.json"));
    let roles = &ref1["roles"];
    let ref_notes = ref1["notes"].as_object().unwrap().len();
    let ref_cards = ref1["cards"].as_array().unwrap().len();
    let ref_revs: usize = ref1["cards"].as_array().unwrap().iter().map(|c| c["revlog"].as_array().unwrap().len()).sum();

    // ---- 1 가져오기: 세 형식 (복습 기록 옮기기 켬)
    for (variant, file) in [("modern", "v1_modern.apkg"), ("legacy", "v1_legacy.apkg"), ("anki2", "v1_anki2.apkg")] {
        let s = state(&w.join(format!("neo_{variant}")), true);
        match import(&s, &w.join(file), true) {
            Ok(r) => {
                let ok = r["notes"] == ref_notes && r["cards"] == ref_cards && r["reviews"] == ref_revs;
                ck.add(
                    &format!("I0-{variant}"),
                    A1,
                    &format!("{file} 가져오기 (복습 기록 옮기기): 결과 수"),
                    ok,
                    format!("Anki 노트 {ref_notes} · 카드 {ref_cards} · 기록 {ref_revs} / Neo 결과 노트 {} · 카드 {} · 기록 {} · 미디어 {} · 알림 {}", r["notes"], r["cards"], r["reviews"], r["media"], r["issues"]),
                );
            }
            Err(e) => {
                ck.add(&format!("I0-{variant}"), A1, &format!("{file} 가져오기"), false, e);
                continue;
            }
        }
        write(&w.join(format!("neo_v1_{variant}.json")), &snapshot(&s, ROOT));
        let io: i64 = q(&s, "SELECT count(*) FROM note_type WHERE name = 'Image Occlusion' AND anki_model_id IS NOT NULL AND is_supported = 1");
        ck.add(&format!("I-io-{variant}"), A1, "가져온 그림 가리기(IO) 유형을 지원 유형으로 알아본다", io == 1, format!("가져온 IO 유형 중 is_supported {io}개"));
        if variant == "modern" {
            // 같은 파일을 다시 (새 노트만 더하기 + 기록 옮기기): 두 벌 · 기록 두 번이 없다 (I5 · I79)
            let (before, cards) = (counts(&s), card_rows(&s));
            let r = import(&s, &w.join(file), true);
            let after = counts(&s);
            let moved = changed_cards(&cards, &card_rows(&s));
            ck.add(
                "I-reimport",
                A1,
                "같은 .apkg 다시 가져오기: 노트 · 카드 · 기록이 늘지 않고 일정도 그대로 (I5 · I79)",
                r.is_ok() && before == after && after.3 == 0 && after.4 == 0 && moved.is_empty(),
                format!("(노트, 카드, 기록, 두 벌 guid, 두 번 기록) 전 {before:?} 후 {after:?}, 바뀐 카드 {moved:?}, 결과 {:?}", r.map(|r| r["issues"].clone())),
            );
        }
    }

    // ---- 2 덱 업데이트: v1 가져오기 → 내 편집 · 공부 → v2 (만든 사람 편집) 업데이트
    let du = state(&w.join("neo_du"), true);
    import(&du, &w.join("v1_modern.apkg"), true).unwrap();
    let g = |role: &str| guid_of(roles, role);
    edit(&du, &g("basic2"), "Front", "바나나!"); // 만든 사람도 Front → 충돌
    edit(&du, &g("rev1"), "Front", "앞1(내 수정)"); // 만든 사람은 Back → 합침
    edit(&du, &g("rev2"), "Back", "뒤2(내 수정)"); // 나만 → 내 것
    {
        // 카드 목록 [삭제] 와 같은 SQL (commands::delete_cards): 카드를 지우고 카드가 없는 노트를 지운다 → origin_gone 트리거
        let con = du.db.lock().unwrap();
        let nid = note_id_in(&con, &g("basic4"));
        con.execute("DELETE FROM card WHERE note_id = ?1", [nid]).unwrap();
        con.execute("DELETE FROM note WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM card WHERE note_id = note.id)", [nid]).unwrap();
    }
    let cloze_id = ntid_by_name(&du, "Cloze");
    {
        let con = du.db.lock().unwrap();
        let css: String = con.query_row("SELECT css FROM note_type WHERE id = ?1", [cloze_id], |r| r.get(0)).unwrap();
        notetype::save_css(&con, cloze_id, &format!("{css}\n.cloze {{ color: green; }}\n")).unwrap(); // 만든 사람도 CSS → 모양 충돌 (DU9)
    }
    // 내 공부: 복습 카드 하나 · 신규 카드 하나 (앱의 평가 함수)
    let (basic1_card, cust1_b): (i64, i64) = {
        let con = du.db.lock().unwrap();
        let c = |guid: &str, ord: i64| -> i64 {
            con.query_row("SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = ?1 AND c.template_ord = ?2", params![guid, ord], |r| r.get(0)).unwrap()
        };
        (c(&g("basic1"), 0), c(&g("cust1"), 1))
    };
    {
        let mut con = du.db.lock().unwrap();
        let now = now_ms();
        con.execute("INSERT INTO study_session (started_at, study_day) VALUES (?1, ?2)", params![now, db::date_from_day(study_day(now))]).unwrap();
        let sid = con.last_insert_rowid();
        commands::answer_for_qa(&mut con, &du.fsrs, sid, basic1_card, 3, now).unwrap();
        commands::answer_for_qa(&mut con, &du.fsrs, sid, cust1_b, 3, now + 1000).unwrap();
    }
    let (cards0, logs0, n0) = (card_rows(&du), rows(&du, "SELECT * FROM review_log ORDER BY id"), counts(&du));
    let v2 = w.join("v2_modern.apkg");
    let none = deck_update::Choices::default();
    let dry = update(&du, &v2, &none, true);
    let take = deck_update::Choices { take_new: vec![du.db.lock().map(|con| note_id_in(&con, &g("basic2"))).unwrap()], new_looks: vec![], ..Default::default() };
    let dry_take = update(&du, &v2, &take, true);
    // [새 버전으로] 고른 적용은 복사본에서 (충돌한 필드에만, DU6)
    let alt = copy_state(&du, &w.join("neo_du_take"));
    let alt_u = update(&alt, &v2, &take, false);
    // 같은 v2 를 구형(anki21, 스키마 11)으로: 판정이 형식과 상관없이 같아야 한다
    let leg = copy_state(&du, &w.join("neo_du_legacy"));
    let leg_u = update(&leg, &w.join("v2_legacy.apkg"), &none, true);
    let applied = update(&du, &v2, &none, false);
    let (dry, u) = match (dry, applied) {
        (Ok(d), Ok(u)) => (d, u),
        (d, u) => {
            ck.add("DU0", A2, "inspect_update · apply_update 가 끝난다", false, format!("dry={:?} apply={:?}", d.err(), u.err()));
            ck.save("rust1");
            panic!("덱 업데이트 실패");
        }
    };
    std::fs::write(w.join("du_summary.json"), serde_json::to_string_pretty(&u).unwrap()).unwrap();
    let b = brief(&u);
    ck.add("DU0", A2, "apply_update 가 자동 백업 뒤 적용한다", u["backup"].as_str().is_some_and(|p| Path::new(p).exists()), format!("backup={}", u["backup"]));
    ck.add("DU20", A2, "사전 검사(dry) 요약 = 적용 결과 (DU20)", brief(&dry) == b, format!("dry={} apply={b}", brief(&dry)));
    let want = json!({ "changed": 4, "merged": 1, "conflicts": 1, "mine_kept": 1, "same": 4, "no_base": 0, "new_notes": 2, "new_note_cards": 2, "missing_notes": 1, "gone": 1 });
    let got: serde_json::Map<String, Value> = want.as_object().unwrap().keys().map(|k| (k.clone(), b[k].clone())).collect();
    ck.add(
        "DU-table",
        A2,
        "판정표: 새 버전 4 (basic1 · cloze1 · cloze2 · cust1) · 합침 1 (rev1) · 충돌 1 (basic2) · 내 것 1 (rev2) · 그대로 4 · 새 노트 2 · 없어진 1 · 내가 지운 1",
        Value::Object(got.clone()) == want,
        format!("기대 {want} / 실제 {}", Value::Object(got)),
    );
    let c0 = &u["conflicts"][0];
    ck.add(
        "DU6",
        A2,
        "충돌 노트: 충돌한 필드는 Front 하나, 기본은 내 것, [새 버전으로] 고르면 요약에 반영",
        c0["fields"].as_array().is_some_and(|f| f.len() == 1 && f[0]["name"] == "Front") && c0["take_new"] == false && dry_take.as_ref().is_ok_and(|d| d["conflicts"][0]["take_new"] == true),
        format!("conflicts[0]={c0}, take_new 고른 dry={:?}", dry_take.map(|d| d["conflicts"][0]["take_new"].clone())),
    );
    ck.add(
        "DU6-apply",
        A2,
        "[새 버전으로] 고른 충돌 노트는 충돌한 필드만 새 버전 (복사본에 적용), 다른 노트는 기본과 같다",
        alt_u.is_ok() && field(&alt, &g("basic2"), "Front") == "바나나 🍌" && field(&alt, &g("rev1"), "Front") == "앞1(내 수정)",
        format!("basic2 Front={:?} · rev1 Front={:?} · 요약 {:?}", field(&alt, &g("basic2"), "Front"), field(&alt, &g("rev1"), "Front"), alt_u.as_ref().map(brief)),
    );
    ck.add("DU-legacy", A2, "v2 를 구형 .apkg(anki21)로 받아도 판정 요약이 신형과 같다", leg_u.as_ref().is_ok_and(|l| brief(l) == b), format!("legacy={:?}", leg_u.as_ref().map(brief)));
    let h = &u["hint"];
    ck.add(
        "DU15",
        A2,
        "B2-u0 근거: v2 노트 14개 중 11개가 이미 있고 같은 Anki 덱이 있어 업데이트가 기본",
        h["notes"] == 14 && h["existing_notes"] == 11 && h["suggested"] == true && h["known_decks"] == json!([ROOT]),
        format!("hint={h}"),
    );
    let vals = |guid: &str| du.db.lock().map(|con| named_fields(&con, note_id_in(&con, guid))).unwrap();
    let tags1: String = q(&du, &format!("SELECT tags FROM note WHERE anki_guid = '{}'", g("basic1").replace('\'', "''")));
    let fields_ok = [
        (vals(&g("basic1")) == vec![("Front".to_string(), "사과".to_string()), ("Back".to_string(), "apple (fruit)".to_string())] && tags1.split_whitespace().any(|t| t == "v2"), "basic1 = v2 값 + 태그 v2"),
        (field(&du, &g("basic2"), "Front") == "바나나!", "basic2 Front = 내 것 (충돌 기본)"),
        (field(&du, &g("rev1"), "Front") == "앞1(내 수정)" && field(&du, &g("rev1"), "Back") == "뒤1 (v2)", "rev1 = 내 Front + 새 Back"),
        (field(&du, &g("rev2"), "Back") == "뒤2(내 수정)", "rev2 Back = 내 것"),
        (field(&du, &g("cust1"), "Source") == "출처값", "cust1 새 필드 Source = 출처값"),
        (field(&du, &g("tin1"), "Answer") == "2", "tin1 이름 바뀐 필드 Answer = 2"),
    ];
    ck.add("DU-fields", A2, "노트 값 (새 버전 · 내 것 · 합침 · 새 필드 · 이름 바뀐 필드)", fields_ok.iter().all(|x| x.0), format!("{:?}", fields_ok.iter().map(|x| (x.1, x.0)).collect::<Vec<_>>()));
    let (cards1, logs1) = (card_rows(&du), rows(&du, "SELECT * FROM review_log ORDER BY id"));
    let moved = changed_cards(&cards0, &cards1);
    ck.add("DU12-sched", A2, "이미 있는 카드의 일정 · 상태 · 덱 · 중요도가 그대로 (FSRS · 내가 공부한 카드 포함)", moved.is_empty(), format!("카드 {}장 중 바뀐 것 {moved:?}", cards0.len()));
    let ref2 = read(&w.join("ref_v2.json"));
    let v2_revs: usize = ref2["cards"].as_array().unwrap().iter().map(|c| c["revlog"].as_array().unwrap().len()).sum();
    ck.add(
        "DU12-revlog",
        A2,
        "복습 기록은 그대로 (새 .apkg 기록은 옮기지 않고 수만 알린다)",
        logs0 == logs1 && u["reviews_skipped"] == v2_revs,
        format!("기록 전 {} · 후 {} 같음={}, reviews_skipped={} (v2 기록 {v2_revs})", logs0.len(), logs1.len(), logs0 == logs1, u["reviews_skipped"]),
    );
    // DU13 빈칸 번호: 기록 · 일정은 번호를 따른다. 없어진 번호의 카드는 지우지 않고, 새 번호는 신규
    let ords = |guid: &str| rows(&du, &format!("SELECT c.template_ord, c.state, (SELECT count(*) FROM review_log l WHERE l.card_id = c.id) FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = '{}' ORDER BY 1", guid.replace('\'', "''")));
    let (o1, o2) = (ords(&g("cloze1")), ords(&g("cloze2")));
    ck.add(
        "DU13",
        A2,
        "빈칸 번호: cloze1 c1↔c2 바꿈 = 카드 3장 그대로, cloze2 c2→c4 = c2 카드 · 기록 남음 · c4 신규 (둘 다 '번호 바뀐 것 같은 노트')",
        o1.len() == 3 && b["cloze"] == json!({ "renumbered_notes": 2, "new_cards": 1, "emptied_cards": 1 }) && o2.iter().any(|r| r.starts_with("1|") && r.ends_with("|1")) && o2.iter().any(|r| r == "3|new|0"),
        format!("cloze1 (ord|state|기록) {o1:?} · cloze2 {o2:?} · 요약 cloze {}", b["cloze"]),
    );
    let b3: i64 = q(&du, &format!("SELECT count(*) FROM card c JOIN note n ON n.id = c.note_id WHERE n.anki_guid = '{}'", g("basic3").replace('\'', "''")));
    let missing_ids = u["missing_card_ids"].as_array().map_or(0, Vec::len);
    ck.add("DU11", A2, "새 버전에 없는 노트(basic3)는 카드째 남고 [카드 목록에서 보기] 에 나온다", b3 == 1 && missing_ids == 1, format!("basic3 카드 {b3}장, missing_card_ids {}", u["missing_card_ids"]));
    let b4: i64 = q(&du, &format!("SELECT count(*) FROM note WHERE anki_guid = '{}'", g("basic4").replace('\'', "''")));
    ck.add("DU3", A2, "내가 지운 노트(basic4)는 다시 넣지 않는다", b4 == 0 && u["gone"] == 1, format!("basic4 노트 {b4}개, gone={}", u["gone"]));
    let cust = type_change(&u, "QA 특수필드");
    let cust_c: i64 = q(&du, "SELECT count(*) FROM card c JOIN note n ON n.id = c.note_id JOIN card_template t ON t.note_type_id = n.note_type_id AND t.ord = c.template_ord WHERE t.name = '카드C' AND c.state = 'new'");
    ck.add(
        "DU-type-new",
        A2,
        "사용자 정의 유형: 새 필드 Source · 새 카드 종류 카드C (노트 2개에 신규 카드)",
        cust["new_fields"] == json!(["Source"]) && cust["new_templates"] == json!(["카드C"]) && cust["new_template_cards"] == 2 && cust_c == 2,
        format!("{}", json!({ "new_fields": cust["new_fields"], "new_templates": cust["new_templates"], "new_template_cards": cust["new_template_cards"], "카드C 신규": cust_c })),
    );
    let tin = type_change(&u, "Basic (type in the answer)");
    let tin_q: String = q(&du, "SELECT t.qfmt FROM card_template t JOIN note_type nt ON nt.id = t.note_type_id WHERE nt.name = 'Basic (type in the answer)' AND t.ord = 0");
    ck.add("DU-type-rename", A2, "이름 바뀐 필드 Back → Answer (템플릿 참조도)", tin["renamed_fields"] == json!([["Back", "Answer"]]) && tin_q.contains("{{type:Answer}}"), format!("renamed={} qfmt={tin_q:?}", tin["renamed_fields"]));
    let basic = type_change(&u, "Basic");
    let basic_q: String = q(&du, "SELECT t.qfmt FROM card_template t JOIN note_type nt ON nt.id = t.note_type_id WHERE nt.name = 'Basic' AND nt.anki_model_id IS NOT NULL AND t.ord = 0");
    ck.add("DU-look-new", A2, "만든 사람만 바꾼 카드 모양(Basic Card 1 앞면)은 새 모양으로", basic["updated_looks"] == json!(["Card 1"]) && basic_q == "<div class=front>{{Front}}</div>", format!("updated_looks={} qfmt={basic_q:?}", basic["updated_looks"]));
    let cl = type_change(&u, "Cloze");
    let cl_css: String = q(&du, &format!("SELECT css FROM note_type WHERE id = {cloze_id}"));
    ck.add(
        "DU9",
        A2,
        "둘 다 바꾼 CSS(Cloze)는 충돌로 보이고 기본은 내 모양",
        cl["looks"].as_array().is_some_and(|l| l.len() == 1 && l[0]["name"] == "CSS" && l[0]["take_new"] == false) && cl_css.contains("color: green") && !cl_css.contains("color: red") && u["kept_looks"] == 1,
        format!("looks={} kept_looks={} 지금 CSS {cl_css:?}", cl["looks"], u["kept_looks"]),
    );
    let again = update(&du, &v2, &none, true).map(|x| brief(&x));
    ck.add(
        "DU-idem",
        A2,
        "같은 v2 를 다시 사전 검사: 바뀌는 것 · 충돌 · 새 노트 · 새 카드 없음 (내 것 3 은 지킨 것)",
        again.as_ref().is_ok_and(|a| a["changed"] == 0 && a["merged"] == 0 && a["conflicts"] == 0 && a["new_notes"] == 0 && a["new_cards"] == 0 && a["kept_looks"] == 0),
        format!("{again:?}"),
    );
    let n1 = counts(&du);
    ck.add("DU-dup", A2, "업데이트 뒤 같은 guid 두 벌 · 같은 기록 두 번 없음", n1.3 == 0 && n1.4 == 0, format!("전 {n0:?} 후 {n1:?}"));

    // ---- 3 내보내기: 업데이트까지 한 DB 를 기록과 함께 .apkg 로 (비교는 real_anki.py anki). 앱에서 만든 노트 하나도 함께 (왕복 기준 확인)
    let rid: i64 = q(&du, &format!("SELECT id FROM deck WHERE name = '{ROOT}' AND parent_id IS NULL"));
    {
        let con = du.db.lock().unwrap();
        let d2: i64 = con.query_row("SELECT id FROM deck WHERE parent_id = ?1 AND name = '2장'", [rid], |r| r.get(0)).unwrap();
        let card = commands::NewCard { deck_id: d2, kind: "basic".into(), front: "Neo에서 만든 노트".into(), back: "neo".into(), importance: None };
        commands::insert_note(&con, &card, "", None).unwrap();
    }
    let out = w.join("neo_export.apkg");
    let r = {
        let con = du.db.lock().unwrap();
        crate::export::export_for_qa(&con, &du.data_dir, rid, &out, true)
    };
    match r {
        Ok(r) => {
            let r = serde_json::to_value(&r).unwrap();
            ck.add("X0", A3, "Neo 내보내기 (.apkg, 복습 기록 포함)", r["missing_media"] == 0 && r["skipped_cards"] == 0, format!("{r}"));
        }
        Err(e) => ck.add("X0", A3, "Neo 내보내기", false, e),
    }
    let mut st = snapshot(&du, ROOT);
    st["native_guid"] = json!(q::<String>(&du, "SELECT anki_guid FROM note WHERE import_batch_id IS NULL AND anki_note_id IS NULL"));
    write(&w.join("neo_state.json"), &st);
    ck.save("rust1");
}

#[test]
#[ignore = "real_anki.py all 이 부른다 (Anki 에서 고치고 공부한 anki_rt.apkg 가 필요)"]
fn phase2() {
    let w = work();
    let mut ck = Checks::default();
    let rt = read(&w.join("rt.json"));
    let src = state(&w.join("neo_du"), false);
    let pkg = w.join("anki_rt.apkg");
    let edited = rt["edited_guid"].as_str().unwrap();
    let new_guid = rt["new_guid"].as_str().unwrap();
    let new_reviews = rt["new_reviews"].as_i64().unwrap();
    let rt_revs: usize = rt["cards"].as_array().unwrap().iter().map(|c| c["revlog"].as_array().unwrap().len()).sum();

    // 새 노트만 더하기 + 기록 옮기기 (I5 · I79): 두 벌 없음, 내 일정 그대로, 기록은 Anki 에서 새로 한 것만 더해진다
    let s = copy_state(&src, &w.join("neo_rt_add"));
    let (n0, c0, l0, e0) = (counts(&s), card_rows(&s), rows(&s, "SELECT * FROM review_log ORDER BY id"), empty_types(&s));
    let r = import(&s, &pkg, true);
    let e1 = empty_types(&s);
    ck.add("RT-add-types", A4, "새 노트만 더하기: 앱에서 만든 노트 유형이 Anki 왕복 뒤 빈 새 유형으로 하나 더 생기지 않는다", e1 == e0, format!("노트 없는 유형 전 {e0:?} 후 {e1:?}"));
    let (n1, c1, l1) = (counts(&s), card_rows(&s), rows(&s, "SELECT * FROM review_log ORDER BY id"));
    let moved = changed_cards(&c0, &c1);
    let has_new: i64 = q(&s, &format!("SELECT count(*) FROM note WHERE anki_guid = '{}'", new_guid.replace('\'', "''")));
    ck.add("RT-add-0", A4, "anki_rt.apkg 새 노트만 더하기 (복습 기록 옮기기)", r.is_ok(), format!("{:?}", r.as_ref().map(|r| json!({"notes": r["notes"], "cards": r["cards"], "reviews": r["reviews"], "issues": r["issues"]}))));
    ck.add("RT-add-notes", A4, "새 노트(Anki 에서 더한 것)만 늘고 두 벌 노트가 없다", n1.0 == n0.0 + 1 && has_new == 1 && n1.3 == 0, format!("노트 {} → {}, 새 guid {has_new}개, 두 벌 guid {}", n0.0, n1.0, n1.3));
    ck.add("RT-add-sched", A4, "이미 있는 카드의 일정은 덮지 않는다 (Anki 에서 공부했어도)", moved.is_empty(), format!("카드 {}장 중 바뀐 것 {moved:?}", c0.len()));
    ck.add(
        "I79",
        A4,
        "복습 기록: 원래 기록은 두 번 들어가지 않고 Anki 에서 새로 한 평가만 더해진다",
        n1.2 == n0.2 + new_reviews && n1.4 == 0 && l1.len() >= l0.len() && l1[..l0.len()] == l0[..],
        format!("기록 {} → {} (Anki 새 평가 {new_reviews}), 같은 카드 · 같은 시각 두 번 {}", n0.2, n1.2, n1.4),
    );
    let b1: String = s.db.lock().map(|con| named_fields(&con, note_id_in(&con, edited))).unwrap().into_iter().find(|x| x.0 == "Back").map(|x| x.1).unwrap_or_default();
    ck.add("RT-add-keep", A4, "새 노트만 더하기는 이미 있는 노트 값을 바꾸지 않는다 (Anki 편집은 업데이트 모드로)", b1 == "apple (fruit)", format!("basic1 Back = {b1:?}"));

    // 업데이트로 가져오기: Anki 편집은 새 버전, 내 수정 · 일정 · 기록 그대로
    let s = copy_state(&src, &w.join("neo_rt_upd"));
    let (n0, c0, l0, e0) = (counts(&s), card_rows(&s), rows(&s, "SELECT * FROM review_log ORDER BY id"), empty_types(&s));
    let none = deck_update::Choices::default();
    let dry = update(&s, &pkg, &none, true);
    let u = update(&s, &pkg, &none, false);
    let (n1, c1, l1) = (counts(&s), card_rows(&s), rows(&s, "SELECT * FROM review_log ORDER BY id"));
    let Ok(u) = u else {
        ck.add("RT-upd-0", A4, "anki_rt.apkg 업데이트로 가져오기", false, format!("{u:?}"));
        ck.save("rust2");
        return;
    };
    let native_guid = rt["native_guid"].as_str().unwrap();
    let native = s.db.lock().map(|con| note_id_in(&con, native_guid)).unwrap();
    let e1 = empty_types(&s);
    ck.add(
        "RT-native-type",
        A4,
        "업데이트: 앱에서 만든 노트는 Anki 왕복 뒤에도 같은 노트 유형으로 알아본다 (노트 유형 다름 0, 빈 새 유형 없음)",
        u["other_type"] == 0 && e1 == e0,
        format!("other_type={} · 노트 없는 유형 전 {e0:?} 후 {e1:?}", u["other_type"]),
    );
    std::fs::write(w.join("rt_update_summary.json"), serde_json::to_string_pretty(&u).unwrap()).unwrap();
    let b = brief(&u);
    ck.add("RT-upd-0", A4, "anki_rt.apkg 업데이트로 가져오기 (dry = 적용)", dry.as_ref().is_ok_and(|d| brief(d) == b), format!("{b}"));
    ck.add(
        "RT-upd-table",
        A4,
        "판정: Anki 에서 고친 노트 1개만 새 버전, 새 노트 1, 충돌 · 내 것 · 합침 0 (내 수정은 내보낸 값이 돌아와 그대로)",
        b["changed"].as_i64() >= Some(1) && b["new_notes"] == 1 && b["mine_kept"] == 0 && b["merged"] == 0 && u["conflicts"].as_array().is_some_and(|c| c.iter().all(|c| c["note_id"] == native)),
        format!("{b} (앱에서 만든 노트는 RT-native 에서 따로)"),
    );
    let types: Vec<Value> = u["types"].as_array().cloned().unwrap_or_default();
    let created: Vec<Value> = types.iter().filter(|t| t["created"] == true).map(|t| json!([t["name"], t["note_type_id"]])).collect();
    ck.add(
        "RT-upd-types",
        A4,
        "이미 있는 노트 유형은 그대로 (Anki 왕복이 필드 · 템플릿 · CSS 를 바꾸지 않았다)",
        types.iter().all(|t| t["created"] == true) && u["kept_looks"] == 0,
        format!("바뀐 유형 {}, 새로 만든 유형 {created:?}", Value::Array(types.iter().filter(|t| t["created"] != true).cloned().collect())),
    );
    let b1: String = s.db.lock().map(|con| named_fields(&con, note_id_in(&con, edited))).unwrap().into_iter().find(|x| x.0 == "Back").map(|x| x.1).unwrap_or_default();
    ck.add("RT-upd-field", A4, "Anki 에서 고친 값이 들어온다", b1 == "apple (Anki에서 고침)", format!("basic1 Back = {b1:?}"));
    let moved = changed_cards(&c0, &c1);
    ck.add("RT-upd-sched", A4, "업데이트: 이미 있는 카드 일정 그대로, 기록 그대로 (Anki 기록은 옮기지 않는다)", moved.is_empty() && l0 == l1 && u["reviews_skipped"] == rt_revs, format!("바뀐 카드 {moved:?}, 기록 {} → {}, reviews_skipped={} (패키지 {rt_revs})", l0.len(), l1.len(), u["reviews_skipped"]));
    ck.add("RT-upd-dup", A4, "두 벌 노트 · 두 번 기록 없음", n1.3 == 0 && n1.4 == 0 && n1.0 == n0.0 + 1, format!("전 {n0:?} 후 {n1:?}"));
    // 앱에서 만든 노트를 Anki 에서 고쳐 돌아왔다: 기준(내보낼 때 origin) · 노트 유형 연결이 있으면 새 버전으로 들어와야 한다 (deck-update.md 8장 미룬 것, C 2단계)
    let nv: Vec<(String, String)> = s.db.lock().map(|con| named_fields(&con, native)).unwrap();
    let in_conflict = u["conflicts"].as_array().is_some_and(|c| c.iter().any(|c| c["note_id"] == native));
    ck.next(
        "RT-native",
        A4,
        "앱에서 만든 노트를 Anki 에서 고쳐 업데이트로 가져오면 묻지 않고 새 버전으로 들어온다 (내보낼 때 기준, deck-update.md 8장 미룬 것)",
        nv.get(1).is_some_and(|x| x.1 == rt["native_value"]) && !in_conflict && u["no_base"] == 0,
        format!("값 {nv:?}, 충돌={in_conflict}, other_type={}, no_base={}, 새로 만든 유형 {created:?}", u["other_type"], u["no_base"]),
    );
    ck.save("rust2");
}
