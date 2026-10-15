//! 동기화 엔진 (drive-sync.md 5장). Tauri 를 모른다: DB(Mutex<Connection>)와 Store 만 받는다.
//!
//! 기록 단위는 행이고 참조는 uid 다 (정수 id 는 기기 안에서만). 행마다 열 단위 HLC 를 sync_row 에 둔다.
//!   - 올리기: sync_outbox(트리거) 를 행으로 묶어 changes/<나>/<번호>.jsonl.gz 새 파일 하나로. HLC 는 이때 seq 순서로 매긴다
//!   - 받기: 다른 기기의 새 파일을 모두 내려받아 한 트랜잭션(sync_flags.applying = 1)에서 합친다
//!   - 합치기: 열마다 큰 HLC 가 이긴다. 아직 올리지 않은 이 기기의 변경은 앞으로 받을 어떤 값보다 크다(INF)
//!     삭제는 그 행의 가장 큰 HLC 보다 클 때만. 복습 기록은 합집합. 두 기기에서 복습한 카드는 FSRS 를 다시 돌린다
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

/// 변경 파일 형식. Drive 에 더 큰 값이 있으면 받지 않는다 ("앱을 업데이트해 주세요")
pub const FORMAT: i64 = 1;
/// 아직 올리지 않은 이 기기 변경의 HLC. 어떤 HLC(숫자로 시작)보다 크다
const INF: &str = "~";
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

/// 올릴 때 다음 값: 받은 것 중 가장 큰 값보다 항상 크게 (5.4)
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
             seen INTEGER NOT NULL DEFAULT 0, resolved INTEGER NOT NULL DEFAULT 0);",
    )
    .map_err(dbe)
}

/// 동기화 작업 전에: 표 + 변경 기록 트리거를 바꾼 열 목록까지 남기게 다시 만든다
/// (db.rs 의 기본 트리거는 무엇이 바뀌었는지 모른다. 다른 기능이 더한 열도 여기서 따라간다)
pub fn init(con: &Connection) -> SRes<()> {
    ensure_tables(con)?;
    let has: i64 = con.query_row("SELECT count(*) FROM pragma_table_info('sync_outbox') WHERE name = 'cols'", [], |r| r.get(0)).map_err(dbe)?;
    if has == 0 {
        con.execute_batch("ALTER TABLE sync_outbox ADD COLUMN cols TEXT").map_err(dbe)?;
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

/// 기기마다 어디까지 받았는가 (변경 파일 번호). 이 기기 것은 올린 번호
pub fn positions(con: &Connection) -> SRes<BTreeMap<String, i64>> {
    Ok(local(con, "pos")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default())
}

fn hlc_last(con: &Connection) -> SRes<String> {
    Ok(meta(con, "hlc_last")?.unwrap_or_else(|| "0".into()))
}

fn bump_hlc_last(con: &Connection, h: &str) -> SRes<()> {
    if h != INF && h > hlc_last(con)?.as_str() {
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

    fn max(&self) -> String {
        self.cols.values().chain(std::iter::once(&self.base)).max().cloned().unwrap_or_default()
    }

    /// 동기화 전에 생긴 행: 마지막으로 고친 벽시계 시각을 HLC 로 쓴다 (처음 합칠 때 "나중에 고친 쪽")
    fn implicit(row: &Map<String, J>) -> Clock {
        let phys = ["updated_at", "created_at", "started_at", "reviewed_at", "written_at"].iter().find_map(|k| row.get(*k).and_then(|v| v.as_i64())).unwrap_or(0);
        Clock { base: hlc(phys, 0, ""), ..Default::default() }
    }
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

/// 아직 올리지 않은 변경 (sync_outbox). 행마다 바뀐 열
#[derive(Default, Debug)]
struct Pending {
    all: bool,
    cols: HashSet<String>,
    deleted: bool,
    at: i64,
}

fn load_pending(con: &Connection) -> SRes<HashMap<(String, String), Pending>> {
    let mut st = con.prepare("SELECT table_name, uid, op, cols, at FROM sync_outbox ORDER BY seq").map_err(dbe)?;
    let rows: Vec<(String, String, String, Option<String>, i64)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?;
    let mut out: HashMap<(String, String), Pending> = HashMap::new();
    for (t, u, op, cols, at) in rows {
        let p = out.entry((t, u)).or_default();
        p.at = p.at.max(at);
        match (op.as_str(), cols) {
            ("insert", _) | ("update", None) => p.all = true,
            ("update", Some(c)) => p.cols.extend(c.split(',').filter(|s| !s.is_empty()).map(String::from)),
            _ => p.deleted = true,
        }
    }
    Ok(out)
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

/// 받은 HLC 의 형식: hlc() 가 만든 값이거나 "0" (시계가 없던 행). INF("~") 보다 큰 값이 남으면 그 행은 다시는 고칠 수 없다
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
/// 잠그기 전 세대의 기기가 늦게 올렸거나 누가 써 넣은 파일이다. 오류로 멈추면 그 파일이 지워질 때까지 아무것도 받지 못한다
fn read_change(cx: &Ctx, path: &str) -> SRes<Option<(J, Vec<Rec>)>> {
    let data = cx.store.get(path)?;
    if cx.key.is_some() && !crypto::is_sealed(&data) {
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

/// 올린 행: (표, uid, 바뀐 열 (None = 전부), HLC, 지움)
type Pushed = (String, String, Option<Vec<String>>, String, bool);

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
    let rows: Vec<(i64, String, String, String, i64, Option<String>)> = {
        let mut st = con.prepare("SELECT seq, table_name, uid, op, at, cols FROM sync_outbox ORDER BY seq").map_err(dbe)?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
    };
    let Some(max_outbox) = rows.last().map(|r| r.0) else { return Ok(None) };
    let me = device_uid(con)?;
    let tables = tables(con)?;
    // HLC 는 seq 순서로 하나씩. 행의 HLC 는 그 행 마지막 변경의 것
    struct Group {
        first_op: String,
        all: bool,
        cols: BTreeSet<String>,
        hlc: String,
    }
    let mut h = hlc_last(con)?;
    let mut order: Vec<(String, String)> = vec![];
    let mut groups: HashMap<(String, String), Group> = HashMap::new();
    for (_, t, u, op, at, cols) in rows {
        h = hlc_next(&h, at, &me);
        let key = (t, u);
        let g = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Group { first_op: op.clone(), all: false, cols: BTreeSet::new(), hlc: String::new() }
        });
        g.hlc = h.clone();
        match (op.as_str(), cols) {
            ("insert", _) | ("update", None) => g.all = true,
            ("update", Some(c)) => g.cols.extend(c.split(',').filter(|s| !s.is_empty()).map(String::from)),
            _ => {}
        }
    }
    let mut recs = vec![];
    let mut out_rows = vec![];
    let mut media = vec![];
    let mut reviews = 0;
    for (t, u) in order {
        let g = &groups[&(t.clone(), u.clone())];
        let Some(table) = tables.iter().find(|x| x.name == t) else { continue };
        match rowid_of(con, &t, &u)? {
            Some(rowid) => {
                let cols: Option<Vec<String>> = if g.all { None } else { Some(g.cols.iter().filter(|c| table.cols.contains(c)).cloned().collect()) };
                if cols.as_ref().is_some_and(|c| c.is_empty()) {
                    continue; // 값이 그대로인 UPDATE
                }
                let Some((_, r)) = export(con, table, rowid)? else { continue };
                let b = if t == "note" && cols.as_ref().is_none_or(|c| c.iter().any(|c| c == "fields")) {
                    let clock = load_clock(con, &t, &u)?.unwrap_or_else(|| Clock::implicit(&r));
                    Some(clock.col("fields").to_string())
                } else {
                    None
                };
                if t == "media"
                    && let (Some(sha), Some(name)) = (r.get("sha256").and_then(|v| v.as_str()), r.get("filename").and_then(|v| v.as_str()))
                    && sha.len() == 64
                    && safe_media_name(name).is_ok()
                {
                    media.push((media_path(sha, name, cx.key()), cx.media_dir.join(name)));
                }
                if t == "review_log" && g.first_op == "insert" {
                    reviews += 1;
                }
                out_rows.push((t.clone(), u.clone(), cols.clone(), g.hlc.clone(), false));
                recs.push(Rec { t, u, op: "up".into(), h: g.hlc.clone(), r: Some(r), c: cols, k: None, b });
            }
            // 이번 묶음에서 생겼다 사라진 행(되돌린 복습 기록 등)은 아무도 모르니 보낼 것도 없다
            None if g.first_op == "insert" => {}
            None => {
                out_rows.push((t.clone(), u.clone(), None, g.hlc.clone(), true));
                recs.push(Rec { t, u, op: "del".into(), h: g.hlc.clone(), r: None, c: None, k: None, b: None });
            }
        }
    }
    let seq = local_i64(con, "my_seq")? + 1;
    set_local(con, "my_seq", &seq.to_string())?;
    let header = json!({ "v": FORMAT, "kind": "changes", "gen": gen_of(con)?, "device": me, "seq": seq, "at": db::now_ms(), "hlc": h, "reviews": reviews });
    let data = encode(&header, &recs, cx.key())?;
    Ok(Some(Batch { seq, path: change_path(&me, seq), data, max_outbox, hlc: h, rows: out_rows, media, reviews }))
}

/// 올리기에 성공한 뒤: 행 HLC 를 남기고, 올린 변경을 지우고, 받은 위치에 이 기기 번호를 적는다
pub fn finish_push(con: &mut Connection, b: &Batch) -> SRes<()> {
    let me = device_uid(con)?;
    let tx = con.transaction().map_err(dbe)?;
    for (t, u, cols, h, deleted) in &b.rows {
        let mut c = load_clock(&tx, t, u)?.unwrap_or_default();
        if *deleted {
            c.deleted = Some(h.clone());
        } else {
            match cols {
                None => {
                    c.base = h.clone();
                    c.cols.clear();
                    c.deleted = None;
                }
                Some(cols) => {
                    if c.base.is_empty() {
                        c.base = "0".into();
                    }
                    for col in cols {
                        c.cols.insert(col.clone(), h.clone());
                    }
                }
            }
        }
        save_clock(&tx, t, u, &c)?;
        if t == "card" {
            // 앵커는 다른 기기가 이 번호를 받았다고 알 때까지 둔다 (K27). 그 사이 엇갈려 온 기록도 이 앵커부터 다시 돌린다
            tx.execute("UPDATE sync_anchor SET srs = json_set(srs, '$[10]', ?2) WHERE card_id = (SELECT id FROM card WHERE uid = ?1)", params![u, b.seq])
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
    let batch = { prepare_push(&mut *lock(db)?, cx)? };
    let Some(batch) = batch else { return Ok(false) };
    (cx.progress)("push", 0, 1);
    if !batch.media.is_empty() {
        let mut have = remote_media(cx.store)?;
        upload_media(cx, &batch.media, &mut have)?;
    }
    // 이 번호의 파일이 이미 있으면 같은 기기 id 를 쓰는 다른 컴퓨터가 있다 (연결된 데이터 폴더를 복사했다). 덮지 않는다
    if !cx.store.list(&batch.path)?.is_empty() {
        return Err(other("이 기기 데이터가 다른 컴퓨터에서 복사된 것 같아요. Google Drive 연결을 끊고 다시 연결해 주세요."));
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
    pub media: HashSet<String>,
    pub locked: bool,
    /// 숨김 공간 전체 크기 (K4 "Neo-Anki 가 쓰는 공간")
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
            r.media.insert(e.path);
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
    let (me, generation, pos) = {
        let con = lock(db)?;
        init(&con)?;
        (device_uid(&con)?, gen_of(&con)?, positions(&con)?)
    };
    (cx.progress)("pull", 0, 1);
    let r = remote(cx.store)?;
    if r.is_empty() {
        return Ok(if generation > 0 { Pulled::Gone } else { Pulled::Done(Report::default()) });
    }
    if r.generation() > generation {
        let snap = r.newest(r.generation()).expect("세대가 있으면 스냅샷도 있다");
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
    // 뒤처진 기기: 필요한 변경 파일이 이미 정리됐으면 스냅샷부터 (5.5)
    let mut from = pos.clone();
    let mut recs: Vec<Rec> = vec![];
    let mut aliases = vec![];
    let gap = r.changes.iter().any(|(d, files)| d != &me && files.first().is_some_and(|(s, _)| *s > from.get(d).copied().unwrap_or(0) + 1));
    if gap && let Some(snap) = r.newest(generation) {
        let (header, snap_recs) = decode(cx.store.get(&snap.path)?, cx.key())?;
        for (d, s) in header["covers"].as_object().into_iter().flatten() {
            let s = s.as_i64().unwrap_or(0);
            if d != &me && s > from.get(d).copied().unwrap_or(0) {
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
        .filter(|(d, _)| *d != &me)
        .flat_map(|(d, files)| files.iter().filter(|(s, _)| *s > from.get(d).copied().unwrap_or(0)).map(move |(s, p)| (d.clone(), *s, p.clone())))
        .collect();
    for (i, (dev, seq, path)) in todo.iter().enumerate() {
        (cx.progress)("pull", i as u64, todo.len() as u64);
        let Some((header, file_recs)) = read_change(cx, path)? else {
            new_pos.insert(dev.clone(), *seq);
            continue;
        };
        let file_gen = header["gen"].as_i64().unwrap_or(0);
        if file_gen > generation {
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
        let report = apply(&tx, recs, Mode::Sync, cx.fsrs)?;
        let mut pos = positions(&tx)?;
        for (d, s) in new_pos {
            let e = pos.entry(d).or_insert(0);
            *e = (*e).max(s);
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

/// 이 기기에 없는 그림을 받는다. 데이터를 먼저 합치고 그림은 뒤에서 (K6-f)
pub fn fetch_media(db: &Mutex<Connection>, cx: &Ctx, have_remote: &HashSet<String>) -> SRes<usize> {
    let rows: Vec<(String, String)> = {
        let con = lock(db)?;
        let mut st = con.prepare("SELECT filename, sha256 FROM media WHERE length(sha256) = 64").map_err(dbe)?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
    };
    let todo: Vec<_> = rows
        .into_iter()
        .filter(|(name, sha)| safe_media_name(name).is_ok() && !cx.media_dir.join(name).exists() && have_remote.contains(&media_path(sha, name, cx.key())))
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
            std::fs::write(cx.media_dir.join(name), bytes).map_err(other)?;
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
        if rec.h > max_h {
            max_h = rec.h.clone();
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
/// 기본 노트 유형은 이름, 가져온 노트 유형은 Anki 모델 id. 처음 합칠 때만 같은 .apkg 의 노트(guid)와 복습 기록(revlog id)
fn natural_match(con: &Connection, table: &Table, vals: &HashMap<String, Value>, mode: Mode) -> SRes<Option<(i64, String)>> {
    let v = |c: &str| vals.get(c).cloned().unwrap_or(Value::Null);
    let mut keys: Vec<Vec<String>> = vec![];
    let own = |cols: &[&str]| cols.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    match table.name {
        "note_type" if v("anki_model_id") == Value::Null => keys.push(own(&["name", "anki_model_id"])),
        "note_type" => keys.push(own(&["anki_model_id"])),
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
    mode: Mode,
    aliases: HashMap<(String, String), String>,
    pending: HashMap<(String, String), Pending>,
    me: String,
    report: Report,
    /// 새로 들어온 복습 기록의 카드 rowid
    new_logs: HashSet<i64>,
    /// 처음 합치기: Drive 에 있던 행, 이 기기 값이 이긴 행
    remote_rows: HashSet<(String, String)>,
    local_won: HashSet<(String, String)>,
    deletes: Vec<(&'static str, String, String)>,
}

enum Step {
    Done,
    /// 부모(덱)가 아직 없다. 다음 바퀴에 다시
    Later,
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
        match rowid_of(self.tx, t, &uid)? {
            Some(rowid) => self.update(table, &uid, rowid, f),
            None => self.insert(table, uid, f),
        }
    }

    fn insert(&mut self, table: &Table, uid: String, f: &Fold) -> SRes<Step> {
        let t = table.name;
        let clock = load_clock(self.tx, t, &uid)?;
        let max_in = f.cols.values().map(|(_, h)| h.clone()).max().unwrap_or_default();
        let tomb = if self.pend(t, &uid).is_some_and(|p| p.deleted) { Some(INF.to_string()) } else { clock.as_ref().and_then(|c| c.deleted.clone()) };
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
        if let Some((rowid, local_uid)) = natural_match(self.tx, table, &vals, self.mode)? {
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
        }
        self.report.applied += 1;
        Ok(Step::Done)
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

    fn update(&mut self, table: &Table, uid: &str, rowid: i64, f: &Fold) -> SRes<Step> {
        let t = table.name;
        let Some((_, local)) = export(self.tx, table, rowid)? else { return Ok(Step::Done) };
        let mut clock = load_clock(self.tx, t, uid)?.unwrap_or_else(|| Clock::implicit(&local));
        let pend_all = self.pend(t, uid).is_some_and(|p| p.all);
        let pend_cols: HashSet<String> = self.pend(t, uid).map(|p| p.cols.clone()).unwrap_or_default();
        let pend_at = self.pend(t, uid).map(|p| p.at).unwrap_or(0);
        let local_hlc = |clock: &Clock, c: &str| if pend_all || pend_cols.contains(c) { INF.to_string() } else { clock.col(c).to_string() };
        // 복습 기록은 바뀌지 않는다 (합집합). 카드가 지워져 참조가 비는 것은 각 기기에서 일어난다
        if t != "review_log" {
            let mut sets: Vec<(String, J, String)> = vec![];
            for (c, (v, h)) in &f.cols {
                if !table.cols.contains(c) {
                    continue;
                }
                let lh = local_hlc(&clock, c);
                // 참조는 별칭을 거친 uid 로 비교한다 (합쳐진 덱을 가리키는 옛 uid)
                let v = &match (table.fks.get(c), v.as_str()) {
                    (Some((target, _)), Some(u)) => J::String(alias_of(&self.aliases, target, u)),
                    _ => v.clone(),
                };
                let same = local.get(c) == Some(v);
                if *h > lh {
                    if !same {
                        if t == "note" && c == "fields" && f.base.as_ref().is_none_or(|b| *b < lh) && lh != hlc(0, 0, "") {
                            self.conflict(uid, h, v, &lh, local.get(c), pend_at, false)?;
                        }
                        sets.push((c.clone(), v.clone(), h.clone()));
                    } else {
                        clock.cols.insert(c.clone(), h.clone());
                    }
                } else if !same {
                    if t == "note" && c == "fields" {
                        self.conflict(uid, h, v, &lh, local.get(c), pend_at, true)?;
                    }
                    if self.mode == Mode::Merge {
                        self.local_won.insert((t.into(), uid.to_string()));
                    }
                }
            }
            let applied = !sets.is_empty();
            for (c, v, h) in sets {
                let val = match table.fks.get(&c) {
                    Some((target, nullable)) => match resolve_fk(self.tx, &self.aliases, target, &v)? {
                        Some(id) => id.map(Value::Integer).unwrap_or(Value::Null),
                        None if *nullable => Value::Null,
                        None => continue, // 옮겨 간 덱이 이 기기에 없다: 이 열은 그대로
                    },
                    None => from_json(&v),
                };
                if self.tx.execute(&format!("UPDATE {t} SET \"{c}\" = ?2 WHERE rowid = ?1"), params![rowid, val]).is_ok() {
                    clock.cols.insert(c, h);
                }
            }
            if applied {
                self.report.applied += 1;
            }
        }
        if let Some(d) = &f.del {
            let newest = if self.pend(t, uid).is_some() { INF.to_string() } else { clock.max() };
            if *d > newest {
                self.deletes.push((t, uid.to_string(), d.clone()));
            }
        }
        // 열 HLC 를 행 기본값보다 작게 남길 필요는 없다
        let base = clock.base.clone();
        clock.cols.retain(|_, h| *h > base);
        save_clock(self.tx, t, uid, &clock)?;
        Ok(Step::Done)
    }

    /// K7: 같은 노트 내용을 두 기기에서 고쳤다. 진 쪽 내용을 남긴다
    #[allow(clippy::too_many_arguments)]
    fn conflict(&mut self, note_uid: &str, h: &str, incoming: &J, lh: &str, local: Option<&J>, pend_at: i64, local_wins: bool) -> SRes<()> {
        let local_dev = if lh == INF { self.me.clone() } else { hlc_device(lh).to_string() };
        let local_at = if lh == INF { pend_at } else { hlc_ms(lh) };
        let local_fields = local.and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let in_fields = incoming.as_str().unwrap_or_default().to_string();
        let (wd, wa, wf, ld, la, lf) = if local_wins {
            (local_dev, local_at, local_fields, hlc_device(h).to_string(), hlc_ms(h), in_fields)
        } else {
            (hlc_device(h).to_string(), hlc_ms(h), in_fields, local_dev, local_at, local_fields)
        };
        self.tx
            .execute(
                "INSERT INTO sync_conflict (note_uid, at, winner_device, winner_at, winner_fields, loser_device, loser_at, loser_fields)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![note_uid, db::now_ms(), wd, wa, wf, ld, la, lf],
            )
            .map_err(dbe)?;
        self.report.conflicts += 1;
        Ok(())
    }

    /// 두 기기 이상에서 복습한 카드: 마지막 공통 시점(앵커)부터 합친 기록을 시간 순으로 FSRS 에 다시 돌린다
    fn recompute(&mut self, fsrs: &fsrs::FSRS) -> SRes<()> {
        for card in std::mem::take(&mut self.new_logs) {
            let anchor: Option<String> = self.tx.query_row("SELECT srs FROM sync_anchor WHERE card_id = ?1", [card], |r| r.get(0)).optional().map_err(dbe)?;
            // 앵커가 없으면 이 기기의 복습은 다른 기기가 모두 받았다: 받은 상태가 그대로 맞다
            let Some(anchor) = anchor.and_then(|a| serde_json::from_str::<Vec<J>>(&a).ok()) else { continue };
            let mut s = SrsState {
                state: anchor[0].as_str().unwrap_or("new").into(),
                step: anchor[1].as_u64().map(|v| v as u32),
                stability: anchor[2].as_f64().map(|v| v as f32),
                difficulty: anchor[3].as_f64().map(|v| v as f32),
                due_at: anchor[4].as_i64(),
                interval_days: anchor[5].as_f64().unwrap_or(0.0),
                last_reviewed_at: anchor[6].as_i64(),
                reps: anchor[7].as_i64().unwrap_or(0),
                lapses: anchor[8].as_i64().unwrap_or(0),
            };
            let from = anchor[9].as_i64().unwrap_or(0);
            let logs: Vec<(i64, u8)> = {
                let mut st = self
                    .tx
                    .prepare("SELECT reviewed_at, rating FROM review_log WHERE card_id = ?1 AND id >= ?2 AND fed_to_scheduler = 1 ORDER BY reviewed_at, uid")
                    .map_err(dbe)?;
                st.query_map([card, from], |r| Ok((r.get(0)?, r.get(1)?))).map_err(dbe)?.collect::<Result<_, _>>().map_err(dbe)?
            };
            for (at, rating) in logs {
                s = srs::review(fsrs, &s, rating, at);
            }
            self.report.recomputed += 1;
            if crate::commands::load_srs(self.tx, card).map_err(other)? == s {
                continue;
            }
            // 바뀐 상태는 이 기기의 변경으로 올린다 (트리거가 바뀐 열만 쌓는다): 복습하지 않은 기기도 같은 상태가 되게
            applying(self.tx, false)?;
            self.tx
                .execute(
                    "UPDATE card SET state = ?2, srs_step = ?3, stability = ?4, difficulty = ?5, due_at = ?6, interval_days = ?7,
                            last_reviewed_at = ?8, reps = ?9, lapses = ?10 WHERE id = ?1",
                    params![card, s.state, s.step, s.stability, s.difficulty, s.due_at, s.interval_days, s.last_reviewed_at, s.reps, s.lapses],
                )
                .map_err(dbe)?;
            applying(self.tx, true)?;
        }
        Ok(())
    }
}

/// 받은 변경을 합친다. 호출하는 쪽이 트랜잭션과 applying 을 잡는다
pub fn apply(tx: &Transaction, recs: Vec<Rec>, mode: Mode, fsrs: &fsrs::FSRS) -> SRes<Report> {
    let tables = tables(tx)?;
    let aliases = load_aliases(tx)?;
    let (mut folds, max_h) = fold(recs, &aliases);
    let mut ap = Applier {
        tx,
        mode,
        aliases,
        pending: load_pending(tx)?,
        me: device_uid(tx)?,
        report: Report::default(),
        new_logs: HashSet::new(),
        remote_rows: HashSet::new(),
        local_won: HashSet::new(),
        deletes: vec![],
    };
    if mode == Mode::Merge {
        // 아직 차례가 오지 않은 행도 Drive 가 아는 uid 다 (insert 가 이 기기 uid 를 버려도 되는지 본다)
        ap.remote_rows.extend(folds.keys().cloned());
    }
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
                if let Step::Later = ap.row(table, k.1.clone(), &f)? {
                    folds.insert(k.clone(), f);
                    later.push(k);
                }
            }
            if later.is_empty() || later.len() == before {
                ap.report.skipped += later.len();
                break;
            }
            keys = later;
        }
    }
    // 지우기는 자식부터 (카드 -> 노트 -> 덱). 실패하면(이 기기에서 새로 단 자식이 있다) 남긴다 = 고친 쪽이 나중
    let mut deletes = std::mem::take(&mut ap.deletes);
    deletes.sort_by_key(|(t, _, _)| std::cmp::Reverse(tables.iter().position(|x| x.name == *t)));
    for (t, uid, d) in deletes {
        if t == "review_log" {
            continue; // 복습 기록은 지우지 않는다
        }
        if tx.execute(&format!("DELETE FROM {t} WHERE uid = ?1"), [&uid]).is_ok() {
            let mut c = load_clock(tx, t, &uid)?.unwrap_or_default();
            c.deleted = Some(d);
            if c.base.is_empty() {
                c.base = "0".into();
            }
            save_clock(tx, t, &uid, &c)?;
            ap.report.applied += 1;
        }
    }
    ap.recompute(fsrs)?;
    if mode == Mode::Merge {
        enqueue_local(tx, &tables, &ap.remote_rows, &ap.local_won)?;
    }
    bump_hlc_last(tx, &max_h)?;
    Ok(ap.report)
}

/// 처음 합친 뒤: Drive 에 없던 이 기기 행과 이 기기 값이 이긴 행을 올릴 변경으로 쌓는다
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
                tx.execute("INSERT INTO sync_outbox (table_name, uid, op, at, cols) VALUES (?1, ?2, 'insert', ?3, NULL)", params![t.name, u, now])
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
            let c = load_clock(con, table.name, &uid)?.unwrap_or_else(|| Clock::implicit(&r));
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
const LOCAL_REFS: [&str; 4] = ["session_card_state", "card_change", "embedding_cache", "import_issue"];

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
    {
        let con = lock(db)?;
        init(&con)?;
        con.execute("UPDATE sync_flags SET enabled = 1, applying = 0", []).map_err(dbe)?;
        // 데이터 폴더를 다른 컴퓨터로 복사하면 기기 id 도 따라온다. 아직 합류하지 않았는데 Drive 에 같은 id 의 기기가 있으면
        // 이 기기는 복사본이다: 새 id 를 쓴다 (같은 id 둘이 서로의 변경 파일을 덮지 않게)
        let me = device_uid(&con)?;
        if gen_of(&con)? == 0 && r.devices.contains(&format!("devices/{me}.json")) {
            let fresh = format!("{:012x}{}", db::now_ms(), crypto::random_hex(10));
            con.execute("UPDATE schema_meta SET value = ?1 WHERE key = 'device_uid'", [&fresh]).map_err(dbe)?;
            set_local(&con, "my_seq", "0")?;
        }
    }
    match how {
        Join::New | Join::Fresh => {
            let generation = r.generation() + 1;
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
            // 스냅샷 뒤의 변경도 함께
            for (d, files) in &r.changes {
                for (s, p) in files {
                    if *s > pos.get(d).copied().unwrap_or(0) {
                        if let Some((h, file_recs)) = read_change(cx, p)?
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
                let report = apply(&tx, recs, if how == Join::Take { Mode::Sync } else { Mode::Merge }, cx.fsrs)?;
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

/// 백업 zip 이름의 시각: neoanki-2026-10-14-0912.zip
pub fn backup_time(name: &str) -> Option<i64> {
    use chrono::TimeZone;
    let s = name.strip_prefix("neoanki-")?.strip_suffix(".zip")?;
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
        zip.start_file("neoanki.sqlite", opts).map_err(other)?;
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

/// zip 안의 neoanki.sqlite 를 파일로 꺼낸다
pub fn unzip_db(zip_bytes: &[u8], to: &Path) -> SRes<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|_| other("백업 파일을 읽을 수 없어요"))?;
    let f = zip.by_name("neoanki.sqlite").map_err(|_| other("백업 파일에 학습 데이터가 없어요"))?;
    let mut out = std::fs::File::create(to).map_err(other)?;
    let n = std::io::copy(&mut f.take(MAX_UNPACKED + 1), &mut out).map_err(other)?;
    if n > MAX_UNPACKED {
        drop(out);
        let _ = std::fs::remove_file(to);
        return Err(other("백업 파일이 너무 커요"));
    }
    Ok(())
}
