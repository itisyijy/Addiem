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

    /// 고친 순간(sync_outbox.at)을 정해서 고친다. 누가 나중에 고쳤는지가 결과를 정하는 시험을 벽시계에 맡기지 않는다
    fn at<R>(&self, at: i64, f: impl FnOnce() -> R) -> R {
        let before = self.n("SELECT coalesce(max(seq), 0) FROM sync_outbox");
        let r = f();
        self.con().execute("UPDATE sync_outbox SET at = ?1 WHERE seq > ?2", params![at, before]).unwrap();
        r
    }

    fn edit_at(&self, at: i64, sql: &str, p: impl rusqlite::Params) {
        self.at(at, || self.con().execute(sql, p).unwrap());
    }

    fn me(&self) -> String {
        self.con().query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get(0)).unwrap()
    }

    fn fields(&self, note_uid: &str) -> String {
        self.con().query_row("SELECT fields FROM note WHERE uid = ?1", [note_uid], |r| r.get(0)).unwrap()
    }

    /// K7 기록 (이긴 기기, 이긴 시각, 이긴 내용, 진 기기, 진 시각, 진 내용)
    fn conflicts(&self) -> Vec<(String, i64, String, String, i64, String)> {
        let con = self.con();
        let mut st = con
            .prepare("SELECT winner_device, winner_at, winner_fields, loser_device, loser_at, loser_fields FROM sync_conflict ORDER BY id")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().map(Result::unwrap).collect()
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
    // 데스크톱은 노트북이 자기 복습을 받았는지 노트북의 다음 파일을 받고서야 안다 (K27): 한 번 더 다시 계산하지만 결과는 같다
    a.review(&env.fsrs, card_a, 3, t2 + 86_400_000);
    env.sync(&a);
    let rb = env.sync(&b);
    assert_eq!(rb.recomputed, 1);
    assert_eq!(b.srs(card_b), a.srs(card_a));
    assert_eq!(b.n("SELECT count(*) FROM sync_anchor"), 0, "서로 받은 것을 확인하면 앵커를 놓는다");
    // 그 뒤로 한 기기에서만 복습하면 그 기기 상태를 그대로
    a.review(&env.fsrs, card_a, 1, t2 + 2 * 86_400_000);
    env.sync(&a);
    let rb = env.sync(&b);
    assert_eq!(rb.recomputed, 0);
    assert_eq!(b.srs(card_b), a.srs(card_a));
}

#[test]
fn reviews_pushed_before_seeing_each_other_are_replayed() {
    // K12 빈틈: 두 기기가 받기를 마친 뒤 서로의 파일이 아직 없을 때 복습을 올렸다 (엇갈린 올리기).
    // 올리면서 앵커를 지우면 나중에 서로의 기록을 받아도 다시 계산하지 않고 한쪽 상태만 남는다.
    // 데스크톱은 받기 전에 한 번 더 복습한다. 복습하지 않은 세 번째 기기도 같은 상태가 돼야 한다
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let c = Dev::new("태블릿");
    engine::join(&c.db, &env.cx(&c), Join::Take, json!({})).unwrap();
    let uid = a.uid("card", card_a);
    let card_b = b.id("card", &uid).unwrap();
    let s0 = a.srs(card_a);
    let (t1, t2, t3) = (1_800_000_000_000, 1_800_000_060_000, 1_800_000_120_000);
    a.review(&env.fsrs, card_a, 3, t1);
    b.review(&env.fsrs, card_b, 1, t2);
    engine::push(&a.db, &env.cx(&a)).unwrap();
    engine::push(&b.db, &env.cx(&b)).unwrap();
    b.review(&env.fsrs, card_b, 3, t3);
    for _ in 0..3 {
        for d in [&a, &b, &c] {
            env.sync(d);
        }
    }
    let expected = [(3, t1), (1, t2), (3, t3)].iter().fold(s0, |s, (r, t)| srs::review(&env.fsrs, &s, *r, *t));
    for d in [&a, &b, &c] {
        assert_eq!(d.srs(d.id("card", &uid).unwrap()), expected, "{}: 세 기록을 시간 순으로", d.n("SELECT count(*) FROM review_log"));
    }
    assert_eq!(a.dump(), b.dump());
    assert_eq!(a.dump(), c.dump());
    assert_eq!(a.n("SELECT count(*) FROM sync_anchor") + b.n("SELECT count(*) FROM sync_anchor"), 0, "모두 받은 뒤에는 앵커를 놓는다");
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
fn draft_link_origin_round_trips() {
    // R1 초안에서 만든 카드의 연결(source_link.origin = 'draft')이 다른 기기로 가고, origin 만 바꾼 고치기도 따라간다
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let deck: i64 = a.n("SELECT id FROM deck WHERE parent_id IS NULL");
    let link = {
        let con = a.con();
        con.execute_batch(&format!(
            "INSERT INTO source_material (deck_id, file_name, rel_path, kind, status, created_at, updated_at) VALUES ({deck}, '3장.pdf', 'sources/1.pdf', 'pdf', 'ready', 0, 0);
             INSERT INTO source_segment (material_id, page, ord, text, signals, weight) VALUES (last_insert_rowid(), 13, 0, '잔차 분석', 'star', 2);
             INSERT INTO source_link (segment_id, card_id, method, origin, created_at) VALUES (last_insert_rowid(), {card_a}, 'manual', 'draft', 7);"
        ))
        .unwrap();
        con.last_insert_rowid()
    };
    env.sync(&a);
    env.sync(&b);
    assert_eq!(b.n("SELECT count(*) FROM source_link WHERE method = 'manual' AND origin = 'draft'"), 1);
    // B 에서 origin 만 지운다 (예: 다시 직접 이음)
    let on_b = b.id("source_link", &a.uid("source_link", link)).unwrap();
    b.con().execute("UPDATE source_link SET origin = NULL WHERE id = ?1", [on_b]).unwrap();
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.n("SELECT count(*) FROM source_link WHERE origin IS NULL"), 1);
    assert_eq!(a.dump(), b.dump());
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

// ---------------------------------------------------------------- QA 2026-10-16 T3 (K28–K33)

#[test]
fn later_edit_wins_even_when_it_syncs_first() {
    // T3-01: A 가 먼저, B 가 나중에 같은 노트를 고쳤다. B 가 먼저 맞추고 A 가 뒤에 맞춰도 "나중에 고친 쪽"(B)이 이긴다 (K28)
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let note = a.uid("note", a.n("SELECT id FROM note"));
    let t = db::now_ms();
    let set = "UPDATE note SET fields = ?2, updated_at = ?3 WHERE uid = ?1";
    let a1 = json!(["결측값 처리 방법?", "A 가 먼저 고침"]).to_string();
    let b1 = json!(["결측값 처리 방법?", "B 가 나중에 고침"]).to_string();
    a.edit_at(t + 1000, set, params![note, a1, t + 1000]);
    b.edit_at(t + 2000, set, params![note, b1, t + 2000]);
    env.sync(&b);
    env.sync(&a);
    env.sync(&b);
    assert_eq!(a.fields(&note), b1, "나중에 고친 쪽");
    assert_eq!(a.dump(), b.dump());
    // 내용이 바뀐 A 에 K7 기록. 시각은 맞춘 시각이 아니라 고친 순간 (T3-02 ③)
    assert_eq!(a.conflicts(), vec![(b.me(), t + 2000, b1.clone(), a.me(), t + 1000, a1.clone())]);
    // 먼저 고친 쪽이 먼저 맞춰도 같다. 이때는 두 기기 모두 진 쪽 내용을 본다
    let a2 = json!(["결측값 처리 방법?", "A 가 다시 먼저"]).to_string();
    let b2 = json!(["결측값 처리 방법?", "B 가 다시 나중에"]).to_string();
    a.edit_at(t + 3000, set, params![note, a2, t + 3000]);
    b.edit_at(t + 4000, set, params![note, b2, t + 4000]);
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.fields(&note), b2);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(a.conflicts().last().unwrap(), &(b.me(), t + 4000, b2.clone(), a.me(), t + 3000, a2.clone()));
    assert_eq!(b.conflicts().last().unwrap(), &(b.me(), t + 4000, b2.clone(), a.me(), t + 3000, a2.clone()));
}

#[test]
fn edit_after_receiving_wins_even_with_a_slow_clock() {
    // 시계가 5초 늦은 B 가 A 의 내용을 받은 뒤 고쳤다: 받은 HLC 보다 크게 매기므로 B 가 이긴다 (K28 시계 차이)
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let note = a.uid("note", a.n("SELECT id FROM note"));
    let t = db::now_ms();
    let x = json!(["Q", "A 가 고침"]).to_string();
    let y = json!(["Q", "B 가 받은 뒤 고침"]).to_string();
    a.edit_at(t + 5000, "UPDATE note SET fields = ?2 WHERE uid = ?1", params![note, x]);
    env.sync(&a);
    env.sync(&b);
    b.edit_at(t, "UPDATE note SET fields = ?2 WHERE uid = ?1", params![note, y]);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.fields(&note), y);
    assert_eq!(a.dump(), b.dump());
    assert!(a.conflicts().is_empty() && b.conflicts().is_empty(), "이어서 고친 것은 두 기기에서 고친 것이 아니다");
}

#[test]
fn first_merge_keeps_losing_versions_on_both_devices() {
    // T3-03: 처음 합칠 때 Drive 쪽이 이겨도 진 이 기기 버전을 K7 에 남긴다 (바탕 기록이 없으니 내용이 다르면 남긴다)
    // T3-04: 이 기기 쪽이 이겨 Drive 를 가진 기기의 내용이 바뀌면 그 기기에도 남는다. 연결을 끊어도 남는다
    // T3-05: 기기 이름은 "다른 기기 ↔ 다른 기기"가 아니라 실제 기기
    let env = Env::new();
    let a = Dev::new("노트북");
    let da = a.deck("공통", None);
    a.note(da, "질문 3 (A 가 먼저 고침)", Some("G3"), 3000);
    a.note(da, "질문 4 (A 가 나중에 고침)", Some("G4"), 4500);
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();
    let b = Dev::new("데스크톱");
    let dbk = b.deck("공통", None);
    b.note(dbk, "질문 3 (B 가 나중에 고침)", Some("G3"), 3500);
    b.note(dbk, "질문 4 (B 가 먼저 고침)", Some("G4"), 4000);
    let report = engine::join(&b.db, &env.cx(&b), Join::Merge, json!({})).unwrap();
    assert_eq!(report.conflicts, 2);
    let on_b = b.conflicts();
    let has = |list: &[(String, i64, String, String, i64, String)], win: &str, win_dev: &str, lose: &str, lose_dev: &str| {
        list.iter().any(|c| c.2.contains(win) && c.0 == win_dev && c.5.contains(lose) && c.3 == lose_dev)
    };
    assert!(has(&on_b, "질문 4 (A", &a.me(), "질문 4 (B", &b.me()), "Drive 쪽이 이겨도: {on_b:?}");
    assert!(has(&on_b, "질문 3 (B", &b.me(), "질문 3 (A", &a.me()), "{on_b:?}");
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    let on_a = a.conflicts();
    assert_eq!(on_a.len(), 1, "{on_a:?}");
    assert!(has(&on_a, "질문 3 (B", &b.me(), "질문 3 (A", &a.me()), "내용이 바뀐 A 에도: {on_a:?}");
    super::forget_local(&a.con()).unwrap();
    engine::ensure_tables(&a.con()).unwrap();
    assert_eq!(a.conflicts().len(), 1, "연결을 끊어도 30일 기록은 이 기기에 남는다");
}

#[test]
fn reviews_on_both_devices_before_first_merge_are_replayed() {
    // T3-06: 같은 .apkg 카드를 연결 전에 A 에서 Good, 나중에 B 에서 Again. 합치면 두 기록을 시간 순으로 돌린 상태가 된다
    let env = Env::new();
    let (t1, t2) = (1_800_000_000_000, 1_800_000_004_000);
    let a = Dev::new("노트북");
    let da = a.deck("공통", None);
    let (_, ca) = a.note(da, "공유 질문 1", Some("G1"), 1000);
    let s0 = a.srs(ca);
    a.review(&env.fsrs, ca, 3, t1);
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();
    let b = Dev::new("데스크톱");
    let dbk = b.deck("공통", None);
    let (_, cb) = b.note(dbk, "공유 질문 1", Some("G1"), 1000);
    b.review(&env.fsrs, cb, 1, t2);
    let report = engine::join(&b.db, &env.cx(&b), Join::Merge, json!({})).unwrap();
    let expected = srs::review(&env.fsrs, &srs::review(&env.fsrs, &s0, 3, t1), 1, t2);
    assert_eq!(report.recomputed, 1);
    assert_eq!(b.srs(b.n("SELECT id FROM card")), expected, "A 의 Good 다음에 B 의 Again");
    assert_eq!(b.n("SELECT count(*) FROM sync_anchor"), 0, "다시 계산하기 전 상태를 공통 시점으로 남기지 않는다");
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.srs(a.n("SELECT id FROM card")), expected);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn deleting_a_card_loses_to_a_later_edit_of_its_note() {
    // T3-07: A 가 카드를 지웠고(노트의 마지막 카드라 노트도), B 가 그 뒤에 노트를 고쳤다.
    // 노트만 되살아나 카드 없는 노트가 되지 않고 카드도 남는다. 지운 기기에서 비었던 복습 기록의 카드 참조도 돌아온다 (K29)
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let card = a.uid("card", card_a);
    let note = a.uid("note", a.n("SELECT id FROM note"));
    a.review(&env.fsrs, card_a, 3, 1_800_000_000_000);
    env.sync(&a);
    env.sync(&b);
    let t = db::now_ms();
    a.edit_at(t + 1000, "DELETE FROM card WHERE uid = ?1", [&card]);
    a.edit_at(t + 1000, "DELETE FROM note WHERE uid = ?1", [&note]);
    let edited = json!(["결측값 처리 방법?", "지운 뒤 B 가 고침"]).to_string();
    b.edit_at(t + 2000, "UPDATE note SET fields = ?2, updated_at = ?3 WHERE uid = ?1", params![note, edited, t + 2000]);
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert!(d.id("card", &card).is_some(), "카드도 남는다");
        assert_eq!(d.fields(&note), edited);
        assert_eq!(d.n("SELECT count(*) FROM review_log WHERE card_id IS NULL"), 0, "복습 기록이 다시 카드를 가리킨다");
    }
    assert_eq!(a.dump(), b.dump());
    // 지운 쪽이 나중이면 둘 다 지운다
    a.edit_at(t + 5000, "DELETE FROM card WHERE uid = ?1", [&card]);
    a.edit_at(t + 5000, "DELETE FROM note WHERE uid = ?1", [&note]);
    env.sync(&a);
    env.sync(&b);
    assert!(b.id("note", &note).is_none() && b.id("card", &card).is_none());
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn deleting_one_card_loses_to_a_later_edit_of_its_note() {
    // K29: 카드 한 장만 지웠어도(노트는 남음) 그 뒤에 다른 기기가 노트를 고쳤으면 카드를 남긴다. 노트를 고친 것은 그 카드를 고친 것이다.
    // 고친 것이 지운 것보다 먼저면(지운 기기가 이미 받았으면) 지운다
    let env = Env::new();
    let (a, b, card_a) = pair(&env);
    let card = a.uid("card", card_a);
    let note = a.uid("note", a.n("SELECT id FROM note"));
    let t = db::now_ms();
    a.edit_at(t + 1000, "DELETE FROM card WHERE uid = ?1", [&card]);
    b.edit_at(t + 2000, "UPDATE note SET tags = 'b' WHERE uid = ?1", [&note]);
    for d in [&a, &b, &a] {
        env.sync(d);
    }
    assert!(a.id("card", &card).is_some() && b.id("card", &card).is_some());
    assert_eq!(a.dump(), b.dump());
    a.edit_at(t + 3000, "DELETE FROM card WHERE uid = ?1", [&card]);
    env.sync(&a);
    env.sync(&b);
    assert!(b.id("card", &card).is_none(), "노트를 고친 것을 받은 뒤에 지웠다");
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn deleting_a_deck_loses_to_a_card_added_later_on_another_device() {
    // T3-08: A 가 덱을 지웠고(카드 · 카드 없는 노트 · 덱 설정까지), B 가 그 뒤에 그 덱에 카드를 넣었다.
    // 덱 · 덱 설정 · 새 카드는 남고 A 가 지운 옛 카드는 지워진 상태로, 세 기기가 모두 같아진다 (K29).
    // B 가 A 의 지우기를 받기 전에 새 카드를 먼저 올렸어도 (A 는 덱이 없어 그 카드를 건너뛴다) 같다
    for b_first in [false, true] {
        let env = Env::new();
        let (a, b, card_a) = pair(&env);
        let c = Dev::new("태블릿");
        engine::join(&c.db, &env.cx(&c), Join::Take, json!({})).unwrap();
        let sub = a.uid("deck", a.n("SELECT id FROM deck WHERE name = '2과목'"));
        a.con().execute("INSERT INTO deck_config (deck_id, daily_new_limit, updated_at) SELECT id, 7, 1000 FROM deck WHERE uid = ?1", [&sub]).unwrap();
        for d in [&a, &b, &c] {
            env.sync(d);
        }
        let old_card = a.uid("card", card_a);
        let t = db::now_ms();
        a.at(t + 1000, || {
            let con = a.con();
            con.execute("DELETE FROM card WHERE deck_id = (SELECT id FROM deck WHERE uid = ?1)", [&sub]).unwrap();
            con.execute("DELETE FROM note WHERE NOT EXISTS (SELECT 1 FROM card WHERE card.note_id = note.id)", []).unwrap();
            con.execute("DELETE FROM deck WHERE uid = ?1", [&sub]).unwrap();
        });
        let sub_b = b.id("deck", &sub).unwrap();
        let (_, new_card) = b.at(t + 2000, || b.note(sub_b, "덱을 지운 뒤 B 가 넣은 카드", None, t + 2000));
        let new_card = b.uid("card", new_card);
        if b_first {
            env.sync(&b);
        }
        for d in [&a, &b, &c, &a, &b, &c] {
            env.sync(d);
        }
        for d in [&a, &b, &c] {
            let who = d.n("SELECT count(*) FROM deck");
            assert!(d.id("deck", &sub).is_some(), "덱이 남는다 (b_first={b_first}, 덱 {who}개)");
            assert!(d.id("card", &new_card).is_some(), "새 카드가 남는다 (b_first={b_first})");
            assert!(d.id("card", &old_card).is_none(), "지운 카드는 지운다");
            assert_eq!(d.n(&format!("SELECT count(*) FROM deck_config WHERE deck_id = (SELECT id FROM deck WHERE uid = '{sub}')")), 1, "덱 설정도 따라 남는다");
        }
        assert_eq!(a.dump(), b.dump(), "b_first={b_first}");
        assert_eq!(a.dump(), c.dump(), "b_first={b_first}");
    }
}

/// 빈칸 노트 하나와 카드 두 장 (c1, c2). (노트 id, c2 카드 id)
fn cloze_note(d: &Dev, deck: i64) -> (i64, i64) {
    let con = d.con();
    let nt: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Cloze' AND anki_model_id IS NULL", [], |r| r.get(0)).unwrap();
    con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, 1000, 1000)", params![nt, json!([CLOZE_BOTH, ""]).to_string()]).unwrap();
    let note = con.last_insert_rowid();
    for ord in 0..2 {
        con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, ?3, 1000, 1000)", params![note, deck, ord]).unwrap();
    }
    (note, con.last_insert_rowid())
}

const CLOZE_BOTH: &str = "{{c1::결측값}} 과 {{c2::이상값}}";

/// 앱이 노트를 저장하는 길 (빈칸 번호에 맞춰 카드를 만들고 지운다)
fn save_note(d: &Dev, at: i64, note: i64, text: &str) {
    d.at(at, || crate::commands::save_note_fields(&d.con(), note, &[text.to_string(), String::new()]).unwrap());
}

#[test]
fn removing_a_cloze_deletes_its_card_on_every_device() {
    // 빈칸 c2 를 지우면 노트 저장과 카드 지우기가 한 동작(같은 ms, 같은 HLC)이다. 같은 HLC 의 노트 수정은 지운 기기가 안 수정이라 카드를 붙잡지 않는다
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (note, c2) = cloze_note(&a, a.n("SELECT id FROM deck WHERE name = '2과목'"));
    env.sync(&a);
    env.sync(&b);
    let c2 = a.uid("card", c2);
    save_note(&a, db::now_ms() + 1000, note, "{{c1::결측값}} 과 이상값");
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    assert!(a.id("card", &c2).is_none() && b.id("card", &c2).is_none(), "지운 빈칸의 카드가 되살아나지 않는다");
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn recreated_card_replaces_the_deleted_one() {
    // 같은 노트 · 같은 ord 의 카드를 동기화하기 전에 지웠다 다시 만들었다: 빈칸 c2 를 지웠다 다시 넣기 / 카드를 지운 뒤 노트를 고쳐 다시 생기기 /
    // 지우기를 받은 다른 기기(C)가 다시 넣기 (B 는 둘을 한 번에 받는다).
    // 옛 카드의 지우기보다 나중에 만든 새 카드가 남고, 받는 기기도 옛 카드를 지운다. 새 카드를 만든 노트 저장은 옛 카드를 고친 것이 아니다.
    // 옛 카드의 복습 기록은 지운 기기에서처럼 카드 참조가 빈 채 남고(A4), 새 카드는 새 카드의 일정(새 카드)으로 시작한다
    for path in ["cloze", "delete_cards", "other_device"] {
        let env = Env::new();
        let (a, b, _) = pair(&env);
        let c = Dev::new("태블릿");
        engine::join(&c.db, &env.cx(&c), Join::Take, json!({})).unwrap();
        let (note, old) = cloze_note(&a, a.n("SELECT id FROM deck WHERE name = '2과목'"));
        a.review(&env.fsrs, old, 3, 1_800_000_000_000);
        for d in [&a, &b, &c] {
            env.sync(d);
        }
        let old = a.uid("card", old);
        let t = db::now_ms();
        let maker = match path {
            "cloze" => {
                save_note(&a, t + 1000, note, "{{c1::결측값}} 과 이상값");
                &a
            }
            "delete_cards" => {
                a.edit_at(t + 1000, "DELETE FROM card WHERE uid = ?1", [&old]);
                &a
            }
            _ => {
                save_note(&a, t + 1000, note, "{{c1::결측값}} 과 이상값");
                env.sync(&a);
                env.sync(&c);
                &c
            }
        };
        let note = maker.id("note", &a.uid("note", note)).unwrap();
        save_note(maker, t + 2000, note, CLOZE_BOTH);
        let new = maker.uid("card", maker.n(&format!("SELECT id FROM card WHERE note_id = {note} AND template_ord = 1")));
        assert_ne!(new, old, "{path}: 새 uid");
        for d in [&a, &c, &b, &a, &c, &b] {
            env.sync(d);
        }
        for d in [&a, &b, &c] {
            assert!(d.id("card", &old).is_none(), "{path}: 옛 카드는 지워진다");
            let id = d.id("card", &new).unwrap_or_else(|| panic!("{path}: 새 카드가 남는다"));
            assert_eq!(d.srs(id).state, "new", "{path}: 새 카드는 새 카드의 일정");
            assert_eq!(d.n("SELECT count(*) FROM review_log WHERE card_id IS NULL"), 1, "{path}: 옛 카드의 복습 기록은 카드 참조 없이 남는다");
        }
        assert_eq!(a.dump(), b.dump(), "{path}");
        assert_eq!(a.dump(), c.dump(), "{path}");
    }
}

#[test]
fn recreated_deck_with_the_same_name_replaces_the_deleted_one() {
    // 덱을 지우고 같은 이름으로 다시 만들었다 (지운 뒤 같은 .apkg 를 다시 가져오기 등) / 다른 덱 이름을 지운 덱 이름으로 바꿨다.
    // 받는 기기가 새 덱을 옛 덱으로 합친 뒤 함께 지우거나, 이름이 겹쳐 바꾸기를 버리면 안 된다
    for path in ["create", "rename"] {
        let env = Env::new();
        let (a, b, old_card) = pair(&env);
        let parent = a.n("SELECT id FROM deck WHERE name = '빅데이터분석기사'");
        let other = a.deck("임시", Some(parent));
        env.sync(&a);
        env.sync(&b);
        let old_card = a.uid("card", old_card);
        let old = a.uid("deck", a.n("SELECT id FROM deck WHERE name = '2과목'"));
        let t = db::now_ms();
        a.at(t + 1000, || {
            let con = a.con();
            con.execute("DELETE FROM card WHERE deck_id = (SELECT id FROM deck WHERE uid = ?1)", [&old]).unwrap();
            con.execute("DELETE FROM note WHERE NOT EXISTS (SELECT 1 FROM card WHERE card.note_id = note.id)", []).unwrap();
            con.execute("DELETE FROM deck WHERE uid = ?1", [&old]).unwrap();
        });
        let deck = if path == "create" {
            a.at(t + 2000, || a.deck("2과목", Some(parent)))
        } else {
            a.edit_at(t + 2000, "UPDATE deck SET name = '2과목' WHERE id = ?1", [other]);
            other
        };
        let (_, new_card) = a.at(t + 2000, || a.note(deck, "다시 가져온 카드", None, t + 2000));
        let new = a.uid("deck", deck);
        let new_card = a.uid("card", new_card);
        for d in [&a, &b, &a, &b] {
            env.sync(d);
        }
        for d in [&a, &b] {
            assert!(d.id("deck", &old).is_none() && d.id("card", &old_card).is_none(), "{path}: 옛 덱 · 카드는 지워진다");
            assert!(d.id("card", &new_card).is_some(), "{path}: 새 카드가 남는다");
            assert_eq!(d.n(&format!("SELECT count(*) FROM deck WHERE uid = '{new}' AND name = '2과목'")), 1, "{path}: 새 덱이 그 이름으로 남는다");
        }
        assert_eq!(a.dump(), b.dump(), "{path}");
    }
}

#[test]
fn recreated_card_merges_into_the_old_one_edited_later_elsewhere() {
    // 옛 카드를 다른 기기가 지우기보다 나중에 고쳤으면 옛 카드도 남는다 (K29). 같은 자리의 새 카드와는 한 장으로 합쳐진다 (작은 uid)
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (note, old) = cloze_note(&a, a.n("SELECT id FROM deck WHERE name = '2과목'"));
    env.sync(&a);
    env.sync(&b);
    let old = a.uid("card", old);
    let t = db::now_ms();
    save_note(&a, t + 1000, note, "{{c1::결측값}} 과 이상값");
    save_note(&a, t + 2000, note, CLOZE_BOTH);
    b.edit_at(t + 3000, "UPDATE card SET importance = 2, importance_source = 'user' WHERE uid = ?1", [&old]);
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert_eq!(d.n(&format!("SELECT count(*) FROM card WHERE template_ord = 1 AND note_id = (SELECT note_id FROM card WHERE uid = '{old}')")), 1, "한 장");
        assert_eq!(d.n(&format!("SELECT importance FROM card WHERE uid = '{old}'")), 2, "나중에 고친 것이 남는다");
    }
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn lock_change_after_pulling_keeps_unpulled_changes_and_media() {
    // T3-09: 잠금을 바꾸는 기기(B)는 먼저 받는다 (sync_set_lock). C 가 올리고 B 가 아직 받지 않은 그림 카드도 새 세대에 들어가,
    // 지난 세대 파일과 Drive 그림을 지운 뒤에도 남는다
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let c = Dev::new("태블릿");
    engine::join(&c.db, &env.cx(&c), Join::Take, json!({})).unwrap();
    let pic = b"L3 picture".to_vec();
    std::fs::write(c.dir.join("media/l3.png"), &pic).unwrap();
    c.con().execute("INSERT INTO media (filename, rel_path, sha256, byte_size, created_at) VALUES ('l3.png', 'media/l3.png', '', 10, 1)", []).unwrap();
    let deck = c.n("SELECT id FROM deck WHERE name = '2과목'");
    c.note(deck, "L3 잠금 중 그림 <img src=\"l3.png\">", None, 2000);
    env.sync(&c);
    env.sync(&a);
    // B: 받기 -> (잠그기: Drive 그림을 지우고) 새 세대 스냅샷 -> 지난 세대 파일 지우기. mod.rs sync_set_lock 순서
    let key = [9u8; 32];
    assert!(matches!(engine::pull(&b.db, &env.cx(&b)).unwrap(), Pulled::Done(_)));
    let before = engine::remote(&env.store).unwrap();
    for p in env.files("media/") {
        env.store.delete(&p).unwrap();
    }
    engine::join(&b.db, &env.cx_key(&b, Some(key)), Join::Fresh, json!({ "kind": "lock" })).unwrap();
    engine::drop_old_generation(&env.store, 1, &before).unwrap();
    // A 는 고르는 창에서 [Drive 데이터로 바꾸기]를 골라도 C 의 카드와 그림을 잃지 않는다
    std::fs::remove_dir_all(a.dir.join("media")).unwrap();
    engine::join(&a.db, &env.cx_key(&a, Some(key)), Join::Take, json!({})).unwrap();
    assert_eq!(a.n("SELECT count(*) FROM note WHERE fields LIKE '%L3 잠금 중 그림%'"), 1);
    assert_eq!(std::fs::read(a.dir.join("media/l3.png")).unwrap(), pic);
    assert!(env.files("changes/").is_empty(), "지난 세대 변경 파일은 지웠다");
}

#[test]
fn restarting_a_wiped_drive_uses_a_newer_generation() {
    // T3-10: 다른 기기가 Drive 에서 지운 뒤 "이 기기 데이터로 다시 시작"은 본 적 있는 어떤 세대보다 큰 세대로 시작한다.
    // 다른 기기는 지난 세대로 보고 버리지 않고 고르는 창을 본다. 잠근 기기가 늦게 올린 암호 파일에 갇히지도 않는다
    let env = Env::new();
    let (a, b, _) = pair(&env);
    engine::join(&b.db, &env.cx(&b), Join::Fresh, json!({ "kind": "fresh" })).unwrap();
    assert!(matches!(engine::pull(&a.db, &env.cx(&a)).unwrap(), Pulled::Choice(_)));
    engine::join(&a.db, &env.cx(&a), Join::Take, json!({})).unwrap();
    for e in env.store.list("").unwrap() {
        env.store.delete(&e.path).unwrap();
    }
    assert!(matches!(engine::pull(&a.db, &env.cx(&a)).unwrap(), Pulled::Gone));
    engine::join(&a.db, &env.cx(&a), Join::Fresh, json!({ "kind": "fresh" })).unwrap();
    assert!(env.files("snapshot/")[0].starts_with("snapshot/g0003-"), "본 세대(2)보다 크게: {:?}", env.files("snapshot/"));
    match engine::pull(&b.db, &env.cx(&b)).unwrap() {
        Pulled::Choice(ch) => assert_eq!(ch.generation, 3),
        _ => panic!("다시 시작한 Drive 를 지난 세대로 보고 버렸다"),
    }
    // 잠근 기기가 지우기 전 세대로 늦게 올린 암호문: 잠기지 않은 Drive 에서는 건너뛴다
    plant(&env, 1, json!([{ "t": "deck", "u": "dz", "op": "up", "h": H, "r": { "name": "잠근 덱", "created_at": 1, "updated_at": 1 } }]), Some([7u8; 32]));
    assert!(matches!(engine::pull(&a.db, &env.cx(&a)).unwrap(), Pulled::Done(_)), "암호로 잠겨 있어요에 갇히지 않는다");
    // 이 기기 세대보다 낮은 세대만 남은 Drive (옛 앱이 다시 시작했다)도 고르는 창
    let c = Dev::new("태블릿");
    engine::join(&c.db, &env.cx(&c), Join::Take, json!({})).unwrap();
    c.con().execute("UPDATE sync_local SET value = '9' WHERE key = 'gen'", []).unwrap();
    assert!(matches!(engine::pull(&c.db, &env.cx(&c)).unwrap(), Pulled::Choice(ch) if ch.generation == 3));
}

#[test]
fn pending_count_includes_card_only_changes() {
    // T3-13: 카드 표만 바뀐 변경(중요도 · 접기 · 덱 이동)도 전송 대기로 센다. 복습으로 바뀐 카드는 복습 묶음으로 센다
    let env = Env::new();
    let (a, _b, card) = pair(&env);
    a.con().execute("UPDATE card SET importance = 2 WHERE id = ?1", [card]).unwrap();
    a.con().execute("UPDATE card SET buried_until = 9999999999999, buried_reason = 'user' WHERE id = ?1", [card]).unwrap();
    assert_eq!(super::pending_counts(&a.con()), (1, 0, 1), "카드 한 장의 변경 1건");
    a.review(&env.fsrs, card, 3, 1_800_000_000_000);
    let (pending, reviews, _) = super::pending_counts(&a.con());
    assert_eq!((pending, reviews), (1, 1), "복습한 카드의 변경은 복습 묶음으로 센다");
}

/// 다른 기기(zz)가 올린 것처럼 변경 파일 하나를 써 넣는다 (변조된 Drive 흉내)
fn plant(env: &Env, seq: i64, recs: J, key: Option<[u8; 32]>) {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    writeln!(gz, "{}", json!({ "v": engine::FORMAT, "gen": 1, "device": "zz", "seq": seq })).unwrap();
    for r in recs.as_array().unwrap() {
        writeln!(gz, "{r}").unwrap();
    }
    env.store.put(&format!("changes/zz/{seq:08}.jsonl.gz"), &engine::seal_opt(gz.finish().unwrap(), key.as_ref())).unwrap();
}

const H: &str = "9999999999999-00000000-zz";

#[test]
fn tampered_paths_are_not_applied() {
    // 받은 그림 이름 · 자료 경로가 이 기기의 다른 파일을 가리키면 행을 받지 않는다 (올리거나 지울 때 그 파일을 건드린다)
    let env = Env::new();
    let (_a, b, _) = pair(&env);
    let deck = b.uid("deck", b.n("SELECT id FROM deck WHERE parent_id IS NULL"));
    let media = |u: &str, name: &str, rel: &str, sha: &str| json!({ "t": "media", "u": u, "op": "up", "h": H, "r": { "filename": name, "rel_path": rel, "sha256": sha, "byte_size": 1, "created_at": 1 } });
    plant(
        &env,
        1,
        json!([
            media("m1", r"C:\Users\me\secret.kdbx", "media/x.png", ""),
            media("m2", "ok.png", "media/../../secret.kdbx", ""),
            media("m3", "ok2.png", "media/ok2.png", "../../../../secret"),
            media("m4", "fine.png", "media/fine.png", ""),
            { "t": "source_material", "u": "s1", "op": "up", "h": H, "r": { "deck_id": deck, "file_name": "a.pdf", "rel_path": "sources/../../secret.pdf", "kind": "pdf", "created_at": 1, "updated_at": 1 } },
        ]),
        None,
    );
    env.sync(&b);
    assert_eq!(b.n("SELECT count(*) FROM media"), 1, "멀쩡한 그림만");
    assert_eq!(b.n("SELECT count(*) FROM media WHERE filename = 'fine.png'"), 1);
    if b.n("SELECT count(*) FROM sqlite_master WHERE name = 'source_material'") == 1 {
        assert_eq!(b.n("SELECT count(*) FROM source_material"), 0);
    }
    // 이미 있는 행을 고치는 변경도 같다
    plant(&env, 2, json!([media("m4", r"..\..\secret.kdbx", "media/fine.png", "")]), None);
    env.sync(&b);
    assert_eq!(b.n("SELECT count(*) FROM media WHERE filename = 'fine.png'"), 1);
}

#[test]
fn locked_drive_ignores_plaintext_files() {
    // 잠근 Drive 에 평문 변경 파일이 있으면 받지 않고 건너뛴다 (멈추지도 않는다). 암호문은 받는다
    let env = Env::new();
    let key = [7u8; 32];
    let a = Dev::new("노트북");
    let cx = env.cx_key(&a, Some(key));
    engine::join(&a.db, &cx, Join::New, json!({})).unwrap();
    let deck = |name: &str| json!([{ "t": "deck", "u": format!("d-{name}"), "op": "up", "h": H, "r": { "name": name, "created_at": 1, "updated_at": 1 } }]);
    plant(&env, 1, deck("평문 덱"), None);
    plant(&env, 2, deck("잠근 덱"), Some(key));
    assert!(matches!(engine::pull(&a.db, &cx).unwrap(), Pulled::Done(_)));
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '평문 덱'"), 0);
    assert_eq!(a.n("SELECT count(*) FROM deck WHERE name = '잠근 덱'"), 1);
    assert!(matches!(engine::open_opt(b"plain".to_vec(), Some(&key)), Err(SyncError::Locked)));
}

#[test]
fn malformed_hlc_is_rejected() {
    // INF("~") 보다 큰 HLC 가 남으면 그 행은 어느 기기에서도 다시 고칠 수 없다. 파일을 읽지 않는다
    let env = Env::new();
    let (_a, b, _) = pair(&env);
    let rec = |h: &str| json!([{ "t": "deck", "u": "d1", "op": "up", "h": h, "r": { "name": "새 덱", "created_at": 1, "updated_at": 1 } }]);
    plant(&env, 1, rec("~~"), None);
    assert!(engine::pull(&b.db, &env.cx(&b)).is_err());
    assert_eq!(b.n("SELECT count(*) FROM deck WHERE name = '새 덱'"), 0);
    let file = |recs: J| {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        writeln!(gz, "{}", json!({ "v": 1 })).unwrap();
        writeln!(gz, "{}", recs[0]).unwrap();
        gz.finish().unwrap()
    };
    assert!(engine::decode(file(rec(H)), None).is_ok());
    assert!(engine::decode(file(rec("0")), None).is_ok(), "시계가 없던 행");
    assert!(engine::decode(file(rec("0000000001000-00000000-")), None).is_ok(), "동기화 전에 생긴 행");
    for bad in ["~", "1000", "0000000001000-0000000g-a", "0000000001000-00000000-a/b"] {
        assert!(engine::decode(file(rec(bad)), None).is_err(), "{bad}");
    }
}

#[test]
fn folder_store_rejects_paths_outside_root() {
    let env = Env::new();
    for bad in [r"changes\..\..\x", "C:/Windows/win.ini", "media/../x", "/etc/passwd"] {
        assert!(matches!(env.store.get(bad), Err(SyncError::Other(_))), "{bad}");
    }
}

#[test]
#[ignore]
fn bench_merge_copy() {
    // 측정: 데이터가 있는 복사본이 합류할 때 단계별 시간. cargo test bench_merge_copy -- --ignored --nocapture
    // BENCH_N (노트 수, 기본 3000) · BENCH_OLD (uid 가 다른 옛 복사본) · BENCH_DIFF (이 기기에만 있는 노트)
    use std::time::Instant;
    let n: i64 = std::env::var("BENCH_N").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let env = Env::new();
    let a = Dev::new("노트북");
    let deck = a.deck("큰 덱", None);
    {
        let mut con = a.con();
        let tx = con.transaction().unwrap();
        let nt: i64 = tx.query_row("SELECT id FROM note_type WHERE name = 'Basic' AND anki_model_id IS NULL", [], |r| r.get(0)).unwrap();
        for i in 0..n {
            tx.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", params![nt, json!([format!("질문 {i}"), "답"]).to_string(), 1000 + i]).unwrap();
            let note = tx.last_insert_rowid();
            tx.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, 0, 1000, 1000)", params![note, deck]).unwrap();
            let card = tx.last_insert_rowid();
            for j in 0..5 {
                tx.execute("INSERT INTO review_log (card_id, reviewed_at, rating, fed_to_scheduler) VALUES (?1, ?2, 3, 1)", params![card, 5000 + i * 10 + j]).unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let copy_dir = tmp("bench-copy");
    std::fs::create_dir_all(copy_dir.join("media")).unwrap();
    a.con().execute("VACUUM INTO ?1", [copy_dir.join("neoanki.sqlite").to_string_lossy()]).unwrap();
    let b = Dev { dir: copy_dir.clone(), db: Mutex::new(db::open(&copy_dir.join("neoanki.sqlite")).unwrap()) };
    if std::env::var("BENCH_DIFF").is_ok() {
        // 이 기기에만 있는 노트 (내용이 달라 같은 것으로 보지 않는다): 합친 뒤 모두 올린다
        b.con().execute("UPDATE note SET fields = json_array('B ' || id, '답')", []).unwrap();
    }
    if std::env::var("BENCH_OLD").is_ok() {
        for t in db::SYNC_TABLES {
            b.con().execute(&format!("UPDATE {t} SET uid = lower(hex(randomblob(16)))"), []).unwrap();
        }
    }
    let t = Instant::now();
    engine::join(&a.db, &env.cx(&a), Join::New, json!({})).unwrap();
    println!("A join New          {:>8.2?}", t.elapsed());
    env.sync(&a);
    let t = Instant::now();
    engine::join(&b.db, &env.cx(&b), Join::Merge, json!({})).unwrap();
    println!("B join Merge        {:>8.2?}  outbox {}", t.elapsed(), b.outbox());
    let t = Instant::now();
    engine::push(&b.db, &env.cx(&b)).unwrap();
    println!("B push              {:>8.2?}", t.elapsed());
    let t = Instant::now();
    env.sync(&a);
    println!("A sync              {:>8.2?}", t.elapsed());
}

// ---------------------------------------------------------------- 8.1 노트 유형 · 템플릿 (notetype.rs)

/// A 에서 앱 Basic 을 복제한 "내 유형" + 그 유형 노트 하나를 만들고 두 기기를 맞춘다. (A 의 유형 id, 노트 uid)
fn user_type(env: &Env, a: &Dev, b: &Dev) -> (i64, String) {
    use crate::notetype as nt;
    let (id, note) = {
        let con = a.con();
        let deck: i64 = con.query_row("SELECT id FROM deck WHERE name = '2과목'", [], |r| r.get(0)).unwrap();
        let id = nt::anki_clone(&con, "basic", Some("내 유형"));
        let values = vec!["질문".to_string(), "답".to_string()];
        nt::add_note(&con, &nt::TypedNote { deck_id: deck, note_type_id: id, values, importance: None }).unwrap();
        (id, con.last_insert_rowid())
    };
    let note = a.con().query_row("SELECT n.uid FROM note n JOIN card c ON c.note_id = n.id WHERE c.id = ?1", [note], |r| r.get(0)).unwrap();
    env.sync(a);
    env.sync(b);
    assert_eq!(a.dump(), b.dump());
    (id, note)
}

/// 이 기기에서 노트를 편집기처럼 고친다 (commands::save_note_fields, 자리 순서)
fn edit_note(d: &Dev, note_uid: &str, values: &[&str]) {
    let id = d.id("note", note_uid).unwrap();
    crate::commands::save_note_fields(&d.con(), id, &values.iter().map(|v| v.to_string()).collect::<Vec<_>>()).unwrap();
}

fn type_of(d: &Dev, note_uid: &str) -> crate::notetype::Nt {
    let id: i64 = d.con().query_row("SELECT note_type_id FROM note WHERE uid = ?1", [note_uid], |r| r.get(0)).unwrap();
    crate::notetype::load(&d.con(), id).unwrap()
}

#[test]
fn field_added_while_note_edited_elsewhere() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, note) = user_type(&env, &a, &b);
    crate::notetype::add_field(&a.con(), id, "예문").unwrap();
    edit_note(&b, &note, &["질문", "B 의 답"]);
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    for d in [&a, &b] {
        let t = type_of(d, &note);
        assert_eq!(t.slots, ["Front", "Back", "예문"]);
        // 짧은 배열(필드를 더하기 전 기기가 쓴 값)은 없는 칸이 빈 값이다. 자리가 밀리지 않는다
        assert_eq!(d.fields(&note), json!(["질문", "B 의 답"]).to_string());
        let map = t.field_map(&serde_json::from_str::<Vec<String>>(&d.fields(&note)).unwrap());
        assert_eq!((map["Back"].as_str(), map["예문"].as_str()), ("B 의 답", ""));
    }
    // 받은 뒤 새 칸을 채우면 그대로 따라간다
    edit_note(&b, &note, &["질문", "B 의 답", "예문 b"]);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.fields(&note), json!(["질문", "B 의 답", "예문 b"]).to_string());
}

#[test]
fn field_renamed_while_note_edited_elsewhere() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, note) = user_type(&env, &a, &b);
    crate::notetype::rename_field(&a.con(), id, 1, "뜻").unwrap();
    edit_note(&b, &note, &["질문", "B 의 답"]);
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    for d in [&a, &b] {
        let t = type_of(d, &note);
        assert_eq!((t.slots[1].as_str(), t.templates[0].afmt.as_str()), ("뜻", r#"{{FrontSide}}<hr id=answer><div class="nt-m nt-left">{{뜻}}</div>"#));
        let values: Vec<String> = serde_json::from_str(&d.fields(&note)).unwrap();
        let r = crate::render::render_card(&t.templates[0].qfmt, &t.templates[0].afmt, &t.field_map(&values), 0);
        assert_eq!(r.back, r#"<div class="nt-l nt-center">질문</div><hr id=answer><div class="nt-m nt-left">B 의 답</div>"#, "이름이 바뀌어도 값은 같은 자리");
    }
}

#[test]
fn template_added_while_note_edited_elsewhere() {
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, note) = user_type(&env, &a, &b);
    let added = crate::notetype::add_template(&a.con(), id, Some("거꾸로"), "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
    // 동기화 중에는 기기마다 다른 번호 (Q2-05): ord 1 이상, 2^16 으로 나눈 나머지가 이 기기 몫
    assert!(added.ord >= 1 && added.cards == 1, "{added:?}");
    edit_note(&b, &note, &["질문", "B 의 답"]);
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    for d in [&a, &b] {
        assert_eq!(d.fields(&note), json!(["질문", "B 의 답"]).to_string());
        let ords: String = d
            .con()
            .query_row("SELECT group_concat(template_ord) FROM (SELECT template_ord FROM card c JOIN note n ON n.id = c.note_id WHERE n.uid = ?1 ORDER BY 1)", [&note], |r| r.get(0))
            .unwrap();
        assert_eq!(ords, format!("0,{}", added.ord), "A 가 만든 카드가 B 에도");
        assert_eq!(type_of(d, &note).templates.len(), 2);
    }
    // B 에서 두 번째 카드의 앞면 필드를 지워도 카드는 남고(Anki 처럼 만들기만), 다시 맞춰진다
    edit_note(&b, &note, &["질문", "B 의 답 2"]);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn same_named_user_types_from_two_devices_stay_apart() {
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let make = |d: &Dev, front: &str| {
        let con = d.con();
        let deck: i64 = con.query_row("SELECT id FROM deck WHERE name = '2과목'", [], |r| r.get(0)).unwrap();
        let id = nt::anki_clone(&con, "basic", Some("내 유형"));
        let values = vec![front.to_string(), "답".to_string()];
        nt::add_note(&con, &nt::TypedNote { deck_id: deck, note_type_id: id, values, importance: None }).unwrap();
    };
    make(&a, "A 의 질문");
    // 모델 id 는 ms 시각이다. 두 기기가 같은 ms 에 만드는 일은 없다고 본다 (note-types.md 2장)
    std::thread::sleep(std::time::Duration::from_millis(3));
    make(&b, "B 의 질문");
    let b_type = b.n("SELECT max(id) FROM note_type");
    crate::notetype::add_field(&b.con(), b_type, "B 만의 필드").unwrap();
    env.sync(&a);
    env.sync(&b);
    env.sync(&a);
    assert_eq!(a.dump(), b.dump());
    for d in [&a, &b] {
        assert_eq!(d.n("SELECT count(*) FROM note_type WHERE name = '내 유형'"), 2, "이름이 같아도 합치지 않는다");
        assert_eq!(d.n("SELECT count(DISTINCT anki_model_id) FROM note_type WHERE name = '내 유형'"), 2);
        // 각 노트는 제 유형에 남는다: A 의 유형은 필드 둘, B 의 유형은 셋
        let fields_of = |front: &str| -> i64 {
            d.n(&format!("SELECT json_array_length(nt.fields) FROM note n JOIN note_type nt ON nt.id = n.note_type_id WHERE n.fields LIKE '%{front}%'"))
        };
        assert_eq!((fields_of("A 의 질문"), fields_of("B 의 질문")), (2, 3));
        assert_eq!(d.n("SELECT count(*) FROM note_type WHERE native_kind = 'basic'"), 1, "앱 유형은 한 벌");
    }
}

// ---------------------------------------------------------------- QA Q2 (8.1 노트 유형 ∥ 동기화)

/// 이 기기의 "2과목" 덱
fn sub_deck(d: &Dev) -> i64 {
    d.n("SELECT id FROM deck WHERE name = '2과목'")
}

/// 그 유형에 노트 하나 (보이는 순서 값). 노트 uid
fn typed_note(d: &Dev, type_id: i64, values: &[&str]) -> String {
    use crate::notetype as nt;
    let deck = sub_deck(d);
    let con = d.con();
    nt::add_note(&con, &nt::TypedNote { deck_id: deck, note_type_id: type_id, values: values.iter().map(|v| v.to_string()).collect(), importance: None }).unwrap();
    con.query_row("SELECT uid FROM note ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap()
}

/// 카드 종류(템플릿) 없는 카드: 학습 · 목록에 첫 필드로 보이고 내보내면 Anki 에서 빈 카드다
fn orphans(d: &Dev) -> i64 {
    d.n("SELECT count(*) FROM card c JOIN note n ON n.id = c.note_id
         WHERE NOT EXISTS (SELECT 1 FROM card_template t WHERE t.note_type_id = n.note_type_id AND (t.ord = c.template_ord OR t.qfmt LIKE '%{{cloze:%'))")
}

/// 그 노트의 카드 ord ("0,1")
fn ords_of(d: &Dev, note_uid: &str) -> String {
    d.con()
        .query_row(
            "SELECT coalesce(group_concat(template_ord), '') FROM (SELECT template_ord FROM card c JOIN note n ON n.id = c.note_id WHERE n.uid = ?1 ORDER BY 1)",
            [note_uid],
            |r| r.get(0),
        )
        .unwrap()
}

fn type_names(d: &Dev, type_uid: &str) -> Vec<String> {
    let con = d.con();
    let mut st = con.prepare("SELECT t.name FROM card_template t JOIN note_type nt ON nt.id = t.note_type_id WHERE nt.uid = ?1 ORDER BY t.ord").unwrap();
    st.query_map([type_uid], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

const REV_Q: &str = "{{Back}}";
const REV_A: &str = "{{FrontSide}}<hr id=answer>{{Front}}";

#[test]
fn type_deleted_while_note_added_elsewhere_keeps_its_card_types() {
    // Q2-01: A 가 노트 없는 유형을 지우는 사이 B 가 그 유형에 노트를 넣고 이름을 바꿨다.
    // 유형은 B 의 노트가 붙잡아 남고(K29), 유형의 일부인 카드 종류도 함께 남는다 (전에는 card_template 0개)
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let id = nt::anki_clone(&a.con(), "basic", Some("지울 유형"));
    nt::add_template(&a.con(), id, Some("거꾸로"), REV_Q, REV_A).unwrap();
    env.sync(&a);
    env.sync(&b);
    let ty = a.uid("note_type", id);
    let t = db::now_ms();
    a.at(t + 1000, || nt::delete_type(&a.con(), id).unwrap());
    let b_id = b.id("note_type", &ty).unwrap();
    b.at(t + 2000, || {
        typed_note(&b, b_id, &["B 가 넣은 노트", "답"]);
        nt::rename_type(&b.con(), b_id, "B 가 바꾼 이름").unwrap();
    });
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert!(d.id("note_type", &ty).is_some(), "유형이 남는다");
        assert_eq!(type_names(d, &ty).len(), 2, "카드 종류도 함께 남는다");
        assert_eq!(orphans(d), 0);
    }
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn card_type_deleted_while_note_edited_elsewhere_leaves_no_orphan() {
    // Q2-02: A 가 카드 종류를 지우는 사이 B 가 노트 하나를 고쳤다. 나중에 고친 노트의 카드는 남고(K29), 그 카드가 쓰는 카드 종류도 남는다.
    // 고치지 않은 노트의 그 카드는 지운다. 템플릿 없는 카드가 생기지 않는다
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, note) = user_type(&env, &a, &b);
    let rev = nt::add_template(&a.con(), id, Some("거꾸로"), REV_Q, REV_A).unwrap().ord;
    let other = typed_note(&a, id, &["그대로", "둔 노트"]);
    env.sync(&a);
    env.sync(&b);
    let ty = a.uid("note_type", id);
    let t = db::now_ms();
    a.at(t + 1000, || nt::delete_template(&a.con(), id, rev, false).unwrap());
    b.at(t + 2000, || edit_note(&b, &note, &["질문", "B 가 고친 답"]));
    for d in [&b, &a, &b, &a] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert_eq!(orphans(d), 0, "템플릿 없는 카드가 없다");
        assert_eq!(type_names(d, &ty).len(), 2, "남은 카드가 카드 종류를 붙잡는다");
        assert_eq!((ords_of(d, &note), ords_of(d, &other).as_str()), (format!("0,{rev}"), "0"));
    }
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn cloze_removed_in_the_same_edit_stays_removed() {
    // Q2-03: 빈칸 c2 를 지우면 같은 트랜잭션에서 노트도 고친다 (같은 HLC). 지운 기기가 스스로 고친 것에 지우기가 지지 않는다
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let note = {
        let con = a.con();
        let cz = db::native_type_id(&con, "cloze").unwrap();
        let deck: i64 = con.query_row("SELECT id FROM deck WHERE name = '2과목'", [], |r| r.get(0)).unwrap();
        con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, 1000, 1000)", params![cz, json!(["물은 {{c1::100}}도에서 {{c2::끓는다}}", ""]).to_string()])
            .unwrap();
        let n = con.last_insert_rowid();
        for ord in 0..2 {
            con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, ?3, 1000, 1000)", params![n, deck, ord]).unwrap();
        }
        n
    };
    let note = a.uid("note", note);
    env.sync(&a);
    env.sync(&b);
    let id = b.id("note", &note).unwrap();
    b.at(db::now_ms() + 1000, || crate::commands::save_note_fields(&b.con(), id, &["물은 {{c1::100}}도에서 끓는다".into(), String::new()]).unwrap());
    assert_eq!(ords_of(&b, &note), "0");
    for d in [&b, &a, &b, &a] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert_eq!(ords_of(d, &note), "0", "c2 카드는 두 기기 모두에서 지워진 채다");
    }
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn card_type_added_while_note_added_elsewhere() {
    // Q2-04: A 가 카드 종류를 더하는 사이 B 가 새 노트를 넣었다. 맞춘 뒤 새 노트에도 새 카드 종류의 카드가 있다.
    // 카드는 그 노트를 쓴 기기가 만든다 (그 카드 종류를 몰랐던 기기). 받은 쪽이 따로 만들어 두 장이 되지 않는다
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, _) = user_type(&env, &a, &b);
    let ty = a.uid("note_type", id);
    let b_id = b.id("note_type", &ty).unwrap();
    let rev = nt::add_template(&a.con(), id, Some("거꾸로"), REV_Q, REV_A).unwrap().ord;
    let both = format!("0,{rev}");
    let star = typed_note(&b, b_id, &["star", "별"]);
    // 엇갈려 맞춘다: A 는 카드 종류만 먼저 올리고, B 는 받지 않고 새 노트를 올린다
    let pull = |d: &Dev| assert!(matches!(engine::pull(&d.db, &env.cx(d)).unwrap(), Pulled::Done(_)));
    let push = |d: &Dev| engine::push(&d.db, &env.cx(d)).unwrap();
    push(&a);
    push(&b);
    pull(&a); // A: star 를 받았다. B 가 거꾸로를 알았는지 A 는 모른다 (만들지 않는다)
    assert_eq!(ords_of(&a, &star), "0");
    pull(&b); // B: 카드 종류를 받았다. star 는 B 가 그 카드 종류를 모르고 쓴 노트다: B 가 만든다
    assert_eq!(ords_of(&b, &star), both, "노트를 쓴 기기가 새 카드 종류의 카드를 만든다");
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert_eq!(ords_of(d, &star), both, "한 장");
        assert_eq!(orphans(d), 0);
    }
    // 평소처럼 맞춰도 (받기 -> 올리기) 같다: B 는 아직 올리지 않은 새 노트에 카드를 만들어 함께 올린다
    let rev2 = nt::add_template(&a.con(), id, Some("둘째"), "{{Back}}!", REV_A).unwrap().ord;
    let moon = typed_note(&b, b_id, &["moon", "달"]);
    for d in [&a, &b, &a] {
        env.sync(d);
    }
    for d in [&a, &b] {
        assert_eq!(ords_of(d, &moon), format!("0,{rev},{rev2}"));
    }
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn card_types_added_on_both_devices_both_stay() {
    // Q2-05: 같은 유형에 두 기기가 따로 카드 종류를 더했다. 기기마다 다른 ord 를 쓰므로 두 카드 종류와 그 카드가 모두 남는다
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, note) = user_type(&env, &a, &b);
    let ty = a.uid("note_type", id);
    let b_id = b.id("note_type", &ty).unwrap();
    // 기기 몫은 기기 uid 끝 4자리. 우연히 같으면(1/65536) 같은 ord 가 되어 예전처럼 한 행으로 합쳐진다 (K39 한계)
    let lane = |d: &Dev| d.me()[d.me().len() - 4..].to_string();
    if lane(&a) == lane(&b) {
        return;
    }
    nt::add_template(&a.con(), id, Some("역방향"), REV_Q, REV_A).unwrap();
    nt::add_template(&b.con(), b_id, Some("힌트 카드"), "힌트: {{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}").unwrap();
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    for d in [&a, &b] {
        let mut names = type_names(d, &ty);
        names.sort();
        assert_eq!(names, ["Card 1", "역방향", "힌트 카드"], "어느 쪽 정의도 사라지지 않는다");
        assert_eq!(ords_of(d, &note).split(',').count(), 3);
        assert_eq!(orphans(d), 0);
    }
    assert_eq!(a.dump(), b.dump());
}

#[test]
fn fields_added_on_both_devices_keep_values_apart() {
    // Q2-06: 같은 유형에 두 기기가 따로 필드를 더했다 (같은 자리 번호). A 는 뒷면에, B 는 앞면에 넣었다.
    // 두 필드가 모두 남고, A 노트의 값은 A 의 필드 아래 그대로 뒷면에만 보인다 (B 의 앞면에 나와 답이 보이지 않는다)
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, _) = user_type(&env, &a, &b);
    let ty = a.uid("note_type", id);
    let b_id = b.id("note_type", &ty).unwrap();
    let t = db::now_ms();
    let a_note = a.at(t + 1000, || {
        nt::add_field(&a.con(), id, "예문A").unwrap();
        let q = nt::load(&a.con(), id).unwrap().templates[0].clone();
        nt::save_template(&a.con(), id, 0, None, &q.qfmt, &format!("{}<br>{{{{예문A}}}}", q.afmt)).unwrap();
        typed_note(&a, id, &["A 의 질문", "A 의 답", "A 의 예문(답)"])
    });
    let b_note = b.at(t + 2000, || {
        nt::add_field(&b.con(), b_id, "힌트B").unwrap();
        let q = nt::load(&b.con(), b_id).unwrap().templates[0].clone();
        nt::save_template(&b.con(), b_id, 0, None, &format!("{}<br>{{{{힌트B}}}}", q.qfmt), &q.afmt).unwrap();
        typed_note(&b, b_id, &["B 의 질문", "B 의 답", "B 의 힌트"])
    });
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    assert_eq!(a.dump(), b.dump());
    for d in [&a, &b] {
        let t = type_of(d, &a_note);
        let mut names: Vec<String> = t.visible().into_iter().map(|(_, n)| n).collect();
        names.sort();
        assert_eq!(names, ["Back", "Front", "예문A", "힌트B"], "두 필드가 모두 남는다");
        let render = |note: &str| {
            let values: Vec<String> = serde_json::from_str(&d.fields(note)).unwrap();
            let map = t.field_map(&values);
            (crate::render::render_card(&t.templates[0].qfmt, &t.templates[0].afmt, &map, 0), map)
        };
        let (r, map) = render(&a_note);
        assert_eq!(map["예문A"], "A 의 예문(답)");
        assert!(!r.front.contains("A 의 예문"), "A 의 뒷면 값이 앞면에 나오지 않는다: {}", r.front);
        assert!(r.back.contains("A 의 예문"));
        let (r, map) = render(&b_note);
        assert_eq!(map["힌트B"], "B 의 힌트");
        assert!(r.front.contains("B 의 힌트"));
    }
}

#[test]
fn same_template_edited_on_both_devices_keeps_the_losing_version() {
    // Q2-05 · Q2-06 지는 쪽 기록 (K7 처럼): 두 기기가 같은 카드 종류의 앞면을 따로 고쳤다. 나중에 고친 쪽으로 맞추고 진 쪽 내용을 남긴다
    use crate::notetype as nt;
    let env = Env::new();
    let (a, b, _) = pair(&env);
    let (id, _) = user_type(&env, &a, &b);
    let ty = a.uid("note_type", id);
    let b_id = b.id("note_type", &ty).unwrap();
    let afmt = nt::load(&a.con(), id).unwrap().templates[0].afmt.clone();
    let t = db::now_ms();
    a.at(t + 1000, || nt::save_template(&a.con(), id, 0, None, "A 앞면 {{Front}}", &afmt).unwrap());
    b.at(t + 2000, || nt::save_template(&b.con(), b_id, 0, None, "B 앞면 {{Front}}", &afmt).unwrap());
    for d in [&a, &b, &a, &b] {
        env.sync(d);
    }
    assert_eq!(a.dump(), b.dump());
    let qfmt = |d: &Dev| -> String {
        d.con().query_row("SELECT qfmt FROM card_template WHERE note_type_id = (SELECT id FROM note_type WHERE uid = ?1)", [&ty], |r| r.get(0)).unwrap()
    };
    assert_eq!(qfmt(&a), "B 앞면 {{Front}}", "나중에 고친 쪽");
    let kept = |d: &Dev| d.n("SELECT count(*) FROM sync_type_conflict WHERE loser = 'A 앞면 {{Front}}' AND winner = 'B 앞면 {{Front}}'");
    assert!(kept(&a) + kept(&b) >= 1, "진 쪽 앞면을 기록에 남긴다");
}

#[test]
fn card_type_deleted_while_a_card_is_recreated_and_a_cloze_re_added_elsewhere() {
    // K37 · K40 섞임: A 가 사용자 유형의 카드 종류 하나를 지우는 사이, B 는 (1) 그 카드 종류의 카드 한 장을 지운 뒤 노트를 고쳐 다시 만들고(같은 노트 · ord 의 새 카드),
    // (2) 빈칸 노트의 c2 를 지웠다 다시 넣었다. B 의 새 카드는 A 의 지우기를 모르고 나중에 만든 것이라 남고 그 카드 종류를 붙잡는다(K37). 옛 카드와 합치지 않는다(K40).
    // 다른 노트의 그 카드 종류 카드는 지운다. 빈칸 노트에는 c1 · c2 가 한 장씩. 템플릿 없는 카드가 없고 두 기기가 같다
    use crate::notetype as nt;
    for b_first in [false, true] {
        let env = Env::new();
        let (a, b, _) = pair(&env);
        let (id, note) = user_type(&env, &a, &b);
        let rev = nt::add_template(&a.con(), id, Some("거꾸로"), REV_Q, REV_A).unwrap().ord;
        let other = typed_note(&a, id, &["그대로", "둔 노트"]);
        let (cz, _) = cloze_note(&a, sub_deck(&a));
        let cz = a.uid("note", cz);
        env.sync(&a);
        env.sync(&b);
        let ty = a.uid("note_type", id);
        let old_rev: String = b.con().query_row("SELECT c.uid FROM card c JOIN note n ON n.id = c.note_id WHERE n.uid = ?1 AND c.template_ord = ?2", params![note, rev], |r| r.get(0)).unwrap();
        let t = db::now_ms();
        a.at(t + 1000, || nt::delete_template(&a.con(), id, rev, false).unwrap());
        b.edit_at(t + 2000, "DELETE FROM card WHERE uid = ?1", [&old_rev]);
        b.at(t + 2500, || edit_note(&b, &note, &["질문", "B 가 고친 답"]));
        let bcz = b.id("note", &cz).unwrap();
        save_note(&b, t + 3000, bcz, "{{c1::결측값}} 과 이상값");
        save_note(&b, t + 3500, bcz, CLOZE_BOTH);
        let new_rev: String = b.con().query_row("SELECT c.uid FROM card c JOIN note n ON n.id = c.note_id WHERE n.uid = ?1 AND c.template_ord = ?2", params![note, rev], |r| r.get(0)).unwrap();
        assert_ne!(new_rev, old_rev, "노트를 고쳐 새 카드가 생겼다");
        let order = if b_first { [&b, &a, &b, &a] } else { [&a, &b, &a, &b] };
        for d in order {
            env.sync(d);
        }
        for d in [&a, &b] {
            assert_eq!(orphans(d), 0, "템플릿 없는 카드가 없다 (b_first={b_first})");
            assert_eq!(type_names(d, &ty).len(), 2, "새 카드가 카드 종류를 붙잡는다");
            assert!(d.id("card", &new_rev).is_some() && d.id("card", &old_rev).is_none(), "새 카드가 남고 옛 카드는 지운다");
            assert_eq!((ords_of(d, &note), ords_of(d, &other).as_str(), ords_of(d, &cz).as_str()), (format!("0,{rev}"), "0", "0,1"));
        }
        assert_eq!(a.dump(), b.dump(), "b_first={b_first}");
    }
}
