fn main() {
    // AU20 읽어주기 음성(resources/tts)은 커밋하지 않는다 (tools/fetch_tts.ps1 이 채운다). 없는 빌드(CI)도 리소스 검사를 지나게 빈 폴더를 둔다
    std::fs::create_dir_all("resources/tts").expect("resources/tts");
    llm_engine();
    tauri_build::build()
}

/// 8.3 개념 관계 확인 엔진 (보조 실행 파일 src-tauri/llm). tools/build_llm.ps1 이 AVX2 로 빌드해 resources/llm 에 넣는다 (커밋하지 않는다).
/// 앱이 띄우기 전에 견줄 크기 · SHA-256 을 박아 둔다. 없으면 빈 값 (엔진 없음 → 핵심어 채점만, 개발 · CI 는 이대로 지나간다).
/// 릴리스 빌드(설치 파일)는 엔진 없이 만들지 않는다 (NEOANKI_ALLOW_NO_LLM=1 이면 알고 빼는 것)
fn llm_engine() {
    use sha2::Digest;
    std::fs::create_dir_all("resources/llm").expect("resources/llm");
    println!("cargo:rerun-if-changed=resources/llm");
    println!("cargo:rerun-if-env-changed=NEOANKI_ALLOW_NO_LLM");
    let (size, sha) = match std::fs::read("resources/llm/neoanki-llm.exe") {
        Ok(b) => (b.len().to_string(), format!("{:x}", sha2::Sha256::digest(&b))),
        Err(_) => (String::new(), String::new()),
    };
    if sha.is_empty() && std::env::var("PROFILE").as_deref() == Ok("release") && std::env::var_os("NEOANKI_ALLOW_NO_LLM").is_none() {
        panic!(
            "resources/llm/neoanki-llm.exe 가 없어요. 릴리스 설치 파일에는 개념 관계 확인 엔진이 들어가야 해요. \
             먼저 tools/build_llm.ps1 을 돌리세요 (알고 빼려면 NEOANKI_ALLOW_NO_LLM=1)"
        );
    }
    println!("cargo:rustc-env=NEOANKI_LLM_SIZE={size}");
    println!("cargo:rustc-env=NEOANKI_LLM_SHA={sha}");
}
