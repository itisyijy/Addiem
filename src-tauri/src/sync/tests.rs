//! 두 기기 시뮬레이션: 임시 DB 두 개 + 같이 쓰는 폴더 하나 (FolderStore). Google 계정 없이 엔진 전체를 돈다

use super::engine::{self, Ctx, Join, Pulled};
use super::store::{FolderStore, Store, SyncError};
use crate::db;
use crate::srs::{self, SrsState};
use rusqlite::{Connection, params};
use serde_json::{Map, Value as J, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

fn tmp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("neoanki-sync-{tag}-{}", super::crypto::random_hex(6)))
}

fn quiet(_: &str, _: u64, _: u64) {}

struct Env {
    root: PathBuf,
    store: FolderStore,
    fsrs: fsrs::FSRS,
}

impl Env {
    fn new() -> Env {
        let root = tmp("drive");
        Env { store: FolderStore::new(&root), root, fsrs: fsrs::FSRS::default() }
    }

    fn cx(&self, d: &Dev) -> Ctx<'_> {
        self.cx_key(d, None)
    }

    fn cx_key(&self, d: &Dev, key: Option<[u8; 32]>) -> Ctx<'_> {
        Ctx { store: &self.store, key, media_dir: d.dir.join("media"), fsrs: &self.fsrs, progress: &quiet }
    }

    /// 앱이 하는 한 바퀴: 받기 -> 올리기 -> 기기 파일
    fn sync(&self, d: &Dev) -> engine::Report {
        let cx = self.cx(d);
        let report = match engine::pull(&d.db, &cx).unwrap() {
            Pulled::Done(r) => r,
            Pulled::Choice(c) => panic!("고르는 창이 뜰 상황이 아니다: {:?}", c.header),
            Pulled::Gone => panic!("Drive 데이터가 없어졌다"),
        };
        engine::push(&d.db, &cx).unwrap();
        engine::write_device(&d.db, &cx, &json!([])).unwrap();
        report
    }

    fn files(&self, prefix: &str) -> Vec<String> {
        self.store.list(prefix).unwrap().into_iter().map(|e| e.path).collect()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

type Dump = BTreeMap<String, Vec<(String, Map<String, J>)>>;

struct Dev {
    dir: PathBuf,
    db: Mutex<Connection>,
}

impl Dev {
    fn new(name: &str) -> Dev {
        let dir = tmp(name);
        std::fs::create_dir_all(dir.join("media")).unwrap();
        let con = db::open(&dir.join("neoanki.sqlite")).unwrap();
        con.execute("UPDATE schema_meta SET value = ?1 WHERE key = 'device_name'", [name]).unwrap();
        Dev { dir, db: Mutex::new(con) }
    }

    fn con(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap()
    }

    fn n(&self, sql: &str) -> i64 {
        self.con().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn deck(&self, name: &str, parent: Option<i64>) -> i64 {
        let con = self.con();
        con.execute("INSERT INTO deck (name, parent_id, created_at, updated_at) VALUES (?1, ?2, 1000, 1000)", params![name, parent]).unwrap();
        con.last_insert_rowid()
    }

    /// Basic 노트 하나와 카드 한 장. (노트 id, 카드 id)
    fn note(&self, deck: i64, front: &str, guid: Option<&str>, updated_at: i64) -> (i64, i64) {
        let con = self.con();
        let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Basic' AND anki_model_id IS NULL", [], |r| r.get(0)).unwrap();
        con.execute(
            "INSERT INTO note (note_type_id, fields, anki_guid, created_at, updated_at) VALUES (?1, ?2, ?3, 1000, ?4)",
            params![nt, json!([front, "답"]).to_string(), guid, updated_at],
        )
        .unwrap();
        let note = con.last_insert_rowid();
        con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, 0, 1000, ?3)", params![note, deck, updated_at]).unwrap();
        (note, con.last_insert_rowid())
    }

    fn uid(&self, table: &str, id: i64) -> String {
        self.con().query_row(&format!("SELECT uid FROM {table} WHERE rowid = ?1"), [id], |r| r.get(0)).unwrap()
    }

    fn id(&self, table: &str, uid: &str) -> Option<i64> {
        use rusqlite::OptionalExtension;
        self.con().query_row(&format!("SELECT rowid FROM {table} WHERE uid = ?1"), [uid], |r| r.get(0)).optional().unwrap()
    }

    fn srs(&self, card: i64) -> SrsState {
        self.con()
            .query_row("SELECT state, srs_step, stability, difficulty, due_at, interval_days, last_reviewed_at, reps, lapses FROM card WHERE id = ?1", [card], |r| {
                Ok(SrsState {
                    state: r.get(0)?,
                    step: r.get(1)?,
                    stability: r.get(2)?,
                    difficulty: r.get(3)?,
                    due_at: r.get(4)?,
                    interval_days: r.get(5)?,
                    last_reviewed_at: r.get(6)?,
                    reps: r.get(7)?,
                    lapses: r.get(8)?,
                })
            })
            .unwrap()
    }

    /// commands::answer 와 같은 순서: 기록을 넣고 카드 일정을 바꾼다
    fn review(&self, fsrs: &fsrs::FSRS, card: i64, rating: u8, at: i64) {
        let prev = self.srs(card);
        let next = srs::review(fsrs, &prev, rating, at);
        let con = self.con();
        con.execute(
            "INSERT INTO review_log (card_id, reviewed_at, rating, prev_state, fed_to_scheduler) VALUES (?1, ?2, ?3, ?4, 1)",
            params![card, at, rating, prev.state],
        )
        .unwrap();
        con.execute(
            "UPDATE card SET state = ?2, srs_step = ?3, stability = ?4, difficulty = ?5, due_at = ?6, interval_days = ?7,
                    last_reviewed_at = ?8, reps = ?9, lapses = ?10, updated_at = ?8 WHERE id = ?1",
            params![card, next.state, next.step, next.stability, next.difficulty, next.due_at, next.interval_days, at, next.reps, next.lapses],
        )
        .unwrap();
    }

    /// 동기화 대상 표 전체 (uid 순서, 참조는 uid). 두 기기가 같으면 맞춰진 것이다
    fn dump(&self) -> Dump {
        let con = self.con();
        let mut out = BTreeMap::new();
        for t in engine::tables(&con).unwrap() {
            let ids: Vec<i64> = con.prepare(&format!("SELECT rowid FROM {} ORDER BY uid", t.name)).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            out.insert(t.name.to_string(), ids.into_iter().filter_map(|id| engine::export(&con, &t, id).unwrap()).collect());
        }
        out
    }

    fn outbox(&self) -> i64 {
        self.n("SELECT count(*) FROM sync_outbox")
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        if let Ok(mut c) = self.db.lock() {
            *c = Connection::open_in_memory().unwrap();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 노트북(A)이 처음 연결하고 데스크톱(B, 빈 기기)이 Drive 데이터로 시작한 상태
fn pair(env: &Env) -> (Dev, Dev, i64) {
    let a = Dev::new("노트북");
    let b = Dev::new("데스크톱");
    let deck = a.deck("빅데이터분석기사", None);
    let sub = a.deck("2과목", Some(deck));
    let (_, card) = a.note(sub, "결측값 처리 방법?", None, 1000);
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();
    engine::join(&b.db, &env.cx(&b), Join::Take, json!({})).unwrap();
    (a, b, card)
}

#[test]
fn first_device_writes_snapshot_and_second_device_takes_it() {
    let env = Env::new();
    let (a, b, card) = pair(&env);
    assert_eq!(env.files("snapshot/").len(), 1);
    assert!(env.files("snapshot/")[0].starts_with("snapshot/g0001-"), "첫 세대");
    assert_eq!(a.outbox(), 0, "스냅샷에 들어간 변경은 다시 올리지 않는다");
    assert_eq!(a.dump(), b.dump(), "같은 덱 · 노트 · 카드, 같은 uid");
    assert_eq!(b.n("SELECT count(*) FROM note_type"), a.n("SELECT count(*) FROM note_type"), "기본 노트 유형이 두 벌이 되지 않는다");
    assert!(b.id("card", &a.uid("card", card)).is_some());
    assert_eq!(b.n("SELECT count(*) FROM schema_meta WHERE key = 'device_uid' AND value = (SELECT value FROM schema_meta WHERE key = 'device_uid')"), 1);
    // 기기 파일
    env.sync(&a);
    env.sync(&b);
    assert_eq!(env.files("devices/").len(), 2);
}

#[test]
fn edits_on_both_devices_meet() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let deck_a = a.n("SELECT id FROM deck WHERE name = '2과목'");
    a.note(deck_a, "A 에서 만든 카드", None, 2000);
    let deck_b = b.n("SELECT id FROM deck WHERE name = '빅데이터분석기사'");
    b.con().execute("UPDATE deck SET name = '빅분기', updated_at = 3000 WHERE id = ?1", [deck_b]).unwrap();
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '빅분기'"), 1);
    assert_eq!(b.n("SELECT count(*) FROM note"), 2);
}

#[test]
fn applying_flag_prevents_echo() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let deck = a.n("SELECT id FROM deck WHERE name = '2과목'");
    a.note(deck, "새 카드", None, 2000);
    a.con().execute("UPDATE deck SET name = '2과목 (개정)' WHERE id = ?1", [deck]).unwrap();
    env.sync(&a);
    let report = env.sync(&b);
    assert!(report.applied >= 3);
    assert_eq!(b.outbox(), 0, "받은 변경이 다시 올릴 변경으로 쌓이지 않는다");
    assert_eq!(b.n("SELECT applying FROM sync_flags"), 0, "적용이 끝나면 늘 꺼진다");
    // B 는 올릴 것이 없으니 새 변경 파일을 만들지 않는다
    let b_uid: String = b.con().query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get(0)).unwrap();
    assert!(env.files(&format!("changes/{b_uid}/")).is_empty());
}

#[test]
fn conflicting_content_edit_keeps_losing_version() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let note_a = a.n("SELECT id FROM note");
    let note_b = b.n("SELECT id FROM note");
    let a_fields = json!(["결측값 처리 방법?", "1. 단순 삭제 — 행 제거"]).to_string();
    let b_fields = json!(["결측값 처리 방법?", "1. 단순 삭제"]).to_string();
    a.con().execute("UPDATE note SET fields = ?2, updated_at = 5000 WHERE id = ?1", params![note_a, a_fields]).unwrap();
    env.sync(&a);
    b.con().execute("UPDATE note SET fields = ?2, updated_at = 6000 WHERE id = ?1", params![note_b, b_fields]).unwrap();
    let rb = env.sync(&b);
    let ra = env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    let now: String = a.con().query_row("SELECT fields FROM note", [], |r| r.get(0)).unwrap();
    assert_eq!(now, b_fields, "나중에 맞춘 쪽");
    assert_eq!((rb.conflicts, ra.conflicts), (1, 1), "두 기기 모두 K7 기록을 남긴다");
    for d in [&a, &b] {
        let (win, lose): (String, String) = d.con().query_row("SELECT winner_fields, loser_fields FROM sync_conflict", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((win.as_str(), lose.as_str()), (b_fields.as_str(), a_fields.as_str()));
    }
    // K7 [이 버전으로 바꾸기] 는 평범한 고치기라 다시 맞춰진다. 새 충돌은 생기지 않는다
    a.con().execute("UPDATE note SET fields = ?2, updated_at = 7000 WHERE id = ?1", params![note_a, a_fields]).unwrap();
    env.sync(&a);
    let rb = env.sync(&b);
    assert_eq!(rb.conflicts, 0);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn same_card_reviewed_on_both_devices_recomputes_fsrs() {
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let card_b = b.id("card", &a.uid("card", card_a)).unwrap();
    let s0 = a.srs(card_a);
    assert_eq!(s0, b.srs(card_b));
    let t1 = 1_800_000_000_000;
    let t2 = t1 + 60_000;
    // 노트북에서 Good, 데스크톱에서 (아직 받지 않은 채) Again
    a.review(&env.fsrs, card_a, 3, t1);
    env.sync(&a);
    b.review(&env.fsrs, card_b, 1, t2);
    let rb = env.sync(&b);
    env.sync(&a);
    assert_eq!(rb.recomputed, 1, "두 기기에서 복습한 카드만 다시 계산한다");
    assert_eq!(a.n("SELECT count(*) FROM review_log"), 2, "복습 기록은 합집합");
    assert_eq!(b.n("SELECT count(*) FROM review_log"), 2);
    let expected = srs::review(&env.fsrs, &srs::review(&env.fsrs, &s0, 3, t1), 1, t2);
    assert_eq!(b.srs(card_b), expected, "공통 시점부터 합친 기록을 시간 순으로");
    assert_eq!(a.srs(card_a), expected, "두 기기의 일정이 같다");
    assert_eq!(a.dump(), b.dump());
    // 한 기기에서만 복습하면 그 기기 상태를 그대로
    a.review(&env.fsrs, card_a, 3, t2 + 86_400_000);
    env.sync(&a);
    let rb = env.sync(&b);
    assert_eq!(rb.recomputed, 0);
    assert_eq!(b.srs(card_b), a.srs(card_a));
}

#[test]
fn delete_versus_later_edit() {
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let uid = a.uid("card", card_a);
    // 노트북에서 지우고, 데스크톱에서 (나중에) 중요도를 바꿨다: 고친 쪽이 나중이면 되살린다
    a.con().execute("DELETE FROM card WHERE id = ?1", [card_a]).unwrap();
    env.sync(&a);
    let card_b = b.id("card", &uid).unwrap();
    b.con().execute("UPDATE card SET importance = 2, importance_source = 'user', updated_at = 9000 WHERE id = ?1", [card_b]).unwrap();
    env.sync(&b);
    env.sync(&a);
    let back = a.id("card", &uid).expect("되살아났다");
    assert_eq!(a.n(&format!("SELECT importance FROM card WHERE id = {back}")), 2);
    assert_eq!(a.dump(), b.dump());
    // 지운 쪽이 나중이면 지운다
    b.con().execute("UPDATE card SET importance = 0 WHERE uid = ?1", [&uid]).unwrap();
    env.sync(&b);
    env.sync(&a);
    a.con().execute("DELETE FROM card WHERE uid = ?1", [&uid]).unwrap();
    env.sync(&a);
    env.sync(&b);
    assert!(a.id("card", &uid).is_none() && b.id("card", &uid).is_none());
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn media_uploaded_once_by_hash_and_fetched() {
    let env = Env::new();
    let a = Dev::new("노트북");
    let png = b"\x89PNG fake image bytes".to_vec();
    std::fs::write(a.dir.join("media/neo-1.png"), &png).unwrap();
    a.con().execute("INSERT INTO media (filename, rel_path, sha256, byte_size, created_at) VALUES ('neo-1.png', 'media/neo-1.png', '', 22, 1)", []).unwrap();
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();
    let sha = super::crypto::hex(&super::crypto::sha256(&png));
    assert_eq!(env.files("media/"), [format!("media/{sha}.png")], "이름이 내용 해시");
    assert_eq!(a.n("SELECT count(*) FROM media WHERE sha256 = '' "), 0, "올리기 전에 해시를 맞췄다");
    let b = Dev::new("데스크톱");
    engine::join(&b.db, &env.cx(&b), Join::Take, json!({})).unwrap();
    assert_eq!(std::fs::read(b.dir.join("media/neo-1.png")).unwrap(), png, "그림을 받았다");
    // 같은 그림을 두 기기에서 따로 넣어도 Drive 에는 하나, 행도 하나로 합쳐진다
    let same = b"same picture".to_vec();
    for d in [&a, &b] {
        std::fs::write(d.dir.join("media/neo-2.png"), &same).unwrap();
        d.con().execute("INSERT INTO media (filename, rel_path, sha256, byte_size, created_at) VALUES ('neo-2.png', 'media/neo-2.png', '', 12, 2)", []).unwrap();
    }
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(env.files("media/").len(), 2);
    assert_eq!(a.n("SELECT count(*) FROM media WHERE filename = 'neo-2.png'"), 1);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn same_deck_created_on_both_devices_becomes_one() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    a.deck("새 시험", None);
    b.deck("새 시험", None);
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '새 시험'"), 1);
    assert_eq!(b.n("SELECT count(*) FROM deck WHERE name = '새 시험'"), 1);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn daily_plan_keeps_the_earliest_plan_of_the_day() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let deck_a = a.n("SELECT id FROM deck WHERE parent_id IS NULL");
    let deck_b = b.n("SELECT id FROM deck WHERE parent_id IS NULL");
    a.con().execute("INSERT INTO daily_plan (study_day, deck_id, planned_review, planned_new, has_target, created_at) VALUES ('2026-10-14', ?1, 10, 5, 1, 1000)", [deck_a]).unwrap();
    b.con().execute("INSERT INTO daily_plan (study_day, deck_id, planned_review, planned_new, has_target, created_at) VALUES ('2026-10-14', ?1, 99, 9, 1, 5000)", [deck_b]).unwrap();
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(b.n("SELECT planned_review FROM daily_plan"), 10, "그날 처음 계산한 계획");
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn k6m_merge_keeps_both_sides() {
    let env = Env::new();
    let a = Dev::new("노트북");
    let common_a = a.deck("공통", None);
    a.deck("노트북 전용", None);
    a.note(common_a, "A 질문", Some("G1"), 1000);
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();

    let b = Dev::new("데스크톱");
    let common_b = b.deck("공통", None);
    let only_b = b.deck("데스크톱 전용", None);
    b.note(common_b, "B 질문 (나중에 고침)", Some("G1"), 2000);
    b.note(only_b, "B 에만 있는 카드", None, 2000);
    let report = engine::join(&b.db, &env.cx(&b), Join::Merge, json!({})).unwrap();
    assert_eq!(b.n("SELECT count(*) FROM deck WHERE name = '공통'"), 1, "이름 경로가 같은 덱은 같은 덱");
    assert_eq!(b.n("SELECT count(*) FROM deck"), 3);
    assert_eq!(b.n("SELECT count(*) FROM note WHERE anki_guid = 'G1'"), 1, "같은 .apkg 노트는 하나로");
    assert_eq!(b.n("SELECT count(*) FROM note_type"), a.n("SELECT count(*) FROM note_type"), "기본 노트 유형은 하나씩");
    let g1: String = b.con().query_row("SELECT fields FROM note WHERE anki_guid = 'G1'", [], |r| r.get(0)).unwrap();
    assert!(g1.contains("B 질문"), "나중에 고친 쪽");
    assert_eq!(report.conflicts, 1, "진 쪽 내용은 K7 에 남는다");
    assert!(b.outbox() > 0, "이 기기에만 있던 것을 올린다");
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '데스크톱 전용'"), 1);
}

#[test]
fn k6m_take_and_fresh() {
    let env = Env::new();
    let (a, _b, _) = pair(&env);
    // 데이터가 있는 세 번째 기기가 Drive 데이터로 바꾼다
    let c = Dev::new("태블릿");
    c.deck("지울 덱", None);
    engine::join(&c.db, &env.cx(&c), Join::Take, json!({})).unwrap();
    assert_eq!(c.n("SELECT count(*) FROM deck WHERE name = '지울 덱'"), 0);
    assert_eq!(a.dump(), c.dump());
    // 새로 시작하면 세대가 오르고, 다른 기기는 받지 않고 고르는 창을 본다
    c.deck("새 출발", None);
    engine::join(&c.db, &env.cx(&c), Join::Fresh, json!({ "kind": "fresh", "device_name": "태블릿" })).unwrap();
    assert!(env.files("snapshot/").iter().any(|p| p.starts_with("snapshot/g0002-")));
    match engine::pull(&a.db, &env.cx(&a)).unwrap() {
        Pulled::Choice(ch) => {
            assert_eq!(ch.generation, 2);
            assert_eq!(ch.header["reason"]["kind"], "fresh");
        }
        _ => panic!("세대가 바뀌면 받지 않는다"),
    }
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '새 출발'"), 0, "몰래 덮지 않는다");
    engine::join(&a.db, &env.cx(&a), Join::Take, json!({})).unwrap();
    assert_eq!(a.dump(), c.dump());
}

#[test]
fn k9_restore_starts_new_generation() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let zip = engine::backup_zip(&a.con(), &a.dir).unwrap();
    let deck = a.n("SELECT id FROM deck WHERE name = '2과목'");
    a.note(deck, "백업 뒤에 만든 카드", None, 3000);
    env.sync(&a);
    env.sync(&b);
    assert_eq!(b.n("SELECT count(*) FROM note"), 2);
    // 노트북에서 되돌린다: 백업 DB 를 이 기기 정체로 바꿔 끼우고 세대를 올린다
    let path = a.dir.join("restored.sqlite");
    engine::unzip_db(&zip, &path).unwrap();
    let restored = db::open(&path).unwrap();
    engine::adopt_restored(&a.con(), &restored).unwrap();
    let me: String = a.con().query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get(0)).unwrap();
    *a.con() = restored;
    assert_eq!(a.n("SELECT count(*) FROM note"), 1);
    assert_eq!(a.con().query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get::<_, String>(0)).unwrap(), me, "기기 정체는 그대로");
    engine::join(&a.db, &env.cx(&a), Join::Fresh, json!({ "kind": "restore", "device_name": "노트북" })).unwrap();
    match engine::pull(&b.db, &env.cx(&b)).unwrap() {
        Pulled::Choice(ch) => assert_eq!(ch.header["reason"]["kind"], "restore"),
        _ => panic!("다른 기기는 K6-m 을 본다"),
    }
    engine::join(&b.db, &env.cx(&b), Join::Take, json!({})).unwrap();
    assert_eq!(b.n("SELECT count(*) FROM note"), 1, "백업 시점으로");
    assert_eq!(a.dump(), b.dump());
    // 그 뒤로는 평소처럼
    a.deck("되돌린 뒤", None);
    env.sync(&a);
    env.sync(&b);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn offline_device_converges_after_many_rounds_and_compaction() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let deck = a.n("SELECT id FROM deck WHERE name = '2과목'");
    for i in 0..3 {
        a.note(deck, &format!("노트북 {i}"), None, 2000 + i);
        env.sync(&a);
    }
    // 데스크톱은 그동안 오프라인으로 따로 고쳤다
    let deck_b = b.n("SELECT id FROM deck WHERE name = '빅데이터분석기사'");
    b.note(deck_b, "데스크톱 오프라인", None, 2500);
    b.con().execute("UPDATE deck SET name = '빅분기 (오프라인)', updated_at = 2600 WHERE id = ?1", [deck_b]).unwrap();
    // 데스크톱을 기다리지 않고 정리했다 (90일 동안 안 쓴 기기): 스냅샷을 쓰고 옛 변경 파일을 지운다
    engine::write_snapshot(&a.db, &env.cx(&a), 1, json!({ "kind": "compact" })).unwrap();
    let a_uid: String = a.con().query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get(0)).unwrap();
    for p in env.files(&format!("changes/{a_uid}/")).into_iter().take(2) {
        env.store.delete(&p).unwrap();
    }
    env.sync(&b); // 스냅샷부터 받고 자기 변경을 합쳐 올린다
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(a.n("SELECT count(*) FROM note"), 5);
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '빅분기 (오프라인)'"), 1);
}

#[test]
fn compaction_deletes_only_files_every_device_has() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let deck = a.n("SELECT id FROM deck WHERE name = '2과목'");
    for i in 0..3 {
        a.note(deck, &format!("카드 {i}"), None, 2000 + i);
        env.sync(&a);
    }
    // 데스크톱은 아직 받지 않았다: 정리해도 지우지 않는다
    engine::write_snapshot(&a.db, &env.cx(&a), 1, json!({ "kind": "compact" })).unwrap();
    assert!(!env.files("changes/").is_empty());
    env.sync(&b);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn newer_format_is_not_applied() {
    use std::io::Write;
    let env = Env::new();
    let (_a, b, _) = pair(&env);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    writeln!(gz, "{}", json!({ "v": engine::FORMAT + 1, "gen": 1, "device": "zz", "seq": 1 })).unwrap();
    env.store.put("changes/zz/00000001.jsonl.gz", &gz.finish().unwrap()).unwrap();
    assert!(matches!(engine::pull(&b.db, &env.cx(&b)), Err(SyncError::NewerSchema)), "앱을 업데이트해 주세요");
}

#[test]
fn locked_drive_needs_the_key() {
    let env = Env::new();
    let a = Dev::new("노트북");
    let deck = a.deck("비밀 덱", None);
    a.note(deck, "암호로 잠근 카드", None, 1000);
    std::fs::write(a.dir.join("media/neo-3.png"), b"locked pic").unwrap();
    a.con().execute("INSERT INTO media (filename, rel_path, sha256, byte_size, created_at) VALUES ('neo-3.png', 'media/neo-3.png', '', 10, 1)", []).unwrap();
    let key = [7u8; 32];
    engine::join(&a.db, &env.cx_key(&a, Some(key)), Join::New, json!({})).unwrap();
    let snap = env.store.get(&env.files("snapshot/")[0]).unwrap();
    assert!(super::crypto::is_sealed(&snap), "Drive 에는 암호문만");
    let media = env.files("media/");
    assert!(media.len() == 1 && !media[0].contains('.'), "그림 이름도 평문 해시가 아니다 (HMAC)");
    let b = Dev::new("데스크톱");
    assert!(matches!(engine::join(&b.db, &env.cx(&b), Join::Take, json!({})), Err(SyncError::Locked)));
    assert!(matches!(engine::join(&b.db, &env.cx_key(&b, Some([8u8; 32])), Join::Take, json!({})), Err(SyncError::Locked)), "틀린 키");
    engine::join(&b.db, &env.cx_key(&b, Some(key)), Join::Take, json!({})).unwrap();
    assert_eq!(a.dump(), b.dump());
    assert_eq!(std::fs::read(b.dir.join("media/neo-3.png")).unwrap(), b"locked pic");
}

#[test]
fn backup_retention_and_names() {
    let day = db::DAY_MS;
    let now = 1_800_000_000_000;
    // 매일 하나씩 30일: 최근 7일은 하루 하나, 그 전은 주 하나, 최대 12개
    let list: Vec<(String, i64)> = (0..30).map(|i| (format!("b{i:02}"), now - i * day)).collect();
    let gone = engine::prune_backups(&list, now);
    let kept: Vec<&String> = list.iter().map(|(n, _)| n).filter(|n| !gone.contains(n)).collect();
    assert!(kept.len() <= 12);
    assert!((0..7).all(|i| kept.contains(&&format!("b{i:02}"))), "최근 7일은 모두");
    assert!(kept.len() > 7 && kept.len() < 30 - 7, "그 전은 주 하나");
    // 같은 날 두 개면 새 것만
    let gone = engine::prune_backups(&[("old".into(), now - 1000), ("new".into(), now)], now);
    assert_eq!(gone, ["old"]);
    assert!(engine::backup_time("neoanki-2026-10-14-0912.zip").is_some());
    assert!(engine::backup_time("neoanki-before-restore.zip").is_none());
}

#[test]
fn hlc_is_monotonic_and_ordered_by_string() {
    let a = engine::hlc(1000, 0, "a");
    assert!(engine::hlc(1000, 1, "a") > a && engine::hlc(1001, 0, "a") > engine::hlc(1000, 99, "z"));
    assert!(engine::hlc(1000, 0x1_0000, "a") > engine::hlc(1000, 0xffff, "a"), "카운터 자리가 넉넉하다");
    assert_eq!(engine::hlc_ms(&a), 1000);
    assert_eq!(engine::hlc_device(&a), "a");
}

#[test]
fn tables_added_by_other_features_sync_too() {
    // db::SYNC_TABLES 에 더한 표(시험 계획 · 원자료 · 서술형)는 표 목록과 자연 키를 스키마에서 읽어 따라간다
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let names: Vec<&str> = engine::tables(&a.con()).unwrap().iter().map(|t| t.name).collect();
    assert_eq!(names.len(), db::SYNC_TABLES.len());
    assert!(names.iter().position(|t| *t == "deck") < names.iter().position(|t| *t == "card"), "부모 먼저");
    let has = |t: &str| names.contains(&t);
    if has("plan_setting") {
        a.con().execute("INSERT INTO plan_setting (key, value) VALUES ('weekday_min', '60')", []).unwrap();
        b.con().execute("INSERT INTO plan_setting (key, value) VALUES ('weekday_min', '45')", []).unwrap();
    }
    if has("written_answer") {
        a.con().execute("INSERT INTO written_answer (card_id, answer, written_at) VALUES (?1, '노트북에서 쓴 답', 5000)", [card_a]).unwrap();
    }
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    if has("plan_setting") {
        assert_eq!(b.n("SELECT count(*) FROM plan_setting WHERE key = 'weekday_min'"), 1, "같은 키는 하나로");
    }
    if has("written_answer") {
        assert_eq!(b.n("SELECT count(*) FROM written_answer"), 1);
    }
}

#[test]
fn every_sync_table_reference_is_translatable() {
    // 다른 기능이 SYNC_TABLES 에 표를 더해도 외래 키가 조용히 깨지지 않게:
    //   동기화 대상 표를 가리키는 열은 uid 로 바꿔 보낼 수 있어야 하고 (대상의 정수 id 를 가리킨다),
    //   기기 안에서만 쓰는 표를 가리키는 열은 보내지 않으므로 다른 기기에서 비워 둘 수 있어야 한다 (NULL 허용)
    let con = db::open_in_memory().unwrap();
    let tables = engine::tables(&con).unwrap();
    for name in db::SYNC_TABLES {
        let t = tables.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} 이 동기화 표 목록에 없다"));
        let mut st = con.prepare(&format!("SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list('{name}')")).unwrap();
        let fks: Vec<(String, String, Option<String>)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
        for (col, target, to) in fks {
            if db::SYNC_TABLES.contains(&target.as_str()) {
                assert_eq!(t.fks.get(&col).map(|(t, _)| *t), Some(target.as_str()), "{name}.{col} -> {target}: uid 로 바꿔 보내야 한다");
                assert!(to.as_deref().is_none_or(|c| c == "id" || c == "rowid"), "{name}.{col} 는 {target} 의 정수 id 를 가리켜야 한다 ({to:?})");
            } else {
                let notnull: i64 = con.query_row(&format!("SELECT \"notnull\" FROM pragma_table_info('{name}') WHERE name = ?1"), [&col], |r| r.get(0)).unwrap();
                assert_eq!(notnull, 0, "{name}.{col} 는 기기 안 표 {target} 를 가리켜 보내지 않는다. 다른 기기에서 비워 둘 수 있어야 한다");
                assert!(!t.cols.contains(&col), "{name}.{col} 는 보내지 않는다");
            }
        }
    }
    // 학습 모드 열 (review_log.kind, 압축 회독) 같은 새 열도 그대로 따라간다
    let log = tables.iter().find(|t| t.name == "review_log").unwrap();
    let mut st = con.prepare("SELECT name FROM pragma_table_info('review_log') WHERE name NOT IN ('id', 'uid')").unwrap();
    for c in st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap) {
        assert!(log.cols.contains(&c) || log.fks.contains_key(&c), "review_log.{c}");
    }
}

#[test]
fn cram_review_kind_and_written_tables_round_trip() {
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let has_kind: i64 = a.n("SELECT count(*) FROM pragma_table_info('review_log') WHERE name = 'kind'");
    let note_a = a.n("SELECT id FROM note");
    if has_kind == 1 {
        a.con().execute("INSERT INTO review_log (card_id, reviewed_at, rating, fed_to_scheduler, kind) VALUES (?1, 5000, 3, 0, 'cram')", [card_a]).unwrap();
    }
    let has = |t: &str| a.n(&format!("SELECT count(*) FROM sqlite_master WHERE name = '{t}'")) == 1;
    if has("note_written") {
        a.con().execute("INSERT INTO note_written (note_id, write_mode, keywords, updated_at) VALUES (?1, 1, '[]', 5000)", [note_a]).unwrap();
    }
    env.sync(&a);
    env.sync(&b);
    assert_eq!(a.dump(), b.dump());
    if has_kind == 1 {
        assert_eq!(b.n("SELECT count(*) FROM review_log WHERE kind IS NOT NULL"), 1);
    }
    if has("note_written") {
        let note_b = b.n("SELECT id FROM note");
        assert_eq!(b.n(&format!("SELECT count(*) FROM note_written WHERE note_id = {note_b}")), 1, "참조는 그 기기의 노트 id 로");
    }
}

#[test]
fn plan_tables_merge_by_natural_key() {
    // plan_setting(key) · plan_conflict(deck_a, deck_b) 를 두 기기에서 따로 만들면 uid 가 달라도 같은 것이다:
    // 작은 uid 하나로 합치고 (별칭), 값은 열마다 나중 것
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let has = |t: &str| a.n(&format!("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '{t}'")) == 1;
    if !has("plan_setting") || !has("plan_conflict") {
        return;
    }
    for d in [&a, &b] {
        d.deck("시험 B", None);
    }
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    let ids = |d: &Dev| {
        let x = d.n("SELECT id FROM deck WHERE name = '빅데이터분석기사'");
        let y = d.n("SELECT id FROM deck WHERE name = '시험 B'");
        (x, y)
    };
    let (ax, ay) = ids(&a);
    let (bx, by) = ids(&b);
    a.con().execute("INSERT INTO plan_setting (key, value) VALUES ('weekday_min', '60')", []).unwrap();
    a.con().execute("INSERT INTO plan_conflict (deck_a, deck_b, mode, created_at) VALUES (?1, ?2, 'spread', 1000)", [ax, ay]).unwrap();
    b.con().execute("INSERT INTO plan_setting (key, value) VALUES ('weekday_min', '45')", []).unwrap();
    b.con().execute("INSERT INTO plan_conflict (deck_a, deck_b, mode, created_at) VALUES (?1, ?2, 'first', 2000)", [bx, by]).unwrap();
    env.sync(&a);
    env.sync(&b); // B 는 아직 올리지 않은 자기 값이 이긴다 (나중에 맞춘 쪽)
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    for d in [&a, &b] {
        assert_eq!(d.n("SELECT count(*) FROM plan_setting WHERE key = 'weekday_min'"), 1);
        assert_eq!(d.n("SELECT count(*) FROM plan_conflict"), 1);
        assert_eq!(d.con().query_row("SELECT value FROM plan_setting WHERE key = 'weekday_min'", [], |r| r.get::<_, String>(0)).unwrap(), "45");
        assert_eq!(d.con().query_row("SELECT mode FROM plan_conflict", [], |r| r.get::<_, String>(0)).unwrap(), "first");
    }
    // 그 뒤 한쪽에서 고치면 같은 행으로 간다 (옛 uid 로 와도 별칭을 따른다)
    a.con().execute("UPDATE plan_setting SET value = '90' WHERE key = 'weekday_min'", []).unwrap();
    env.sync(&a);
    env.sync(&b);
    assert_eq!(b.con().query_row("SELECT value FROM plan_setting WHERE key = 'weekday_min'", [], |r| r.get::<_, String>(0)).unwrap(), "90");
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn copied_data_folder_gets_its_own_device_id() {
    // 연결한 적 없는 복사본: 합류할 때 새 기기 id. 연결된 채로 복사한 것: 남의 변경 파일을 덮지 않고 멈춘다
    let env = Env::new();
    let (a, _b, _) = pair(&env);
    let copy_dir = tmp("copy");
    std::fs::create_dir_all(copy_dir.join("media")).unwrap();
    a.con().execute("VACUUM INTO ?1", [copy_dir.join("neoanki.sqlite").to_string_lossy()]).unwrap();
    let clone = Dev { dir: copy_dir.clone(), db: Mutex::new(db::open(&copy_dir.join("neoanki.sqlite")).unwrap()) };
    let id = |d: &Dev| d.con().query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get::<_, String>(0)).unwrap();
    assert_eq!(id(&clone), id(&a));
    // 연결된 채로 복사됐다: A 가 먼저 올린 번호를 복사본이 다시 쓰려 하면 멈춘다
    a.deck("A 의 새 덱", None);
    env.sync(&a);
    clone.deck("복사본의 새 덱", None);
    assert!(engine::push(&clone.db, &env.cx(&clone)).is_err(), "같은 번호의 파일을 덮지 않는다");
    // 끊고 다시 연결하면 새 id 로 합류한다
    super::forget_local(&clone.con()).unwrap();
    engine::join(&clone.db, &env.cx(&clone), Join::Merge, json!({})).unwrap();
    assert_ne!(id(&clone), id(&a));
    env.sync(&clone);
    env.sync(&a);
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '복사본의 새 덱'"), 1);
    assert_eq!(a.dump(), clone.dump());
}

#[test]
fn merging_two_copies_of_one_old_database_does_not_duplicate() {
    // 동기화 전에 DB 를 복사해 두 컴퓨터에서 따로 쓴 경우: uid 는 열 때 각자 만들어 모두 다르다.
    // 앱에서 만든 노트 · 복습 기록 · 세션도 (같은 ms, 같은 내용이면) 하나로 합친다.
    // uid 는 시각 순이라 어느 컴퓨터가 먼저 열었는지에 따라 합치는 쪽 uid 가 더 작을 수도 클 수도 있다: 둘 다 본다
    merge_old_copy("00");
    merge_old_copy("ff");
}

fn merge_old_copy(b_uid_prefix: &str) {
    let env = Env::new();
    let a = Dev::new("노트북");
    let deck = a.deck("직접 만든 덱", None);
    let (_, card) = a.note(deck, "앱에서 만든 카드", None, 1000);
    a.con().execute("INSERT INTO study_session (started_at, study_day, ended_at, end_reason) VALUES (5000, '2026-10-14', 6000, 'completed')", []).unwrap();
    a.review(&env.fsrs, card, 3, 5500);
    let copy_dir = tmp("old-copy");
    std::fs::create_dir_all(copy_dir.join("media")).unwrap();
    a.con().execute("VACUUM INTO ?1", [copy_dir.join("neoanki.sqlite").to_string_lossy()]).unwrap();
    let b = Dev { dir: copy_dir.clone(), db: Mutex::new(db::open(&copy_dir.join("neoanki.sqlite")).unwrap()) };
    for t in db::SYNC_TABLES {
        b.con().execute(&format!("UPDATE {t} SET uid = ?1 || lower(hex(randomblob(15)))"), [b_uid_prefix]).unwrap();
    }
    b.con().execute("UPDATE schema_meta SET value = lower(hex(randomblob(16))) WHERE key = 'device_uid'", []).unwrap();
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();
    engine::join(&b.db, &env.cx(&b), Join::Merge, json!({})).unwrap();
    for (t, n) in [("deck", 1), ("note", 1), ("card", 1), ("review_log", 1), ("study_session", 1)] {
        assert_eq!(b.n(&format!("SELECT count(*) FROM {t}")), n, "{t}");
    }
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump(), "합치는 쪽 uid 가 {b_uid_prefix}…");
}

#[test]
fn exam_marks_round_trip_and_tag_fill_converges() {
    // 8.4 기출: 사용자가 단 표시(이름 · 지우기)는 다른 기기로 간다.
    // 태그에서 채운 값은 올리지 않고(변경 기록 없음) 기기마다 같은 태그로 같은 값을 만든다
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let deck = a.n("SELECT id FROM deck WHERE name = '2과목'");
    let (marked, _) = a.note(deck, "직접 표시할 카드", None, 2000);
    let (tagged, _) = a.note(deck, "태그 카드", None, 2000);
    a.con().execute("UPDATE note SET tags = '기출::2023_중간' WHERE id = ?1", [tagged]).unwrap();
    env.sync(&a);
    env.sync(&b);
    // 두 기기가 각자 앱을 다시 열었다: 태그에서 채우고, 올릴 것은 생기지 않는다
    let before = a.outbox();
    for d in [&a, &b] {
        assert_eq!(crate::exam::fill_from_tags(&d.con()).unwrap(), 1);
    }
    assert_eq!(a.outbox(), before);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(b.n("SELECT count(*) FROM note WHERE exam_hits = 1 AND exam_names = '[\"2023 중간\"]'"), 1);
    // A 에서 표시하고, 태그로 채운 것은 지운다 → B 도 같고, B 가 다시 열어도 지운 것을 되살리지 않는다
    crate::exam::mark(&a.con(), "note", &[marked], Some("2024 기말")).unwrap();
    crate::exam::set(&a.con(), "note", &[crate::exam::ExamState { id: tagged, hits: None, names: vec![] }]).unwrap();
    env.sync(&a);
    env.sync(&b);
    assert_eq!(crate::exam::fill_from_tags(&b.con()).unwrap(), 0);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(b.n("SELECT count(*) FROM note WHERE exam_hits = 1 AND exam_names = '[\"2024 기말\"]'"), 1);
    assert_eq!(b.n("SELECT count(*) FROM note WHERE exam_hits = 0"), 1);
}
