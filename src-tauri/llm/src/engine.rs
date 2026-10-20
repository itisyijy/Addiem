//! 모델 읽기 · 생성 · 고르기 (candle 0.11 quantized_qwen3 · quantized_qwen2, CPU). 프롬프트는 본체가 만든다

use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::models::{quantized_qwen2, quantized_qwen3};
use std::path::Path;
use std::time::Instant;

pub type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub const STOPPED: &str = "cancelled";
/// 입력을 이만큼씩 읽고 사이사이 멈춤을 본다 (candle CPU 양자화 행렬곱은 토큰마다 따로 돌아 묶어도 빨라지지 않는다)
const CHUNK: usize = 16;

enum Weights {
    Qwen3(quantized_qwen3::ModelWeights),
    Qwen2(quantized_qwen2::ModelWeights),
}

/// 한 번의 기록: 새로 읽은 토큰 · 쓴 토큰 · 걸린 시간
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gen {
    pub prompt: usize,
    pub out: usize,
    pub ms: u64,
}

pub struct Llm {
    w: Weights,
    tok: tokenizers::Tokenizer,
    eos: Vec<u32>,
    /// 모델 안(KV 캐시)에 읽혀 있는 토큰. 앞이 같은 입력은 그 뒤만 읽는다 (답을 쓰는 동안 앞부분을 미리 읽어 둔다)
    seen: Vec<u32>,
}

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
        let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json")).map_err(broken)?;
        let eos = ["<|im_end|>", "<|endoftext|>"].iter().filter_map(|t| tok.token_to_id(t)).collect();
        Ok(Llm { w, tok, eos, seen: vec![] })
    }

    pub fn arch(&self) -> &'static str {
        match self.w {
            Weights::Qwen3(_) => "qwen3",
            Weights::Qwen2(_) => "qwen2",
        }
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

    /// ids 를 모델에 읽힌다. 이미 읽혀 있는 앞부분은 건너뛴다 (새로 읽은 수, 마지막 로짓)
    fn feed(&mut self, ids: &[u32], stop: &dyn Fn() -> bool) -> Res<(usize, Option<Tensor>)> {
        if self.seen.is_empty() || !ids.starts_with(&self.seen) {
            self.reset();
        }
        let n = self.seen.len();
        Ok((ids.len() - n, self.push(&ids[n..], stop)?))
    }

    /// 앞부분을 미리 읽어 둔다. 같은 앞부분으로 generate · choose 하면 그 뒤만 읽는다
    pub fn prime(&mut self, prefix: &str, stop: &dyn Fn() -> bool) -> Res<Gen> {
        let t = Instant::now();
        let ids = self.encode(prefix)?;
        let (read, _) = self.feed(&ids, stop)?;
        Ok(Gen { prompt: read, out: 0, ms: t.elapsed().as_millis() as u64 })
    }

    fn joined(&self, prefix: &str, rest: &str) -> Res<Vec<u32>> {
        let mut ids = self.encode(prefix)?;
        ids.extend(self.encode(rest)?);
        Ok(ids)
    }

    /// prefix + rest 다음을 탐욕 디코딩 (같은 입력에 같은 결과). rest 는 비지 않아야 한다. stop() 이 참이면 멈춘다
    pub fn generate(&mut self, prefix: &str, rest: &str, max: usize, stop: &dyn Fn() -> bool) -> Res<(String, Gen)> {
        let t = Instant::now();
        let ids = self.joined(prefix, rest)?;
        let (read, last) = self.feed(&ids, stop)?;
        let mut logits = last.ok_or("읽을 글이 없어요.")?;
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
        let g = Gen { prompt: read, out: out.len(), ms: t.elapsed().as_millis() as u64 };
        Ok((self.tok.decode(&out, true).map_err(err)?, g))
    }

    /// prefix + rest 를 읽은 뒤 slots 를 차례로 이어 읽으며, 자리마다 options 의 첫 토큰 로짓을 견준다.
    /// 자리마다 options 위의 로그 확률(options 끼리 정규화)을 돌려주고, 가장 높은 것을 이어 붙여 다음 자리로 간다 (글을 쓰지 않아 형식이 틀릴 수 없다).
    /// 빈 자리는 방금 읽은 것 바로 다음
    pub fn choose(&mut self, prefix: &str, rest: &str, slots: &[String], options: &[String], stop: &dyn Fn() -> bool) -> Res<(Vec<Vec<f32>>, Gen)> {
        let t = Instant::now();
        let ids = self.joined(prefix, rest)?;
        let (mut read, mut last) = self.feed(&ids, stop)?;
        let firsts = options.iter().map(|o| self.encode(o)?.first().copied().ok_or_else(|| "빈 선택지".to_string())).collect::<Res<Vec<u32>>>()?;
        if (1..firsts.len()).any(|i| firsts[..i].contains(&firsts[i])) {
            return Err("선택지의 첫 토큰이 겹쳐요.".into());
        }
        let mut all = vec![];
        for (i, slot) in slots.iter().enumerate() {
            if !slot.is_empty() {
                let s = self.encode(slot)?;
                read += s.len();
                last = self.push(&s, stop)?;
            }
            let logits = last.take().ok_or("읽을 글이 없어요.")?.squeeze(0).and_then(|l| l.to_vec1::<f32>()).map_err(err)?;
            let lp = log_softmax(&firsts.iter().map(|&f| logits[f as usize]).collect::<Vec<_>>());
            let best = (0..lp.len()).max_by(|&a, &b| lp[a].total_cmp(&lp[b])).unwrap_or(0);
            all.push(lp);
            if i + 1 < slots.len() {
                let o = self.encode(&options[best])?;
                read += o.len();
                last = self.push(&o, stop)?;
            }
        }
        Ok((all, Gen { prompt: read, out: slots.len(), ms: t.elapsed().as_millis() as u64 }))
    }
}

pub fn log_softmax(x: &[f32]) -> Vec<f32> {
    let m = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let lse = m + x.iter().map(|v| (v - m).exp()).sum::<f32>().ln();
    x.iter().map(|v| v - lse).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_softmax_is_normalized() {
        let lp = super::log_softmax(&[1.0, 2.0, 3.0]);
        assert!((lp.iter().map(|v| v.exp()).sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(lp[2] > lp[1] && lp[1] > lp[0]);
    }
}
