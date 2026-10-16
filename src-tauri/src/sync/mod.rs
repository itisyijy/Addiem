//! Google Drive 보관 · 동기화 (MVP 8.6, docs/engineering/drive-sync.md). 화면 명령과 백그라운드 작업.
//!
//! 네트워크는 모두 여기(Rust)서 쓴다. 웹뷰 CSP 의 connect-src 는 연결한 뒤에도 열지 않는다.
//! 개발용: NEOANKI_SYNC_FOLDER=<폴더> 로 띄우면 Google 대신 그 폴더를 Drive 처럼 쓴다 (로그인 없음).
//!
//! 언제 (5.1): 앱 실행 받기 -> 올리기 · 학습을 마칠 때(화면이 sync_pause(false)) 받기 -> 올리기 ·
//! 창을 닫을 때 올리기(최대 10초, K8) · 30분마다 받기 · [지금 동기화]. 학습 중에는 하지 않는다

pub mod auth;
pub mod crypto;
pub mod engine;
pub mod store;
#[cfg(test)]
mod tests;

use crate::commands::AppState;
use engine::{Ctx, Join, Pulled};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Value as J, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use store::{DriveStore, FolderStore, SyncError, Store};
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 앱이 열려 있을 때 받기 주기
const PERIOD: Duration = Duration::from_secs(30 * 60);
/// 창을 닫을 때 기다리는 최대 시간 (K8)
const CLOSE_WAIT: Duration = Duration::from_secs(10);
const NO_CLIENT: &str = "이 빌드에는 Google 연결이 설정되지 않았어요";

/// 화면(K1–K10)이 보는 상태. "sync-status" 이벤트로도 온다
#[derive(Serialize, Clone, Default)]
pub struct Status {
    /// Google 클라이언트가 들어 있는 빌드이거나 개발용 폴더 (K1-n 이면 false)
    pub configured: bool,
    pub folder: bool,
    pub connected: bool,
    /// K3 [나중에]: 연결은 유지하고 동기화는 꺼 둠
    pub enabled: bool,
    pub account: Option<String>,
    /// off | paused | idle | syncing | offline | login | quota | update | choice | gone | locked | error
    pub state: String,
    /// pull | push | media | snapshot | backup | restore
    pub phase: Option<String>,
    pub done: u64,
    pub total: u64,
    pub last_sync: Option<i64>,
    /// 아직 올리지 않은 변경 묶음 (복습 기록은 세션 단위, K5)
    pub pending: i64,
    pub pending_reviews: i64,
    pub pending_notes: i64,
    /// Drive 에서 Neo-Anki 가 쓰는 공간 (바이트)
    pub usage: Option<u64>,
    /// Drive 전체 (쓴 양, 한도)
    pub quota: Option<(u64, u64)>,
    pub devices: Vec<Device>,
    pub device_name: String,
    /// daily | weekly | off
    pub backup: String,
    pub lock: bool,
    /// 다른 기기가 되돌리기 · 새로 시작을 했다 (K6-m). 그 스냅샷의 머리
    pub choice: Option<J>,
    /// 보지 않은 K7 기록 수와 가장 큰 번호 (토스트를 한 번만)
    pub conflicts: i64,
    pub conflict_seq: i64,
    pub error: Option<String>,
    pub studying: bool,
    /// 받기로 이 기기 데이터가 바뀔 때마다 커진다 (홈 · 덱 화면이 다시 읽는다)
    pub changed_seq: i64,
    /// K8 창을 닫는 중
    pub closing: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct Device {
    pub uid: String,
    pub name: String,
    pub at: Option<i64>,
    pub me: bool,
    /// 90일 동안 맞추지 않음 ("오래 안 씀")
    pub stale: bool,
    /// 마지막으로 올린 복습 기록 수와 시각
    pub reviews: Option<i64>,
    pub reviews_at: Option<i64>,
}

enum Job {
    Sync { pull: bool, push: bool },
}

pub struct SyncState(Arc<Shared>);

struct Shared {
    app: AppHandle,
    folder: Option<PathBuf>,
    /// 동기화 작업은 한 번에 하나
    run: Mutex<()>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    status: Mutex<Status>,
    paused: AtomicBool,
    /// 접근 토큰과 만료 시각 (ms). 새로 고칠 토큰은 키체인에
    token: Mutex<Option<(String, i64)>>,
    key: Mutex<Option<[u8; 32]>>,
    auth_cancel: Arc<AtomicBool>,
    closing: AtomicBool,
    fsrs: fsrs::FSRS,
    agent: ureq::Agent,
    /// 로그인 직후, 합류 전의 계정
    account: Mutex<Option<String>>,
}

impl Shared {
    fn state(&self) -> &AppState {
        self.app.state::<AppState>().inner()
    }

    fn db(&self) -> &Mutex<Connection> {
        &self.state().db
    }

    fn with_db<T>(&self, f: impl FnOnce(&Connection) -> Result<T, SyncError>) -> Result<T, SyncError> {
        let con = self.db().lock().map_err(|_| store::other("데이터베이스를 잠그지 못했어요"))?;
        engine::ensure_tables(&con)?;
        f(&con)
    }

    fn configured(&self) -> bool {
        self.folder.is_some() || auth::client().is_some()
    }

    fn access_token(&self, force: bool) -> Result<String, SyncError> {
        let mut cached = self.token.lock().map_err(store::other)?;
        if let Some((t, exp)) = cached.as_ref()
            && !force
            && *exp > crate::db::now_ms()
        {
            return Ok(t.clone());
        }
        let client = auth::client().ok_or_else(|| store::other(NO_CLIENT))?;
        let refresh = auth::load_secret(auth::REFRESH).ok_or(SyncError::Auth)?;
        let t = auth::refresh(&self.agent, client, &refresh)?;
        if let Some(r) = &t.refresh {
            let _ = auth::save_secret(auth::REFRESH, r);
        }
        *cached = Some((t.access.clone(), t.expires));
        Ok(t.access)
    }

    fn emit(&self) {
        if let Ok(s) = self.status.lock() {
            let _ = self.app.emit("sync-status", s.clone());
        }
    }

    fn set(&self, f: impl FnOnce(&mut Status)) {
        if let Ok(mut s) = self.status.lock() {
            f(&mut s);
        }
        self.emit();
    }
}

fn store_of(sh: &Arc<Shared>) -> Result<Box<dyn Store>, SyncError> {
    if let Some(f) = &sh.folder {
        return Ok(Box::new(FolderStore::new(f)));
    }
    auth::client().ok_or_else(|| store::other(NO_CLIENT))?;
    let s = sh.clone();
    Ok(Box::new(DriveStore::new(Box::new(move |force| s.access_token(force)))))
}

/// 잠근 Drive 의 키. 이 기기 키체인에서 한 번 읽어 둔다
fn key_of(sh: &Shared) -> Option<[u8; 32]> {
    use base64::Engine;
    let mut k = sh.key.lock().ok()?;
    if k.is_none() {
        let s = auth::load_secret(auth::KEY)?;
        let b = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
        *k = b.try_into().ok();
    }
    *k
}

fn with_ctx<T>(sh: &Arc<Shared>, f: impl FnOnce(&Ctx) -> Result<T, SyncError>) -> Result<T, SyncError> {
    let store = store_of(sh)?;
    let locked = sh.with_db(|c| Ok(engine::local(c, "lock")?.as_deref() == Some("1")))?;
    // 잠갔을 때만 키를 쓴다. 다른 기기가 잠금을 푼 Drive 를 옛 키로 읽거나 잠그면 서로 읽지 못한다
    let key = if locked { Some(key_of(sh).ok_or(SyncError::Locked)?) } else { None };
    let progress = |phase: &str, done: u64, total: u64| {
        let phase = phase.to_string();
        sh.set(|s| {
            s.phase = Some(phase);
            s.done = done;
            s.total = total;
        });
    };
    let cx = Ctx { store: &*store, key, media_dir: crate::commands::media_dir(&sh.state().data_dir), fsrs: &sh.fsrs, progress: &progress };
    f(&cx)
}

// ---------------------------------------------------------------- 시작 · 백그라운드

/// lib.rs setup 에서 AppState 다음에 부른다
pub fn setup(app: &AppHandle) {
    // 개발 빌드에서만. 배포 빌드에서 켜지면 연결 끊기가 실제 Google 토큰과 잠금 키를 지운다
    let folder = std::env::var_os("NEOANKI_SYNC_FOLDER").filter(|v| !v.is_empty() && cfg!(debug_assertions)).map(PathBuf::from);
    let (tx, rx) = mpsc::channel();
    let sh = Arc::new(Shared {
        app: app.clone(),
        folder,
        run: Mutex::new(()),
        jobs: Mutex::new(Some(tx)),
        status: Mutex::new(Status::default()),
        paused: AtomicBool::new(false),
        token: Mutex::new(None),
        key: Mutex::new(None),
        auth_cancel: Arc::new(AtomicBool::new(false)),
        closing: AtomicBool::new(false),
        fsrs: fsrs::FSRS::default(),
        agent: store::agent(),
        account: Mutex::new(None),
    });
    app.manage(SyncState(sh.clone()));
    refresh(&sh);
    std::thread::spawn(move || worker(sh, rx));
}

fn worker(sh: Arc<Shared>, rx: mpsc::Receiver<Job>) {
    // 앱 실행: 받기 -> 올리기
    run(&sh, true, true);
    let mut failures = 0u32;
    loop {
        let offline = sh.status.lock().map(|s| s.state == "offline").unwrap_or(false);
        // 오프라인이면 30초부터 두 배씩 (최대 30분) 다시 해 본다. 연결되면 저절로 올린다 (8장)
        let wait = if offline { (Duration::from_secs(30) * 2u32.pow(failures.min(6))).min(PERIOD) } else { PERIOD };
        failures = if offline { failures + 1 } else { 0 };
        match rx.recv_timeout(wait) {
            Ok(Job::Sync { pull, push }) => run(&sh, pull, push),
            Err(mpsc::RecvTimeoutError::Timeout) if !sh.paused.load(Ordering::Relaxed) => run(&sh, true, offline),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn enqueue(sh: &Shared, pull: bool, push: bool) {
    if let Ok(Some(tx)) = sh.jobs.lock().as_deref() {
        let _ = tx.send(Job::Sync { pull, push });
    }
}

enum Outcome {
    Skip,
    Done { quota: bool },
    Choice(J),
    Gone,
}

fn connected(con: &Connection) -> Result<(bool, bool), SyncError> {
    Ok((engine::local(con, "account")?.is_some(), engine::local(con, "enabled")?.as_deref() != Some("0") && engine::gen_of(con)? > 0))
}

/// 받기 · 올리기 한 번. 학습 중이면 하지 않는다 (창을 닫을 때의 올리기는 예외)
fn run(sh: &Arc<Shared>, pull: bool, push: bool) {
    let Ok(_g) = sh.run.lock() else { return };
    if sh.paused.load(Ordering::Relaxed) && !sh.closing.load(Ordering::Relaxed) {
        return;
    }
    let outcome = run_once(sh, pull, push);
    let now = crate::db::now_ms();
    match outcome {
        Ok(Outcome::Skip) => {}
        Ok(Outcome::Done { quota }) => {
            let _ = sh.with_db(|c| engine::set_local(c, "last_sync", &now.to_string()));
            sh.set(|s| {
                s.state = if quota { "quota" } else { "idle" }.into();
                s.error = None;
                s.choice = None;
            });
        }
        Ok(Outcome::Choice(h)) => sh.set(|s| {
            s.state = "choice".into();
            s.choice = Some(h);
        }),
        Ok(Outcome::Gone) => sh.set(|s| s.state = "gone".into()),
        Err(e) => {
            let state = match &e {
                SyncError::Offline(_) => "offline",
                SyncError::Auth => "login",
                SyncError::Quota => "quota",
                SyncError::NewerSchema => "update",
                SyncError::Locked => "locked",
                _ => "error",
            };
            sh.set(|s| {
                s.state = state.into();
                s.error = Some(e.to_string());
            });
        }
    }
    sh.set(|s| s.phase = None);
    refresh(sh);
}

fn run_once(sh: &Arc<Shared>, pull: bool, push: bool) -> Result<Outcome, SyncError> {
    if sh.state().startup_error.lock().map(|e| e.is_some()).unwrap_or(true) {
        return Ok(Outcome::Skip); // 학습 데이터를 열지 못했다 (S1-6): 빈 DB 를 올리면 안 된다
    }
    let (conn, enabled) = sh.with_db(connected)?;
    if !conn || !enabled {
        return Ok(Outcome::Skip);
    }
    sh.set(|s| s.state = "syncing".into());
    with_ctx(sh, |cx| {
        let db = sh.db();
        if pull {
            match engine::pull(db, cx)? {
                // 받은 것이 있으면 화면이 지금 보는 데이터를 다시 읽는다
                Pulled::Done(r) if r.applied > 0 => sh.set(|s| s.changed_seq += 1),
                Pulled::Done(_) => {}
                Pulled::Choice(c) => return Ok(Outcome::Choice(json!({ "gen": c.generation, "header": c.header, "locked": cx.store.list("crypto.json")?.len() == 1 }))),
                Pulled::Gone => return Ok(Outcome::Gone),
            }
            // 이 기능 전에 잠근 기기: 다시 시작할 때 쓸 crypto.json 을 한 번 받아 둔다 (T3-11)
            if cx.key.is_some()
                && sh.with_db(|c| engine::local(c, "crypto"))?.is_none()
                && let Ok(raw) = cx.store.get("crypto.json")
            {
                keep_crypto(sh, &raw)?;
            }
        }
        let mut quota = false;
        if push {
            engine::push(db, cx)?;
            // 공간이 모자라면 백업부터 멈춘다. 변경은 올라갔다 (8장)
            match backup(sh, cx) {
                Err(SyncError::Quota) => quota = true,
                Err(SyncError::Offline(e)) => return Err(SyncError::Offline(e)),
                _ => {}
            }
        }
        if pull {
            let _ = engine::compact(db, cx);
        }
        let backups: J = sh.with_db(|c| Ok(engine::local(c, "my_backups")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(json!([]))))?;
        engine::write_device(db, cx, &backups)?;
        if let Ok((account, q)) = cx.store.about() {
            let usage = engine::remote(cx.store).ok().map(|r| r.bytes);
            sh.set(|s| {
                s.quota = q;
                s.usage = usage;
                if s.account.is_none() {
                    s.account = Some(account);
                }
            });
            if q.is_some_and(|(used, limit)| used + 50 * 1024 * 1024 > limit) {
                quota = true;
            }
        }
        Ok(Outcome::Done { quota })
    })
}

/// 그날 첫 올리기라면 Drive 백업 zip 을 만들고 보관 규칙을 적용한다 (5.2 ④). 여러 기기가 같은 날 올려도 하루 하나
fn backup(sh: &Arc<Shared>, cx: &Ctx) -> Result<(), SyncError> {
    let setting = sh.with_db(|c| engine::local(c, "backup"))?.unwrap_or_else(|| "daily".into());
    if setting == "off" {
        return Ok(());
    }
    let list = cx.store.list("backups/")?;
    let now = crate::db::now_ms();
    let times: Vec<(String, i64)> =
        list.iter().filter_map(|e| e.path.strip_prefix("backups/").map(|n| (n.to_string(), engine::backup_time(n).unwrap_or(e.at)))).collect();
    let today = chrono::Local::now().format("neoanki-%Y-%m-%d").to_string();
    let newest = times.iter().map(|(_, t)| *t).max().unwrap_or(0);
    let due = match setting.as_str() {
        "weekly" => now - newest >= 7 * crate::db::DAY_MS,
        _ => !times.iter().any(|(n, _)| n.starts_with(&today)),
    };
    if !due {
        return Ok(());
    }
    if let Ok((_, Some((used, limit)))) = cx.store.about()
        && used + 50 * 1024 * 1024 > limit
    {
        return Err(SyncError::Quota);
    }
    (cx.progress)("backup", 0, 1);
    let tmp = sh.state().data_dir.join("backups");
    let (zip, counts) = sh.with_db(|c| Ok((engine::backup_zip(c, &tmp)?, engine::counts(c)?)))?;
    let name = chrono::Local::now().format("neoanki-%Y-%m-%d-%H%M.zip").to_string();
    let size = zip.len();
    cx.store.put(&format!("backups/{name}"), &engine::seal_opt(zip, cx.key.as_ref()))?;
    let mut all = times;
    all.push((name.clone(), now));
    for old in engine::prune_backups(&all, now) {
        cx.store.delete(&format!("backups/{old}"))?;
    }
    // 이 기기가 만든 백업 (K4 "노트북 · 6.1MB", K9 "카드 4,612장 · 복습 기록 17,918개")
    sh.with_db(|c| {
        let mut mine: Vec<J> = engine::local(c, "my_backups")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
        mine.push(json!({ "name": name, "at": now, "size": size, "cards": counts.cards, "reviews": counts.reviews }));
        let keep = mine.len().saturating_sub(20);
        engine::set_local(c, "my_backups", &serde_json::to_string(&mine[keep..]).map_err(store::other)?)
    })
}

/// DB 에서 다시 읽어 상태를 맞춘다 (설정 · 대기 묶음 · 기기 목록 · K7)
fn refresh(sh: &Arc<Shared>) {
    let read = sh.with_db(|c| {
        let (conn, enabled) = connected(c)?;
        let me = engine::device_uid(c)?;
        let my_name = engine::meta(c, "device_name")?.unwrap_or_else(|| "이 기기".into());
        let last_sync = engine::local(c, "last_sync")?.and_then(|v| v.parse().ok());
        let last_push: J = engine::local(c, "last_push")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(J::Null);
        let cache: Vec<J> = engine::local(c, "devices")?.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
        let now = crate::db::now_ms();
        let mut devices = vec![Device {
            uid: me.clone(),
            name: my_name.clone(),
            at: last_sync,
            me: true,
            stale: false,
            reviews: last_push["reviews"].as_i64(),
            reviews_at: last_push["at"].as_i64(),
        }];
        for d in cache.iter().filter(|d| d["uid"].as_str().is_some_and(|u| u != me)) {
            let at = d["at"].as_i64();
            devices.push(Device {
                uid: d["uid"].as_str().unwrap_or_default().into(),
                name: d["name"].as_str().unwrap_or("다른 기기").into(),
                at,
                me: false,
                stale: at.is_some_and(|a| now - a > engine::STALE_MS),
                reviews: d["last_push"]["reviews"].as_i64(),
                reviews_at: d["last_push"]["at"].as_i64(),
            });
        }
        let since = now - 30 * crate::db::DAY_MS;
        let conflicts = c.query_row("SELECT count(*), coalesce(max(id), 0) FROM sync_conflict WHERE seen = 0 AND at > ?1", [since], |r| Ok((r.get(0)?, r.get(1)?))).unwrap_or((0, 0));
        Ok((
            conn,
            enabled,
            engine::local(c, "account")?,
            engine::local(c, "backup")?.unwrap_or_else(|| "daily".into()),
            engine::local(c, "lock")?.as_deref() == Some("1"),
            last_sync,
            pending_counts(c),
            devices,
            my_name,
            conflicts,
        ))
    });
    let configured = sh.configured();
    let folder = sh.folder.is_some();
    let studying = sh.paused.load(Ordering::Relaxed);
    let closing = sh.closing.load(Ordering::Relaxed);
    sh.set(|s| {
        s.configured = configured;
        s.folder = folder;
        s.studying = studying;
        s.closing = closing;
        if let Ok((conn, enabled, account, backup, lock, last_sync, (pending, reviews, notes), devices, name, (cn, cmax))) = read {
            s.connected = conn;
            s.enabled = enabled;
            s.account = account;
            s.backup = backup;
            s.lock = lock;
            s.last_sync = last_sync;
            s.pending = pending;
            s.pending_reviews = reviews;
            s.pending_notes = notes;
            s.devices = devices;
            s.device_name = name;
            s.conflicts = cn;
            // 새 K7 기록이 생기면 번호가 커진다. 화면은 이 번호가 커질 때 토스트를 한 번 띄운다
            s.conflict_seq = cmax;
            if !conn {
                s.state = "off".into();
            } else if !enabled && s.state != "choice" && s.state != "gone" {
                s.state = "paused".into();
            } else if s.state.is_empty() || s.state == "off" || s.state == "paused" {
                s.state = "idle".into();
            }
        }
    });
}

/// 아직 올리지 않은 변경 (전송 대기 묶음, 복습 기록, 카드 변경). 복습은 세션 하나를 한 묶음으로 센다 (K5).
/// 카드 표만 바뀐 변경(중요도 · 접기 · 덱 이동)도 카드 변경이다. 복습으로 바뀐 카드는 그 복습 묶음에 든다 (T3-13)
fn pending_counts(c: &Connection) -> (i64, i64, i64) {
    let n = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let reviews = n("SELECT count(*) FROM sync_outbox WHERE table_name = 'review_log' AND op = 'insert'");
    let sessions = n("SELECT count(DISTINCT coalesce(r.session_id, 0)) FROM sync_outbox o JOIN review_log r ON r.uid = o.uid
                      WHERE o.table_name = 'review_log' AND o.op = 'insert'");
    let notes = n("SELECT count(DISTINCT uid) FROM sync_outbox WHERE table_name = 'note'");
    let cards = n("SELECT count(DISTINCT o.uid) FROM sync_outbox o WHERE o.table_name = 'card' AND NOT EXISTS (
                     SELECT 1 FROM sync_outbox x JOIN review_log l ON l.uid = x.uid JOIN card c ON c.id = l.card_id
                     WHERE x.table_name = 'review_log' AND x.op = 'insert' AND c.uid = o.uid)");
    let others = n("SELECT count(*) FROM (SELECT DISTINCT table_name, uid FROM sync_outbox WHERE table_name NOT IN ('review_log', 'card', 'note', 'study_session', 'daily_plan'))");
    (sessions + notes + cards + others, reviews, notes + cards)
}

// ---------------------------------------------------------------- 창 닫기 (K8)

fn pending_now(sh: &Shared) -> i64 {
    sh.with_db(|c| Ok(c.query_row("SELECT count(*) FROM sync_outbox", [], |r| r.get::<_, i64>(0)).unwrap_or(0))).unwrap_or(0)
}

/// 올리지 않은 변경이 있으면 바로 닫지 않고 K8 창을 띄운 뒤 최대 10초 올린다.
/// 오프라인 · 로그인 만료면 창 없이 닫는다. 변경은 이 기기 DB 에 있으므로 잃는 것은 없다
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    let tauri::WindowEvent::CloseRequested { api, .. } = event else { return };
    let Some(state) = window.try_state::<SyncState>() else { return };
    let sh = state.0.clone();
    if sh.closing.load(Ordering::Relaxed) {
        return; // 닫는 중에 한 번 더 누르면 그대로 닫는다
    }
    let blocked = sh.status.lock().map(|s| !s.connected || !s.enabled || ["offline", "login", "quota", "update", "choice", "gone", "locked"].contains(&s.state.as_str())).unwrap_or(true);
    if blocked || pending_now(&sh) == 0 {
        return;
    }
    api.prevent_close();
    sh.closing.store(true, Ordering::Relaxed);
    refresh(&sh);
    let w = window.clone();
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel();
        let s2 = sh.clone();
        std::thread::spawn(move || {
            run(&s2, false, true);
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(CLOSE_WAIT);
        let _ = w.destroy();
    });
}

// ---------------------------------------------------------------- 명령

#[tauri::command]
pub fn sync_status(state: State<SyncState>) -> Status {
    refresh(&state.0);
    state.0.status.lock().map(|s| s.clone()).unwrap_or_default()
}

/// 학습 화면에 들어가면 true, 나오면 false. 나올 때(학습을 마칠 때) 받기 -> 올리기
#[tauri::command]
pub fn sync_pause(state: State<SyncState>, paused: bool) {
    let was = state.0.paused.swap(paused, Ordering::Relaxed);
    if was && !paused {
        enqueue(&state.0, true, true);
    }
    refresh(&state.0);
}

/// [지금 동기화]. 학습 중에는 누를 수 없다
#[tauri::command]
pub fn sync_now(state: State<SyncState>) -> Res<()> {
    if state.0.paused.load(Ordering::Relaxed) {
        return Err("학습 중에는 동기화하지 않아요. 학습을 마치면 올려요.".into());
    }
    enqueue(&state.0, true, true);
    Ok(())
}

/// K8 [기다리지 않고 닫기]
#[tauri::command]
pub fn sync_close_now(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.destroy();
    }
}

/// 로그인 결과 ("sync-auth" 이벤트)
#[derive(Serialize, Clone)]
struct AuthEvent {
    ok: bool,
    /// timeout | cancelled | denied | scopes | error
    reason: Option<String>,
    message: Option<String>,
    probe: Option<Probe>,
}

#[derive(Serialize, Clone)]
pub struct Probe {
    pub account: String,
    /// Drive 에 Neo-Anki 데이터가 있으면 (K6-f · K6-m)
    pub remote: Option<J>,
    pub local: J,
    pub device_name: String,
}

/// K1 [Google Drive 연결] / K6 [Google Drive에서 불러오기]. 브라우저를 열고 로그인 주소를 돌려준다 ([링크 복사]).
/// 결과는 "sync-auth" 이벤트로. 개발용 폴더면 로그인 없이 바로 끝난다
#[tauri::command]
pub fn sync_connect(state: State<SyncState>) -> Res<Option<String>> {
    let sh = state.0.clone();
    if sh.folder.is_some() {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let ev = match probe(&sh) {
                Ok(p) => AuthEvent { ok: true, reason: None, message: None, probe: Some(p) },
                Err(e) => AuthEvent { ok: false, reason: Some("error".into()), message: Some(e.to_string()), probe: None },
            };
            let _ = sh.app.emit("sync-auth", ev);
        });
        return Ok(None);
    }
    let client = auth::client().ok_or(NO_CLIENT)?;
    let listener = auth::loopback().map_err(err)?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr().map_err(err)?.port());
    let p = auth::pkce();
    let url = auth::auth_url(client.0, &redirect, &p);
    sh.auth_cancel.store(false, Ordering::Relaxed);
    open_url(&url);
    std::thread::spawn(move || {
        let fail = |reason: &str, message: String| AuthEvent { ok: false, reason: Some(reason.into()), message: Some(message), probe: None };
        let ev = match auth::wait_code(listener, &p.state, &sh.auth_cancel) {
            Err(auth::WaitError::Timeout) => fail("timeout", "시간이 지났어요".into()),
            Err(auth::WaitError::Cancelled) => fail("cancelled", String::new()),
            Err(auth::WaitError::Denied) => fail("denied", "로그인을 취소했어요".into()),
            Err(auth::WaitError::Other(e)) => fail("error", e),
            Ok(code) => match auth::exchange(&sh.agent, client, &code, &p.verifier, &redirect) {
                Err(e) => fail("error", e.to_string()),
                // 받은 토큰은 쓰지 않고 버린다. 폐기(revoke)하면 이 계정의 다른 기기 로그인까지 끊긴다 (others_active)
                Ok(t) if !auth::granted_all(&t.scope) => fail("scopes", "권한 2개가 모두 필요해요".into()),
                Ok(t) => {
                    if let Some(r) = &t.refresh {
                        let _ = auth::save_secret(auth::REFRESH, r);
                    }
                    if let Ok(mut c) = sh.token.lock() {
                        *c = Some((t.access.clone(), t.expires));
                    }
                    match probe(&sh) {
                        Ok(p) => AuthEvent { ok: true, reason: None, message: None, probe: Some(p) },
                        Err(e) => fail("error", e.to_string()),
                    }
                }
            },
        };
        let _ = sh.app.emit("sync-auth", ev);
    });
    Ok(Some(url))
}

/// K2 [취소] · Esc: 루프백 서버를 닫는다
#[tauri::command]
pub fn sync_connect_cancel(state: State<SyncState>) {
    state.0.auth_cancel.store(true, Ordering::Relaxed);
}

/// Drive 와 이 기기에 무엇이 있는지 (K3 · K6-f · K6-m 을 고른다)
fn probe(sh: &Arc<Shared>) -> Result<Probe, SyncError> {
    let store = store_of(sh)?;
    let (account, _) = store.about()?;
    if let Ok(mut a) = sh.account.lock() {
        *a = Some(account.clone());
    }
    let r = engine::remote(&*store)?;
    let remote = if r.is_empty() {
        None
    } else {
        let devices = engine::read_devices(&*store, &r);
        let me = sh.with_db(engine::device_uid)?;
        // K6-m [Drive 새로 시작] 이 말하는 다른 기기들. 연결을 끊었다 다시 연결하면 기기 목록 기억이 없다 (T3-18)
        let names: Vec<&str> = devices
            .iter()
            .filter(|d| d["uid"].as_str() != Some(me.as_str()) && crate::db::now_ms() - d["at"].as_i64().unwrap_or(0) < engine::STALE_MS)
            .filter_map(|d| d["name"].as_str())
            .collect();
        let last = devices.iter().filter(|d| d["gen"].as_i64() == Some(r.generation())).max_by_key(|d| d["at"].as_i64().unwrap_or(0));
        let counts = match last {
            Some(d) => d["counts"].clone(),
            None => match r.newest(r.generation()) {
                Some(s) => engine::decode(store.get(&s.path)?, None).map(|(h, _)| h["counts"].clone()).unwrap_or(J::Null),
                None => J::Null,
            },
        };
        Some(json!({
            "gen": r.generation(), "locked": r.locked, "devices": devices.len(), "names": names, "bytes": r.bytes,
            "device": last.and_then(|d| d["name"].as_str()), "at": last.and_then(|d| d["at"].as_i64()), "counts": counts,
        }))
    };
    let (local, name) = sh.with_db(|c| {
        let k = engine::counts(c)?;
        let empty = k.cards == 0 && k.reviews == 0 && k.decks == 0;
        Ok((
            json!({ "decks": k.decks, "cards": k.cards, "reviews": k.reviews, "media_bytes": k.media_bytes, "empty": empty }),
            engine::meta(c, "device_name")?.unwrap_or_default(),
        ))
    })?;
    Ok(Probe { account, remote, local, device_name: name })
}

#[tauri::command]
pub async fn sync_probe(state: State<'_, SyncState>) -> Res<Probe> {
    let sh = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || probe(&sh).map_err(err)).await.map_err(err)?
}

#[derive(Deserialize)]
pub struct JoinOpts {
    pub mode: Join,
    pub device_name: Option<String>,
    /// daily | weekly | off
    pub backup: Option<String>,
    /// 암호로 잠그기 (새로 시작) 또는 잠긴 Drive 열기
    pub password: Option<String>,
    /// K3 [나중에]: 연결만 하고 동기화는 꺼 둔다
    #[serde(default)]
    pub later: bool,
}

fn save_key(sh: &Shared, key: [u8; 32]) -> Result<(), SyncError> {
    use base64::Engine;
    auth::save_secret(auth::KEY, &base64::engine::general_purpose::STANDARD.encode(key)).map_err(store::other)?;
    if let Ok(mut k) = sh.key.lock() {
        *k = Some(key);
    }
    Ok(())
}

/// 잠긴 Drive 의 비밀번호를 확인하고 키를 이 기기 키체인에 둔다
fn unlock(sh: &Arc<Shared>, store: &dyn Store, password: &str) -> Result<(), SyncError> {
    let raw = store.get("crypto.json")?;
    let kdf: crypto::KdfParams = serde_json::from_slice(&raw).map_err(store::other)?;
    let key = crypto::open_key(password, &kdf).map_err(store::other)?.ok_or_else(|| store::other("암호가 맞지 않아요"))?;
    save_key(sh, key)?;
    keep_crypto(sh, &raw)
}

/// crypto.json 을 이 기기에도 둔다: Drive 가 지워진 뒤 이 기기 데이터로 다시 시작할 때 같은 잠금(같은 salt · 키)으로 다시 올린다 (T3-11)
fn keep_crypto(sh: &Shared, raw: &[u8]) -> Result<(), SyncError> {
    sh.with_db(|c| engine::set_local(c, "crypto", &String::from_utf8_lossy(raw)))
}

/// 새로 시작(New · Fresh)할 때 Drive 잠금 (T3-11)
#[derive(Debug, PartialEq)]
enum LockPlan {
    /// 잠그지 않는다: Drive 의 crypto.json 과 이 기기 잠금 키를 지운다
    Off,
    /// 새 비밀번호로 새로 잠근다 (새 salt)
    New(String),
    /// 지금 잠금 그대로 (같은 salt · 키): 비밀번호가 있으면 그것으로, 없으면 이 기기 키체인 키로 연다
    Keep { crypto: Vec<u8>, password: Option<String> },
}

/// kind: fresh(사용자가 고른 시작 · 새로 시작) · lock · unlock (K4 잠그기 바꾸기).
/// 잠근 기기나 잠근 Drive 를 새로 시작해도 잠금을 말없이 풀지 않는다: 이 기기 키로 잠근 채 다시 올리고,
/// 그럴 수 없으면(키나 crypto.json 이 없다, 다른 기기가 잠갔다) 비밀번호를 묻는다 (Locked)
fn lock_plan(kind: &str, password: Option<&str>, local_lock: bool, has_key: bool, remote: Option<Vec<u8>>, cached: Option<Vec<u8>>) -> Result<LockPlan, SyncError> {
    let password = password.filter(|p| !p.is_empty()).map(String::from);
    Ok(match (kind, password) {
        ("unlock", _) => LockPlan::Off,
        ("lock", Some(p)) => LockPlan::New(p),
        // 비밀번호를 넣었다: 잠근 Drive 면 그 잠금을 연다. 이 기기에 남긴 잠금이 있으면 그것으로 (맞지 않으면 새로 잠근다)
        (_, Some(p)) => match remote.or(cached) {
            Some(crypto) => LockPlan::Keep { crypto, password: Some(p) },
            None => LockPlan::New(p),
        },
        (_, None) if local_lock && has_key => LockPlan::Keep { crypto: remote.or(cached).ok_or(SyncError::Locked)?, password: None },
        (_, None) if local_lock || remote.is_some() => return Err(SyncError::Locked),
        (_, None) => LockPlan::Off,
    })
}

fn set_lock(sh: &Arc<Shared>, store: &dyn Store, plan: LockPlan, remote_locked: bool) -> Result<(), SyncError> {
    match plan {
        LockPlan::Off => {
            if remote_locked {
                store.delete("crypto.json")?;
                // 잠근 동안의 그림 이름(HMAC)은 평문 Drive 에서 쓸 수 없다. 스냅샷이 다시 올린다
                for e in store.list("media/")? {
                    store.delete(&e.path)?;
                }
            }
            auth::delete_secret(auth::KEY);
            if let Ok(mut k) = sh.key.lock() {
                *k = None;
            }
            sh.with_db(|c| engine::set_local(c, "lock", "0"))
        }
        LockPlan::New(pw) => {
            let (key, kdf) = crypto::new_key(&pw).map_err(store::other)?;
            for e in store.list("media/")? {
                store.delete(&e.path)?;
            }
            let raw = serde_json::to_string(&kdf).map_err(store::other)?;
            store.put("crypto.json", raw.as_bytes())?;
            save_key(sh, key)?;
            keep_crypto(sh, raw.as_bytes())?;
            sh.with_db(|c| engine::set_local(c, "lock", "1"))
        }
        LockPlan::Keep { crypto: raw, password } => {
            let kdf: crypto::KdfParams = serde_json::from_slice(&raw).map_err(store::other)?;
            let key = match password {
                Some(p) => match crypto::open_key(&p, &kdf).map_err(store::other)? {
                    Some(k) => k,
                    // 이 기기에 남긴 잠금과 다른 비밀번호: 그 비밀번호로 새로 잠근다 (Drive 는 비어 있다)
                    None if !remote_locked => return set_lock(sh, store, LockPlan::New(p), remote_locked),
                    None => return Err(store::other("암호가 맞지 않아요")),
                },
                None => key_of(sh).filter(|k| crypto::key_fits(k, &kdf)).ok_or(SyncError::Locked)?,
            };
            if !remote_locked {
                store.put("crypto.json", &raw)?;
            }
            save_key(sh, key)?;
            keep_crypto(sh, &raw)?;
            sh.with_db(|c| engine::set_local(c, "lock", "1"))
        }
    }
}

/// 바꾸거나 합치기 전의 이 기기 백업 (K6-m "어느 쪽이든 바꾸기 전에 이 기기 백업을 먼저 만들어요")
fn local_backup(sh: &Shared, suffix: &str) -> Result<Option<PathBuf>, SyncError> {
    let data_dir = sh.state().data_dir.clone();
    sh.with_db(|c| {
        let k = engine::counts(c)?;
        if k.cards == 0 && k.reviews == 0 {
            return Ok(None);
        }
        let path = crate::commands::write_backup(c, &data_dir).map_err(store::other)?;
        let to = path.with_file_name(format!("neoanki-{suffix}-{}.zip", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        std::fs::rename(&path, &to).map_err(store::other)?;
        Ok(Some(to))
    })
}

fn join_blocking(sh: &Arc<Shared>, o: JoinOpts) -> Result<(), SyncError> {
    let _g = sh.run.lock().map_err(store::other)?;
    join_locked(sh, o, "fresh")
}

/// run 잠금을 쥔 채로 부른다. fresh_kind: 새로 시작할 때 다른 기기 고르는 창에 보일 이유 (fresh · lock · unlock)
fn join_locked(sh: &Arc<Shared>, o: JoinOpts, fresh_kind: &str) -> Result<(), SyncError> {
    let store = store_of(sh)?;
    let account = match sh.account.lock().ok().and_then(|a| a.clone()) {
        Some(a) => a,
        None => store.about()?.0,
    };
    sh.with_db(|c| {
        if let Some(n) = o.device_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            c.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('device_name', ?1)", [n]).map_err(store::other)?;
        }
        engine::set_local(c, "account", &account)?;
        // 고르는 창 · 다시 시작은 백업 설정을 보내지 않는다: 사용자가 끈 백업을 "매일"로 되돌리지 않는다. 처음 연결이면 매일
        match o.backup.as_deref() {
            Some(b) => engine::set_local(c, "backup", b)?,
            None if engine::local(c, "backup")?.is_none() => engine::set_local(c, "backup", "daily")?,
            None => {}
        }
        engine::set_local(c, "enabled", if o.later { "0" } else { "1" })
    })?;
    if o.later {
        return Ok(());
    }
    let remote_locked = store.list("crypto.json")?.len() == 1;
    match (o.mode, o.password.as_deref()) {
        // 새로 시작: 잠근 기기 · 잠근 Drive 의 잠금은 말없이 풀지 않는다 (T3-11)
        (Join::New | Join::Fresh, password) => {
            let remote = if remote_locked { Some(store.get("crypto.json")?) } else { None };
            let (local_lock, cached) = sh.with_db(|c| Ok((engine::local(c, "lock")?.as_deref() == Some("1"), engine::local(c, "crypto")?)))?;
            let plan = lock_plan(fresh_kind, password, local_lock, key_of(sh).is_some(), remote, cached.map(String::into_bytes))?;
            set_lock(sh, &*store, plan, remote_locked)?;
        }
        (_, pw) if remote_locked => {
            if let Some(pw) = pw {
                unlock(sh, &*store, pw)?;
            } else if key_of(sh).is_none() {
                return Err(SyncError::Locked);
            }
            sh.with_db(|c| engine::set_local(c, "lock", "1"))?;
        }
        // 잠기지 않은 Drive 에 합류한다 (다른 기기가 잠금을 풀었을 수도 있다): 남은 잠금 키는 지운다
        _ => {
            auth::delete_secret(auth::KEY);
            if let Ok(mut k) = sh.key.lock() {
                *k = None;
            }
            sh.with_db(|c| engine::set_local(c, "lock", "0"))?;
        }
    }
    if o.mode != Join::New {
        local_backup(sh, "before-sync")?;
    }
    let reason = match o.mode {
        Join::Fresh => json!({ "kind": fresh_kind, "device_name": sh.with_db(|c| engine::meta(c, "device_name"))? }),
        _ => json!({ "kind": "new" }),
    };
    drop(store);
    with_ctx(sh, |cx| engine::join(sh.db(), cx, o.mode, reason).map(|_| ()))?;
    if let Ok(mut s) = sh.status.lock() {
        s.choice = None;
        s.state = "idle".into();
        // 이 기기 데이터가 바뀌었다 (Drive 데이터로 바꾸기 · 합치기): 화면이 다시 읽는다
        s.changed_seq += 1;
    }
    Ok(())
}

/// K3 [동기화 시작] · K6-f [이 데이터로 시작] · K6-m [합치기 / Drive 데이터로 바꾸기 / Drive 새로 시작]
#[tauri::command]
pub async fn sync_join(state: State<'_, SyncState>, opts: JoinOpts) -> Res<()> {
    let sh = state.0.clone();
    let later = opts.later;
    let r = tauri::async_runtime::spawn_blocking({
        let sh = sh.clone();
        move || join_blocking(&sh, opts)
    })
    .await
    .map_err(err)?;
    refresh(&sh);
    r.map_err(err)?;
    if !later {
        enqueue(&sh, true, true);
    }
    Ok(())
}

/// 이 기기의 동기화 기록을 지운다 (연결 끊기 · 모든 데이터 삭제). 이 기기 데이터는 그대로다.
/// K7 두 기기에서 고친 카드(sync_conflict)는 이 기기 데이터라 남긴다: 30일 동안 진 쪽 내용으로 되돌릴 수 있다 (K31). 모든 데이터 삭제는 따로 지운다
pub fn forget_local(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "UPDATE sync_flags SET enabled = 0, applying = 0;
         DELETE FROM sync_outbox; DELETE FROM sync_tombstone;
         DROP TABLE IF EXISTS sync_local; DROP TABLE IF EXISTS sync_row; DROP TABLE IF EXISTS sync_alias;
         DROP TABLE IF EXISTS sync_anchor;",
    )
}

/// 이 계정으로 맞추는 다른 기기가 있는지 (90일 안에 맞춘 기기). 모르면(오프라인 · 만료) 있다고 본다.
/// Google 폐기는 토큰 하나가 아니라 이 계정이 앱에 준 권한 전체를 없애서, 다른 기기도 "다시 로그인"이 된다
fn others_active(sh: &Arc<Shared>) -> bool {
    let me = sh.with_db(engine::device_uid).unwrap_or_default();
    store_of(sh).ok().is_none_or(|s| others_in(&*s, &me, crate::db::now_ms()))
}

fn others_in(store: &dyn Store, me: &str, now: i64) -> bool {
    let Ok(r) = engine::remote(store) else { return true };
    engine::read_devices(store, &r).iter().any(|d| d["uid"].as_str() != Some(me) && now - d["at"].as_i64().unwrap_or(0) < engine::STALE_MS)
}

/// K10. delete_remote 면 Drive 의 백업과 동기화 데이터도 지우고 Google 권한도 폐기한다.
/// 남기면 다른 기기가 없을 때만 폐기하고, 있으면 이 기기 키체인의 토큰만 지운다
#[tauri::command]
pub async fn sync_disconnect(state: State<'_, SyncState>, delete_remote: bool) -> Res<()> {
    let sh = state.0.clone();
    let r = tauri::async_runtime::spawn_blocking({
        let sh = sh.clone();
        move || -> Result<(), SyncError> {
            let _g = sh.run.lock().map_err(store::other)?;
            if delete_remote {
                let store = store_of(&sh)?;
                for e in store.list("backups/")?.into_iter().chain(store.list("")?) {
                    store.delete(&e.path)?;
                }
            } else if let Ok(store) = store_of(&sh) {
                // 남기더라도 이 기기 파일은 지운다. 남으면 다른 기기가 끊은 기기를 90일 동안 기다린다 (기기 목록 · 정리 · 폐기 판단).
                // 못 지우면(오프라인) 90일 뒤 "오래 안 씀"으로 빠진다
                let me = sh.with_db(engine::device_uid).unwrap_or_default();
                let _ = store.delete(&format!("devices/{me}.json"));
            }
            if sh.folder.is_none()
                && (delete_remote || !others_active(&sh))
                && let Some(t) = auth::load_secret(auth::REFRESH)
            {
                auth::revoke(&sh.agent, &t);
            }
            auth::delete_secret(auth::REFRESH);
            auth::delete_secret(auth::KEY);
            *sh.token.lock().map_err(store::other)? = None;
            *sh.key.lock().map_err(store::other)? = None;
            *sh.account.lock().map_err(store::other)? = None;
            let con = sh.db().lock().map_err(store::other)?;
            forget_local(&con).map_err(store::other)?;
            engine::init(&con)
        }
    })
    .await
    .map_err(err)?;
    if let Ok(mut s) = sh.status.lock() {
        *s = Status::default();
    }
    refresh(&sh);
    r.map_err(err)
}

#[derive(Deserialize)]
pub struct SettingsOpts {
    pub device_name: Option<String>,
    pub backup: Option<String>,
}

/// K4 [바꾸기]: 기기 이름, Drive 자동 백업
#[tauri::command]
pub fn sync_settings(state: State<SyncState>, opts: SettingsOpts) -> Res<()> {
    state
        .0
        .with_db(|c| {
            if let Some(n) = opts.device_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                c.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('device_name', ?1)", [n]).map_err(store::other)?;
            }
            if let Some(b) = opts.backup.as_deref().filter(|b| ["daily", "weekly", "off"].contains(b)) {
                engine::set_local(c, "backup", b)?;
            }
            Ok(())
        })
        .map_err(err)?;
    refresh(&state.0);
    Ok(())
}

/// K4 암호로 잠그기 [바꾸기]. 켜거나 끄면 Drive 를 새 세대로 다시 올린다 (다른 기기는 K6-m 을 거친다)
#[tauri::command]
pub async fn sync_set_lock(state: State<'_, SyncState>, password: Option<String>) -> Res<()> {
    let sh = state.0.clone();
    let r = tauri::async_runtime::spawn_blocking({
        let sh = sh.clone();
        move || -> Result<(), SyncError> {
            // 끝까지 run 잠금을 쥔다: 새 세대로 옮긴 뒤 이 기기가 올린 변경을 아래에서 지우지 않게
            let _g = sh.run.lock().map_err(store::other)?;
            // 먼저 받는다: 다른 기기가 올리고 이 기기가 아직 받지 않은 변경 · 그림도 새 세대에 들어간다 (T3-09).
            // 고르는 창이 떠 있거나 Drive 가 없어졌으면 바꾸지 않는다
            match with_ctx(&sh, |cx| engine::pull(sh.db(), cx))? {
                Pulled::Done(_) => {}
                _ => return Err(store::other("먼저 Drive 데이터와 맞춰 주세요. 상단의 동기화 표시에서 고를 수 있어요.")),
            }
            let old_gen = sh.with_db(engine::gen_of)?;
            let kind = if password.as_deref().is_some_and(|p| !p.is_empty()) { "lock" } else { "unlock" };
            let opts = JoinOpts { mode: Join::Fresh, device_name: None, backup: sh.with_db(|c| engine::local(c, "backup"))?, password, later: false };
            // 지울 변경 파일은 새 세대를 올리기 전에 정한다
            let store = store_of(&sh)?;
            let before = engine::remote(&*store)?;
            join_locked(&sh, opts, kind)?;
            engine::drop_old_generation(&*store, old_gen, &before)
        }
    })
    .await
    .map_err(err)?;
    refresh(&sh);
    r.map_err(err)?;
    enqueue(&sh, true, true);
    Ok(())
}

/// 잠긴 Drive 를 이 기기에서 열 비밀번호 (K6-f 다음 "암호를 입력하세요")
#[tauri::command]
pub async fn sync_unlock(state: State<'_, SyncState>, password: String) -> Res<()> {
    let sh = state.0.clone();
    let r = tauri::async_runtime::spawn_blocking({
        let sh = sh.clone();
        move || {
            let store = store_of(&sh)?;
            unlock(&sh, &*store, &password)
        }
    })
    .await
    .map_err(err)?;
    r.map_err(err)?;
    enqueue(&sh, true, true);
    Ok(())
}

// ---------------------------------------------------------------- Drive 백업 (K4 · K9)

#[derive(Serialize)]
pub struct DriveBackup {
    pub name: String,
    pub at: i64,
    pub size: u64,
    pub device: Option<String>,
    pub cards: Option<i64>,
    pub reviews: Option<i64>,
}

fn backups_of(sh: &Arc<Shared>) -> Result<Vec<DriveBackup>, SyncError> {
    let store = store_of(sh)?;
    let r = engine::remote(&*store)?;
    let devices = engine::read_devices(&*store, &r);
    let mut out: Vec<DriveBackup> = store
        .list("backups/")?
        .into_iter()
        .filter_map(|e| {
            let name = e.path.strip_prefix("backups/")?.to_string();
            let info = devices.iter().find_map(|d| d["backups"].as_array()?.iter().find(|b| b["name"].as_str() == Some(&name)).map(|b| (d["name"].clone(), b.clone())));
            Some(DriveBackup {
                at: engine::backup_time(&name).unwrap_or(e.at),
                size: e.size,
                device: info.as_ref().and_then(|(n, _)| n.as_str().map(String::from)),
                cards: info.as_ref().and_then(|(_, b)| b["cards"].as_i64()),
                reviews: info.as_ref().and_then(|(_, b)| b["reviews"].as_i64()),
                name,
            })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.at));
    Ok(out)
}

#[tauri::command]
pub async fn sync_backups(state: State<'_, SyncState>) -> Res<Vec<DriveBackup>> {
    let sh = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || backups_of(&sh).map_err(err)).await.map_err(err)?
}

fn download_backup(sh: &Arc<Shared>, name: &str) -> Result<PathBuf, SyncError> {
    let store = store_of(sh)?;
    let data = store.get(&format!("backups/{name}"))?;
    // 잠그기 전에 만든 백업은 평문이다 (잠가도 지우지 않는다). 사용자가 골라 되돌리는 것이라 그대로 연다
    let zip = if crypto::is_sealed(&data) { engine::open_opt(data, key_of(sh).as_ref())? } else { data };
    let tmp = sh.state().data_dir.join("backups").join(format!(".drive-{}.sqlite", crypto::random_hex(6)));
    std::fs::create_dir_all(tmp.parent().expect("부모")).map_err(store::other)?;
    engine::unzip_db(&zip, &tmp)?;
    Ok(tmp)
}

#[derive(Serialize)]
pub struct RestorePreview {
    pub name: String,
    pub at: i64,
    pub device: Option<String>,
    pub cards: i64,
    pub reviews: i64,
    /// 이 백업 뒤의 복습 기록 · 카드 변경 (되돌리면 빠진다. 방금 남긴 이 기기 백업에는 있다)
    pub lost_reviews: i64,
    pub lost_changes: i64,
    /// 고르는 창을 보게 될 다른 기기
    pub others: Vec<String>,
}

/// K9 머리: 무엇이 사라지는지 숫자로 먼저
#[tauri::command]
pub async fn sync_restore_preview(state: State<'_, SyncState>, name: String) -> Res<RestorePreview> {
    let sh = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<RestorePreview, SyncError> {
        let tmp = download_backup(&sh, &name)?;
        let result = sh.with_db(|c| {
            c.execute("ATTACH DATABASE ?1 AS b", [tmp.to_string_lossy()]).map_err(store::other)?;
            let n = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).map_err(store::other);
            let r = (|| {
                Ok((
                    n("SELECT count(*) FROM b.card")?,
                    n("SELECT count(*) FROM b.review_log")?,
                    n("SELECT count(*) FROM main.review_log m WHERE NOT EXISTS (SELECT 1 FROM b.review_log x WHERE x.uid = m.uid)")?,
                    n("SELECT count(*) FROM main.note m WHERE NOT EXISTS (SELECT 1 FROM b.note x WHERE x.uid = m.uid AND x.fields = m.fields)")?,
                ))
            })();
            let _ = c.execute("DETACH DATABASE b", []);
            r
        });
        let _ = std::fs::remove_file(&tmp);
        let (cards, reviews, lost_reviews, lost_changes) = result?;
        let list = backups_of(&sh)?;
        let b = list.iter().find(|b| b.name == name);
        let others = sh.status.lock().map(|s| s.devices.iter().filter(|d| !d.me && !d.stale).map(|d| d.name.clone()).collect()).unwrap_or_default();
        Ok(RestorePreview { at: b.map(|b| b.at).unwrap_or(0), device: b.and_then(|b| b.device.clone()), name, cards, reviews, lost_reviews, lost_changes, others })
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// K9 되돌리기: ① 지금 DB 를 neoanki-before-restore-<시각>.zip 으로 ② Drive 백업으로 DB 복원 (그림은 동기화 공간에서)
/// ③ 세대를 올린 새 스냅샷 ④ 다른 기기는 다음 받기에서 K6-m. 하나라도 실패하면 ①로 되돌린다
#[tauri::command]
pub async fn sync_restore(state: State<'_, SyncState>, name: String) -> Res<()> {
    let sh = state.0.clone();
    let r = tauri::async_runtime::spawn_blocking({
        let sh = sh.clone();
        move || restore_blocking(&sh, &name)
    })
    .await
    .map_err(err)?;
    refresh(&sh);
    r.map_err(err)
}

fn restore_blocking(sh: &Arc<Shared>, name: &str) -> Result<(), SyncError> {
    let _g = sh.run.lock().map_err(store::other)?;
    sh.set(|s| {
        s.state = "syncing".into();
        s.phase = Some("restore".into());
    });
    let tmp = download_backup(sh, name)?;
    local_backup(sh, "before-restore")?;
    // 이 기기의 정체 (기기 id · 이름 · HLC) 와 연결 설정은 백업이 아니라 지금 것을 쓴다
    {
        let restored = crate::db::open(&tmp).map_err(|e| store::other(format!("백업을 열지 못했어요: {e}")))?;
        sh.with_db(|c| engine::adopt_restored(c, &restored))?;
    }
    let data_dir = sh.state().data_dir.clone();
    let db_path = data_dir.join("neoanki.sqlite");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let old = |suffix: &str| data_dir.join(format!(".neoanki-old-{stamp}.sqlite{suffix}"));
    let swap = |from: &dyn Fn(&str) -> PathBuf, to: &dyn Fn(&str) -> PathBuf| {
        for suffix in ["", "-wal", "-shm"] {
            if from(suffix).exists() {
                let _ = std::fs::rename(from(suffix), to(suffix));
            }
        }
    };
    let cur = |suffix: &str| PathBuf::from(format!("{}{suffix}", db_path.display()));
    let restored: Result<(), SyncError> = (|| {
        let mut guard = sh.db().lock().map_err(store::other)?;
        // Windows 는 열린 파일을 옮기지 못한다: 연결부터 닫는다
        *guard = crate::db::open_in_memory().map_err(store::other)?;
        swap(&cur, &old);
        std::fs::copy(&tmp, &db_path).map_err(store::other)?;
        *guard = crate::db::open(&db_path).map_err(|e| store::other(format!("백업을 열지 못했어요: {e}")))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    let undo = |e: SyncError| -> SyncError {
        if let Ok(mut guard) = sh.db().lock() {
            if let Ok(mem) = crate::db::open_in_memory() {
                *guard = mem;
            }
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(cur(suffix));
            }
            swap(&old, &cur);
            if let Ok(con) = crate::db::open(&db_path) {
                *guard = con;
            }
        }
        e
    };
    restored.map_err(undo)?;
    let reason = json!({
        "kind": "restore", "backup": name, "backup_at": engine::backup_time(name),
        "device_name": sh.with_db(|c| engine::meta(c, "device_name"))?,
    });
    let joined = with_ctx(sh, |cx| engine::join(sh.db(), cx, Join::Fresh, reason).map(|_| ()));
    joined.map_err(undo)?;
    // 백업에 있던 그림 중 이 기기에 없는 것은 동기화 공간에서
    let _ = with_ctx(sh, |cx| {
        let r = engine::remote(cx.store)?;
        engine::fetch_media(sh.db(), cx, &r.media)
    });
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(old(suffix));
    }
    sh.set(|s| {
        s.state = "idle".into();
        s.phase = None;
        s.changed_seq += 1;
    });
    Ok(())
}

/// K4 ⋯ [그림까지 묶어 내려받기]: Drive 백업 + 그림을 이 기기 백업(I19)과 같은 zip 으로
#[tauri::command]
pub async fn sync_backup_full(state: State<'_, SyncState>, name: String, path: String) -> Res<String> {
    let sh = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, SyncError> {
        use std::io::Write;
        let tmp = download_backup(&sh, &name)?;
        let files: Vec<(String, String)> = {
            let c = Connection::open(&tmp).map_err(store::other)?;
            let mut st = c.prepare("SELECT filename, sha256 FROM media").map_err(store::other)?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(store::other)?.filter_map(Result::ok).collect()
        };
        let result = with_ctx(&sh, |cx| {
            let out = std::fs::File::create(&path).map_err(store::other)?;
            let mut zip = zip::ZipWriter::new(out);
            let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("neoanki.sqlite", deflate).map_err(store::other)?;
            zip.write_all(&std::fs::read(&tmp).map_err(store::other)?).map_err(store::other)?;
            for (i, (file, sha)) in files.iter().enumerate() {
                (cx.progress)("media", i as u64, files.len() as u64);
                if crate::commands::safe_media_name(file).is_err() {
                    continue;
                }
                let bytes = match std::fs::read(cx.media_dir.join(file)) {
                    Ok(b) => b,
                    Err(_) => match cx.store.get(&engine::media_path(sha, file, cx.key.as_ref())) {
                        Ok(b) => engine::open_opt(b, cx.key.as_ref())?,
                        Err(_) => continue,
                    },
                };
                zip.start_file(format!("media/{file}"), stored).map_err(store::other)?;
                zip.write_all(&bytes).map_err(store::other)?;
            }
            zip.finish().map_err(store::other)?;
            Ok(path.clone())
        });
        let _ = std::fs::remove_file(&tmp);
        sh.set(|s| s.phase = None);
        result
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// K4 ⋯ [이 백업 지우기]
#[tauri::command]
pub async fn sync_delete_backup(state: State<'_, SyncState>, name: String) -> Res<()> {
    let sh = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || store_of(&sh)?.delete(&format!("backups/{name}"))).await.map_err(err)?.map_err(err)
}

/// K5 공간 부족 [오래된 백업 정리]: 가장 새 백업 3개만 남긴다
#[tauri::command]
pub async fn sync_prune_backups(state: State<'_, SyncState>) -> Res<usize> {
    let sh = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<usize, SyncError> {
        let store = store_of(&sh)?;
        let list = backups_of(&sh)?;
        for b in list.iter().skip(3) {
            store.delete(&format!("backups/{}", b.name))?;
        }
        Ok(list.len().saturating_sub(3))
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// K4 [Drive에서 열기] · K5 [Drive 열기]: 보이는 백업 폴더를 브라우저(개발용 폴더면 탐색기)로
#[tauri::command]
pub async fn sync_open_folder(state: State<'_, SyncState>) -> Res<()> {
    let sh = state.0.clone();
    let link = tauri::async_runtime::spawn_blocking(move || store_of(&sh)?.folder_link()).await.map_err(err)?.map_err(err)?;
    open_url(&link);
    Ok(())
}

/// 시스템 기본 브라우저 (또는 탐색기)로 연다. 앱 안 웹뷰로 로그인하지 않는다 (Google 정책)
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let r = if url.starts_with("http") {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()
    } else {
        std::process::Command::new("explorer").arg(url).spawn()
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    let _ = r;
}

// ---------------------------------------------------------------- K7 두 기기에서 고친 카드

#[derive(Serialize)]
pub struct Version {
    pub device: String,
    pub at: i64,
    /// 필드 값 (HTML 을 뺀 글자)
    pub fields: Vec<String>,
    pub current: bool,
    /// 자동으로 맞출 때 이긴 버전 (나중에 고친 쪽)
    pub winner: bool,
}

#[derive(Serialize)]
pub struct ConflictView {
    pub id: i64,
    pub question: String,
    pub deck_path: Vec<String>,
    pub versions: [Version; 2],
    /// [이 버전으로 바꾸기]로 사용자가 고른 적이 있다. 지금 내용 꼬리표: 고른 적 없으면 "나중에 고친 쪽", 있으면 "직접 고른 내용" (T3-02)
    pub picked: bool,
}

/// HTML -> 줄 단위 글자 (줄바꿈 태그는 줄로)
fn text(html: &str) -> String {
    // strip_tags 는 공백을 하나로 접는다: 줄을 먼저 나누고 줄마다 벗긴다
    let mut s = html.to_string();
    for tag in ["<br>", "<br/>", "<br />", "</div>", "</p>", "</li>"] {
        s = s.replace(tag, "\u{1}").replace(&tag.to_uppercase(), "\u{1}");
    }
    s.split('\u{1}').map(crate::render::strip_tags).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 노트 한 버전을 카드 첫 장으로 그린다: [앞면, 답]. 템플릿을 모르면 필드 그대로
fn render_note(c: &Connection, note_uid: &str, fields: &str) -> Vec<String> {
    let values: Vec<String> = serde_json::from_str(fields).unwrap_or_default();
    let tpl: Option<(String, String, String, i64)> = c
        .query_row(
            "SELECT nt.fields, t.qfmt, t.afmt, coalesce((SELECT min(template_ord) FROM card WHERE note_id = n.id), 0)
             FROM note n JOIN note_type nt ON nt.id = n.note_type_id JOIN card_template t ON t.note_type_id = nt.id
             WHERE n.uid = ?1 ORDER BY t.ord LIMIT 1",
            [note_uid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((names, q, a, ord)) = tpl else { return values.iter().map(|v| text(v)).collect() };
    let names: Vec<String> = serde_json::from_str(&names).unwrap_or_default();
    let map = names.into_iter().zip(values).collect();
    let r = crate::render::render_card(&q, &a, &map, ord as u32);
    vec![text(&r.front), text(crate::commands::answer_part(&r.back))]
}

/// 지난 30일 K7 기록. 지금 적용된 쪽이 앞이다
#[tauri::command]
pub fn sync_conflicts(state: State<SyncState>) -> Res<Vec<ConflictView>> {
    let names: std::collections::HashMap<String, String> =
        state.0.status.lock().map(|s| s.devices.iter().map(|d| (d.uid.clone(), d.name.clone())).collect()).unwrap_or_default();
    let name = |uid: &str| names.get(uid).cloned().unwrap_or_else(|| "다른 기기".into());
    state
        .0
        .with_db(|c| {
            let since = crate::db::now_ms() - 30 * crate::db::DAY_MS;
            let mut st = c
                .prepare(
                    "SELECT x.id, x.winner_device, x.winner_at, x.winner_fields, x.loser_device, x.loser_at, x.loser_fields, n.fields,
                            (SELECT deck_id FROM card WHERE note_id = n.id ORDER BY template_ord LIMIT 1), x.note_uid, x.resolved
                     FROM sync_conflict x JOIN note n ON n.uid = x.note_uid WHERE x.at > ?1 ORDER BY x.id DESC",
                )
                .map_err(store::other)?;
            type Row = (i64, String, i64, String, String, i64, String, String, Option<i64>, String, i64);
            let rows: Vec<Row> = st
                .query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)))
                .map_err(store::other)?
                .filter_map(Result::ok)
                .collect();
            let mut out = vec![];
            for (id, wd, wa, wf, ld, la, lf, now, deck, uid, picked) in rows {
                let mut a = Version { device: name(&wd), at: wa, fields: render_note(c, &uid, &wf), current: wf == now, winner: true };
                let mut b = Version { device: name(&ld), at: la, fields: render_note(c, &uid, &lf), current: lf == now && wf != now, winner: false };
                if b.current {
                    std::mem::swap(&mut a, &mut b);
                }
                let mut path = vec![];
                let mut d = deck;
                while let Some(id) = d {
                    let row: Option<(String, Option<i64>)> =
                        c.query_row("SELECT name, parent_id FROM deck WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(store::other)?;
                    let Some((n, p)) = row else { break };
                    path.insert(0, n);
                    d = p;
                }
                let question = render_note(c, &uid, &now).into_iter().next().unwrap_or_default();
                out.push(ConflictView { id, question, deck_path: path, versions: [a, b], picked: picked != 0 });
            }
            Ok(out)
        })
        .map_err(err)
}

/// K7 [이 버전으로 바꾸기]: 다른 쪽 내용으로 고친다. 이것도 새 변경 하나로 동기화된다
#[tauri::command]
pub fn sync_conflict_use(state: State<SyncState>, id: i64) -> Res<()> {
    state
        .0
        .with_db(|c| {
            let (note, wf, lf): (String, String, String) =
                c.query_row("SELECT note_uid, winner_fields, loser_fields FROM sync_conflict WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(store::other)?;
            let now: String = c.query_row("SELECT fields FROM note WHERE uid = ?1", [&note], |r| r.get(0)).map_err(store::other)?;
            let other = if now == wf { lf } else { wf };
            c.execute("UPDATE note SET fields = ?2, updated_at = ?3 WHERE uid = ?1", rusqlite::params![note, other, crate::db::now_ms()]).map_err(store::other)?;
            c.execute("UPDATE sync_conflict SET resolved = 1 WHERE id = ?1", [id]).map_err(store::other)?;
            Ok(())
        })
        .map_err(err)?;
    refresh(&state.0);
    Ok(())
}

/// K7 토스트를 보여줬다 (다시 띄우지 않는다)
#[tauri::command]
pub fn sync_conflicts_seen(state: State<SyncState>) -> Res<()> {
    state.0.with_db(|c| c.execute("UPDATE sync_conflict SET seen = 1", []).map(|_| ()).map_err(store::other)).map_err(err)?;
    refresh(&state.0);
    Ok(())
}

#[cfg(test)]
mod revoke_tests {
    use super::*;
    use store::{Entry, SRes};

    /// 목록을 못 읽는 저장소 (오프라인 · 만료)
    struct Down;
    impl Store for Down {
        fn list(&self, _: &str) -> SRes<Vec<Entry>> {
            Err(SyncError::Offline("x".into()))
        }
        fn get(&self, _: &str) -> SRes<Vec<u8>> {
            Err(SyncError::Auth)
        }
        fn put(&self, _: &str, _: &[u8]) -> SRes<()> {
            Err(SyncError::Auth)
        }
        fn delete(&self, _: &str) -> SRes<()> {
            Err(SyncError::Auth)
        }
        fn about(&self) -> SRes<(String, Option<(u64, u64)>)> {
            Err(SyncError::Auth)
        }
        fn folder_link(&self) -> SRes<String> {
            Err(SyncError::Auth)
        }
    }

    #[test]
    fn revoke_only_without_other_devices() {
        let dir = std::env::temp_dir().join(format!("neoanki-revoke-{}", crypto::random_hex(6)));
        let s = FolderStore::new(&dir);
        let now = 1000 * crate::db::DAY_MS;
        let dev = |uid: &str, at: i64| s.put(&format!("devices/{uid}.json"), json!({ "uid": uid, "at": at }).to_string().as_bytes()).unwrap();
        assert!(!others_in(&s, "me", now), "Drive 에 아무도 없으면 폐기한다");
        dev("me", now);
        assert!(!others_in(&s, "me", now), "나만 있으면 폐기한다");
        dev("old", now - engine::STALE_MS - 1);
        assert!(!others_in(&s, "me", now), "90일 넘게 안 맞춘 기기는 기다리지 않는다");
        dev("b", now - crate::db::DAY_MS);
        assert!(others_in(&s, "me", now), "다른 기기가 쓰고 있으면 폐기하지 않는다 (그 기기도 로그아웃된다)");
        assert!(others_in(&Down, "me", now), "모르면 있다고 본다");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restarting_never_turns_the_lock_off_silently() {
        // T3-11: "이 기기 데이터로 다시 시작"(fresh, 비밀번호 없음)이 잠금을 말없이 끄지 않는다
        let c = || Some(b"{}".to_vec());
        let keep = |password: Option<&str>| LockPlan::Keep { crypto: b"{}".to_vec(), password: password.map(String::from) };
        assert_eq!(lock_plan("unlock", None, true, true, c(), c()), Ok(LockPlan::Off), "K4 잠금 풀기만 끈다");
        assert_eq!(lock_plan("lock", Some("pw"), false, false, None, None), Ok(LockPlan::New("pw".into())));
        assert_eq!(lock_plan("fresh", None, false, false, None, None), Ok(LockPlan::Off), "잠그지 않은 기기 · Drive");
        // 잠근 기기가 지운 Drive 를 다시 시작: 이 기기에 남긴 crypto.json 과 키체인 키로 그대로 잠근다
        assert_eq!(lock_plan("fresh", None, true, true, None, c()), Ok(keep(None)));
        assert_eq!(lock_plan("fresh", None, true, true, c(), None), Ok(keep(None)), "잠긴 Drive 그대로");
        // 남긴 것이나 키가 없으면 비밀번호를 묻는다
        assert_eq!(lock_plan("fresh", None, true, true, None, None), Err(SyncError::Locked));
        assert_eq!(lock_plan("fresh", None, true, false, None, c()), Err(SyncError::Locked));
        // 다른 기기가 잠근 Drive 를 잠그지 않은 기기가 새로 시작: 비밀번호를 묻고, 넣으면 그 잠금을 잇는다
        assert_eq!(lock_plan("fresh", None, false, false, c(), None), Err(SyncError::Locked));
        assert_eq!(lock_plan("fresh", Some("pw"), false, false, c(), None), Ok(keep(Some("pw"))));
        assert_eq!(lock_plan("fresh", Some("pw"), true, false, None, None), Ok(LockPlan::New("pw".into())), "남긴 잠금이 없으면 새로 잠근다");
    }
}
