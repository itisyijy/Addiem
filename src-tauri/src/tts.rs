//! AU20 읽어주기 (Anki `{{tts 언어:필드}}`). Supertonic 3 (Supertone, OpenRAIL-M) fp32 · 목소리 F1 · 8단계를
//! ONNX Runtime 으로 CPU 에서 돌린다. 모델과 onnxruntime.dll 은 설치 파일 리소스에 들어 있다 (resources/tts, tools/fetch_tts.ps1).
//! 공식 Rust 예제(supertone-oss-archive/supertonic rust/src/helper.rs, MIT)에서 필요한 부분만 옮겼다.
//! 만든 소리는 메모리에서만 재생한다. 노트 · 미디어 · 내보내기에 쓰지 않는다 (OpenRAIL-M 부록 A (e)).
//! 파일이 없거나(개발 · CI 빌드) 엔진이 뜨지 않으면 화면이 OS 음성(speechSynthesis)으로 읽는다 (cardHtml.ts)

use base64::Engine as _;
use ort::session::Session;
use ort::value::Tensor;
use rand_distr::{Distribution, StandardNormal};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};
use unicode_normalization_alignments::UnicodeNormalization;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Supertonic 3 이 읽는 언어 (예제의 AVAILABLE_LANGS 에서 "na" 를 뺐다)
const LANGS: [&str; 31] = [
    "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu", "id", "it", "lt", "lv", "nl", "pl",
    "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi",
];
/// 사용자가 들어 보고 고른 품질 (fp32 · 8단계)
const STEPS: usize = 8;
/// 리소스 폴더(tts/) 안의 파일. 첫 줄이 ONNX Runtime
const FILES: [&str; 8] = [
    "onnxruntime/onnxruntime.dll",
    "supertonic-3/onnx/duration_predictor.onnx",
    "supertonic-3/onnx/text_encoder.onnx",
    "supertonic-3/onnx/vector_estimator.onnx",
    "supertonic-3/onnx/vocoder.onnx",
    "supertonic-3/onnx/tts.json",
    "supertonic-3/onnx/unicode_indexer.json",
    "supertonic-3/voice_styles/F1.json",
];

/// Anki 언어("ko_KR", "en-US") 의 앞부분 ("ko")
pub fn prefix(lang: &str) -> String {
    lang.split(['_', '-']).next().unwrap_or_default().to_ascii_lowercase()
}

/// 내장 음성이 읽는 언어인가 (Supertonic 언어 이름)
pub fn engine_lang(lang: &str) -> Option<&'static str> {
    let p = prefix(lang);
    LANGS.into_iter().find(|l| *l == p)
}

/// 템플릿이 쓰는 읽어주기 언어 (`{{tts en_US ...:필드}}` 의 en_US)
pub fn template_langs(tpl: &str) -> impl Iterator<Item = &str> {
    tpl.split("{{tts ").skip(1).filter_map(|s| s.split([' ', ':', '}']).next()).filter(|l| !l.is_empty())
}

/// AU23 가져오기 참고 줄의 이름. 내장 음성에 없는 언어만 이름을 둔다 (나머지는 개발 빌드에서만 나온다)
pub fn no_voice(lang: &str) -> &'static str {
    match prefix(lang).as_str() {
        "zh" => "읽어주기(중국어)",
        "yue" => "읽어주기(광둥어)",
        "th" => "읽어주기(태국어)",
        "he" | "iw" => "읽어주기(히브리어)",
        "fa" => "읽어주기(페르시아어)",
        "ca" => "읽어주기(카탈로니아어)",
        "nb" | "no" | "nn" => "읽어주기(노르웨이어)",
        "ms" => "읽어주기(말레이어)",
        "sr" => "읽어주기(세르비아어)",
        "fil" | "tl" => "읽어주기(필리핀어)",
        _ => "읽어주기(다른 언어)",
    }
}

fn dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().resource_dir().ok().map(|d| d.join("tts"))
}

fn installed(dir: &Path) -> bool {
    FILES.iter().all(|f| dir.join(f).is_file())
}

struct Engine {
    /// 세션보다 오래 살아야 한다. 프로세스에 하나만 만들 수 있다
    _env: ort::environment::Environment,
    dp: Session,
    enc: Session,
    est: Session,
    voc: Session,
    indexer: Vec<i64>,
    style_ttl: Tensor<f32>,
    style_dp: Tensor<f32>,
    rate: usize,
    /// 잠재 한 칸이 맡는 표본 수, 잠재 채널 수
    chunk: usize,
    latent: usize,
}

/// 처음 쓸 때 한 번 올린다 (약 0.8초 · 470MB). 실패도 기억한다 (환경은 프로세스에 하나라 다시 만들 수 없다).
/// ponytail: 엔진 하나를 잠가 한 번에 한 문장씩 읽는다. 카드 한 장의 칩 몇 개라 충분하다
static ENGINE: OnceLock<Result<Mutex<Engine>, String>> = OnceLock::new();

fn engine(dir: &Path) -> Res<&'static Mutex<Engine>> {
    ENGINE.get_or_init(|| load(dir).map(Mutex::new)).as_ref().map_err(Clone::clone)
}

fn read<T: DeserializeOwned>(path: PathBuf) -> Res<T> {
    serde_json::from_slice(&std::fs::read(path).map_err(err)?).map_err(err)
}

fn load(dir: &Path) -> Res<Engine> {
    if !installed(dir) {
        return Err("읽어주기 음성 파일이 없어요.".into());
    }
    // 절대 경로로만 연다. System32 의 onnxruntime.dll(Windows ML)을 잡지 않게
    let env = ort::init_from(dir.join(FILES[0])).map_err(err)?.with_name("neo-anki-tts").build().map_err(err)?;
    let m = dir.join("supertonic-3");
    let open = |name: &str| Session::builder(&env).map_err(err)?.commit_from_file(m.join("onnx").join(name)).map_err(err);
    let (dp, enc, est, voc) =
        (open("duration_predictor.onnx")?, open("text_encoder.onnx")?, open("vector_estimator.onnx")?, open("vocoder.onnx")?);
    let cfg: serde_json::Value = read(m.join("onnx/tts.json"))?;
    let num = |p: &str| cfg.pointer(p).and_then(serde_json::Value::as_u64).map(|v| v as usize).ok_or_else(|| format!("tts.json 에 {p} 가 없어요"));
    let ccf = num("/ttl/chunk_compress_factor")?;
    #[derive(serde::Deserialize)]
    struct Style {
        data: Vec<Vec<Vec<f32>>>,
        dims: Vec<i64>,
    }
    #[derive(serde::Deserialize)]
    struct Voice {
        style_ttl: Style,
        style_dp: Style,
    }
    let voice: Voice = read(m.join("voice_styles/F1.json"))?;
    let tensor = |s: Style| Tensor::from_array((s.dims, s.data.into_iter().flatten().flatten().collect::<Vec<f32>>())).map_err(err);
    Ok(Engine {
        indexer: read(m.join("onnx/unicode_indexer.json"))?,
        style_ttl: tensor(voice.style_ttl)?,
        style_dp: tensor(voice.style_dp)?,
        rate: num("/ae/sample_rate")?,
        chunk: num("/ae/base_chunk_size")? * ccf,
        latent: num("/ttl/latent_dim")? * ccf,
        dp,
        enc,
        est,
        voc,
        _env: env,
    })
}

/// 모델에 넣을 글자 (예제 preprocess_text 를 정규식 없이). NFKD · 이모지와 기호 정리 · 끝 문장부호 · <언어> 태그
fn preprocess(text: &str, lang: &str) -> String {
    let emoji = |c: char| matches!(c as u32, 0x1F1E6..=0x1F1FF | 0x1F300..=0x1F64F | 0x1F680..=0x1FAFF | 0x2600..=0x27BF);
    let mut t: String = text.nfkd().map(|(c, _)| c).filter(|c| !emoji(*c)).collect();
    for (from, to) in [
        ("–", "-"),
        ("‑", "-"),
        ("—", "-"),
        ("_", " "),
        ("\u{201C}", "\""),
        ("\u{201D}", "\""),
        ("\u{2018}", "'"),
        ("\u{2019}", "'"),
        ("´", "'"),
        ("`", "'"),
        ("[", " "),
        ("]", " "),
        ("|", " "),
        ("/", " "),
        ("#", " "),
        ("→", " "),
        ("←", " "),
        ("©", ""),
        ("\\", ""),
        ("@", " at "),
        ("e.g.,", "for example, "),
        ("i.e.,", "that is, "),
    ] {
        t = t.replace(from, to);
    }
    for p in [",", ".", "!", "?", ";", ":", "'"] {
        t = t.replace(&format!(" {p}"), p);
    }
    for q in ["\"", "'"] {
        while t.contains(&q.repeat(2)) {
            t = t.replace(&q.repeat(2), q);
        }
    }
    let mut t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let ends = ".!?;:,'\"\u{201C}\u{201D}\u{2018}\u{2019})]}…。」』】〉》›»";
    if t.chars().last().is_some_and(|c| !ends.contains(c)) {
        t.push('.');
    }
    format!("<{lang}>{t}</{lang}>")
}

/// 문장 끝(. ! ? 뒤 띄어쓰기, 。！？)에서 나눈다
fn sentences(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let spaced = it.peek().is_none_or(|(_, n)| n.is_whitespace());
        if matches!(c, '。' | '！' | '？') || (matches!(c, '.' | '!' | '?') && spaced) {
            out.push(&text[start..i + c.len_utf8()]);
            start = i + c.len_utf8();
        }
    }
    out.push(&text[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty()).collect()
}

/// 한 번에 읽을 조각. 글자 수로 센다 (원본은 UTF-8 바이트를 셌다): 한국어 · 일본어 120자, 그 밖 300자.
/// 문장을 이어 붙이고, 너무 긴 문장은 낱말로, 띄어쓰기 없이 긴 글은 글자로 자른다
fn chunks(text: &str, lang: &str) -> Vec<String> {
    let max = if matches!(lang, "ko" | "ja") { 120 } else { 300 };
    let mut out: Vec<String> = vec![];
    let mut push = |piece: &str| match out.last_mut() {
        Some(last) if last.chars().count() + 1 + piece.chars().count() <= max => {
            last.push(' ');
            last.push_str(piece);
        }
        _ => out.push(piece.to_string()),
    };
    for s in sentences(text) {
        if s.chars().count() <= max {
            push(s);
            continue;
        }
        for w in s.split_whitespace() {
            for part in w.chars().collect::<Vec<_>>().chunks(max) {
                push(&part.iter().collect::<String>());
            }
        }
    }
    out
}

/// 조각 하나 -> 소리 (예제 _infer, 묶음 크기 1). speed 는 Anki 의 speed= (1 이 보통)
fn infer(e: &mut Engine, text: &str, lang: &str, speed: f32) -> Res<Vec<f32>> {
    let ids: Vec<i64> = preprocess(text, lang).chars().map(|c| e.indexer.get(c as usize).copied().unwrap_or(-1)).collect();
    let n = ids.len();
    let ids = Tensor::from_array(([1, n], ids)).map_err(err)?;
    let mask = Tensor::from_array(([1, 1, n], vec![1f32; n])).map_err(err)?;
    // 예제의 보통 빠르기가 1.05 다
    let dur = {
        let o = e.dp.run(ort::inputs! { "text_ids" => &ids, "style_dp" => &e.style_dp, "text_mask" => &mask }).map_err(err)?;
        o["duration"].try_extract_tensor::<f32>().map_err(err)?.1[0] / (1.05 * speed.clamp(0.5, 2.0))
    };
    let emb = {
        let o = e.enc.run(ort::inputs! { "text_ids" => &ids, "style_ttl" => &e.style_ttl, "text_mask" => &mask }).map_err(err)?;
        let (shape, data) = o["text_emb"].try_extract_tensor::<f32>().map_err(err)?;
        Tensor::from_array((shape.clone(), data.to_vec())).map_err(err)?
    };
    let len = (dur.max(0.0) * e.rate as f32) as usize;
    let frames = len.div_ceil(e.chunk).max(1);
    let mut rng = rand::rng();
    let mut x: Vec<f32> = (0..e.latent * frames).map(|_| StandardNormal.sample(&mut rng)).collect();
    let latent_mask = Tensor::from_array(([1, 1, frames], vec![1f32; frames])).map_err(err)?;
    let total = Tensor::from_array(([1usize], vec![STEPS as f32])).map_err(err)?;
    for step in 0..STEPS {
        let cur = Tensor::from_array(([1usize], vec![step as f32])).map_err(err)?;
        let xt = Tensor::from_array(([1, e.latent, frames], x)).map_err(err)?;
        let o = e
            .est
            .run(ort::inputs! {
                "noisy_latent" => &xt, "text_emb" => &emb, "style_ttl" => &e.style_ttl, "latent_mask" => &latent_mask,
                "text_mask" => &mask, "current_step" => &cur, "total_step" => &total
            })
            .map_err(err)?;
        x = o["denoised_latent"].try_extract_tensor::<f32>().map_err(err)?.1.to_vec();
    }
    let xt = Tensor::from_array(([1, e.latent, frames], x)).map_err(err)?;
    let o = e.voc.run(ort::inputs! { "latent" => &xt }).map_err(err)?;
    let wav = o["wav_tts"].try_extract_tensor::<f32>().map_err(err)?.1;
    Ok(wav[..len.min(wav.len())].to_vec())
}

/// 글 -> 소리. 조각 사이는 0.3초 쉰다 (예제 call)
fn speak(e: &mut Engine, text: &str, lang: &str, speed: f32) -> Res<Vec<f32>> {
    let mut out = vec![];
    for (i, part) in chunks(text, lang).iter().enumerate() {
        if i > 0 {
            out.resize(out.len() + e.rate * 3 / 10, 0.0);
        }
        out.extend(infer(e, part, lang, speed)?);
    }
    Ok(trim(&out, e.rate))
}

/// 앞뒤 무음을 자른다 (실측 앞 260–570ms · 뒤 330–1040ms). 가장 큰 소리의 2% 를 넘는 곳부터 끝까지, 앞 20ms · 뒤 80ms 는 남긴다
fn trim(s: &[f32], rate: usize) -> Vec<f32> {
    let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
    let loud = |v: &f32| v.abs() > peak * 0.02;
    match (s.iter().position(loud), s.iter().rposition(loud)) {
        (Some(a), Some(b)) => s[a.saturating_sub(rate / 50)..(b + 1 + rate * 8 / 100).min(s.len())].to_vec(),
        _ => s.to_vec(),
    }
}

/// 모노 16비트 PCM WAV (44바이트 머리 + 표본)
fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    // fmt 길이 16 · PCM 1 · 채널 1 · 표본률 · 초당 바이트 · 블록 2 · 16비트
    for (v, n) in [(16, 4), (1, 2), (1, 2), (rate, 4), (rate * 2, 4), (2, 2), (16, 2)] {
        out.extend_from_slice(&u32::to_le_bytes(v)[..n]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// 이 설치본이 내장 음성으로 읽는 언어. 파일이 없으면(개발 · CI 빌드) 비어 있고 화면은 OS 음성만 쓴다
#[tauri::command]
pub fn tts_langs(app: AppHandle) -> Vec<&'static str> {
    if dir(&app).is_some_and(|d| installed(&d)) { LANGS.to_vec() } else { vec![] }
}

/// 읽어주기 칩이 있는 카드가 보이면 엔진을 미리 올려 둔다
#[tauri::command]
pub async fn tts_preload(app: AppHandle) -> Res<()> {
    let dir = dir(&app).ok_or("리소스 폴더를 찾을 수 없어요.")?;
    tauri::async_runtime::spawn_blocking(move || engine(&dir).map(|_| ())).await.map_err(err)?
}

/// 읽어 준 소리 (data:audio/wav). 화면이 메모리에만 둔다. 내장 음성에 없는 언어면 오류 (화면은 OS 음성으로)
#[tauri::command]
pub async fn tts_speak(app: AppHandle, lang: String, text: String, speed: Option<f32>) -> Res<String> {
    let lang = engine_lang(&lang).ok_or("이 언어는 내장 음성이 읽지 못해요.")?;
    let dir = dir(&app).ok_or("리소스 폴더를 찾을 수 없어요.")?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut e = engine(&dir)?.lock().map_err(err)?;
        let samples = speak(&mut e, &text, lang, speed.unwrap_or(1.0))?;
        Ok(format!("data:audio/wav;base64,{}", base64::engine::general_purpose::STANDARD.encode(wav(&samples, e.rate as u32))))
    })
    .await
    .map_err(err)?
}

/// 설정 > 오픈소스 라이선스 전문. which: "supertonic" | "onnxruntime" (MIT + ThirdPartyNotices)
#[tauri::command]
pub fn tts_license(app: AppHandle, which: String) -> Res<String> {
    let d = dir(&app).ok_or("리소스 폴더를 찾을 수 없어요.")?;
    let files: &[&str] = match which.as_str() {
        "supertonic" => &["supertonic-3/LICENSE"],
        "onnxruntime" => &["onnxruntime/LICENSE", "onnxruntime/ThirdPartyNotices.txt"],
        _ => return Err("모르는 라이선스예요.".into()),
    };
    let texts: Vec<String> = files.iter().map(|f| std::fs::read_to_string(d.join(f)).map_err(|_| "라이선스 파일이 없어요.".to_string())).collect::<Res<_>>()?;
    Ok(texts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_map_by_prefix() {
        assert_eq!((engine_lang("ko_KR"), engine_lang("en-US"), engine_lang("FR_fr"), engine_lang("pt_BR")), (Some("ko"), Some("en"), Some("fr"), Some("pt")));
        assert_eq!((engine_lang("zh_CN"), engine_lang("yue"), engine_lang("")), (None, None, None), "중국어 등은 OS 음성으로");
        assert_eq!(no_voice("zh_TW"), "읽어주기(중국어)");
        assert_eq!(no_voice("xx_YY"), "읽어주기(다른 언어)");
        let tpl = "{{tts en_US:Front}}<br>{{tts ko_KR voices=Heami speed=1.2:Back}}{{tts zh_CN:cloze:Text}}{{tts }}";
        assert_eq!(template_langs(tpl).collect::<Vec<_>>(), ["en_US", "ko_KR", "zh_CN"]);
    }

    #[test]
    fn text_is_prepared_like_the_example() {
        assert_eq!(preprocess("They had to abandon the car", "en"), "<en>They had to abandon the car.</en>");
        assert_eq!(preprocess("a — b [c], d ?", "en"), "<en>a - b c, d?</en>");
        assert_eq!(preprocess("“hi”😀", "en"), "<en>\"hi\"</en>", "따옴표는 곧게, 이모지는 뺀다");
        // NFKD: 한글은 자모로 풀린다 (모델이 그렇게 배웠다)
        assert_eq!(preprocess("한", "ko"), "<ko>\u{1112}\u{1161}\u{11AB}.</ko>");
        assert_eq!(preprocess("", "en"), "<en></en>");
    }

    #[test]
    fn long_text_is_chunked_by_characters() {
        assert_eq!(chunks("One. Two! Three", "en"), ["One. Two! Three"]);
        assert_eq!(sentences("3.5 kg. 다음。次"), ["3.5 kg.", "다음。", "次"]);
        // 한국어는 120자: 바이트로 세면 40자에서 잘렸다
        let ko = "가".repeat(100);
        assert_eq!(chunks(&format!("{ko}. {ko}."), "ko").len(), 2);
        assert_eq!(chunks(&format!("{ko}."), "ko"), [format!("{ko}.")]);
        // 띄어쓰기 없는 긴 일본어는 글자로 자른다
        let ja = "あ".repeat(250);
        assert_eq!(chunks(&ja, "ja").iter().map(|c| c.chars().count()).collect::<Vec<_>>(), [120, 120, 10]);
        // 긴 영어 문장은 낱말 사이에서
        let en = "word ".repeat(100);
        let c = chunks(&en, "en");
        assert!(c.len() == 2 && c.iter().all(|p| p.chars().count() <= 300 && !p.starts_with(' ')));
        assert!(chunks("", "en").is_empty());
    }

    #[test]
    fn wav_header_and_trim() {
        let w = wav(&[0.0, 1.0, -1.0, 2.0], 44_100);
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 1, "PCM");
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 1, "모노");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 44_100);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 88_200);
        assert_eq!((u16::from_le_bytes(w[32..34].try_into().unwrap()), u16::from_le_bytes(w[34..36].try_into().unwrap())), (2, 16));
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        let pcm: Vec<i16> = w[44..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(pcm, [0, 32767, -32767, 32767], "넘치면 자른다");

        // 1kHz 기준: 앞 300ms · 뒤 500ms 무음 -> 앞 20ms · 뒤 80ms 만 남는다
        let mut s = vec![0.0; 300];
        s.extend(vec![0.5; 100]);
        s.extend(vec![0.001; 500]);
        let t = trim(&s, 1000);
        assert_eq!(t.len(), 20 + 100 + 80);
        assert_eq!(trim(&[0.0; 10], 1000).len(), 10, "무음뿐이면 그대로");
    }

    /// 실제 모델로 한 문장 (resources/tts 가 있을 때만): cargo test tts -- --ignored --nocapture
    #[test]
    #[ignore]
    fn synthesizes_one_sentence() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/tts");
        assert!(installed(&dir), "tools/fetch_tts.ps1 로 resources/tts 를 먼저 채운다");
        let t = std::time::Instant::now();
        let e = engine(&dir).unwrap();
        let load = t.elapsed();
        let mut e = e.lock().unwrap();
        for (text, lang) in [("They had to abandon the car.", "en"), ("버리다, 포기하다", "ko"), ("abandonner", "fr")] {
            let t = std::time::Instant::now();
            let s = speak(&mut e, text, lang, 1.0).unwrap();
            println!("{lang} {text}: {:.2}s 소리 · {} ms", s.len() as f32 / e.rate as f32, t.elapsed().as_millis());
            assert!(s.len() > e.rate / 4 && s.iter().any(|v| v.abs() > 0.05), "들리는 소리가 나온다");
        }
        println!("올리기 {} ms", load.as_millis());
    }
}
