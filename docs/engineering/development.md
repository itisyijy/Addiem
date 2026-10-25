# 개발 문서

앱을 빌드하고 고치는 사람을 위한 안내다. 사용자 안내는 [README](../../README.md)에 있다.

## Documentation

- [Anki 장기 사용자 인터뷰](../user-research/anki-power-user-interview.md)
- [Addiem 제품 방향](../product/product-direction.md)
- [Addiem MVP 범위](../product/mvp-scope.md)
- [Addiem MVP 화면 흐름](../product/mvp-screen-flow.md)
- [MVP 다음 단계: 원자료 기반 중요도 판정](../product/post-mvp-source-analysis.md)

### 설계

- [ADR 0001. 기술 스택과 책임 경계](../adr/0001-tech-stack.md)
- [데이터 모델](data-model.md) — 정본은 [`db/schema.sql`](../../db/schema.sql)
- [오늘의 학습 큐 명세](study-queue-spec.md)
- [JSON 일괄 가져오기 형식](json-import-format.md)
- [UI 문구 기준](../design/copy-guide.md) — 용어표와 톤·배치 규칙. 근거는 [UI 문구 이해도 검토](../design/ui-copy-review.md)

### 스파이크

- [1. Anki 패키지 구조 실측](spike-01-apkg-parsing.md)
- [5. FSRS 적합성](spike-05-fsrs.md)

### 검토 대기

- [검토 대기 결정 목록](../DECISIONS-PENDING-REVIEW.md)

## 개발 환경

언어마다 프로젝트 안에서 버전을 고정한다. 전역에 설치하는 것은 언어 런타임과 rustup뿐이다.

| 언어 | 고정 장치 | 버전 |
|---|---|---|
| Python | `.venv/` + [`requirements.txt`](../../requirements.txt) | 3.12 |
| Rust | [`rust-toolchain.toml`](../../rust-toolchain.toml). 이 폴더 안에서 rustup이 자동으로 이 버전을 쓴다 | 1.99.0 |
| Node | `package.json` 의 `engines` + `package-lock.json` | 24 |

### 필요한 것

| 도구 | 확인 |
|---|---|
| Python 3.12 | `python --version` |
| Node 24 | `node --version` |
| Rust (rustup) | `rustc --version`. 없으면 [rustup.rs](https://rustup.rs) |
| MSVC C++ 빌드 도구 | Visual Studio 2022의 "C++를 사용한 데스크톱 개발" |
| WebView2 | Windows 11에는 기본 포함 |

Tauri 기준으로 한 번에 점검하려면 이렇게 한다.

```bash
npx @tauri-apps/cli@2 info
```

### 처음 한 번

```powershell
npm install
python -m venv .venv
.venv\Scripts\python -m pip install -r requirements.txt
```

Rust는 따로 할 일이 없다. 이 폴더에서 `cargo` 를 처음 실행하면 `rust-toolchain.toml` 의 버전이 자동으로 설치된다.

### 앱 실행

```powershell
npm run tauri dev       # 개발 모드. 프론트엔드는 저장하면 바로 반영, Rust는 다시 컴파일
npm run tauri build     # NSIS 설치 파일 생성 (src-tauri/target/release/bundle/nsis/)
```

데이터는 `%APPDATA%\io.github.itisyijy.addiem\` 에 저장된다 (DB, `media/`, `backups/`). 다른 폴더로 시험하려면 환경 변수 `ADDIEM_DATA_DIR` 을 지정한다.

```powershell
$env:ADDIEM_DATA_DIR = "C:\temp\addiem-test"; npm run tauri dev
```

### 읽어주기 음성 (빌드)

Anki `{{tts}}` 읽어주기는 설치 파일에 들어 있는 Supertonic 3 음성(fp32 · 목소리 F1, OpenRAIL-M)과 ONNX Runtime 1.31.0(MIT)으로 이 기기에서 읽는다. 약 400MB라 저장소에는 없다. 빌드 전에 `src-tauri/resources/tts/`(gitignore)를 채운다. 파일마다 SHA-256 을 확인한다.

```powershell
pwsh -File tools\fetch_tts.ps1             # %LOCALAPPDATA%\addiem-dev\tts 에 받아 둔 파일을 복사 (-Source 로 다른 폴더)
pwsh -File tools\fetch_tts.ps1 -Download   # 받아 둔 것이 없으면 Hugging Face 고정 리비전 · ONNX Runtime GitHub 릴리스에서 받는다
```

폴더가 비어 있어도 `cargo build` · `npm run tauri dev` · CI 는 그대로 돈다(`build.rs` 가 빈 폴더를 만든다). 그 빌드의 읽어주기는 OS 음성(Windows 설정 › 시간 및 언어 › 음성)으로만 읽는다. 실제 모델로 한 문장 읽어 보는 시험은 `cd src-tauri; cargo test tts -- --ignored --nocapture`.

### 서술형 개념 관계 확인 엔진 (8.3)

서술형 답의 개념 관계를 이 기기에서 살펴보는 실험 기능은 추론을 보조 실행 파일 `addiem-llm.exe`(`src-tauri/llm`, candle)가 한다. 본체는 AVX2 없이 빌드해 모든 CPU 에서 돌고, 엔진만 AVX2 로 빌드한다(candle 의 양자화 계산은 컴파일 때만 AVX2 를 켠다). 본체는 CPU 가 AVX2 · FMA · F16C 를 지원하고, 앱 리소스 폴더의 엔진 파일이 빌드 때 잰 크기 · SHA-256 과 같을 때만 띄운다. 모델(Qwen2.5-1.5B-Instruct 4비트, 약 1.1GB)은 설치 파일에 넣지 않고 설정 › 서술형 채점에서 사용자가 받는다.

```powershell
pwsh -File tools\build_llm.ps1             # RUSTFLAGS +avx2,+fma,+f16c 로 따로 빌드해 src-tauri/resources/llm/ 에 넣는다 (gitignore)
```

엔진이 없어도 `cargo build` · `npm run tauri dev` · CI 는 그대로 돈다(`build.rs` 가 빈 폴더를 만들고, 앱은 "이 설치에는 엔진이 없어요"로 핵심어 채점만 한다). **릴리스 빌드(`npm run tauri build`)는 엔진 없이는 멈춘다**(알고 빼려면 `ADDIEM_ALLOW_NO_LLM=1`). 개발셋 측정은 `cd src-tauri; $env:ADDIEM_LLM_BIN="<addiem-llm.exe>"; $env:ADDIEM_LLM_DIR="<model.gguf · tokenizer.json 폴더>"; cargo test --lib eval83 -- --ignored --nocapture` (DECISIONS J).

### Google Drive 동기화 (개발)

[설계](drive-sync.md) · 코드는 `src-tauri/src/sync/`. Google 연결은 빌드할 때 넣는 OAuth 클라이언트가 있어야 켜진다. 없으면 설정의 Google Drive 칸이 "이 앱 버전에서는 Google Drive 연결을 사용할 수 없어요"로 보이고 나머지는 그대로 동작한다.

1. Google Cloud 콘솔에서 Drive API 를 켜고, OAuth 동의 화면에 범위 `drive.file` · `drive.appdata` 두 개만 넣는다 (테스트 단계에서는 테스트 사용자를 등록).
2. 사용자 인증 정보 > OAuth 클라이언트 ID > **데스크톱 앱**을 만든다. 리디렉션은 `http://127.0.0.1:<임의 포트>` 루프백이라 따로 적지 않는다.
3. 빌드하는 셸에서 두 값을 넣고 빌드한다 (`option_env!` 라 빌드할 때만 읽는다. 바꾸면 다시 빌드).

```powershell
$env:ADDIEM_GOOGLE_CLIENT_ID = "<클라이언트 ID>.apps.googleusercontent.com"
$env:ADDIEM_GOOGLE_CLIENT_SECRET = "<클라이언트 보안 비밀>"   # 데스크톱 클라이언트에 Google 이 주는 값. PKCE 와 함께 쓴다
npm run tauri dev
```

토큰과 잠금 키는 OS 키체인(Windows 자격 증명 관리자, 서비스 이름 `io.github.itisyijy.addiem`)에만 저장된다.

Google 계정 없이 시험하려면 로컬 폴더를 Drive 대신 쓴다. 같은 폴더를 가리키는 두 인스턴스(데이터 폴더는 다르게)가 두 기기처럼 맞춰진다. 로그인 창 없이 바로 연결된다.

```powershell
$env:ADDIEM_SYNC_FOLDER = "C:\temp\addiem-drive"; $env:ADDIEM_DATA_DIR = "C:\temp\addiem-a"; npm run tauri dev
```

엔진 테스트(두 기기 시뮬레이션)는 `cargo test sync::` 로 돈다.

### ChatGPT 연결 (개발)

환경 변수가 필요 없다 (Sign in with ChatGPT 의 직접 흐름은 클라이언트 비밀이 없다). 실제 계정 없이 화면 상태를 보려면 debug 빌드에서 가짜 응답을 켠다. 성격별로 묶기(L4)와 자료 근거 AI(R1 초안 · R2 채점 보조 · R3 근거)가 같이 쓴다. 네트워크와 키체인을 쓰지 않고, 켜 둔 동안에는 [다시 로그인] 등을 눌러도 로그인 창을 열지 않는다. 빠진 범위 · 비슷한 카드 같은 기기 안 검사는 가짜가 아니라 실제로 돈다 (비슷한 카드 · 근거 검사는 분석 모델이 있을 때만).

```powershell
# ok | slow | usage_limit(= limit) | expired | offline | unavailable | malformed
$env:ADDIEM_AI_FAKE = "ok"; $env:ADDIEM_DATA_DIR = "C:\temp\addiem-test"; npm run tauri dev
```

### 확인

```powershell
cd src-tauri; cargo test; cargo clippy --all-targets; cd ..
npx tsc --noEmit
```

### 배포 체크리스트

`npm run tauri build` 는 NSIS 설치 파일 하나(`Addiem_<버전>_x64-setup.exe`)만 만든다. 현재 사용자에게 설치하고(`%LOCALAPPDATA%\Addiem`, 관리자 권한 없음) 설치 화면은 한국어다(Windows 표시 언어가 영어면 영어). 첫 배포 전에 확인할 것:

- [ ] **코드 서명**: 인증서(OV · EV 또는 Azure Trusted Signing)를 준비하고 `bundle.windows` 에 `certificateThumbprint` + `digestAlgorithm: "sha256"` + `timestampUrl`, 또는 `signCommand`(signtool 등)를 넣는다. 서명이 없으면 SmartScreen 경고가 뜬다. 확인은 `Get-AuthenticodeSignature <exe>`. **지금은 서명 없이 배포한다 (사용자 결정 2026-10-20, 공개 배포 전에 다시 정한다).** 설치하는 사람에게 함께 알릴 것:
  - 설치 파일이나 앱을 처음 열 때 "Windows의 PC 보호" 파란 창이 뜨면 **[추가 정보] → [실행]** 을 누른다.
  - 백신(Defender)이 서술형 개념 관계 확인 엔진(`llm\addiem-llm.exe`)을 막거나 지우면 그 기능만 꺼지고(설정 › 서술형 채점에 "이 설치에는 엔진이 없어요"), 핵심어 채점과 나머지 기능은 그대로다.
- [ ] **게시자 이름**: `bundle.publisher`(지금 `itisyijy`). 프로그램 추가/제거의 게시자, exe 의 CompanyName, NSIS 레지스트리 키 `HKCU\Software\<게시자>\Addiem` 가 이 값에서 나온다. NSIS 는 이 키로 이전 설치 폴더를 찾으니 첫 배포 전에 정하고 그 뒤로는 바꾸지 않는다. 데이터 폴더 · 키체인 이름은 `identifier` 에서 나오므로 영향이 없다. `bundle.copyright` 는 `© 2026 itisyijy`.
- [x] **아이콘**: Addiem 아이콘 (만드는 방법은 `design/logo/README.md`). 설치 파일 아이콘은 `bundle.windows.nsis.installerIcon`. 이름을 바꾸기 전 설치는 NSIS 훅 `src-tauri/windows/hooks.nsh` 가 데이터를 남기고 먼저 지운다.
- [ ] **Google OAuth**: 배포 빌드를 하는 셸에 `ADDIEM_GOOGLE_CLIENT_ID` · `ADDIEM_GOOGLE_CLIENT_SECRET` 을 넣는다. 빠져도 빌드는 성공하고 Drive 연결만 꺼지니(`option_env!`) 빌드 전에 확인한다. OAuth 동의 화면을 테스트에서 프로덕션으로 게시한다(테스트 상태에서는 등록한 테스트 사용자만 로그인된다).
- [ ] **읽어주기 음성**: `tools\fetch_tts.ps1` 로 `src-tauri/resources/tts/` 의 11개 파일을 채운 뒤 빌드한다. 비어 있어도 빌드는 성공하고 OS 음성으로만 읽는다.
- [ ] **개념 관계 확인 엔진**: `tools\build_llm.ps1` 로 `src-tauri/resources/llm/addiem-llm.exe` 를 만든 뒤 빌드한다(없으면 릴리스 빌드가 멈춘다). 엔진 exe 도 본체처럼 코드 서명하지 않으면 SmartScreen · Defender 가 막을 수 있다.
- [ ] **WebView2**: `bundle.windows.webviewInstallMode` 는 기본값 `downloadBootstrapper` 다. WebView2 가 없는 PC(일부 Windows 10)는 설치 중에 인터넷이 필요하다. 오프라인 배포라면 `embedBootstrapper` 또는 `offlineInstaller`.
- [ ] **업데이트**: 자동 업데이트는 없다(`tauri-plugin-updater` 안 씀). 새 버전은 설치 파일을 다시 받아 설치한다. 버전은 `tauri.conf.json` · `Cargo.toml` · `package.json` 을 같이 올리고, 설정 화면 맨 아래에 보인다.

### 구조

| 경로 | 내용 |
|---|---|
| `src/screens/` | 화면. [Figma](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx) 화면 24장과 이름이 같다 (Home = A1·A2, Deck = C1, Study = D1~D4 …) |
| `src/ui.tsx`, `src/styles/` | Figma 컴포넌트와 디자인 토큰 |
| `src-tauri/src/` | 데이터 계층. Rust: `db` 스키마, `queue` 오늘의 큐, `srs` FSRS, `render` Anki 템플릿, `import` 가져오기, `commands` 화면이 부르는 명령 |
| `db/schema.sql` | 데이터 모델 정본 |
| `src-tauri/llm/` | 8.3 개념 관계 확인 엔진 (보조 실행 파일, 본체와 따로 AVX2 로 빌드) |
| `tools/` | Python 참조 구현. Rust로 옮긴 원본. `fetch_tts.ps1` 은 읽어주기 음성 채우기, `build_llm.ps1` 은 개념 관계 확인 엔진 빌드 |

### Python 도구 사용

```powershell
.venv\Scripts\Activate.ps1      # 이후 python 은 가상환경의 것
```

## 도구

```bash
python tools/import_reference.py --self-test     # 스키마 자체 검사
python tools/import_reference.py <파일.colpkg>    # Anki 패키지 -> Addiem 스키마
python tools/import_reference.py <파일.json>      # JSON 일괄 가져오기
python tools/queue_reference.py                  # 큐 알고리즘 테스트
python tools/queue_reference.py <적재된.sqlite>   # 실제 데이터로 큐 구성
```

가상환경을 활성화한 상태에서 실행한다. `tools/` 는 스파이크 산출물이고, 실제 앱은 ADR 0001에 따라 Rust(Tauri)로 만든다.
