//! 8.3 서술형 2단계: 로컬 소형 언어 모델로 개념 관계 확인 (DECISIONS J 2단계).
//! 모델(GGUF)은 설치 파일에 넣지 않는다. 사용자가 크기 · 남은 공간 · 라이선스 · 출처를 보고 [내려받기]를 누를 때만
//! 고정 리비전에서 받고 파일마다 크기와 SHA-256 을 확인한다 (embed.rs 와 같은 길). 받은 뒤에는 인터넷 없이 CPU 에서 돌고, 답을 밖으로 보내지 않는다.
//! 모델이 없거나 받다가 실패해도 핵심어 검사와 평가는 그대로다. 결과는 비교 화면에 참고로 보인다 (추천에 합치는 규칙 written.rs merge 는 지금 끔).
//! 추론 경계: Llm::load(폴더) · prime(앞부분) · generate(앞부분, 나머지, 최대 토큰, 멈춤) · choose(앞부분, 나머지, 자리, 선택지, 멈춤).
//! 글만 주고받으므로 나중에 AVX2 로 빌드한 별도 실행 파일(stdin/stdout JSON)로 옮길 수 있다 (DECISIONS J)

use crate::commands::AppState;
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::models::{quantized_qwen2, quantized_qwen3};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tokenizers::Tokenizer;

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
const DIR: &str = "qwen3-0.6b-q8";
/// Qwen3-0.6B Q8_0 (Qwen 팀 공식 GGUF) · tokenizer.json (원본 저장소). 리비전을 고정했다. 바꾸면 크기와 해시를 함께 다시 적는다.
/// 고른 까닭 (DECISIONS J): 개발셋에서 1.7B · 1.5B 와 판별력이 같게 낮고, 가장 작고(0.65GB) 지금 빌드(AVX2 없음)에서 가장 빠르다
const FILES: [File; 2] = [
    File {
        name: "model.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/23749fefcc72300e3a2ad315e1317431b06b590a/Qwen3-0.6B-Q8_0.gguf",
        size: 639_446_688,
        sha: "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031",
    },
    File {
        name: "tokenizer.json",
        url: "https://huggingface.co/Qwen/Qwen3-0.6B/resolve/c1899de289a04d12100db370d81485cdf75e47ca/tokenizer.json",
        size: 11_422_654,
        sha: "aeb13307a71acd8fe81861d94ad54ab689df773318809eed3cbe794b4492dae4",
    },
];
/// 받기 확인 창에 그대로 보인다
const NAME: &str = "Qwen3-0.6B (8비트)";
const LICENSE: &str = "Apache-2.0";
const SOURCE: &str = "Hugging Face · Qwen/Qwen3-0.6B-GGUF (Qwen 팀 공식)";
/// 받은 뒤에도 남겨 둘 여유 (임시 파일 · 다른 기능)
const MARGIN: u64 = 200_000_000;

static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// 확인 차례 번호. 새 확인 · 멈추기 · 지우기가 올리면 돌던 생성이 다음 토큰에서 멈춘다
static JOB: AtomicU64 = AtomicU64::new(0);
static LOADED: Mutex<Option<Llm>> = Mutex::new(None);

fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join(DIR)
}

/// 받은 파일이 모두 있는가. 해시는 받을 때 확인했으므로 여기서는 크기만 본다
fn installed(dir: &Path) -> bool {
    FILES.iter().all(|f| dir.join(f.name).metadata().is_ok_and(|m| m.len() == f.size))
}

/// 개념 관계 확인을 할 수 있는가 (모델을 받았다)
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
    license: &'static str,
    source: &'static str,
    /// CPU 가 AVX2 를 지원하는가 (실행할 때 확인) · 이 빌드가 AVX2 로 컴파일됐는가. candle 의 양자화 계산은 둘 다일 때만 빠르다
    avx2_cpu: bool,
    avx2_build: bool,
    /// 이 기기에서 잰 토큰당 시간 (마지막 생성, ms). 아직 안 돌렸으면 없다
    ms_per_token: Option<u64>,
}

/// 마지막 생성의 토큰당 시간 (ms). 0 = 아직 모름
static MS_PER_TOKEN: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "x86_64")]
fn avx2_cpu() -> bool {
    std::is_x86_feature_detected!("avx2")
}

#[cfg(not(target_arch = "x86_64"))]
fn avx2_cpu() -> bool {
    false
}

#[tauri::command]
pub fn llm_status(state: State<AppState>) -> LlmStatus {
    let dir = model_dir(&state.data_dir);
    let bytes = FILES.iter().map(|f| f.size).sum();
    LlmStatus {
        ready: installed(&dir),
        downloading: DOWNLOADING.load(Ordering::SeqCst),
        bytes,
        free: free_bytes(&dir),
        need: bytes + MARGIN,
        name: NAME,
        license: LICENSE,
        source: SOURCE,
        avx2_cpu: avx2_cpu(),
        avx2_build: cfg!(target_feature = "avx2"),
        ms_per_token: Some(MS_PER_TOKEN.load(Ordering::SeqCst)).filter(|&m| m > 0),
    }
}

#[derive(Serialize, Clone)]
struct DownloadProgress {
    done: u64,
    total: u64,
}

/// [내려받기]. "llm-progress" 이벤트로 진행을 알린다. 공간이 모자라면 시작하지 않고, 멈추거나 실패하면 받던 파일만 지운다
#[tauri::command]
pub async fn llm_download(app: AppHandle) -> Res<()> {
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

/// 돌던 확인을 멈추고 읽어 둔 모델을 놓는다. 불러오는 중이면 끝날 때까지 기다리므로 백그라운드에서
async fn unload() -> Res<()> {
    llm_stop();
    tauri::async_runtime::spawn_blocking(|| LOADED.lock().map(|mut m| *m = None).map_err(err)).await.map_err(err)?
}

/// 모델 지우기. 돌던 확인은 멈춘다
#[tauri::command]
pub async fn llm_delete(app: AppHandle) -> Res<()> {
    if DOWNLOADING.load(Ordering::SeqCst) {
        return Err("내려받기가 끝난 뒤에 지울 수 있어요.".into());
    }
    unload().await?;
    let dir = model_dir(&app.state::<AppState>().data_dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|_| "모델 파일을 지우지 못했어요. 앱을 다시 연 뒤에 해 주세요.".to_string())?;
    }
    Ok(())
}

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

/// 받을 주소. 디버그 빌드는 NEOANKI_LLM_URL(파일 이름 앞부분)로 바꿀 수 있다 (캡처 · 오프라인 시험용, 실제 모델은 다시 받지 않는다)
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
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into();
    for f in &FILES {
        let path = dir.join(f.name);
        if path.metadata().is_ok_and(|m| m.len() == f.size) && sha256_file(&path)? == f.sha {
            done += f.size;
            continue;
        }
        let part = dir.join(format!("{}.part", f.name));
        let res = fetch(app, &agent, f, &part, done, total);
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

fn fetch(app: &AppHandle, agent: &ureq::Agent, f: &File, part: &Path, base: u64, total: u64) -> Res<()> {
    fn offline<E: std::fmt::Display>(e: E) -> String {
        format!("모델을 내려받지 못했어요. 인터넷 연결을 확인하고 다시 시도해 주세요. ({e})")
    }
    let mut resp = agent.get(&url_of(f)).call().map_err(offline)?;
    // ureq 의 limit 는 딱 그 크기에서도 "더 있는지" 보다가 오류를 낸다. 한 바이트 넉넉히 두고 크기는 아래에서 맞춘다
    let mut reader = resp.body_mut().with_config().limit(f.size + 1).reader();
    let mut file = std::fs::File::create(part).map_err(err)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let (mut got, mut last) = (0u64, 0u64);
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("내려받기를 멈췄어요.".into());
        }
        let n = reader.read(&mut buf).map_err(offline)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(err)?;
        got += n as u64;
        if got - last >= 1 << 22 {
            last = got;
            let _ = app.emit("llm-progress", DownloadProgress { done: base + got, total });
        }
    }
    file.flush().map_err(err)?;
    if got != f.size || format!("{:x}", h.finalize()) != f.sha {
        return Err("받은 모델 파일이 원본과 달라요. 다시 시도해 주세요.".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ 불러오기 · 생성

enum Weights {
    Qwen3(quantized_qwen3::ModelWeights),
    Qwen2(quantized_qwen2::ModelWeights),
}

/// 생성 한 번의 기록 (측정용): 입력 토큰 · 출력 토큰 · 입력 처리 · 출력 시간
#[derive(Clone, Copy, Debug, Default)]
pub struct Gen {
    pub prompt: usize,
    pub out: usize,
    pub prefill: Duration,
    pub decode: Duration,
}

pub struct Llm {
    w: Weights,
    tok: Tokenizer,
    eos: Vec<u32>,
    /// 모델 안(KV 캐시)에 읽혀 있는 토큰. 앞이 같은 입력은 그 뒤만 읽는다 (답을 쓰는 동안 앞부분을 미리 읽어 둔다)
    seen: Vec<u32>,
    pub log: Vec<Gen>,
}

const STOPPED: &str = "확인을 멈췄어요.";
/// 입력을 이만큼씩 읽고 사이사이 멈춤을 본다 (candle CPU 양자화 행렬곱은 토큰마다 따로 돌아 묶어도 빨라지지 않는다)
const CHUNK: usize = 16;

impl Llm {
    /// GGUF(qwen2 · qwen3) 와 tokenizer.json 이 든 폴더에서 읽는다
    pub fn load(dir: &Path) -> Res<Llm> {
        fn broken<E: std::fmt::Display>(e: E) -> String {
            format!("모델 파일을 읽지 못했어요. 모델을 지우고 다시 내려받아 주세요. ({e})")
        }
        let mut f = std::fs::File::open(dir.join("model.gguf")).map_err(broken)?;
        let mut ct = gguf_file::Content::read(&mut f).map_err(broken)?;
        let arch = ct.metadata.get("general.architecture").and_then(|v| v.to_string().ok()).cloned().unwrap_or_default();
        // candle 의 qwen3 는 general.dtype 이 없으면 RoPE 표를 f16 으로 만든다 (위치 수백부터 각도가 0.25 넘게 어긋난다). 계산은 f32 로
        ct.metadata.insert("general.dtype".into(), gguf_file::Value::U32(0));
        let dev = Device::Cpu;
        let w = match arch.as_str() {
            "qwen3" => Weights::Qwen3(quantized_qwen3::ModelWeights::from_gguf(ct, &mut f, &dev).map_err(broken)?),
            "qwen2" => Weights::Qwen2(quantized_qwen2::ModelWeights::from_gguf(ct, &mut f, &dev).map_err(broken)?),
            a => return Err(broken(format!("모르는 모델 구조 {a}"))),
        };
        let tok = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(broken)?;
        let eos = ["<|im_end|>", "<|endoftext|>"].iter().filter_map(|t| tok.token_to_id(t)).collect();
        Ok(Llm { w, tok, eos, seen: vec![], log: vec![] })
    }

    fn qwen3(&self) -> bool {
        matches!(self.w, Weights::Qwen3(_))
    }

    fn encode(&self, text: &str) -> Res<Vec<u32>> {
        Ok(self.tok.encode(text, false).map_err(err)?.get_ids().to_vec())
    }

    fn reset(&mut self) {
        match &mut self.w {
            Weights::Qwen3(m) => m.clear_kv_cache(),
            Weights::Qwen2(m) => m.clear_kv_cache(),
        }
        self.seen.clear();
    }

    /// 토큰을 이어 읽고 마지막 자리의 로짓. 실패하면 처음부터 (KV 캐시가 어디까지 찼는지 모른다)
    fn step(&mut self, ids: &[u32]) -> Res<Tensor> {
        let pos = self.seen.len();
        let x = Tensor::new(ids, &Device::Cpu).and_then(|t| t.unsqueeze(0)).map_err(err)?;
        let res = match &mut self.w {
            Weights::Qwen3(m) => m.forward(&x, pos),
            Weights::Qwen2(m) => m.forward(&x, pos),
        };
        match res {
            Ok(l) => {
                self.seen.extend_from_slice(ids);
                Ok(l)
            }
            Err(e) => {
                self.reset();
                Err(err(e))
            }
        }
    }

    /// 읽힌 것 뒤에 이어 읽는다. 마지막 로짓 (ids 가 비면 없다)
    fn push(&mut self, ids: &[u32], stop: &dyn Fn() -> bool) -> Res<Option<Tensor>> {
        let mut last = None;
        for chunk in ids.chunks(CHUNK) {
            if stop() {
                return Err(STOPPED.into());
            }
            last = Some(self.step(chunk)?);
        }
        Ok(last)
    }

    /// ids 를 모델에 읽힌다. 이미 읽혀 있는 앞부분은 건너뛴다. 새로 읽은 것이 있으면 마지막 로짓
    fn feed(&mut self, ids: &[u32], stop: &dyn Fn() -> bool) -> Res<Option<Tensor>> {
        if self.seen.is_empty() || !ids.starts_with(&self.seen) {
            self.reset();
        }
        let n = self.seen.len();
        self.push(&ids[n..], stop)
    }

    /// 앞이 같아 건너뛸 토큰 수
    fn kept(&self, ids: &[u32]) -> usize {
        if !self.seen.is_empty() && ids.starts_with(&self.seen) { self.seen.len() } else { 0 }
    }

    fn record(&mut self, g: Gen) {
        let tokens = (g.prompt + g.out).max(1) as u128;
        MS_PER_TOKEN.store(((g.prefill + g.decode).as_millis() / tokens) as u64, Ordering::SeqCst);
        self.log.push(g);
    }

    /// prefix + rest 를 읽은 뒤 slots 를 차례로 이어 읽으며, 자리마다 options 가운데 첫 토큰의 로짓이 가장 높은 것을 고르고 그 글을 이어 붙인다.
    /// 출력 형식이 틀릴 수 없다 (글을 쓰지 않고 고르기만 한다). 고른 번호들
    pub fn choose(&mut self, prefix: &str, rest: &str, slots: &[String], options: &[&str], stop: &dyn Fn() -> bool) -> Res<Vec<usize>> {
        let t = Instant::now();
        let mut ids = self.encode(prefix)?;
        ids.extend(self.encode(rest)?);
        let mut read = ids.len() - self.kept(&ids);
        let mut last = self.feed(&ids, stop)?;
        let firsts = options.iter().map(|o| self.encode(o)?.first().copied().ok_or_else(|| "빈 선택지".to_string())).collect::<Res<Vec<u32>>>()?;
        if (1..firsts.len()).any(|i| firsts[..i].contains(&firsts[i])) {
            return Err("선택지의 첫 토큰이 겹쳐요.".into());
        }
        let mut picked = vec![];
        for (i, slot) in slots.iter().enumerate() {
            // 빈 자리는 방금 읽은 것 바로 다음
            if !slot.is_empty() {
                let s = self.encode(slot)?;
                read += s.len();
                last = self.push(&s, stop)?;
            }
            let logits = last.take().ok_or("읽을 글이 없어요.")?.squeeze(0).and_then(|l| l.to_vec1::<f32>()).map_err(err)?;
            let best = (0..options.len()).max_by(|&a, &b| logits[firsts[a] as usize].total_cmp(&logits[firsts[b] as usize])).unwrap_or(0);
            picked.push(best);
            if i + 1 < slots.len() {
                let o = self.encode(options[best])?;
                read += o.len();
                last = self.push(&o, stop)?;
            }
        }
        self.record(Gen { prompt: read, out: slots.len(), prefill: t.elapsed(), decode: Duration::ZERO });
        Ok(picked)
    }

    /// 앞부분을 미리 읽어 둔다. 같은 앞부분으로 generate 하면 그 뒤만 읽는다
    pub fn prime(&mut self, prefix: &str, stop: &dyn Fn() -> bool) -> Res<()> {
        let ids = self.encode(prefix)?;
        self.feed(&ids, stop).map(|_| ())
    }

    /// prefix + rest 다음을 탐욕 디코딩 (같은 입력에 같은 결과). rest 는 비지 않아야 한다. stop() 이 참이면 멈춘다
    pub fn generate(&mut self, prefix: &str, rest: &str, max: usize, stop: &dyn Fn() -> bool) -> Res<String> {
        let mut ids = self.encode(prefix)?;
        ids.extend(self.encode(rest)?);
        let t = Instant::now();
        let kept = self.kept(&ids);
        let mut logits = self.feed(&ids, stop)?.ok_or("읽을 글이 없어요.")?;
        let mut g = Gen { prompt: ids.len() - kept, prefill: t.elapsed(), ..Default::default() };
        let t = Instant::now();
        let mut out = vec![];
        for _ in 0..max {
            let next = logits.squeeze(0).and_then(|l| l.argmax(0)).and_then(|t| t.to_scalar::<u32>()).map_err(err)?;
            if self.eos.contains(&next) {
                break;
            }
            out.push(next);
            if stop() {
                return Err(STOPPED.into());
            }
            logits = self.step(&[next])?;
        }
        g.out = out.len();
        g.decode = t.elapsed();
        self.record(g);
        self.tok.decode(&out, true).map_err(err)
    }
}

// ------------------------------------------------------------------ 개념 관계 확인 (두 단계)
// 1) 모범 답안에서 핵심 관계를 뽑는다. 2) 내 답이 관계마다 맞음 · 빠짐 · 거꾸로 · 부정 · 틀림인지 판정한다.
// 카드가 나와 답을 쓰는 동안(prepare) 1) 과 2) 의 앞부분(지시 · 질문 · 모범 답안 · 관계)을 미리 읽어 두고,
// 비교할 때는 내 답과 판정 낱말 몇 개만 읽고 쓴다

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

/// 출력(첫머리 포함)의 첫 JSON 문자열 배열. 뒤에 붙은 말은 버린다
fn strings(text: &str) -> Option<Vec<String>> {
    let start = text.find('[')?;
    let v: Vec<String> = serde_json::Deserializer::from_str(&text[start..]).into_iter().next()?.ok()?;
    Some(v.into_iter().map(|s| s.trim().to_string()).collect())
}

/// 뽑은 관계 검사: 1~5개, 빈 것 없이 80자까지, 같은 것은 하나로. 어기면 없다 (기존 추천 그대로)
pub fn parse_relations(text: &str) -> Option<Vec<String>> {
    let mut v = strings(text)?;
    v.dedup();
    ((1..=5).contains(&v.len()) && v.iter().all(|r| !r.is_empty() && r.chars().count() <= 80)).then_some(v)
}

pub struct Check {
    pub relations: Vec<Relation>,
    pub verdict: Verdict,
}

/// 같은 모범 답안의 관계 (앱을 닫을 때까지, 최근 64개)
static RELS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(vec![]);

/// 1단계. 형식이 틀리면 없다
pub fn extract(llm: &mut Llm, question: &str, model: &str, stop: &dyn Fn() -> bool) -> Res<Option<Vec<String>>> {
    let key = format!("{question}\n{model}");
    if let Some((_, r)) = RELS.lock().map_err(err)?.iter().find(|(k, _)| *k == key) {
        return Ok(Some(r.clone()));
    }
    let text = llm.generate("", &extract_prompt(question, model, llm.qwen3()), 160, stop)?;
    let rels = parse_relations(&format!("{PREFIX}{text}"));
    if let Some(r) = &rels {
        let mut cache = RELS.lock().map_err(err)?;
        if cache.len() >= 64 {
            cache.remove(0);
        }
        cache.push((key, r.clone()));
    }
    Ok(rels)
}

/// 답을 쓰는 동안: 관계를 뽑고 판정의 앞부분을 읽어 둔다
pub fn warm(llm: &mut Llm, question: &str, model: &str, stop: &dyn Fn() -> bool) -> Res<()> {
    if let Some(rels) = extract(llm, question, model, stop)? {
        llm.prime(&judge_prefix(question, model, &rels), stop)?;
    }
    Ok(())
}

/// 두 단계 모두. 어느 단계든 형식이 틀리면 없다
pub fn check(llm: &mut Llm, question: &str, model: &str, answer: &str, stop: &dyn Fn() -> bool) -> Res<Option<Check>> {
    let Some(rels) = extract(llm, question, model, stop)? else { return Ok(None) };
    let rest = format!("{}{}", clip(answer, 600), tail(llm.qwen3()));
    // "1: ok\n2: missing" 꼴로 자리마다 다섯 낱말 가운데 고르게 한다 (작은 모델이 낱말 목록을 그대로 베끼던 것을 막는다)
    let slots: Vec<String> = (1..=rels.len()).map(|i| if i == 1 { "1:".to_string() } else { format!("\n{i}:") }).collect();
    let options = VERDICTS.map(|v| format!(" {v}"));
    let picked = llm.choose(&judge_prefix(question, model, &rels), &rest, &slots, &options.each_ref().map(String::as_str), stop)?;
    let relations: Vec<Relation> = rels.into_iter().zip(picked).map(|(rel, i)| Relation { rel, verdict: VERDICTS[i].to_string() }).collect();
    Ok(Some(Check { verdict: verdict(&relations), relations }))
}

/// 카드 하나에 걸어 둘 시간. 넘으면 멈추고 기존 추천 그대로
const TIMEOUT: Duration = Duration::from_secs(60);

/// 새 확인 차례. 앞서 돌던 확인은 멈춘다
pub fn begin() -> u64 {
    JOB.fetch_add(1, Ordering::SeqCst) + 1
}

/// 미리 뽑기 차례 (확인과 따로 센다: 비교를 눌러도 같은 카드의 뽑기는 이어서 쓴다)
static PREP: AtomicU64 = AtomicU64::new(0);

/// 읽어 둔 모델로 f. 처음이면 읽는다 (막는 함수, 백그라운드 스레드에서). [llm_release] 까지 둔다
fn with_model<T>(data_dir: &Path, f: impl FnOnce(&mut Llm) -> Res<T>) -> Res<T> {
    let dir = model_dir(data_dir);
    if !installed(&dir) {
        return Err("개념 관계 확인 모델이 없어요.".into());
    }
    let mut slot = LOADED.lock().map_err(err)?;
    if slot.is_none() {
        *slot = Some(Llm::load(&dir)?);
    }
    f(slot.as_mut().unwrap())
}

/// 확인 하나. 시간이 넘거나 멈추면 오류
pub fn run(data_dir: &Path, job: u64, question: &str, model: &str, answer: &str) -> Res<Option<Check>> {
    let deadline = Instant::now() + TIMEOUT;
    let stop = || JOB.load(Ordering::SeqCst) != job || Instant::now() > deadline;
    let res = with_model(data_dir, |m| if stop() { Err(STOPPED.into()) } else { check(m, question, model, answer, &stop) });
    if res.is_err() && Instant::now() > deadline {
        return Err("시간이 오래 걸려 멈췄어요.".into());
    }
    res
}

/// 서술형 카드가 나오면: 모델을 읽어 두고 그 카드의 관계를 미리 뽑는다 (답을 쓰는 동안). 새 카드가 오면 앞의 것은 멈춘다
pub fn prepare(data_dir: &Path, question: &str, model: &str) -> Res<()> {
    let job = PREP.fetch_add(1, Ordering::SeqCst) + 1;
    let deadline = Instant::now() + TIMEOUT;
    let stop = || PREP.load(Ordering::SeqCst) != job || Instant::now() > deadline;
    with_model(data_dir, |m| if stop() { Ok(()) } else { warm(m, question, model, &stop) })
}

/// 돌던 확인 · 미리 뽑기를 멈춘다 (평가했거나 다음 카드)
#[tauri::command]
pub fn llm_stop() {
    JOB.fetch_add(1, Ordering::SeqCst);
    PREP.fetch_add(1, Ordering::SeqCst);
}

/// 학습 화면을 떠나면 메모리를 돌려준다 (모델 크기만큼 차지한다)
#[tauri::command]
pub async fn llm_release() -> Res<()> {
    unload().await
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
    fn output_is_validated() {
        let rels = format!("{PREFIX}수요 증가 → 가격 상승\", \"수요 증가 → 거래량 증가\"] 끝");
        assert_eq!(parse_relations(&rels).unwrap(), ["수요 증가 → 가격 상승", "수요 증가 → 거래량 증가"], "뒤에 붙은 말은 버린다");
        for bad in [format!("{PREFIX}\"]"), "[]".into(), format!("{PREFIX}{}\"]", "가".repeat(81)), format!("{PREFIX}a\""), "관계가 없어요".into()] {
            assert!(parse_relations(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn prompts_turn_qwen3_thinking_off_and_clip() {
        let p = extract_prompt("q", &"Z".repeat(2000), true);
        assert!(p.ends_with(&format!("<|im_start|>assistant\n<think>\n\n</think>\n\n{PREFIX}")));
        assert_eq!(p.chars().filter(|&c| c == 'Z').count(), 600);
        let j = judge_prefix("q", "m", &["a → b".into(), "c → d".into()]);
        assert!(j.ends_with("1. a → b\n2. c → d\n학생 답:\n"), "답 앞까지가 앞부분");
        assert!(!tail(false).contains("<think>"));
    }

    /// 8.3 측정: `NEOANKI_EVAL83=<jsonl> NEOANKI_LLM_DIR=<model.gguf · tokenizer.json 폴더> cargo test --lib eval83 -- --ignored --nocapture`.
    /// 규칙만(핵심어 = 모범 답안 제안 칩 6개, 항목에 "keywords" 가 있으면 그것) · 모델 판정만 · 합친 추천(written::merge)을 case 별로 낸다.
    /// 추천 3 Good = correct, 2 Hard = partial, 1 Again = wrong, 없음 = none 으로 견준다. 지연은 관계 뽑기(같은 모범 답안은 한 번) · 판정을 따로
    #[test]
    #[ignore]
    fn eval83() {
        use crate::written::{compare, merge, suggest_from, Keyword};
        use std::collections::BTreeMap;
        let path = std::env::var("NEOANKI_EVAL83").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/eval83-dev.jsonl").into());
        let items: Vec<serde_json::Value> = std::fs::read_to_string(&path).unwrap().lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect();
        // NEOANKI_EVAL_LIMIT=n: 앞 n개만 (속도만 볼 때)
        let items = &items[..std::env::var("NEOANKI_EVAL_LIMIT").ok().and_then(|n| n.parse().ok()).unwrap_or(items.len()).min(items.len())];
        let mut llm = std::env::var("NEOANKI_LLM_DIR").ok().map(|d| {
            let t = Instant::now();
            let m = Llm::load(Path::new(&d)).unwrap();
            println!("모델 {d} 불러오기 {} ms", t.elapsed().as_millis());
            m
        });
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
        // case → [개수, 규칙 맞음, 모델 맞음, 합침 맞음], 그리고 (방식, 정답, 예측) 혼동 행렬
        let mut by_case: BTreeMap<String, [usize; 4]> = BTreeMap::new();
        let mut confusion: BTreeMap<(&str, String, String), usize> = BTreeMap::new();
        let (mut judge_ms, mut prep_ms) = (vec![], vec![]);
        for it in items {
            let s = |k: &str| it[k].as_str().unwrap_or_default().to_string();
            let (case, gold, q, model, answer) = (s("case"), s("label"), s("question"), s("model"), s("answer"));
            let keywords: Vec<Keyword> = match it.get("keywords").and_then(|k| k.as_array()) {
                Some(k) => k.iter().filter_map(|w| w.as_str()).map(|w| Keyword { word: w.into(), synonyms: vec![], source: "user".into() }).collect(),
                None => suggest_from(&model, &[], 6).into_iter().map(|w| Keyword { word: w, synonyms: vec![], source: "suggested".into() }).collect(),
            };
            let rules = compare(&answer, &model, &keywords).suggest;
            let (concept, detail) = match llm.as_mut() {
                Some(m) => {
                    // 앱과 같은 차례: 카드가 나오면 미리(관계 뽑기 · 판정 앞부분), 비교할 때 판정
                    let t = Instant::now();
                    warm(m, &q, &model, &|| false).unwrap();
                    let prep = t.elapsed().as_millis();
                    m.log.clear();
                    let t = Instant::now();
                    let c = check(m, &q, &model, &answer, &|| false).unwrap();
                    let judge = t.elapsed().as_millis();
                    prep_ms.push(prep);
                    judge_ms.push(judge);
                    let g = m.log.last().copied().unwrap_or_default();
                    let mut d = format!("미리 {prep}ms · 비교 뒤 {judge}ms(입력 {}토큰 {}ms, 출력 {}토큰) ", g.prompt, g.prefill.as_millis(), g.out);
                    d += &c.as_ref().map(|c| c.relations.iter().map(|r| format!("{}={}", r.rel, r.verdict)).collect::<Vec<_>>().join(" | ")).unwrap_or("형식 틀림".into());
                    (c.map(|c| c.verdict), d)
                }
                None => (None, String::new()),
            };
            let merged = merge(rules, concept);
            let e = by_case.entry(case.clone()).or_default();
            e[0] += 1;
            e[1] += (label(rules) == gold) as usize;
            e[2] += (v_label(concept) == gold) as usize;
            e[3] += (label(merged) == gold) as usize;
            *confusion.entry(("규칙", gold.clone(), label(rules).into())).or_default() += 1;
            *confusion.entry(("모델", gold.clone(), v_label(concept).into())).or_default() += 1;
            *confusion.entry(("합침", gold.clone(), label(merged).into())).or_default() += 1;
            println!("{} {case:10} 정답 {gold:7} 규칙 {:7} 모델 {:7} 합침 {:7} · {detail}", s("id"), label(rules), concept.map_or("none".into(), |v| format!("{v:?}").to_lowercase()), label(merged));
        }
        println!("\ncase        개수  규칙  모델  합침");
        let mut tot = [0; 4];
        for (case, v) in &by_case {
            println!("{case:10} {:>5} {:>5} {:>5} {:>5}", v[0], v[1], v[2], v[3]);
            (0..4).for_each(|i| tot[i] += v[i]);
        }
        let pct = |x: usize| x as f64 * 100.0 / tot[0] as f64;
        println!("정확도      {:>5} {:>4.1}% {:>4.1}% {:>4.1}%", tot[0], pct(tot[1]), pct(tot[2]), pct(tot[3]));
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
        #[cfg(windows)]
        println!("최대 메모리(작업 집합) {} MB", peak_mb());
    }

    #[cfg(windows)]
    fn peak_mb() -> usize {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            faults: u32,
            peak_ws: usize,
            rest: [usize; 7],
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(p: isize, c: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: 이 프로세스의 핸들과 크기를 맞춘 구조체를 넘긴다
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        c.peak_ws / 1_000_000
    }
}
