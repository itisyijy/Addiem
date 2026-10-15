//! 덱 내보내기 (mvp-scope 7장 "지원 범위 안의 덱을 .apkg로 다시 내보내기"). import.rs 의 거울이다.
//!
//! 형식: 구버전 스키마 11 collection.anki2 + media 목록 JSON ({"0": "a.png"}) + 번호 붙은 미디어 파일.
//! Anki 2.1 이상과 우리 가져오기가 모두 읽는다.
//!
//! 원칙
//!   - 반쯤 쓴 .apkg 를 남기지 않는다. 옆에 임시 파일로 다 쓴 뒤 이름을 바꾼다
//!   - Anki 식별자(anki_*)가 있으면 그대로 쓴다. Anki 로 다시 가져가면 같은 노트로 알아본다
//!   - 복습 기록은 사용자가 고를 때만 넣는다. 아니면 모두 새 카드로 내보내고 일시 정지만 지킨다 (가져오기와 같다)

use crate::commands::{safe_media_name, AppState};
use crate::db::{self, now_ms};
use crate::render::{render_card, sound_names, strip_sound, strip_tags};
use crate::srs;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};
use zip::write::SimpleFileOptions;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const SEP: &str = "\x1f"; // Anki 가 노트 필드를 나누는 구분자

/// Anki 스키마 11 (anki/storage.py 의 _addSchema, _updateIndices 그대로)
const SCHEMA_11: &str = "
create table col (
    id              integer primary key,
    crt             integer not null,
    mod             integer not null,
    scm             integer not null,
    ver             integer not null,
    dty             integer not null,
    usn             integer not null,
    ls              integer not null,
    conf            text not null,
    models          text not null,
    decks           text not null,
    dconf           text not null,
    tags            text not null
);
create table notes (
    id              integer primary key,
    guid            text not null,
    mid             integer not null,
    mod             integer not null,
    usn             integer not null,
    tags            text not null,
    flds            text not null,
    sfld            integer not null,
    csum            integer not null,
    flags           integer not null,
    data            text not null
);
create table cards (
    id              integer primary key,
    nid             integer not null,
    did             integer not null,
    ord             integer not null,
    mod             integer not null,
    usn             integer not null,
    type            integer not null,
    queue           integer not null,
    due             integer not null,
    ivl             integer not null,
    factor          integer not null,
    reps            integer not null,
    lapses          integer not null,
    left            integer not null,
    odue            integer not null,
    odid            integer not null,
    flags           integer not null,
    data            text not null
);
create table revlog (
    id              integer primary key,
    cid             integer not null,
    usn             integer not null,
    ease            integer not null,
    ivl             integer not null,
    lastIvl         integer not null,
    factor          integer not null,
    time            integer not null,
    type            integer not null
);
create table graves (
    usn             integer not null,
    oid             integer not null,
    type            integer not null
);
create index ix_notes_usn on notes (usn);
create index ix_cards_usn on cards (usn);
create index ix_revlog_usn on revlog (usn);
create index ix_cards_nid on cards (nid);
create index ix_cards_sched on cards (did, queue, due);
create index ix_revlog_cid on revlog (cid);
create index ix_notes_csum on notes (csum);
";

/// Anki 기본 노트 유형의 CSS 와 LaTeX 머리말 (anki/models.py defaultModel)
const CSS: &str = ".card {\n    font-family: arial;\n    font-size: 20px;\n    text-align: center;\n    color: black;\n    background-color: white;\n}\n";
const CLOZE_CSS: &str = "\n.cloze {\n    font-weight: bold;\n    color: blue;\n}\n";
const LATEX_PRE: &str = "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n";
const LATEX_POST: &str = "\\end{document}";

#[derive(Serialize, Default, Debug)]
pub struct ExportResult {
    notes: usize,
    cards: usize,
    media: usize,
    /// 노트가 쓰지만 미디어 폴더에 없어 넣지 못한 파일 수
    missing_media: usize,
    /// 넣은 복습 기록 수. with_history 가 false 면 0
    reviews: usize,
    path: String,
    /// 찾지 못한 미디어를 쓰는 카드 (C1-xr [카드 보기])
    missing_cards: Vec<i64>,
    /// 이름이 있는 그림 가리기 칸 수 (C1-xr "칸 이름 N개는 Anki에서 보이지 않아요", M8)
    occlusion_labels: usize,
}

const CANCELLED: &str = "내보내기를 멈췄어요. 만들던 파일만 지웠어요.";

/// 진행 알림과 취소 (C1-xp). step: notes | reviews | media | zip
struct Job<'a> {
    emit: &'a dyn Fn(&'static str, usize, usize),
    cancelled: &'a dyn Fn() -> bool,
}

impl Job<'_> {
    fn tick(&self, step: &'static str, done: usize, total: usize) -> Res<()> {
        if (self.cancelled)() {
            return Err(CANCELLED.into());
        }
        (self.emit)(step, done, total);
        Ok(())
    }
}

#[derive(Serialize, Clone)]
struct Progress {
    step: &'static str,
    done: usize,
    total: usize,
}

/// 덱 하나를 하위 덱과 함께 .apkg 로 내보낸다. path 는 화면이 저장 대화상자에서 받는다.
/// 진행은 "export-progress" 이벤트로 보내고, cancel_export 로 멈춘다 (가져오기와 같은 취소 표시를 쓴다. 둘은 동시에 돌지 않는다)
#[tauri::command]
pub async fn export_apkg(app: AppHandle, state: State<'_, AppState>, deck_id: i64, path: String, with_history: bool) -> Res<ExportResult> {
    state.import_cancel.store(false, Ordering::Relaxed);
    let con = state.db.lock().map_err(err)?;
    let emit = |step, done, total| {
        let _ = app.emit("export-progress", Progress { step, done, total });
    };
    let cancelled = || state.import_cancel.load(Ordering::Relaxed);
    export(&con, &state.data_dir, deck_id, Path::new(&path), with_history, &Job { emit: &emit, cancelled: &cancelled })
}

#[tauri::command]
pub fn cancel_export(state: State<AppState>) {
    state.import_cancel.store(true, Ordering::Relaxed);
}

/// C1-xe 확인 창의 덱 상자: 경로, 하위 덱·노트·카드·미디어·복습 기록 수
#[derive(Serialize)]
pub struct ExportPreview {
    path: Vec<String>,
    subdecks: usize,
    notes: i64,
    cards: i64,
    media: usize,
    reviews: i64,
}

#[tauri::command]
pub fn export_preview(state: State<AppState>, deck_id: i64) -> Res<ExportPreview> {
    let con = state.db.lock().map_err(err)?;
    let ids = scope(&con, deck_id)?;
    if ids.is_empty() {
        return Err("내보낼 덱을 찾지 못했어요.".into());
    }
    let in_decks = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let count = |sql: &str| con.query_row(&sql.replace("$IDS", &in_decks), [], |r| r.get::<_, i64>(0)).map_err(err);
    let mut media = BTreeSet::new();
    {
        let mut st = con
            .prepare(&format!("SELECT fields FROM note WHERE id IN (SELECT note_id FROM card WHERE deck_id IN ({in_decks}))"))
            .map_err(err)?;
        for f in st.query_map([], |r| r.get::<_, String>(0)).map_err(err)? {
            media_refs(&f.map_err(err)?, &mut media);
        }
    }
    // 경로는 맨 위 덱부터
    let mut path = vec![];
    let mut cur = Some(deck_id);
    while let Some(id) = cur {
        let (name, parent): (String, Option<i64>) =
            con.query_row("SELECT name, parent_id FROM deck WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        path.insert(0, name);
        cur = parent;
    }
    Ok(ExportPreview {
        path,
        subdecks: ids.len() - 1,
        notes: count("SELECT count(DISTINCT note_id) FROM card WHERE deck_id IN ($IDS)")?,
        cards: count("SELECT count(*) FROM card WHERE deck_id IN ($IDS)")?,
        media: media.len(),
        reviews: count("SELECT count(*) FROM review_log WHERE card_id IN (SELECT id FROM card WHERE deck_id IN ($IDS)) AND kind IS NULL")?,
    })
}

/// 고른 덱과 보관하지 않은 하위 덱
fn scope(con: &Connection, deck_id: i64) -> Res<Vec<i64>> {
    let mut st = con
        .prepare(
            "WITH RECURSIVE t(id) AS (
                 SELECT id FROM deck WHERE id = ?1 AND archived_at IS NULL
                 UNION ALL SELECT d.id FROM deck d JOIN t ON d.parent_id = t.id WHERE d.archived_at IS NULL)
             SELECT id FROM t",
        )
        .map_err(err)?;
    let rows = st.query_map([deck_id], |r| r.get(0)).map_err(err)?;
    rows.collect::<Result<_, _>>().map_err(err)
}

// ------------------------------------------------------------------ C1-xc CSV

#[derive(Serialize)]
pub struct CsvResult {
    rows: usize,
    path: String,
}

/// 열 이름(화면) 과 제목 줄. 제목은 B2-c 가 다시 가져올 때 자동으로 연결되는 이름이다
const CSV_COLUMNS: [(&str, &str); 6] =
    [("front", "앞면"), ("back", "뒷면"), ("deck", "하위 덱"), ("tags", "태그"), ("importance", "중요도"), ("due", "다음 복습일")];
/// 앱이 만드는 노트 유형. 필드를 그대로 쓴다 (빈칸 표시 {{c1::}} 도 지킨다). 그 밖은 카드에 그려진 앞면·답을 쓴다
const NATIVE: [&str; 3] = ["Basic", "Basic (and reversed card)", "Cloze"];

/// C1-xc. 노트 한 줄. UTF-8(BOM), 쉼표, 모든 칸 따옴표 (엑셀에서 한글이 깨지지 않게).
/// 중요도는 직접 정한 것만 쓴다. 비우면 다시 가져올 때 하위 덱 설정을 따른다
#[tauri::command]
pub fn export_csv(state: State<AppState>, deck_id: i64, path: String, columns: Vec<String>, plain_text: bool) -> Res<CsvResult> {
    let con = state.db.lock().map_err(err)?;
    let (out, rows) = csv_text(&con, deck_id, &columns, plain_text)?;
    // 반쯤 쓴 파일을 남기지 않는다
    let target = Path::new(&path);
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, out).map_err(err)?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })?;
    Ok(CsvResult { rows, path })
}

/// CSV 글 전체와 줄 수 (제목 줄 제외)
fn csv_text(con: &Connection, deck_id: i64, columns: &[String], plain_text: bool) -> Res<(String, usize)> {
    let cols: Vec<(&str, &str)> = CSV_COLUMNS.iter().copied().filter(|(k, _)| columns.iter().any(|c| c == k)).collect();
    if cols.is_empty() {
        return Err("넣을 열을 하나 이상 골라 주세요.".into());
    }
    let paths: HashMap<i64, String> = {
        let mut st = con
            .prepare(
                "WITH RECURSIVE t(id, path) AS (
                     SELECT id, name FROM deck WHERE id = ?1 AND archived_at IS NULL
                     UNION ALL SELECT d.id, t.path || '::' || d.name FROM deck d JOIN t ON d.parent_id = t.id WHERE d.archived_at IS NULL)
                 SELECT id, path FROM t",
            )
            .map_err(err)?;
        let rows = st.query_map([deck_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    if paths.is_empty() {
        return Err("내보낼 덱을 찾지 못했어요.".into());
    }
    let in_decks = paths.keys().map(i64::to_string).collect::<Vec<_>>().join(",");
    let mut st = con
        .prepare(&format!(
            "SELECT n.fields, n.tags, nt.name, nt.fields, nt.anki_model_id IS NULL, c.deck_id, c.importance, c.state, c.due_at, t.qfmt, t.afmt, c.template_ord
             FROM note n JOIN note_type nt ON nt.id = n.note_type_id
             JOIN card c ON c.id = (SELECT min(id) FROM card WHERE note_id = n.id AND deck_id IN ({in_decks}))
             LEFT JOIN card_template t ON t.note_type_id = nt.id AND t.ord = (CASE WHEN t.qfmt LIKE '%{{{{cloze:%' THEN 0 ELSE c.template_ord END)
             ORDER BY n.id"
        ))
        .map_err(err)?;
    let mut rows = st.query([]).map_err(err)?;
    let mut out = String::from("\u{feff}");
    let line = |cells: Vec<String>| cells.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect::<Vec<_>>().join(",") + "\r\n";
    out += &line(cols.iter().map(|(_, h)| h.to_string()).collect());
    let mut n = 0;
    while let Some(r) = rows.next().map_err(err)? {
        // CSV 는 글자만: [sound:] 은 지운다 (카드 화면의 소리 칩 자리 표시도 남지 않게 렌더 전에)
        let fields: Vec<String> = serde_json::from_str::<Vec<String>>(&r.get::<_, String>(0).map_err(err)?).map_err(err)?.iter().map(|f| strip_sound(f)).collect();
        let (ntname, native): (String, bool) = (r.get(2).map_err(err)?, r.get(4).map_err(err)?);
        let (front, back) = if native && NATIVE.contains(&ntname.as_str()) {
            (fields.first().cloned().unwrap_or_default(), fields.get(1).cloned().unwrap_or_default())
        } else {
            let names: Vec<String> = serde_json::from_str(&r.get::<_, String>(3).map_err(err)?).unwrap_or_default();
            let map: HashMap<String, String> = names.into_iter().zip(fields.iter().cloned()).collect();
            let (q, a): (Option<String>, Option<String>) = (r.get(9).map_err(err)?, r.get(10).map_err(err)?);
            let ord: i64 = r.get(11).map_err(err)?;
            let shown = render_card(&q.unwrap_or_default(), &a.unwrap_or_default(), &map, ord as u32);
            (shown.front, crate::commands::answer_part(&shown.back).to_string())
        };
        let text = |s: String| if plain_text { strip_tags(&s) } else { s };
        let level: Option<i64> = r.get(6).map_err(err)?;
        let (state, due): (String, Option<i64>) = (r.get(7).map_err(err)?, r.get(8).map_err(err)?);
        let cell = |key: &str| -> Res<String> {
            Ok(match key {
                "front" => text(front.clone()),
                "back" => text(back.clone()),
                "deck" => paths.get(&r.get::<_, i64>(5).map_err(err)?).cloned().unwrap_or_default(),
                "tags" => r.get::<_, String>(1).map_err(err)?.split_whitespace().collect::<Vec<_>>().join(" "),
                "importance" => level.map(|l| ["보충", "일반", "핵심"][l.clamp(0, 2) as usize].to_string()).unwrap_or_default(),
                _ => due.filter(|_| state != "new").map(|d| db::date_from_day(db::study_day(d))).unwrap_or_default(),
            })
        };
        out += &line(cols.iter().map(|(k, _)| cell(k)).collect::<Res<_>>()?);
        n += 1;
    }
    Ok((out, n))
}

/// C1-xr [폴더에서 보기]. 탐색기에서 파일을 고른 채로 연다
#[tauri::command]
pub fn reveal_file(path: String) -> Res<()> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("파일을 찾지 못했어요.".into());
    }
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", p.display()));
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(p);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(p.parent().unwrap_or(p));
        c
    };
    cmd.spawn().map_err(err)?;
    Ok(())
}

/// 임시 파일에 다 쓴 뒤 이름을 바꾼다. 실패하면 임시 파일만 지우고 대상 파일은 건드리지 않는다
fn export(con: &Connection, data_dir: &Path, deck_id: i64, path: &Path, with_history: bool, job: &Job) -> Res<ExportResult> {
    let col = data_dir.join(format!("export-{}.anki2", std::process::id()));
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&col); // 지난번에 죽어서 남은 것
    let result = write_package(con, data_dir, deck_id, &col, &tmp, path, with_history, job);
    let _ = std::fs::remove_file(&col);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn write_package(con: &Connection, data_dir: &Path, deck_id: i64, col: &Path, tmp: &Path, path: &Path, with_history: bool, job: &Job) -> Res<ExportResult> {
    let (mut r, media) = build_collection(con, deck_id, col, with_history, job)?;
    let missing = pack(con, data_dir, col, tmp, &media, job)?;
    r.missing_media = missing.len();
    r.media = media.len() - missing.len();
    // 찾지 못한 파일을 쓰는 카드 (C1-xr [카드 보기])
    let in_decks = scope(con, deck_id)?.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    for name in &missing {
        let mut st = con
            .prepare(&format!(
                "SELECT c.id FROM card c JOIN note n ON n.id = c.note_id WHERE c.deck_id IN ({in_decks}) AND instr(n.fields, ?1) > 0"
            ))
            .map_err(err)?;
        for id in st.query_map([name], |row| row.get(0)).map_err(err)? {
            r.missing_cards.push(id.map_err(err)?);
        }
    }
    r.missing_cards.sort_unstable();
    r.missing_cards.dedup();
    job.tick("zip", 1, 1)?;
    std::fs::rename(tmp, path).map_err(err)?;
    r.path = path.to_string_lossy().into_owned();
    Ok(r)
}

// ------------------------------------------------------------------ 컬렉션

struct Note {
    id: i64,
    note_type_id: i64,
    fields: String,
    tags: String,
    created_at: i64,
    updated_at: i64,
    anki_note_id: Option<i64>,
    anki_guid: Option<String>,
}

struct Card {
    id: i64,
    note_id: i64,
    deck_id: i64,
    ord: i64,
    state: String,
    due_at: Option<i64>,
    interval_days: f64,
    stability: Option<f64>,
    difficulty: Option<f64>,
    step: Option<i64>,
    reps: i64,
    lapses: i64,
    created_at: i64,
    updated_at: i64,
    anki_card_id: Option<i64>,
}

/// 겹치면 1씩 올린다. Anki id 는 표마다 유일하면 된다
fn unique(used: &mut HashSet<i64>, mut id: i64) -> i64 {
    while !used.insert(id) {
        id += 1;
    }
    id
}

/// 스키마 11 컬렉션을 col 에 쓴다. 돌려준다: (개수, 노트가 쓰는 미디어 이름)
fn build_collection(con: &Connection, deck_id: i64, col: &Path, with_history: bool, job: &Job) -> Res<(ExportResult, BTreeSet<String>)> {
    let now = now_ms();

    // 덱. 고른 덱이 맨 위 이름이다. 보관한 덱은 그 아래까지 뺀다
    let decks: Vec<(i64, String, i64, i64, Option<i64>)> = {
        let mut st = con
            .prepare(
                "WITH RECURSIVE t(id, path) AS (
                     SELECT id, name FROM deck WHERE id = ?1 AND archived_at IS NULL
                     UNION ALL
                     SELECT d.id, t.path || '::' || d.name FROM deck d JOIN t ON d.parent_id = t.id WHERE d.archived_at IS NULL)
                 SELECT t.id, t.path, d.created_at, d.updated_at, d.anki_deck_id FROM t JOIN deck d ON d.id = t.id",
            )
            .map_err(err)?;
        let rows = st.query_map([deck_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    if decks.is_empty() {
        return Err("내보낼 덱을 찾지 못했어요.".into());
    }
    let mut used = HashSet::from([1]); // 1 은 Anki 의 Default 덱
    let mut deck_json = serde_json::Map::from_iter([("1".to_string(), deck(1, "Default", now / 1000))]);
    let mut deck_map: HashMap<i64, i64> = HashMap::new();
    for (id, path, created, updated, anki) in &decks {
        let aid = unique(&mut used, anki.unwrap_or(*created));
        deck_map.insert(*id, aid);
        deck_json.insert(aid.to_string(), deck(aid, path, updated / 1000));
    }
    // 우리 DB 의 정수 id 라 SQL 에 바로 넣어도 안전하다
    let in_decks = deck_map.keys().map(i64::to_string).collect::<Vec<_>>().join(",");

    // 신규 카드 순서는 우리 큐와 같다 (commands::load_cards 의 position)
    let cards: Vec<Card> = {
        let mut st = con
            .prepare(&format!(
                "SELECT id, note_id, deck_id, template_ord, state, due_at, interval_days, stability, difficulty, srs_step,
                        reps, lapses, created_at, updated_at, anki_card_id
                 FROM card WHERE deck_id IN ({in_decks}) ORDER BY COALESCE(anki_card_id, id)"
            ))
            .map_err(err)?;
        let rows = st
            .query_map([], |r| {
                Ok(Card {
                    id: r.get(0)?,
                    note_id: r.get(1)?,
                    deck_id: r.get(2)?,
                    ord: r.get(3)?,
                    state: r.get(4)?,
                    due_at: r.get(5)?,
                    interval_days: r.get(6)?,
                    stability: r.get(7)?,
                    difficulty: r.get(8)?,
                    step: r.get(9)?,
                    reps: r.get(10)?,
                    lapses: r.get(11)?,
                    created_at: r.get(12)?,
                    updated_at: r.get(13)?,
                    anki_card_id: r.get(14)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    if cards.is_empty() {
        return Err("내보낼 카드가 없어요.".into());
    }
    let notes: Vec<Note> = {
        let mut st = con
            .prepare(&format!(
                "SELECT id, note_type_id, fields, tags, created_at, updated_at, anki_note_id, anki_guid FROM note
                 WHERE id IN (SELECT note_id FROM card WHERE deck_id IN ({in_decks})) ORDER BY id"
            ))
            .map_err(err)?;
        let rows = st
            .query_map([], |r| {
                Ok(Note {
                    id: r.get(0)?,
                    note_type_id: r.get(1)?,
                    fields: r.get(2)?,
                    tags: r.get(3)?,
                    created_at: r.get(4)?,
                    updated_at: r.get(5)?,
                    anki_note_id: r.get(6)?,
                    anki_guid: r.get(7)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };

    let root_did = deck_map[&deck_id];
    let mut used = HashSet::new();
    let mut models: HashMap<i64, Model> = HashMap::new();
    for ntid in notes.iter().map(|n| n.note_type_id).collect::<BTreeSet<_>>() {
        models.insert(ntid, model(con, ntid, root_did, &mut used)?);
    }

    // 컬렉션 생성일. 복습 기한이 이날부터의 일수라 가장 이른 날로 둔다 (음수 기한을 피한다)
    let crt_day = db::study_day(cards.iter().flat_map(|c| [Some(c.created_at), c.due_at]).flatten().min().unwrap_or(now));
    let crt = (db::study_day_end_ms(crt_day - 1) + 1) / 1000;

    let mut out = Connection::open(col).map_err(err)?;
    let tx = out.transaction().map_err(err)?;
    tx.execute_batch(SCHEMA_11).map_err(err)?;

    let mut media = BTreeSet::new();
    let mut labels = 0;
    let mut note_ids: HashMap<i64, i64> = HashMap::new();
    let mut used = HashSet::new();
    let total = notes.len() + cards.len();
    for (i, n) in notes.iter().enumerate() {
        if i % 200 == 0 {
            job.tick("notes", i, total)?;
        }
        let m = &models[&n.note_type_id];
        let mut flds: Vec<String> = serde_json::from_str(&n.fields).map_err(err)?;
        flds.resize(m.nfields.max(1), String::new()); // Anki 는 필드 수가 노트 유형과 같아야 한다
        if m.io {
            labels += crate::occlusion::label_count(&flds[0]);
        }
        for f in &flds {
            media_refs(f, &mut media);
        }
        let id = unique(&mut used, n.anki_note_id.unwrap_or(n.created_at));
        note_ids.insert(n.id, id);
        let tags = n.tags.split_whitespace().collect::<Vec<_>>().join(" ");
        let guid = match &n.anki_guid {
            Some(g) => g.clone(),
            None => {
                // 앱에서 만든 노트. guid 를 남겨 두면 Anki 에서 공부한 뒤 다시 가져와도 같은 노트로 본다 (중복 방지)
                let g = guid_for(n.id, n.created_at);
                con.execute("UPDATE note SET anki_guid = ?2 WHERE id = ?1 AND anki_guid IS NULL", params![n.id, g]).map_err(err)?;
                g
            }
        };
        tx.execute(
            "INSERT INTO notes (id, guid, mid, mod, usn, tags, flds, sfld, csum, flags, data) VALUES (?1, ?2, ?3, ?4, -1, ?5, ?6, ?7, ?8, 0, '')",
            params![
                id,
                guid,
                m.id,
                n.updated_at / 1000,
                if tags.is_empty() { tags } else { format!(" {tags} ") },
                flds.join(SEP),
                strip_tags(flds.get(m.sortf).unwrap_or(&flds[0])),
                checksum(&flds[0])
            ],
        )
        .map_err(err)?;
    }

    // 카드. 같은 노트의 카드는 신규 순서가 같다 (Anki 와 같다)
    let mut card_ids: HashMap<i64, i64> = HashMap::new();
    let mut positions: HashMap<i64, i64> = HashMap::new();
    let mut used = HashSet::new();
    for (i, c) in cards.iter().enumerate() {
        if i % 200 == 0 {
            job.tick("notes", notes.len() + i, total)?;
        }
        let next = positions.len() as i64 + 1;
        let pos = *positions.entry(c.note_id).or_insert(next);
        let s = anki_schedule(c, crt_day, pos, with_history);
        let data = match (with_history, c.stability, c.difficulty) {
            (true, Some(s), Some(d)) => json!({ "s": s, "d": d }).to_string(),
            _ => String::new(),
        };
        let (reps, lapses) = if with_history { (c.reps, c.lapses) } else { (0, 0) };
        let id = unique(&mut used, c.anki_card_id.unwrap_or(c.created_at));
        card_ids.insert(c.id, id);
        tx.execute(
            "INSERT INTO cards (id, nid, did, ord, mod, usn, type, queue, due, ivl, factor, reps, lapses, left, odue, odid, flags, data)
             VALUES (?1, ?2, ?3, ?4, ?5, -1, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, 0, 0, ?14)",
            params![
                id,
                note_ids[&c.note_id],
                deck_map[&c.deck_id],
                c.ord,
                c.updated_at / 1000,
                s.ctype,
                s.queue,
                s.due,
                s.ivl,
                s.factor,
                reps,
                lapses,
                s.left,
                data
            ],
        )
        .map_err(err)?;
    }

    let reviews = if with_history {
        job.tick("reviews", 0, 1)?;
        write_revlog(con, &tx, &in_decks, &card_ids)?
    } else {
        0
    };

    let conf = json!({
        "activeDecks": [1], "curDeck": 1, "newSpread": 0, "collapseTime": 1200, "timeLim": 0, "estTimes": true,
        "dueCounts": true, "curModel": models.values().map(|m| m.id).min(), "nextPos": positions.len() + 1,
        "sortType": "noteFld", "sortBackwards": false, "addToCur": true, "dayLearnFirst": false,
        "schedVer": 2, "rollover": db::cutoff_hour(),
    });
    let models_json: serde_json::Map<String, Value> = models.into_values().map(|m| (m.id.to_string(), m.json)).collect();
    tx.execute(
        "INSERT INTO col (id, crt, mod, scm, ver, dty, usn, ls, conf, models, decks, dconf, tags) VALUES (1, ?1, ?2, ?2, 11, 0, 0, 0, ?3, ?4, ?5, ?6, '{}')",
        params![crt, now, conf.to_string(), Value::Object(models_json).to_string(), Value::Object(deck_json).to_string(), dconf().to_string()],
    )
    .map_err(err)?;
    tx.commit().map_err(err)?;
    let result = ExportResult { notes: notes.len(), cards: cards.len(), reviews, occlusion_labels: labels, ..Default::default() };
    Ok((result, media))
}

/// 복습 기록. 스케줄러에 넘기지 않은 평가(fed_to_scheduler = 0)도 사실이므로 넣는다. 간격은 그대로다
fn write_revlog(con: &Connection, tx: &Connection, in_decks: &str, card_ids: &HashMap<i64, i64>) -> Res<usize> {
    let mut st = con
        .prepare(&format!(
            "SELECT card_id, reviewed_at, rating, elapsed_ms, prev_interval_days, next_interval_days, prev_state FROM review_log
             WHERE card_id IN (SELECT id FROM card WHERE deck_id IN ({in_decks})) AND kind IS NULL ORDER BY reviewed_at"
        ))
        .map_err(err)?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(err)?;
    let days = |d: Option<f64>| d.map_or(0, |d| d.round() as i64);
    let mut used = HashSet::new();
    let mut n = 0;
    for row in rows {
        let (card_id, at, ease, elapsed, prev, next, prev_state) = row.map_err(err)?;
        let Some(cid) = card_ids.get(&card_id) else { continue };
        // type: 0 학습, 1 복습, 2 재학습 (import.rs import_revlog 의 거꾸로)
        let rtype = match prev_state.as_deref() {
            Some("review") => 1,
            Some("relearning") => 2,
            _ => 0,
        };
        tx.execute(
            "INSERT INTO revlog (id, cid, usn, ease, ivl, lastIvl, factor, time, type) VALUES (?1, ?2, -1, ?3, ?4, ?5, 2500, ?6, ?7)",
            params![unique(&mut used, at), cid, ease, days(next.or(prev)), days(prev), elapsed.unwrap_or(0).clamp(0, 60_000), rtype],
        )
        .map_err(err)?;
        n += 1;
    }
    Ok(n)
}

/// Anki cards 표의 일정 열
struct Sched {
    ctype: i64,
    queue: i64,
    due: i64,
    ivl: i64,
    factor: i64,
    left: i64,
}

/// 우리 일정 -> Anki 일정. import.rs schedule() 의 거꾸로다.
/// 복습은 기한이 crt 부터의 일수, 학습 중은 epoch 초. 접어두기(buried)는 세션 안의 일이라 내보내지 않는다
fn anki_schedule(c: &Card, crt_day: i64, pos: i64, with_history: bool) -> Sched {
    // 일시 정지는 원래 상태를 따로 남기지 않는다. 일정으로 되짚는다
    let state = match (c.state.as_str(), c.due_at) {
        ("suspended", None) => "new",
        ("suspended", Some(_)) if c.interval_days >= 1.0 => "review",
        ("suspended", Some(_)) if c.lapses > 0 => "relearning",
        ("suspended", Some(_)) => "learning",
        (s, _) => s,
    };
    let new = Sched { ctype: 0, queue: 0, due: pos, ivl: 0, factor: 0, left: 0 };
    let s = match (state, c.due_at) {
        _ if !with_history => new,
        ("review", Some(at)) => Sched {
            ctype: 2,
            queue: 2,
            due: db::study_day(at) - crt_day,
            ivl: (c.interval_days.round() as i64).max(1),
            factor: 2500,
            left: 0,
        },
        ("learning" | "relearning", Some(at)) => {
            let relearn = state == "relearning";
            let steps = (if relearn { srs::RELEARNING_STEPS.len() } else { srs::LEARNING_STEPS.len() }) as i64;
            // 남은 단계 수. Anki 는 천의 자리에 오늘 안에 끝낼 단계 수를 둔다
            let left = (steps - c.step.unwrap_or(0)).max(1);
            Sched { ctype: if relearn { 3 } else { 1 }, queue: 1, due: at / 1000, ivl: 0, factor: 2500, left: left * 1000 + left }
        }
        _ => new,
    };
    if c.state == "suspended" { Sched { queue: -1, ..s } } else { s }
}

// ------------------------------------------------------------------ JSON (스키마 11 col 표의 덱, 옵션, 노트 유형)

/// 덱 하나 (anki/decks.py defaultDeck)
fn deck(id: i64, name: &str, modified: i64) -> Value {
    json!({
        "id": id, "name": name, "mod": modified, "usn": -1, "desc": "", "dyn": 0, "conf": 1,
        "collapsed": false, "browserCollapsed": false, "extendNew": 10, "extendRev": 50,
        "newToday": [0, 0], "revToday": [0, 0], "lrnToday": [0, 0], "timeToday": [0, 0],
    })
}

/// 옵션 그룹 1 (anki/decks.py defaultConf). 학습 단계는 우리 FSRS 단계와 맞춘다
fn dconf() -> Value {
    let minutes = |steps: &[i64]| steps.iter().map(|ms| *ms as f64 / 60_000.0).collect::<Vec<_>>();
    json!({ "1": {
        "id": 1, "name": "Default", "mod": 0, "usn": 0, "dyn": false,
        "maxTaken": 60, "timer": 0, "autoplay": true, "replayq": true,
        "new": {
            "delays": minutes(&srs::LEARNING_STEPS), "ints": [1, 4, 7], "initialFactor": 2500,
            "separate": true, "order": 1, "perDay": 20, "bury": false,
        },
        "rev": {
            "perDay": 200, "ease4": 1.3, "fuzz": 0.05, "minSpace": 1, "ivlFct": 1.0,
            "maxIvl": 36500, "bury": false, "hardFactor": 1.2,
        },
        "lapse": { "delays": minutes(&srs::RELEARNING_STEPS), "mult": 0.0, "minInt": 1, "leechFails": 8, "leechAction": 1 },
    }})
}

struct Model {
    id: i64,
    json: Value,
    nfields: usize,
    sortf: usize,
    /// Anki 그림 가리기 유형 (occlusion.rs). 칸 필드는 첫 필드다
    io: bool,
}

/// 노트 유형 -> Anki 모델. 원본 JSON(anki_raw)이 있으면 그대로 쓰고, 없으면 스키마 11 기본값으로 만든다.
/// id 는 Anki 에서 온 유형이면 그 id, 아니면 만든 시각(ms) + 행 번호 (기본 유형 셋은 만든 시각이 같다)
fn model(con: &Connection, ntid: i64, did: i64, used: &mut HashSet<i64>) -> Res<Model> {
    let (name, fields, css, created, updated, anki_id, raw): (String, String, String, i64, i64, Option<i64>, Option<String>) = con
        .query_row(
            "SELECT name, fields, css, created_at, updated_at, anki_model_id, anki_raw FROM note_type WHERE id = ?1",
            [ntid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .map_err(err)?;
    // 원본 JSON 은 필드 목록이 있을 때만 믿는다
    let raw = raw
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .filter(|v| v["flds"].as_array().is_some_and(|a| !a.is_empty()));
    if let Some(mut v) = raw {
        let id = v["id"].as_i64().or_else(|| v["id"].as_str()?.parse().ok()).or(anki_id).unwrap_or(created + ntid);
        let id = unique(used, id);
        v["id"] = json!(id);
        let nfields = v["flds"].as_array().map_or(1, Vec::len);
        let sortf = v["sortf"].as_u64().unwrap_or(0) as usize;
        let io = crate::occlusion::is_template(&v["tmpls"].to_string());
        return Ok(Model { id, json: v, nfields, sortf, io });
    }

    let fields: Vec<String> = serde_json::from_str(&fields).map_err(err)?;
    let tmpls: Vec<(i64, String, String, String)> = con
        .prepare("SELECT ord, name, qfmt, afmt FROM card_template WHERE note_type_id = ?1 ORDER BY ord")
        .and_then(|mut s| s.query_map([ntid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
        .map_err(err)?;
    let cloze = tmpls.iter().any(|t| t.2.contains("{{cloze:"));
    let io = tmpls.iter().any(|t| crate::occlusion::is_template(&t.2));
    let id = unique(used, anki_id.unwrap_or(created + ntid));
    let css = match (css.trim().is_empty(), io) {
        (false, _) => css,
        (true, true) => crate::occlusion::CSS.into(),
        (true, false) => format!("{CSS}{}", if cloze { CLOZE_CSS } else { "" }),
    };
    let mut json = json!({
        "id": id, "name": name, "type": i64::from(cloze), "mod": updated / 1000, "usn": -1, "sortf": 0, "did": did,
        "tmpls": tmpls.iter().map(|(ord, n, q, a)| json!({
            "name": n, "ord": ord, "qfmt": q, "afmt": a, "did": null, "bqfmt": "", "bafmt": "",
        })).collect::<Vec<_>>(),
        "flds": fields.iter().enumerate().map(|(i, f)| json!({
            "name": f, "ord": i, "sticky": false, "rtl": false, "font": "Arial", "size": 20, "media": [],
        })).collect::<Vec<_>>(),
        "css": css, "latexPre": LATEX_PRE, "latexPost": LATEX_POST, "latexsvg": false,
        "req": tmpls.iter().map(|(ord, _, q, _)| json!([ord, "any", referenced(q, &fields)])).collect::<Vec<_>>(),
        "tags": [], "vers": [],
    });
    // Anki 가 그림 가리기 유형으로 알아보고 IO 편집기를 연다 (rslib notetype/schema11.rs · Anki 26.9.2 가 내보낸 모델과 같다).
    // 필드는 tag 로 찾는다: 0 Occlusion · 1 Image · 2 Header · 3 Back Extra · 4 Comments. Comments 만 지울 수 있다
    if io {
        json["originalStockKind"] = json!(crate::occlusion::STOCK_KIND);
        for (i, f) in json["flds"].as_array_mut().into_iter().flatten().enumerate().take(5) {
            f["tag"] = json!(i);
            f["preventDeletion"] = json!(i < 4);
        }
    }
    Ok(Model { id, json, nfields: fields.len(), sortf: 0, io })
}

/// 앞면 템플릿이 쓰는 필드 번호. 구버전 Anki 가 카드를 만들지 판단하는 req 에 쓴다
fn referenced(qfmt: &str, fields: &[String]) -> Vec<usize> {
    let found: BTreeSet<usize> = qfmt
        .split("{{")
        .skip(1)
        .filter_map(|s| s.split_once("}}"))
        .filter_map(|(tag, _)| {
            let name = tag.trim_start_matches(['#', '^', '/']).rsplit(':').next()?.trim();
            fields.iter().position(|f| f == name)
        })
        .collect();
    found.into_iter().collect()
}

// ------------------------------------------------------------------ 노트 값

/// 필드가 쓰는 미디어 파일 이름. <img src="a.png"> 와 [sound:a.mp3]. 주소(https://)나 경로는 건너뛴다
pub(crate) fn media_refs(html: &str, out: &mut impl Extend<String>) {
    let srcs = html.match_indices("src=").map(|(i, _)| {
        let rest = &html[i + 4..];
        match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split([' ', '>']).next().unwrap_or(""),
        }
    });
    out.extend(srcs.chain(sound_names(html)).filter(|n| safe_media_name(n).is_ok()).map(String::from));
}

/// Anki 의 중복 검사 값(csum). 첫 필드 글자의 sha1 앞 4바이트
fn checksum(field: &str) -> i64 {
    sha1(strip_tags(field).as_bytes())[..4].iter().fold(0, |n, b| (n << 8) | i64::from(*b))
}

/// Anki guid64 처럼 64비트 값을 base91 로 쓴다. 노트마다 고정이라 다시 내보내도 같다
fn guid_for(id: i64, created_at: i64) -> String {
    const TABLE: &[u8; 91] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&()*+,-./:;<=>?@[]^_`{|}~";
    let mut n = sha1(format!("neo-anki:{id}:{created_at}").as_bytes())[..8].iter().fold(0u64, |n, b| (n << 8) | u64::from(*b));
    let mut out = vec![];
    loop {
        out.push(TABLE[(n % 91) as usize]);
        n /= 91;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

/// SHA-1 (FIPS 180-1). csum 과 guid 에만 쓴다. 보안 용도가 아니다
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (wi, b) in w.iter_mut().zip(block.chunks(4)) {
            *wi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (o, x) in out.chunks_mut(4).zip(h) {
        o.copy_from_slice(&x.to_be_bytes());
    }
    out
}

// ------------------------------------------------------------------ zip

/// collection.anki2 와 미디어를 zip 으로 묶는다. 돌려준다: 미디어 폴더에 없어 넣지 못한 파일 이름
fn pack(con: &Connection, data_dir: &Path, col: &Path, out: &Path, media: &BTreeSet<String>, job: &Job) -> Res<Vec<String>> {
    let mut zip = zip::ZipWriter::new(File::create(out).map_err(err)?);
    zip.start_file("collection.anki2", SimpleFileOptions::default()).map_err(err)?;
    std::io::copy(&mut File::open(col).map_err(err)?, &mut zip).map_err(err)?;
    // 그림과 소리는 이미 압축돼 있다. 그대로 넣는다 (Anki 도 그렇다)
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut map = serde_json::Map::new();
    let mut missing = vec![];
    for (i, name) in media.iter().enumerate() {
        job.tick("media", i, media.len())?;
        let rel: Option<String> =
            con.query_row("SELECT rel_path FROM media WHERE filename = ?1", [name], |r| r.get(0)).optional().map_err(err)?;
        let rel = rel.filter(|r| crate::commands::safe_rel_path(r, "media")).unwrap_or_else(|| format!("media/{name}"));
        let Ok(mut f) = File::open(data_dir.join(rel)) else {
            missing.push(name.clone());
            continue;
        };
        let key = map.len().to_string();
        zip.start_file(&key, stored).map_err(err)?;
        std::io::copy(&mut f, &mut zip).map_err(err)?;
        map.insert(key, json!(name));
    }
    job.tick("zip", 0, 1)?;
    zip.start_file("media", SimpleFileOptions::default()).map_err(err)?;
    zip.write_all(Value::Object(map).to_string().as_bytes()).map_err(err)?;
    zip.finish().map_err(err)?.sync_all().map_err(err)?;
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{insert_note, NewCard};
    use crate::db::DAY_MS;
    use std::sync::Mutex;

    const QUIET: Job = Job { emit: &|_, _, _| {}, cancelled: &|| false };

    /// 테스트가 끝나면 (실패해도) 지운다. 먼저 만들어 가장 나중에 버려지게 한다
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn state(dir: &Path) -> AppState {
        std::fs::create_dir_all(dir.join("media")).unwrap();
        AppState {
            db: Mutex::new(db::open(&dir.join("neoanki.sqlite")).unwrap()),
            fsrs: fsrs::FSRS::default(),
            data_dir: dir.to_path_buf(),
            undo: Mutex::new(vec![]),
            import_cancel: Default::default(),
            startup_error: Mutex::new(None),
        }
    }

    fn strings(con: &Connection, sql: &str) -> Vec<String> {
        con.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // 56바이트: 길이가 다음 블록으로 넘어가는 경계
        assert_eq!(hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        assert_eq!(hex(&sha1("The quick brown fox jumps over the lazy dog".as_bytes())), "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12");
        assert_eq!(checksum("<b>abc</b>"), 0xa999_3e36, "csum 은 태그를 뗀 글자의 앞 8자리");
    }

    #[test]
    fn note_helpers() {
        let mut m = BTreeSet::new();
        media_refs(r#"<img src="a.png"><img src='b c.jpg'><img src=d.gif>[sound:e.mp3]<img src="https://x.com/y.png">"#, &mut m);
        assert_eq!(m.into_iter().collect::<Vec<_>>(), ["a.png", "b c.jpg", "d.gif", "e.mp3"]);
        let f = ["Front".to_string(), "Back".to_string()];
        assert_eq!(referenced("{{#Back}}{{text:Back}}{{/Back}} {{Front}} {{FrontSide}}", &f), [0, 1]);
        assert_eq!(guid_for(7, 1), guid_for(7, 1), "다시 내보내도 같다");
        assert_ne!(guid_for(7, 1), guid_for(8, 1));
    }

    #[test]
    fn csv_rows() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-csv-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0);
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('정보처리기사', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        con.execute("INSERT INTO deck (parent_id, name, created_at, updated_at) VALUES (?1, '1과목', 0, 0)", [root]).unwrap();
        let sub = con.last_insert_rowid();
        let add = |deck_id: i64, kind: &str, front: &str, back: &str, importance: Option<i64>| {
            insert_note(&con, &NewCard { deck_id, kind: kind.into(), front: front.into(), back: back.into(), importance }, "a  b", None).unwrap();
        };
        add(root, "basic", "<b>TCP</b>, \"연결\"", "답<br>둘째 줄", Some(2));
        add(sub, "cloze", "{{c1::서울}}은 {{c2::한국}}", "", None);
        add(sub, "reversed", "앞", "뒤", None);
        let all = ["front", "back", "deck", "tags", "importance"].map(String::from);
        let (text, rows) = csv_text(&con, root, &all, true).unwrap();
        assert_eq!(rows, 3, "노트마다 한 줄 (앞뒤 카드 두 장도 한 줄)");
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines[0], "\u{feff}\"앞면\",\"뒷면\",\"하위 덱\",\"태그\",\"중요도\"", "BOM + 다시 가져올 때 자동 연결되는 제목");
        assert_eq!(lines[1], r#""TCP, ""연결""","답 둘째 줄","정보처리기사","a b","핵심""#, "따옴표는 두 번, 서식은 빼고 줄바꿈은 띄어쓰기");
        assert!(lines[2].starts_with(r#""{{c1::서울}}은 {{c2::한국}}","","정보처리기사::1과목""#), "빈칸 표시는 지킨다: {}", lines[2]);
        let (html, _) = csv_text(&con, sub, &["front".to_string()], false).unwrap();
        assert!(html.contains("{{c1::서울}}") && !html.contains("TCP"), "하위 덱만");
        assert!(csv_text(&con, root, &[], true).is_err());
    }

    #[test]
    fn round_trip_through_importer() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-export-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        let deck = |name: &str, parent: Option<i64>| {
            con.execute("INSERT INTO deck (parent_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", params![parent, name, now_ms()])
                .unwrap();
            con.last_insert_rowid()
        };
        let root = deck("정보처리기사", None);
        let sub = deck("1과목", Some(root));
        let leaf = deck("요약", Some(sub));
        let archived = deck("보관", Some(root));
        let other = deck("다른 덱", None);
        con.execute("UPDATE deck SET archived_at = 1 WHERE id = ?1", [archived]).unwrap();
        let add = |deck_id: i64, kind: &str, front: &str, back: &str| {
            let card = NewCard { deck_id, kind: kind.into(), front: front.into(), back: back.into(), importance: None };
            insert_note(&con, &card, "과목::1 중요", None).unwrap()
        };
        add(root, "basic", "<b>TCP</b> 는?", r#"연결 지향 <img src="neo-a.png"> [sound:gone.mp3]"#);
        add(sub, "cloze", "{{c1::서울}}은 {{c2::한국}}의 수도", "");
        add(leaf, "reversed", "앞", "뒤");
        add(archived, "basic", "보관한 덱", "x");
        add(other, "basic", "다른 덱", "x");
        std::fs::write(a.data_dir.join("media/neo-a.png"), b"png-bytes").unwrap();
        con.execute("INSERT INTO media (filename, rel_path, sha256, byte_size, created_at) VALUES ('neo-a.png', 'media/neo-a.png', '', 9, 0)", [])
            .unwrap();

        // 복습 카드, 학습 중 카드, 일시 정지 카드 하나씩
        let card = |deck_id: i64, ord: i64| -> i64 {
            con.query_row("SELECT id FROM card WHERE deck_id = ?1 AND template_ord = ?2", params![deck_id, ord], |r| r.get(0)).unwrap()
        };
        let now = now_ms();
        let review_due = now + 5 * DAY_MS;
        con.execute(
            "UPDATE card SET state = 'review', due_at = ?2, interval_days = 7, stability = 12.5, difficulty = 4.2, reps = 3, lapses = 1,
                             last_reviewed_at = ?3 WHERE id = ?1",
            params![card(root, 0), review_due, now - 2 * DAY_MS],
        )
        .unwrap();
        let learning_due = now + 600_000;
        con.execute(
            "UPDATE card SET state = 'learning', srs_step = 1, due_at = ?2, stability = 1.5, difficulty = 5.0, reps = 1 WHERE id = ?1",
            params![card(sub, 0), learning_due],
        )
        .unwrap();
        con.execute("UPDATE card SET state = 'suspended' WHERE id = ?1", [card(leaf, 1)]).unwrap();
        // 스케줄러에 넘기지 않은 기록도 내보낸다. 다른 덱의 기록은 빠진다
        for (card_id, at, rating, fed) in [(card(root, 0), now - 9 * DAY_MS, 3, 1), (card(root, 0), now - 2 * DAY_MS, 1, 0), (card(other, 0), now, 3, 1)] {
            con.execute(
                "INSERT INTO review_log (card_id, reviewed_at, rating, elapsed_ms, prev_interval_days, next_interval_days, prev_state, fed_to_scheduler)
                 VALUES (?1, ?2, ?3, 90000, 2, 7, 'review', ?4)",
                params![card_id, at, rating, fed],
            )
            .unwrap();
        }

        let out = tmp.0.join("정보처리기사.apkg");
        let r = export(&con, &a.data_dir, root, &out, true, &QUIET).unwrap();
        assert_eq!(r.missing_cards.len(), 1, "찾지 못한 그림을 쓰는 카드");
        assert_eq!((r.notes, r.cards, r.media, r.missing_media, r.reviews), (3, 5, 1, 1, 2));
        assert!(out.exists() && !tmp.0.join("정보처리기사.apkg.tmp").exists(), "임시 파일은 이름이 바뀌어 남지 않는다");
        let leftovers = std::fs::read_dir(&a.data_dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("export-"));
        assert_eq!(leftovers.count(), 0, "임시 컬렉션도 지운다");

        // 두 번째 DB 로 가져온다 (실제 가져오기 경로 그대로)
        let b = state(&tmp.0.join("b"));
        let res = crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), true, &mut vec![]).unwrap();
        let res = serde_json::to_value(res).unwrap();
        let n = |k: &str| res[k].as_u64().unwrap();
        assert_eq!((n("notes"), n("cards"), n("reviews"), n("media"), n("skipped")), (3, 5, 2, 1, 0));

        let bc = b.db.lock().unwrap();
        let paths = "WITH RECURSIVE t(id, path) AS (SELECT id, name FROM deck WHERE parent_id IS NULL
                     UNION ALL SELECT d.id, t.path || '::' || d.name FROM deck d JOIN t ON d.parent_id = t.id) SELECT path FROM t ORDER BY path";
        assert_eq!(strings(&bc, paths), ["정보처리기사", "정보처리기사::1과목", "정보처리기사::1과목::요약"]);
        let exported = format!("SELECT fields || '|' || tags FROM note WHERE id IN (SELECT note_id FROM card WHERE deck_id IN ({root}, {sub}, {leaf})) ORDER BY 1");
        assert_eq!(strings(&bc, "SELECT fields || '|' || tags FROM note ORDER BY 1"), strings(&con, &exported), "필드와 태그가 그대로");

        let (due, ivl, s, d, reps): (i64, f64, f64, f64, i64) = bc
            .query_row("SELECT due_at, interval_days, stability, difficulty, reps FROM card WHERE state = 'review'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap();
        assert_eq!(db::study_day(due), db::study_day(review_due), "복습 기한은 같은 학습일");
        assert_eq!((ivl, reps), (7.0, 3));
        assert!((s - 12.5).abs() < 1e-4 && (d - 4.2).abs() < 1e-4, "FSRS 기억 상태가 그대로: {s} {d}");
        let due: i64 = bc.query_row("SELECT due_at FROM card WHERE state = 'learning'", [], |r| r.get(0)).unwrap();
        assert_eq!(due, learning_due / 1000 * 1000, "학습 중 기한은 초 단위로 옮긴다");
        assert_eq!(strings(&bc, "SELECT state || count(*) FROM card GROUP BY state ORDER BY state"), ["learning1", "new2", "review1", "suspended1"]);
        assert_eq!(strings(&bc, "SELECT group_concat(rating) FROM (SELECT rating FROM review_log ORDER BY reviewed_at)"), ["3,1"]);
        assert_eq!(std::fs::read(b.data_dir.join("media/neo-a.png")).unwrap(), b"png-bytes");

        // 기록 없이: 모두 새 카드, 일시 정지만 지킨다
        let out = tmp.0.join("no-history.apkg");
        let r = export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        assert_eq!((r.cards, r.reviews), (5, 0));
        let c = state(&tmp.0.join("c"));
        crate::import::run_import_anki(&|_, _, _| {}, &c, out.to_str().unwrap(), true, &mut vec![]).unwrap();
        let cc = c.db.lock().unwrap();
        assert_eq!(strings(&cc, "SELECT state || count(*) FROM card GROUP BY state ORDER BY state"), ["new4", "suspended1"]);
        assert_eq!(strings(&cc, "SELECT count(*) || '' FROM review_log"), ["0"]);
        assert!(export(&con, &a.data_dir, archived, &out, true, &QUIET).is_err(), "보관한 덱은 내보내지 않는다");
        // 취소하면 만들던 파일만 지우고 있던 파일은 그대로
        let stop = Job { emit: &|_, _, _| {}, cancelled: &|| true };
        assert_eq!(export(&con, &a.data_dir, root, &out, true, &stop).unwrap_err(), CANCELLED);
        assert!(out.exists());
        assert!(out.exists(), "실패해도 이미 있던 파일은 그대로");
    }

    /// AU7 소리: 필드의 [sound:] 글자와 소리 파일이 내보내기 · 가져오기를 그대로 지난다. 빠진 파일만 경고 (AU5)
    #[test]
    fn sound_round_trip() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-sound-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('영단어', 0, 0)", []).unwrap();
        let deck = con.last_insert_rowid();
        for (front, back) in [("abandon [sound:voice.mp3]", "버리다 [sound:voice.mp3]"), ("car", "자동차 [sound:gone.mp3]")] {
            insert_note(&con, &NewCard { deck_id: deck, kind: "basic".into(), front: front.into(), back: back.into(), importance: None }, "", None).unwrap();
        }
        std::fs::write(a.data_dir.join("media/voice.mp3"), b"ID3-sound").unwrap();
        let out = tmp.0.join("sound.apkg");
        let r = export(&con, &a.data_dir, deck, &out, false, &QUIET).unwrap();
        assert_eq!((r.media, r.missing_media), (1, 1), "voice.mp3 는 넣고 gone.mp3 는 빠졌다");

        let b = state(&tmp.0.join("b"));
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap()).unwrap();
        let bc = b.db.lock().unwrap();
        assert_eq!(strings(&bc, "SELECT fields FROM note ORDER BY 1"), strings(&con, "SELECT fields FROM note ORDER BY 1"), "[sound:] 글자가 그대로");
        assert_eq!(std::fs::read(b.data_dir.join("media/voice.mp3")).unwrap(), b"ID3-sound");
        let sound: Vec<&Value> = res["issues"].as_array().unwrap().iter().filter(|i| i["kind"] == "missing_sound" || i["kind"] == "sound").collect();
        assert_eq!(sound.len(), 1, "소리가 든 노트 경고(I28)는 없고 빠진 파일 경고만: {sound:?}");
        assert_eq!(sound[0]["detail"], "소리 파일이 빠진 노트 1개 · 학습할 때 그 자리에 \"파일 없음\"으로 보여요");
    }

    /// 내보낸 .apkg 의 그림 가리기 모델 JSON (스키마 11 col.models)
    fn io_model(apkg: &Path, work: &Path) -> Value {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(apkg).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        let path = work.join(format!("models-{}.anki2", now_ms()));
        std::fs::write(&path, raw).unwrap();
        let models: String = Connection::open(&path).unwrap().query_row("SELECT models FROM col", [], |r| r.get(0)).unwrap();
        let models: Value = serde_json::from_str(&models).unwrap();
        models.as_object().unwrap().values().find(|m| m["originalStockKind"] == 6).cloned().expect("그림 가리기 모델")
    }

    /// M8: 그림 가리기 노트를 내보내고 다시 가져오면 칸 · 이름 · 카드 번호가 그대로고, Anki 가 IO 유형으로 알아볼 표시가 붙는다
    #[test]
    fn image_occlusion_round_trip() {
        use crate::occlusion;
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-io-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('해부학', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        std::fs::write(a.data_dir.join("media/neo-io.png"), b"png").unwrap();
        con.execute("INSERT INTO media (filename, rel_path, sha256, byte_size, created_at) VALUES ('neo-io.png', 'media/neo-io.png', '', 3, 0)", [])
            .unwrap();
        let field = "{{c1::image-occlusion:rect:left=.1:top=.2:width=.25:height=.1:label=심장\\: 좌심실:oi=1}}<br>\
                     {{c3::image-occlusion:ellipse:left=.5:top=.5:rx=.1:ry=.05:oi=1}}<br>\
                     {{c3::image-occlusion:rect:left=.6:top=.1:width=.2:height=.1:oi=1}}<br>";
        let ntid: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Image Occlusion' AND anki_model_id IS NULL", [], |r| r.get(0)).unwrap();
        let fields = serde_json::to_string(&[field, "<img src=\"neo-io.png\">", "머리말", "덧붙임", ""]).unwrap();
        con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, 1, 1)", params![ntid, fields]).unwrap();
        let nid = con.last_insert_rowid();
        for ord in [0, 2] {
            con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, ?3, 1, 1)", params![nid, root, ord])
                .unwrap();
        }

        let out = tmp.0.join("io.apkg");
        let r = serde_json::to_value(export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap()).unwrap();
        assert_eq!((r["cards"].as_u64(), r["media"].as_u64(), r["occlusion_labels"].as_u64()), (Some(2), Some(1), Some(1)));
        // Anki 26.9.2 가 내보낸 모델과 같은 표시 (rslib notetype/schema11.rs)
        let m = io_model(&out, &tmp.0);
        assert_eq!((m["type"].as_i64(), m["name"].as_str()), (Some(1), Some("Image Occlusion")), "빈칸 종류");
        let flds: Vec<(String, i64, bool)> = m["flds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["name"].as_str().unwrap().to_string(), f["tag"].as_i64().unwrap(), f["preventDeletion"].as_bool().unwrap()))
            .collect();
        let want = [("Occlusion", 0, true), ("Image", 1, true), ("Header", 2, true), ("Back Extra", 3, true), ("Comments", 4, false)];
        assert_eq!(flds, want.map(|(n, t, p)| (n.to_string(), t, p)));
        assert_eq!((m["css"].as_str(), m["tmpls"][0]["qfmt"].as_str()), (Some(occlusion::CSS), Some(occlusion::QFMT)), "Anki 의 CSS · 템플릿 그대로");

        // 가져오기: 노트 유형 그대로, 스크립트를 "확인이 필요한 카드"로 세지 않는다, 칸 이름이 살아난다
        let b = state(&tmp.0.join("b"));
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap()).unwrap();
        assert_eq!((res["notes"].as_u64(), res["cards"].as_u64(), res["check_cards"].as_u64()), (Some(1), Some(2), Some(0)));
        let bc = b.db.lock().unwrap();
        let (bfields, bnid): (String, i64) = bc.query_row("SELECT fields, id FROM note", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(bfields, fields, "필드가 글자 그대로");
        assert!(occlusion::is_note(&bc, bnid).unwrap());
        let (shapes, hide_all) = occlusion::parse(&serde_json::from_str::<Vec<String>>(&bfields).unwrap()[0]);
        assert_eq!((shapes[0].label.as_str(), hide_all), ("심장: 좌심실", true));
        assert_eq!(strings(&bc, "SELECT group_concat(template_ord) FROM (SELECT template_ord FROM card ORDER BY 1)"), ["0,2"], "칸 번호 = 카드");

        // 가져온 유형(Anki id 있음)을 다시 내보내도 IO 표시가 붙는다
        let broot: i64 = bc.query_row("SELECT deck_id FROM card LIMIT 1", [], |r| r.get(0)).unwrap();
        let out2 = tmp.0.join("io2.apkg");
        export(&bc, &b.data_dir, broot, &out2, false, &QUIET).unwrap();
        assert_eq!(io_model(&out2, &tmp.0)["flds"][1]["tag"].as_i64(), Some(1));
    }

    /// 실제 Anki 가 만든 그림 가리기 .apkg 를 가져와 본다 (손으로 돌린다. 개인 데이터일 수 있어 파일은 저장소에 두지 않는다):
    ///   NEOANKI_IO_APKG=<경로> cargo test real_anki_io_package -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_anki_io_package() {
        let path = std::env::var("NEOANKI_IO_APKG").expect("NEOANKI_IO_APKG");
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-io-real-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0);
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &a, &path, false, &mut vec![]).unwrap()).unwrap();
        println!("import: notes {} cards {} check_cards {} issues {}", res["notes"], res["cards"], res["check_cards"], res["issues"]);
        assert_eq!(res["check_cards"].as_u64(), Some(0), "IO 스크립트를 '확인이 필요한 카드'로 세지 않는다");
        let con = a.db.lock().unwrap();
        let names = crate::commands::note_field_names(&con).unwrap();
        let mut st = con
            .prepare(
                "SELECT n.id, n.fields, nt.name, t.qfmt, t.afmt, c.template_ord FROM card c JOIN note n ON n.id = c.note_id
                 JOIN note_type nt ON nt.id = n.note_type_id JOIN card_template t ON t.note_type_id = nt.id ORDER BY n.id, c.template_ord",
            )
            .unwrap();
        let rows: Vec<(i64, String, String, String, String, u32)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert!(!rows.is_empty());
        for (nid, fields, nt, q, a, ord) in rows {
            assert!(crate::occlusion::is_note(&con, nid).unwrap(), "{nt}");
            let r = crate::commands::render_fields(&names, &nt, &fields, Some(&q), Some(&a), ord);
            println!("note {nid} c{} front: {}\n  back: {}", ord + 1, r.front.replace('\n', " "), r.back.replace('\n', " "));
            assert!(r.front.contains("<div class=\"io\">") && r.front.contains("io-mask--ask") && !r.front.contains("<script"));
        }
        let out = tmp.0.join("again.apkg");
        let root: i64 = con.query_row("SELECT deck_id FROM card LIMIT 1", [], |r| r.get(0)).unwrap();
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        std::fs::copy(&out, std::env::var("NEOANKI_IO_OUT").unwrap_or_else(|_| format!("{path}.neo.apkg"))).unwrap();
    }
}
