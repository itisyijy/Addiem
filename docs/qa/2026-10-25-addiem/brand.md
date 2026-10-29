# QA · Addiem 로고 교체 (PR #57)

- **날짜**: 2026-10-25 · **브랜치**: `feat/addiem-brand` (fa710a8)
- **목적**: 로고·이름 교체로 생긴 시인성 문제와 기능 문제를 찾는다. 이 문서는 찾기만 하고, 수정은 따로 한다.

## 검사 환경

| 항목 | 값 |
|---|---|
| 화면 검사 빌드 | `npm run tauri dev` (debug) |
| 설치 검사 빌드 | `NEOANKI_ALLOW_NO_LLM=1 npm run tauri build`. QA 전용이라 분석 모델 엔진을 뺐다. 엔진이 없으면 릴리스 빌드는 실패하도록 되어 있다 (`build.rs:21`) |
| 데이터 폴더 | `%TEMP%\na-qa-brand\data`. WebView2 프로필은 `wv` (dev), `wv-installed` (설치본) |
| 동기화 · AI | `NEOANKI_SYNC_FOLDER=%TEMP%\na-qa-brand\sync`, `NEOANKI_AI_FAKE=ok`. 실제 로그인은 하지 않았다 |
| 원격 디버깅(CDP) | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9450` |
| 드라이버 | Node 24 내장 WebSocket으로 직접 작성 (스크래치패드 `qa-brand/cdp.mjs`). 대비는 캡처 PNG의 실제 픽셀에서 잰다 |
| 샘플 데이터 | `import_json`으로 넣었다. 덱 3개 (빅데이터분석기사 + 하위 덱 1개, 시험일 +12일 · 정보처리기사 · 토익 영단어), 카드 60장 |
| 캡처 | `%TEMP%\na-qa-brand\shots` |
| 화면 | 2560×1440, 배율 100% (96 DPI) |

## 발견한 문제

| ID | 심각도 | 화면 | 재현 | 기대 | 실제 | 의심 위치 | 캡처 |
|---|---|---|---|---|---|---|---|
| QB-01 | 높음 | 설치 · 예전 앱 제거 | 예전 Neo-Anki가 설치된 PC에 Addiem을 설치한 뒤, 예전 Neo-Anki를 지우면서 '앱 데이터 삭제'를 체크한다 | Addiem 데이터는 그대로 남는다 | 두 앱이 같은 identifier(`io.github.itisyijy.neoanki`)를 쓴다. 제거 프로그램은 `RmDir /r "$APPDATA\${BUNDLEID}"` · `"$LOCALAPPDATA\${BUNDLEID}"`를 지운다 (Tauri NSIS 템플릿). 그래서 **Addiem 데이터도 같이 지워진다**. 실행하지 않고 템플릿 코드로 확인했다 | `src-tauri/tauri.conf.json` (`productName`만 바뀌고 identifier는 같다) | — |
| QB-02 | 중간 | 설치 | 설치하면 Addiem이 예전 Neo-Anki를 덮어쓰지 않는다 | 예전 앱을 대체하거나 정리한다 | `%LOCALAPPDATA%\Addiem`에 따로 설치된다. 시작 메뉴에 'Addiem'과 'Neo-Anki'가 둘 다 생기고, 둘 다 `neo-anki.exe`를 실행하며 같은 데이터를 쓴다. 두 앱을 동시에 띄우면 같은 DB를 함께 쓴다 (실행해 보지는 않았다) | `tauri.conf.json` · NSIS 설정 | — |
| QB-03 | 중간 | 고대비 테마 | 앱 테마를 '밝게'로 고정하고 Windows 고대비(검정)를 켠다. 반대 조합(앱 '어둡게' + Windows 고대비 흰색)도 같다 | 워드마크가 보인다 | 상단 메뉴 배경은 시스템 색(검정·흰색)으로 바뀌는데 SVG `fill`은 그대로다. 그래서 글자가 **약 1.1:1로 거의 안 보인다**. 점과 i 점만 보인다. 앱 테마와 Windows 테마가 같은 쪽이면 문제없다 (17.8:1 · 18.1:1). 예전 글자 로고는 Windows가 색을 바꿔 줘서 보였으니, 이번 변경으로 생긴 문제다 | `src/ui.tsx` (Wordmark · Logo). `forced-colors`용 CSS가 없다 | B2-forced-app-light-os-dark.png · B2-forced-mismatch-crops.png |
| QB-04 | 중간 | 설치 파일 | 내려받은 `Addiem_0.1.0_x64-setup.exe`를 본다 | Addiem 아이콘 | NSIS 기본 아이콘이다. `installerIcon`이 설정되어 있지 않다 (이전에도 같았지만 이번 브랜드 작업의 범위다) | `tauri.conf.json` `bundle.windows.nsis` | B4-exeicons-x4.png |
| QB-05 | 낮음 | 상단 메뉴 (라이트) | 라이트 테마의 아무 화면 | 주황 점이 3:1 이상 (이번 QA 목표) | 오른쪽 위 점(그날)과 i 점이 **2.07:1**이다. 반전 카드(덱 상세의 '오늘' 카드)가 상단 메뉴 밑으로 지나가면 메뉴가 회색이 되어 **1.31:1**까지 내려간다. 같은 상황에서 대각선 점은 3.58:1이다. 색상 차이로 점이 구분되기는 한다 | `tokens.css` `--brand-day` (#ff8a3d) | B1-home-light.png · B1-deck-light-scrolled.png |
| QB-06 | 낮음 | 창 제목 표시줄 · 16px | 설치본을 실행해 창 제목 표시줄을 본다 | 점 격자가 알아보인다 | 16px에서는 점이 1–2px라 보라색 타일 위 주황 점 하나 정도로만 보인다. 작업 표시줄(24px)은 잘 알아보인다 | `design/logo/addiem-icon-small.svg` | B4-titlebar-x3.png · B4-ico-light.png |
| QB-07 | 낮음 | Drive 연결 (K1) | Google Drive 연결을 처음 연다 | 이름이 한 가지로 보인다 | "Addiem이 만든 파일만 보고 관리하기" 바로 아래에 `"Neo-Anki 백업" 폴더`가 나온다. 폴더 이름을 일부러 그대로 둔 이유가 화면에 설명되어 있지 않다. 개발용 폴더 동기화 모드에서는 이 창이 나오지 않아 코드로만 확인했다 | `src/sync.tsx:496-497` | — |
| QB-08 | 낮음 | 복구 · 백업 | 백업 복구 창, 미디어 포함 백업 저장, 백업 오류 | — | `neoanki-before-restore-….zip`, `neoanki-…-with-media.zip`, `neoanki.sqlite` 같은 예전 이름이 보인다. 앱 데이터와 묶인 이름이라 그대로 둔 것이지만, Addiem 문구 옆에 섞여 보인다 | `sync.tsx:1139` · `:1422` · `commands.rs:2854` | — |
| QB-09 | 낮음 | ChatGPT 연결 | 이미 ChatGPT를 연결해 둔 사용자 | 연결 앱 이름이 Addiem이다 | `agent_name_hint`는 `client_id`가 없을 때(처음 연결)만 보낸다. 등록은 연결을 끊어도 남으므로, 기존 사용자에게는 계속 Neo-Anki로 보일 가능성이 높다 | `src-tauri/src/ai.rs:196` | — |
| QB-10 | 참고 | 상단 메뉴 포커스 | Tab으로 로고 버튼에 간다 | — | 포커스 테두리는 잘 보인다(라이트·다크). 다만 둥근 상단 메뉴 안에 네모 테두리이고, 오른쪽 위 주황 점에 바짝 붙는다 | 전역 `:focus-visible` (`tokens.css`) | B3-focus-both.png |
| QB-11 | 참고 | 작업 관리자 | 설치본 실행 | — | 실행 파일 이름은 여전히 `neo-anki.exe`다 (Cargo 패키지 이름). 파일 설명·제품 이름은 Addiem이다 | `src-tauri/Cargo.toml` | — |
| QB-12 | 참고 | 문서 | — | — | README에 예전 정보가 남아 있다: 설치 파일 이름(`Neo-Anki_0.1.0_x64-setup.exe`, 실제는 `Addiem_0.1.0_x64-setup.exe`), 설치 경로, 예전 로고 스크린샷 12장. `docs/engineering/development.md:148`(아이콘이 Tauri 기본이라는 설명)과 `docs/design/copy-guide.md`도 Neo-Anki다 | 문서 | — |
| QB-13 | 참고 (사용자 확인) | Google 로그인 동의 화면 | — | — | 동의 화면의 앱 이름은 Google Cloud 콘솔의 OAuth 클라이언트 설정에서 나온다. 콘솔이 아직 Neo-Anki라면 앱의 "Addiem이 요청하는 권한" 문구와 어긋난다 | Google Cloud 콘솔 | — |

## 수정 결과 (df91bb3, PR #57)

| ID | 수정 | 확인 |
|---|---|---|
| QB-01 · QB-02 | 설치 전 단계에 NSIS 훅(`src-tauri/windows/hooks.nsh`)을 넣었다. 예전 Neo-Anki가 있으면 그 제거 프로그램을 조용한 모드(`/S`)로 먼저 돌린다. 조용한 모드에서는 '앱 데이터 삭제'가 체크되지 않으므로 데이터는 남는다 | 가짜 예전 앱 'Neo-Anki-QA'로 시험했다. 훅에서 제품 이름만 바꿨다. 예전 앱의 폴더 · 레지스트리 · 시작 메뉴 바로가기가 지워졌고, 두 데이터 폴더에 넣어 둔 표시 파일은 남았다. 공유 WebView 데이터는 오늘 바뀐 파일이 0개였고, 실제 Neo-Anki 설치는 그대로였다 |
| QB-03 | `forced-colors: active`일 때 워드마크와 대각선 점은 CanvasText, 격자 점은 GrayText, 주황 점은 Highlight로 그린다 (`components.css`) | 앱 테마와 OS 테마 조합 4가지 모두에서 글자 대비가 21:1이다. 일반 모드는 그대로다 (15.7:1 · 13.6:1) |
| QB-04 | `bundle.windows.nsis.installerIcon`을 `icons/icon.ico`로 설정했다 | 설치 파일이 Addiem 아이콘으로 보인다 (QB04-setup-icon-x4.png) |

### 2차 수정 (d829633)

| ID | 수정 | 확인 |
|---|---|---|
| QB-05 | 라이트 모드 브랜드 주황을 #E05C06으로 바꿨다(이전 #FF8A3D). 앱 토큰, Figma `brand/the-day`, 라이트 로고 SVG에 모두 반영했다. 앱 강조색 `--accent-warm`은 그대로다 | 주황 점과 i 점이 **3.23–3.25:1**이다(이전 2.07:1). 반전 카드가 메뉴 밑으로 지나갈 때는 2.05:1이다(이전 1.31:1). 메뉴가 반투명이라 이 경우는 아직 3:1 아래다. 다크 모드는 그대로다 |
| QB-06 | 16px 전용 아이콘(`addiem-icon-16.svg`)을 새로 만들었다. 대각선 점 3개만, 픽셀에 맞춰 그렸다. `icon.ico`의 16px을 이것으로 바꿨다 | 16px에서 흰 점 2개와 주황 점 1개가 또렷하다 (QB06-ico-small-light.png) |
| QB-07 | Drive 권한 안내에 "Neo-Anki는 Addiem의 예전 이름이에요"를 덧붙였다 | 코드로 확인했다 (개발용 폴더 모드에서는 이 창이 나오지 않는다) |
| QB-08 | 설정 › 이 기기 백업에 "폴더와 파일 이름의 neoanki는 Addiem의 예전 이름이에요"를 덧붙였다. 복구 오류 문구에서 내부 파일 이름을 뺐다 | 실행 중인 앱의 설정 화면에서 문구를 읽어 확인했다. 오류 문구 테스트도 새 문구로 바꿨다 |
| QB-09 | 저장한 ChatGPT 등록은 'Addiem' 이름으로 한 것일 때만 다시 쓴다(`ai_client_name` 메타). 그 전 등록이면 다음 연결 때 새로 등록해 ChatGPT에 Addiem으로 보이게 한다. 지금 연결은 그때까지 그대로다 | 단위 테스트 `old_registration_is_replaced_on_next_connect`를 추가했다. 실제 ChatGPT 연결은 하지 않았다 |

회귀: tsc · clippy(경고 0) · `cargo test --lib` 351개(새 테스트 1개 포함)가 통과했다.

QB-10 ~ QB-13(참고 항목)은 고치지 않았다.

## 상단 로고 대비 (실제 픽셀 측정, 2배 해상도)

같은 조건에서 시각을 바꿔 3번, 동작 줄이기를 켜고 1번 쟀고, 표에는 가장 낮은 값을 적었다. 로고는 WCAG 적용 대상이 아니라서 기준은 이번 QA 목표다.

| 요소 | 테마 | 글자/점 | 배경 | 대비 | 기준 | 판정 |
|---|---|---|---|---|---|---|
| 워드마크 글자 | 라이트 | #16151f | 메뉴(블롭 위) | 15.7:1 | 4.5:1 | 통과 |
| 워드마크 글자 | 라이트 · 반전 카드 위 | #16151f | #bebec0 | 9.75:1 | 4.5:1 | 통과 |
| 워드마크 글자 | 다크 | #ecebf5 | #231f33 | 13.5:1 | 4.5:1 | 통과 |
| 대각선 점 | 라이트 | #4433ff | 메뉴 | 5.9:1 | 3:1 | 통과 |
| 대각선 점 | 라이트 · 반전 카드 위 | #4433ff | #bebec1 | 3.58:1 | 3:1 | 통과 |
| 대각선 점 | 다크 | #7a6dff | #222130 | 4.11:1 | 3:1 | 통과 |
| 주황 점 · i 점 | 라이트 | #ff8a3d | 메뉴 | 2.07:1 | 3:1 | **미달 (QB-05)** |
| 주황 점 · i 점 | 라이트 · 반전 카드 위 | #ff8a3d | #c1c1c4 | 1.31:1 | 3:1 | **미달 (QB-05)** |
| 주황 점 · i 점 | 다크 | #ff9a55 | #222130 | 7.5:1 | 3:1 | 통과 |
| 격자 점 (꾸밈) | 라이트 | #d9d5ff | 메뉴 | 1.25:1 | 기록만 | 연하지만 격자 모양은 보인다 |
| 격자 점 (꾸밈) | 다크 | #3a3470 | 메뉴 | 1.41–1.48:1 | 기록만 | 연하지만 격자 모양은 보인다 |

## 통과한 것

- **레이아웃**: 1180×760, 1280×800, 1920×1080 모두 같았다.
  - 상단 메뉴 높이 60px, 로고 32×32, 워드마크 81×30
  - 메뉴 높이 기준 세로 중심 차이 0
  - 오른쪽 버튼과 겹치지 않는다 (간격 735px 이상)
- **화면별 표시**: 홈 · 덱 상세 · 학습 시작 전 요약 · 가져오기 결과 · 설정에서 라이트·다크 모두 정상이다 (B1-*.png).
- **테마 전환**: 설정의 테마 전환(밝게 · 어둡게 · 시스템 + OS 변경)에서 로고 색 4가지가 바로 바뀐다.
- **첫 화면**: 다크로 저장해 두고 다시 켜면 DOMContentLoaded 시점에 이미 `data-theme=dark`다. 로고는 그 뒤에 그려지므로 밝은 색이 깜빡이지 않는다.
- **고대비 테마**: 앱 테마와 Windows 대비 테마가 같은 쪽이면 워드마크 17.8:1 / 18.1:1, 대각선 점 5.5:1 / 6.6:1로 잘 보인다.
- **접근성**: 로고 버튼은 role `button`, 이름 "Addiem"(내용에서 읽음)이다. 키보드 Tab으로 도달하고 `:focus-visible` 테두리가 보인다.
- **동작**: 로고를 누르면 홈으로 간다. 가져오기 중(`lockNav`)에는 이동하지 않는다.
- **아이콘 파일**: `icon.ico`에 16/24/32/48/64/256이 다 들어 있다. 16·24·32는 3×3, 48 이상은 4×4다.
- **설치본 파일 정보**: 실행 파일 아이콘은 Addiem이고, 파일 설명·제품 이름은 Addiem, 버전은 0.1.0이다.
- **설치**: `/S`로 조용히 설치했다.
  - 설치 폴더 `%LOCALAPPDATA%\Addiem`
  - '앱 및 기능': Addiem 0.1.0, 게시자 itisyijy, 아이콘 = exe
  - 시작 메뉴 'Addiem' 바로가기
  - 창 제목 'Addiem'. 작업 표시줄 아이콘(100%, 3×3 형태)도 다른 앱 사이에서 잘 알아보인다.
- **삭제**: `/S`로 지웠다.
  - Addiem 폴더, 레지스트리, 바로가기가 모두 지워졌다.
  - QA 데이터, 예전 Neo-Anki 설치(14개 파일)와 바로가기, 공유 WebView 데이터(738개 파일)는 설치 전과 바이트까지 같다.
  - 조용한 삭제에서는 '앱 데이터 삭제'가 체크되지 않는다는 것을 템플릿에서 확인했다.
- **바깥으로 나가는 이름**: Google 로그인 완료 페이지(`auth.rs`)의 제목과 "Addiem으로 돌아가세요", 설정의 버전 표시 "Addiem 0.1.0".
- **남은 문구**: 앱 코드에 의도하지 않은 'Neo-Anki'가 없다 (`git grep`).
- **회귀**: `tsc --noEmit` 통과. 같은 브랜치에서 `npm run build` 통과, `cargo test --lib` 350개 통과. clippy와 전체 cargo test 결과는 아래 '회귀 결과'에 적는다.

## 못 해 본 것

- **복구(시작 오류) 화면**: 일부러 DB를 망가뜨려야 해서 하지 않았다.
- **화면 배율 125% · 150% · 200%**: 시스템 설정이라 바꾸지 않았다. 대신 아이콘 32·48px을 꺼내 판정했다.
- **Google Drive 연결 창(K1)**: 개발용 폴더 동기화 모드에서는 나오지 않아 코드로만 확인했다.
- **실제 Windows 고대비 테마**: 시스템 설정이라 켜지 않았다. CDP로 `forced-colors`를 흉내 내어 검사했다.
- **두 앱 동시 실행**: 같은 DB를 함께 쓰는 위험이 있어 하지 않았다.
- **분석 모델 엔진이 든 정식 릴리스 설치 파일**: 만들지 않았다.

## 회귀 결과

- `npx tsc --noEmit` 통과
- `npm run build` 통과
- `cargo clippy --all-targets -- -D warnings` 경고 0
- `cargo test`: 350 passed · 0 failed · 9 ignored
- PR #57의 GitHub CI: QA를 마칠 때 아직 실행 중이었다. Auto-fix가 켜져 있어 실패하면 알림이 온다.
