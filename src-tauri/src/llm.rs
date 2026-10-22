//! 8.3 서술형 2단계: 개념 관계 확인 (DECISIONS J). 본체 쪽: 모델 받기 · 지우기 · 상태, 추론 엔진(보조 실행 파일 src-tauri/llm) 띄우기 · 주고받기,
//! 프롬프트 · 판정 규칙 · 캐시. 추론(candle)은 엔진이 한다: 본체는 AVX2 없이 빌드해 모든 CPU 에서 돌고, 엔진만 AVX2 로 빌드한다
//! (candle 의 양자화 계산은 컴파일 때만 AVX2 를 켠다). 엔진은 CPU 가 AVX2 · FMA · F16C 를 지원하고, 앱 리소스 폴더의 llm/neoanki-llm.exe 가
//! 빌드 때 잰 크기 · SHA-256 과 같을 때만 그 절대 경로로 띄운다(현재 폴더 · PATH 를 찾지 않는다).
//! 모델은 설치 파일에 넣지 않는다. 사용자가 크기 · 남은 공간 · 라이선스 · 출처를 보고 [내려받기]를 누를 때만 고정 리비전에서 받고 파일마다 크기와 SHA-256 을 확인한다.
//! 모델이 없거나 · 이 PC 에서 쓸 수 없거나 · 엔진이 멈춰도 핵심어 검사와 평가는 그대로다. 결과는 비교 화면에 참고로만 보인다 (추천은 핵심어만, 디자인 J4)

use crate::commands::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 받을 파일 하나: 저장 이름, 고정 리비전 주소, 바이트, SHA-256
struct File {
    name: &'static str,
    url: &'static str,
    size: u64,
    sha: &'static str,
}

/// 모델 폴더 (data_dir/models/DIR). 모델을 바꾸면 폴더 이름도 바꾼다 (옛 파일과 섞이지 않게)
const DIR: &str = "qwen2.5-1.5b-instruct-q4km";
/// Qwen2.5-1.5B-Instruct Q4_K_M (Qwen 팀 공식 GGUF) · tokenizer.json (원본 저장소). 리비전을 고정했다. 바꾸면 크기와 해시를 함께 다시 적는다.
/// 사용자 결정 (2026-10-20): 기존 1.5B 를 기본으로 둔다 (0.6B 로 줄이는 것은 정확도 비교 없이 정하지 않는다)
const FILES: [File; 2] = [
    File {
        name: "model.gguf",
        url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/91cad51170dc346986eccefdc2dd33a9da36ead9/qwen2.5-1.5b-instruct-q4_k_m.gguf",
        size: 1_117_320_736,
        sha: "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e",
    },
    File {
        name: "tokenizer.json",
        url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct/resolve/989aa7980e4cf806f80c7fef2b1adb7bc71aa306/tokenizer.json",
        size: 7_031_645,
        sha: "c0382117ea329cdf097041132f6d735924b697924d6f6fc3945713e96ce87539",
    },
];
/// 받기 확인 창 · 설정 줄에 그대로 보인다 (모델 목록 값)
const NAME: &str = "Qwen2.5-1.5B-Instruct (4비트)";
const SHORT: &str = "1.5B 모델 (4비트)";
const LICENSE: &str = "Apache-2.0";
const SOURCE: &str = "Hugging Face · Qwen/Qwen2.5-1.5B-Instruct-GGUF (Qwen 팀 공식)";
/// 연결이 멈췄을 때 (데이터가 dl::IDLE 동안 오지 않음)
const STALLED: &str = "연결이 멈춰 모델을 내려받지 못했어요. 인터넷 연결을 확인하고 다시 시도해 주세요.";
/// 받은 뒤에도 남겨 둘 여유 (임시 파일 · 다른 기능)
const MARGIN: u64 = 200_000_000;

static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// 확인 차례 · 미리 하기 차례. 새 확인 · 멈추기 · 지우기 · 떠나기가 올리면 돌던 일이 멈춘다 (미리 하기는 비교를 눌러도 이어서 쓴다)
static JOB: AtomicU64 = AtomicU64::new(0);
static PREP: AtomicU64 = AtomicU64::new(0);
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
/// 엔진이 다시 띄워도 멈췄다: 이번 학습에서는 핵심어 채점만 (학습 화면을 떠나면 풀린다)
static OFF: AtomicBool = AtomicBool::new(false);
/// 지난 확인 한 번(비교 → 결과)에 걸린 시간 (ms, 0 = 아직 없음)
static LAST_MS: AtomicU64 = AtomicU64::new(0);

fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(DIR)
}

/// 받은 파일이 모두 있는가. 해시는 받을 때 확인했으므로 여기서는 크기만 본다
fn installed(dir: &Path) -> bool {
    FILES.iter().all(|f| dir.join(f.name).metadata().is_ok_and(|m| m.len() == f.size))
}

/// 모델을 받았는가
pub fn ready(data_dir: &Path) -> bool {
    installed(&model_dir(data_dir))
}

/// 그 폴더가 있는 드라이브의 남은 공간. 폴더가 아직 없으면 있는 윗 폴더에서 잰다
#[cfg(windows)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let dir = dir.ancestors().find(|p| p.exists())?;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut avail = 0u64;
    // SAFETY: 끝이 0 인 경로와 u64 하나를 넘긴다. 나머지 둘은 받지 않는다(null)
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(avail)
}

/// ponytail: 설치 파일은 Windows(nsis)뿐이라 다른 OS 는 재지 않는다 (확인 창에 "확인하지 못했어요")
#[cfg(not(windows))]
fn free_bytes(_: &Path) -> Option<u64> {
    None
}

// ------------------------------------------------------------------ 이 PC 에서 쓸 수 있는가

/// 엔진은 +avx2,+fma,+f16c 로 빌드한다. 하나라도 없으면 띄우지 않는다 (띄우면 잘못된 명령으로 죽는다)
#[cfg(target_arch = "x86_64")]
pub fn cpu_ok() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("NEOANKI_LLM_NO_AVX2").is_some() {
        return false; // 개발 · 캡처: AVX2 없는 PC 흉내
    }
    std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") && std::is_x86_feature_detected!("f16c")
}

#[cfg(not(target_arch = "x86_64"))]
pub fn cpu_ok() -> bool {
    false
}

/// build.rs 가 resources/llm/neoanki-llm.exe 에서 잰 크기 · SHA-256 (없이 빌드하면 빈 값 → 엔진 없음)
const ENGINE_SIZE: &str = env!("NEOANKI_LLM_SIZE");
const ENGINE_SHA: &str = env!("NEOANKI_LLM_SHA");

fn sha256_file(path: &Path) -> Res<String> {
    let mut f = std::fs::File::open(path).map_err(err)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// 띄워도 되는 엔진 파일인가: 빌드 때 잰 크기 · SHA-256 과 같다 (바꿔치기 막기)
fn trusted(path: &Path) -> bool {
    !ENGINE_SHA.is_empty()
        && path.metadata().is_ok_and(|m| m.len().to_string() == ENGINE_SIZE)
        && sha256_file(path).is_ok_and(|h| h == ENGINE_SHA)
}

/// 엔진 실행 파일: 앱 리소스 폴더의 절대 경로만. 디버그 빌드는 NEOANKI_LLM_BIN 으로 바꿀 수 있다 (측정 · 캡처)
pub fn engine_bin(app: &AppHandle) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("NEOANKI_LLM_BIN") {
        return Some(p.into());
    }
    let p = app.path().resource_dir().ok()?.join("llm").join("neoanki-llm.exe");
    (p.is_absolute() && trusted(&p)).then_some(p)
}

// ------------------------------------------------------------------ 상태 · 받기 · 지우기

#[derive(Serialize)]
pub struct LlmStatus {
    ready: bool,
    downloading: bool,
    /// 내려받을 전체 크기
    bytes: u64,
    /// 모델 폴더가 있는 드라이브의 남은 공간. 모르면 없다
    free: Option<u64>,
    /// 받기 전에 비워 둬야 할 공간 (크기 + 여유)
    need: u64,
    name: &'static str,
    short: &'static str,
    license: &'static str,
    source: &'static str,
    /// 이 PC 의 CPU 가 AVX2 · FMA · F16C 를 지원한다 (아니면 이 기능을 쓸 수 없다)
    cpu_ok: bool,
    /// 이 설치에 믿을 수 있는 엔진 파일이 있다 (개발 빌드는 tools/build_llm.ps1 전에는 없다)
    engine: bool,
    /// 엔진이 다시 띄워도 멈춰 이번 학습에서는 껐다
    off: bool,
    /// 지난 확인 한 번에 걸린 시간 (ms)
    last_ms: Option<u64>,
}

/// 받는 중에 앱이 닫혀 남은 .part 를 지운다 (받는 중이 아닐 때만. 그동안 새 받기가 시작되지 않게 깃발을 잠깐 쥔다)
fn clean_parts(dir: &Path) {
    if DOWNLOADING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return;
    }
    for f in &FILES {
        let _ = std::fs::remove_file(dir.join(format!("{}.part", f.name)));
    }
    DOWNLOADING.store(false, Ordering::SeqCst);
}

#[tauri::command]
pub fn llm_status(app: AppHandle) -> LlmStatus {
    let dir = model_dir(&app.state::<AppState>().data_dir);
    clean_parts(&dir);
    let bytes = FILES.iter().map(|f| f.size).sum();
    LlmStatus {
        ready: installed(&dir),
        downloading: DOWNLOADING.load(Ordering::SeqCst),
        bytes,
        free: free_bytes(&dir),
        need: bytes + MARGIN,
        name: NAME,
        short: SHORT,
        license: LICENSE,
        source: SOURCE,
        cpu_ok: cpu_ok(),
        engine: engine_bin(&app).is_some(),
        off: OFF.load(Ordering::SeqCst),
        last_ms: Some(LAST_MS.load(Ordering::SeqCst)).filter(|&m| m > 0),
    }
}

#[derive(Serialize, Clone)]
struct DownloadProgress {
    done: u64,
    total: u64,
}

/// [내려받기]. "llm-progress" 이벤트로 진행을 알린다. 이 PC 에서 쓸 수 없거나 공간이 모자라면 시작하지 않고, 멈추거나 실패하면 받던 파일만 지운다
#[tauri::command]
pub async fn llm_download(app: AppHandle) -> Res<()> {
    if !cpu_ok() {
        return Err("이 PC의 CPU가 빠른 계산(AVX2)을 지원하지 않아 쓸 수 없어요.".into());
    }
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("이미 내려받고 있어요.".into());
    }
    CANCEL.store(false, Ordering::SeqCst);
    let dir = model_dir(&app.state::<AppState>().data_dir);
    let res = tauri::async_runtime::spawn_blocking(move || download(&app, &dir)).await.map_err(err);
    DOWNLOADING.store(false, Ordering::SeqCst);
    res?
}

#[tauri::command]
pub fn llm_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 돌던 확인을 멈추고 엔진을 끝낸다 (메모리를 돌려준다). 엔진이 일하는 중이면 그 요청이 멈출 때까지 기다리므로 백그라운드에서
async fn unload() -> Res<()> {
    llm_stop();
    tauri::async_runtime::spawn_blocking(|| ENGINE.lock().map(|mut e| *e = None).map_err(err)).await.map_err(err)?
}

/// 모델 지우기. 돌던 확인은 멈춘다
#[tauri::command]
pub async fn llm_delete(app: AppHandle) -> Res<()> {
    if DOWNLOADING.load(Ordering::SeqCst) {
        return Err("내려받기가 끝난 뒤에 지울 수 있어요.".into());
    }
    unload().await?;
    LAST_MS.store(0, Ordering::SeqCst); // "지난 확인은 N초" 는 지운 모델의 값이다
    let dir = model_dir(&app.state::<AppState>().data_dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|_| "모델 파일을 지우지 못했어요. 앱을 다시 연 뒤에 해 주세요.".to_string())?;
    }
    Ok(())
}

/// 받을 주소. 디버그 빌드는 NEOANKI_LLM_URL(파일 이름 앞부분)로 바꿀 수 있다 (캡처 · 오프라인 시험, 실제 모델을 다시 받지 않는다)
fn url_of(f: &File) -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("NEOANKI_LLM_URL") {
        return format!("{}/{}", base.trim_end_matches('/'), f.name);
    }
    f.url.to_string()
}

fn download(app: &AppHandle, dir: &Path) -> Res<()> {
    std::fs::create_dir_all(dir).map_err(err)?;
    let total: u64 = FILES.iter().map(|f| f.size).sum();
    // 이미 다 받은 파일은 다시 받지 않으니 남은 것만 센다
    let left: u64 = FILES.iter().filter(|f| !dir.join(f.name).metadata().is_ok_and(|m| m.len() == f.size)).map(|f| f.size).sum();
    if free_bytes(dir).is_some_and(|free| free < left + MARGIN) {
        return Err(format!("저장 공간이 모자라요. {}GB 이상 비운 뒤 다시 해 주세요.", gb(left + MARGIN)));
    }
    let mut done = 0;
    for f in &FILES {
        let path = dir.join(f.name);
        if path.metadata().is_ok_and(|m| m.len() == f.size) && sha256_file(&path)? == f.sha {
            done += f.size;
            continue;
        }
        let part = dir.join(format!("{}.part", f.name));
        let res = fetch(app, f, &part, done, total);
        if res.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        res?;
        std::fs::rename(&part, &path).map_err(err)?;
        done += f.size;
    }
    let _ = app.emit("llm-progress", DownloadProgress { done: total, total });
    Ok(())
}

/// "1.3" (GB, 소수 한 자리)
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

/// 파일 하나 받기. 읽기는 dl::stream 이 따로 도는 스레드에서 해서, 연결이 멈춰도 [멈추기]가 곧바로 먹고 30초 동안 데이터가 없으면 끝난다.
/// 화면에는 사용자 문구만, 원인 글은 로그로 (J1)
fn fetch(app: &AppHandle, f: &File, part: &Path, base: u64, total: u64) -> Res<()> {
    let mut file = std::fs::File::create(part).map_err(err)?;
    let mut h = Sha256::new();
    let (mut got, mut last) = (0u64, 0u64);
    // ureq 의 limit 는 딱 그 크기에서도 "더 있는지" 보다가 오류를 낸다. 한 바이트 넉넉히 두고 크기는 아래에서 맞춘다
    let res = crate::dl::stream(&url_of(f), f.size + 1, crate::dl::IDLE, &|| CANCEL.load(Ordering::SeqCst), |b| {
        h.update(b);
        file.write_all(b).map_err(err)?;
        got += b.len() as u64;
        if got - last >= 1 << 22 {
            last = got;
            let _ = app.emit("llm-progress", DownloadProgress { done: base + got, total });
        }
        Ok(())
    });
    match res {
        Ok(()) => {}
        Err(crate::dl::Fail::Cancelled) => return Err("내려받기를 멈췄어요.".into()),
        Err(crate::dl::Fail::Stalled) => return Err(STALLED.into()),
        Err(crate::dl::Fail::Net(e)) => {
            eprintln!("[llm] {} 받기 실패: {e}", f.name);
            return Err("모델을 내려받지 못했어요. 인터넷 연결을 확인하고 다시 시도해 주세요.".into());
        }
        Err(crate::dl::Fail::Write(e)) => return Err(e),
    }
    file.flush().map_err(err)?;
    if got != f.size || format!("{:x}", h.finalize()) != f.sha {
        return Err("받은 모델 파일이 원본과 달라요. 다시 시도해 주세요.".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ 엔진 (보조 실행 파일과 한 줄 JSON 주고받기)

/// 한 번의 기록 (측정용): 새로 읽은 토큰 · 쓴 토큰 · 걸린 시간
#[allow(dead_code)] // 측정(eval83)에서 읽는다
#[derive(Clone, Copy, Debug, Default)]
pub struct Gen {
    pub prompt: u64,
    pub out: u64,
    pub ms: u64,
}

const STOPPED: &str = "확인을 멈췄어요.";
const DEAD: &str = "개념 관계 확인 엔진이 멈췄어요.";
const BROKEN: &str = "개념 관계 확인 엔진의 답이 깨졌어요.";
const OFF_MSG: &str = "엔진이 다시 띄워도 멈춰서 이번 학습에서는 개념 관계 확인을 껐어요.";

/// 엔진 프로세스 하나. 떨어뜨리면(지우기 · 떠나기 · 실패) 끝낸다. 본체가 죽으면 엔진의 stdin 이 닫혀 엔진도 스스로 끝난다
pub struct Engine {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<String>,
    next: u64,
    arch: String,
    pub log: Vec<Gen>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Engine {
    /// 띄우고 모델을 읽힌다. args 는 시험용 (가짜 엔진)
    pub fn start(bin: &Path, args: &[&str], model: &Path, stop: &dyn Fn() -> bool) -> Res<Engine> {
        let mut cmd = Command::new(bin);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: 콘솔 창을 띄우지 않는다
        }
        let mut child = cmd.spawn().map_err(|e| format!("개념 관계 확인 엔진을 띄우지 못했어요. ({e})"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(DEAD.into());
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut e = Engine { child, stdin, rx, next: 0, arch: String::new(), log: vec![] };
        let v = e.call(json!({ "op": "load", "dir": model }), stop)?;
        e.arch = v["arch"].as_str().unwrap_or_default().to_string();
        Ok(e)
    }

    /// 요청 하나를 보내고 답을 기다린다. stop() 이 참이 되면 멈추라고 보내고, 5초 안에 답이 없으면 엔진이 멈춘 것으로 본다
    fn call(&mut self, mut req: Value, stop: &dyn Fn() -> bool) -> Res<Value> {
        self.next += 1;
        let id = self.next;
        req["id"] = id.into();
        writeln!(self.stdin, "{req}").and_then(|_| self.stdin.flush()).map_err(|_| DEAD.to_string())?;
        let mut cancelled: Option<Instant> = None;
        loop {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    let v: Value = serde_json::from_str(&line).map_err(|_| BROKEN.to_string())?;
                    if v["id"].as_u64() != Some(id) {
                        continue; // 앞서 멈춘 요청의 늦은 답
                    }
                    return match v["err"].as_str() {
                        Some("cancelled") => Err(STOPPED.into()),
                        Some(e) => Err(e.into()),
                        None => Ok(v["ok"].clone()),
                    };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancelled.is_none() && stop() {
                        let _ = writeln!(self.stdin, "{}", json!({ "op": "cancel", "id": id })).and_then(|_| self.stdin.flush());
                        cancelled = Some(Instant::now());
                    }
                    if cancelled.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
                        return Err(DEAD.into());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(DEAD.into()),
            }
        }
    }

    fn record(&mut self, v: &Value) {
        let n = |k: &str| v["gen"][k].as_u64().unwrap_or(0);
        self.log.push(Gen { prompt: n("prompt"), out: n("out"), ms: n("ms") });
    }

    fn qwen3(&self) -> bool {
        self.arch == "qwen3"
    }

    /// 앞부분을 미리 읽어 둔다. 같은 앞부분으로 generate · choose 하면 그 뒤만 읽는다
    pub fn prime(&mut self, prefix: &str, stop: &dyn Fn() -> bool) -> Res<()> {
        self.call(json!({ "op": "prime", "prefix": prefix }), stop).map(|_| ())
    }

    /// prefix + rest 다음을 탐욕 디코딩
    pub fn generate(&mut self, prefix: &str, rest: &str, max: usize, stop: &dyn Fn() -> bool) -> Res<String> {
        let v = self.call(json!({ "op": "generate", "prefix": prefix, "rest": rest, "max": max }), stop)?;
        self.record(&v);
        v["text"].as_str().map(String::from).ok_or_else(|| BROKEN.to_string())
    }

    /// 자리마다 options 위의 로그 확률 (엔진이 가장 높은 것을 이어 붙여 다음 자리로 간다)
    pub fn choose(&mut self, prefix: &str, rest: &str, slots: &[String], options: &[String], stop: &dyn Fn() -> bool) -> Res<Vec<Vec<f32>>> {
        let v = self.call(json!({ "op": "choose", "prefix": prefix, "rest": rest, "slots": slots, "options": options }), stop)?;
        self.record(&v);
        let lp: Vec<Vec<f32>> = serde_json::from_value(v["logprobs"].clone()).map_err(|_| BROKEN.to_string())?;
        (lp.len() == slots.len() && lp.iter().all(|l| l.len() == options.len())).then_some(lp).ok_or_else(|| BROKEN.to_string())
    }

    /// 엔진 프로세스의 최대 작업 집합 (MB, 측정용)
    #[allow(dead_code)]
    pub fn peak_mb(&mut self) -> Res<u64> {
        Ok(self.call(json!({ "op": "stats" }), &|| false)?["peak_mb"].as_u64().unwrap_or(0))
    }
}

// ------------------------------------------------------------------ 개념 관계 확인 (두 단계)
// 1) 모범 답안에서 핵심 관계를 뽑는다. 2) 내 답이 관계마다 맞음 · 빠짐 · 거꾸로 · 부정 · 틀림인지 다섯 낱말 가운데 고른다 (decide).
// 카드가 나와 답을 쓰는 동안(prepare) 1) 과 2) 의 앞부분을 미리 하고, 비교할 때는 내 답과 판정 자리만 읽는다

/// 관계 하나와 내 답의 판정. verdict: ok · missing · reversed · negated · wrong
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Relation {
    pub rel: String,
    pub verdict: String,
}

const VERDICTS: [&str; 5] = ["ok", "missing", "reversed", "negated", "wrong"];

/// 전체 판정 (규칙으로 낸다, 모델이 내지 않는다). missing: 관계를 하나도 말하지 않았다
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Correct,
    Partial,
    Missing,
    Wrong,
}

/// 하나라도 거꾸로 · 부정 · 틀림이면 Wrong, 모두 맞으면 Correct, 맞은 것과 빠진 것이 섞이면 Partial, 다 빠졌으면 Missing
pub fn verdict(rels: &[Relation]) -> Verdict {
    let any = |v: &[&str]| rels.iter().any(|r| v.contains(&r.verdict.as_str()));
    if any(&["reversed", "negated", "wrong"]) {
        Verdict::Wrong
    } else if rels.iter().all(|r| r.verdict == "ok") {
        Verdict::Correct
    } else if any(&["ok"]) {
        Verdict::Partial
    } else {
        Verdict::Missing
    }
}

/// 자리 하나의 판정: "ok" 의 로그 확률이 다른 낱말 가운데 가장 높은 것보다 OK_MARGIN 이상 낮지 않으면 ok, 아니면 그 낱말.
/// 작은 모델은 바꿔 말한 답도 빠짐 · 거꾸로로 보는 쪽으로 크게 기울어 있다(가장 높은 낱말을 그대로 고르면 정답 · 부분 23개 중 22개를 틀렸다고 짚었다).
/// 그 치우침을 값 하나로 덜어 낸다 (DECISIONS J7: 개발셋 41개로 고른 값, 모델마다 다르다)
pub fn decide(lp: &[f32]) -> usize {
    let (best, top) = (1..lp.len()).map(|i| (i, lp[i])).fold((0, f32::NEG_INFINITY), |m, x| if x.1 > m.1 { x } else { m });
    if lp[0] + OK_MARGIN >= top { 0 } else { best }
}

/// ok 를 남기는 여유 (자연로그). Qwen2.5-1.5B 개발셋에서 3.0~4.5 가 비슷하게 좋아 그 안쪽 값 (0.6B 도 4.5 근처, 1.7B 는 7.5)
const OK_MARGIN: f32 = 4.0;

/// 글이 길면 앞부분만 (지연이 입력 길이에 비례한다)
fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect::<String>().trim().to_string()
}

/// 모델 출력이 이어 쓸 첫머리 (JSON 문자열 배열로 붙잡는다)
const PREFIX: &str = "[\"";

/// ChatML 의 끝 (사용자 말 닫기 · 모델 차례). Qwen3 는 생각 모드를 끈 템플릿 (빈 think 블록)
fn tail(qwen3: bool) -> String {
    let think = if qwen3 { "<think>\n\n</think>\n\n" } else { "" };
    format!("<|im_end|>\n<|im_start|>assistant\n{think}")
}

fn extract_prompt(question: &str, model: &str, qwen3: bool) -> String {
    format!(
        "<|im_start|>system\nYou turn a model answer into grading points. Output only a JSON array of Korean strings.<|im_end|>\n<|im_start|>user\n\
         List the 1-4 key relations a grader must check in the model answer (cause → effect, increase/decrease, comparison, order, condition). \
         Write each one briefly in Korean using the model answer's words, like \"A → B\". Keep every step of a process.\n\
         Example: model answer \"운동하면 근육이 산소를 더 써서 심박수가 오른다\" → [\"운동 → 근육의 산소 사용 증가\", \"산소 사용 증가 → 심박수 상승\"]\n\n\
         질문: {}\n모범 답안: {}{}{PREFIX}",
        clip(question, 300),
        clip(model, 600),
        tail(qwen3)
    )
}

/// 판정 지시 · 질문 · 모범 답안 · 관계 (답을 쓰기 전에 알 수 있는 앞부분)
fn judge_prefix(question: &str, model: &str, rels: &[String]) -> String {
    let list: String = rels.iter().enumerate().map(|(i, r)| format!("{}. {r}\n", i + 1)).collect();
    format!(
        "<|im_start|>system\nYou grade a student's short answer strictly.<|im_end|>\n<|im_start|>user\n\
         Label how the student's answer states each numbered key relation of the model answer:\n\
         ok = same meaning (wording may differ); missing = not stated; reversed = cause and effect, the order, or the two compared things are swapped; \
         negated = denied, or the opposite direction (increase ↔ decrease); wrong = says something else that is incorrect.\n\
         Answer one line per relation, like \"1: label\".\n\n\
         질문: {}\n모범 답안: {}\n핵심 관계:\n{list}학생 답:\n",
        clip(question, 300),
        clip(model, 600)
    )
}

/// "1: ok\n2: missing" 꼴의 자리 · 다섯 낱말 (앞 빈칸 포함)
fn slots(n: usize) -> Vec<String> {
    (1..=n).map(|i| if i == 1 { "1:".to_string() } else { format!("\n{i}:") }).collect()
}

fn options() -> Vec<String> {
    VERDICTS.iter().map(|v| format!(" {v}")).collect()
}

/// 출력(첫머리 포함)의 첫 JSON 문자열 배열. 뒤에 붙은 말은 버린다
fn strings(text: &str) -> Option<Vec<String>> {
    let start = text.find('[')?;
    let v: Vec<String> = serde_json::Deserializer::from_str(&text[start..]).into_iter().next()?.ok()?;
    Some(v.into_iter().map(|s| s.trim().to_string()).collect())
}

/// 뽑은 관계 검사: 1~5개, 빈 것 없이 80자까지, 같은 것은 하나로. 어기면 없다 (결과 없음)
pub fn parse_relations(text: &str) -> Option<Vec<String>> {
    let mut v = strings(text)?;
    v.dedup();
    ((1..=5).contains(&v.len()) && v.iter().all(|r| !r.is_empty() && r.chars().count() <= 80)).then_some(v)
}

/// 글 묶음의 SHA-256 (캐시 열쇠)
fn key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    format!("{:x}", h.finalize())
}

/// 최근 64개만 두는 캐시 (앱을 닫을 때까지, DB 에 쓰지 않는다)
struct Cache<T>(Vec<(String, T)>);

impl<T: Clone> Cache<T> {
    fn get(&self, k: &str) -> Option<T> {
        self.0.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone())
    }
    fn put(&mut self, k: String, v: T) {
        if self.0.len() >= 64 {
            self.0.remove(0);
        }
        self.0.push((k, v));
    }
}

/// 모범 답안마다: 뽑은 관계 (질문 · 모범 답안의 해시)
static PREPARED: Mutex<Cache<Vec<String>>> = Mutex::new(Cache(vec![]));
/// 답마다: 판정 결과 (질문 · 모범 답안 · 내 답의 해시). 같은 답을 다시 확인하지 않는다
static RESULTS: Mutex<Cache<Vec<Relation>>> = Mutex::new(Cache(vec![]));

/// 1단계. 같은 모범 답안은 다시 하지 않는다. 관계 형식이 틀리면 없다
fn prepared(e: &mut Engine, question: &str, model: &str, stop: &dyn Fn() -> bool) -> Res<Option<Vec<String>>> {
    let k = key(&[question, model]);
    if let Some(p) = PREPARED.lock().map_err(err)?.get(&k) {
        return Ok(Some(p));
    }
    let text = e.generate("", &extract_prompt(question, model, e.qwen3()), 160, stop)?;
    let Some(rels) = parse_relations(&format!("{PREFIX}{text}")) else { return Ok(None) };
    PREPARED.lock().map_err(err)?.put(k, rels.clone());
    Ok(Some(rels))
}

/// 답을 쓰는 동안: 관계를 뽑고 판정의 앞부분을 읽어 둔다
pub fn warm(e: &mut Engine, question: &str, model: &str, stop: &dyn Fn() -> bool) -> Res<()> {
    if let Some(rels) = prepared(e, question, model, stop)? {
        e.prime(&judge_prefix(question, model, &rels), stop)?;
    }
    Ok(())
}

/// 판정과 그 근거 (측정용: 자리마다 다섯 낱말의 로그 확률)
pub struct Detail {
    pub relations: Vec<Relation>,
    #[allow(dead_code)] // 측정(eval83)에서 읽는다
    pub lp: Vec<Vec<f32>>,
}

pub fn check_detail(e: &mut Engine, question: &str, model: &str, answer: &str, stop: &dyn Fn() -> bool) -> Res<Option<Detail>> {
    let Some(rels) = prepared(e, question, model, stop)? else { return Ok(None) };
    let rest = format!("{}{}", clip(answer, 600), tail(e.qwen3()));
    let lp = e.choose(&judge_prefix(question, model, &rels), &rest, &slots(rels.len()), &options(), stop)?;
    let relations = rels.into_iter().zip(&lp).map(|(rel, l)| Relation { rel, verdict: VERDICTS[decide(l)].to_string() }).collect();
    Ok(Some(Detail { relations, lp }))
}

/// 같은 질문 · 모범 답안 · 답의 판정이 캐시에 있으면 (엔진 없이)
fn cached(question: &str, model: &str, answer: &str) -> Res<Option<Vec<Relation>>> {
    Ok(RESULTS.lock().map_err(err)?.get(&key(&[question, model, answer])))
}

/// 두 단계 모두. 같은 답은 캐시에서. 관계 형식이 틀리면 없다
pub fn check(e: &mut Engine, question: &str, model: &str, answer: &str, stop: &dyn Fn() -> bool) -> Res<Option<Vec<Relation>>> {
    if let Some(r) = cached(question, model, answer)? {
        return Ok(Some(r));
    }
    let r = check_detail(e, question, model, answer, stop)?.map(|d| d.relations);
    if let Some(r) = &r {
        RESULTS.lock().map_err(err)?.put(key(&[question, model, answer]), r.clone());
    }
    Ok(r)
}

/// 카드 하나에 걸어 둘 시간. 넘으면 엔진이 멈춘 것으로 본다
const TIMEOUT: Duration = Duration::from_secs(60);

/// 새 확인 차례. 앞서 돌던 확인은 멈춘다
pub fn begin() -> u64 {
    JOB.fetch_add(1, Ordering::SeqCst) + 1
}

/// 띄워 둔 엔진으로 f. 없으면 띄운다. f 가 멈춤 말고 다른 이유로 실패하면 엔진을 끝낸다 (다음에 새로 띄운다)
fn with_engine<T>(bin: &Path, data_dir: &Path, stop: &dyn Fn() -> bool, f: impl FnOnce(&mut Engine) -> Res<T>) -> Res<T> {
    let mut slot = ENGINE.lock().map_err(err)?;
    if slot.is_none() {
        *slot = Some(Engine::start(bin, &[], &model_dir(data_dir), stop)?);
    }
    let res = f(slot.as_mut().unwrap());
    if res.as_ref().is_err_and(|e| e != STOPPED) {
        *slot = None;
    }
    res
}

/// 엔진이 이 PC · 이 설치에서 쓸 수 있는가 (아니면 부르지 않는다)
fn usable(bin: Option<&Path>, data_dir: &Path) -> Res<PathBuf> {
    if !cpu_ok() {
        return Err("이 PC의 CPU가 빠른 계산(AVX2)을 지원하지 않아 개념 관계 확인을 쓸 수 없어요.".into());
    }
    let bin = bin.ok_or("이 설치에는 개념 관계 확인 엔진이 없어요.")?;
    if !installed(&model_dir(data_dir)) {
        return Err("개념 관계 확인 모델이 없어요.".into());
    }
    if OFF.load(Ordering::SeqCst) {
        return Err(OFF_MSG.into());
    }
    Ok(bin.to_path_buf())
}

/// 확인 하나 (막는 함수, 백그라운드 스레드에서). 엔진이 죽거나 · 답이 깨지거나 · 시간이 넘으면 한 번 다시 띄워 다시 하고,
/// 그래도 안 되면 이번 학습에서는 끈다 (OFF, 화면이 알린다). 사용자가 멈춘 것은 실패로 치지 않는다
pub fn run(bin: Option<&Path>, data_dir: &Path, job: u64, question: &str, model: &str, answer: &str) -> Res<Option<Vec<Relation>>> {
    let bin = usable(bin, data_dir)?;
    // 캐시는 엔진 잠금 앞에서 본다: 다른 카드의 미리 하기가 엔진을 쥐고 있어도 기다리지 않는다 (QA Q5 L1: 되돌리기로 돌아온 카드가 9.5초)
    if let Some(r) = cached(question, model, answer)? {
        return Ok(Some(r));
    }
    let started = Instant::now();
    let mut last = String::new();
    for _ in 0..2 {
        let deadline = Instant::now() + TIMEOUT;
        let stop = || JOB.load(Ordering::SeqCst) != job || Instant::now() > deadline;
        match with_engine(&bin, data_dir, &stop, |e| check(e, question, model, answer, &stop)) {
            Ok(r) => {
                LAST_MS.store(started.elapsed().as_millis() as u64, Ordering::SeqCst);
                return Ok(r);
            }
            Err(_) if JOB.load(Ordering::SeqCst) != job => return Err(STOPPED.into()),
            Err(e) => {
                // 시간이 넘었으면 엔진은 아직 일하는 중일 수 있다: 끝내고 새로 띄운다
                ENGINE.lock().map_err(err)?.take();
                last = e;
            }
        }
    }
    OFF.store(true, Ordering::SeqCst);
    Err(format!("{OFF_MSG} ({last})"))
}

/// 화면이 멈추라고 한 미리 하기 표(token)의 가장 큰 값. 표는 화면이 늘려 가며 준다 (그 카드를 떠나면 그 표까지 멈춘다)
static PREP_CANCELLED: AtomicU64 = AtomicU64::new(0);

/// 서술형 카드가 나오면: 엔진을 띄워 모델을 읽고 그 카드의 관계 · 판정 앞부분을 미리 한다 (답을 쓰는 동안).
/// 새 카드의 미리 하기가 오거나 화면이 그 표를 멈추면(카드가 바뀜 · 되돌리기 · 접어두기) 멈추고 엔진 잠금을 놓는다.
/// 실패는 여기서 알리지 않는다 (비교할 때 run 이 다시 띄우고 알린다)
pub fn prepare(bin: Option<&Path>, data_dir: &Path, question: &str, model: &str, token: u64) -> Res<()> {
    let bin = usable(bin, data_dir)?;
    let job = PREP.fetch_add(1, Ordering::SeqCst) + 1;
    let deadline = Instant::now() + TIMEOUT;
    let stop = || PREP.load(Ordering::SeqCst) != job || PREP_CANCELLED.load(Ordering::SeqCst) >= token || Instant::now() > deadline;
    if stop() {
        return Ok(()); // 시작하기 전에 이미 떠난 카드
    }
    with_engine(&bin, data_dir, &stop, |e| if stop() { Ok(()) } else { warm(e, question, model, &stop) })
}

/// 그 표의 미리 하기를 멈춘다 (화면이 카드를 떠날 때). 늦게 와도 그 뒤의 미리 하기(더 큰 표)는 멈추지 않는다
#[tauri::command]
pub fn llm_stop_prepare(token: u64) {
    PREP_CANCELLED.fetch_max(token, Ordering::SeqCst);
}

/// 돌던 확인 · 미리 하기를 멈춘다 (평가했거나 다음 카드)
#[tauri::command]
pub fn llm_stop() {
    JOB.fetch_add(1, Ordering::SeqCst);
    PREP.fetch_add(1, Ordering::SeqCst);
}

/// 학습 화면을 떠나면 엔진을 끝내 메모리를 돌려준다. 이번 학습에서 껐던 것도 푼다
#[tauri::command]
pub async fn llm_release() -> Res<()> {
    unload().await?;
    OFF.store(false, Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(v: &str) -> Relation {
        Relation { rel: "a → b".into(), verdict: v.into() }
    }

    #[test]
    fn overall_verdict_from_relations() {
        assert_eq!(verdict(&[rel("ok"), rel("ok")]), Verdict::Correct);
        assert_eq!(verdict(&[rel("ok"), rel("missing")]), Verdict::Partial);
        assert_eq!(verdict(&[rel("missing"), rel("missing")]), Verdict::Missing, "다 빠짐");
        for bad in ["reversed", "negated", "wrong"] {
            assert_eq!(verdict(&[rel("ok"), rel("ok"), rel(bad)]), Verdict::Wrong, "{bad} 하나면 오답");
        }
    }

    #[test]
    fn relations_are_validated() {
        let rels = format!("{PREFIX}수요 증가 → 가격 상승\", \"수요 증가 → 거래량 증가\"] 끝");
        assert_eq!(parse_relations(&rels).unwrap(), ["수요 증가 → 가격 상승", "수요 증가 → 거래량 증가"], "뒤에 붙은 말은 버린다");
        for bad in [format!("{PREFIX}\"]"), "[]".into(), format!("{PREFIX}{}\"]", "가".repeat(81)), format!("{PREFIX}a\""), "관계가 없어요".into()] {
            assert!(parse_relations(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn ok_wins_unless_another_label_is_far_ahead() {
        assert_eq!(decide(&[-0.5, -1.5, -2.0, -3.0, -3.0]), 0);
        assert_eq!(decide(&[-4.0, -0.5, -2.0, -3.0, -3.0]), 0, "빠짐이 3.5 앞서도 ok (여유 4)");
        assert_eq!(decide(&[-5.0, -3.0, -0.2, -3.0, -3.0]), 2, "거꾸로가 4.8 앞서면 거꾸로");
    }

    #[test]
    fn prompts_turn_qwen3_thinking_off_and_clip() {
        let p = extract_prompt("q", &"Z".repeat(2000), true);
        assert!(p.ends_with(&format!("<|im_start|>assistant\n<think>\n\n</think>\n\n{PREFIX}")));
        assert_eq!(p.chars().filter(|&c| c == 'Z').count(), 600);
        let j = judge_prefix("q", "m", &["a → b".into(), "c → d".into()]);
        assert!(j.ends_with("1. a → b\n2. c → d\n학생 답:\n"), "답 앞까지가 앞부분");
        assert!(!tail(false).contains("<think>"));
        assert_eq!(slots(3), ["1:", "\n2:", "\n3:"]);
    }

    #[test]
    fn caches_keep_the_last_64() {
        let mut c = Cache(vec![]);
        for i in 0..70 {
            c.put(key(&["q", &i.to_string()]), i);
        }
        assert_eq!((c.0.len(), c.get(&key(&["q", "69"])), c.get(&key(&["q", "0"]))), (64, Some(69), None));
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]), "경계가 다르면 다른 열쇠");
    }

    /// 엔진이 깨진 답을 내거나 죽으면: 오류로 돌려주고 엔진을 끝낸다 (다음 확인에 새로 띄운다). 진짜 프로세스로 (cmd.exe 가 가짜 엔진)
    #[cfg(windows)]
    #[test]
    fn broken_or_dead_engine_is_an_error_not_a_hang() {
        let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
        let t = Instant::now();
        let broken = Engine::start(cmd, &["/c", "echo not-json& ping -n 3 127.0.0.1 >nul"], Path::new("."), &|| false).err().unwrap();
        assert_eq!(broken, BROKEN);
        let dead = Engine::start(cmd, &["/c", "exit 3"], Path::new("."), &|| false).err().unwrap();
        assert_eq!(dead, DEAD);
        assert!(t.elapsed() < Duration::from_secs(10), "기다리다 멈추지 않는다");
    }

    /// 8.3 측정: 엔진(보조 실행 파일)을 실제로 띄워 앱과 같은 차례로 (카드가 나오면 미리 하기 → 비교 뒤 판정).
    /// `NEOANKI_EVAL83=<jsonl> NEOANKI_LLM_BIN=<neoanki-llm.exe> NEOANKI_LLM_DIR=<model.gguf · tokenizer.json 폴더> cargo test --lib eval83 -- --ignored --nocapture`
    /// (`NEOANKI_EVAL_LIMIT=n` 앞 n개, `NEOANKI_EVAL_DUMP=<파일>` 자리마다 로그 확률을 JSON 줄로 남긴다,
    /// `NEOANKI_EVAL_FROM_DUMP=<파일>` 엔진 없이 남긴 관계 · 로그 확률로 판정 규칙 · 가드만 다시 잰다).
    /// 규칙만(핵심어 = 모범 답안 제안 칩 6개, 항목에 "keywords" 가 있으면 그것) · 모델 판정 · 모델 + 가드(written::guard, 앱과 같음) · 합침(merge 후보, 가드 뒤)을 case 별로 낸다.
    /// 추천 3 Good = correct, 2 Hard = partial, 1 Again = wrong, 없음 = none 으로 견준다
    #[test]
    #[ignore]
    fn eval83() {
        use crate::written::{Keyword, compare, guard, suggest_from};
        // 합침 후보 (화면은 쓰지 않는다, J6): 관계가 거꾸로 · 부정 · 틀림이면 Again, 아니면 핵심어 추천 그대로
        let merge = |rules: Option<u8>, v: Option<Verdict>| if v == Some(Verdict::Wrong) { Some(1) } else { rules };
        use std::collections::BTreeMap;
        let path = std::env::var("NEOANKI_EVAL83").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/eval83-dev.jsonl").into());
        let items: Vec<Value> = std::fs::read_to_string(&path).unwrap().lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect();
        let items = &items[..std::env::var("NEOANKI_EVAL_LIMIT").ok().and_then(|n| n.parse().ok()).unwrap_or(items.len()).min(items.len())];
        let mut dump = std::env::var("NEOANKI_EVAL_DUMP").ok().map(|p| std::fs::File::create(p).unwrap());
        // 남긴 덤프 (id → 관계 · 로그 확률)
        let saved: BTreeMap<String, (Vec<String>, Vec<Vec<f32>>)> = std::env::var("NEOANKI_EVAL_FROM_DUMP")
            .map(|p| {
                std::fs::read_to_string(p).unwrap().lines().filter(|l| !l.trim().is_empty()).map(|l| {
                    let v: Value = serde_json::from_str(l).unwrap();
                    (v["id"].as_str().unwrap().to_string(), (serde_json::from_value(v["rels"].clone()).unwrap(), serde_json::from_value(v["lp"].clone()).unwrap()))
                }).collect()
            })
            .unwrap_or_default();
        let mut engine = match (std::env::var("NEOANKI_LLM_BIN"), std::env::var("NEOANKI_LLM_DIR")) {
            (Ok(bin), Ok(dir)) => {
                let t = Instant::now();
                let e = Engine::start(Path::new(&bin), &[], Path::new(&dir), &|| false).unwrap();
                println!("엔진 {bin} · 모델 {dir} ({}) 띄우고 불러오기 {} ms", e.arch, t.elapsed().as_millis());
                Some(e)
            }
            _ => None,
        };
        let label = |s: Option<u8>| match s {
            Some(3) => "correct",
            Some(2) => "partial",
            Some(_) => "wrong",
            None => "none",
        };
        let v_label = |v: Option<Verdict>| match v {
            Some(Verdict::Correct) => "correct",
            Some(Verdict::Partial) => "partial",
            Some(Verdict::Wrong | Verdict::Missing) => "wrong",
            None => "none",
        };
        // case → [개수, 규칙 맞음, 모델 맞음, 가드 맞음, 합침 맞음], 그리고 (방식, 정답, 예측) 혼동 행렬
        let mut by_case: BTreeMap<String, [usize; 5]> = BTreeMap::new();
        let mut confusion: BTreeMap<(&str, String, String), usize> = BTreeMap::new();
        // 정답 · 부분 수, 그중 "틀린 관계 있음"으로 짚은 수 (가드 전 · 후)
        let (mut judge_ms, mut prep_ms, mut flagged) = (vec![], vec![], [0usize; 3]);
        for it in items {
            let s = |k: &str| it[k].as_str().unwrap_or_default().to_string();
            let (case, gold, q, model, answer) = (s("case"), s("label"), s("question"), s("model"), s("answer"));
            let keywords: Vec<Keyword> = match it.get("keywords").and_then(|k| k.as_array()) {
                Some(k) => k.iter().filter_map(|w| w.as_str()).map(|w| Keyword { word: w.into(), synonyms: vec![], source: "user".into() }).collect(),
                None => suggest_from(&model, &[], 6).into_iter().map(|w| Keyword { word: w, synonyms: vec![], source: "suggested".into() }).collect(),
            };
            let rules = compare(&answer, &model, &keywords).suggest;
            let (rels, detail): (Option<Vec<Relation>>, String) = if let Some((rels, lp)) = saved.get(&s("id")) {
                let r: Vec<Relation> = rels.iter().zip(lp).map(|(rel, l)| Relation { rel: rel.clone(), verdict: VERDICTS[decide(l)].to_string() }).collect();
                (Some(r), "덤프에서 ".into())
            } else if let Some(e) = engine.as_mut() {
                let t = Instant::now();
                warm(e, &q, &model, &|| false).unwrap();
                let prep = t.elapsed().as_millis();
                let t = Instant::now();
                let d = check_detail(e, &q, &model, &answer, &|| false).unwrap();
                let judge = t.elapsed().as_millis();
                prep_ms.push(prep);
                judge_ms.push(judge);
                let g = e.log.last().copied().unwrap_or_default();
                if let (Some(f), Some(d)) = (dump.as_mut(), d.as_ref()) {
                    let rels: Vec<&str> = d.relations.iter().map(|r| r.rel.as_str()).collect();
                    writeln!(f, "{}", json!({ "id": s("id"), "case": case, "label": gold, "rules": label(rules), "rels": rels, "lp": d.lp })).unwrap();
                }
                (d.map(|d| d.relations), format!("미리 {prep}ms · 비교 뒤 {judge}ms(입력 {}토큰) ", g.prompt))
            } else {
                (None, String::new())
            };
            let concept = rels.as_ref().map(|r| verdict(r));
            let guarded_rels = rels.map(|mut r| {
                guard(&mut r, &keywords, &answer);
                r
            });
            let guarded = guarded_rels.as_ref().map(|r| verdict(r));
            let detail = detail + &guarded_rels.as_ref().map(|r| r.iter().map(|r| format!("{}={}", r.rel, r.verdict)).collect::<Vec<_>>().join(" | ")).unwrap_or("형식 틀림".into());
            if gold != "wrong" {
                flagged[0] += 1;
                flagged[1] += (concept == Some(Verdict::Wrong)) as usize;
                flagged[2] += (guarded == Some(Verdict::Wrong)) as usize;
            }
            let merged = merge(rules, guarded);
            let e = by_case.entry(case.clone()).or_default();
            e[0] += 1;
            e[1] += (label(rules) == gold) as usize;
            e[2] += (v_label(concept) == gold) as usize;
            e[3] += (v_label(guarded) == gold) as usize;
            e[4] += (label(merged) == gold) as usize;
            *confusion.entry(("규칙", gold.clone(), label(rules).into())).or_default() += 1;
            *confusion.entry(("모델", gold.clone(), v_label(concept).into())).or_default() += 1;
            *confusion.entry(("가드", gold.clone(), v_label(guarded).into())).or_default() += 1;
            *confusion.entry(("합침", gold.clone(), label(merged).into())).or_default() += 1;
            let raw = |v: Option<Verdict>| v.map_or("none".into(), |v| format!("{v:?}").to_lowercase());
            println!("{} {case:10} 정답 {gold:7} 규칙 {:7} 모델 {:7} 가드 {:7} 합침 {:7} · {detail}", s("id"), label(rules), raw(concept), raw(guarded), label(merged));
        }
        println!("\ncase        개수  규칙  모델  가드  합침");
        let mut tot = [0; 5];
        for (case, v) in &by_case {
            println!("{case:10} {:>5} {:>5} {:>5} {:>5} {:>5}", v[0], v[1], v[2], v[3], v[4]);
            (0..5).for_each(|i| tot[i] += v[i]);
        }
        let pct = |x: usize| x as f64 * 100.0 / tot[0] as f64;
        println!("정확도      {:>5} {:>4.1}% {:>4.1}% {:>4.1}% {:>4.1}%", tot[0], pct(tot[1]), pct(tot[2]), pct(tot[3]), pct(tot[4]));
        println!("정답 · 부분을 \"틀린 관계 있음\"으로 짚음: 가드 전 {}/{} · 가드 뒤 {}/{}", flagged[1], flagged[0], flagged[2], flagged[0]);
        println!("\n혼동 행렬 (방식 · 정답 → 예측: 수)");
        for ((how, gold, pred), n) in &confusion {
            println!("{how} {gold:7} → {pred:7}: {n}");
        }
        for (name, t) in [("비교 뒤 판정(카드당)", &mut judge_ms), ("미리 하기(카드가 나와 답을 쓰는 동안)", &mut prep_ms)] {
            if !t.is_empty() {
                t.sort();
                println!("{name} ms: 평균 {} · 중앙 {} · 최대 {}", t.iter().sum::<u128>() / t.len() as u128, t[t.len() / 2], t[t.len() - 1]);
            }
        }
        if let Some(e) = engine.as_mut() {
            println!("엔진 최대 메모리(작업 집합) {} MB", e.peak_mb().unwrap());
        }
    }
}
