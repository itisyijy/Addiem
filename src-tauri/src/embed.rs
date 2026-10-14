//! I3 문장 임베딩 (post-mvp 6장). multilingual-e5-small(MIT)을 candle 로 CPU 에서 돌린다.
//! 모델은 사용자가 [내려받기]를 누를 때만 Hugging Face 의 고정 리비전에서 받고(앱의 유일한 외부 연결),
//! 파일마다 크기와 SHA-256 을 확인한다. 받은 뒤에는 인터넷 없이 동작하고, 자료나 카드는 기기 밖으로 나가지 않는다.

use crate::commands::AppState;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const REPO: &str = "intfloat/multilingual-e5-small";
/// 고정 리비전 (2026-04-02). 바꾸면 아래 크기와 해시를 함께 다시 적는다
const REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
/// (파일, 바이트, SHA-256)
const FILES: [(&str, u64, &str); 3] = [
    ("config.json", 655, "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959"),
    ("tokenizer.json", 17_082_730, "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39"),
    ("model.safetensors", 470_641_600, "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477"),
];
/// embedding_cache.model. 모델이 바뀌면 캐시를 다시 만든다
pub const MODEL_KEY: &str = "multilingual-e5-small@614241f";

static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static LOADED: Mutex<Option<Arc<Embedder>>> = Mutex::new(None);

fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join("multilingual-e5-small")
}

/// 받은 파일이 모두 있는가. 해시는 받을 때 확인했으므로 여기서는 크기만 본다
fn installed(dir: &Path) -> bool {
    FILES.iter().all(|(name, size, _)| dir.join(name).metadata().is_ok_and(|m| m.len() == *size))
}

#[derive(Serialize)]
pub struct ModelStatus {
    ready: bool,
    downloading: bool,
    /// 내려받을 전체 크기 (배너 "약 490MB")
    bytes: u64,
    name: &'static str,
    analyzing: Option<crate::source::Progress>,
}

#[tauri::command]
pub fn model_status(state: State<AppState>) -> ModelStatus {
    ModelStatus {
        ready: installed(&model_dir(&state.data_dir)),
        downloading: DOWNLOADING.load(Ordering::SeqCst),
        bytes: FILES.iter().map(|f| f.1).sum(),
        name: REPO,
        analyzing: crate::source::analyzing(),
    }
}

#[derive(Serialize, Clone)]
struct DownloadProgress {
    done: u64,
    total: u64,
}

/// [내려받기]. "model-progress" 이벤트로 진행을 알린다. 멈추거나 실패하면 받던 파일만 지운다
#[tauri::command]
pub async fn model_download(app: AppHandle) -> Res<()> {
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
pub fn model_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 모델 지우기. 임베딩 캐시도 같이 비운다 (이 모델로만 만든 것이라)
#[tauri::command]
pub fn model_delete(state: State<AppState>) -> Res<()> {
    if crate::source::analyzing().is_some() || DOWNLOADING.load(Ordering::SeqCst) {
        return Err("분석이나 내려받기가 끝난 뒤에 지울 수 있어요.".into());
    }
    *LOADED.lock().map_err(err)? = None;
    let dir = model_dir(&state.data_dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|_| "모델 파일을 지우지 못했어요. 앱을 다시 연 뒤에 해 주세요.".to_string())?;
    }
    let con = state.db.lock().map_err(err)?;
    con.execute("DELETE FROM embedding_cache", []).map_err(err)?;
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

fn download(app: &AppHandle, dir: &Path) -> Res<()> {
    std::fs::create_dir_all(dir).map_err(err)?;
    let total: u64 = FILES.iter().map(|f| f.1).sum();
    let mut done = 0;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(60)))
        .build()
        .into();
    for (name, size, sha) in FILES {
        let path = dir.join(name);
        // 지난번에 다 받은 파일은 건너뛴다
        if path.metadata().is_ok_and(|m| m.len() == size) && sha256_file(&path)? == sha {
            done += size;
            continue;
        }
        let part = dir.join(format!("{name}.part"));
        let res = fetch(app, &agent, name, size, sha, &part, done, total);
        if res.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        res?;
        std::fs::rename(&part, &path).map_err(err)?;
        done += size;
    }
    let _ = app.emit("model-progress", DownloadProgress { done: total, total });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fetch(app: &AppHandle, agent: &ureq::Agent, name: &str, size: u64, sha: &str, part: &Path, base: u64, total: u64) -> Res<()> {
    let url = format!("https://huggingface.co/{REPO}/resolve/{REVISION}/{name}");
    // 원인은 괄호 안에 남긴다 (문의할 때 보여줄 수 있게)
    fn offline<E: std::fmt::Display>(e: E) -> String {
        format!("모델을 내려받지 못했어요. 인터넷 연결을 확인하고 다시 시도해 주세요. ({e})")
    }
    let mut resp = agent.get(&url).call().map_err(offline)?;
    // ureq 의 limit 는 딱 그 크기에서도 "더 있는지" 보다가 오류를 낸다. 한 바이트 넉넉히 두고 크기는 아래에서 맞춘다
    let mut reader = resp.body_mut().with_config().limit(size + 1).reader();
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
        if got - last >= 1 << 20 {
            last = got;
            let _ = app.emit("model-progress", DownloadProgress { done: base + got, total });
        }
    }
    file.flush().map_err(err)?;
    if got != size || format!("{:x}", h.finalize()) != sha {
        return Err("받은 모델 파일이 원본과 달라요. 다시 시도해 주세요.".into());
    }
    Ok(())
}

pub struct Embedder {
    bert: BertModel,
    tok: Tokenizer,
}

/// 처음 쓸 때 한 번 읽어 둔다 (앱을 닫을 때까지)
pub fn load(data_dir: &Path) -> Res<Arc<Embedder>> {
    let mut slot = LOADED.lock().map_err(err)?;
    if let Some(m) = slot.as_ref() {
        return Ok(m.clone());
    }
    let dir = model_dir(data_dir);
    if !installed(&dir) {
        return Err("분석 모델이 없어요. 중요도 화면에서 먼저 내려받아 주세요.".into());
    }
    fn broken<E>(_: E) -> String {
        "모델 파일을 읽지 못했어요. 모델을 지우고 다시 내려받아 주세요.".into()
    }
    let cfg: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).map_err(broken)?).map_err(broken)?;
    // SAFETY: 앱이 받은 파일을 매핑해 읽기만 한다. 지우기(model_delete)는 이 모델을 놓은 뒤에만 한다
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DType::F32, &Device::Cpu) }.map_err(broken)?;
    let bert = BertModel::load(vb, &cfg).map_err(broken)?;
    let mut tok = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(broken)?;
    // 카드와 강조 구간은 짧다. 긴 글은 앞 256 토큰만 본다. XLM-R 의 패딩은 <pad>(1)
    tok.with_truncation(Some(TruncationParams { max_length: 256, ..Default::default() })).map_err(broken)?;
    tok.with_padding(Some(PaddingParams { pad_id: 1, pad_token: "<pad>".into(), ..Default::default() }));
    let m = Arc::new(Embedder { bert, tok });
    *slot = Some(m.clone());
    Ok(m)
}

impl Embedder {
    /// L2 정규화한 문장 벡터 (평균 풀링). 카드와 구간을 서로 견주므로(대칭) e5 규칙대로 둘 다 "query: "
    pub fn embed(&self, texts: &[String]) -> Res<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts.iter().map(|t| format!("query: {t}")).collect();
        let enc = self.tok.encode_batch(inputs, true).map_err(err)?;
        let ids: Vec<Tensor> = enc.iter().map(|e| Tensor::new(e.get_ids(), &Device::Cpu)).collect::<Result<_, _>>().map_err(err)?;
        let mask: Vec<Tensor> =
            enc.iter().map(|e| Tensor::new(e.get_attention_mask(), &Device::Cpu)).collect::<Result<_, _>>().map_err(err)?;
        let (ids, mask) = (Tensor::stack(&ids, 0).map_err(err)?, Tensor::stack(&mask, 0).map_err(err)?);
        let out = self.bert.forward(&ids, &ids.zeros_like().map_err(err)?, Some(&mask)).map_err(err)?;
        let m = mask.to_dtype(DType::F32).and_then(|m| m.unsqueeze(2)).map_err(err)?;
        let mean = out.broadcast_mul(&m).and_then(|x| x.sum(1)).and_then(|s| s.broadcast_div(&m.sum(1)?)).map_err(err)?;
        let norm = mean.sqr().and_then(|x| x.sum_keepdim(1)).and_then(|x| x.sqrt()).map_err(err)?;
        mean.broadcast_div(&norm).and_then(|v| v.to_vec2::<f32>()).map_err(err)
    }
}
