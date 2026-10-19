//! SQLite 연결과 스키마. 정본은 db/schema.sql 이다.

use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};

const SCHEMA: &str = include_str!("../../db/schema.sql");
pub const DAY_MS: i64 = 86_400_000;
/// 하루 경계. 기본은 새벽 4시 이전을 전날로 친다 (study-queue-spec 2장). E1 에서 바꾼다.
/// schema_meta.day_cutoff_hour 가 정본이고, 여기에는 열 때 읽어 둔다
static DAY_CUTOFF_HOUR: AtomicI64 = AtomicI64::new(4);

pub fn cutoff_hour() -> i64 {
    DAY_CUTOFF_HOUR.load(Ordering::Relaxed)
}

pub fn set_cutoff_hour(con: &Connection, hour: i64) -> rusqlite::Result<()> {
    con.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('day_cutoff_hour', ?1)", [hour.to_string()])?;
    DAY_CUTOFF_HOUR.store(hour, Ordering::Relaxed);
    Ok(())
}

/// 데이터를 열지 못했을 때 앱이 뜨도록 쓰는 빈 DB (S1-6)
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let con = Connection::open_in_memory()?;
    con.execute_batch(SCHEMA)?;
    migrate(&con)?;
    seed_note_types(&con)?;
    Ok(con)
}

/// 스키마 1 이후에 더한 열과 동기화 선행 스키마. 이미 있으면 건너뛴다.
/// 메모리 DB(S1-6 빈 데이터)도 같은 열을 가져야 화면이 같은 쿼리를 쓸 수 있다
fn migrate(con: &Connection) -> rusqlite::Result<()> {
    for (table, col, ty) in [
        ("deck_config", "today_new", "INTEGER"),
        ("deck_config", "today_new_day", "TEXT"),
        ("deck_config", "paused", "INTEGER NOT NULL DEFAULT 0"),
        ("deck_config", "paused_until", "TEXT"),
        ("review_log", "overdue", "INTEGER"),
        // C1-w 약한 챕터: 확정 표시, [아니에요] 뒤 다시 제안하지 않는 날, "요즘 괜찮아요" 안내를 띄웠는가
        ("deck", "weak", "INTEGER NOT NULL DEFAULT 0"),
        ("deck", "weak_dismissed_until", "TEXT"),
        ("deck", "weak_recovery_shown", "INTEGER NOT NULL DEFAULT 0"),
        // G1 [그대로 두기]: 이 시각까지 점검 목록에 다시 올리지 않는다
        ("card", "quality_hidden_until", "INTEGER"),
        // H1 "실제 속도에 맞춰 다시 나누기"를 끄면 정해 둔 하루 몫. NULL 이면 매일 다시 나눈다
        ("deck_config", "fixed_new_per_day", "INTEGER"),
        // J1 · C4 "이 덱은 써서 답하기". 1 이면 켬, NULL 이면 템플릿대로. 하위 덱은 가장 가까운 위 덱을 따른다
        ("deck_config", "write_mode", "INTEGER"),
        // H3 압축 회독: 'cram' 이면 SRS 에 넘기지 않은 기록. 오늘의 계획 · 지표 쿼리는 kind IS NULL 만 센다 (히트맵은 함께 센다)
        ("review_log", "kind", "TEXT"),
        // 큐 명세 4.6: 접어둔 카드 복귀의 평가. 오늘 넣은 신규로 세지 않는다 (I82)
        ("review_log", "from_buried", "INTEGER"),
        // Anki 기본 유형 표시 (originalStockKind: 1 Basic · 2 앞뒤 · 4 type-in · 5 Cloze · 6 IO). 가져온 유형만, 내보낼 때 다시 쓴다 (I78)
        ("note_type", "anki_stock_kind", "INTEGER"),
        // R1 "초안에서 만듦": 'draft' 면 초안을 승인해 만든 카드의 연결. method 는 manual 이라 연결 규칙은 직접 연결과 같다.
        // 동기화 대상 표라 CHECK 를 바꾸는 재생성 대신 열을 더한다 (NULL 허용: 예전 앱이 만든 행에는 없다)
        ("source_link", "origin", "TEXT"),
        // 8.4 기출 이력 (exam.rs): 횟수 · 시험 이름 JSON 배열. 0 이면 지운 것, NULL 이면 노트 태그에서 채운다
        ("note", "exam_hits", "INTEGER"),
        ("note", "exam_names", "TEXT"),
        ("source_segment", "exam_hits", "INTEGER"),
        ("source_segment", "exam_names", "TEXT"),
        // 8.5 시험 전 회독 · 혼합 (cram.rs): 'mix' 세션의 평가는 일정에 반영하고 회독 진행으로 센다. [처음부터 다시] 를 누른 시각
        ("study_session", "mode", "TEXT"),
        ("deck_config", "mix_since", "INTEGER"),
        // 8.1 노트 유형 (notetype.rs, docs/engineering/note-types.md): 앱 유형의 종류(이름 대신), 앱에서 만든 사용자 유형,
        // 보이는 필드 순서(자리 번호 JSON, NULL = 자리 순서), 다음 템플릿 번호(지운 ord 를 다시 쓰지 않는다)
        ("note_type", "native_kind", "TEXT"),
        ("note_type", "user_made", "INTEGER NOT NULL DEFAULT 0"),
        ("note_type", "field_order", "TEXT"),
        ("note_type", "next_ord", "INTEGER"),
        // Q31 "지난번" (recall.rs save_marks): 그때의 항목 · 보기 (노트를 고치면 읽지 않는다), 만든 평가 기록 id 와 바로 전 값 (되돌리기)
        ("recall_mark", "shown", "TEXT"),
        ("recall_mark", "log_id", "INTEGER"),
        ("recall_mark", "prev", "TEXT"),
        // 자리별 필드 id (앱에서 더한 필드, NULL = 없음). 두 기기가 같은 자리에 따로 더한 필드를 동기화가 가린다 (Q2-06)
        ("note_type", "field_ids", "TEXT"),
        // 8.7 덱 업데이트 (import/deck_update.rs): 마지막으로 받은 원본(B). 노트는 자리별 해시 + 태그, 유형은 Anki 필드 · 카드 종류 · CSS (DU1).
        // 동기화 대상 표의 NULL 허용 열이라 다른 기기도 같은 기준으로 판정한다 (DU2, 옛 앱은 모르는 열을 건너뛴다)
        ("note", "origin", "TEXT"),
        ("note_type", "origin", "TEXT"),
    ] {
        let has: i64 = con.query_row(
            &format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [col],
            |r| r.get(0),
        )?;
        if has == 0 {
            con.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {ty}"))?;
        }
    }
    // R1: 변경 기록에 초안(draft)도 남긴다. CHECK 는 바꿀 수 없어 표를 다시 만든다 (기기 안 표 · 기존 행 그대로 · 한 번만)
    let sql: String = con.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'card_change'", [], |r| r.get(0))?;
    if !sql.contains("'draft'") {
        let new = sql.replacen("card_change", "card_change_new", 1).replace("('split')", "('split','draft')");
        con.execute_batch(&format!(
            "BEGIN;
             {new};
             INSERT INTO card_change_new SELECT id, kind, deck_id, card_id, summary, payload, created_at, undone_at FROM card_change;
             DROP TABLE card_change;
             ALTER TABLE card_change_new RENAME TO card_change;
             CREATE INDEX IF NOT EXISTS idx_card_change_deck ON card_change(deck_id, created_at);
             COMMIT;"
        ))?;
    }
    // 8.2 고쳐 쓰기(rewrite)도 남긴다. 위와 같은 방법으로 한 번만 (위 블록이 'draft' 를 넣은 뒤의 글자를 다시 읽는다)
    let sql: String = con.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'card_change'", [], |r| r.get(0))?;
    if !sql.contains("'rewrite'") {
        let new = sql.replacen("card_change", "card_change_new", 1).replace("('split','draft')", "('split','draft','rewrite')");
        con.execute_batch(&format!(
            "BEGIN;
             {new};
             INSERT INTO card_change_new SELECT id, kind, deck_id, card_id, summary, payload, created_at, undone_at FROM card_change;
             DROP TABLE card_change;
             ALTER TABLE card_change_new RENAME TO card_change;
             CREATE INDEX IF NOT EXISTS idx_card_change_deck ON card_change(deck_id, created_at);
             COMMIT;"
        ))?;
    }
    // DU3 내가 지운 노트: 덱 업데이트가 다시 넣지 않는다. 지우는 길(카드 · 덱 · 카드 종류 · 빈칸 · 받은 삭제)이 여럿이라 트리거 하나로 모은다.
    // 동기화 대상 표다 (DU42): 스냅샷으로 합류한 기기도 알아야 그 기기의 업데이트가 지운 노트를 되살려 다른 기기에 퍼뜨리지 않는다.
    // 이미 있으면 그대로 둔다 (IGNORE): REPLACE 는 행을 새로 만들어 동기화 uid 가 바뀐다
    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS origin_gone (guid TEXT PRIMARY KEY, at INTEGER NOT NULL);
         DROP TRIGGER IF EXISTS note_origin_gone;
         CREATE TRIGGER note_origin_gone AFTER DELETE ON note WHEN OLD.origin IS NOT NULL AND OLD.anki_guid IS NOT NULL
         BEGIN INSERT OR IGNORE INTO origin_gone (guid, at) VALUES (OLD.anki_guid, CAST(unixepoch('subsec') * 1000 AS INTEGER)); END;",
    )?;
    sync_schema(con)
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let con = Connection::open(path)?;
    con.pragma_update(None, "journal_mode", "WAL")?;
    con.pragma_update(None, "foreign_keys", "ON")?;
    // 스키마는 멱등이다 (IF NOT EXISTS). 새 테이블이 기존 DB 에도 생긴다
    con.execute_batch(SCHEMA)?;
    migrate(&con)?;
    // 기본 노트 유형도 멱등. 나중에 더한 유형(앞뒤 둘 다)이 기존 DB 에도 생긴다. native_kind 열을 쓰므로 migrate 뒤
    seed_note_types(&con)?;
    // 8.4 이 기능보다 먼저 가져온 노트 · 다른 기기에서 받은 노트의 기출 태그
    let tx = con.unchecked_transaction()?;
    crate::exam::fill_from_tags(&tx)?;
    tx.commit()?;
    // 앱을 닫아 끝나지 못한 세션. 합격선 ② 계산에서 "완주하지 못함" 으로 센다
    con.execute(
        "UPDATE study_session SET ended_at = started_at, end_reason = 'app_exit' WHERE ended_at IS NULL",
        [],
    )?;
    let hour: Option<String> = con.query_row("SELECT value FROM schema_meta WHERE key = 'day_cutoff_hour'", [], |r| r.get(0)).optional()?;
    if let Some(h) = hour.and_then(|h| h.parse::<i64>().ok()).filter(|h| (0..=12).contains(h)) {
        DAY_CUTOFF_HOUR.store(h, Ordering::Relaxed);
    }
    Ok(con)
}

/// Drive 동기화 대상 (drive-sync.md 6장). session_card_state · import_* · schema_meta 는 기기 안에서만 쓴다
pub const SYNC_TABLES: [&str; 19] = [
    "deck", "deck_config", "note_type", "card_template", "note", "card", "review_log", "media", "study_session", "daily_plan",
    // H 시험 계획: 쓸 수 있는 시간 · 당겨 맞추기 · 충돌 조정은 사용자의 결정이라 기기 사이에 따라간다
    "plan_setting", "plan_shift", "plan_conflict",
    // I 원자료: 자료 목록 · 강조 구간 · 카드 연결. 자료 파일과 임베딩 캐시(embedding_cache)는 기기 안에만
    "source_material", "source_segment", "source_link",
    // J 서술형: 노트별 필수 키워드 · 써서 답하기, 카드별 지난번에 쓴 답
    "note_written", "written_answer",
    // 8.7 덱 업데이트: 내가 지운 노트 (guid). 다시 넣지 않는다 (DU3 · DU42)
    "origin_gone",
];

/// ULID 처럼 시간 순으로 정렬되는 uid: 48비트 ms 시각 + 80비트 난수, 16진수 32자
const NEW_UID: &str = "printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || lower(hex(randomblob(10)))";

/// 동기화 선행 스키마 (drive-sync.md 6장). 멱등이라 열 때마다 부른다.
///   - 대상 테이블마다 uid 열 (없던 행은 채우고, 새 행은 트리거가 채운다) + 유일 인덱스
///   - sync_outbox: 무엇이 바뀌었는지. sync_tombstone: 지운 사실 (삭제는 CASCADE 라 흔적이 남지 않는다)
///   - 둘 다 동기화를 켰을 때만 쌓고, 받은 변경을 적용하는 동안(applying)은 쌓지 않는다 (되울림 방지)
///   - HLC 는 트리거가 아니라 올릴 때 seq 순서로 매긴다. 트리거는 벽시계 시각(at)만 남긴다
fn sync_schema(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_flags (
             id       INTEGER PRIMARY KEY CHECK (id = 1),
             enabled  INTEGER NOT NULL DEFAULT 0,
             applying INTEGER NOT NULL DEFAULT 0
         );
         INSERT OR IGNORE INTO sync_flags (id) VALUES (1);
         CREATE TABLE IF NOT EXISTS sync_outbox (
             seq        INTEGER PRIMARY KEY AUTOINCREMENT,
             table_name TEXT    NOT NULL,
             uid        TEXT    NOT NULL,
             op         TEXT    NOT NULL CHECK (op IN ('insert','update','delete')),
             at         INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS sync_tombstone (
             table_name TEXT    NOT NULL,
             uid        TEXT    NOT NULL,
             deleted_at INTEGER NOT NULL,
             PRIMARY KEY (table_name, uid)
         );",
    )?;
    let has = |table: &str, col: &str| -> rusqlite::Result<bool> {
        con.query_row(&format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name = ?1"), [col], |r| r.get::<_, i64>(0)).map(|n| n > 0)
    };
    if !has("card_template", "updated_at")? {
        con.execute_batch("ALTER TABLE card_template ADD COLUMN updated_at INTEGER")?;
    }
    let now_expr = "CAST(unixepoch('subsec') * 1000 AS INTEGER)";
    let on = "(SELECT enabled AND NOT applying FROM sync_flags WHERE id = 1)";
    for t in SYNC_TABLES {
        if !has(t, "uid")? {
            con.execute_batch(&format!("ALTER TABLE {t} ADD COLUMN uid TEXT"))?;
        }
        con.execute_batch(&format!(
            "UPDATE {t} SET uid = {NEW_UID} WHERE uid IS NULL;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_{t}_uid ON {t}(uid);
             DROP TRIGGER IF EXISTS {t}_uid_fill;
             DROP TRIGGER IF EXISTS {t}_sync_insert;
             -- 넣기는 트리거 하나로: uid 를 먼저 채우고 나서 기록한다 (트리거끼리의 실행 순서는 보장되지 않는다)
             CREATE TRIGGER IF NOT EXISTS {t}_after_insert AFTER INSERT ON {t}
             BEGIN
               UPDATE {t} SET uid = {NEW_UID} WHERE rowid = NEW.rowid AND uid IS NULL;
               INSERT INTO sync_outbox (table_name, uid, op, at) SELECT '{t}', uid, 'insert', {now_expr} FROM {t} WHERE rowid = NEW.rowid AND {on};
             END;
             CREATE TRIGGER IF NOT EXISTS {t}_sync_update AFTER UPDATE ON {t} WHEN OLD.uid IS NOT NULL AND {on}
             BEGIN INSERT INTO sync_outbox (table_name, uid, op, at) VALUES ('{t}', NEW.uid, 'update', {now_expr}); END;
             CREATE TRIGGER IF NOT EXISTS {t}_sync_delete AFTER DELETE ON {t} WHEN OLD.uid IS NOT NULL AND {on}
             BEGIN
               INSERT INTO sync_outbox (table_name, uid, op, at) VALUES ('{t}', OLD.uid, 'delete', {now_expr});
               INSERT OR REPLACE INTO sync_tombstone (table_name, uid, deleted_at) VALUES ('{t}', OLD.uid, {now_expr});
             END;"
        ))?;
    }
    // 이 기기 (처음 열 때 한 번). 이름은 컴퓨터 이름, K3 에서 바꾼다
    let device: Option<String> = con.query_row("SELECT value FROM schema_meta WHERE key = 'device_uid'", [], |r| r.get(0)).optional()?;
    if device.is_none() {
        con.execute(&format!("INSERT INTO schema_meta (key, value) VALUES ('device_uid', {NEW_UID})"), [])?;
        let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "이 기기".into());
        con.execute("INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('device_name', ?1)", [name])?;
        con.execute("INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('hlc_last', '0')", [])?;
    }
    Ok(())
}

/// (이름, 앞면, 뒷면)
type Template<'a> = (&'a str, &'a str, &'a str);
/// 앱이 직접 만드는 노트 유형 (종류, Anki 기본 유형과 같은 이름·필드·템플릿). 없는 종류만 넣는다
const NATIVE_TYPES: [(&str, &str, &str, &[Template<'static>]); 6] = [
    ("basic", "Basic", r#"["Front","Back"]"#, &[("Card 1", "{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}")]),
    ("cloze", "Cloze", r#"["Text","Back Extra"]"#, &[("Cloze", "{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}")]),
    (
        "reversed",
        "Basic (and reversed card)",
        r#"["Front","Back"]"#,
        &[("Card 1", "{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}"), ("Card 2", "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}")],
    ),
    // J 서술형 (C3 네 번째 유형). Anki 에 내보내면 type-in 입력칸이 있는 카드가 된다. Anki 에 같은 기본 유형이 없어 이름은 우리말
    (
        "written",
        "서술형",
        r#"["질문","모범 답안","채점 메모"]"#,
        &[(
            "Card 1",
            "{{질문}}{{type:모범 답안}}",
            "{{FrontSide}}<hr id=answer>{{모범 답안}}{{#채점 메모}}<div class=\"grading-memo\">{{채점 메모}}</div>{{/채점 메모}}",
        )],
    ),
    // Q 목록 회상 · 선택형 (C3 여섯 · 일곱 번째 유형). Anki 에 같은 기본 유형이 없어 앱 전용 이름. 스크립트 없는 템플릿 (recall.rs)
    ("list", crate::recall::LIST, crate::recall::LIST_FIELDS_JSON, &[("Card 1", crate::recall::LIST_QFMT, crate::recall::LIST_AFMT)]),
    ("choice", crate::recall::CHOICE, crate::recall::CHOICE_FIELDS_JSON, &[("Card 1", crate::recall::CHOICE_QFMT, crate::recall::CHOICE_AFMT)]),
];

pub(crate) fn seed_note_types(con: &Connection) -> rusqlite::Result<()> {
    for (kind, name, fields, templates) in NATIVE_TYPES {
        seed_one(con, kind, name, fields, templates)?;
    }
    // M 그림 가리기: Anki 의 Image Occlusion 과 같은 유형 (템플릿이 길어 occlusion.rs 에 둔다)
    crate::occlusion::seed(con)
}

/// 앱 유형 하나. native_kind 가 생기기 전 DB · 옛 앱의 Drive 스냅샷은 이름으로 한 번 채운다 (모델 id 가 있는 가져온 유형은 건드리지 않는다).
/// 그다음 그 종류가 없을 때만 넣는다 (이름을 보지 않는다)
pub(crate) fn seed_one(con: &Connection, kind: &str, name: &str, fields: &str, templates: &[Template]) -> rusqlite::Result<()> {
    con.execute(
        "UPDATE note_type SET native_kind = ?1 WHERE native_kind IS NULL AND anki_model_id IS NULL AND user_made = 0 AND name = ?2",
        [kind, name],
    )?;
    let exists: bool = con.query_row("SELECT EXISTS (SELECT 1 FROM note_type WHERE native_kind = ?1)", [kind], |r| r.get(0))?;
    if exists {
        return Ok(());
    }
    let now = now_ms();
    con.execute(
        "INSERT INTO note_type (name, fields, created_at, updated_at, native_kind) VALUES (?1, ?2, ?3, ?3, ?4)",
        rusqlite::params![name, fields, now, kind],
    )?;
    let id = con.last_insert_rowid();
    for (ord, (tname, qfmt, afmt)) in templates.iter().enumerate() {
        con.execute(
            "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, ord as i64, tname, qfmt, afmt],
        )?;
    }
    Ok(())
}

/// 앱 유형의 id (basic · reversed · cloze · written · list · choice · io)
pub(crate) fn native_type_id(con: &Connection, kind: &str) -> rusqlite::Result<i64> {
    con.query_row("SELECT id FROM note_type WHERE native_kind = ?1 ORDER BY id LIMIT 1", [kind], |r| r.get(0))
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 시각(ms) -> 학습일 번호. 로컬 시간대 기준이고 새벽 4시에 날이 바뀐다.
/// 날짜 비교만 하면 되므로 epoch 이후 일수로 둔다
pub fn study_day(ms: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let local = Local.timestamp_millis_opt(ms).single().unwrap_or_else(Local::now);
    let shifted = local - chrono::Duration::hours(cutoff_hour());
    shifted.date_naive().signed_duration_since(chrono::NaiveDate::default()).num_days()
}

/// 학습일 번호 -> 그날이 끝나는 시각(ms). 다음날 새벽 4시 직전
pub fn study_day_end_ms(day: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let date = chrono::NaiveDate::default() + chrono::Duration::days(day + 1);
    let at = date.and_hms_opt(cutoff_hour() as u32, 0, 0).unwrap();
    Local.from_local_datetime(&at).earliest().map(|d| d.timestamp_millis()).unwrap_or(0) - 1
}

/// 'YYYY-MM-DD' -> 학습일 번호
pub fn day_from_date(s: &str) -> Option<i64> {
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(d.signed_duration_since(chrono::NaiveDate::default()).num_days())
}

pub fn date_from_day(day: i64) -> String {
    (chrono::NaiveDate::default() + chrono::Duration::days(day)).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_groundwork() {
        let dir = std::env::temp_dir().join(format!("neoanki-sync-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let con = open(&dir.join("t.sqlite")).unwrap();
        let count = |sql: &str| con.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        // 모든 대상 테이블에 uid. 열기 전에 있던 행(기본 노트 유형)도 채워졌다
        for t in SYNC_TABLES {
            assert_eq!(count(&format!("SELECT count(*) FROM pragma_table_info('{t}') WHERE name = 'uid'")), 1, "{t}");
        }
        assert_eq!(count("SELECT count(*) FROM note_type WHERE uid IS NULL OR length(uid) <> 32"), 0);
        assert_eq!(count("SELECT count(*) FROM schema_meta WHERE key IN ('device_uid', 'device_name', 'hlc_last')"), 3);

        // 꺼져 있으면 아무것도 쌓지 않는다. 새 행에도 uid 는 생긴다
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('꺼짐', 0, 0)", []).unwrap();
        assert_eq!(count("SELECT count(*) FROM sync_outbox"), 0);
        assert_eq!(count("SELECT count(*) FROM deck WHERE name = '꺼짐' AND uid IS NOT NULL"), 1);

        // 켜면 넣기 · 고치기 · 지우기가 쌓이고, 지우면 흔적이 남는다 (CASCADE 로 지워진 자식 행도)
        con.execute("UPDATE sync_flags SET enabled = 1", []).unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('켜짐', 0, 0)", []).unwrap();
        let id = con.last_insert_rowid();
        con.execute("INSERT INTO deck_config (deck_id, updated_at) VALUES (?1, 0)", [id]).unwrap();
        con.execute("UPDATE deck SET name = '켜짐2' WHERE id = ?1", [id]).unwrap();
        con.execute("DELETE FROM deck WHERE id = ?1", [id]).unwrap();
        let ops: Vec<String> = con
            .prepare("SELECT table_name || ':' || op FROM sync_outbox ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(ops, ["deck:insert", "deck_config:insert", "deck:update", "deck_config:delete", "deck:delete"]);
        assert_eq!(count("SELECT count(*) FROM sync_tombstone WHERE table_name IN ('deck', 'deck_config')"), 2);

        // 받은 변경을 적용하는 동안은 쌓지 않는다 (되울림 방지)
        con.execute("UPDATE sync_flags SET applying = 1", []).unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('받음', 0, 0)", []).unwrap();
        assert_eq!(count("SELECT count(*) FROM sync_outbox"), 5);
        drop(con);
        // 두 번 열어도 그대로 (멱등)
        let con = open(&dir.join("t.sqlite")).unwrap();
        assert_eq!(con.query_row("SELECT count(*) FROM sync_outbox", [], |r| r.get::<_, i64>(0)).unwrap(), 5);
        drop(con);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn card_change_accepts_drafts_after_rebuild() {
        // 예전 DB: card_change.kind 가 'split' 만 받는다. 열면 표를 다시 만들어 'draft' 도 받고, 기존 행은 그대로
        let dir = std::env::temp_dir().join(format!("neoanki-draft-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sqlite");
        let con = open(&path).unwrap();
        con.execute_batch(
            "DROP TABLE card_change;
             CREATE TABLE card_change (id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('split')), deck_id INTEGER, card_id INTEGER,
               summary TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL, undone_at INTEGER);
             INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at, undone_at) VALUES ('split', 1, 2, '3장으로 나누기', '{}', 5, 6);",
        )
        .unwrap();
        assert!(con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES ('draft', 's', '{}', 0)", []).is_err());
        drop(con);
        for _ in 0..2 {
            let con = open(&path).unwrap();
            let row: (String, i64, i64, String, i64, Option<i64>) = con
                .query_row("SELECT kind, deck_id, card_id, summary, created_at, undone_at FROM card_change WHERE id = 1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })
                .unwrap();
            assert_eq!(row, ("split".into(), 1, 2, "3장으로 나누기".into(), 5, Some(6)));
            con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES ('draft', 's', '{}', 0)", []).unwrap();
            assert!(con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES ('other', 's', '{}', 0)", []).is_err());
            assert_eq!(con.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'idx_card_change_deck'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
            // source_link.origin 은 NULL 을 받는다 (예전 앱이 만든 행)
            assert_eq!(con.query_row("SELECT \"notnull\" FROM pragma_table_info('source_link') WHERE name = 'origin'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn card_change_accepts_rewrites_after_rebuild() {
        // 8.2 앞의 DB 두 가지: 'split' 만 받는 것(R1 앞) · 'split','draft' 를 받는 것. 열면 'rewrite' 도 받고 기존 행은 그대로, 다시 열어도 한 번만
        for old in ["('split')", "('split','draft')"] {
            let dir = std::env::temp_dir().join(format!("neoanki-rewrite-{}-{}-{}", std::process::id(), now_ms(), old.len()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("t.sqlite");
            let con = open(&path).unwrap();
            con.execute_batch(&format!(
                "DROP TABLE card_change;
                 CREATE TABLE card_change (id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN {old}), deck_id INTEGER, card_id INTEGER,
                   summary TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL, undone_at INTEGER);
                 INSERT INTO card_change (kind, deck_id, card_id, summary, payload, created_at, undone_at) VALUES ('split', 1, 2, '3장으로 나누기', '{{}}', 5, 6);"
            ))
            .unwrap();
            assert!(con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES ('rewrite', 's', '{}', 0)", []).is_err());
            drop(con);
            for _ in 0..2 {
                let con = open(&path).unwrap();
                let row: (String, i64, i64, String, i64, Option<i64>) = con
                    .query_row("SELECT kind, deck_id, card_id, summary, created_at, undone_at FROM card_change WHERE id = 1", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                    })
                    .unwrap();
                assert_eq!(row, ("split".into(), 1, 2, "3장으로 나누기".into(), 5, Some(6)), "{old}");
                for kind in ["draft", "rewrite"] {
                    con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES (?1, 's', '{}', 0)", [kind]).unwrap();
                }
                assert!(con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES ('other', 's', '{}', 0)", []).is_err());
                assert_eq!(con.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'idx_card_change_deck'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        // 새 DB 는 schema.sql 그대로 받는다
        let con = open_in_memory().unwrap();
        con.execute("INSERT INTO card_change (kind, summary, payload, created_at) VALUES ('rewrite', 's', '{}', 0)", []).unwrap();
    }

    #[test]
    fn in_memory_has_migrated_columns() {
        // S1-6 빈 데이터도 같은 쿼리를 쓴다 (deck.weak 등)
        let con = open_in_memory().unwrap();
        con.query_row("SELECT count(weak) + count(uid) FROM deck", [], |r| r.get::<_, i64>(0)).unwrap();
    }

    #[test]
    fn schema_applies_and_seeds() {
        let dir = std::env::temp_dir().join(format!("neoanki-test-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let con = open(&dir.join("t.sqlite")).unwrap();
        let n: i64 = con.query_row("SELECT count(*) FROM note_type", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 7, "Basic · Cloze · Basic (and reversed card) · 서술형 · 목록 회상 · 선택형 · Image Occlusion");
        drop(con);
        // 두 번 열어도 다시 시드하지 않는다
        let con = open(&dir.join("t.sqlite")).unwrap();
        let n: i64 = con.query_row("SELECT count(*) FROM note_type", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 7);
    }

    #[test]
    fn day_round_trip() {
        let d = day_from_date("2026-10-14").unwrap();
        assert_eq!(date_from_day(d), "2026-10-14");
        assert!(study_day_end_ms(d) > 0);
        assert_eq!(study_day(study_day_end_ms(d)), d, "그날의 마지막 순간은 그날이다");
        assert_eq!(study_day(study_day_end_ms(d) + 1), d + 1, "1ms 뒤는 다음날이다");
    }
}
