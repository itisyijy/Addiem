//! Addiem 8.3 개념 관계 확인 추론 엔진 (보조 실행 파일, DECISIONS J11~).
//! 본체(src-tauri/src/llm.rs)가 앱 리소스 폴더의 이 파일을 절대 경로로 띄우고 stdin/stdout 으로 한 줄짜리 JSON 을 주고받는다.
//! 본체는 AVX2 없이 빌드해 모든 CPU 에서 돌고, 이 실행 파일만 +avx2,+fma,+f16c 로 빌드한다(tools/build_llm.ps1, candle 의 양자화 계산은
//! 컴파일 때만 AVX2 를 켠다). 본체가 CPU 를 먼저 확인하고 지원할 때만 부른다.
//!
//! 요청 {"id":1,"op":"load","dir":"..."} → 답 {"id":1,"ok":{...}} 또는 {"id":1,"err":"..."}.
//! op: load(dir) · prime(prefix) · generate(prefix, rest, max) · choose(prefix, rest, slots, options) · stats.
//! {"op":"cancel","id":n} 은 n 번까지의 요청을 멈춘다(멈춘 요청은 err "cancelled"). stdin 이 닫히면 하던 일과 상관없이 끝난다 (본체가 죽어도 남지 않게)

mod engine;

use engine::{Gen, Llm, Res};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;

/// 멈추라는 요청 번호 (이 번호까지의 요청은 멈춘다)
static CANCEL: AtomicU64 = AtomicU64::new(0);

fn main() {
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            // 깨진 줄은 번호 0 으로 알린다
            let v = serde_json::from_str::<Value>(&line).unwrap_or_else(|_| json!({ "id": 0, "op": "bad" }));
            if v["op"] == "cancel" {
                CANCEL.fetch_max(v["id"].as_u64().unwrap_or(0), Ordering::SeqCst);
            } else if tx.send(v).is_err() {
                break;
            }
        }
        // 본체가 끝났다 (stdin 닫힘)
        std::process::exit(0);
    });
    let mut llm: Option<Llm> = None;
    for req in rx {
        let id = req["id"].as_u64().unwrap_or(0);
        let line = match handle(&mut llm, &req, &|| CANCEL.load(Ordering::SeqCst) >= id) {
            Ok(v) => json!({ "id": id, "ok": v }),
            Err(e) => json!({ "id": id, "err": e }),
        };
        let mut out = std::io::stdout().lock();
        if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

fn stat(g: Gen) -> Value {
    json!({ "prompt": g.prompt, "out": g.out, "ms": g.ms })
}

fn strings(v: &Value, key: &str) -> Res<Vec<String>> {
    v[key].as_array().ok_or_else(|| format!("{key} 가 없어요"))?.iter().map(|s| s.as_str().map(String::from).ok_or_else(|| format!("{key} 는 글이어야 해요"))).collect()
}

fn handle(llm: &mut Option<Llm>, req: &Value, stop: &dyn Fn() -> bool) -> Res<Value> {
    let s = |k: &str| req[k].as_str().ok_or_else(|| format!("{k} 가 없어요"));
    match req["op"].as_str().unwrap_or("") {
        "load" => {
            let t = Instant::now();
            *llm = None; // 앞 모델의 메모리를 먼저 놓는다
            let m = Llm::load(Path::new(s("dir")?))?;
            let arch = m.arch();
            *llm = Some(m);
            Ok(json!({ "arch": arch, "ms": t.elapsed().as_millis() as u64 }))
        }
        "stats" => Ok(json!({ "peak_mb": peak_mb(), "avx2": cfg!(target_feature = "avx2") })),
        op => {
            let m = llm.as_mut().ok_or("모델을 먼저 불러야 해요.")?;
            match op {
                "prime" => m.prime(s("prefix")?, stop).map(stat),
                "generate" => {
                    let max = req["max"].as_u64().unwrap_or(64).min(512) as usize;
                    m.generate(s("prefix")?, s("rest")?, max, stop).map(|(text, g)| json!({ "text": text, "gen": stat(g) }))
                }
                "choose" => m
                    .choose(s("prefix")?, s("rest")?, &strings(req, "slots")?, &strings(req, "options")?, stop)
                    .map(|(lp, g)| json!({ "logprobs": lp, "gen": stat(g) })),
                _ => Err(format!("모르는 요청이에요: {op}")),
            }
        }
    }
}

/// 이 프로세스의 최대 작업 집합 (MB). 측정용
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

#[cfg(not(windows))]
fn peak_mb() -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_without_a_model_fail_cleanly() {
        let mut llm = None;
        let never = || false;
        assert!(handle(&mut llm, &json!({ "op": "generate", "prefix": "", "rest": "a" }), &never).unwrap_err().contains("먼저"));
        assert!(handle(&mut llm, &json!({ "op": "load", "dir": "Z:/없는 폴더" }), &never).unwrap_err().contains("모델 파일"));
        assert!(handle(&mut llm, &json!({ "op": "bad" }), &never).is_err());
        assert!(handle(&mut llm, &json!({ "op": "stats" }), &never).unwrap()["peak_mb"].is_number());
    }
}
