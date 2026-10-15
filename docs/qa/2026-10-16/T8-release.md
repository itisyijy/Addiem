# T8 배포 빌드 · 설치 파일 (2026-10-16)

대상: `%TEMP%\na-qa` (qa/integration 5e38699) · TTS 리소스 11개(`src-tauri\resources\tts`, fetch_tts.ps1 고정 SHA-256과 모두 같음).
설치는 하지 않았다. 설치 파일은 열어서 내용만 봤고(Bandizip `bz.exe`로 NSIS, `msiexec /a` 관리자 추출로 MSI), 실행은 NSIS 안의 파일을 그대로 풀어 둔 폴더(= 설치했을 때와 같은 배치)에서 포터블로 했다.
측정하는 동안 다른 트랙의 neo-anki 인스턴스 8개와 빌드가 같이 돌고 있었다. 시간 · 메모리 수치는 부하가 있는 상태에서 잰 값이다.

## 발견

| ID | 심각도 | 항목 | 재현 | 기대 | 실제 | 근거 경로 |
|---|---|---|---|---|---|---|
| T8-01 | 중간 | 게시자(Publisher) 이름이 "github"로 나옴 | `tauri.conf.json`에 `bundle.publisher`가 없는 상태로 `tauri bundle` 실행 → MSI Property·NSIS 스크립트·exe 버전 정보 확인 | 게시자가 개발자 이름(예: itisyijy)으로 나온다 | 식별자 `io.github.itisyijy.neoanki`의 두 번째 조각을 가져와서 `Manufacturer=github`가 됨. 프로그램 추가/제거의 게시자가 "github", exe CompanyName도 "github", 레지스트리 키는 `HKCU\Software\github\Neo-Anki`. NSIS는 이 키로 이전 설치 폴더를 찾기 때문에, 첫 배포 뒤에 바꾸면 업그레이드할 때 이전 설치를 못 찾을 수 있다. 첫 배포 전에 `bundle.publisher` · `copyright`를 정해 두는 게 좋다 | `%TEMP%\na-qa-t8\target\release\wix\x64\main.wxs`(20·64행), `...\nsis\x64\installer.nsi`(35·696행), 의심 파일 `src-tauri/tauri.conf.json` |
| T8-02 | 중간 | 앱 아이콘이 Tauri 기본 아이콘 그대로, 설치 파일은 NSIS 기본 아이콘 | exe·setup.exe 아이콘 추출 | Neo-Anki 고유 아이콘 | `src-tauri/icons/*`는 첫 커밋(8381ad4) 뒤로 바뀐 적이 없는 Tauri 템플릿 로고(노랑·하늘색 고리). `INSTALLERICON ""`이라 setup.exe에는 NSIS 기본 아이콘이 붙음 | `%TEMP%\na-qa-t8\shots\icons-compare.png`(왼쪽부터 앱 exe · setup.exe · icons/128x128.png) |
| T8-03 | 중간 | 코드 서명 설정이 없음 | `tauri.conf.json`에 `bundle.windows`(certificateThumbprint · signCommand · timestampUrl)가 없음. `Get-AuthenticodeSignature` 실행 | 배포용 설치 파일은 서명돼 있다 | exe와 setup.exe 모두 `NotSigned`. 내려받아 실행하면 SmartScreen이 "Windows의 PC 보호" 경고를 띄울 가능성이 높다(지시대로 서명은 하지 않았다) | 의심 파일 `src-tauri/tauri.conf.json` |
| T8-04 | 중간(확인 필요) | 창 최소 크기 1180×760이 흔한 노트북 화면보다 큼 | 설정만 검토함: `app.windows[0].minWidth 1180 / minHeight 760` | 작업 영역에 창이 다 들어간다 | 1920×1080을 150% 배율로 쓰면 논리 해상도가 1280×720이라 최소 높이 760을 넘는다. 1366×768을 100%로 써도 작업 표시줄을 빼면 약 728이다. 그러면 창 아래쪽(평가 버튼 등)이 화면 밖으로 나갈 수 있다. 실제 배율은 시스템 설정이라 바꿔 보지 않았다. T6는 CDP 뷰포트만 바꿔서 이 상황을 잡지 못한다 | 의심 파일 `src-tauri/tauri.conf.json` |
| T8-05 | 낮음 | 설치 프로그램 화면이 영어뿐 | MSI `-cultures:en-us`, ProductLanguage 1033. NSIS `MUI_LANGUAGE "English"` | 앱이 한국어이니 설치 화면도 한국어 | 설치 · 제거 화면, 마지막 "Launch Neo-Anki" 체크박스까지 모두 영어. `bundle.windows.wix.language` / `nsis.languages`로 바꿀 수 있다 | `...\nsis\x64\installer.nsi`(464행), `logs\bundle2.log` |
| T8-06 | 낮음 | `targets: "all"`이라 설치 범위가 다른 설치 파일 두 개가 나옴 | `tauri bundle` | 계획대로 NSIS 하나 | MSI는 컴퓨터 전체에 설치(ALLUSERS=1, Program Files, 관리자 권한 필요), NSIS는 현재 사용자에게 설치(%LOCALAPPDATA%). 둘 다 설치하면 앱이 두 벌 생긴다. 번들 시간과 산출물도 두 배(약 740MB) | MSI Property 표(아래 메타데이터), 의심 파일 `src-tauri/tauri.conf.json` |
| T8-07 | 낮음 | 앱 안에 버전 표시가 없음 | 설정 화면 전체 텍스트 확인, `src` 아래에서 version을 찾아봄 | 설정 어딘가에 "버전 0.1.0" | 백엔드는 `AppInfo.version`을 돌려주지만(`commands.rs:2318`) 화면 어디에도 그리지 않는다(`src/api.ts:363` 타입에만 있음). 문의나 지원할 때 버전을 알 수 없다 | `src/api.ts:363`, `src/screens/Settings.tsx` |
| T8-08 | 낮음 | 음성 파일 없는 빌드에서 가져오기 안내 문구가 프랑스어를 "다른 언어"라고 부름 | 음성 파일 없는 exe로 `tts-test.apkg` 가져오기(voices=ko,en) | "이 기기에 프랑스어 음성이 없어…" | 안내는 "이 기기에 **다른 언어** 음성이 없어 읽어주기 2장은 소리 없이 보여요"인데 카드 칩은 "이 기기에 프랑스어 음성 없음"이라 서로 다르다. `no_voice()`는 내장 음성에 없는 언어만 이름으로 부른다. 리소스를 빼고 만든 빌드(개발 · CI)에서만 생긴다 | `src-tauri/src/tts.rs:59-73`, `shots\no-tts-answer.png` |
| T8-09 | 낮음(참고) | `NEOANKI_GOOGLE_CLIENT_ID` 없이 만든 설치 파일은 Drive를 쓸 수 없음 | 이번 release 빌드로 설정 화면 열기 | 배포 빌드에는 클라이언트 ID가 들어간다 | "이 앱 버전에서는 Google Drive 연결을 사용할 수 없어요 · Google Drive 연결이 설정된 배포 버전이 필요해요"가 뜬다. 안내는 정상이다. 다만 빌드할 때 값이 빠졌는지 막아 주는 장치가 없고(`option_env!`), README에는 `tauri dev` 예시만 있다. 배포 체크리스트에 넣을 것 | `src-tauri/src/sync/auth.rs:15-16`, `README.md:98-104` |
| T8-10 | 정보 | 자동 업데이트 없음 | `plugins.updater` 없음, `tauri-plugin-updater` 의존성 없음 | — | 새 버전은 사용자가 직접 받아 다시 설치해야 한다(지금 계획 범위라면 그대로 둬도 됨) | `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` |
| T8-11 | 정보 | WebView2 설치 방식은 `downloadBootstrapper`(/silent, 기본값) | 생성된 NSIS · WiX 스크립트 | — | WebView2가 없는 PC(일부 Windows 10)에서는 설치 중에 인터넷이 있어야 한다. 오프라인 설치 옵션은 없다. Windows 11에는 기본으로 들어 있다(이 PC 155.0.4283.45) | `installer.nsi` 56행, `main.wxs` 201-211행 |
| T8-12 | 정보 | WiX ICE 경고 4개 | `light.exe` 출력 | — | ICE03(DownloadAndInvokeBootstrapper 문자열 길이 초과), ICE40(REINSTALLMODE), ICE57, ICE61. 모두 Tauri 기본 템플릿에서 나오는 것이고 MSI를 계속 낼 때만 신경 쓰면 된다 | `%TEMP%\na-qa-t8\logs\bundle2.log` |
| T8-13 | 정보(QA 환경) | 공용 target 폴더에서 `npm run tauri build`가 마지막 단계에서 실패함 | `CARGO_TARGET_DIR=%TEMP%\na-qa-target`에서 `npm run tauri build` 실행 | 빌드와 번들이 끝까지 된다 | 컴파일(LTO)은 끝났지만 `failed to remove file ...\na-qa-target\release\neo-anki.exe (os error 5)`로 멈췄다. 다른 트랙이 이 exe를 target 폴더에서 바로 띄워 둔 상태였다(PID 111084, 00:59:05 시작). 같은 폴더에서 `tauri bundle`을 다시 해도 번들러가 exe를 그 자리에서 고쳐 쓰려다 os error 32로 실패한다. 그래서 새로 만든 `deps\neo_anki.exe`를 `%TEMP%\na-qa-t8\target\release`에 복사해 두고 거기서 `tauri bundle`을 돌렸다. 공용 target에 남은 `release\bundle\msi\Neo-Anki_0.1.0_x64_en-US.msi`는 실패한 시도의 산출물(00:48 exe, 번들 종류 표시 실패)이니 쓰지 말 것. 앞으로는 트랙마다 exe를 복사해서 실행하라고 안내할 것 | `%TEMP%\na-qa-t8\logs\build.log`, `logs\bundle.log` |

## 크기 · 시간

| 항목 | 값 |
|---|---|
| NSIS `Neo-Anki_0.1.0_x64-setup.exe` | 384,280,813 B (366.48 MiB) · LZMA solid, 압축률 87.5% (원본 438,982,370 B) |
| MSI `Neo-Anki_0.1.0_x64_en-US.msi` | 390,332,416 B (372.25 MiB) |
| `neo-anki.exe` (release, strip · LTO) | 22,393,344 B |
| TTS 리소스 11개 합계 | 416,266,982 B (vector_estimator.onnx 하나가 256.5MB) |
| 설치 후 예상 크기 (NSIS EstimatedSize) | 428,379 KB |
| `npm run tauri build` (vite 빌드 + LTO 증분 컴파일, 마지막 복사에서 실패) | 400.5 s |
| `tauri bundle` MSI + NSIS (처음 쓰는 WiX 3.14 · NSIS 3.11 · nsis_tauri_utils 받기 포함, SHA 확인됨) | 206.5 s |
| NSIS 풀기 (Bandizip) / MSI 관리자 추출 | 8.2 s / 2.9 s |
| 시작 1회차 (빈 데이터 · 빈 WebView2 프로필) | CDP 열림 364 ms · 홈 DOM 762 ms · FCP 883 ms |
| 시작 2회차 (덱 1개, 프로필 재사용) | 355 / 618 / 661 ms |
| 시작 3회차 | 346 / 577 / 615 ms |
| 메모리, 홈에서 쉬는 상태 (앱 + WebView2 7개 프로세스) | 작업 집합 389.5–397.4 MB · 전용 210–229 MB (앱 프로세스만은 약 32 MB / 7 MB) |
| 메모리, 읽어주기 한 번 한 뒤 | 앱 프로세스 477.6 MB / 460.6 MB · 전체 작업 집합 883.6 MB |
| 읽어주기 첫 합성 (엔진 올리기 포함, 한국어 2문장) / 두 번째 (영어 1문장) | 1,972 ms / 514 ms (WAV 2.25초) |

시간은 앱을 띄운 시각(node `Date.now()`)부터 잰다. "홈 DOM"은 `main` 요소가 처음 보인 때(25 ms마다 확인), FCP는 `performance.timeOrigin + first-contentful-paint`이다. 측정 기록: `%TEMP%\na-qa-t8\logs\startup-runs.jsonl`.

## 메타데이터

- tauri.conf.json: productName `Neo-Anki`, version `0.1.0`(Cargo.toml · package.json과 같음), identifier `io.github.itisyijy.neoanki`, bundle targets `all`, resources `resources/tts/ → tts/`.
- CSP(release): `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src ipc: http://ipc.localhost; media-src 'self' data:; object-src 'none'; base-uri 'none'; frame-src 'none'; form-action 'none'`. 실행 중에 인라인 스크립트를 넣어 보니 막혔다(`script-src-elem` 위반, 실행 안 됨). 실제 헤더에는 Tauri가 sha256 해시를 붙인다. devCsp는 dev에서만 쓴다.
- exe 버전 정보: FileDescription/ProductName `Neo-Anki`, File/ProductVersion `0.1.0`, CompanyName `github`, LegalCopyright 비어 있음, 서명 없음. setup.exe도 같고 CompanyName은 비어 있음.
- MSI: ProductName Neo-Anki · ProductVersion 0.1.0 · Manufacturer github · ALLUSERS=1 · ProductLanguage 1033 · UpgradeCode {E26B76A1-9DC5-5174-824B-0809B91AB7D4} · ARPPRODUCTICON 있음.
- NSIS: INSTALLMODE currentUser · INSTALLWEBVIEW2MODE downloadBootstrapper · INSTALLERICON/UNINSTALLERICON 없음 · COPYRIGHT 없음.
- 업데이트 설정 없음, 코드 서명 설정 없음.

## 통과한 것

- MSI와 NSIS 둘 다 만들어진다(T8-13 우회 후). 두 설치 파일 모두 TTS 11개 파일(모델 7개 · onnxruntime.dll · 라이선스 3개: Supertonic LICENSE, ORT LICENSE, ThirdPartyNotices)이 `tts\…` 아래에 있고, SHA-256이 원본 · fetch_tts.ps1 고정값과 11개 모두 같다. 앱 exe와 같은 폴더(리소스 폴더) 아래에 놓인다.
- 설치 파일 안의 exe는 빌드된 exe와 3바이트만 다르다(번들 종류 표시 `UNK` → `NSS`/`MSI`). 나머지는 같다.
- `onnxruntime.dll`은 앱 리소스 폴더에서만 올라온다: 읽어주기 전에는 0개, 한 번 읽은 뒤 `…\extract\nsis\tts\onnxruntime\onnxruntime.dll`(1.31.0.20261008) 하나뿐이다. System32에 있는 1.17(Windows ML)은 올라오지 않았다.
- 음성 파일이 있는 빌드: `tts_langs` 31개, 설정에 "오픈소스 라이선스" 섹션이 보이고 전문도 읽힌다(OpenRAIL-M 14,959자, ORT MIT + ThirdPartyNotices 339,126자). 칩은 영어 · 한국어 · 프랑스어가 내장 음성, 중국어만 "이 기기에 중국어 음성 없음"(비활성, 안내 툴팁). 프랑스어 칩을 누르면 읽는 중 → 끝으로 돌아온다(OS 음성은 쓰지 않음).
- 음성 파일이 없는 빌드(exe만 `%TEMP%\na-qa-t8\no-tts\`에 복사): `tts_langs`가 `[]`, `tts_speak`·`tts_preload`는 "읽어주기 음성 파일이 없어요.", `tts_license`는 "라이선스 파일이 없어요."로 깔끔하게 실패한다. 칩은 OS 음성으로 넘어간다(Heami ko-KR, Mark/Zira/David en-US). 프랑스어 · 중국어는 "이 기기에 … 음성 없음" 점선 칩이다. 한국어 칩을 누르면 `speechSynthesis`로 읽고 끝나면 상태가 원래대로 돌아온다. 라이선스 섹션은 숨겨진다. 콘솔 오류 · 예외는 0개.
- 음성 파일이 있는 빌드에서도 콘솔 오류는 일부러 넣은 CSP 확인용 1건 말고는 없다. 앱 stdout/stderr 로그는 비어 있다.
- 홈 첫 화면이 0.6–0.9초 안에 뜬다(부하가 있는 상태에서).
- `msiexec /a` 관리자 추출은 제품을 등록하지 않았다(Uninstall 키도 `HKCU\Software\github\Neo-Anki`도 없음). 실제 %APPDATA%는 건드리지 않았다(데이터는 `%TEMP%\na-qa-t8\data`, `no-tts-data`).

## 못 해 본 것 (이유)

- 실제 설치 · 제거 · 업그레이드, 시작 메뉴 · 바탕 화면 바로 가기, 설치 중 WebView2 부트스트래퍼 받기, SmartScreen 경고: 이 PC에 설치하지 않는다는 원칙 때문.
- 공용 target에서 `npm run tauri build`를 한 번에 끝까지 성공시키기: 다른 트랙이 target 안의 exe를 실행 중이라 막혔다(T8-13). 같은 빌드 결과물로 따로 번들했다.
- 고배율(150%) · 작은 화면에서 창 최소 크기 확인(T8-04): 디스플레이 배율은 시스템 설정이라 바꾸지 않았다.
- 소리를 실제로 들어 보기: 재생 상태가 바뀌는 것과 WAV 길이만 확인했다.
- 조용한 PC에서 잰 시작 시간 · 메모리: 다른 QA 트랙이 같이 돌고 있었다.
- 코드 서명: 지시대로 하지 않았다(설정이 없다는 것만 보고).

## 산출물 위치

- 설치 파일: `%TEMP%\na-qa-t8\target\release\bundle\nsis\Neo-Anki_0.1.0_x64-setup.exe`, `...\bundle\msi\Neo-Anki_0.1.0_x64_en-US.msi`
- 생성된 스크립트: `%TEMP%\na-qa-t8\target\release\nsis\x64\installer.nsi`, `...\wix\x64\main.wxs`
- 풀어 둔 내용: `%TEMP%\na-qa-t8\extract\nsis`, `%TEMP%\na-qa-t8\extract\msi\PFiles\Neo-Anki`
- 로그: `%TEMP%\na-qa-t8\logs\` (build.log, bundle.log, bundle2.log, msi-admin.log, startup-runs.jsonl, *-console.json, src-sha.json)
- 캡처: `%TEMP%\na-qa-t8\shots\` (tts-answer.png, no-tts-answer.png, icons-compare.png)
- 참고: Tauri 번들러가 처음 쓰면서 `%LOCALAPPDATA%\tauri\`에 WiX 3.14와 NSIS 3.11을 받아 두었다(github.com/wixtoolset · tauri-apps/binary-releases, 해시 확인됨. 설치 프로그램은 아니고 도구 캐시다).
