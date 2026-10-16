fn main() {
    // AU20 읽어주기 음성(resources/tts)은 커밋하지 않는다 (tools/fetch_tts.ps1 이 채운다). 없는 빌드(CI)도 리소스 검사를 지나게 빈 폴더를 둔다
    std::fs::create_dir_all("resources/tts").expect("resources/tts");
    tauri_build::build()
}
