//! 동기화 엔진 (drive-sync.md 5장). Tauri 를 모른다: DB(Mutex<Connection>)와 Store 만 받는다.
//!
//! 기록 단위는 행이고 참조는 uid 다 (정수 id 는 기기 안에서만). 행마다 열 단위 HLC 를 sync_row 에 둔다.
//!   - 고칠 때: 트리거가 sync_outbox 에 벽시계 시각(at)을 남긴다. 그 변경의 HLC 는 "고친 순간"이다 (K28):
//!     hlc_next(그때까지 받은 가장 큰 HLC = floor, at). 받기 · 올리기 전에도 바뀌지 않는다
//!   - 올리기: sync_outbox 를 행으로 묶어 changes/<나>/<번호>.jsonl.gz 새 파일 하나로. 열마다 고친 순간의 HLC
//!   - 받기: 다른 기기의 새 파일을 모두 내려받아 한 트랜잭션(sync_flags.applying = 1)에서 합친다
//!   - 합치기: 열마다 큰 HLC 가 이긴다 = 나중에 고친 쪽 (아직 올리지 않은 이 기기 변경도 고친 순간의 HLC 로 겨룬다)
//!     삭제는 그 행 · 그 카드의 노트를 마지막으로 고친 것보다 나중일 때만 (K29). 복습 기록은 합집합. 두 기기에서 복습한 카드는 FSRS 를 다시 돌린다
//!   - 같은 것을 가리키는 두 행(같은 이름 경로의 덱, 같은 파일 이름의 그림, 기본 노트 유형 …)은 uid 가 작은 쪽으로 합친다 (sync_alias)

use super::crypto;
use crate::commands::{safe_media_name, safe_rel_path};
use super::store::{SRes, Store, SyncError, other};
use crate::db::{self, SYNC_TABLES};
use crate::srs::{self, SrsState};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 변경 파일 형식. Drive 에 더 큰 값이 있으면 받지 않는다 ("앱을 업데이트해 주세요").
/// K28 의 열마다 다른 HLC(k)는 옛 앱도 읽는 필드라 올리지 않았다
pub const FORMAT: i64 = 1;
/// sync_outbox.cols 의 표시 (엔진이 넣는 기록). 옛 앱은 넣기로만 읽는다 (모든 열)
/// 지금 열 HLC 그대로 다시 보낸다: 처음 합친 결과 · 다른 기기에 없을 수 있는 행 (새 HLC 를 매기지 않는다)
const RESEND: &str = "*";
/// 지우기를 받지 않고 남긴 행을 모든 열로 다시 보낸다. HLC 는 그 지우기 바로 다음 (floor = 지운 HLC, at = 0)
const REVIVE: &str = "+";
/// 정리 (5.5)
const COMPACT_FILES: usize = 200;
const COMPACT_MS: i64 = 30 * db::DAY_MS;
pub const STALE_MS: i64 = 90 * db::DAY_MS;
/// 카드의 FSRS 상태 열 (두 기기에서 복습하면 다시 계산하는 열)
const SRS_COLS: [&str; 9] = ["state", "srs_step", "stability", "difficulty", "due_at", "interval_days", "last_reviewed_at", "reps", "lapses"];

fn dbe(e: rusqlite::Error) -> SyncError {
    other(e)
}

// ---------------------------------------------------------------- HLC

/// (물리 시각 ms, 카운터, 기기). 문자열 비교가 곧 크기 비교다
pub fn hlc(phys: i64, counter: u32, dev: &str) -> String {
    format!("{:013}-{counter:08x}-{dev}", phys.max(0))
}

fn hlc_split(h: &str) -> (i64, u32) {
    let mut it = h.splitn(3, '-');
    let p = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let c = it.next().and_then(|s| u32::from_str_radix(s, 16).ok()).unwrap_or(0);
    (p, c)
}

pub fn hlc_ms(h: &str) -> i64 {
    hlc_split(h).0
}

pub fn hlc_device(h: &str) -> &str {
    h.splitn(3, '-').nth(2).unwrap_or("")
}

/// 고친 순간의 HLC: 벽시계 at, 단 그때까지 받은 가장 큰 값(last)보다 항상 크게.
/// 시계가 늦은 기기도 받은 값을 고치면 그 값을 이긴다 (K28)
fn hlc_next(last: &str, at: i64, dev: &str) -> String {
    let (p, c) = hlc_split(last);
    if at > p { hlc(at, 0, dev) } else { hlc(p, c + 1, dev) }
}

// ---------------------------------------------------------------- 이 기기 상태

/// 동기화 전용 로컬 테이블. Drive 로 가지 않는다 (스냅샷은 SYNC_TABLES 만 담는다). 있으면 아무것도 바꾸지 않는다
pub fn ensure_tables(con: &Connection) -> SRes<()> {
    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_local (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         -- 행마다 HLC: hlc = 행 기본값, cols = 열마다 다른 값 (JSON), deleted = 지운 HLC (흔적)
         CREATE TABLE IF NOT EXISTS sync_row (table_name TEXT NOT NULL, uid TEXT NOT NULL, hlc TEXT NOT NULL, cols TEXT, deleted TEXT, PRIMARY KEY (table_name, uid));
         -- 같은 것을 가리키는 두 uid. 작은 쪽이 남는다
         CREATE TABLE IF NOT EXISTS sync_alias (table_name TEXT NOT NULL, from_uid TEXT NOT NULL, to_uid TEXT NOT NULL, PRIMARY KEY (table_name, from_uid));
         -- uid 를 바꿀 때 그 uid 로 가던 별칭을 찾는다 (없으면 처음 합칠 때 행마다 표 전체를 훑는다)
         CREATE INDEX IF NOT EXISTS idx_sync_alias_to ON sync_alias (table_name, to_uid);
         -- 다른 기기가 받았다고 확인하지 못한 첫 복습 전의 카드 상태 (마지막 공통 시점, K27). log_id 이후의 기록을 다시 돌린다. 올리면 [10] 에 번호
         CREATE TABLE IF NOT EXISTS sync_anchor (card_id INTEGER PRIMARY KEY, srs TEXT NOT NULL);
         -- K7 두 기기에서 고친 카드. 진 쪽 내용을 남긴다 (30일)
         CREATE TABLE IF NOT EXISTS sync_conflict (
             id INTEGER PRIMARY KEY, note_uid TEXT NOT NULL, at INTEGER NOT NULL,
             winner_device TEXT, winner_at INTEGER, winner_fields TEXT,
             loser_device TEXT, loser_at INTEGER, loser_fields TEXT,
             seen INTEGER NOT NULL DEFAULT 0, resolved INTEGER NOT NULL DEFAULT 0);
         -- K39 두 기기에서 고친 노트 유형 정의 (카드 종류 앞 · 뒷면 · 이름, CSS, 필드 이름, 같은 자리에 더한 필드). 진 쪽을 남긴다 (30일)
         CREATE TABLE IF NOT EXISTS sync_type_conflict (
             id INTEGER PRIMARY KEY, type_uid TEXT NOT NULL, what TEXT NOT NULL, at INTEGER NOT NULL,
             winner_device TEXT, winner TEXT, loser_device TEXT, loser TEXT, seen INTEGER NOT NULL DEFAULT 0);",
    )
    .map_err(dbe)?;
    // 변경 기록의 바뀐 열 목록, 고친 순간에 받아 둔 가장 큰 HLC (K28. 비어 있으면 지금의 hlc_last 가 그 값이다: bump_hlc_last 가 올리기 전에 채운다).
    // K7 기록의 버전마다 노트 유형 (K41: 다른 유형으로 옮기는 사이 고친 내용은 옛 유형 자리 배열이다. NULL = 이 기능 전 기록)
    for (table, col) in [("sync_outbox", "cols"), ("sync_outbox", "floor"), ("sync_conflict", "winner_type"), ("sync_conflict", "loser_type")] {
        let has: i64 = con.query_row(&format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name = ?1"), [col], |r| r.get(0)).map_err(dbe)?;
        if has == 0 {
            con.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} TEXT")).map_err(dbe)?;
        }
    }
    Ok(())
}

/// 동기화 작업 전에: 표 + 변경 기록 트리거를 바꾼 열 목록까지 남기게 다시 만든다
/// (db.rs 의 기본 트리거는 무엇이 바뀌었는지 모른다. 다른 기능이 더한 열도 여기서 따라간다)
pub fn init(con: &Connection) -> SRes<()> {
    ensure_tables(con)?;
    // 이 기기에서 생긴 행의 HLC 를 고정한다 (한 번). 비워 두면 나중에 고친 updated_at 으로 읽혀 고치지 않은 열까지 나중 값이 된다
    if gen_of(con)? > 0 && local(con, "implicit_saved")?.is_none() {
        save_implicit(con)?;
    }
    let on = "(SELECT enabled AND NOT applying FROM sync_flags WHERE id = 1)";
    let now = "CAST(unixepoch('subsec') * 1000 AS INTEGER)";
    for t in SYNC_TABLES {
        let cols = raw_cols(con, t)?;
        let changed = cols
            .iter()
            .map(|c| format!("(CASE WHEN OLD.\"{c}\" IS NOT NEW.\"{c}\" THEN '{c},' ELSE '' END)"))
            .collect::<Vec<_>>()
            .join(" || ");
        con.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {t}_sync_update;
             CREATE TRIGGER {t}_sync_update AFTER UPDATE ON {t} WHEN OLD.uid IS NOT NULL AND {on}
             BEGIN INSERT INTO sync_outbox (table_name, uid, op, at, cols) VALUES ('{t}', NEW.uid, 'update', {now}, {changed}); END;"
        ))
        .map_err(dbe)?;
    }
    let srs = SRS_COLS.join(", ");
    let old = SRS_COLS.iter().map(|c| format!("OLD.{c}")).collect::<Vec<_>>().join(", ");
    con.execute_batch(&format!(
        "CREATE TRIGGER IF NOT EXISTS card_sync_anchor BEFORE UPDATE OF {srs} ON card WHEN {on}
         BEGIN INSERT OR IGNORE INTO sync_anchor (card_id, srs)
               VALUES (OLD.id, json_array({old}, (SELECT coalesce(max(id), 0) FROM review_log WHERE card_id = OLD.id))); END;"
    ))
    .map_err(dbe)?;
    Ok(())
}

/// 표의 열 (id · uid 빼고)
fn raw_cols(con: &Connection, t: &str) -> SRes<Vec<String>> {
    let mut st = con.prepare(&format!("SELECT name FROM pragma_table_info('{t}') WHERE name NOT IN ('id', 'uid')")).map_err(dbe)?;
    st.query_map([], |r| r.get(0)).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)
}

pub fn meta(con: &Connection, key: &str) -> SRes<Option<String>> {
    con.query_row("SELECT value FROM schema_meta WHERE key = ?1", [key], |r| r.get(0)).optional().map_err(dbe)
}

pub fn device_uid(con: &Connection) -> SRes<String> {
    meta(con, "device_uid")?.ok_or_else(|| other("기기 id 가 없어요"))
}

pub fn local(con: &Connection, key: &str) -> SRes<Option<String>> {
    con.query_row("SELECT value FROM sync_local WHERE key = ?1", [key], |r| r.get(0)).optional().map_err(dbe)
}

pub fn set_local(con: &Connection, key: &str, value: &str) -> SRes<()> {
    con.execute("INSERT OR REPLACE INTO sync_local (key, value) VALUES (?1, ?2)", [key, value]).map(|_| ()).map_err(dbe)
}

fn local_i64(con: &Connection, key: &str) -> SRes<i64> {
    Ok(local(con, key)?.and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// 세대. 0 이면 아직 Drive 에 합류하지 않았다
pub fn gen_of(con: &Connection) -> SRes<i64> {
    local_i64(con, "gen")
}

/// 이 기기 백업으로 되돌렸다 (S1-6). 받은 위치 · 올린 번호도 백업 때로 돌아가서, 백업 뒤에 이 기기가 올린 변경 파일을
/// 다음 받기가 다른 기기 것처럼 다시 받는다. 그 전에는 올리지 않는다 (Drive 에 있는 번호를 다시 쓰지 않게, Q2-09)
pub fn mark_restored(con: &Connection) -> SRes<()> {
    ensure_tables(con)?;
    if gen_of(con)? > 0 {
        set_local(con, "restored", "1")?;
    }
    Ok(())
}

/// 새 기기 id (db.rs NEW_UID 와 같은 꼴: 앞 12자리 ms 시각 + 난수)
fn new_device_uid() -> String {
    format!("{:012x}{}", db::now_ms(), crypto::random_hex(10))
}

/// 연결된 데이터 폴더를 복사해 다른 곳(다른 경로 · 다른 컴퓨터)에서 열었는가 (QA T5-02, K46). 지문은 부르는 쪽이 만든다 (경로 + 컴퓨터 이름).
/// 다르면 올리기 전에 새 기기 id 로 옮긴다: 같은 id 둘이면 먼저 올린 쪽이 다른 쪽을 막고 서로의 변경을 받지 못한다.
/// 옛 id 의 받은 위치(pos)는 그대로 둬, 원래 기기가 그 뒤 올리는 변경을 다른 기기 것처럼 받는다. 바꿨으면 true.
/// 지문이 없으면(이 기능 전에 연결했다) 지금 것을 남긴다: 이 기능 전에 만든 복사본은 알아보지 못한다
pub fn check_copy(con: &Connection, fingerprint: &str) -> SRes<bool> {
    ensure_tables(con)?;
    if gen_of(con)? == 0 {
        return Ok(false);
    }
    let known = local(con, "fingerprint")?;
    set_local(con, "fingerprint", fingerprint)?;
    if known.is_none_or(|f| f == fingerprint) {
        return Ok(false);
    }
    con.execute("UPDATE schema_meta SET value = ?1 WHERE key = 'device_uid'", [new_device_uid()]).map_err(dbe)?;
    set_local(con, "my_seq", "0")?;
    // 원래 기기가 만든 Drive 백업 · 올린 기록은 이 기기 것이 아니다
    con.execute("DELETE FROM sync_local WHERE key IN ('my_backups', 'last_push')", []).map_err(dbe)?;
    Ok(true)
}

/// Drive 가 아는 이 기기 id 의 마지막 번호: 변경 파일과 기기 파일(devices/<나>.json) 중 큰 것.
/// 정리(5.5)는 기기 파일의 번호 뒤 파일을 지우지 않는다. 변경 파일이 모두 지워졌어도 기기 파일에 남는다
fn pushed_top(r: &Remote, devices: &[J], me: &str) -> i64 {
    let file = devices.iter().filter(|d| d["uid"].as_str() == Some(me)).filter_map(|d| d["seq"].as_i64()).max().unwrap_or(0);
    r.changes.get(me).and_then(|v| v.last()).map_or(0, |(s, _)| *s).max(file)
}

/// 기기마다 어디까지 받았는가 (변경 파일 번호). 이 기기 것은 올린 번호
pub fn positions(con: &Connection) -> SRes<BTreeMap<String, i64>> {
    Ok(local(con, "pos")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default())
}

fn hlc_last(con: &Connection) -> SRes<String> {
    Ok(meta(con, "hlc_last")?.unwrap_or_else(|| "0".into()))
}

/// 받은(또는 올린) 가장 큰 HLC 를 남긴다. 그 전에 floor 가 빈 변경 기록에 지금 값을 적는다:
/// 트리거는 floor 를 쓰지 않으므로 "비어 있음 = 고친 순간의 hlc_last = 지금 값"이고, 그 값은 여기서만 바뀐다 (K28)
fn bump_hlc_last(con: &Connection, h: &str) -> SRes<()> {
    let last = hlc_last(con)?;
    if h > last.as_str() {
        con.execute("UPDATE sync_outbox SET floor = ?1 WHERE floor IS NULL", [&last]).map_err(dbe)?;
        con.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('hlc_last', ?1)", [h]).map_err(dbe)?;
    }
    Ok(())
}

/// 받은 변경을 적용하는 동안: 트리거가 쌓지 않는다. 트랜잭션 안에서만 켠다 (실패하면 함께 되돌아가 늘 꺼진다)
fn applying(tx: &Transaction, on: bool) -> SRes<()> {
    tx.execute("UPDATE sync_flags SET applying = ?1", [on as i64]).map(|_| ()).map_err(dbe)
}

// ---------------------------------------------------------------- 표 정보

#[derive(Clone)]
pub struct Table {
    pub name: &'static str,
    /// 동기화하는 열. id · uid 와 동기화하지 않는 표(import_batch 등)를 가리키는 열은 뺀다
    pub cols: Vec<String>,
    /// 외래 키 열 -> (대상 표, 대상이 없으면 NULL 로 두는가). CASCADE 면 행을 건너뛴다
    pub fks: HashMap<String, (&'static str, bool)>,
    /// 자연 키: 유일 인덱스 · 의미 있는 정수 기본 키 (deck_config.deck_id 등). 같은 값이면 같은 것이다
    pub keys: Vec<Vec<String>>,
}

fn rows<T: rusqlite::types::FromSql>(con: &Connection, sql: &str) -> SRes<Vec<Vec<T>>> {
    let mut st = con.prepare(sql).map_err(dbe)?;
    let n = st.column_count();
    st.query_map([], |r| (0..n).map(|i| r.get(i)).collect()).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)
}

/// 동기화 대상 표 (db::SYNC_TABLES, 다른 기능이 더해도 따라간다). 외래 키 순서 (부모 먼저). 지울 때는 거꾸로
pub fn tables(con: &Connection) -> SRes<Vec<Table>> {
    let mut all = vec![];
    for name in SYNC_TABLES {
        let mut fks = HashMap::new();
        let mut local_only = HashSet::new();
        for r in rows::<String>(con, &format!("SELECT \"from\", \"table\", on_delete FROM pragma_foreign_key_list('{name}')"))? {
            match SYNC_TABLES.iter().find(|t| **t == r[1]) {
                Some(t) => {
                    fks.insert(r[0].clone(), (*t, r[2].eq_ignore_ascii_case("SET NULL")));
                }
                None => {
                    local_only.insert(r[0].clone());
                }
            }
        }
        let cols: Vec<String> = raw_cols(con, name)?.into_iter().filter(|c| !local_only.contains(c)).collect();
        let mut keys = vec![];
        for idx in rows::<String>(con, &format!("SELECT name FROM pragma_index_list('{name}') WHERE \"unique\" = 1 AND partial = 0"))? {
            let k: Vec<String> = rows::<String>(con, &format!("SELECT name FROM pragma_index_info('{}')", idx[0]))?.into_iter().map(|r| r[0].clone()).collect();
            if !k.is_empty() && k.iter().all(|c| cols.contains(c)) {
                keys.push(k);
            }
        }
        // INTEGER PRIMARY KEY 는 인덱스가 없다. id 가 아니면 (deck_config.deck_id) 자연 키다
        let pk: Vec<Vec<String>> = rows(con, &format!("SELECT name, upper(type) FROM pragma_table_info('{name}') WHERE pk > 0"))?;
        if pk.len() == 1 && pk[0][0] != "id" && pk[0][1] == "INTEGER" {
            keys.push(vec![pk[0][0].clone()]);
        }
        all.push(Table { name, cols, fks, keys });
    }
    // 부모 먼저 (자기 자신을 가리키는 deck.parent_id 는 apply 가 여러 바퀴로 푼다)
    let mut ordered: Vec<Table> = vec![];
    while !all.is_empty() {
        let ready = all.iter().position(|t| t.fks.values().all(|(p, _)| *p == t.name || ordered.iter().any(|o| o.name == *p))).unwrap_or(0);
        ordered.push(all.remove(ready));
    }
    Ok(ordered)
}

fn to_json(v: ValueRef) -> J {
    use base64::Engine;
    match v {
        ValueRef::Null => J::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => J::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => json!({ "$b": base64::engine::general_purpose::STANDARD.encode(b) }),
    }
}

fn from_json(j: &J) -> Value {
    use base64::Engine;
    match j {
        J::Null => Value::Null,
        J::Bool(b) => Value::Integer(*b as i64),
        J::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Real(n.as_f64().unwrap_or(0.0)),
        },
        J::String(s) => Value::Text(s.clone()),
        J::Object(o) => match o.get("$b").and_then(|b| b.as_str()) {
            Some(b) => Value::Blob(base64::engine::general_purpose::STANDARD.decode(b).unwrap_or_default()),
            None => Value::Text(j.to_string()),
        },
        J::Array(_) => Value::Text(j.to_string()),
    }
}

/// 행 하나를 uid 참조로. (uid, 열 -> 값)
pub(crate) fn export(con: &Connection, t: &Table, rowid: i64) -> SRes<Option<(String, Map<String, J>)>> {
    let list = t.cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
    let row = con
        .prepare_cached(&format!("SELECT uid, {list} FROM {} WHERE rowid = ?1", t.name))
        .map_err(dbe)?
        .query_row([rowid], |r| {
            let uid: Option<String> = r.get(0)?;
            let mut m = Map::new();
            for (i, c) in t.cols.iter().enumerate() {
                m.insert(c.clone(), to_json(r.get_ref(i + 1)?));
            }
            Ok((uid, m))
        })
        .optional()
        .map_err(dbe)?;
    let Some((Some(uid), mut m)) = row else { return Ok(None) };
    for (col, (target, _)) in &t.fks {
        if let Some(J::Number(n)) = m.get(col) {
            let id = n.as_i64().unwrap_or(0);
            let fk: Option<String> = con
                .prepare_cached(&format!("SELECT uid FROM {target} WHERE rowid = ?1"))
                .map_err(dbe)?
                .query_row([id], |r| r.get(0))
                .optional()
                .map_err(dbe)?
                .flatten();
            m.insert(col.clone(), fk.map(J::String).unwrap_or(J::Null));
        }
    }
    Ok(Some((uid, m)))
}

fn rowid_of(con: &Connection, table: &str, uid: &str) -> SRes<Option<i64>> {
    con.prepare_cached(&format!("SELECT rowid FROM {table} WHERE uid = ?1")).map_err(dbe)?.query_row([uid], |r| r.get(0)).optional().map_err(dbe)
}

// ---------------------------------------------------------------- 행 HLC

#[derive(Clone, Debug, Default)]
struct Clock {
    base: String,
    cols: BTreeMap<String, String>,
    deleted: Option<String>,
}

impl Clock {
    fn col(&self, c: &str) -> &str {
        self.cols.get(c).map(|s| s.as_str()).unwrap_or(&self.base)
    }

    /// 동기화 전에 생긴 이 기기 행: 마지막으로 고친 벽시계 시각을 HLC 로 쓴다 (처음 합칠 때 "나중에 고친 쪽").
    /// 기기 자리는 이 기기 (K7 에 "이 기기"로 보인다. 옛 앱이 만든 스냅샷은 비어 있다)
    fn implicit(row: &Map<String, J>, dev: &str) -> Clock {
        let phys = IMPLICIT_TIME.iter().find_map(|k| row.get(*k).and_then(|v| v.as_i64())).unwrap_or(0);
        Clock { base: hlc(phys, 0, dev), ..Default::default() }
    }
}

const IMPLICIT_TIME: [&str; 5] = ["updated_at", "created_at", "started_at", "reviewed_at", "written_at"];

/// 시계가 없는 이 기기 행에 Clock::implicit 과 같은 HLC 를 적어 고정한다 (스냅샷에 실은 값 그대로).
/// 적어 두지 않으면 그 행을 고쳤을 때 updated_at 이 바뀌어, 고치지 않은 열까지 나중 값으로 읽힌다
fn save_implicit(con: &Connection) -> SRes<()> {
    let me = device_uid(con)?;
    for t in SYNC_TABLES {
        let cols = raw_cols(con, t)?;
        let time: Vec<&str> = IMPLICIT_TIME.iter().copied().filter(|c| cols.iter().any(|x| x == c)).collect();
        let phys = if time.is_empty() { "0".to_string() } else { format!("coalesce({}, 0)", time.join(", ")) };
        con.execute(
            &format!("INSERT OR IGNORE INTO sync_row (table_name, uid, hlc) SELECT '{t}', uid, printf('%013d-00000000-%s', max(0, {phys}), ?1) FROM {t} WHERE uid IS NOT NULL"),
            [&me],
        )
        .map_err(dbe)?;
    }
    set_local(con, "implicit_saved", "1")
}

fn load_clock(con: &Connection, table: &str, uid: &str) -> SRes<Option<Clock>> {
    con.prepare_cached("SELECT hlc, cols, deleted FROM sync_row WHERE table_name = ?1 AND uid = ?2")
        .map_err(dbe)?
        .query_row([table, uid], |r| {
            let cols: Option<String> = r.get(1)?;
            Ok(Clock { base: r.get(0)?, cols: cols.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default(), deleted: r.get(2)? })
        })
        .optional()
        .map_err(dbe)
}

fn save_clock(con: &Connection, table: &str, uid: &str, c: &Clock) -> SRes<()> {
    let cols = (!c.cols.is_empty()).then(|| serde_json::to_string(&c.cols).unwrap_or_default());
    con.prepare_cached("INSERT OR REPLACE INTO sync_row (table_name, uid, hlc, cols, deleted) VALUES (?1, ?2, ?3, ?4, ?5)")
        .map_err(dbe)?
        .execute(params![table, uid, c.base, cols, c.deleted])
        .map(|_| ())
        .map_err(dbe)
}

/// 아직 올리지 않은 이 기기 변경 (sync_outbox). 행마다 열 -> 고친 순간의 HLC (K28)
#[derive(Default, Debug, Clone)]
struct Pending {
    /// 모든 열에 걸친 변경 (넣기 · 열 목록 없는 고치기 · 되살리기)의 HLC
    all: Option<String>,
    cols: HashMap<String, String>,
    /// 이 기기에서 지운 HLC
    deleted: Option<String>,
    /// 지금 열 HLC 그대로 다시 보낸다 (RESEND)
    resend: bool,
    /// 이 기기에서 새로 만든 행 (첫 기록이 트리거의 넣기)
    created: bool,
}

fn max_of<'a>(a: Option<&'a str>, b: Option<&'a str>) -> Option<&'a str> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

impl Pending {
    /// 이 열을 고친 HLC
    fn col(&self, c: &str) -> Option<&str> {
        max_of(self.all.as_deref(), self.cols.get(c).map(|s| s.as_str()))
    }

    /// 이 행에 남은 이 기기 변경 중 가장 나중
    fn newest(&self) -> Option<&str> {
        let cols = self.cols.values().map(|s| s.as_str()).max();
        max_of(max_of(self.all.as_deref(), cols), self.deleted.as_deref())
    }

    fn all_cols(&self) -> bool {
        self.all.is_some() || self.resend
    }
}

/// 열 c 의 이 기기 HLC: 받은 값과 아직 올리지 않은 이 기기 변경 중 큰 것.
/// 받은 값이 이 기기 변경을 이겼으면 지금 값도 받은 값이라 받은 HLC 가 맞다
fn col_hlc(clock: &Clock, p: Option<&Pending>, c: &str) -> String {
    max_of(Some(clock.col(c)), p.and_then(|p| p.col(c))).unwrap_or_default().to_string()
}

/// 변경 기록을 행으로 묶는다 (처음 기록 순서). 기록마다 HLC = hlc_next(floor, at): 고친 순간 (K28)
fn load_pending(con: &Connection) -> SRes<Vec<((String, String), Pending)>> {
    let me = device_uid(con)?;
    let last = hlc_last(con)?;
    let mut st = con.prepare("SELECT table_name, uid, op, cols, at, floor FROM sync_outbox ORDER BY seq").map_err(dbe)?;
    type Row = (String, String, String, Option<String>, i64, Option<String>);
    let rows: Vec<Row> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?;
    let mut order: Vec<(String, String)> = vec![];
    let mut out: HashMap<(String, String), Pending> = HashMap::new();
    for (t, u, op, cols, at, floor) in rows {
        let key = (t, u);
        let first = !out.contains_key(&key);
        if first {
            order.push(key.clone());
        }
        let p = out.entry(key).or_default();
        let h = hlc_next(floor.as_deref().unwrap_or(&last), at, &me);
        let raise = |slot: &mut Option<String>| {
            if slot.as_ref().is_none_or(|o| h > *o) {
                *slot = Some(h.clone());
            }
        };
        match (op.as_str(), cols.as_deref()) {
            ("insert", Some(RESEND)) => p.resend = true,
            ("insert", c) => {
                p.created |= first && c.is_none();
                raise(&mut p.all);
            }
            ("update", None) => raise(&mut p.all),
            ("update", Some(c)) => {
                for col in c.split(',').filter(|s| !s.is_empty()) {
                    let e = p.cols.entry(col.to_string()).or_default();
                    if h > *e {
                        *e = h.clone();
                    }
                }
            }
            _ => raise(&mut p.deleted),
        }
    }
    Ok(order.into_iter().map(|k| {
        let p = out.remove(&k).unwrap_or_default();
        (k, p)
    }).collect())
}

// ---------------------------------------------------------------- 파일 형식

/// 변경 한 줄. 참조 열의 값은 uid 다
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Rec {
    pub t: String,
    pub u: String,
    /// "up" (넣기 · 고치기) | "del"
    pub op: String,
    pub h: String,
    /// 행 전체. 지워진 행을 되살릴 때 필요해서 고친 열만이 아니라 늘 전부 싣는다
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<Map<String, J>>,
    /// 고친 열. 없으면 전부 (넣기)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<Vec<String>>,
    /// 스냅샷: h 와 다른 열 HLC
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<BTreeMap<String, String>>,
    /// note.fields 를 고칠 때 바탕이 된 값의 HLC. 받는 쪽이 이것보다 새 값을 가졌으면 동시에 고친 것이다 (K7)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<String>,
}

pub fn seal_opt(data: Vec<u8>, key: Option<&[u8; 32]>) -> Vec<u8> {
    match key {
        Some(k) => crypto::seal(k, &data),
        None => data,
    }
}

/// 잠갔으면(키가 있으면) 암호문만 받는다. 평문은 누가 써 넣었는지 알 수 없다
pub fn open_opt(data: Vec<u8>, key: Option<&[u8; 32]>) -> SRes<Vec<u8>> {
    match key {
        None if !crypto::is_sealed(&data) => Ok(data),
        None => Err(SyncError::Locked),
        Some(k) => crypto::unseal(k, &data).map_err(|_| SyncError::Locked),
    }
}

/// 풀었을 때 크기 한도 (gzip · zip 폭탄). 스냅샷은 통째로 메모리에 읽는다
/// ponytail: 복습 기록이 수백만 건이면 스냅샷이 1 GiB 에 닿는다. 그때는 줄 단위로 읽어 합친다
const MAX_UNPACKED: u64 = 1 << 30;

/// 받은 HLC 의 형식: hlc() 가 만든 값이거나 "0" (시계가 없던 행). 숫자가 아닌 큰 값("~")이 남으면 그 행은 다시는 고칠 수 없다
fn valid_hlc(h: &str) -> bool {
    let mut it = h.splitn(3, '-');
    let (phys, counter, dev) = (it.next().unwrap_or(""), it.next(), it.next());
    h == "0"
        || (phys.len() >= 13
            && phys.bytes().all(|b| b.is_ascii_digit())
            && counter.is_some_and(|c| c.len() == 8 && c.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            && dev.is_some_and(|d| d.bytes().all(|b| b.is_ascii_alphanumeric())))
}

fn encode(header: &J, recs: &[Rec], key: Option<&[u8; 32]>) -> SRes<Vec<u8>> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    writeln!(gz, "{header}").map_err(other)?;
    for r in recs {
        serde_json::to_writer(&mut gz, r).map_err(other)?;
        gz.write_all(b"\n").map_err(other)?;
    }
    Ok(seal_opt(gz.finish().map_err(other)?, key))
}

pub fn decode(data: Vec<u8>, key: Option<&[u8; 32]>) -> SRes<(J, Vec<Rec>)> {
    let plain = open_opt(data, key)?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(&plain[..]).take(MAX_UNPACKED + 1).read_to_string(&mut text).map_err(|e| other(format!("변경 파일을 읽지 못했어요: {e}")))?;
    if text.len() as u64 > MAX_UNPACKED {
        return Err(other("변경 파일이 너무 커요"));
    }
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: J = serde_json::from_str(lines.next().unwrap_or("{}")).map_err(other)?;
    if header["v"].as_i64().unwrap_or(0) > FORMAT {
        return Err(SyncError::NewerSchema);
    }
    let recs: Vec<Rec> = lines.map(serde_json::from_str).collect::<Result<_, _>>().map_err(other)?;
    if recs.iter().any(|r| !valid_hlc(&r.h) || r.k.iter().flatten().any(|(_, h)| !valid_hlc(h)) || r.b.as_deref().is_some_and(|b| !valid_hlc(b))) {
        return Err(other("변경 파일을 읽지 못했어요: 시각 값이 올바르지 않아요"));
    }
    Ok((header, recs))
}

/// 다른 기기의 변경 파일 하나. 잠근 Drive 의 평문 파일은 받지 않고 건너뛴다 (None):
/// 잠그기 전 세대의 기기가 늦게 올렸거나 누가 써 넣은 파일이다. 오류로 멈추면 그 파일이 지워질 때까지 아무것도 받지 못한다.
/// 잠기지 않은 Drive(crypto.json 없음)의 암호문도 같다: 지운 Drive 를 다시 시작하기 전 세대의 잠근 기기가 올린 것이다 (T3-10)
fn read_change(cx: &Ctx, path: &str, locked: bool) -> SRes<Option<(J, Vec<Rec>)>> {
    let data = cx.store.get(path)?;
    if (cx.key.is_some() && !crypto::is_sealed(&data)) || (cx.key.is_none() && !locked && crypto::is_sealed(&data)) {
        return Ok(None);
    }
    decode(data, cx.key()).map(Some)
}

/// changes/<기기>/<번호>.jsonl.gz -> (기기, 번호)
fn parse_change(path: &str) -> Option<(String, i64)> {
    let rest = path.strip_prefix("changes/")?;
    let (dev, file) = rest.split_once('/')?;
    Some((dev.to_string(), file.strip_suffix(".jsonl.gz")?.parse().ok()?))
}

/// snapshot/g<세대>-<기기>-<번호>.jsonl.gz
#[derive(Clone, Debug)]
pub struct SnapName {
    pub path: String,
    pub generation: i64,
    pub device: String,
    /// 올린 시각 (ms)
    pub at: i64,
}

fn parse_snapshot(path: &str, at: i64) -> Option<SnapName> {
    let name = path.strip_prefix("snapshot/g")?.strip_suffix(".jsonl.gz")?;
    let mut it = name.splitn(3, '-');
    let s = SnapName { path: path.into(), generation: it.next()?.parse().ok()?, device: it.next()?.into(), at };
    it.next()?.parse::<i64>().ok().map(|_| s)
}

fn change_path(dev: &str, seq: i64) -> String {
    format!("changes/{dev}/{seq:08}.jsonl.gz")
}

/// 그림 이름. 내용 해시라 같은 파일은 한 번만 올린다. 잠갔으면 키를 넣은 HMAC (7장)
pub fn media_path(sha: &str, filename: &str, key: Option<&[u8; 32]>) -> String {
    match key {
        Some(k) => format!("media/{}", crypto::hex(&crypto::hmac_sha256(k, sha.as_bytes()))),
        None => {
            let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()));
            match ext {
                Some(e) => format!("media/{sha}.{e}"),
                None => format!("media/{sha}"),
            }
        }
    }
}

// ---------------------------------------------------------------- 문맥

pub struct Ctx<'a> {
    pub store: &'a dyn Store,
    pub key: Option<[u8; 32]>,
    pub media_dir: PathBuf,
    pub fsrs: &'a fsrs::FSRS,
    /// 진행 표시 (단계, 끝난 수, 전체)
    pub progress: &'a (dyn Fn(&str, u64, u64) + Sync),
}

impl Ctx<'_> {
    fn key(&self) -> Option<&[u8; 32]> {
        self.key.as_ref()
    }
}

fn lock(db: &Mutex<Connection>) -> SRes<std::sync::MutexGuard<'_, Connection>> {
    db.lock().map_err(|_| other("데이터베이스를 잠그지 못했어요"))
}

/// 동기화 전에 생긴 그림은 sha256 열이 비었거나 다른 해시다. 파일을 읽어 맞춘다 (그림 이름 = 내용 해시)
fn fix_media_hashes(con: &mut Connection, media_dir: &Path) -> SRes<()> {
    let rows: Vec<(i64, String)> = {
        let mut st = con.prepare("SELECT id, filename FROM media WHERE length(sha256) <> 64").map_err(dbe)?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
    };
    if rows.is_empty() {
        return Ok(());
    }
    let tx = con.transaction().map_err(dbe)?;
    applying(&tx, true)?;
    for (id, name) in rows.into_iter().filter(|(_, n)| safe_media_name(n).is_ok()) {
        if let Ok(bytes) = std::fs::read(media_dir.join(&name)) {
            tx.execute("UPDATE media SET sha256 = ?2 WHERE id = ?1", params![id, crypto::hex(&crypto::sha256(&bytes))]).map_err(dbe)?;
        }
    }
    applying(&tx, false)?;
    tx.commit().map_err(dbe)
}

// ---------------------------------------------------------------- 올리기 (5.2)

/// 올린 행: 열마다 보낸 HLC (all = 모든 열을 보냈다), 또는 지운 HLC
struct Pushed {
    t: String,
    u: String,
    all: bool,
    cols: BTreeMap<String, String>,
    deleted: Option<String>,
}

pub struct Batch {
    seq: i64,
    path: String,
    data: Vec<u8>,
    max_outbox: i64,
    hlc: String,
    rows: Vec<Pushed>,
    /// 올릴 그림: (Drive 경로, 이 기기 파일)
    media: Vec<(String, PathBuf)>,
    pub reviews: i64,
}

/// 묶을 것이 없으면 None. 파일 번호는 여기서 미리 차지한다 (올리다 실패한 번호를 다시 쓰지 않는다)
pub fn prepare_push(con: &mut Connection, cx: &Ctx) -> SRes<Option<Batch>> {
    init(con)?;
    fix_media_hashes(con, &cx.media_dir)?;
    let max_outbox: Option<i64> = con.query_row("SELECT max(seq) FROM sync_outbox", [], |r| r.get(0)).map_err(dbe)?;
    let Some(max_outbox) = max_outbox else { return Ok(None) };
    let me = device_uid(con)?;
    let tables = tables(con)?;
    let mut recs = vec![];
    let mut out_rows = vec![];
    let mut media = vec![];
    let mut reviews = 0;
    let mut top = String::new();
    for ((t, u), p) in load_pending(con)? {
        let Some(table) = tables.iter().find(|x| x.name == t) else { continue };
        match rowid_of(con, &t, &u)? {
            Some(rowid) => {
                let names: Vec<String> = if p.all_cols() { table.cols.clone() } else { table.cols.iter().filter(|c| p.cols.contains_key(*c)).cloned().collect() };
                if names.is_empty() {
                    continue; // 값이 그대로인 UPDATE
                }
                let Some((_, r)) = export(con, table, rowid)? else { continue };
                let clock = load_clock(con, &t, &u)?.unwrap_or_else(|| Clock::implicit(&r, &me));
                // 열마다 고친 순간의 HLC. 받은 값이 이겼던 열은 받은 HLC 그대로 (지금 값이 그 값이다)
                let per: BTreeMap<String, String> = names.iter().map(|c| (c.clone(), col_hlc(&clock, Some(&p), c))).collect();
                let h = per.values().min().cloned().unwrap_or_default();
                top = top.max(per.values().max().cloned().unwrap_or_default());
                let k: BTreeMap<String, String> = per.iter().filter(|(_, v)| **v != h).map(|(c, v)| (c.clone(), v.clone())).collect();
                // 처음 합친 결과는 Drive 쪽 내용을 바탕으로 고친 것이 아니다: 받는 쪽이 다르면 K7 에 남긴다
                let b = (t == "note" && per.contains_key("fields")).then(|| if p.resend && p.all.is_none() { "0".to_string() } else { clock.col("fields").to_string() });
                if t == "media"
                    && let (Some(sha), Some(name)) = (r.get("sha256").and_then(|v| v.as_str()), r.get("filename").and_then(|v| v.as_str()))
                    && sha.len() == 64
                    && safe_media_name(name).is_ok()
                {
                    media.push((media_path(sha, name, cx.key()), cx.media_dir.join(name)));
                }
                // 새로 올린 복습 기록 (처음 합친 뒤 Drive 가 모르던 이 기기 기록도)
                if t == "review_log" && (p.created || p.resend) {
                    reviews += 1;
                }
                let all = p.all_cols();
                recs.push(Rec { t: t.clone(), u: u.clone(), op: "up".into(), h, r: Some(r), c: (!all).then_some(names), k: (!k.is_empty()).then_some(k), b });
                out_rows.push(Pushed { t, u, all, cols: per, deleted: None });
            }
            // 이번 묶음에서 생겼다 사라진 행(되돌린 복습 기록 등)은 아무도 모르니 보낼 것도 없다.
            // 이 기기가 지우지 않았는데 없는 행은 받은 지우기가 지웠다 (다른 기기도 안다)
            None if p.created => {}
            None => {
                let Some(d) = p.deleted else { continue };
                top = top.max(d.clone());
                recs.push(Rec { t: t.clone(), u: u.clone(), op: "del".into(), h: d.clone(), r: None, c: None, k: None, b: None });
                out_rows.push(Pushed { t, u, all: false, cols: BTreeMap::new(), deleted: Some(d) });
            }
        }
    }
    let seq = local_i64(con, "my_seq")? + 1;
    set_local(con, "my_seq", &seq.to_string())?;
    let header = json!({ "v": FORMAT, "kind": "changes", "gen": gen_of(con)?, "device": me, "seq": seq, "at": db::now_ms(), "hlc": top, "reviews": reviews });
    let data = encode(&header, &recs, cx.key())?;
    Ok(Some(Batch { seq, path: change_path(&me, seq), data, max_outbox, hlc: top, rows: out_rows, media, reviews }))
}

/// 올리기에 성공한 뒤: 행 HLC 를 남기고, 올린 변경을 지우고, 받은 위치에 이 기기 번호를 적는다
pub fn finish_push(con: &mut Connection, b: &Batch) -> SRes<()> {
    let me = device_uid(con)?;
    let tx = con.transaction().map_err(dbe)?;
    for p in &b.rows {
        let mut c = load_clock(&tx, &p.t, &p.u)?.unwrap_or_default();
        if let Some(d) = &p.deleted {
            if c.deleted.as_ref().is_none_or(|old| d > old) {
                c.deleted = Some(d.clone());
            }
            if c.base.is_empty() {
                c.base = "0".into();
            }
        } else {
            // 열 HLC 는 보낸 값으로 (보낸 값은 이미 지금 값 이상이다. 그래도 내려가지 않게 max)
            let cols: BTreeMap<String, String> = p.cols.iter().map(|(col, h)| (col.clone(), h.clone().max(c.col(col).to_string()))).collect();
            if p.all {
                c.base = cols.values().min().cloned().unwrap_or_else(|| "0".into());
                c.deleted = None;
                c.cols.clear();
            } else if c.base.is_empty() {
                c.base = "0".into();
            }
            for (col, h) in cols {
                c.cols.insert(col, h);
            }
            let base = c.base.clone();
            c.cols.retain(|_, h| *h > base);
        }
        save_clock(&tx, &p.t, &p.u, &c)?;
        if p.t == "card" {
            // 앵커는 다른 기기가 이 번호를 받았다고 알 때까지 둔다 (K27). 그 사이 엇갈려 온 기록도 이 앵커부터 다시 돌린다
            tx.execute("UPDATE sync_anchor SET srs = json_set(srs, '$[10]', ?2) WHERE card_id = (SELECT id FROM card WHERE uid = ?1)", params![p.u, b.seq])
                .map_err(dbe)?;
        }
    }
    tx.execute("DELETE FROM sync_outbox WHERE seq <= ?1", [b.max_outbox]).map_err(dbe)?;
    // 지운 흔적은 sync_row.deleted 에 HLC 로 남았다. 트리거가 따로 쌓는 sync_tombstone 은 비워 DB 를 작게
    tx.execute("DELETE FROM sync_tombstone", []).map_err(dbe)?;
    bump_hlc_last(&tx, &b.hlc)?;
    let mut pos = positions(&tx)?;
    pos.insert(me, b.seq);
    set_local(&tx, "pos", &serde_json::to_string(&pos).map_err(other)?)?;
    set_local(&tx, "last_push", &json!({ "at": db::now_ms(), "reviews": b.reviews }).to_string())?;
    tx.commit().map_err(dbe)
}

/// 그림은 Drive 에 없을 때만 올린다 (sha256 이름이라 같은 그림은 한 번)
fn upload_media(cx: &Ctx, media: &[(String, PathBuf)], have: &mut HashSet<String>) -> SRes<()> {
    let todo: Vec<_> = media.iter().filter(|(p, _)| !have.contains(p)).collect();
    for (i, (path, file)) in todo.iter().enumerate() {
        (cx.progress)("media", i as u64, todo.len() as u64);
        let Ok(bytes) = std::fs::read(file) else { continue };
        cx.store.put(path, &seal_opt(bytes, cx.key()))?;
        have.insert(path.clone());
    }
    Ok(())
}

fn remote_media(store: &dyn Store) -> SRes<HashSet<String>> {
    Ok(store.list("media/")?.into_iter().map(|e| e.path).collect())
}

/// 올리기 한 번. 올린 변경이 없으면 false
pub fn push(db: &Mutex<Connection>, cx: &Ctx) -> SRes<bool> {
    let (me, my_seq) = {
        let con = lock(db)?;
        ensure_tables(&con)?;
        // 올릴 것이 없다. 또는 이 기기 백업으로 되돌렸다: 받기가 Drive 와 다시 맞춘 뒤에 올린다 (mark_restored)
        let empty: bool = con.query_row("SELECT NOT EXISTS (SELECT 1 FROM sync_outbox)", [], |r| r.get(0)).map_err(dbe)?;
        if empty || local(&con, "restored")?.is_some() {
            return Ok(false);
        }
        (device_uid(&con)?, local_i64(&con, "my_seq")?)
    };
    // 이 기기가 올린 번호보다 큰 이 기기 파일이 있으면 같은 기기 id 를 쓰는 다른 데이터가 올렸다
    // (연결된 데이터 폴더를 복사했거나, 앱 밖에서 옛 파일로 되돌렸다). 덮지 않는다.
    // 번호를 차지하기 전에 멈춘다: 차지한 뒤에 멈추면 다음 번에는 말없이 다음 번호로 올리고 그 파일들은 끝내 받지 않는다 (Q2-09)
    let mine: Vec<J> = cx.store.get(&format!("devices/{me}.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).into_iter().collect();
    if pushed_top(&remote(cx.store)?, &mine, &me) > my_seq {
        return Err(other("이 기기 데이터를 다른 컴퓨터로 복사했거나 앱 밖에서 예전 파일로 되돌린 것 같아요. 설정 › Google Drive에서 연결을 끊고(Drive 데이터 남기기) 다시 연결해 주세요."));
    }
    let batch = { prepare_push(&mut *lock(db)?, cx)? };
    let Some(batch) = batch else { return Ok(false) };
    (cx.progress)("push", 0, 1);
    if !batch.media.is_empty() {
        let mut have = remote_media(cx.store)?;
        upload_media(cx, &batch.media, &mut have)?;
    }
    cx.store.put(&batch.path, &batch.data)?;
    finish_push(&mut *lock(db)?, &batch)?;
    Ok(true)
}

// ---------------------------------------------------------------- 받기 (5.3)

#[derive(Default, Debug, Clone, Serialize)]
pub struct Report {
    /// 적용한 변경 줄
    pub applied: usize,
    /// 두 기기에서 고친 카드 (새로 생긴 K7 기록)
    pub conflicts: usize,
    /// FSRS 를 다시 계산한 카드
    pub recomputed: usize,
    pub skipped: usize,
}

/// Drive 의 지금 모습
#[derive(Default, Debug)]
pub struct Remote {
    pub snapshots: Vec<SnapName>,
    /// 기기 -> (번호, 경로) 오름차순
    pub changes: BTreeMap<String, Vec<(i64, String)>>,
    pub devices: Vec<String>,
    /// 그림 경로 -> 크기
    pub media: HashMap<String, u64>,
    pub locked: bool,
    /// 숨김 공간 전체 크기 (K4 "Addiem이 쓰는 공간")
    pub bytes: u64,
}

impl Remote {
    pub fn generation(&self) -> i64 {
        self.snapshots.iter().map(|s| s.generation).max().unwrap_or(0)
    }

    /// 그 세대에서 가장 많이 포함한 스냅샷 (5.5: 둘 다 올바르고, 더 많이 포함한 쪽을 쓴다).
    /// 쓰는 기기는 받기를 마친 뒤에 쓰므로 나중에 올린 것이 더 많이 포함한다 (번호는 기기마다 따로 세서 비교할 수 없다)
    pub fn newest(&self, generation: i64) -> Option<&SnapName> {
        self.snapshots.iter().filter(|s| s.generation == generation).max_by_key(|s| (s.at, s.path.clone()))
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }
}

pub fn remote(store: &dyn Store) -> SRes<Remote> {
    let mut r = Remote::default();
    for e in store.list("")? {
        r.bytes += e.size;
        if let Some(s) = parse_snapshot(&e.path, e.at) {
            r.snapshots.push(s);
        } else if let Some((dev, seq)) = parse_change(&e.path) {
            r.changes.entry(dev).or_default().push((seq, e.path));
        } else if e.path.starts_with("devices/") && e.path.ends_with(".json") {
            r.devices.push(e.path);
        } else if e.path.starts_with("media/") {
            r.media.insert(e.path, e.size);
        } else if e.path == "crypto.json" {
            r.locked = true;
        }
    }
    r.changes.values_mut().for_each(|v| v.sort());
    Ok(r)
}

/// 다른 기기 파일 (devices/<uid>.json)
pub fn read_devices(store: &dyn Store, r: &Remote) -> Vec<J> {
    r.devices.iter().filter_map(|p| store.get(p).ok()).filter_map(|b| serde_json::from_slice(&b).ok()).collect()
}

/// 세대가 바뀌었다 (다른 기기가 되돌리기 · 새로 시작). 이 기기는 고르는 창(K6-m)을 거친다
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub generation: i64,
    pub header: J,
}

pub enum Pulled {
    Done(Report),
    /// 받지 않고 고르게 한다
    Choice(Choice),
    /// Drive 의 데이터가 없어졌다 (다른 기기가 지웠다, K10)
    Gone,
}

/// 받기 한 번: 다른 기기의 새 변경 파일을 모두 받아 한 트랜잭션에서 합친다
pub fn pull(db: &Mutex<Connection>, cx: &Ctx) -> SRes<Pulled> {
    let (me, generation, pos, restored) = {
        let con = lock(db)?;
        init(&con)?;
        (device_uid(&con)?, gen_of(&con)?, positions(&con)?, local(&con, "restored")?.is_some())
    };
    // 받을 기기. 이 기기 백업으로 되돌렸으면 백업 뒤에 이 기기가 올린 파일도 받는다 (mark_restored)
    let theirs = |d: &str| d != me || restored;
    (cx.progress)("pull", 0, 1);
    let r = remote(cx.store)?;
    if r.is_empty() {
        return Ok(if generation > 0 { Pulled::Gone } else { Pulled::Done(Report::default()) });
    }
    // 세대가 올랐다(다른 기기가 되돌리기 · 새로 시작 · 잠그기) 또는 이 기기 세대의 스냅샷이 없다(지운 Drive 를 다른 기기가 다시 시작했다, T3-10)
    if r.generation() > generation || r.newest(generation).is_none() {
        let snap = r.newest(r.generation()).expect("세대가 있으면 스냅샷도 있다");
        saw_generation(db, r.generation())?;
        // 다른 기기가 잠갔으면 머리도 못 읽는다. 고르는 창이 암호를 묻는다. 잠금을 풀었으면 평문이라 키 없이 머리만 읽는다
        let data = cx.store.get(&snap.path)?;
        let key = cx.key().filter(|_| crypto::is_sealed(&data));
        let header = match decode(data, key) {
            Ok((h, _)) => h,
            Err(SyncError::Locked) => json!({ "gen": r.generation(), "device": snap.device, "reason": { "kind": "lock" } }),
            Err(e) => return Err(e),
        };
        return Ok(Pulled::Choice(Choice { generation: r.generation(), header }));
    }
    // 뒤처진 기기: 필요한 변경 파일이 이미 정리됐으면 스냅샷부터 (5.5).
    // 되돌린 기기도: 백업 뒤에 이 기기가 올린 파일은 정리돼 스냅샷에만 있을 수 있다 (남은 파일로는 알 수 없다)
    let mut from = pos.clone();
    let mut recs: Vec<Rec> = vec![];
    let mut aliases = vec![];
    let gap = restored || r.changes.iter().any(|(d, files)| theirs(d) && files.first().is_some_and(|(s, _)| *s > from.get(d).copied().unwrap_or(0) + 1));
    if gap && let Some(snap) = r.newest(generation) {
        let (header, snap_recs) = decode(cx.store.get(&snap.path)?, cx.key())?;
        for (d, s) in header["covers"].as_object().into_iter().flatten() {
            let s = s.as_i64().unwrap_or(0);
            if theirs(d) && s > from.get(d).copied().unwrap_or(0) {
                from.insert(d.clone(), s);
            }
        }
        aliases.extend(header["aliases"].as_array().cloned().unwrap_or_default());
        recs.extend(snap_recs);
    }
    let mut new_pos = from.clone();
    let todo: Vec<(String, i64, String)> = r
        .changes
        .iter()
        .filter(|(d, _)| theirs(d))
        .flat_map(|(d, files)| files.iter().filter(|(s, _)| *s > from.get(d).copied().unwrap_or(0)).map(move |(s, p)| (d.clone(), *s, p.clone())))
        .collect();
    for (i, (dev, seq, path)) in todo.iter().enumerate() {
        (cx.progress)("pull", i as u64, todo.len() as u64);
        let Some((header, file_recs)) = read_change(cx, path, r.locked)? else {
            new_pos.insert(dev.clone(), *seq);
            continue;
        };
        let file_gen = header["gen"].as_i64().unwrap_or(0);
        if file_gen > generation {
            saw_generation(db, file_gen)?;
            return Ok(Pulled::Choice(Choice { generation: file_gen, header }));
        }
        // 세대가 바뀌기 전에 다른 기기가 쓴 변경은 버린다 (되돌린 상태를 다시 덮지 않게)
        if file_gen == generation {
            recs.extend(file_recs);
        }
        new_pos.insert(dev.clone(), *seq);
    }
    let devices = read_devices(cx.store, &r);
    let report = {
        let mut con = lock(db)?;
        let tx = con.transaction().map_err(dbe)?;
        applying(&tx, true)?;
        add_aliases(&tx, &aliases)?;
        let report = apply(&tx, recs, Mode::Sync, cx.fsrs, "")?;
        let mut pos = positions(&tx)?;
        for (d, s) in new_pos {
            let e = pos.entry(d).or_insert(0);
            *e = (*e).max(s);
        }
        if restored {
            // 다음 올리기는 Drive 에 있는 이 기기 번호 뒤부터
            let seq = local_i64(&tx, "my_seq")?.max(pushed_top(&r, &devices, &me));
            set_local(&tx, "my_seq", &seq.to_string())?;
            pos.insert(me.clone(), seq);
            tx.execute("DELETE FROM sync_local WHERE key = 'restored'", []).map_err(dbe)?;
        }
        set_local(&tx, "pos", &serde_json::to_string(&pos).map_err(other)?)?;
        set_local(&tx, "devices", &serde_json::to_string(&devices).map_err(other)?)?;
        release_anchors(&tx, &me, generation, &devices)?;
        applying(&tx, false)?;
        tx.commit().map_err(dbe)?;
        report
    };
    fetch_media(db, cx, &r.media)?;
    Ok(Pulled::Done(report))
}

/// 본 적 있는 가장 큰 세대. 지운 Drive 를 다시 시작할 때 이보다 큰 세대를 쓴다 (다른 기기가 지난 세대로 보고 버리지 않게, T3-10)
fn saw_generation(db: &Mutex<Connection>, g: i64) -> SRes<()> {
    let con = lock(db)?;
    if g > local_i64(&con, "seen_gen")? {
        set_local(&con, "seen_gen", &g.to_string())?;
    }
    Ok(())
}

/// 앵커를 놓아도 되는가 (K27): 이 기기가 올린 복습을 활성 기기가 모두 받았고, 이 기기도 그 기기가 그 뒤 올린 것까지 받았다.
/// 그러면 엇갈려 올린 기록은 모두 여기 와서 다시 계산을 마쳤다. 아직 올리지 않은 변경이 있는 카드는 그대로 둔다
fn release_anchors(tx: &Transaction, me: &str, generation: i64, devices: &[J]) -> SRes<()> {
    let pos = positions(tx)?;
    let now = db::now_ms();
    let seen = devices
        .iter()
        .filter(|d| d["uid"].as_str().is_some_and(|u| u != me) && d["gen"].as_i64() == Some(generation) && now - d["at"].as_i64().unwrap_or(0) < STALE_MS)
        .map(|d| {
            let caught_up = pos.get(d["uid"].as_str().unwrap_or_default()).copied().unwrap_or(0) >= d["seq"].as_i64().unwrap_or(0);
            if caught_up { d["pos"][me].as_i64().unwrap_or(0) } else { 0 }
        })
        .min()
        .unwrap_or(i64::MAX);
    tx.execute(
        "DELETE FROM sync_anchor WHERE json_extract(srs, '$[10]') <= ?1
           AND NOT EXISTS (SELECT 1 FROM sync_outbox o JOIN card c ON o.table_name = 'card' AND o.uid = c.uid WHERE c.id = sync_anchor.card_id)",
        [seen],
    )
    .map(|_| ())
    .map_err(dbe)
}

/// 이 기기에 없는 그림을 받는다. 데이터를 먼저 합치고 그림은 뒤에서 (K6-f).
/// 받다 끊겨 Drive 것보다 작은 파일도 다시 받는다. 임시 이름(.<이름>.part)에 쓰고 바꿔, 끊겨도 잘린 파일이 제 이름으로 남지 않는다 (QA T5-04 Drive 쪽, K50)
pub fn fetch_media(db: &Mutex<Connection>, cx: &Ctx, have_remote: &HashMap<String, u64>) -> SRes<usize> {
    let rows: Vec<(String, String)> = {
        let con = lock(db)?;
        let mut st = con.prepare("SELECT filename, sha256 FROM media WHERE length(sha256) = 64").map_err(dbe)?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
    };
    // Drive 파일 크기 - 잠근 파일의 머리 · nonce · 태그 = 그림 크기
    let overhead = if cx.key.is_some() { crypto::SEALED_EXTRA } else { 0 };
    let short = |name: &str, size: u64| std::fs::metadata(cx.media_dir.join(name)).map_or(true, |m| m.len() < size.saturating_sub(overhead));
    let todo: Vec<_> = rows
        .into_iter()
        .filter(|(name, sha)| safe_media_name(name).is_ok() && have_remote.get(&media_path(sha, name, cx.key())).is_some_and(|size| short(name, *size)))
        .collect();
    if todo.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(&cx.media_dir).map_err(other)?;
    let mut n = 0;
    for (i, (name, sha)) in todo.iter().enumerate() {
        (cx.progress)("media", i as u64, todo.len() as u64);
        let bytes = open_opt(cx.store.get(&media_path(sha, name, cx.key()))?, cx.key())?;
        // 받은 그림이 해시와 다르면 버린다 (중간에 깨진 파일)
        if crypto::hex(&crypto::sha256(&bytes)) == *sha {
            let part = cx.media_dir.join(format!(".{name}.part"));
            std::fs::write(&part, bytes).map_err(other)?;
            std::fs::rename(&part, cx.media_dir.join(name)).map_err(other)?;
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------- 합치기 (5.4)

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// 평소 받기
    Sync,
    /// 처음 합칠 때 (K6-m "둘 다 남기고 합치기"): 같은 .apkg 에서 온 노트·복습 기록도 같은 것으로 본다
    Merge,
}

/// 한 행에 온 변경을 모은 것
#[derive(Default)]
struct Fold {
    del: Option<String>,
    /// 가장 새 행 전체와 그 HLC
    row: Option<(String, Map<String, J>)>,
    /// 열 -> (값, HLC). 열마다 가장 큰 HLC
    cols: BTreeMap<String, (J, String)>,
    /// note.fields 의 바탕 HLC (가장 새 fields 를 보낸 변경의 것)
    base: Option<String>,
}

fn fold(recs: Vec<Rec>, aliases: &HashMap<(String, String), String>) -> (HashMap<(String, String), Fold>, String) {
    let mut out: HashMap<(String, String), Fold> = HashMap::new();
    let mut max_h = String::new();
    for rec in recs {
        let uid = alias_of(aliases, &rec.t, &rec.u);
        // 열마다 다른 HLC(k)도 받은 값이다: 그 뒤에 고친 이 기기 변경은 그보다 커야 한다
        for h in std::iter::once(&rec.h).chain(rec.k.iter().flat_map(|k| k.values())) {
            if *h > max_h {
                max_h = h.clone();
            }
        }
        let f = out.entry((rec.t.clone(), uid)).or_default();
        if rec.op == "del" {
            if f.del.as_ref().is_none_or(|d| rec.h > *d) {
                f.del = Some(rec.h);
            }
            continue;
        }
        let Some(r) = rec.r else { continue };
        let names: Vec<String> = rec.c.clone().unwrap_or_else(|| r.keys().cloned().collect());
        for c in names {
            let Some(v) = r.get(&c) else { continue };
            let h = rec.k.as_ref().and_then(|k| k.get(&c)).cloned().unwrap_or_else(|| rec.h.clone());
            if f.cols.get(&c).is_none_or(|(_, old)| h > *old) {
                if c == "fields" {
                    f.base = rec.b.clone();
                }
                f.cols.insert(c.clone(), (v.clone(), h));
            }
        }
        if f.row.as_ref().is_none_or(|(h, _)| rec.h > *h) {
            f.row = Some((rec.h.clone(), r));
        }
    }
    (out, max_h)
}

fn load_aliases(con: &Connection) -> SRes<HashMap<(String, String), String>> {
    let mut st = con.prepare("SELECT table_name, from_uid, to_uid FROM sync_alias").map_err(dbe)?;
    let rows = st.query_map([], |r| Ok(((r.get::<_, String>(0)?, r.get::<_, String>(1)?), r.get::<_, String>(2)?))).map_err(dbe)?;
    rows.collect::<Result<_, _>>().map_err(dbe)
}

fn alias_of(aliases: &HashMap<(String, String), String>, table: &str, uid: &str) -> String {
    let mut u = uid.to_string();
    for _ in 0..8 {
        match aliases.get(&(table.to_string(), u.clone())) {
            Some(to) if *to != u => u = to.clone(),
            _ => break,
        }
    }
    u
}

fn add_aliases(con: &Connection, list: &[J]) -> SRes<()> {
    for a in list {
        if let (Some(t), Some(f), Some(to)) = (a[0].as_str(), a[1].as_str(), a[2].as_str()) {
            con.execute("INSERT OR REPLACE INTO sync_alias (table_name, from_uid, to_uid) VALUES (?1, ?2, ?3)", [t, f, to]).map_err(dbe)?;
        }
    }
    Ok(())
}

/// 같은 것을 가리키는 이 기기의 행 (자연 키). 있으면 (rowid, uid).
/// 유일 인덱스(이름 경로가 같은 덱, 같은 파일 이름의 그림, 같은 날 같은 덱의 계획 …)와 그 밖에 몇 가지:
/// 기본 노트 유형은 종류(native_kind, 옛 앱이 보낸 것은 이름), 가져온 · 사용자 노트 유형은 Anki 모델 id. 처음 합칠 때만 같은 .apkg 의 노트(guid)와 복습 기록(revlog id)
fn natural_match(con: &Connection, table: &Table, vals: &HashMap<String, Value>, mode: Mode) -> SRes<Option<(i64, String)>> {
    let v = |c: &str| vals.get(c).cloned().unwrap_or(Value::Null);
    let mut keys: Vec<Vec<String>> = vec![];
    let own = |cols: &[&str]| cols.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    match table.name {
        // 8.1: 가져온 · 사용자 유형은 모델 id (두 기기에서 따로 만든 같은 이름 유형은 다른 것), 앱 유형은 종류,
        // 종류가 없으면(옛 앱이 보낸 기본 유형) 예전처럼 이름
        "note_type" if v("anki_model_id") != Value::Null => keys.push(own(&["anki_model_id"])),
        "note_type" if v("native_kind") != Value::Null => keys.push(own(&["native_kind", "anki_model_id"])),
        "note_type" => keys.push(own(&["name", "anki_model_id"])),
        "note" if mode == Mode::Merge && v("anki_guid") != Value::Null => keys.push(own(&["anki_guid"])),
        "review_log" if mode == Mode::Merge && v("anki_revlog_id") != Value::Null => keys.push(own(&["anki_revlog_id"])),
        // 같은 DB 를 복사해 따로 쓰다 처음 합칠 때: 앱에서 만든 노트 · 복습 기록 · 세션은 Anki id 가 없다.
        // 같은 ms 에 같은 내용으로 생긴 것은 같은 것이다 (두 기기가 우연히 같을 수 없다)
        "note" if mode == Mode::Merge => keys.push(own(&["note_type_id", "created_at", "fields"])),
        "review_log" if mode == Mode::Merge => keys.push(own(&["card_id", "reviewed_at", "rating"])),
        "study_session" if mode == Mode::Merge => keys.push(own(&["started_at", "study_day"])),
        _ => {}
    }
    keys.extend(table.keys.iter().cloned());
    for key in keys {
        // 값이 비어 있는 열은 자연 키가 되지 못한다. 단 덱의 parent_id (최상위 덱) 와 기본 노트 유형의 모델 id 는 비어 있는 것이 값이다
        if key.iter().any(|c| !vals.contains_key(c) && c != "anki_model_id")
            || key.iter().any(|c| v(c) == Value::Null && c != "parent_id" && c != "anki_model_id")
        {
            continue;
        }
        let cond = key.iter().enumerate().map(|(i, c)| format!("\"{c}\" IS ?{}", i + 1)).collect::<Vec<_>>().join(" AND ");
        let found = con
            .prepare_cached(&format!("SELECT rowid, uid FROM {} WHERE {cond} AND uid IS NOT NULL", table.name))
            .map_err(dbe)?
            .query_row(rusqlite::params_from_iter(key.iter().map(|c| v(c))), |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))
            .optional()
            .map_err(dbe)?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// 이 기기 행의 uid 를 바꾼다 (더 작은 uid 로 합칠 때). 흔적 · 올릴 변경 · 별칭도 따라간다
fn rename_uid(con: &Connection, t: &str, old: &str, new: &str) -> SRes<()> {
    // 처음 합칠 때는 행마다 부른다: 문장을 매번 다시 만들지 않는다
    let run = |sql: &str, args: &[&str]| con.prepare_cached(sql).and_then(|mut st| st.execute(rusqlite::params_from_iter(args))).map_err(dbe);
    run(&format!("UPDATE {t} SET uid = ?2 WHERE uid = ?1"), &[old, new])?;
    run("DELETE FROM sync_row WHERE table_name = ?1 AND uid = ?2", &[t, new])?;
    run("UPDATE sync_row SET uid = ?3 WHERE table_name = ?1 AND uid = ?2", &[t, old, new])?;
    run("UPDATE sync_outbox SET uid = ?3 WHERE table_name = ?1 AND uid = ?2", &[t, old, new])?;
    run("UPDATE sync_alias SET to_uid = ?3 WHERE table_name = ?1 AND to_uid = ?2", &[t, old, new])?;
    run("INSERT OR REPLACE INTO sync_alias (table_name, from_uid, to_uid) VALUES (?1, ?2, ?3)", &[t, old, new])?;
    if t == "note" {
        // K7 기록도 같은 노트를 가리키게 (처음 합칠 때 Drive 쪽 uid 를 따른다)
        run("UPDATE sync_conflict SET note_uid = ?2 WHERE note_uid = ?1", &[old, new])?;
    }
    Ok(())
}

/// 이 기기 파일 경로를 정하는 값 (그림 이름 · 자료 복사본 경로). 변조된 변경 파일이 다른 파일을 가리키면 행을 받지 않는다
fn safe_paths(t: &str, f: &Fold) -> bool {
    let row = f.row.iter().flat_map(|(_, r)| r.iter());
    row.chain(f.cols.iter().map(|(c, (v, _))| (c, v))).all(|(c, v)| match (t, c.as_str()) {
        ("media", "filename") => v.as_str().is_some_and(|n| safe_media_name(n).is_ok()),
        // 동기화 전에 생긴 그림은 비어 있다. 64자리만 경로에 쓰지만 hex 가 아닌 글자는 받지 않는다
        ("media", "sha256") => v.as_str().is_some_and(|s| s.bytes().all(|b| b.is_ascii_hexdigit())),
        ("media", "rel_path") => v.as_str().is_some_and(|p| safe_rel_path(p, "media")),
        ("source_material", "rel_path") => v.as_str().is_some_and(|p| safe_rel_path(p, "sources")),
        _ => true,
    })
}

/// uid 참조 -> 이 기기 rowid. 대상이 없으면 Err(()) (CASCADE 참조면 행을 건너뛴다)
fn resolve_fk(con: &Connection, aliases: &HashMap<(String, String), String>, target: &str, v: &J) -> SRes<Option<Option<i64>>> {
    match v.as_str() {
        None => Ok(Some(None)),
        Some(u) => Ok(rowid_of(con, target, &alias_of(aliases, target, u))?.map(Some)),
    }
}

struct Applier<'a> {
    tx: &'a Transaction<'a>,
    tables: &'a [Table],
    mode: Mode,
    aliases: HashMap<(String, String), String>,
    pending: HashMap<(String, String), Pending>,
    me: String,
    /// 받은 값의 HLC 에 기기 자리가 비었을 때(옛 앱이 만든 스냅샷) K7 에 적을 기기: 그 스냅샷을 쓴 기기
    src: String,
    report: Report,
    /// 새로 들어온 복습 기록의 카드 rowid 와 그 기록 uid
    new_logs: HashSet<i64>,
    new_log_uids: HashSet<String>,
    /// 처음 합치기: Drive 에 있던 행, 이 기기 값이 이긴 행
    remote_rows: HashSet<(String, String)>,
    local_won: HashSet<(String, String)>,
    /// 받은 지우기 (표, uid, HLC): 지울 것과, 나중에 고쳐서 남긴 것 (K29)
    deletes: Vec<(&'static str, String, String)>,
    refused: Vec<(&'static str, String, String)>,
    /// 처음 합치기: 두 기기에서 따로 복습한 카드의 (이 기기 상태, Drive 상태) (T3-06)
    merge_srs: HashMap<i64, (SrsState, SrsState)>,
    /// 받아서 새로 넣은 노트, 새로 받았거나 앞면이 바뀐 카드 종류 (rowid). 끝에 빠진 카드를 만든다 (Q2-04)
    new_notes: HashSet<i64>,
    new_tpls: HashSet<i64>,
    /// 같은 자리에 따로 더한 필드: (노트 유형 rowid, 원래 자리, 새 자리, 진 쪽 필드 id). 끝에 진 쪽 노트 값을 옮긴다 (Q2-06)
    moves: Vec<(i64, usize, usize, String)>,
    /// 이 묶음에 지우기가 온 행. 같은 자연 키로 새 행이 오면 같은 것으로 합치지 않고 지우기를 정한 뒤 다시 넣는다 (K40)
    doomed: HashSet<(String, String)>,
    /// 지우기를 기다리는 행 (지웠다 다시 만든 행과 그것을 가리키는 행)
    parked: HashSet<(String, String)>,
    /// 지웠다 다시 만든 자리의 옛 행
    replaced: HashSet<(String, String)>,
    /// 노트를 나중에 고친 것만 붙잡은 카드 지우기. 같은 자리에 새 카드가 왔으면 지운다 (K40)
    by_note: Vec<(&'static str, String, String)>,
    /// 다시 보낸 refused 의 수 (delete_rows 를 두 번 불러도 한 번만)
    revived: usize,
    /// K41 다른 유형으로 옮긴 노트를 받았다 (rowid): 끝에 새 유형에 카드 종류가 없는 카드를 지운다
    retyped: HashSet<i64>,
    /// K41 같은 노트의 다른 카드가 아직 그 번호에 있어 잠시 비켜 둔 카드 (rowid, 음수 ord). 제 차례에 새 번호로 가지 않으면 끝에 지운다
    displaced: Vec<i64>,
}

/// 같은 자리에 두 기기가 따로 더한 필드: 잃은 것이 없다 (둘 다 남기고 나중 쪽이 그 자리, 다른 쪽은 끝 자리). 화면이 이 제목으로 따로 그린다
pub const KEPT_BOTH: &str = "같은 자리에 두 기기가 필드를 더해 둘 다 남겼어요";

/// 두 기기에서 고친 노트 유형 정의를 남기는 열 (K39): (표, 열, 화면 이름)
const TYPE_COLS: [(&str, &str, &str); 5] =
    [("card_template", "qfmt", "앞면"), ("card_template", "afmt", "뒷면"), ("card_template", "name", "이름"), ("note_type", "css", "CSS"), ("note_type", "name", "유형 이름")];

/// note_type.fields · field_ids 를 자리마다 합친 것 (Q2-06)
#[derive(Default, Debug, PartialEq)]
struct Slots {
    names: Vec<String>,
    ids: Vec<String>,
    /// 같은 자리에 따로 더해 옮긴 진 쪽 필드 (원래 자리, 새 자리, 그 필드 id)
    moved: Vec<(usize, usize, String)>,
    /// 같은 필드를 두 기기가 다르게 바꾼 이름 (이긴 이름, 진 이름)
    renamed: Vec<(String, String)>,
}

/// 자리마다 합친다. 같은 필드(같은 id, 처음부터 있던 필드는 둘 다 빈 id)는 이긴 쪽 이름, 한쪽에만 있는 자리는 그대로.
/// 같은 자리를 두 기기가 따로 더했으면(id 가 다르다) 이긴 쪽이 그 자리를 갖고 진 쪽은 끝 새 자리로 간다 (이긴 쪽이 이미 그 필드를 옮겨 두었으면 그 자리로).
/// 이름까지 같으면 같은 필드로 본다. 두 기기가 같은 (win, lose) 를 보므로 결과가 같다
/// ponytail: 진 쪽 이름이 이긴 쪽의 다른 자리 이름과 같으면 이름이 겹친 채 둔다 (두 기기가 같은 이름을 다른 자리에 동시에 더한 경우)
fn merge_slots(win: &(Vec<String>, Vec<String>), lose: &(Vec<String>, Vec<String>)) -> Slots {
    let id = |s: &(Vec<String>, Vec<String>), i: usize| s.1.get(i).cloned().unwrap_or_default();
    let mut out = Slots::default();
    let mut later = vec![];
    for i in 0..win.0.len().max(lose.0.len()) {
        let (wi, li) = (id(win, i), id(lose, i));
        let elsewhere = win.1.iter().position(|x| !li.is_empty() && *x == li);
        let (name, nid) = match (win.0.get(i), lose.0.get(i)) {
            (Some(w), Some(l)) => {
                if wi != li && !l.is_empty() && l != w {
                    later.push((i, l.clone(), li));
                } else if wi == li && !w.is_empty() && !l.is_empty() && l != w {
                    out.renamed.push((w.clone(), l.clone()));
                }
                (w.clone(), wi)
            }
            (Some(w), None) => (w.clone(), wi),
            // 진 쪽에만 있는 자리. 이긴 쪽이 이미 다른 자리로 옮긴 필드면 빈 자리로 두고 값은 그 자리로
            (None, Some(_)) if elsewhere.is_some() => {
                out.moved.push((i, elsewhere.unwrap_or_default(), li));
                (String::new(), String::new())
            }
            (None, Some(l)) => (l.clone(), li),
            (None, None) => unreachable!(),
        };
        out.names.push(name);
        out.ids.push(nid);
    }
    for (from, name, lid) in later {
        let to = match out.ids.iter().position(|x| *x == lid) {
            Some(j) => j,
            None => {
                out.names.push(name);
                out.ids.push(lid.clone());
                out.names.len() - 1
            }
        };
        out.moved.push((from, to, lid));
    }
    out
}

/// JSON 배열 글 -> 목록 (NULL · 깨진 값은 빈 목록)
fn str_list(v: Option<&J>) -> Vec<String> {
    v.and_then(|v| v.as_str()).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
}

/// field_ids 열 값: 끝의 빈 id 는 뗀다. 모두 비었으면 NULL (더한 필드가 없는 유형 · 옛 앱과 같다)
fn ids_value(ids: &[String]) -> Value {
    let n = ids.iter().rposition(|x| !x.is_empty()).map_or(0, |i| i + 1);
    if n == 0 { Value::Null } else { Value::Text(serde_json::to_string(&ids[..n]).unwrap_or_default()) }
}

enum Step {
    Done,
    /// 부모(덱)가 아직 없다. 다음 바퀴에 다시
    Later,
    /// 지우기를 정한 뒤에 다시 (K40)
    Park,
}

impl Applier<'_> {
    fn pend(&self, t: &str, u: &str) -> Option<&Pending> {
        self.pending.get(&(t.to_string(), u.to_string()))
    }

    /// 들어온 값들을 이 기기 값(rowid 참조)으로. 대상 없는 CASCADE 참조가 있으면 None
    fn local_values(&self, table: &Table, vals: &Map<String, J>) -> SRes<Option<HashMap<String, Value>>> {
        let mut out = HashMap::new();
        for (c, v) in vals {
            if !table.cols.contains(c) {
                continue; // 이 기기에 없는 열 (더 새 앱이 더한 열)
            }
            match table.fks.get(c) {
                Some((target, nullable)) => match resolve_fk(self.tx, &self.aliases, target, v)? {
                    Some(id) => {
                        out.insert(c.clone(), id.map(Value::Integer).unwrap_or(Value::Null));
                    }
                    None if *nullable => {
                        out.insert(c.clone(), Value::Null);
                    }
                    None => return Ok(None),
                },
                None => {
                    out.insert(c.clone(), from_json(v));
                }
            }
        }
        Ok(Some(out))
    }

    fn row(&mut self, table: &Table, uid: String, f: &Fold) -> SRes<Step> {
        let t = table.name;
        if !safe_paths(t, f) {
            return Ok(self.skip());
        }
        // 기다리는 행을 가리키면 같이 기다린다 (새 덱의 카드, 새 카드의 복습 기록)
        let refs = f.row.iter().flat_map(|(_, r)| r.iter()).chain(f.cols.iter().map(|(c, (v, _))| (c, v)));
        if !self.parked.is_empty()
            && refs.filter_map(|(c, v)| Some((table.fks.get(c)?.0, v.as_str()?))).any(|(p, u)| self.parked.contains(&(p.to_string(), alias_of(&self.aliases, p, u))))
        {
            return Ok(Step::Park);
        }
        match rowid_of(self.tx, t, &uid)? {
            Some(rowid) => self.update(table, &uid, rowid, f),
            None => self.insert(table, uid, f),
        }
    }

    fn insert(&mut self, table: &Table, uid: String, f: &Fold) -> SRes<Step> {
        let t = table.name;
        let clock = load_clock(self.tx, t, &uid)?;
        let max_in = f.cols.values().map(|(_, h)| h.clone()).max().unwrap_or_default();
        // 이 기기에서 지운 것(아직 올리지 않았어도 지운 순간의 HLC)과 받아서 지운 것 중 나중
        let tomb = max_of(self.pend(t, &uid).and_then(|p| p.deleted.as_deref()), clock.as_ref().and_then(|c| c.deleted.as_deref())).map(String::from);
        // 지운 쪽이 나중이면 넣지 않는다. 고친 쪽이 나중이면 되살린다 (5.4 삭제)
        if let Some(d) = &f.del
            && *d >= max_in
        {
            let mut c = clock.unwrap_or_default();
            if c.deleted.as_ref().is_none_or(|old| d > old) {
                c.deleted = Some(d.clone());
                if c.base.is_empty() {
                    c.base = "0".into();
                }
                save_clock(self.tx, t, &uid, &c)?;
            }
            return Ok(Step::Done);
        }
        if tomb.as_ref().is_some_and(|d| *d >= max_in) {
            self.report.skipped += 1;
            return Ok(Step::Done);
        }
        let Some((row_h, row)) = &f.row else { return Ok(Step::Done) };
        let mut merged = row.clone();
        for (c, (v, _)) in &f.cols {
            merged.insert(c.clone(), v.clone());
        }
        let Some(vals) = self.local_values(table, &merged)? else {
            // 부모가 없다: 덱은 부모가 같은 묶음 뒤쪽에 있을 수 있어 다음 바퀴에, 나머지는 지워진 부모라 건너뛴다
            return Ok(if t == "deck" { Step::Later } else { self.skip() });
        };
        // K41 다른 유형으로 옮긴 노트에 새 유형의 카드 종류가 없는 번호의 카드가 왔다: 옮긴 것을 모르고 옛 유형으로 만든 카드다. 넣지 않는다
        // (그 기기도 옮긴 것을 받으면 지운다, settle_retyped). 옮기지 않은 노트는 그대로 둔다 (K37 의 남은 카드가 카드 종류를 붙잡는 길)
        if t == "card"
            && let (Some(Value::Integer(note)), Some(Value::Integer(ord))) = (vals.get("note_id"), vals.get("template_ord"))
            && self.orphan_in_moved(*note, *ord)?
        {
            return Ok(self.skip());
        }
        if let Some((rowid, local_uid)) = natural_match(self.tx, table, &vals, self.mode)? {
            // K41 다른 유형으로 옮긴 카드(번호가 바뀌었다)와 같은 자리의 다른 카드는 옛 유형의 카드다: 합치지 않는다 (일정이 섞인다)
            if t == "card" && self.moved_card(&local_uid)? {
                return Ok(self.skip());
            }
            // 이 묶음이 그 행을 지운다: 지웠다 다시 만든 것(빈칸 c2 를 지웠다 다시 넣기, 같은 이름의 덱)이라 같은 것이 아니다.
            // 합치면 옛 행의 지우기가 새 행까지 지운다. 지우기를 정한 뒤 다시: 지웠으면 새 행으로, 남았으면 그때 합친다 (K40)
            if self.doomed.contains(&(t.to_string(), local_uid.clone())) {
                self.replaced.insert((t.to_string(), local_uid));
                return Ok(Step::Park);
            }
            if t == "daily_plan" {
                return self.daily_plan(table, &uid, rowid, &local_uid, &vals, row_h);
            }
            // 같은 것: 작은 uid 로 합치고 열 단위로 맞춘다. 모든 기기가 같은 쪽을 고르므로 별칭도 같아진다.
            // 단 처음 합칠 때 Drive 가 모르는 이 기기 uid 는 버리고 Drive 쪽을 따른다: 별칭은 이 기기에만 남아서,
            // 이 기기 uid 로 올리면 처음 합치기 전용 자연 키(guid · K13)가 없는 다른 기기에는 새 행으로 두 벌이 된다
            let keep = if uid < local_uid || (self.mode == Mode::Merge && !self.remote_rows.contains(&(t.to_string(), local_uid.clone()))) {
                rename_uid(self.tx, t, &local_uid, &uid)?;
                self.rekey_pending(t, &local_uid, &uid);
                self.aliases.insert((t.into(), local_uid.clone()), uid.clone());
                uid.clone()
            } else {
                self.tx.execute("INSERT OR REPLACE INTO sync_alias (table_name, from_uid, to_uid) VALUES (?1, ?2, ?3)", [t, &uid, &local_uid]).map_err(dbe)?;
                self.aliases.insert((t.into(), uid.clone()), local_uid.clone());
                local_uid.clone()
            };
            return self.update(table, &keep, rowid, f);
        }
        let names: Vec<&String> = vals.keys().collect();
        let sql = format!(
            "INSERT INTO {t} (uid, {}) VALUES (?1, {})",
            names.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
            (0..names.len()).map(|i| format!("?{}", i + 2)).collect::<Vec<_>>().join(", ")
        );
        let mut args: Vec<Value> = vec![Value::Text(uid.clone())];
        args.extend(names.iter().map(|c| vals[*c].clone()));
        if self.tx.execute(&sql, rusqlite::params_from_iter(args)).is_err() {
            // 다른 제약(유일 키 등)에 걸린 행. 묶음 전체를 실패시키지 않고 건너뛴다
            return Ok(self.skip());
        }
        let new = self.tx.last_insert_rowid();
        match t {
            "note" => self.new_notes.insert(new),
            "card_template" => self.new_tpls.insert(new),
            _ => false,
        };
        let mut c = Clock { base: row_h.clone(), ..Default::default() };
        for (col, (_, h)) in &f.cols {
            if *h != c.base {
                c.cols.insert(col.clone(), h.clone());
            }
        }
        save_clock(self.tx, t, &uid, &c)?;
        if t == "review_log"
            && let Some(Value::Integer(card)) = vals.get("card_id")
        {
            self.new_logs.insert(*card);
            self.new_log_uids.insert(uid);
        }
        self.report.applied += 1;
        Ok(Step::Done)
    }

    /// 다른 유형으로 옮겨 번호(template_ord)가 바뀐 카드인가 (번호는 그 밖에는 바뀌지 않는다, NT21)
    fn moved_card(&self, uid: &str) -> SRes<bool> {
        if self.pend("card", uid).is_some_and(|p| p.col("template_ord").is_some()) {
            return Ok(true);
        }
        Ok(load_clock(self.tx, "card", uid)?.is_some_and(|c| c.cols.contains_key("template_ord")))
    }

    /// K41 다른 유형으로 옮긴 노트(유형 열이 바뀌었다)인데 그 유형(빈칸 유형 아님)에 이 번호의 카드 종류가 없다
    fn orphan_in_moved(&self, note: i64, ord: i64) -> SRes<bool> {
        let Some(uid) = self.tx.query_row("SELECT uid FROM note WHERE id = ?1", [note], |r| r.get::<_, Option<String>>(0)).optional().map_err(dbe)?.flatten() else {
            return Ok(false);
        };
        let moved = self.pend("note", &uid).is_some_and(|p| p.col("note_type_id").is_some())
            || load_clock(self.tx, "note", &uid)?.is_some_and(|c| c.cols.contains_key("note_type_id"));
        if !moved {
            return Ok(false);
        }
        self.tx
            .query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM note x JOIN card_template t ON t.note_type_id = x.note_type_id WHERE x.id = ?1 AND (t.ord = ?2 OR t.qfmt LIKE '%{{cloze:%'))",
                params![note, ord],
                |r| r.get(0),
            )
            .map_err(dbe)
    }

    /// K41 같은 노트에서 그 번호를 차지한 다른 카드를 음수 번호로 비켜 둔다. 비켰으면 true
    fn displace(&mut self, rowid: i64, to: &Value) -> SRes<bool> {
        let Value::Integer(to) = to else { return Ok(false) };
        let other: Option<i64> = self
            .tx
            .query_row(
                "SELECT o.rowid FROM card o JOIN card c ON c.note_id = o.note_id WHERE c.rowid = ?1 AND o.template_ord = ?2 AND o.rowid <> ?1",
                params![rowid, to],
                |r| r.get(0),
            )
            .optional()
            .map_err(dbe)?;
        let Some(other) = other else { return Ok(false) };
        self.tx.execute("UPDATE card SET template_ord = -1 - rowid WHERE rowid = ?1", [other]).map_err(dbe)?;
        self.displaced.push(other);
        Ok(true)
    }

    /// K41 다른 유형으로 옮긴 노트를 받은 뒤: 새 유형에 카드 종류가 없는 그 노트의 카드(옮긴 것을 모르고 이 기기가 옛 유형으로 만든 카드)와
    /// 제 번호로 가지 못한 비켜 둔 카드를 지운다. 이 기기의 변경으로 올린다 (다른 기기에도 없게). 복습 기록은 통계에 남는다 (A4)
    fn settle_retyped(&mut self) -> SRes<()> {
        let mut notes: BTreeSet<i64> = std::mem::take(&mut self.retyped).into_iter().collect();
        for c in std::mem::take(&mut self.displaced) {
            if let Some(n) = self.tx.query_row("SELECT note_id FROM card WHERE rowid = ?1", [c], |r| r.get::<_, i64>(0)).optional().map_err(dbe)? {
                notes.insert(n);
            }
        }
        if notes.is_empty() {
            return Ok(());
        }
        applying(self.tx, false)?;
        for n in notes {
            let gone = self
                .tx
                .execute(
                    "DELETE FROM card WHERE note_id = ?1 AND (template_ord < 0 OR (
                       NOT EXISTS (SELECT 1 FROM note x JOIN card_template t ON t.note_type_id = x.note_type_id WHERE x.id = ?1 AND t.qfmt LIKE '%{{cloze:%')
                       AND NOT EXISTS (SELECT 1 FROM note x JOIN card_template t ON t.note_type_id = x.note_type_id WHERE x.id = ?1 AND t.ord = card.template_ord)))",
                    [n],
                )
                .map_err(dbe)?;
            self.report.skipped += gone;
        }
        applying(self.tx, true)
    }

    fn skip(&mut self) -> Step {
        self.report.skipped += 1;
        Step::Done
    }

    fn rekey_pending(&mut self, t: &str, old: &str, new: &str) {
        if let Some(p) = self.pending.remove(&(t.to_string(), old.to_string())) {
            self.pending.insert((t.to_string(), new.to_string()), p);
        }
    }

    /// 같은 날 같은 덱의 계획이 두 기기에 있으면 먼저 만든 쪽 (그날 처음 계산한 계획)
    fn daily_plan(&mut self, table: &Table, uid: &str, rowid: i64, local_uid: &str, vals: &HashMap<String, Value>, h: &str) -> SRes<Step> {
        let local_created: i64 = self.tx.query_row("SELECT created_at FROM daily_plan WHERE rowid = ?1", [rowid], |r| r.get(0)).map_err(dbe)?;
        let incoming = match vals.get("created_at") {
            Some(Value::Integer(i)) => *i,
            _ => i64::MAX,
        };
        if (incoming, uid) < (local_created, local_uid) {
            rename_uid(self.tx, table.name, local_uid, uid)?;
            for (c, v) in vals {
                self.tx.execute(&format!("UPDATE daily_plan SET \"{c}\" = ?2 WHERE rowid = ?1"), params![rowid, v]).map_err(dbe)?;
            }
            save_clock(self.tx, table.name, uid, &Clock { base: h.to_string(), ..Default::default() })?;
            self.report.applied += 1;
        } else {
            self.tx.execute("INSERT OR REPLACE INTO sync_alias (table_name, from_uid, to_uid) VALUES ('daily_plan', ?1, ?2)", [uid, local_uid]).map_err(dbe)?;
        }
        Ok(Step::Done)
    }

    /// 그 행을 다른 기기(by 가 아닌 기기)가 마지막으로 고친 HLC: 받은 값 · 아직 올리지 않은 이 기기 변경 중 나중.
    /// 지운 기기 자신이 고친 것은 빼고 본다: 같은 트랜잭션에서 고쳤든(빈칸 지우기는 노트도 고친다, Q2-03) 지운 뒤에 고쳤든 그 기기는 지운 것을 안다
    fn newest(&self, t: &str, uid: &str, by: &str) -> SRes<String> {
        let clock = match load_clock(self.tx, t, uid)? {
            Some(c) => c,
            None => match (self.tables.iter().find(|x| x.name == t), rowid_of(self.tx, t, uid)?) {
                (Some(table), Some(id)) => export(self.tx, table, id)?.map(|(_, r)| Clock::implicit(&r, &self.me)).unwrap_or_default(),
                _ => Clock::default(),
            },
        };
        let others = clock.cols.values().chain(std::iter::once(&clock.base)).filter(|h| hlc_device(h) != by).max().cloned().unwrap_or_default();
        Ok(max_of(Some(others.as_str()), self.pend(t, uid).and_then(|p| p.newest())).unwrap_or_default().to_string())
    }

    fn update(&mut self, table: &Table, uid: &str, rowid: i64, f: &Fold) -> SRes<Step> {
        let t = table.name;
        let Some((_, local)) = export(self.tx, table, rowid)? else { return Ok(Step::Done) };
        let stored = load_clock(self.tx, t, uid)?;
        // 처음 합치기의 시계 없는 이 기기 행 (연결 전에 생겼다 · 연결을 끊었다 다시 합친다): 값이 Drive 와 같은 열은 Drive 의 HLC 로 (K45).
        // 이 기기 HLC(행 시각 + 이 기기 id)로 두면 같은 값인데 Drive 것보다 나중으로 읽혀, 다른 기기가 그 뒤 고친 내용을 K41 이 버렸다 (QA T5-01)
        let fresh = stored.is_none() && self.mode == Mode::Merge;
        let mut clock = stored.unwrap_or_else(|| Clock::implicit(&local, &self.me));
        let pend = self.pend(t, uid).cloned();
        if t == "review_log" {
            // 복습 기록은 바뀌지 않는다 (합집합). 단 카드를 지웠다가 되살려 받은 기기에서 비어 버린 카드 참조는 채운다 (K29)
            if local.get("card_id").is_some_and(|v| v.is_null())
                && let Some((v, _)) = f.cols.get("card_id")
                && let Some(Some(card)) = resolve_fk(self.tx, &self.aliases, "card", v)?
            {
                self.tx.execute("UPDATE review_log SET card_id = ?2 WHERE rowid = ?1", params![rowid, card]).map_err(dbe)?;
            }
        } else {
            let mut sets: Vec<(String, J, String)> = vec![];
            let mut srs_differs = false;
            // K41 다른 유형으로 옮긴 노트의 자리 배열(fields)은 새 유형의 자리다. 유형 열과 내용 열을 따로 겨루면 옛 유형 자리로 쓴 내용이
            // 새 유형 아래 엉뚱한 필드에 보인다. 그래서 내용은 유형을 정한 쪽 것으로: 받은 쪽이 나중에 옮겼으면 받은 내용 (Some(true)),
            // 받은 쪽이 이 기기가 옮긴 것을 모르고 고쳤으면(바탕이 옮긴 것보다 옛것) 이 기기 내용 (Some(false)). 진 쪽 내용은 K7 에 남는다.
            // 처음 합치기(Merge)는 바탕이 없어 보지 않는다
            // 받은 쪽이 옮긴 것이 이기면 그 HLC (내용 열이 바뀌지 않은 옮기기도 행 전체의 내용을 쓴다)
            let mut moved_at: Option<String> = None;
            let fields_rule: Option<bool> = if t == "note" && self.mode == Mode::Sync {
                let lt = col_hlc(&clock, pend.as_ref(), "note_type_id");
                let moved_in = f.cols.get("note_type_id").filter(|(v, _)| {
                    let v = v.as_str().map(|u| J::String(alias_of(&self.aliases, "note_type", u))).unwrap_or_else(|| v.clone());
                    local.get("note_type_id") != Some(&v)
                });
                match moved_in {
                    Some((_, h)) => {
                        if *h > lt {
                            moved_at = Some(h.clone());
                        }
                        Some(*h > lt)
                    }
                    // 이 기기가 옮긴 적이 있을 때만 (유형 열 HLC 가 행 기본값과 따로 있다). 행 기본값은 만든 · 합친 시각이지 옮긴 시각이 아니다 (K45)
                    None if f.cols.contains_key("fields") => {
                        let moved_here = max_of(clock.cols.get("note_type_id").map(String::as_str), pend.as_ref().and_then(|p| p.col("note_type_id")));
                        f.base.as_deref().filter(|b| *b != "0" && moved_here.is_some_and(|m| *b < m)).map(|_| false)
                    }
                    None => None,
                }
            } else {
                None
            };
            // K7 기록의 버전마다 노트 유형: 받은 쪽은 그 기기 행의 유형 (옮겼으면 받은 유형 열), 이 기기는 지금 유형.
            // 받은 쪽이 이 기기가 옮긴 것을 모르고 고쳤으면 옮기기 전 유형이다: 그 uid 는 여기 없어 "?" (어느 유형과도 다르다)
            let type_of = |v: Option<&J>| v.and_then(|v| v.as_str()).map(|u| alias_of(&self.aliases, "note_type", u)).unwrap_or_default();
            let note_types = if t == "note" {
                let here = type_of(local.get("note_type_id"));
                let there = match f.cols.get("note_type_id").map(|(v, _)| v).or_else(|| f.row.as_ref().and_then(|(_, r)| r.get("note_type_id"))) {
                    Some(v) => type_of(Some(v)),
                    None if fields_rule == Some(false) => "?".into(),
                    None => here.clone(),
                };
                (there, here)
            } else {
                Default::default()
            };
            let types = (note_types.0.as_str(), note_types.1.as_str());
            // 필드 목록은 열 하나로 통째로 겨루지 않고 자리마다 합친다 (Q2-06)
            let merged = t == "note_type" && f.cols.contains_key("fields");
            if merged {
                self.merge_fields(uid, rowid, &local, &mut clock, pend.as_ref(), f)?;
            }
            for (c, (v, h)) in &f.cols {
                if !table.cols.contains(c) || (merged && (c == "fields" || c == "field_ids")) {
                    continue;
                }
                // 이 기기 값의 HLC: 받은 값이거나, 아직 올리지 않았으면 고친 순간 (K28)
                let lh = col_hlc(&clock, pend.as_ref(), c);
                // 참조는 별칭을 거친 uid 로 비교한다 (합쳐진 덱을 가리키는 옛 uid)
                let v = &match (table.fks.get(c), v.as_str()) {
                    (Some((target, _)), Some(u)) => J::String(alias_of(&self.aliases, target, u)),
                    _ => v.clone(),
                };
                let same = local.get(c) == Some(v);
                // K41 (위): 나중에 고친 쪽이 아니라 유형을 정한 쪽 내용으로. 그대로 겨뤄도 같은 결과면 아래 보통 길
                if c == "fields"
                    && !same
                    && let Some(take) = fields_rule
                    && take != (*h > lh)
                {
                    self.conflict(uid, h, v, &lh, local.get(c), !take, types)?;
                    if take {
                        sets.push((c.clone(), v.clone(), h.clone()));
                    }
                    continue;
                }
                srs_differs |= !same && t == "card" && SRS_COLS.contains(&c.as_str());
                // 노트 유형 정의를 이 기기도 고쳤는데 아직 올리지 않았다: 받은 쪽은 이 기기 것을 모르고 고쳤다. 진 쪽을 남긴다 (K39)
                if !same
                    && let Some((_, _, label)) = TYPE_COLS.iter().find(|(tt, cc, _)| *tt == t && cc == c)
                    && pend.as_ref().and_then(|p| p.col(c)).is_some()
                {
                    let (mine, theirs) = (local.get(c).and_then(|x| x.as_str()).unwrap_or_default(), v.as_str().unwrap_or_default());
                    let (win, lose) = if *h > lh { ((theirs, h.as_str()), (mine, lh.as_str())) } else { ((mine, lh.as_str()), (theirs, h.as_str())) };
                    self.type_conflict(t, uid, &local, label, win, lose)?;
                }
                if *h > lh {
                    if !same {
                        // 받은 쪽이 이 기기 값을 모르고 고쳤으면(바탕이 더 옛것) 동시에 고친 것이다. 처음 합칠 때는 바탕이 없다: 내용이 다르면 남긴다
                        if t == "note" && c == "fields" && hlc_ms(&lh) > 0 && (self.mode == Mode::Merge || f.base.as_ref().is_none_or(|b| *b < lh)) {
                            self.conflict(uid, h, v, &lh, local.get(c), false, types)?;
                        }
                        sets.push((c.clone(), v.clone(), h.clone()));
                    } else {
                        clock.cols.insert(c.clone(), h.clone());
                    }
                } else if !same {
                    if t == "note" && c == "fields" {
                        self.conflict(uid, h, v, &lh, local.get(c), true, types)?;
                    }
                    if self.mode == Mode::Merge {
                        self.local_won.insert((t.into(), uid.to_string()));
                    }
                } else if fresh {
                    clock.cols.insert(c.clone(), h.clone());
                }
            }
            if srs_differs && self.mode == Mode::Merge {
                let incoming = |c: &str| f.cols.get(c).map(|(v, _)| v).or_else(|| f.row.as_ref().and_then(|(_, r)| r.get(c)));
                self.merge_srs.insert(rowid, (srs_of(|c| local.get(c)), srs_of(incoming)));
            }
            // K41 (위): 옮기면서 자리 배열이 그대로였다 (같은 자리로 옮김). 받은 행의 내용으로 맞춘다 (이 기기가 옮긴 것을 모르고 고친 내용은 진다)
            if !f.cols.contains_key("fields")
                && let (Some(h), Some((_, row))) = (&moved_at, &f.row)
                && let Some(v) = row.get("fields")
                && local.get("fields") != Some(v)
            {
                let lh = col_hlc(&clock, pend.as_ref(), "fields");
                self.conflict(uid, h, v, &lh, local.get("fields"), false, types)?;
                sets.push(("fields".into(), v.clone(), h.clone()));
            }
            let applied = !sets.is_empty();
            if t == "note" && sets.iter().any(|(c, _, _)| c == "note_type_id") {
                self.retyped.insert(rowid);
            }
            // 앞면이 바뀐 카드 종류: 보낸 기기가 모르던 이 기기 노트에 카드가 생길 수 있다 (Q2-04)
            if t == "card_template" && sets.iter().any(|(c, _, _)| c == "qfmt") {
                self.new_tpls.insert(rowid);
            }
            for (c, v, h) in sets {
                let val = match table.fks.get(&c) {
                    Some((target, nullable)) => match resolve_fk(self.tx, &self.aliases, target, &v)? {
                        Some(id) => id.map(Value::Integer).unwrap_or(Value::Null),
                        None if *nullable => Value::Null,
                        None => continue, // 옮겨 간 덱이 이 기기에 없다: 이 열은 그대로
                    },
                    None => from_json(&v),
                };
                match self.tx.execute(&format!("UPDATE {t} SET \"{c}\" = ?2 WHERE rowid = ?1"), params![rowid, val]) {
                    Ok(_) => {
                        clock.cols.insert(c, h);
                    }
                    // 이 묶음이 지울 행이 아직 그 이름(유일 키)을 차지했다 (지운 덱 이름으로 바꾸기): 지운 뒤 다시
                    // K41 다른 유형으로 옮기며 번호를 맞바꿨다: 같은 노트의 다른 카드가 아직 그 번호에 있다. 그 카드를 잠시 음수 번호로 비켜 두고 다시 쓴다.
                    // 비켜 둔 카드는 이 묶음의 제 차례에 새 번호로 간다 (가지 않으면 끝에 지운다, settle_retyped)
                    Err(_) if t == "card" && c == "template_ord" && self.displace(rowid, &val)? => {
                        if self.tx.execute("UPDATE card SET template_ord = ?2 WHERE rowid = ?1", params![rowid, val]).is_ok() {
                            clock.cols.insert(c, h);
                        } else if !self.doomed.is_empty() {
                            return Ok(Step::Park);
                        }
                    }
                    Err(_) if !self.doomed.is_empty() => return Ok(Step::Park),
                    Err(_) => {}
                }
            }
            if applied {
                self.report.applied += 1;
            }
        }
        if fresh {
            // 위에서 정하지 않은 열(이 기기 값이 이긴 열 · Drive 가 보내지 않은 열)은 이 기기 HLC 그대로, 행 기본값은 그중 가장 작은 것
            let mine = clock.base.clone();
            for c in &table.cols {
                clock.cols.entry(c.clone()).or_insert_with(|| mine.clone());
            }
            clock.base = clock.cols.values().min().cloned().unwrap_or(mine);
        }
        // 열 HLC 를 행 기본값보다 작게 남길 필요는 없다
        let base = clock.base.clone();
        clock.cols.retain(|_, h| *h > base);
        save_clock(self.tx, t, uid, &clock)?;
        if let Some(d) = &f.del {
            // 지운 것이 그 행을 마지막으로 고친 것보다 나중일 때만 지운다. 카드는 노트(앞뒤 내용)를 고친 것도 그 카드를 고친 것이다 (K29).
            // 지운 기기가 이미 받은 수정이면 지운 HLC 가 더 크다 (고친 순간의 HLC 는 받은 값보다 크다): 다른 기기가 모르고 나중에 고친 것만 남긴다.
            // 지운 기기 자신이 고친 것은 보지 않는다 (같은 트랜잭션의 노트 수정 · 지운 뒤 고친 것, K37). 같은 HLC 는 같은 기기의 같은 동작이라 지운다 (K40)
            let by = hlc_device(d);
            let own = self.newest(t, uid, by)?;
            let note = match (t, local.get("note_id").and_then(|v| v.as_str())) {
                ("card", Some(n)) => Some(self.newest("note", n, by)?),
                _ => None,
            };
            if *d >= own && note.as_ref().is_none_or(|n| d >= n) {
                self.deletes.push((t, uid.to_string(), d.clone()));
            } else if *d >= own {
                self.by_note.push((t, uid.to_string(), d.clone()));
            } else if t != "review_log" {
                self.refused.push((t, uid.to_string(), d.clone()));
            }
        }
        Ok(Step::Done)
    }

    /// K7: 같은 노트 내용을 두 기기에서 고쳤다. 진 쪽 내용을 남긴다. 시각은 고친 순간 (K28)
    /// types: (받은 버전의 노트 유형 uid, 이 기기 버전의 노트 유형 uid). [이 버전으로 바꾸기]가 다른 유형의 자리 배열을 넣지 않게 남긴다 (K41)
    #[allow(clippy::too_many_arguments)]
    fn conflict(&mut self, note_uid: &str, h: &str, incoming: &J, lh: &str, local: Option<&J>, local_wins: bool, types: (&str, &str)) -> SRes<()> {
        let local_dev = hlc_device(lh).to_string();
        // 옛 앱이 만든 스냅샷은 HLC 에 기기가 없다: 그 스냅샷을 쓴 기기
        let in_dev = match hlc_device(h) {
            "" => self.src.clone(),
            d => d.to_string(),
        };
        let local_fields = local.and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let in_fields = incoming.as_str().unwrap_or_default().to_string();
        let (wd, wa, wf, wt, ld, la, lf, lt) = if local_wins {
            (local_dev, hlc_ms(lh), local_fields, types.1, in_dev, hlc_ms(h), in_fields, types.0)
        } else {
            (in_dev, hlc_ms(h), in_fields, types.0, local_dev, hlc_ms(lh), local_fields, types.1)
        };
        let ty = |t: &str| (!t.is_empty()).then(|| t.to_string());
        self.tx
            .execute(
                "INSERT INTO sync_conflict (note_uid, at, winner_device, winner_at, winner_fields, loser_device, loser_at, loser_fields, winner_type, loser_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![note_uid, db::now_ms(), wd, wa, wf, ld, la, lf, ty(wt), ty(lt)],
            )
            .map_err(dbe)?;
        self.report.conflicts += 1;
        Ok(())
    }

    /// K39: 노트 유형 정의를 두 기기에서 고쳤다. 진 쪽을 남긴다 (노트 유형 편집기가 보인다). win · lose = (내용, HLC)
    fn type_conflict(&mut self, t: &str, uid: &str, local: &Map<String, J>, label: &str, win: (&str, &str), lose: (&str, &str)) -> SRes<()> {
        let (type_uid, what) = if t == "card_template" {
            let tpl = local.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            (local.get("note_type_id").and_then(|v| v.as_str()).unwrap_or_default().to_string(), format!("카드 종류 '{tpl}' {label}"))
        } else {
            (uid.to_string(), label.to_string())
        };
        let dev = |h: &str| match hlc_device(h) {
            "" => self.src.clone(),
            d => d.to_string(),
        };
        self.tx
            .execute(
                "INSERT INTO sync_type_conflict (type_uid, what, at, winner_device, winner, loser_device, loser) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![type_uid, what, db::now_ms(), dev(win.1), win.0, dev(lose.1), lose.0],
            )
            .map_err(dbe)?;
        Ok(())
    }

    /// note_type.fields · field_ids 를 자리마다 합쳐 쓴다 (merge_slots, Q2-06). 열 하나로 통째로 겨루면 다른 기기가 더한 필드가 사라진다.
    /// 합친 것이 받은 값과 다르면(이 기기 쪽 필드를 살렸다) 이 기기의 변경으로 올려 다른 기기도 같아진다. 진 쪽 노트 값은 끝에 옮긴다 (move_values)
    fn merge_fields(&mut self, uid: &str, rowid: i64, local: &Map<String, J>, clock: &mut Clock, pend: Option<&Pending>, f: &Fold) -> SRes<()> {
        let Some((v, h)) = f.cols.get("fields") else { return Ok(()) };
        let ids_in = f.cols.get("field_ids").map(|(x, _)| x).or_else(|| f.row.as_ref().and_then(|(_, r)| r.get("field_ids")));
        let theirs = (str_list(Some(v)), str_list(ids_in));
        let mine = (str_list(local.get("fields")), str_list(local.get("field_ids")));
        let lh = col_hlc(clock, pend, "fields");
        let incoming = *h > lh;
        let (win, lose, wh, loh) = if incoming { (&theirs, &mine, h.as_str(), lh.as_str()) } else { (&mine, &theirs, lh.as_str(), h.as_str()) };
        let m = merge_slots(win, lose);
        let same_as = |s: &(Vec<String>, Vec<String>)| s.0 == m.names && ids_value(&s.1) == ids_value(&m.ids);
        if incoming {
            clock.cols.insert("fields".into(), h.clone());
            if let Some((_, ih)) = f.cols.get("field_ids") {
                clock.cols.insert("field_ids".into(), ih.clone());
            }
        }
        if !same_as(&mine) {
            // 받은 그대로면 받은 값, 아니면 합친 값을 이 기기의 변경으로 (트리거가 쌓는다)
            let local_change = !same_as(&theirs);
            if local_change {
                applying(self.tx, false)?;
            }
            self.tx
                .execute(
                    "UPDATE note_type SET fields = ?2, field_ids = ?3 WHERE rowid = ?1",
                    params![rowid, serde_json::to_string(&m.names).map_err(other)?, ids_value(&m.ids)],
                )
                .map_err(dbe)?;
            if local_change {
                applying(self.tx, true)?;
            }
            self.report.applied += 1;
        }
        // 진 쪽 기록 (K39): 같은 필드 이름을 이 기기도 바꿨는데 아직 올리지 않았다 · 같은 자리에 따로 더한 필드는 새 자리로 옮겼다
        if pend.and_then(|p| p.col("fields")).is_some() {
            for (w, l) in &m.renamed {
                self.type_conflict("note_type", uid, local, "필드 이름", (w, wh), (l, loh))?;
            }
        }
        for (from, to, id) in m.moved {
            let at_from = m.names.get(from).cloned().unwrap_or_default();
            let moved = m.names.get(to).cloned().unwrap_or_default();
            self.type_conflict("note_type", uid, local, KEPT_BOTH, (&at_from, wh), (&moved, loh))?;
            self.moves.push((rowid, from, to, id));
        }
        Ok(())
    }

    /// 같은 자리에 따로 더한 필드의 진 쪽 노트 값을 그 필드의 새 자리로 (Q2-06). 그 기기가 필드를 더한 뒤 쓴 노트만:
    /// 노트 내용의 HLC 가 그 기기 것이고 필드를 더한 시각(id) 뒤. 이긴 쪽 노트 · 더하기 전 노트는 그 자리에 진 쪽 값이 없다.
    /// 이 기기의 변경으로 올린다 (두 기기가 같은 노트를 같은 값으로 옮긴다)
    /// ponytail: 진 쪽 정의를 받은 세 번째 기기가 쓴 노트는 옮기지 않는다 (그 기기 노트의 값은 이긴 쪽 이름 아래 보인다)
    fn move_values(&mut self) -> SRes<()> {
        let moves = std::mem::take(&mut self.moves);
        if moves.is_empty() {
            return Ok(());
        }
        applying(self.tx, false)?;
        for (ty, from, to, id) in moves {
            let (dev, born) = (hlc_device(&id), hlc_ms(&id));
            let notes: Vec<(i64, String, String)> = {
                let mut st = self.tx.prepare_cached("SELECT id, uid, fields FROM note WHERE note_type_id = ?1 AND uid IS NOT NULL").map_err(dbe)?;
                st.query_map([ty], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
            };
            for (nid, nuid, fields) in notes {
                let clock = load_clock(self.tx, "note", &nuid)?.unwrap_or_default();
                let h = col_hlc(&clock, self.pend("note", &nuid), "fields");
                let mut v: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
                if hlc_device(&h) != dev || hlc_ms(&h) < born || v.get(from).is_none_or(|x| x.is_empty()) {
                    continue;
                }
                v.resize(v.len().max(to + 1), String::new());
                v[to] = std::mem::take(&mut v[from]);
                self.tx.execute("UPDATE note SET fields = ?2 WHERE id = ?1", params![nid, serde_json::to_string(&v).map_err(other)?]).map_err(dbe)?;
            }
        }
        applying(self.tx, true)
    }

    /// 그 행을 마지막으로 정한 HLC 와 그것이 이 기기 결정인가: 노트는 내용(fields), 카드 종류는 앞면(qfmt).
    /// 아직 올리지 않은 이 기기 변경이면 (None, true): 다른 기기는 아무도 모른다
    fn decided(&self, t: &str, uid: &str, col: &str) -> SRes<(Option<String>, bool)> {
        if self.pend(t, uid).is_some_and(|p| p.created || p.col(col).is_some()) {
            return Ok((None, true));
        }
        let h = load_clock(self.tx, t, uid)?.map(|c| c.col(col).to_string()).unwrap_or_default();
        let mine = hlc_device(&h) == self.me;
        Ok((Some(h), mine))
    }

    /// 받은 노트 · 카드 종류로 빠진 카드를 만든다 (Q2-04, 앞면이 비지 않으면). 카드 (노트 N, 카드 종류 T) 는 N 을 쓴 기기가 T 를 알았거나
    /// T 를 쓴 기기가 N 을 알았으면 이미 정해졌다 (만들었거나, 앞면이 비었거나, 지웠다). 둘 다 몰랐을 때만 만든다.
    /// 몰랐다는 것은 이 기기가 그 결정을 한 쪽일 때만 안다: 받은 것은 이 기기가 처음 보는 것이고, HLC 가 작은 쪽은 큰 쪽을 볼 수 없었다
    /// (받은 뒤에 고친 것은 늘 받은 것보다 크다, K28). 그래서 결정한 기기가 만들고 이 기기의 변경으로 올린다. 앱 유형은 앱이 카드를 만든다
    fn fill_cards(&mut self) -> SRes<()> {
        let notes = std::mem::take(&mut self.new_notes);
        let tpls = std::mem::take(&mut self.new_tpls);
        if notes.is_empty() && tpls.is_empty() {
            return Ok(());
        }
        let mut want: BTreeMap<i64, BTreeSet<u32>> = BTreeMap::new();
        let rows = |sql: &str, id: i64| -> SRes<Vec<(i64, String, u32, String)>> {
            let mut st = self.tx.prepare_cached(sql).map_err(dbe)?;
            st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)
        };
        // 받은 카드 종류 T × 이 기기가 정한 노트 N: T 를 쓴 기기가 N 을 몰랐다 (N 이 아직 올리지 않은 것이거나 T 보다 나중)
        for t in &tpls {
            let list = rows(
                "SELECT n.id, n.uid, t.ord, t.uid FROM card_template t JOIN note_type nt ON nt.id = t.note_type_id JOIN note n ON n.note_type_id = nt.id
                 WHERE t.rowid = ?1 AND nt.native_kind IS NULL AND n.uid IS NOT NULL AND t.uid IS NOT NULL",
                *t,
            )?;
            for (nid, nuid, ord, tuid) in list {
                let (nh, mine) = self.decided("note", &nuid, "fields")?;
                let (th, _) = self.decided("card_template", &tuid, "qfmt")?;
                if mine && (nh.is_none() || th.zip(nh).is_some_and(|(th, nh)| th < nh)) {
                    want.entry(nid).or_default().insert(ord);
                }
            }
        }
        // 받은 노트 N × 이 기기가 정한 카드 종류 T: N 을 쓴 기기가 T 를 몰랐다
        for n in &notes {
            let list = rows(
                "SELECT n.id, n.uid, t.ord, t.uid FROM note n JOIN note_type nt ON nt.id = n.note_type_id JOIN card_template t ON t.note_type_id = nt.id
                 WHERE n.id = ?1 AND nt.native_kind IS NULL AND n.uid IS NOT NULL AND t.uid IS NOT NULL",
                *n,
            )?;
            for (nid, nuid, ord, tuid) in list {
                let (th, mine) = self.decided("card_template", &tuid, "qfmt")?;
                let (nh, _) = self.decided("note", &nuid, "fields")?;
                if mine && (th.is_none() || nh.zip(th).is_some_and(|(nh, th)| nh < th)) {
                    want.entry(nid).or_default().insert(ord);
                }
            }
        }
        if want.is_empty() {
            return Ok(());
        }
        applying(self.tx, false)?;
        let mut types: HashMap<i64, crate::notetype::Nt> = HashMap::new();
        for (nid, ords) in want {
            let (ty, fields): (i64, String) = self.tx.query_row("SELECT note_type_id, fields FROM note WHERE id = ?1", [nid], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dbe)?;
            let nt = match types.entry(ty) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(crate::notetype::load(self.tx, ty).map_err(other)?),
            };
            let values: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
            crate::notetype::add_missing_cards_where(self.tx, nt, nid, &values, |o| ords.contains(&o)).map_err(other)?;
        }
        applying(self.tx, true)
    }

    /// 두 기기 이상에서 복습한 카드: 마지막 공통 시점(앵커)부터 합친 기록을 시간 순으로 FSRS 에 다시 돌린다
    fn recompute(&mut self, fsrs: &fsrs::FSRS) -> SRes<()> {
        for card in std::mem::take(&mut self.new_logs) {
            let anchor: Option<String> = self.tx.query_row("SELECT srs FROM sync_anchor WHERE card_id = ?1", [card], |r| r.get(0)).optional().map_err(dbe)?;
            // 앵커가 없으면 이 기기의 복습은 다른 기기가 모두 받았다: 받은 상태가 그대로 맞다
            let Some(anchor) = anchor.and_then(|a| serde_json::from_str::<Vec<J>>(&a).ok()) else { continue };
            let mut s = srs_of(|c| SRS_COLS.iter().position(|x| *x == c).and_then(|i| anchor.get(i)));
            let from = anchor.get(9).and_then(|v| v.as_i64()).unwrap_or(0);
            for (_, id, at, rating) in self.fed_logs(card)? {
                if id >= from {
                    s = srs::review(fsrs, &s, rating, at);
                }
            }
            self.report.recomputed += 1;
            self.write_srs(card, &s)?;
        }
        Ok(())
    }

    /// 처음 합치기 (T3-06): 연결 전에 두 기기에서 따로 복습한 카드. 갈라지기 전 상태(앵커)는 어디에도 없다 (앵커는 동기화를 켠 뒤에만 남는다).
    /// 한쪽 복습이 모두 다른 쪽보다 먼저면, 먼저 끝난 쪽 상태에서 나중 쪽 기록을 다시 돌린 것이 합친 기록 전체를 시간 순으로 돌린 것과 같다.
    /// ponytail: 두 쪽 복습이 시간상 엇갈리면 지금처럼 열마다 나중 값을 쓴다. 잦으면 동기화를 켜기 전에도 앵커를 남긴다
    fn merge_replay(&mut self, fsrs: &fsrs::FSRS) -> SRes<()> {
        for (card, (local, remote)) in std::mem::take(&mut self.merge_srs) {
            let logs = self.fed_logs(card)?;
            // Drive 에서 새로 들어온 기록 / Drive 가 모르는 이 기기 기록 (같은 기록은 합칠 때 Drive 쪽 uid 를 따랐다)
            let theirs: Vec<(i64, u8)> = logs.iter().filter(|(u, ..)| self.new_log_uids.contains(u)).map(|(_, _, at, r)| (*at, *r)).collect();
            let mine: Vec<(i64, u8)> =
                logs.iter().filter(|(u, ..)| !self.remote_rows.contains(&("review_log".to_string(), u.clone()))).map(|(_, _, at, r)| (*at, *r)).collect();
            let (Some(t0), Some(t1), Some(m0), Some(m1)) = (theirs.first(), theirs.last(), mine.first(), mine.last()) else { continue };
            let (start, rest) = if t1.0 <= m0.0 {
                (remote, &mine)
            } else if m1.0 <= t0.0 {
                (local, &theirs)
            } else {
                continue;
            };
            let s = rest.iter().fold(start, |s, (at, r)| srs::review(fsrs, &s, *r, *at));
            self.report.recomputed += 1;
            self.write_srs(card, &s)?;
            // 쓰면서 트리거가 남긴 앵커는 다시 계산하기 전 상태라 틀린 공통 시점이다. 이 결과를 올리면 다른 기기와 같은 출발점이다
            self.tx.execute("DELETE FROM sync_anchor WHERE card_id = ?1", [card]).map_err(dbe)?;
        }
        Ok(())
    }

    /// 일정에 넘긴 복습 기록 (uid, id, 시각, 평가). 시간 순
    fn fed_logs(&self, card: i64) -> SRes<Vec<(String, i64, i64, u8)>> {
        let mut st = self
            .tx
            .prepare_cached("SELECT uid, id, reviewed_at, rating FROM review_log WHERE card_id = ?1 AND fed_to_scheduler = 1 ORDER BY reviewed_at, uid")
            .map_err(dbe)?;
        st.query_map([card], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(dbe)?
            .collect::<Result<_, _>>()
            .map_err(dbe)
    }

    /// 다시 계산한 상태가 다르면 이 기기의 변경으로 올린다 (트리거가 바뀐 열만 쌓는다): 복습하지 않은 기기도 같은 상태가 되게
    fn write_srs(&self, card: i64, s: &SrsState) -> SRes<()> {
        if crate::commands::load_srs(self.tx, card).map_err(other)? == *s {
            return Ok(());
        }
        applying(self.tx, false)?;
        self.tx
            .execute(
                "UPDATE card SET state = ?2, srs_step = ?3, stability = ?4, difficulty = ?5, due_at = ?6, interval_days = ?7,
                        last_reviewed_at = ?8, reps = ?9, lapses = ?10 WHERE id = ?1",
                params![card, s.state, s.step, s.stability, s.difficulty, s.due_at, s.interval_days, s.last_reviewed_at, s.reps, s.lapses],
            )
            .map_err(dbe)?;
        applying(self.tx, true)
    }

    /// 받은 지우기를 적용한다 (K29). 지울 행을 붙잡는 남은 행이 있으면 지우지 않는다:
    ///   - 붙잡기: 지우지 않는 자식(나중에 고쳤거나 새로 단 카드 · 하위 덱 …)이 가리키는 부모 (노트 · 덱 · 노트 유형)
    ///   - 따라가기: 남긴 부모의 일부 (노트의 카드, 덱의 설정, 서술형 정보)
    ///
    /// 남긴 행은 지운 기기가 되살리게 다시 보내고, 붙잡은 자식도 다시 보낸다 (지운 기기는 부모가 없어 건너뛰었을 수 있다)
    fn delete_rows(&mut self) -> SRes<()> {
        // 노트를 나중에 고친 것만 붙잡은 카드: 그 자리(노트 · ord)에 새 카드가 왔으면 그 노트 저장은 새 카드를 만든 것이다 (K40)
        for (t, u, d) in std::mem::take(&mut self.by_note) {
            let to = if self.replaced.contains(&(t.to_string(), u.clone())) { &mut self.deletes } else { &mut self.refused };
            to.push((t, u, d));
        }
        let mut cand: HashMap<(&'static str, String), String> =
            std::mem::take(&mut self.deletes).into_iter().filter(|(t, _, _)| *t != "review_log").map(|(t, u, d)| ((t, u), d)).collect();
        let mut kept: HashSet<(&'static str, String)> = self.refused.iter().map(|(t, u, _)| (*t, u.clone())).collect();
        let holds: Vec<(&'static str, String, &'static str)> = self
            .tables
            .iter()
            .filter(|t| !DERIVED.contains(&t.name))
            .flat_map(|t| t.fks.iter().filter(|(_, (_, set_null))| !set_null).map(move |(col, (parent, _))| (t.name, col.clone(), *parent)))
            .collect();
        let mut survivors: BTreeSet<(&'static str, String)> = BTreeSet::new();
        loop {
            let mut moved = vec![];
            for (t, u) in cand.keys() {
                let Some(rowid) = rowid_of(self.tx, t, u)? else { continue };
                let mut by = vec![];
                for (child, col, _) in holds.iter().filter(|(_, _, p)| p == t) {
                    let mut st = self.tx.prepare_cached(&format!("SELECT uid FROM {child} WHERE \"{col}\" = ?1 AND uid IS NOT NULL")).map_err(dbe)?;
                    let uids: Vec<String> = st.query_map([rowid], |r| r.get(0)).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?;
                    by.extend(uids.into_iter().map(|cu| (*child, cu)).filter(|k| !cand.contains_key(k)));
                }
                if *t == "card_template" {
                    let mut st = self.tx.prepare_cached(CARDS_OF_TEMPLATE).map_err(dbe)?;
                    let uids: Vec<String> = st.query_map([rowid], |r| r.get(0)).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?;
                    by.extend(uids.into_iter().map(|cu| ("card", cu)).filter(|k| !cand.contains_key(k)));
                }
                let mut follows = false;
                for (child, col) in PART_OF.iter().filter(|(c, _)| c == t) {
                    let Some(parent) = self.tables.iter().find(|x| x.name == *child).and_then(|x| x.fks.get(*col)).map(|(p, _)| *p) else { continue };
                    let pu: Option<String> = self
                        .tx
                        .query_row(&format!("SELECT uid FROM {parent} WHERE id = (SELECT \"{col}\" FROM {child} WHERE uid = ?1)"), [u], |r| r.get(0))
                        .optional()
                        .map_err(dbe)?;
                    follows |= pu.is_some_and(|pu| kept.contains(&(parent, pu)));
                }
                if !by.is_empty() || follows {
                    moved.push(((*t, u.clone()), by));
                }
            }
            if moved.is_empty() {
                break;
            }
            for (k, by) in moved {
                if let Some(d) = cand.remove(&k) {
                    self.refused.push((k.0, k.1.clone(), d));
                    kept.insert(k);
                    survivors.extend(by);
                }
            }
        }
        // 자식부터 (카드 -> 노트 -> 덱)
        let mut order: Vec<((&'static str, String), String)> = cand.into_iter().collect();
        order.sort_by_key(|((t, u), _)| (std::cmp::Reverse(self.tables.iter().position(|x| x.name == *t)), u.clone()));
        for ((t, uid), d) in order {
            if self.tx.execute(&format!("DELETE FROM {t} WHERE uid = ?1"), [&uid]).is_err() {
                self.refused.push((t, uid, d)); // 다른 참조가 붙잡았다
                continue;
            }
            let mut c = load_clock(self.tx, t, &uid)?.unwrap_or_default();
            c.deleted = Some(d);
            if c.base.is_empty() {
                c.base = "0".into();
            }
            save_clock(self.tx, t, &uid, &c)?;
            self.report.applied += 1;
        }
        // 남긴 행: 모든 열을 지운 HLC 바로 다음으로 다시 보낸다. 카드는 지운 기기에서 비어 버린 복습 기록의 참조도 채우게 그 기록까지
        let now = db::now_ms();
        let put = |t: &str, u: &str, cols: &str, at: i64, floor: Option<&str>| {
            self.tx
                .execute("INSERT INTO sync_outbox (table_name, uid, op, at, cols, floor) VALUES (?1, ?2, 'insert', ?3, ?4, ?5)", params![t, u, at, cols, floor])
                .map(|_| ())
                .map_err(dbe)
        };
        for (t, u, d) in &self.refused[self.revived..] {
            let Some(rowid) = rowid_of(self.tx, t, u)? else { continue };
            put(t, u, REVIVE, 0, Some(d))?;
            if *t == "card" {
                let mut st = self.tx.prepare_cached("SELECT uid FROM review_log WHERE card_id = ?1 AND uid IS NOT NULL").map_err(dbe)?;
                let logs: Vec<String> = st.query_map([rowid], |r| r.get(0)).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?;
                for l in logs {
                    put("review_log", &l, RESEND, now, None)?;
                }
            }
        }
        for (t, u) in survivors {
            put(t, &u, RESEND, now, None)?;
        }
        self.revived = self.refused.len();
        Ok(())
    }
}

/// 남은 자식이 있어도 부모를 붙잡지 않는 표 (기기가 저절로 만드는 것: 오늘의 계획 · 자료 분석 결과 · 자동 연결)
const DERIVED: [&str; 3] = ["daily_plan", "source_segment", "source_link"];
/// 부모를 남기면 함께 남는 그 부모의 일부 (K29): 노트의 카드, 덱의 설정, 서술형 키워드 · 지난번 답, 노트 유형의 카드 종류 (Q2-01)
const PART_OF: [(&str, &str); 5] =
    [("card", "note_id"), ("deck_config", "deck_id"), ("note_written", "note_id"), ("written_answer", "card_id"), ("card_template", "note_type_id")];
/// 카드 종류를 붙잡는 카드 (외래 키가 아니라 노트 유형 + ord 로 가리킨다, Q2-02). 빈칸 유형은 템플릿 하나가 모든 카드를 그린다
const CARDS_OF_TEMPLATE: &str = "SELECT c.uid FROM card c JOIN note n ON n.id = c.note_id JOIN card_template t ON t.note_type_id = n.note_type_id
                                 WHERE t.rowid = ?1 AND (c.template_ord = t.ord OR t.qfmt LIKE '%{{cloze:%') AND c.uid IS NOT NULL";

/// 열 이름 -> 값에서 FSRS 상태 (SRS_COLS 와 같은 열)
fn srs_of<'a>(get: impl Fn(&str) -> Option<&'a J>) -> SrsState {
    let g = |c: &str| get(c).cloned().unwrap_or(J::Null);
    SrsState {
        state: g("state").as_str().unwrap_or("new").into(),
        step: g("srs_step").as_u64().map(|v| v as u32),
        stability: g("stability").as_f64().map(|v| v as f32),
        difficulty: g("difficulty").as_f64().map(|v| v as f32),
        due_at: g("due_at").as_i64(),
        interval_days: g("interval_days").as_f64().unwrap_or(0.0),
        last_reviewed_at: g("last_reviewed_at").as_i64(),
        reps: g("reps").as_i64().unwrap_or(0),
        lapses: g("lapses").as_i64().unwrap_or(0),
    }
}

/// 받은 변경을 합친다. 호출하는 쪽이 트랜잭션과 applying 을 잡는다. src: 받은 스냅샷을 쓴 기기 (K7 기기 이름)
pub fn apply(tx: &Transaction, recs: Vec<Rec>, mode: Mode, fsrs: &fsrs::FSRS, src: &str) -> SRes<Report> {
    let tables = tables(tx)?;
    let aliases = load_aliases(tx)?;
    let (mut folds, max_h) = fold(recs, &aliases);
    // 받은 가장 큰 HLC 를 먼저 남긴다: 합치는 동안 이 기기가 새로 쓰는 변경(합친 필드 목록 · 옮긴 값 · 다시 계산한 일정 · 만든 카드)은
    // 받은 것을 본 뒤의 변경이라 받은 어떤 값보다 커야 한다 (K28). 그 전에 쌓인 변경의 floor 는 bump 가 옛 값으로 채운다
    bump_hlc_last(tx, &max_h)?;
    let mut ap = Applier {
        tx,
        tables: &tables,
        mode,
        aliases,
        pending: load_pending(tx)?.into_iter().collect(),
        me: device_uid(tx)?,
        src: src.to_string(),
        report: Report::default(),
        new_logs: HashSet::new(),
        new_log_uids: HashSet::new(),
        remote_rows: HashSet::new(),
        local_won: HashSet::new(),
        deletes: vec![],
        refused: vec![],
        merge_srs: HashMap::new(),
        new_notes: HashSet::new(),
        new_tpls: HashSet::new(),
        moves: vec![],
        doomed: folds.iter().filter(|(_, f)| f.del.is_some()).map(|(k, _)| k.clone()).collect(),
        parked: HashSet::new(),
        replaced: HashSet::new(),
        by_note: vec![],
        revived: 0,
        retyped: HashSet::new(),
        displaced: vec![],
    };
    if mode == Mode::Merge {
        // 아직 차례가 오지 않은 행도 Drive 가 아는 uid 다 (insert 가 이 기기 uid 를 버려도 되는지 본다)
        ap.remote_rows.extend(folds.keys().cloned());
    }
    let mut parked: Vec<(&Table, String, Fold)> = vec![];
    for table in &tables {
        let mut keys: Vec<(String, String)> = folds.keys().filter(|(t, _)| t == table.name).cloned().collect();
        // 같은 HLC 순서로 (먼저 생긴 부모가 먼저)
        keys.sort_by_key(|k| folds[k].row.as_ref().map(|(h, _)| h.clone()).unwrap_or_default());
        // 덱은 부모가 같은 묶음 안에 있을 수 있다. 줄어들지 않을 때까지 바퀴를 돈다 (남은 것은 부모가 지워진 덱)
        loop {
            let before = keys.len();
            let mut later = vec![];
            for k in keys {
                let f = folds.remove(&k).expect("키");
                match ap.row(table, k.1.clone(), &f)? {
                    Step::Done => {}
                    Step::Later => {
                        folds.insert(k.clone(), f);
                        later.push(k);
                    }
                    Step::Park => {
                        ap.parked.insert(k.clone());
                        parked.push((table, k.1, f));
                    }
                }
            }
            if later.is_empty() || later.len() == before {
                ap.report.skipped += later.len();
                break;
            }
            keys = later;
        }
    }
    ap.delete_rows()?;
    // 지우기를 기다린 행 (부모 먼저 쌓였다): 옛 행을 지웠으면 새 행으로 넣고, 남았으면 같은 것으로 합친다 (K40)
    if !parked.is_empty() {
        ap.doomed.clear();
        ap.parked.clear();
        for (table, uid, f) in parked {
            if let Step::Later = ap.row(table, uid, &f)? {
                ap.report.skipped += 1;
            }
        }
        ap.delete_rows()?;
    }
    // 받은 행 · 지우기가 모두 정해진 뒤에: 진 쪽 노트 값 옮기기, 빠진 카드 만들기 (다시 넣은 노트 · 카드 종류도 insert 가 new_notes · new_tpls 에 넣었다).
    // 받은 가장 큰 HLC 는 처음에 남겼다 (bump_hlc_last): 여기서 쓰는 이 기기 변경은 받은 어떤 값보다 크다 (K28)
    ap.settle_retyped()?;
    ap.move_values()?;
    ap.fill_cards()?;
    ap.recompute(fsrs)?;
    if mode == Mode::Merge {
        ap.merge_replay(fsrs)?;
        enqueue_local(tx, &tables, &ap.remote_rows, &ap.local_won)?;
    }
    Ok(ap.report)
}

/// 처음 합친 뒤: Drive 에 없던 이 기기 행과 이 기기 값이 이긴 행을 지금 열 HLC 그대로 올린다 (RESEND).
/// 처음 합친 결과는 Drive 쪽을 바탕으로 고친 것이 아니라서, 받는 기기도 내용이 다르면 K7 에 남긴다 (T3-04)
fn enqueue_local(tx: &Transaction, tables: &[Table], remote: &HashSet<(String, String)>, won: &HashSet<(String, String)>) -> SRes<()> {
    let now = db::now_ms();
    for t in tables {
        let uids: Vec<String> = {
            let mut st = tx.prepare(&format!("SELECT uid FROM {} WHERE uid IS NOT NULL", t.name)).map_err(dbe)?;
            st.query_map([], |r| r.get(0)).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
        };
        for u in uids {
            let key = (t.name.to_string(), u.clone());
            if !remote.contains(&key) || won.contains(&key) {
                tx.execute("INSERT INTO sync_outbox (table_name, uid, op, at, cols) VALUES (?1, ?2, 'insert', ?3, ?4)", params![t.name, u, now, RESEND])
                    .map_err(dbe)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 스냅샷 (5.5)

pub struct Counts {
    pub decks: i64,
    pub cards: i64,
    pub reviews: i64,
    pub media_bytes: i64,
}

pub fn counts(con: &Connection) -> SRes<Counts> {
    let n = |sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).map_err(dbe);
    Ok(Counts {
        decks: n("SELECT count(*) FROM deck WHERE parent_id IS NULL AND archived_at IS NULL")?,
        cards: n("SELECT count(*) FROM card")?,
        reviews: n("SELECT count(*) FROM review_log")?,
        media_bytes: n("SELECT coalesce(sum(byte_size), 0) FROM media")?,
    })
}

/// 전체 상태: 모든 행(열 HLC 포함)과 지운 흔적. 받는 쪽은 변경 파일과 같은 방법으로 합친다
fn build_snapshot(con: &Connection, generation: i64, reason: &J, key: Option<&[u8; 32]>) -> SRes<(String, Vec<u8>)> {
    let me = device_uid(con)?;
    let seq = local_i64(con, "my_seq")?;
    let mut recs = vec![];
    for table in tables(con)? {
        let ids: Vec<i64> = {
            let mut st = con.prepare(&format!("SELECT rowid FROM {} WHERE uid IS NOT NULL", table.name)).map_err(dbe)?;
            st.query_map([], |r| r.get(0)).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
        };
        for id in ids {
            let Some((uid, r)) = export(con, &table, id)? else { continue };
            let c = load_clock(con, table.name, &uid)?.unwrap_or_else(|| Clock::implicit(&r, &me));
            let k = (!c.cols.is_empty()).then(|| c.cols.clone());
            let b = (table.name == "note").then(|| c.col("fields").to_string());
            recs.push(Rec { t: table.name.into(), u: uid, op: "up".into(), h: c.base, r: Some(r), c: None, k, b });
        }
        let mut st = con.prepare("SELECT uid, deleted FROM sync_row WHERE table_name = ?1 AND deleted IS NOT NULL").map_err(dbe)?;
        let tombs: Vec<(String, String)> = st.query_map([table.name], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?;
        for (u, d) in tombs {
            if rowid_of(con, table.name, &u)?.is_none() {
                recs.push(Rec { t: table.name.into(), u, op: "del".into(), h: d, r: None, c: None, k: None, b: None });
            }
        }
    }
    let aliases: Vec<J> = {
        let mut st = con.prepare("SELECT table_name, from_uid, to_uid FROM sync_alias").map_err(dbe)?;
        st.query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?]))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
    };
    let mut covers = positions(con)?;
    covers.insert(me.clone(), seq);
    let c = counts(con)?;
    let header = json!({
        "v": FORMAT, "kind": "snapshot", "gen": generation, "device": me, "device_name": meta(con, "device_name")?, "seq": seq,
        "covers": covers, "hlc": hlc_last(con)?, "at": db::now_ms(), "reason": reason, "aliases": aliases,
        "counts": { "decks": c.decks, "cards": c.cards, "reviews": c.reviews, "media_bytes": c.media_bytes },
    });
    Ok((format!("snapshot/g{generation:04}-{me}-{seq:08}.jsonl.gz"), encode(&header, &recs, key)?))
}

/// 스냅샷을 올린다. 그림도 함께 (없는 것만). 만들 때까지 쌓인 변경의 마지막 번호를 돌려준다 (그만큼은 스냅샷에 들어 있다)
pub fn write_snapshot(db: &Mutex<Connection>, cx: &Ctx, generation: i64, reason: J) -> SRes<i64> {
    let (path, data, media, upto) = {
        let mut con = lock(db)?;
        init(&con)?;
        fix_media_hashes(&mut con, &cx.media_dir)?;
        let upto: i64 = con.query_row("SELECT coalesce(max(seq), 0) FROM sync_outbox", [], |r| r.get(0)).map_err(dbe)?;
        let (path, data) = build_snapshot(&con, generation, &reason, cx.key())?;
        let media: Vec<(String, PathBuf)> = {
            let mut st = con.prepare("SELECT filename, sha256 FROM media WHERE length(sha256) = 64").map_err(dbe)?;
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(dbe)?
                .filter_map(Result::ok)
                .filter(|(name, _)| safe_media_name(name).is_ok())
                .map(|(name, sha)| (media_path(&sha, &name, cx.key()), cx.media_dir.join(&name)))
                .collect()
        };
        (path, data, media, upto)
    };
    let mut have = remote_media(cx.store)?;
    upload_media(cx, &media, &mut have)?;
    (cx.progress)("snapshot", 0, 1);
    cx.store.put(&path, &data)?;
    Ok(upto)
}

// ---------------------------------------------------------------- 처음 연결 (K3 · K6-f · K6-m)

#[derive(Clone, Copy, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Join {
    /// Drive 가 비어 있다: 이 기기 데이터로 시작 (세대 1)
    New,
    /// Drive 데이터로 바꾸기 (K6-f [이 데이터로 시작], K6-m)
    Take,
    /// 둘 다 남기고 합치기 (K6-m 기본)
    Merge,
    /// 이 기기 데이터로 Drive 새로 시작 (세대 + 1)
    Fresh,
}

/// 동기화 대상 표를 비운다 (Drive 데이터로 바꾸기). applying 이라 지운 기록이 쌓이지 않는다
/// 이 기기 안에서만 쓰지만 동기화 대상 행을 정수 id 로 가리키는 표. Drive 데이터로 바꾸면 id 가 달라져 비운다
const LOCAL_REFS: [&str; 6] = ["session_card_state", "card_change", "embedding_cache", "import_issue", "recall_mark", "update_journal"];

fn clear_synced(tx: &Transaction) -> SRes<()> {
    for t in LOCAL_REFS {
        let exists: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [t], |r| r.get(0)).map_err(dbe)?;
        if exists {
            tx.execute(&format!("DELETE FROM {t}"), []).map_err(dbe)?;
        }
    }
    for t in tables(tx)?.iter().rev() {
        tx.execute(&format!("DELETE FROM {}", t.name), []).map_err(dbe)?;
    }
    tx.execute_batch("DELETE FROM sync_row; DELETE FROM sync_alias; DELETE FROM sync_anchor; DELETE FROM sync_outbox; DELETE FROM sync_tombstone;").map_err(dbe)
}

/// 처음 합류. 바꾸거나 합치기 전의 이 기기 백업은 부르는 쪽(mod.rs)이 먼저 만든다
pub fn join(db: &Mutex<Connection>, cx: &Ctx, how: Join, reason: J) -> SRes<Report> {
    let r = remote(cx.store)?;
    // K43: 새로 시작(K3)은 Drive 가 비어 있을 때만. 화면이 로그인 때 본 Drive 는 그 사이 다른 기기가 채웠을 수 있다 (두 기기가 K3 를 함께 열었다)
    if how == Join::New && !r.is_empty() {
        return Err(SyncError::Found);
    }
    {
        let con = lock(db)?;
        init(&con)?;
        con.execute("UPDATE sync_flags SET enabled = 1, applying = 0", []).map_err(dbe)?;
        // 데이터 폴더를 다른 컴퓨터로 복사하면 기기 id 도 따라온다. 아직 합류하지 않았는데 Drive 에 같은 id 의 기기나 변경 파일이 있으면
        // 이 기기는 복사본이거나 연결을 끊었던 기기다: 새 id 를 쓴다 (같은 id 둘이 서로의 변경 파일을 덮지 않게.
        // [연결 끊기]는 Drive 의 기기 파일을 지우므로 변경 파일도 본다, Q2-09)
        let me = device_uid(&con)?;
        if gen_of(&con)? == 0 && (r.devices.contains(&format!("devices/{me}.json")) || r.changes.contains_key(&me)) {
            con.execute("UPDATE schema_meta SET value = ?1 WHERE key = 'device_uid'", [new_device_uid()]).map_err(dbe)?;
            set_local(&con, "my_seq", "0")?;
        }
    }
    match how {
        Join::New | Join::Fresh => {
            // 지금 Drive 와 이 기기가 본 어떤 세대보다 크게: 지운 Drive 를 다시 시작해도 다른 기기가 지난 세대로 보고 버리지 않는다 (T3-10)
            let generation = {
                let con = lock(db)?;
                // 이 기기 행의 HLC 를 스냅샷에 싣는 값으로 고정한다
                save_implicit(&con)?;
                r.generation().max(gen_of(&con)?).max(local_i64(&con, "seen_gen")?) + 1
            };
            // 스냅샷을 올린 뒤에야 이 기기를 새 세대로 옮긴다 (실패하면 그대로).
            // 다른 기기의 지난 세대 파일은 머리의 세대를 보고 건너뛰므로 받은 위치는 그대로 둔다
            let upto = write_snapshot(db, cx, generation, reason)?;
            let mut con = lock(db)?;
            let tx = con.transaction().map_err(dbe)?;
            // 스냅샷에 들어간 변경은 다시 올리지 않는다
            tx.execute("DELETE FROM sync_outbox WHERE seq <= ?1", [upto]).map_err(dbe)?;
            tx.execute("DELETE FROM sync_anchor", []).map_err(dbe)?;
            let me = device_uid(&tx)?;
            let mut pos = positions(&tx)?;
            pos.insert(me, local_i64(&tx, "my_seq")?);
            set_local(&tx, "gen", &generation.to_string())?;
            set_local(&tx, "pos", &serde_json::to_string(&pos).map_err(other)?)?;
            tx.commit().map_err(dbe)?;
            Ok(Report::default())
        }
        Join::Take | Join::Merge => {
            let generation = r.generation();
            let snap = r.newest(generation).ok_or_else(|| other("Drive 에 받을 데이터가 없어요"))?.clone();
            let (header, mut recs) = decode(cx.store.get(&snap.path)?, cx.key())?;
            let me = { device_uid(&*lock(db)?)? };
            let mut pos: BTreeMap<String, i64> = header["covers"].as_object().into_iter().flatten().map(|(d, s)| (d.clone(), s.as_i64().unwrap_or(0))).collect();
            // 스냅샷 뒤의 변경도 함께. 파일마다 받는 데 시간이 걸린다: 고르는 창이 진행을 보인다 (R-03)
            let total: usize = r.changes.iter().map(|(d, f)| f.iter().filter(|(s, _)| *s > pos.get(d).copied().unwrap_or(0)).count()).sum();
            let mut done = 0;
            for (d, files) in &r.changes {
                for (s, p) in files {
                    if *s > pos.get(d).copied().unwrap_or(0) {
                        (cx.progress)("pull", done, total as u64);
                        done += 1;
                        if let Some((h, file_recs)) = read_change(cx, p, r.locked)?
                            && h["gen"].as_i64().unwrap_or(0) == generation
                        {
                            recs.extend(file_recs);
                        }
                        pos.insert(d.clone(), *s);
                    }
                }
            }
            // 이 기기의 번호는 이 기기가 정한다 (Drive 에 남은 예전 이 기기 파일과 겹치지 않게)
            let mine = pos.remove(&me).unwrap_or(0);
            let devices = read_devices(cx.store, &r);
            let report = {
                let mut con = lock(db)?;
                let tx = con.transaction().map_err(dbe)?;
                applying(&tx, true)?;
                if how == Join::Take {
                    clear_synced(&tx)?;
                }
                add_aliases(&tx, header["aliases"].as_array().map(|v| v.as_slice()).unwrap_or(&[]))?;
                let report = apply(&tx, recs, if how == Join::Take { Mode::Sync } else { Mode::Merge }, cx.fsrs, header["device"].as_str().unwrap_or_default())?;
                let my_seq = local_i64(&tx, "my_seq")?.max(mine);
                set_local(&tx, "my_seq", &my_seq.to_string())?;
                pos.insert(me.clone(), my_seq);
                set_local(&tx, "gen", &generation.to_string())?;
                set_local(&tx, "pos", &serde_json::to_string(&pos).map_err(other)?)?;
                set_local(&tx, "devices", &serde_json::to_string(&devices).map_err(other)?)?;
                applying(&tx, false)?;
                if how == Join::Take {
                    // 스냅샷이 옛 앱에서 왔으면 빠진 기본 노트 유형을 채운다 (이것은 올릴 변경이다)
                    db::seed_note_types(&tx).map_err(dbe)?;
                }
                tx.commit().map_err(dbe)?;
                report
            };
            fetch_media(db, cx, &r.media)?;
            Ok(report)
        }
    }
}

/// K17 잠그기를 바꿔 새 세대로 옮긴 뒤: 지난 세대 스냅샷과, 옮기기 전에 본 변경 파일(평문이거나 옛 키)을 지운다.
/// 지울 변경 파일은 새 세대를 올리기 전에 정한다 (번호만으로는 세대를 모르고 옛 키로 잠근 파일은 머리도 못 읽는다)
pub fn drop_old_generation(store: &dyn Store, old_gen: i64, before: &Remote) -> SRes<()> {
    for s in remote(store)?.snapshots.iter().filter(|s| s.generation <= old_gen) {
        store.delete(&s.path)?;
    }
    for (_, p) in before.changes.values().flatten() {
        store.delete(p)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 기기 파일 · 정리

/// devices/<나>.json: 기기 이름, 마지막 동기화, 어디까지 받았는지 (4장). 기기마다 자기 파일만 고친다
pub fn write_device(db: &Mutex<Connection>, cx: &Ctx, backups: &J) -> SRes<()> {
    let body = {
        let con = lock(db)?;
        let c = counts(&con)?;
        let last_push: J = local(&con, "last_push")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(J::Null);
        json!({
            "uid": device_uid(&con)?, "name": meta(&con, "device_name")?, "at": db::now_ms(), "gen": gen_of(&con)?,
            "pos": positions(&con)?, "seq": local_i64(&con, "my_seq")?, "last_push": last_push, "backups": backups, "v": FORMAT,
            "app": env!("CARGO_PKG_VERSION"),
            "counts": { "decks": c.decks, "cards": c.cards, "reviews": c.reviews, "media_bytes": c.media_bytes },
        })
    };
    let me = body["uid"].as_str().unwrap_or_default().to_string();
    cx.store.put(&format!("devices/{me}.json"), body.to_string().as_bytes())
}

/// 변경 파일이 200개를 넘거나 마지막 스냅샷이 30일 지났으면 새 스냅샷을 쓰고, 모든 활성 기기가 받은 옛 파일을 지운다
pub fn compact(db: &Mutex<Connection>, cx: &Ctx) -> SRes<bool> {
    let r = remote(cx.store)?;
    let (generation, me) = {
        let con = lock(db)?;
        (gen_of(&con)?, device_uid(&con)?)
    };
    let files: usize = r.changes.values().map(|v| v.len()).sum();
    let newest_at = r.newest(generation).map(|s| s.at).unwrap_or(0);
    let old = newest_at > 0 && db::now_ms() - newest_at > COMPACT_MS;
    if generation == 0 || (files <= COMPACT_FILES && !old) {
        return Ok(false);
    }
    write_snapshot(db, cx, generation, json!({ "kind": "compact" }))?;
    let r = remote(cx.store)?;
    let devices = read_devices(cx.store, &r);
    let now = db::now_ms();
    let active: Vec<&J> = devices.iter().filter(|d| d["gen"].as_i64() == Some(generation) && now - d["at"].as_i64().unwrap_or(0) < STALE_MS).collect();
    let covers = match r.newest(generation) {
        Some(s) => decode(cx.store.get(&s.path)?, cx.key())?.0["covers"].clone(),
        None => return Ok(true),
    };
    for (dev, files) in &r.changes {
        let covered = covers[dev].as_i64().unwrap_or(0);
        // 모든 활성 기기가 받은 위치 (자기 파일은 자기 번호)
        let applied = active.iter().map(|d| if d["uid"].as_str() == Some(dev.as_str()) { d["seq"].as_i64().unwrap_or(0) } else { d["pos"][dev].as_i64().unwrap_or(0) }).min().unwrap_or(0);
        for (seq, path) in files {
            if *seq <= covered.min(applied) {
                cx.store.delete(path)?;
            }
        }
    }
    // 같은 세대의 옛 스냅샷은 가장 새 것 두 개만. 지난 세대는 모든 활성 기기가 넘어왔으면 지운다
    let mut same: Vec<&SnapName> = r.snapshots.iter().filter(|s| s.generation == generation).collect();
    same.sort_by_key(|s| std::cmp::Reverse(s.at));
    let all_moved = devices.iter().filter(|d| now - d["at"].as_i64().unwrap_or(0) < STALE_MS).all(|d| d["gen"].as_i64() == Some(generation));
    for s in &r.snapshots {
        if (s.generation == generation && same.iter().skip(2).any(|x| x.path == s.path)) || (s.generation < generation && all_moved) {
            cx.store.delete(&s.path)?;
        }
    }
    let _ = me;
    Ok(true)
}

// ---------------------------------------------------------------- Drive 백업 (5.2 ④)

/// 보관 규칙: 최근 7일은 하루 하나, 그 전은 주 하나, 최대 12개. (이름, 만든 시각 ms) -> 지울 이름
pub fn prune_backups(list: &[(String, i64)], now: i64) -> Vec<String> {
    use chrono::{Datelike, TimeZone};
    let mut sorted: Vec<&(String, i64)> = list.iter().collect();
    sorted.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    let mut seen = HashSet::new();
    let mut keep = vec![];
    for (name, at) in &sorted {
        let d = chrono::Local.timestamp_millis_opt(*at).single().unwrap_or_else(chrono::Local::now);
        let slot = if now - at < 7 * db::DAY_MS { format!("d{}", d.date_naive()) } else { format!("w{}-{}", d.iso_week().year(), d.iso_week().week()) };
        if seen.insert(slot) {
            keep.push(name.clone());
        }
    }
    keep.truncate(12);
    sorted.into_iter().filter(|(n, _)| !keep.contains(n)).map(|(n, _)| n.clone()).collect()
}

/// 백업 zip 이름의 시각: addiem-2026-10-14-0912.zip (예전 이름 addiem- 도)
pub fn backup_time(name: &str) -> Option<i64> {
    use chrono::TimeZone;
    let s = crate::legacy::backup_rest(name)?.strip_suffix(".zip")?;
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d-%H%M").ok()?;
    chrono::Local.from_local_datetime(&dt).earliest().map(|d| d.timestamp_millis())
}

/// 카드 · 기록 · 설정만 담은 zip (그림 없음, 4장). DB 사본을 VACUUM INTO 로 만들어 그대로 넣는다
pub fn backup_zip(con: &Connection, tmp_dir: &Path) -> SRes<Vec<u8>> {
    std::fs::create_dir_all(tmp_dir).map_err(other)?;
    let copy = tmp_dir.join(format!(".drive-backup-{}.sqlite", crypto::random_hex(6)));
    let _ = std::fs::remove_file(&copy);
    con.execute("VACUUM INTO ?1", [copy.to_string_lossy()]).map_err(dbe)?;
    let result = (|| -> SRes<Vec<u8>> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(crate::db::FILE, opts).map_err(other)?;
        zip.write_all(&std::fs::read(&copy).map_err(other)?).map_err(other)?;
        zip.finish().map_err(other)?;
        Ok(out.into_inner())
    })();
    let _ = std::fs::remove_file(&copy);
    result
}

/// K9: 되돌린 DB 를 이 기기 것으로. 기기 정체(id · 이름 · HLC)와 연결 설정은 지금 것을 쓰고, 백업 시점에 쌓여 있던 변경은 버린다
/// (되돌린 상태 전체가 새 세대 스냅샷으로 올라간다)
pub fn adopt_restored(current: &Connection, restored: &Connection) -> SRes<()> {
    init(restored)?;
    restored
        .execute_batch("DELETE FROM sync_local; DELETE FROM sync_outbox; DELETE FROM sync_tombstone; DELETE FROM sync_anchor; UPDATE sync_flags SET enabled = 1, applying = 0;")
        .map_err(dbe)?;
    for k in ["device_uid", "device_name", "hlc_last"] {
        if let Some(v) = meta(current, k)? {
            restored.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES (?1, ?2)", [k, &v]).map_err(dbe)?;
        }
    }
    let mut st = current.prepare("SELECT key, value FROM sync_local").map_err(dbe)?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(dbe)? {
        let (k, v) = row.map_err(dbe)?;
        set_local(restored, &k, &v)?;
    }
    Ok(())
}

/// zip 안의 DB 를 파일로 꺼낸다
pub fn unzip_db(zip_bytes: &[u8], to: &Path) -> SRes<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|_| other("백업 파일을 읽을 수 없어요"))?;
    let f = crate::legacy::zip_db(&mut zip).ok_or_else(|| other("백업 파일에 학습 데이터가 없어요"))?;
    let mut out = std::fs::File::create(to).map_err(other)?;
    let n = std::io::copy(&mut f.take(MAX_UNPACKED + 1), &mut out).map_err(other)?;
    if n > MAX_UNPACKED {
        drop(out);
        let _ = std::fs::remove_file(to);
        return Err(other("백업 파일이 너무 커요"));
    }
    Ok(())
}
