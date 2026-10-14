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
    seed_note_types(&con)?;
    migrate(&con)?;
    Ok(con)
}

/// 스키마 1 이후에 더한 열. 이미 있으면 건너뛴다.
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
    Ok(())
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let con = Connection::open(path)?;
    con.pragma_update(None, "journal_mode", "WAL")?;
    con.pragma_update(None, "foreign_keys", "ON")?;
    // 스키마는 멱등이다 (IF NOT EXISTS). 새 테이블이 기존 DB 에도 생긴다
    con.execute_batch(SCHEMA)?;
    // 기본 노트 유형도 멱등. 나중에 더한 유형(앞뒤 둘 다)이 기존 DB 에도 생긴다
    seed_note_types(&con)?;
    migrate(&con)?;
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

/// (이름, 앞면, 뒷면)
type Template = (&'static str, &'static str, &'static str);
/// 앱이 직접 만드는 노트 유형 (Anki 기본 유형과 같은 이름·필드·템플릿). 없는 것만 넣는다
const NATIVE_TYPES: [(&str, &str, &[Template]); 4] = [
    ("Basic", r#"["Front","Back"]"#, &[("Card 1", "{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}")]),
    ("Cloze", r#"["Text","Back Extra"]"#, &[("Cloze", "{{cloze:Text}}", "{{cloze:Text}}<br>{{Back Extra}}")]),
    (
        "Basic (and reversed card)",
        r#"["Front","Back"]"#,
        &[("Card 1", "{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}"), ("Card 2", "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}")],
    ),
    // J 서술형 (C3 네 번째 유형). Anki 에 내보내면 type-in 입력칸이 있는 카드가 된다. Anki 에 같은 기본 유형이 없어 이름은 우리말
    (
        "서술형",
        r#"["질문","모범 답안","채점 메모"]"#,
        &[(
            "Card 1",
            "{{질문}}{{type:모범 답안}}",
            "{{FrontSide}}<hr id=answer>{{모범 답안}}{{#채점 메모}}<div class=\"grading-memo\">{{채점 메모}}</div>{{/채점 메모}}",
        )],
    ),
];

pub(crate) fn seed_note_types(con: &Connection) -> rusqlite::Result<()> {
    let now = now_ms();
    for (name, fields, templates) in NATIVE_TYPES {
        let exists: bool =
            con.query_row("SELECT EXISTS (SELECT 1 FROM note_type WHERE name = ?1 AND anki_model_id IS NULL)", [name], |r| r.get(0))?;
        if exists {
            continue;
        }
        con.execute("INSERT INTO note_type (name, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", rusqlite::params![name, fields, now])?;
        let id = con.last_insert_rowid();
        for (ord, (tname, qfmt, afmt)) in templates.iter().enumerate() {
            con.execute(
                "INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, ord as i64, tname, qfmt, afmt],
            )?;
        }
    }
    Ok(())
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
    fn in_memory_has_migrated_columns() {
        // S1-6 빈 데이터도 같은 쿼리를 쓴다 (deck.weak 등)
        let con = open_in_memory().unwrap();
        con.query_row("SELECT count(weak) FROM deck", [], |r| r.get::<_, i64>(0)).unwrap();
    }

    #[test]
    fn schema_applies_and_seeds() {
        let dir = std::env::temp_dir().join(format!("neoanki-test-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let con = open(&dir.join("t.sqlite")).unwrap();
        let n: i64 = con.query_row("SELECT count(*) FROM note_type", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 4, "Basic · Cloze · Basic (and reversed card) · 서술형");
        drop(con);
        // 두 번 열어도 다시 시드하지 않는다
        let con = open(&dir.join("t.sqlite")).unwrap();
        let n: i64 = con.query_row("SELECT count(*) FROM note_type", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 4);
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
