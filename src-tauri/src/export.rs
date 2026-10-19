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
    /// 카드 종류(템플릿)가 없어 뺀 카드 수. Anki 는 템플릿 없는 노트 유형이 든 패키지를 통째로 거부한다 (Q2-01 · Q2-02)
    skipped_cards: usize,
    /// 같은 guid 를 가진 두 번째 노트 (두 기기 업데이트로 생긴 두 벌, DU-06). Anki 가 하나로 합치지 않게 새 guid 로 내보냈다
    duplicate_guids: usize,
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
    /// 목록 회상 · 선택형 카드. Anki 에서는 하나씩 공개하거나 보기를 고를 수 없어 확인 창에 알린다 (Q3)
    recall_cards: i64,
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
        recall_cards: count(&format!(
            "SELECT count(*) FROM card c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id
             WHERE c.deck_id IN ($IDS) AND nt.name IN ('{}', '{}')",
            crate::recall::LIST,
            crate::recall::CHOICE
        ))?,
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
    /// 카드가 모두 앞면이 비어 뺀 노트 (Q2-12)
    empty_fronts: usize,
}

/// 열 이름(화면) 과 제목 줄. 제목은 B2-c 가 다시 가져올 때 자동으로 연결되는 이름이다
const CSV_COLUMNS: [(&str, &str); 6] =
    [("front", "앞면"), ("back", "뒷면"), ("deck", "하위 덱"), ("tags", "태그"), ("importance", "중요도"), ("due", "다음 복습일")];

/// C1-xc. 노트 한 줄. UTF-8(BOM), 쉼표, 모든 칸 따옴표 (엑셀에서 한글이 깨지지 않게).
/// 중요도는 직접 정한 것만 쓴다. 비우면 다시 가져올 때 하위 덱 설정을 따른다
#[tauri::command]
pub fn export_csv(state: State<AppState>, deck_id: i64, path: String, columns: Vec<String>, plain_text: bool) -> Res<CsvResult> {
    let con = state.db.lock().map_err(err)?;
    let (out, rows, empty_fronts) = csv_text(&con, deck_id, &columns, plain_text)?;
    // 반쯤 쓴 파일을 남기지 않는다
    let target = Path::new(&path);
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, out).map_err(err)?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })?;
    Ok(CsvResult { rows, path, empty_fronts })
}

/// CSV 글 전체, 줄 수 (제목 줄 제외), 앞면이 비어 뺀 노트 수
fn csv_text(con: &Connection, deck_id: i64, columns: &[String], plain_text: bool) -> Res<(String, usize, usize)> {
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
    // 노트마다 그 덱 안의 첫 카드 종류(ord) 카드 하나의 면. 기기마다 같은 순서로 (만든 시각 · uid, 동기화로 같다. 정수 id 는 기기마다 다르다, Q2-12).
    // 앞면이 빈 카드(NT49)는 건너뛰고 다음 카드 종류를 쓴다. 모두 비었으면 그 노트는 빼고 센다
    let mut st = con
        .prepare(&format!(
            "SELECT n.fields, n.tags, nt.name, nt.fields, nt.native_kind IN ('basic', 'reversed', 'cloze'), c.deck_id, c.importance, c.state, c.due_at, t.qfmt, t.afmt, c.template_ord,
                    n.id, n.note_type_id
             FROM note n JOIN note_type nt ON nt.id = n.note_type_id
             JOIN card c ON c.note_id = n.id AND c.deck_id IN ({in_decks})
             LEFT JOIN card_template t ON t.note_type_id = nt.id AND {}
             ORDER BY n.created_at, n.uid, c.template_ord, c.uid",
            crate::notetype::TEMPLATE_OF_CARD
        ))
        .map_err(err)?;
    let fronts = crate::notetype::Fronts::load(con)?;
    let (mut seen, mut done) = (HashSet::new(), HashSet::new());
    let mut rows = st.query([]).map_err(err)?;
    let mut out = String::from("\u{feff}");
    // 스프레드시트가 수식으로 실행하지 않게 = + - @ 탭 CR 로 시작하는 칸은 앞에 ' 를 붙인다 (OWASP CSV Injection, T7-13). 가져오기(csv.ts)가 다시 뗀다
    let guard = |c: &String| if c.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{c}") } else { c.clone() };
    let line = |cells: Vec<String>| cells.iter().map(|c| format!("\"{}\"", guard(c).replace('"', "\"\""))).collect::<Vec<_>>().join(",") + "\r\n";
    out += &line(cols.iter().map(|(_, h)| h.to_string()).collect());
    let mut n = 0;
    while let Some(r) = rows.next().map_err(err)? {
        let (nid, ntid, ord): (i64, i64, i64) = (r.get(12).map_err(err)?, r.get(13).map_err(err)?, r.get(11).map_err(err)?);
        let raw: String = r.get(0).map_err(err)?;
        seen.insert(nid);
        if done.contains(&nid) || fronts.empty(ntid, ord as u32, &raw) {
            continue;
        }
        done.insert(nid);
        // CSV 는 글자만: [sound:] 은 지운다 (카드 화면의 소리 칩 자리 표시도 남지 않게 렌더 전에)
        let fields: Vec<String> = serde_json::from_str::<Vec<String>>(&raw).map_err(err)?.iter().map(|f| strip_sound(f)).collect();
        // 앱 Basic · 앞뒤 · Cloze 는 필드를 그대로 쓴다 (빈칸 표시 {{c1::}} 도 지킨다). 그 밖은 카드에 그려진 앞면·답을 쓴다
        let native: bool = r.get::<_, Option<bool>>(4).map_err(err)?.unwrap_or(false);
        let (q, a): (Option<String>, Option<String>) = (r.get(9).map_err(err)?, r.get(10).map_err(err)?);
        let q = q.unwrap_or_default();
        let names: Vec<String> = serde_json::from_str(&r.get::<_, String>(3).map_err(err)?).unwrap_or_default();
        let field = |i: usize| fields.get(i).cloned().unwrap_or_default();
        // 가져온 빈칸 유형(.apkg 왕복 포함)도 앱 Cloze 처럼 빈칸 원문을 한 번 쓴다. 카드 한 장의 화면이면 c2 부터의 빈칸 표시가 사라진다
        let cloze = crate::occlusion::cloze_field(&q).and_then(|f| names.iter().position(|n| n == f));
        let (front, back) = if crate::occlusion::is_template(&q) {
            crate::occlusion::csv_cells(&fields)
        } else if native {
            (field(0), field(1))
        } else if let Some(i) = cloze {
            (field(i), field(usize::from(i == 0))) // 뒷면 = 빈칸 필드가 아닌 첫 필드 (Back Extra)
        } else {
            let map: HashMap<String, String> = names.into_iter().zip(fields.iter().cloned()).filter(|(k, _)| !k.is_empty()).collect();
            let shown = render_card(&q, &a.unwrap_or_default(), &map, ord as u32);
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
    Ok((out, n, seen.len() - done.len()))
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
    exam_hits: Option<i64>,
    exam_names: Option<String>,
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
/// 8.4 그림 · 표 영역에서 만든 노트의 출처 태그 (.apkg 만, DECISIONS IT32): `Neo-Anki::출처::4장_회귀분석::슬라이드_3` · `Neo-Anki::출처::3장::12쪽`.
/// 이 앱이 단 태그만 알아보게 `Neo-Anki::` 아래에 둔다. 사용자가 직접 단 `출처::…` 태그는 내보내기 · 가져오기 모두 건드리지 않는다 (QA T2-02).
/// 노트 글은 건드리지 않는다 (IT14). 앱 안 연결은 source_link 가 그대로 하고, 다시 가져오면 이 태그는 뺀다 (import.rs)
pub(crate) const SOURCE_TAG: &str = "Neo-Anki::출처::";

fn source_tags(con: &Connection) -> Res<HashMap<i64, Vec<String>>> {
    let mut st = con
        .prepare(
            "SELECT DISTINCT c.note_id, m.file_name, m.kind, s.page FROM source_link l
             JOIN source_segment s ON s.id = l.segment_id AND s.signals IN ('figure', 'table')
             JOIN source_material m ON m.id = s.material_id JOIN card c ON c.id = l.card_id
             WHERE l.method != 'rejected' ORDER BY c.note_id, m.file_name, s.page",
        )
        .map_err(err)?;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))).map_err(err)?;
    for row in rows {
        let (note, file, kind, page) = row.map_err(err)?;
        // 태그는 띄어쓰기로 나뉘어 기출 태그처럼 _ 로 (exam::to_tags)
        let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s).split_whitespace().collect::<Vec<_>>().join("_");
        let place = if kind == "pptx" { format!("슬라이드_{page}") } else { format!("{page}쪽") };
        let tag = format!("{SOURCE_TAG}{stem}::{place}");
        let list = out.entry(note).or_default();
        if !list.contains(&tag) {
            list.push(tag);
        }
    }
    Ok(out)
}

/// 이전 출처 태그는 빼고 지금 연결로 다시 단다 (쌓이지 않게)
fn with_source_tags(tags: &str, sources: &[String]) -> String {
    without_source_tags(tags).split_whitespace().map(str::to_string).chain(sources.iter().cloned()).collect::<Vec<_>>().join(" ")
}

/// 이 앱이 단 출처 태그만 뺀다. Anki 는 태그 대소문자를 컬렉션에 이미 있는 태그에 맞추므로 영문 부분은 대소문자를 가리지 않는다
pub(crate) fn without_source_tags(tags: &str) -> String {
    let ours = |t: &str| t.get(..SOURCE_TAG.len()).is_some_and(|h| h.eq_ignore_ascii_case(SOURCE_TAG));
    tags.split_whitespace().filter(|t| !ours(t)).collect::<Vec<_>>().join(" ")
}

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
    let mut cards: Vec<Card> = {
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
    let mut notes: Vec<Note> = {
        let mut st = con
            .prepare(&format!(
                "SELECT id, note_type_id, fields, tags, created_at, updated_at, anki_note_id, anki_guid, exam_hits, exam_names FROM note
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
                    exam_hits: r.get(8)?,
                    exam_names: r.get(9)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };

    let sources = source_tags(con)?;
    let root_did = deck_map[&deck_id];
    let mut used = HashSet::new();
    let mut models: HashMap<i64, Model> = HashMap::new();
    for ntid in notes.iter().map(|n| n.note_type_id).collect::<BTreeSet<_>>() {
        models.insert(ntid, model(con, ntid, root_did, &mut used)?);
    }
    // 카드 종류가 없는 카드는 뺀다 (동기화로 남은 고아 카드 · 템플릿이 하나도 없는 유형). Anki 는 그런 유형이 든 패키지를 통째로 거부하고,
    // 다시 센 ord 로 다른 카드 종류에 붙으면 앞면이 빈 카드가 된다. 카드가 모두 빠진 노트 · 유형도 뺀다 (Q2-01 · Q2-02)
    let type_of: HashMap<i64, i64> = notes.iter().map(|n| (n.id, n.note_type_id)).collect();
    let all = cards.len();
    cards.retain(|c| type_of.get(&c.note_id).is_some_and(|t| models[t].draws(c.ord)));
    let skipped_cards = all - cards.len();
    if cards.is_empty() {
        return Err(format!("내보낼 수 있는 카드가 없어요. 카드 종류가 없는 카드 {skipped_cards}장은 Anki가 받지 않아요."));
    }
    let kept: HashSet<i64> = cards.iter().map(|c| c.note_id).collect();
    notes.retain(|n| kept.contains(&n.id));
    let kept: HashSet<i64> = notes.iter().map(|n| n.note_type_id).collect();
    models.retain(|t, _| kept.contains(t));
    // 왕복 기준 (DU55): 만든 사람이 없는 유형만 (사용자 유형은 내보낸 정의, 앱 기본 유형은 내보낸 모델 id)
    for (ntid, m) in &models {
        let our_ord = |a: u32| m.ords.iter().find(|(_, x)| **x == a as i64).map_or(a, |(o, _)| *o as u32);
        crate::import::deck_update::exported_type_origin(con, *ntid, &m.json, &m.slots, our_ord)?;
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
    let mut guids: HashSet<String> = HashSet::new();
    let mut duplicate_guids = 0;
    for (i, n) in notes.iter().enumerate() {
        if i % 200 == 0 {
            job.tick("notes", i, total)?;
        }
        let m = &models[&n.note_type_id];
        // 8.1 자리 고정 필드: 보이는 순서로 다시 배열한다 (지운 자리는 빠진다). Anki 는 필드 수가 노트 유형과 같아야 한다
        let stored: Vec<String> = serde_json::from_str(&n.fields).map_err(err)?;
        let mut flds: Vec<String> = m.slots.iter().map(|s| stored.get(*s).cloned().unwrap_or_default()).collect();
        flds.resize(m.nfields.max(1), String::new());
        if m.io {
            labels += crate::occlusion::label_count(&flds[0]);
        }
        for f in &flds {
            media_refs(f, &mut media);
        }
        let id = unique(&mut used, n.anki_note_id.unwrap_or(n.created_at));
        note_ids.insert(n.id, id);
        // 기출 표시를 태그로 (N20). Anki 에서 보이고, 다시 가져오면 같은 표시가 된다
        let tags = crate::exam::to_tags(&n.tags, n.exam_hits, n.exam_names.clone());
        // 왕복 기준 (DU55): 앱에서 만든 노트만, 처음 내보낼 때의 값. 만든 사람의 원본은 덮지 않는다
        crate::import::deck_update::exported_note_origin(con, n.id, &stored, &tags)?;
        let tags = with_source_tags(&tags, sources.get(&n.id).map_or(&[][..], Vec::as_slice));
        let guid = match &n.anki_guid {
            // 같은 guid 가 이미 나갔으면 이 노트는 새 guid 로 (Anki 는 같은 guid 를 한 노트로 본다)
            Some(g) if !guids.insert(g.clone()) => {
                duplicate_guids += 1;
                guid_for(n.id, n.created_at)
            }
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
                type_of.get(&c.note_id).and_then(|t| models[t].ords.get(&c.ord)).copied().unwrap_or(c.ord),
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
    let result = ExportResult { notes: notes.len(), cards: cards.len(), reviews, occlusion_labels: labels, skipped_cards, duplicate_guids, ..Default::default() };
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
    /// Anki 필드 순서대로 우리 자리 번호 (보이는 순서, 지운 자리는 없다)
    slots: Vec<usize>,
    /// 우리 템플릿 ord -> Anki ord (Anki 는 0 부터 빈틈없이 센다. 지운 ord 의 빈자리를 메운다). 빈칸 유형은 비어 있다 (카드 ord = 빈칸 번호)
    ords: HashMap<i64, i64>,
    /// 템플릿 수와 빈칸 유형인가 (draws)
    templates: usize,
    cloze: bool,
}

impl Model {
    /// 이 ord 의 카드를 그리는 템플릿이 있다 (빈칸 유형은 템플릿 하나가 모든 카드를 그린다)
    fn draws(&self, ord: i64) -> bool {
        self.templates > 0 && (self.cloze || self.ords.contains_key(&ord))
    }
}

/// 노트 유형 -> Anki 모델. 원본 JSON(anki_raw)이 있으면 그대로 쓰고, 없으면 스키마 11 기본값으로 만든다.
/// id 는 Anki 에서 온 유형이면 그 id, 아니면 만든 시각(ms) + 행 번호 (기본 유형 셋은 만든 시각이 같다)
fn model(con: &Connection, ntid: i64, did: i64, used: &mut HashSet<i64>) -> Res<Model> {
    let (name, css, created, updated, anki_id, raw, kind): (String, String, i64, i64, Option<i64>, Option<String>, Option<String>) = con
        .query_row(
            "SELECT name, css, created_at, updated_at, anki_model_id, anki_raw, native_kind FROM note_type WHERE id = ?1",
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
        let templates = v["tmpls"].as_array().map_or(0, Vec::len);
        return Ok(Model { id, json: v, nfields, sortf, io, slots: (0..nfields).collect(), ords: HashMap::new(), templates, cloze: true });
    }

    let nt = crate::notetype::load(con, ntid)?;
    let (slots, fields): (Vec<usize>, Vec<String>) = nt.visible().into_iter().unzip();
    let tmpls: Vec<(i64, String, String, String)> =
        nt.templates.iter().enumerate().map(|(i, t)| (i as i64, t.name.clone(), t.qfmt.clone(), t.afmt.clone())).collect();
    let cloze = nt.cloze();
    let ords: HashMap<i64, i64> = if cloze { HashMap::new() } else { nt.templates.iter().enumerate().map(|(i, t)| (t.ord as i64, i as i64)).collect() };
    let io = tmpls.iter().any(|t| crate::occlusion::is_template(&t.2));
    let id = unique(used, anki_id.unwrap_or(created + ntid));
    // 쉬운 편집 줄 상자 CSS 만 있는 유형(앱 유형 복제본)도 빈 CSS 처럼 Anki 기본 CSS 를 쓰고 그 뒤에 붙인다
    let css = match (crate::notetype::without_layout_css(&css).trim().is_empty(), io) {
        (false, _) => css,
        (true, true) => crate::occlusion::CSS.into(),
        (true, false) => format!(
            "{CSS}{}{}{css}",
            if cloze { CLOZE_CSS } else { "" },
            if crate::recall::is_special(&name, &fields) { crate::recall::CSS } else { "" }
        ),
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
    // Anki 기본 유형 표시 (rslib notetype/schema11.rs). 가져온 · 복제한 유형은 남긴 값, 앱 유형은 종류로 (I78).
    // 없으면 Anki 가 사용자 정의 유형으로 보고 "기본값으로 되돌리기"를 숨긴다
    let native = match kind.as_deref() {
        Some("basic") => Some(1),
        Some("reversed") => Some(2),
        Some("cloze") => Some(5),
        _ => None,
    };
    let stock = nt.stock.or(native).or(io.then_some(crate::occlusion::STOCK_KIND));
    if let Some(k) = stock.filter(|k| *k > 0) {
        json["originalStockKind"] = json!(k);
    }
    // Anki 가 그림 가리기 유형으로 알아보고 IO 편집기를 연다 (Anki 26.9.2 가 내보낸 모델과 같다).
    // 필드는 tag 로 찾는다: 0 Occlusion · 1 Image · 2 Header · 3 Back Extra · 4 Comments. Comments 만 지울 수 있다
    if io {
        for (i, f) in json["flds"].as_array_mut().into_iter().flatten().enumerate().take(5) {
            f["tag"] = json!(i);
            f["preventDeletion"] = json!(i < 4);
        }
    }
    Ok(Model { id, json, nfields: fields.len(), sortf: 0, io, slots, ords, templates: tmpls.len(), cloze })
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
        // 줄은 노트를 만든 시각 순서 (같은 ms 면 uid 순서라 정해 둔다)
        con.execute("UPDATE note SET created_at = id", []).unwrap();
        let all = ["front", "back", "deck", "tags", "importance"].map(String::from);
        let (text, rows, _) = csv_text(&con, root, &all, true).unwrap();
        assert_eq!(rows, 3, "노트마다 한 줄 (앞뒤 카드 두 장도 한 줄)");
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines[0], "\u{feff}\"앞면\",\"뒷면\",\"하위 덱\",\"태그\",\"중요도\"", "BOM + 다시 가져올 때 자동 연결되는 제목");
        assert_eq!(lines[1], r#""TCP, ""연결""","답 둘째 줄","정보처리기사","a b","핵심""#, "따옴표는 두 번, 서식은 빼고 줄바꿈은 띄어쓰기");
        assert!(lines[2].starts_with(r#""{{c1::서울}}은 {{c2::한국}}","","정보처리기사::1과목""#), "빈칸 표시는 지킨다: {}", lines[2]);
        let (html, ..) = csv_text(&con, sub, &["front".to_string()], false).unwrap();
        assert!(html.contains("{{c1::서울}}") && !html.contains("TCP"), "하위 덱만");
        assert!(csv_text(&con, root, &[], true).is_err());

        // T7-13 수식 주입: = + - @ 로 시작하는 칸은 ' 를 붙여 글자로 (OWASP)
        add(sub, "basic", "=HYPERLINK(\"http://x\",\"y\")", "+SUM(1,2)", None);
        add(sub, "basic", "@cmd", "-2+3", None);
        let (text, ..) = csv_text(&con, sub, &["front".to_string(), "back".to_string()], true).unwrap();
        assert!(text.contains(r#""'=HYPERLINK(""http://x"",""y"")","'+SUM(1,2)""#), "{text}");
        assert!(text.contains(r#""'@cmd","'-2+3""#) && text.contains(r#""앞","뒤""#), "다른 칸은 그대로: {text}");
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
        assert_eq!(sound[0]["detail"], "소리 파일이 빠진 노트 1개 · 학습할 때 그 자리에 \"소리 파일을 찾을 수 없어요\"로 보여요");
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

    /// 쉬운 편집 줄 모양: 앱 유형 복제본(빈 CSS)에 줄 상자를 쓰면 노트 CSS 는 줄 상자 CSS 뿐이다. Anki 에는 기본 CSS 뒤에 붙여 같은 모양으로 나간다
    /// 쉬운 편집 줄 모양을 Anki 로: 앱 유형 복제본(빈 CSS)은 노트 CSS 가 줄 상자 CSS 뿐이라 Anki 기본 CSS 뒤에 붙는다.
    /// Anki 기본 `.card { text-align: center }` 아래에서도 줄 상자(앞면 L · 가운데, 뒷면 M · 왼쪽)가 제 정렬을 가진다 (NT41)
    #[test]
    fn layout_css_exports_after_anki_default() {
        use crate::notetype as nt;
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-ntcss-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('줄', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let id = nt::clone_type(&con, db::native_type_id(&con, "basic").unwrap(), Some("줄 모양")).unwrap();
        let field = |slot, name: &str| nt::FieldEdit { slot: Some(slot), name: name.into() };
        // 쉬운 편집이 만든 템플릿: 모양을 정하지 않은 줄도 그 면의 기본 모양 상자로
        let layout = nt::layout_of("{{질문}}", "{{FrontSide}}<hr id=answer>{{답변}}").unwrap();
        let (qfmt, afmt) = nt::build_layout(&layout);
        assert_eq!((qfmt.as_str(), afmt.as_str()), (r#"<div class="nt-l nt-center">{{질문}}</div>"#, r#"{{FrontSide}}<hr id=answer><div class="nt-m nt-left">{{답변}}</div>"#));
        let edit = nt::TypeEdit {
            fields: vec![field(0, "질문"), field(1, "답변")],
            templates: vec![nt::TemplateEdit { ord: Some(0), name: None, qfmt: qfmt.clone(), afmt: afmt.clone() }],
            css: String::new(),
        };
        nt::apply_edit(&con, id, &edit).unwrap();
        assert_eq!(nt::load(&con, id).unwrap().css, nt::LAYOUT_CSS);
        let typed = nt::TypedNote { deck_id: root, note_type_id: id, values: vec!["q".into(), "a".into()], importance: None };
        nt::add_note(&con, &typed).unwrap();
        let model_id: i64 = con.query_row("SELECT anki_model_id FROM note_type WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        let out = tmp.0.join("css.apkg");
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        let col = tmp.0.join("css.anki2");
        std::fs::write(&col, raw).unwrap();
        let c = Connection::open(&col).unwrap();
        let models: Value = serde_json::from_str(&c.query_row("SELECT models FROM col", [], |r| r.get::<_, String>(0)).unwrap()).unwrap();
        let m = &models[model_id.to_string()];
        let css = m["css"].as_str().unwrap().to_string();
        assert!(css.starts_with(CSS) && css.ends_with(nt::LAYOUT_CSS), "{css}");
        // 내보낸 HTML + CSS: 상자 그대로, 정렬 규칙이 .card 뒤에 있어 상자가 제 정렬을 가진다 (상자 안 글자는 .card 의 가운데를 물려받지 않는다)
        assert_eq!((m["tmpls"][0]["qfmt"].as_str(), m["tmpls"][0]["afmt"].as_str()), (Some(qfmt.as_str()), Some(afmt.as_str())));
        let at = |rule: &str| css.find(rule).unwrap_or_else(|| panic!("{rule} 없음: {css}"));
        assert!(at(".card {") < at("div.nt-center { text-align: center; }") && at(".card {") < at("div.nt-left { text-align: left; }"));
        assert!(at("div.nt-l { font-size: 30px; }") > 0 && at("div.nt-m { font-size: 20px; }") > 0);
    }

    /// 8.1 사용자 유형 (필드 순서 바꿈 · 지운 필드 · 더한 템플릿 · 지운 템플릿 번호) -> .apkg -> 새 DB 와 같은 DB 로 가져오기
    #[test]
    fn user_note_type_round_trip() {
        use crate::notetype as nt;
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-nt-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('영단어', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let id = nt::anki_clone(&con, "basic", Some("내 단어"));
        nt::add_field(&con, id, "예문").unwrap();
        nt::add_field(&con, id, "메모").unwrap();
        nt::reorder_fields(&con, id, &[2, 0, 1, 3]).unwrap();
        nt::add_template(&con, id, Some("거꾸로"), "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        nt::add_template(&con, id, Some("예문"), "{{예문}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        nt::delete_template(&con, id, 1, false).unwrap();
        nt::save_css(&con, id, ".card { color: red; }").unwrap();
        let typed = |v: [&str; 4]| nt::TypedNote { deck_id: root, note_type_id: id, values: v.map(String::from).to_vec(), importance: None };
        assert_eq!(nt::add_note(&con, &typed(["He ate an apple.", "apple", "사과", "비밀 메모"])).unwrap(), 2);
        nt::delete_field(&con, id, 3).unwrap();
        let model_id: i64 = con.query_row("SELECT anki_model_id FROM note_type WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(strings(&con, "SELECT fields FROM note"), [r#"["apple","사과","He ate an apple.","비밀 메모"]"#]);

        let out = tmp.0.join("nt.apkg");
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        // Anki 모델: 보이는 순서의 필드 (지운 필드 없음), 템플릿 ord 0 · 1, 만들 때 정한 모델 id
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        let col = tmp.0.join("nt.anki2");
        std::fs::write(&col, raw).unwrap();
        let c = Connection::open(&col).unwrap();
        let models: Value = serde_json::from_str(&c.query_row("SELECT models FROM col", [], |r| r.get::<_, String>(0)).unwrap()).unwrap();
        let m = &models[model_id.to_string()];
        let names: Vec<&str> = m["flds"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["예문", "Front", "Back"]);
        let tmpls: Vec<(i64, &str)> = m["tmpls"].as_array().unwrap().iter().map(|t| (t["ord"].as_i64().unwrap(), t["name"].as_str().unwrap())).collect();
        assert_eq!(tmpls, [(0, "Card 1"), (1, "예문")]);
        assert_eq!((m["originalStockKind"].as_i64(), m["css"].as_str(), m["type"].as_i64()), (Some(1), Some(".card { color: red; }"), Some(0)));
        assert_eq!(m["req"], json!([[0, "any", [1]], [1, "any", [0]]]), "req 는 Anki 필드 번호");
        let flds: String = c.query_row("SELECT flds FROM notes", [], |r| r.get(0)).unwrap();
        assert_eq!(flds, ["He ate an apple.", "apple", "사과"].join(SEP), "지운 필드 값은 나가지 않는다");
        assert_eq!(strings(&c, "SELECT group_concat(ord) FROM (SELECT ord FROM cards ORDER BY ord)"), ["0,1"]);

        // 새 DB: 자리 = Anki 순서, 카드 ord 그대로
        let b = state(&tmp.0.join("b"));
        crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap();
        {
            let bc = b.db.lock().unwrap();
            let bid: i64 = bc.query_row("SELECT id FROM note_type WHERE anki_model_id = ?1", [model_id], |r| r.get(0)).unwrap();
            let t = nt::load(&bc, bid).unwrap();
            assert_eq!((t.name.as_str(), t.slots.clone(), t.css.as_str()), ("내 단어", vec!["예문".to_string(), "Front".into(), "Back".into()], ".card { color: red; }"));
            assert_eq!(t.templates.iter().map(|t| (t.ord, t.qfmt.as_str())).collect::<Vec<_>>(), [(0, r#"<div class="nt-l nt-center">{{Front}}</div>"#), (1, "{{예문}}")]);
            assert_eq!(strings(&bc, "SELECT fields FROM note"), [r#"["He ate an apple.","apple","사과"]"#]);
            let names = crate::commands::note_field_names(&bc).unwrap();
            let q: String = bc.query_row(&format!("SELECT t.qfmt FROM card c JOIN card_template t ON t.note_type_id = {bid} AND {} WHERE c.template_ord = 1", nt::TEMPLATE_OF_CARD), [], |r| r.get(0)).unwrap();
            let r = crate::commands::render_fields(&names, bid, &strings(&bc, "SELECT fields FROM note")[0], Some(&q), Some(""), 1);
            assert_eq!(r.front, "He ate an apple.");
        }

        // 같은 DB 로 다시 (Anki 에서 새로 만든 노트처럼 guid 가 없는 노트): 유형은 덮지 않고, 값은 자리로 · 카드는 우리 ord 로
        con.execute("DELETE FROM note", []).unwrap();
        drop(con);
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &a, out.to_str().unwrap(), false, &mut vec![]).unwrap()).unwrap();
        assert_eq!((res["notes"].as_u64(), res["cards"].as_u64()), (Some(1), Some(2)));
        let con = a.db.lock().unwrap();
        assert_eq!(strings(&con, "SELECT count(*) || '' FROM note_type WHERE user_made = 1"), ["1"]);
        assert_eq!(strings(&con, "SELECT fields FROM note"), [r#"["apple","사과","He ate an apple.",""]"#]);
        assert_eq!(strings(&con, "SELECT group_concat(template_ord) FROM (SELECT template_ord FROM card ORDER BY 1)"), ["0,2"]);
    }

    #[test]
    fn cards_without_card_type_are_left_out() {
        // Q2-01 · Q2-02: 동기화로 템플릿이 하나도 없는 유형 · 지워진 카드 종류의 카드가 남았다. 그대로 쓰면 Anki 가 패키지를 통째로 거부하거나
        // 다시 센 ord 로 다른 카드 종류에 붙어 앞면이 빈 카드가 된다. 그 카드는 빼고 수를 알린다. 다른 카드는 나간다
        use crate::notetype as nt;
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-orphan-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('영단어', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let typed = |id: i64, v: [&str; 2]| nt::add_note(&con, &nt::TypedNote { deck_id: root, note_type_id: id, values: v.map(String::from).to_vec(), importance: None }).unwrap();
        let ok = nt::anki_clone(&con, "basic", Some("멀쩡한 유형"));
        nt::add_template(&con, ok, Some("거꾸로"), "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        typed(ok, ["apple", "사과"]);
        // 고아 카드: 카드 종류 ord 1 을 지운 뒤에 남은 카드 (동기화가 되살렸다)
        con.execute("DELETE FROM card_template WHERE note_type_id = ?1 AND ord = 1", [ok]).unwrap();
        let empty = nt::anki_clone(&con, "basic", Some("템플릿 없는 유형"));
        typed(empty, ["river", "강"]);
        con.execute("DELETE FROM card_template WHERE note_type_id = ?1", [empty]).unwrap();
        let out = tmp.0.join("o.apkg");
        let r = serde_json::to_value(export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap()).unwrap();
        assert_eq!((r["notes"].as_u64(), r["cards"].as_u64(), r["skipped_cards"].as_u64()), (Some(1), Some(1), Some(2)));
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        let col = tmp.0.join("o.anki2");
        std::fs::write(&col, raw).unwrap();
        let c = Connection::open(&col).unwrap();
        let models: Value = serde_json::from_str(&c.query_row("SELECT models FROM col", [], |r| r.get::<_, String>(0)).unwrap()).unwrap();
        assert!(models.as_object().unwrap().values().all(|m| !m["tmpls"].as_array().unwrap().is_empty()), "템플릿 없는 모델은 넣지 않는다");
        assert_eq!(strings(&c, "SELECT group_concat(ord) FROM cards"), ["0"]);
        // 모두 빠지면 내보내지 않고 알린다
        con.execute("DELETE FROM card WHERE note_id IN (SELECT id FROM note WHERE note_type_id = ?1)", [ok]).unwrap();
        assert!(export(&con, &a.data_dir, root, &tmp.0.join("p.apkg"), false, &QUIET).unwrap_err().contains("카드 종류가 없는 카드 1장"));
    }

    #[test]
    fn csv_rows_follow_note_and_card_type_order() {
        // Q2-12: 줄 순서와 쓰는 카드는 기기마다 같아야 한다 (정수 id 는 기기마다 다르다). 노트는 만든 시각 · uid, 카드는 카드 종류 순서.
        // 앞면이 빈 카드(NT49)는 건너뛰고, 모두 비었으면 그 노트는 빼고 센다
        use crate::notetype as nt;
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-csvord-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('영단어', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let id = nt::anki_clone(&con, "basic", Some("내 단어"));
        nt::add_field(&con, id, "예문").unwrap();
        // 첫 카드 종류는 예문이 앞면: 예문이 없는 노트는 그 카드가 앞면 빔
        nt::save_template(&con, id, 0, None, "{{예문}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        nt::add_template(&con, id, Some("뜻"), "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        let note = |created: i64, v: [&str; 3]| {
            con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", params![id, json!(v).to_string(), created]).unwrap();
            let n = con.last_insert_rowid();
            // 받은 순서처럼 큰 ord 카드가 먼저 생겼다 (id 가 더 작다)
            for ord in [1, 0] {
                con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 0)", params![n, root, ord]).unwrap();
            }
        };
        note(2000, ["cloud", "구름", "White clouds."]);
        note(1000, ["star", "별", ""]);
        note(3000, ["", "", ""]);
        con.execute("UPDATE note SET fields = json_array('', '', '') WHERE created_at = 3000", []).unwrap();
        let (text, rows, empty) = csv_text(&con, root, &["front".to_string(), "back".to_string()], true).unwrap();
        let lines: Vec<&str> = text.split("\r\n").skip(1).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, [r#""별","star""#, r#""White clouds.","cloud""#], "만든 시각 순, 예문 앞면이 비면 다음 카드 종류");
        assert_eq!((rows, empty), (2, 1));
    }

    #[test]
    fn reimport_after_deleting_a_card_type_and_a_field() {
        // Q2-08: 같은 모델 id 로 다시 가져올 때 이 앱에서 지운 카드 종류의 카드는 같은 순번의 다른 카드 종류에 붙지 않고 빠진다.
        // 그 카드의 복습 기록은 카드 없이 통계에만 남고, 결과에 한 줄로 알린다.
        // Q2-13: 이 앱에서 지운(숨긴) 필드의 값은 숨긴 자리에 넣고 알린다 (NT10)
        use crate::notetype as nt;
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-redel-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('영단어', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let id = nt::anki_clone(&con, "basic", Some("단어장"));
        nt::add_field(&con, id, "메모").unwrap();
        let rev = nt::add_template(&con, id, Some("뜻 → 단어"), "{{Back}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap().ord;
        nt::add_template(&con, id, Some("메모 → 단어"), "{{Back}} {{메모}}", "{{FrontSide}}<hr id=answer>{{Front}}").unwrap();
        let typed = nt::TypedNote { deck_id: root, note_type_id: id, values: ["yak", "야크", "털이 긴 소"].map(String::from).to_vec(), importance: None };
        assert_eq!(nt::add_note(&con, &typed).unwrap(), 3);
        let card: i64 = con.query_row("SELECT id FROM card WHERE template_ord = ?1", [rev], |r| r.get(0)).unwrap();
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating, fed_to_scheduler) VALUES (?1, 1700000000000, 3, 1)", [card]).unwrap();
        let out = tmp.0.join("w.apkg");
        export(&con, &a.data_dir, root, &out, true, &QUIET).unwrap();
        // 이 앱에서 "뜻 → 단어"와 "메모" 필드를 지우고 (메모를 쓰는 카드 종류도), 노트를 지운 뒤 다시 가져온다
        nt::delete_template(&con, id, rev, false).unwrap();
        let memo = nt::load(&con, id).unwrap().templates.iter().find(|t| t.name == "메모 → 단어").unwrap().ord;
        nt::delete_template(&con, id, memo, false).unwrap();
        nt::delete_field(&con, id, 2).unwrap();
        con.execute("DELETE FROM note", []).unwrap();
        con.execute("DELETE FROM review_log", []).unwrap();
        drop(con);
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &a, out.to_str().unwrap(), true, &mut vec![]).unwrap()).unwrap();
        assert_eq!((res["notes"].as_u64(), res["cards"].as_u64()), (Some(1), Some(1)), "{res}");
        let issues: Vec<&str> = res["issues"].as_array().unwrap().iter().map(|i| i["detail"].as_str().unwrap()).collect();
        assert!(issues.iter().any(|d| d.contains("이 앱에서 지운 카드 종류 '뜻 → 단어'(단어장)의 카드 1장은 넣지 않았어요. 그 카드의 복습 기록 1건은 통계에만 남겼어요")), "{issues:?}");
        assert!(issues.iter().any(|d| d.contains("이 앱에서 지운 필드 '메모'의 값이 있는 노트 1개")), "{issues:?}");
        let con = a.db.lock().unwrap();
        assert_eq!(strings(&con, "SELECT group_concat(template_ord) FROM card"), ["0"], "다른 카드 종류에 붙지 않는다");
        assert_eq!(strings(&con, "SELECT fields FROM note"), [r#"["yak","야크","털이 긴 소"]"#], "숨긴 자리에 값을 남긴다");
        assert_eq!(strings(&con, "SELECT count(*) || '' FROM review_log WHERE card_id IS NULL"), ["1"], "기록은 통계에만");
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

    /// Q: 내보내고 새 DB 로 가져온다 (실제 가져오기 경로 그대로). 가져온 DB 와 결과
    fn recall_round_trip(tmp: &Path, a: &AppState, deck_id: i64) -> (AppState, Value) {
        let out = tmp.join("q.apkg");
        export(&a.db.lock().unwrap(), &a.data_dir, deck_id, &out, true, &QUIET).unwrap();
        let b = state(&tmp.join("b"));
        let res = crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), true, &mut vec![]).unwrap();
        (b, serde_json::to_value(res).unwrap())
    }

    /// Q1 · Q3 · Q10: 목록 노트 1개 = 카드 1장 → 빠뜨림 추천 → 내보내고 다시 가져오면 목록으로 알아보고 복습 기록이 1:1
    #[test]
    fn list_recall_round_trip() {
        use crate::recall::{self, Special};
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-list-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let deck_id = {
            let con = a.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('정보처리기사', 0, 0)", []).unwrap();
            let deck_id = con.last_insert_rowid();
            let items = ["응용 계층", "표현 계층", "세션 계층", "전송 계층", "네트워크 계층", "데이터 링크 계층", "물리 계층"];
            let card = recall::ListNew {
                deck_id,
                question: "OSI 7계층을 위에서부터 차례로 말해 보세요.".into(),
                items: items.map(String::from).to_vec(),
                ordered: true,
                extra: "<b>All People Seem To Need Data Processing</b>".into(),
                importance: Some(2),
            };
            recall::insert(&con, recall::LIST, deck_id, &recall::list_fields(&card).unwrap(), card.importance, "", None).unwrap();
            let id: i64 = con.query_row("SELECT id FROM card", [], |r| r.get(0)).unwrap();
            assert_eq!(con.query_row("SELECT count(*) FROM card", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "노트 1개 = 카드 1장");
            assert_eq!(recall::suggest_for(&con, id, &[3, 6]).unwrap(), Some(2), "7개 중 5개 기억 → Hard (Q1-b)");
            assert_eq!(recall::suggest_for(&con, id, &[3, 3, 99]).unwrap(), Some(3), "같은 번호 · 없는 번호는 세지 않는다");
            con.execute(
                "INSERT INTO review_log (card_id, reviewed_at, rating, elapsed_ms, prev_state, fed_to_scheduler) VALUES (?1, ?2, 2, 5000, 'new', 1)",
                params![id, now_ms() - DAY_MS],
            )
            .unwrap();
            con.execute("INSERT INTO recall_mark (card_id, marks, marked_at) VALUES (?1, '[3,6]', 1)", [id]).unwrap();
            // Anki 쪽 모습: 앞면 질문 + 순서, 뒷면 항목 (스크립트 없음)
            let names = crate::commands::note_field_names(&con).unwrap();
            let fields: String = con.query_row("SELECT fields FROM note", [], |r| r.get(0)).unwrap();
            let nt = crate::db::native_type_id(&con, "list").unwrap();
            let r = crate::commands::render_fields(&names, nt, &fields, Some(recall::LIST_QFMT), Some(recall::LIST_AFMT), 0);
            assert!(r.front.contains("순서대로") && !r.front.contains("세션 계층"));
            assert!(crate::commands::answer_part(&r.back).contains("응용 계층<br>표현 계층"));
            deck_id
        };
        let (b, res) = recall_round_trip(&tmp.0, &a, deck_id);
        let n = |k: &str| res[k].as_u64();
        assert_eq!((n("notes"), n("cards"), n("reviews"), n("check_cards")), (Some(1), Some(1), Some(1), Some(0)));
        let bc = b.db.lock().unwrap();
        let id: i64 = bc.query_row("SELECT id FROM card", [], |r| r.get(0)).unwrap();
        let Some(Special::List { items, ordered, question, extra }) = recall::card_special(&bc, id).unwrap() else { panic!("목록으로 알아본다") };
        assert_eq!((items.len(), items[2].as_str(), ordered), (7, "세션 계층", true));
        assert!(question.starts_with("OSI 7계층") && extra.contains("<b>"));
        assert_eq!(recall::suggest_for(&bc, id, &[3, 6]).unwrap(), Some(2));
        assert_eq!(bc.query_row("SELECT count(*) FROM recall_mark", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "빠뜨림은 이 기기에만 (Q9)");
    }

    /// Q5 · Q7 · Q10: 선택형. 정답 번호 · 섞기 · 해설이 그대로고, Anki 앞면은 보기 목록 · 뒷면은 정답 굵게 (CSS)
    #[test]
    fn choice_round_trip() {
        use crate::recall::{self, Special};
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-choice-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let deck_id = {
            let con = a.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('네트워크', 0, 0)", []).unwrap();
            let deck_id = con.last_insert_rowid();
            let card = recall::ChoiceNew {
                deck_id,
                question: "TCP 3-way handshake에서 두 번째로 오가는 것은?".into(),
                options: ["SYN", "SYN-ACK", "ACK", "FIN"].map(String::from).to_vec(),
                correct: vec![1],
                shuffle: true,
                extra: "서버가 SYN을 받으면 SYN-ACK로 답해요.".into(),
                importance: None,
            };
            recall::insert(&con, recall::CHOICE, deck_id, &recall::choice_fields(&card).unwrap(), None, "", None).unwrap();
            let id: i64 = con.query_row("SELECT id FROM card", [], |r| r.get(0)).unwrap();
            assert_eq!(recall::suggest_for(&con, id, &[3]).unwrap(), Some(1), "ACK 를 고름 → Again (Q2)");
            assert_eq!(recall::suggest_for(&con, id, &[2]).unwrap(), Some(3));
            assert_eq!(recall::suggest_for(&con, id, &[]).unwrap(), None, "고르지 않으면 추천 없음");
            let names = crate::commands::note_field_names(&con).unwrap();
            let fields: String = con.query_row("SELECT fields FROM note", [], |r| r.get(0)).unwrap();
            let nt = crate::db::native_type_id(&con, "choice").unwrap();
            let r = crate::commands::render_fields(&names, nt, &fields, Some(recall::CHOICE_QFMT), Some(recall::CHOICE_AFMT), 0);
            assert!(r.front.contains("<li class=\"o2\">SYN-ACK</li>") && !r.front.contains("o5") && !r.front.contains("<script"));
            assert!(r.back.contains("data-answer=\",2,\""));
            deck_id
        };
        let (b, res) = recall_round_trip(&tmp.0, &a, deck_id);
        assert_eq!((res["cards"].as_u64(), res["check_cards"].as_u64()), (Some(1), Some(0)), "스크립트가 없다");
        let bc = b.db.lock().unwrap();
        let id: i64 = bc.query_row("SELECT id FROM card", [], |r| r.get(0)).unwrap();
        let Some(Special::Choice { options, correct, shuffle, extra, .. }) = recall::card_special(&bc, id).unwrap() else { panic!("선택형으로 알아본다") };
        assert_eq!(options.iter().map(|o| o.1.as_str()).collect::<Vec<_>>(), ["SYN", "SYN-ACK", "ACK", "FIN"]);
        assert_eq!((correct, shuffle, extra.as_str()), (vec![2], true, "서버가 SYN을 받으면 SYN-ACK로 답해요."));
        let css: String = bc.query_row("SELECT css FROM note_type WHERE name = ?1 AND anki_model_id IS NOT NULL", [recall::CHOICE], |r| r.get(0)).unwrap();
        assert!(css.contains("[data-answer*=\",2,\"] .o2"), "정답 굵게는 CSS 로");
        drop(bc);

        // Q10 이견: Anki 에서 필드를 지운 선택형은 알아보지 못하고 가져오기 결과에 한 줄
        let c = state(&tmp.0.join("c"));
        let cc = c.db.lock().unwrap();
        cc.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('바꾼 덱', 0, 0)", []).unwrap();
        let deck = cc.last_insert_rowid();
        cc.execute("INSERT INTO note_type (name, fields, created_at, updated_at) VALUES (?1, '[\"질문\",\"보기1\",\"정답\"]', 0, 0)", [recall::CHOICE])
            .unwrap();
        let nt = cc.last_insert_rowid();
        cc.execute("INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt) VALUES (?1, 0, 'Card 1', '{{질문}}', '{{보기1}}')", [nt]).unwrap();
        cc.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, '[\"q\",\"a\",\"1\"]', 0, 0)", [nt]).unwrap();
        cc.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, 0, 0, 0)", params![cc.last_insert_rowid(), deck])
            .unwrap();
        let out = tmp.0.join("changed.apkg");
        export(&cc, &c.data_dir, deck, &out, false, &QUIET).unwrap();
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap()).unwrap();
        assert!(res["issues"].as_array().unwrap().iter().any(|i| i["kind"] == "recall_type"), "{res}");
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
                "SELECT n.id, n.fields, nt.id, t.qfmt, t.afmt, c.template_ord FROM card c JOIN note n ON n.id = c.note_id
                 JOIN note_type nt ON nt.id = n.note_type_id JOIN card_template t ON t.note_type_id = nt.id ORDER BY n.id, c.template_ord",
            )
            .unwrap();
        let rows: Vec<(i64, String, i64, String, String, u32)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert!(!rows.is_empty());
        for (nid, fields, nt, q, a, ord) in rows {
            assert!(crate::occlusion::is_note(&con, nid).unwrap(), "{nt}");
            let r = crate::commands::render_fields(&names, nt, &fields, Some(&q), Some(&a), ord);
            println!("note {nid} c{} front: {}\n  back: {}", ord + 1, r.front.replace('\n', " "), r.back.replace('\n', " "));
            assert!(r.front.contains("<div class=\"io\">") && r.front.contains("io-mask--ask") && !r.front.contains("<script"));
        }
        let out = tmp.0.join("again.apkg");
        let root: i64 = con.query_row("SELECT deck_id FROM card LIMIT 1", [], |r| r.get(0)).unwrap();
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        std::fs::copy(&out, std::env::var("NEOANKI_IO_OUT").unwrap_or_else(|_| format!("{path}.neo.apkg"))).unwrap();
    }

    /// QA (실제 Anki 26.9.2 왕복, 2026-10-16): Anki 가 내보낸 기본 유형(originalStockKind 1 Basic · 2 앞뒤 · 4 type-in · 5 Cloze)을
    /// 가져와 다시 내보내면 그 표시가 빠진다. 필드 · 카드 · 기록은 그대로라 Anki 에서 공부는 되지만 유형이 사용자 정의로 보인다
    /// ("기본값으로 되돌리기"가 사라진다). Anki 처럼 모델에 표시를 붙인 패키지로 확인한다 (I78)
    #[test]
    fn imported_stock_kind_survives_export() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-stock-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('QA', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        insert_note(&con, &NewCard { deck_id: root, kind: "basic".into(), front: "앞".into(), back: "뒤".into(), importance: None }, "", None).unwrap();
        let ours = tmp.0.join("ours.apkg");
        export(&con, &a.data_dir, root, &ours, false, &QUIET).unwrap();

        // Anki 가 내보낸 것처럼: Basic 모델에 originalStockKind 1
        let mut zip = zip::ZipArchive::new(File::open(&ours).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        let col = tmp.0.join("anki.anki2");
        std::fs::write(&col, raw).unwrap();
        {
            let c = Connection::open(&col).unwrap();
            let mut models: Value = serde_json::from_str(&c.query_row("SELECT models FROM col", [], |r| r.get::<_, String>(0)).unwrap()).unwrap();
            for m in models.as_object_mut().unwrap().values_mut() {
                m["originalStockKind"] = json!(1);
            }
            c.execute("UPDATE col SET models = ?1", [models.to_string()]).unwrap();
        }
        let anki = tmp.0.join("anki.apkg");
        let mut w = zip::ZipWriter::new(File::create(&anki).unwrap());
        w.start_file("collection.anki2", SimpleFileOptions::default()).unwrap();
        w.write_all(&std::fs::read(&col).unwrap()).unwrap();
        w.start_file("media", SimpleFileOptions::default()).unwrap();
        w.write_all(b"{}").unwrap();
        w.finish().unwrap();

        let b = state(&tmp.0.join("b"));
        crate::import::run_import_anki(&|_, _, _| {}, &b, anki.to_str().unwrap(), false, &mut vec![]).unwrap();
        let bc = b.db.lock().unwrap();
        let broot: i64 = bc.query_row("SELECT id FROM deck WHERE name = 'QA'", [], |r| r.get(0)).unwrap();
        let again = tmp.0.join("again.apkg");
        export(&bc, &b.data_dir, broot, &again, false, &QUIET).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&again).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        std::fs::write(&col, raw).unwrap();
        let models: Value =
            serde_json::from_str(&Connection::open(&col).unwrap().query_row("SELECT models FROM col", [], |r| r.get::<_, String>(0)).unwrap()).unwrap();
        let basic = models.as_object().unwrap().values().find(|m| m["name"] == "Basic").unwrap();
        assert_eq!(basic["originalStockKind"], 1, "가져온 기본 유형의 표시가 다시 내보낼 때도 남아야 한다");
    }

    /// N20: 기출 표시가 Anki 태그로 나가고, 새 DB 로 다시 가져오면 같은 표시가 된다. 지운 표시(0)는 태그도 뺀다
    #[test]
    fn exam_marks_survive_round_trip() {
        use crate::exam::{self, ExamState};
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-exam-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('QA', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let add = |front: &str, tags: &str| -> i64 {
            insert_note(&con, &NewCard { deck_id: root, kind: "basic".into(), front: front.into(), back: "b".into(), importance: None }, tags, None).unwrap();
            con.query_row("SELECT max(id) FROM note", [], |r| r.get(0)).unwrap()
        };
        let (named, removed, plain) = (add("1", "과목::1 과목::기출::2023"), add("2", "기출 중요"), add("3", ""));
        add("4", "과목::1");
        // 앱처럼: 열 때 태그에서 채우고, 화면에서 표시 · 지우기
        exam::fill_from_tags(&con).unwrap();
        exam::mark(&con, "note", &[named], Some("2024 기말")).unwrap();
        exam::mark(&con, "note", &[plain], None).unwrap();
        exam::set(&con, "note", &[ExamState { id: removed, hits: None, names: vec![] }]).unwrap();
        let out = tmp.0.join("exam.apkg");
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();

        let b = state(&tmp.0.join("b"));
        crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap();
        let bc = b.db.lock().unwrap();
        let rows = |c: &Connection| -> Vec<(String, ExamState)> {
            let mut st = c.prepare("SELECT tags, exam_hits, exam_names FROM note ORDER BY fields").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, ExamState::of(0, r.get(1)?, r.get(2)?)))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        let (before, after) = (rows(&con), rows(&bc));
        let tags: Vec<&str> = after.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(tags, ["과목::1 과목::기출::2023 기출::2024_기말", "중요", "기출", "과목::1"], "앞 단계가 있는 기출 태그는 그대로, 정하지 않은 노트는 손대지 않는다");
        assert_eq!(after.into_iter().map(|r| r.1).collect::<Vec<_>>(), before.into_iter().map(|r| r.1).collect::<Vec<_>>());
    }

    /// IT: 그림 · 표 영역에 이은 노트만 .apkg 에 출처 태그가 붙고 (노트 글은 그대로), 다시 가져오면 그 태그는 빠진다
    #[test]
    fn region_source_tag_on_apkg_only() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-srctag-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('회귀', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let add = |front: &str, tags: &str| -> i64 {
            insert_note(&con, &NewCard { deck_id: root, kind: "basic".into(), front: front.into(), back: "b".into(), importance: None }, tags, None).unwrap();
            con.query_row("SELECT max(id) FROM card", [], |r| r.get(0)).unwrap()
        };
        let (fig, text) = (add("그림 카드", "과목::1 출처::4장_회귀분석::슬라이드_3 neo-anki::출처::옛것::1쪽"), add("글 카드", ""));
        add("따로", "중요");
        con.execute_batch(&format!(
            "INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, created_at, updated_at) VALUES (1, {root}, '4장 회귀분석.pptx', 'sources/1.pptx', 'pptx', 0, 0), (2, {root}, '3장.pdf', 'sources/2.pdf', 'pdf', 0, 0);
             INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight, rects) VALUES (1, 1, 3, 0, '그림', 'figure', 0, '[[0,0,1,1]]'), (2, 2, 12, 0, '표', 'table', 0, '[[0,0,1,1]]'), (3, 1, 3, 1, '강조', 'star', 2, NULL);
             INSERT INTO source_link (segment_id, card_id, method, created_at) VALUES (1, {fig}, 'manual', 0), (2, {fig}, 'manual', 0), (3, {text}, 'manual', 0);"
        ))
        .unwrap();
        let out = tmp.0.join("src.apkg");
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut zip.by_name("collection.anki2").unwrap(), &mut raw).unwrap();
        let col = tmp.0.join("src.anki2");
        std::fs::write(&col, raw).unwrap();
        let col = Connection::open(&col).unwrap();
        let tags = |front: &str| -> String { col.query_row("SELECT tags || '|' || flds FROM notes WHERE flds LIKE ?1", [format!("{front}%")], |r| r.get(0)).unwrap() };
        assert_eq!(
            tags("그림 카드"),
            " 과목::1 출처::4장_회귀분석::슬라이드_3 Neo-Anki::출처::3장::12쪽 Neo-Anki::출처::4장_회귀분석::슬라이드_3 |그림 카드\u{1f}b",
            "이 앱이 단 옛 출처 태그만 빼고 지금 연결로. 사용자가 단 같은 모양의 출처:: 태그는 그대로 (T2-02), 노트 글도 그대로"
        );
        assert_eq!(tags("글 카드"), "|글 카드\u{1f}b", "글 강조 구간은 출처 태그가 없다");
        assert_eq!(tags("따로"), " 중요 |따로\u{1f}b");

        let b = state(&tmp.0.join("b"));
        crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap();
        let bc = b.db.lock().unwrap();
        let back: String = bc.query_row("SELECT tags FROM note WHERE fields LIKE '%그림 카드%'", [], |r| r.get(0)).unwrap();
        assert_eq!(back, "과목::1 출처::4장_회귀분석::슬라이드_3", "다시 가져오면 이 앱이 단 출처 태그만 빠진다 (T2-02)");
    }

    /// 릴리스 QA: 그림 가리기 노트는 CSV 에 겹침 HTML(io-mask · canvas) 대신 질문 · 그림 파일 · 칸 이름을 쓴다
    #[test]
    fn csv_occlusion_is_plain_text() {
        let con = db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('해부학', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let ntid: i64 = con.query_row("SELECT id FROM note_type WHERE name = 'Image Occlusion'", [], |r| r.get(0)).unwrap();
        let occ = "{{c1::image-occlusion:rect:left=.1:top=.2:width=.25:height=.1:label=좌심실:oi=1}}<br>\
                   {{c2::image-occlusion:rect:left=.5:top=.5:width=.1:height=.1:oi=1}}<br>";
        let fields = serde_json::to_string(&[occ, "<img src=\"heart.png\">", "<b>심장</b>", "", ""]).unwrap();
        con.execute("INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, 1, 1)", params![ntid, fields]).unwrap();
        let nid = con.last_insert_rowid();
        for ord in [0, 1] {
            con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, ?3, 1, 1)", params![nid, root, ord])
                .unwrap();
        }
        for plain in [true, false] {
            let (text, ..) = csv_text(&con, root, &["front", "back"].map(String::from), plain).unwrap();
            assert_eq!(text.split("\r\n").nth(1), Some(r#""심장 (heart.png)","1 · 좌심실, 2""#), "plain_text {plain}");
        }
    }

    /// 릴리스 QA: .apkg 왕복으로 가져온 빈칸 유형도 앱 Cloze 처럼 빈칸 원문을 한 번 쓴다 (c1 카드 화면만 쓰면 c2 부터 빈칸이 빠진다)
    #[test]
    fn csv_imported_cloze_keeps_every_cloze() {
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-csv-cloze-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let con = a.db.lock().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('QA', 0, 0)", []).unwrap();
        let root = con.last_insert_rowid();
        let card = NewCard { deck_id: root, kind: "cloze".into(), front: "{{c1::서울}}은 {{c2::한국}}의 수도".into(), back: "덧붙임".into(), importance: None };
        insert_note(&con, &card, "", None).unwrap();
        let out = tmp.0.join("cloze.apkg");
        export(&con, &a.data_dir, root, &out, false, &QUIET).unwrap();
        let b = state(&tmp.0.join("b"));
        crate::import::run_import_anki(&|_, _, _| {}, &b, out.to_str().unwrap(), false, &mut vec![]).unwrap();
        let bc = b.db.lock().unwrap();
        let broot: i64 = bc.query_row("SELECT id FROM deck WHERE name = 'QA'", [], |r| r.get(0)).unwrap();
        let cols = ["front", "back"].map(String::from);
        let (text, ..) = csv_text(&bc, broot, &cols, false).unwrap();
        assert_eq!(text, csv_text(&con, root, &cols, false).unwrap().0);
        assert!(text.contains(r#""{{c1::서울}}은 {{c2::한국}}의 수도","덧붙임""#), "{text}");
    }

    /// 실제 Anki 가 만든 .apkg · .colpkg 를 복습 기록과 함께 가져와 요약을 찍고, 맨 위 덱을 기록과 함께 다시 내보낸다 (손으로 돌리는 QA).
    /// Anki 쪽 비교는 그 결과를 Anki 로 다시 읽어서 한다:
    ///   NEOANKI_RT_IN=<패키지> NEOANKI_RT_OUT=<내보낼 .apkg> cargo test real_anki_roundtrip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_anki_roundtrip() {
        let input = std::env::var("NEOANKI_RT_IN").expect("NEOANKI_RT_IN");
        let out = PathBuf::from(std::env::var("NEOANKI_RT_OUT").expect("NEOANKI_RT_OUT"));
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-rt-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0);
        let res = serde_json::to_value(crate::import::run_import_anki(&|_, _, _| {}, &a, &input, true, &mut vec![]).unwrap()).unwrap();
        println!("import: {res}");
        let con = a.db.lock().unwrap();
        let show = |label: &str, sql: &str| println!("{label}: {:?}", strings(&con, sql));
        let paths = "WITH RECURSIVE t(id, path) AS (SELECT id, name FROM deck WHERE parent_id IS NULL
                     UNION ALL SELECT d.id, t.path || '::' || d.name FROM deck d JOIN t ON d.parent_id = t.id)";
        show("decks", &format!("{paths} SELECT t.path || ' imp=' || d.importance || '/' || d.importance_source FROM t JOIN deck d ON d.id = t.id ORDER BY 1"));
        show("notes", "SELECT nt.name || ' | tags=' || n.tags || ' | exam=' || coalesce(n.exam_hits, '') || ' | cards=' || (SELECT count(*) FROM card c WHERE c.note_id = n.id)
                       FROM note n JOIN note_type nt ON nt.id = n.note_type_id ORDER BY 1");
        show("cards", &format!("{paths} SELECT nt.name || ' ord=' || c.template_ord || ' ' || c.state || ' ivl=' || c.interval_days || ' reps=' || c.reps || ' lapses=' || c.lapses || ' @' || t.path
                       FROM card c JOIN note n ON n.id = c.note_id JOIN note_type nt ON nt.id = n.note_type_id JOIN t ON t.id = c.deck_id ORDER BY 1"));
        show("reviews", "SELECT count(*) || ' (card 없음 ' || sum(card_id IS NULL) || ')' FROM review_log");
        show("media", "SELECT filename || ' ' || byte_size FROM media ORDER BY 1");
        let root: i64 = con.query_row("SELECT id FROM deck WHERE parent_id IS NULL AND name = 'QA'", [], |r| r.get(0)).unwrap();
        let r = export(&con, &a.data_dir, root, &out, true, &QUIET).unwrap();
        println!("export: {}", serde_json::to_value(r).unwrap());
    }

    // ---------------------------------------------------------------- DU55 Anki 왕복 기준

    /// "Anki 에서 고친" 패키지: collection.anki2 를 꺼내 SQL 을 돌리고 다시 묶는다 (미디어는 그대로)
    fn edit_apkg(src: &Path, dst: &Path, sql: &str) {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(File::open(src).unwrap()).unwrap();
        let col = dst.with_extension("anki2");
        let mut raw = vec![];
        zip.by_name("collection.anki2").unwrap().read_to_end(&mut raw).unwrap();
        std::fs::write(&col, raw).unwrap();
        Connection::open(&col).unwrap().execute_batch(sql).unwrap();
        let mut out = zip::ZipWriter::new(File::create(dst).unwrap());
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().unwrap().to_string();
            let mut b = vec![];
            e.read_to_end(&mut b).unwrap();
            out.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
            out.write_all(if name == "collection.anki2" { std::fs::read(&col).unwrap() } else { b }.as_slice()).unwrap();
        }
        out.finish().unwrap();
        let _ = std::fs::remove_file(col);
    }

    fn update(s: &AppState, p: &Path, dry: bool) -> crate::import::deck_update::UpdateSummary {
        use crate::import::deck_update::{self, Choices, Req};
        let choices = Choices::default();
        let req = Req { path: p, selected: None, choices: &choices, voices: &[], emit: &|_, _, _| {}, cancel: &|| false };
        let mut con = s.db.lock().unwrap();
        if dry { deck_update::run(&mut con, &s.data_dir, &req, true, &mut vec![]).unwrap() } else { deck_update::apply(&mut con, &s.data_dir, &req).unwrap() }
    }

    #[test]
    fn my_deck_round_trip_through_anki_uses_the_export_as_base() {
        // DU55 (A): 앱에서 만든 노트는 처음 내보낼 때의 값이 원본. Anki 에서만 고친 노트는 "새 버전", 내보낸 뒤 앱에서만 고친 노트는 "내 것",
        // 둘 다 고쳤으면 충돌(기본 내 것). 앱 기본 유형은 내보낸 모델 id 로 찾고 정의는 건드리지 않는다
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-rt55-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let out = tmp.0.join("내 덱.apkg");
        let (deck, basic, before_type, before_types) = {
            let con = a.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('내 덱', 0, 0)", []).unwrap();
            let deck = con.last_insert_rowid();
            for (q, b) in [("Anki 에서 고칠 것", "답1"), ("앱에서 고칠 것", "답2"), ("둘 다 고칠 것", "답3"), ("그대로", "답4")] {
                insert_note(&con, &NewCard { deck_id: deck, kind: "basic".into(), front: q.into(), back: b.into(), importance: None }, "", None).unwrap();
            }
            let basic = db::native_type_id(&con, "basic").unwrap();
            let dump = |sql: &str| strings(&con, sql).join("\n");
            let ty = dump(&format!("SELECT name || fields || css || coalesce(native_kind, '') || user_made || coalesce(anki_model_id, '') FROM note_type WHERE id = {basic} UNION ALL SELECT ord || name || qfmt || afmt FROM card_template WHERE note_type_id = {basic}"));
            (deck, basic, ty, dump("SELECT count(*) || '' FROM note_type"))
        };
        {
            let con = a.db.lock().unwrap();
            export(&con, &a.data_dir, deck, &out, false, &QUIET).unwrap();
            assert_eq!(strings(&con, "SELECT count(*) || '' FROM note WHERE origin IS NULL"), ["0"], "앱에서 만든 노트에 원본");
            let m: String = con.query_row("SELECT origin FROM note_type WHERE id = ?1", [basic], |r| r.get(0)).unwrap();
            assert!(m.starts_with(r#"{"m":["#), "앱 기본 유형은 내보낸 모델 id 만: {m}");
            // 다시 내보내도 원본은 그대로 (처음 것)
            let before: Vec<String> = strings(&con, "SELECT origin FROM note ORDER BY id");
            con.execute("UPDATE note SET fields = '[\"앱에서 고칠 것\",\"답2 (앱)\"]', updated_at = 5 WHERE fields LIKE '%앱에서 고칠 것%'", []).unwrap();
            con.execute("UPDATE note SET fields = '[\"둘 다 고칠 것\",\"답3 (앱)\"]', updated_at = 5 WHERE fields LIKE '%둘 다 고칠 것%'", []).unwrap();
            export(&con, &a.data_dir, deck, &tmp.0.join("두 번째.apkg"), false, &QUIET).unwrap();
            assert_eq!(strings(&con, "SELECT origin FROM note ORDER BY id"), before, "다시 내보내도 처음 원본");
        }
        // Anki 에서 고쳤다 (첫 번째 내보낸 사본)
        let back = tmp.0.join("Anki 에서.apkg");
        edit_apkg(
            &out,
            &back,
            "UPDATE notes SET flds = replace(flds, '답1', '답1 (Anki)') WHERE flds LIKE 'Anki 에서 고칠 것%';
             UPDATE notes SET flds = replace(flds, '답3', '답3 (Anki)') WHERE flds LIKE '둘 다 고칠 것%';",
        );
        let r = update(&a, &back, true);
        assert_eq!((r.changed, r.mine_kept, r.conflicts.len(), r.same, r.no_base, r.other_type, r.new_notes), (1, 1, 1, 1, 0, 0, 0));
        assert!(r.types.is_empty(), "앱 기본 유형은 바뀌는 것이 없다");
        update(&a, &back, false);
        let con = a.db.lock().unwrap();
        let backs = strings(&con, "SELECT json_extract(fields, '$[1]') FROM note ORDER BY id");
        assert_eq!(backs, ["답1 (Anki)", "답2 (앱)", "답3 (앱)", "답4"], "Anki 수정은 들어오고 앱 수정은 지킨다 (충돌은 내 것)");
        // 앱 기본 유형은 그대로: 정의 · 목록 행 · 숨긴 필드 · 새 유형 없음
        let dump = |sql: &str| strings(&con, sql).join("\n");
        assert_eq!(dump(&format!("SELECT name || fields || css || coalesce(native_kind, '') || user_made || coalesce(anki_model_id, '') FROM note_type WHERE id = {basic} UNION ALL SELECT ord || name || qfmt || afmt FROM card_template WHERE note_type_id = {basic}")), before_type);
        assert_eq!(dump("SELECT count(*) || '' FROM note_type"), before_types, "새 \"Basic\" 이 생기지 않는다");
        let row = serde_json::to_value(crate::notetype::list(&con).unwrap().into_iter().find(|t| serde_json::to_value(t).unwrap()["id"] == basic).unwrap()).unwrap();
        assert_eq!((row["native_kind"].as_str(), row["user_made"].as_bool()), (Some("basic"), Some(false)), "가져온 유형으로 보이지 않는다");
        assert!(serde_json::to_value(crate::notetype::detail(&con, basic).unwrap()).unwrap()["hidden"].as_array().unwrap().is_empty());
    }

    #[test]
    fn shared_deck_base_survives_my_export() {
        // 코디네이터 고정 테스트 (DU55): 공유 덱 v1 → 앱에서 고침 → 내 덱 내보내기 → 만든 사람의 v2 업데이트 = 내 수정을 지킨다.
        // 내보내기는 가져온 노트의 원본(만든 사람 기준)을 덮지 않는다
        use crate::import::deck_update::testpkg::{self, Model, note};
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-rt55s-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let model = || Model { id: 7, name: "Basic", fields: vec!["Front", "Back"], templates: vec![("Card 1", "{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}")], css: "" };
        let (p1, p2) = (tmp.0.join("v1.apkg"), tmp.0.join("v2.apkg"));
        testpkg::write(&p1, &[model()], &[(1, "공유덱")], &[note(1, "g1", 7, &["Q", "A"], &[(1, 0)]), note(2, "g2", 7, &["Q2", "A2"], &[(1, 0)])], &[]);
        crate::import::run_import_anki(&|_, _, _| {}, &a, p1.to_str().unwrap(), false, &mut vec![]).unwrap();
        let origins = |a: &AppState| strings(&a.db.lock().unwrap(), "SELECT * FROM (SELECT origin FROM note ORDER BY id) UNION ALL SELECT origin FROM note_type WHERE anki_model_id = 7");
        let before = origins(&a);
        {
            let con = a.db.lock().unwrap();
            let id: i64 = con.query_row("SELECT id FROM note WHERE anki_guid = 'g1'", [], |r| r.get(0)).unwrap();
            crate::commands::save_note_fields(&con, id, &["Q".into(), "A (내 수정)".into()]).unwrap();
            let deck: i64 = con.query_row("SELECT id FROM deck WHERE name = '공유덱'", [], |r| r.get(0)).unwrap();
            export(&con, &a.data_dir, deck, &tmp.0.join("내보냄.apkg"), true, &QUIET).unwrap();
        }
        assert_eq!(origins(&a), before, "가져온 노트 · 유형의 원본은 그대로");
        testpkg::write(&p2, &[model()], &[(1, "공유덱")], &[note(1, "g1", 7, &["Q", "A v2"], &[(1, 0)]), note(2, "g2", 7, &["Q2", "A2 v2"], &[(1, 0)])], &[]);
        let r = update(&a, &p2, false);
        assert_eq!((r.conflicts.len(), r.changed), (1, 1));
        let con = a.db.lock().unwrap();
        assert_eq!(strings(&con, "SELECT json_extract(fields, '$[1]') FROM note ORDER BY id"), ["A (내 수정)", "A2 v2"], "내 수정은 충돌로 지키고 (기본 내 것), 고치지 않은 노트만 새 버전");
    }

    #[test]
    fn reimport_from_anki_never_resets_study_state() {
        // 사용자 결정 2026-10-19 ②: 복습 기록 병합은 미룬다. 지금 길(업데이트 · 새 노트만 더하기)이 이미 있는 카드의 일정 · 상태를 덮거나 초기화하지 않고,
        // Anki 카드 ↔ 우리 카드가 노트 guid + 카드 순서로 이어지는지 고정한다 (내보내기는 카드 id 를 남기지 않는다: anki_card_id 는 신규 순서에 쓰인다)
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-rt55r-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let out = tmp.0.join("내 덱.apkg");
        let schedule = |a: &AppState| strings(&a.db.lock().unwrap(), "SELECT id || state || coalesce(due_at, '') || coalesce(stability, '') || reps || lapses FROM card ORDER BY id");
        let card = {
            let con = a.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('내 덱', 0, 0)", []).unwrap();
            let deck = con.last_insert_rowid();
            insert_note(&con, &NewCard { deck_id: deck, kind: "reversed".into(), front: "앞".into(), back: "뒤".into(), importance: None }, "", None).unwrap();
            let card: i64 = con.query_row("SELECT id FROM card WHERE template_ord = 1", [], |r| r.get(0)).unwrap();
            con.execute("UPDATE card SET state = 'review', due_at = ?2, stability = 9.5, difficulty = 5.0, reps = 4, interval_days = 6, last_reviewed_at = ?3 WHERE id = ?1", params![card, now_ms() + 6 * DAY_MS, now_ms() - DAY_MS]).unwrap();
            con.execute("INSERT INTO review_log (card_id, reviewed_at, rating, prev_state, fed_to_scheduler) VALUES (?1, ?2, 3, 'review', 1)", params![card, now_ms() - DAY_MS]).unwrap();
            export(&con, &a.data_dir, deck, &out, true, &QUIET).unwrap();
            assert_eq!(strings(&con, "SELECT count(*) || '' FROM card WHERE anki_card_id IS NOT NULL"), ["0"], "카드 id 를 쓰지 않는다 (신규 순서가 바뀌지 않게)");
            card
        };
        let before = schedule(&a);
        // Anki 에서 그 카드를 한 번 더 공부하고(기록 + 일정) 내용도 고쳤다
        let back = tmp.0.join("Anki 에서.apkg");
        edit_apkg(
            &out,
            &back,
            "INSERT INTO revlog (id, cid, usn, ease, ivl, lastIvl, factor, time, type) SELECT (SELECT max(id) FROM revlog) + 86400000, id, -1, 3, 15, 6, 2500, 5000, 1 FROM cards WHERE ord = 1;
             UPDATE cards SET ivl = 15, due = due + 9, reps = reps + 1 WHERE ord = 1;
             UPDATE notes SET flds = replace(flds, '뒤', '뒤 (Anki)');",
        );
        // 업데이트: 내용만, 일정 · 상태 · 기록 그대로 (DU12 · DU23)
        let r = update(&a, &back, false);
        assert_eq!((r.changed, r.reviews_skipped), (1, 2));
        assert_eq!(schedule(&a), before, "업데이트는 일정 · 상태를 쓰지 않는다");
        // 새 노트만 더하기 + 기록 옮기기 (I79): 기록은 guid + 순서로 같은 카드에 붙고, 이미 있는 카드의 일정은 덮지 않는다. 두 번 가져와도 한 번
        for _ in 0..2 {
            crate::import::run_import_anki(&|_, _, _| {}, &a, back.to_str().unwrap(), true, &mut vec![]).unwrap();
        }
        let con = a.db.lock().unwrap();
        assert_eq!(strings(&con, &format!("SELECT count(*) || '' FROM review_log WHERE card_id = {card}")), ["2"], "같은 카드에 Anki 기록 하나 더 (중복 없음)");
        assert_eq!(strings(&con, "SELECT count(*) || '' FROM review_log WHERE card_id IS NULL"), ["0"]);
        drop(con);
        assert_eq!(schedule(&a), before, "기록을 옮겨도 일정 · 상태는 그대로");
        assert_eq!(strings(&a.db.lock().unwrap(), "SELECT count(*) || '' FROM note"), ["1"], "같은 노트가 두 벌 되지 않는다");
    }

    #[test]
    fn my_note_type_round_trip_takes_the_template_changed_in_anki() {
        // DU55: 사용자 유형은 처음 내보낼 때의 정의가 원본. Anki 에서 고친 카드 종류 앞면은 "새 버전"으로 들어온다 (충돌이 아니다)
        let tmp = TempDir(std::env::temp_dir().join(format!("neoanki-rt55t-{}-{}", std::process::id(), now_ms())));
        let a = state(&tmp.0.join("a"));
        let out = tmp.0.join("내 유형.apkg");
        let (ntid, deck) = {
            let con = a.db.lock().unwrap();
            con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('내 덱', 0, 0)", []).unwrap();
            let deck = con.last_insert_rowid();
            let ntid = crate::notetype::anki_clone(&con, "basic", Some("내 유형"));
            crate::notetype::add_note(&con, &crate::notetype::TypedNote { deck_id: deck, note_type_id: ntid, values: vec!["Q".into(), "A".into()], importance: None }).unwrap();
            export(&con, &a.data_dir, deck, &out, false, &QUIET).unwrap();
            let o: Value = serde_json::from_str(&con.query_row("SELECT origin FROM note_type WHERE id = ?1", [ntid], |r| r.get::<_, String>(0)).unwrap()).unwrap();
            assert_eq!(o["f"], json!([["Front", 0], ["Back", 1]]), "내보낸 필드 (Anki 순서, 우리 자리)");
            (ntid, deck)
        };
        let back = tmp.0.join("Anki 에서.apkg");
        edit_apkg(&out, &back, r#"UPDATE col SET models = (SELECT json_set(models, '$."' || key || '".tmpls[0].qfmt', '<b>{{Front}}</b>') FROM json_each(col.models));"#);
        let r = update(&a, &back, false);
        assert_eq!((r.conflicts.len(), r.kept_looks, r.types.len()), (0, 0, 1));
        assert_eq!(serde_json::to_value(&r.types[0]).unwrap()["updated_looks"], json!(["Card 1"]));
        let con = a.db.lock().unwrap();
        assert_eq!(strings(&con, &format!("SELECT qfmt FROM card_template WHERE note_type_id = {ntid}")), ["<b>{{Front}}</b>"]);
        // 다시 내보내도 원본은 업데이트가 쓴 것 (내보내기는 이미 있는 원본을 덮지 않는다)
        let o = strings(&con, &format!("SELECT origin FROM note_type WHERE id = {ntid}"));
        export(&con, &a.data_dir, deck, &tmp.0.join("again.apkg"), false, &QUIET).unwrap();
        assert_eq!(strings(&con, &format!("SELECT origin FROM note_type WHERE id = {ntid}")), o);
    }
}
