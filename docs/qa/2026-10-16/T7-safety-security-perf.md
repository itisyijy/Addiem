# T7 데이터 안전 · 보안 · 성능 (2026-10-16)

- 대상: `qa/integration` 5e38699 의 **release** exe (`na-qa-target\release\neo-anki.exe`, TTS 리소스 포함)
- 환경: Windows 11 Home 10.0.26300, RAM 34 GB. 데이터 폴더 `%TEMP%\na-qa-t7\data-*`, WebView2 프로필 `%TEMP%\na-qa-t7\wv-*`, CDP 9460 (병렬 실행 때만 9465)
- 입력은 전부 로컬에서 만든 것 (`na-qa-t7\scripts\gen*.py`). 앱 조작은 CDP(클릭 · 키 · 화면이 쓰는 것과 같은 `__TAURI_INTERNALS__.invoke`). 외부 유출 확인용으로 `127.0.0.1:9461` 에 요청 기록 서버를 띄웠다 (`ev\egress.log` — 끝까지 요청 0건)
- 증거 경로는 모두 `%TEMP%\na-qa-t7\` 기준. 코드는 고치지 않았다

## 1. 문제 목록

| ID | 심각도 | 기능 | 재현 | 기대 | 실제 | 의심 파일:줄 | 증거 |
|---|---|---|---|---|---|---|---|
| T7-01 | 높음 | 학습 · 큰 미디어 | `<img src="t7img100.png">` (100 MB) 한 장짜리 .apkg 가져오기 → 학습 시작 | 그림이 늦게라도 뜨거나 "너무 큰 파일" 안내, 화면은 계속 동작 | 100 MB: 카드 틀만 보이고 렌더러가 **11분 넘게 멈춤**(CDP 응답 없음, 마우스 · 키 모두 무반응, 강제 종료 말고 방법 없음). 600 MB: 앱 프로세스 사설 메모리 6.4 GB까지 오른 뒤 렌더러 충돌 "이 페이지에 문제가 있습니다 · STATUS_BREAKPOINT". 콘솔에 `IPC custom protocol failed … Unexpected end of JSON input`. 10 MB ≈2초, 20 MB ≈3초, 50 MB ≈5초. 다시 켜도 같은 카드가 먼저 나와 그 덱 학습이 막힌다. `[sound:]` 도 같은 길(`soundLength` → `read_media`)을 탄다 | `src-tauri/src/commands.rs:2226-2231` (`read_media` 가 파일 전체를 base64 data URL 로, 크기 제한 없음) · `src/cardHtml.ts:26-33, 250-262` (data URL 을 HTML 문자열에 넣고 캐시에 영구 보관) · `src/screens/Study.tsx:543-548` (같은 HTML 을 앞면/질문/답 세 번 그림 + `withMath` 재파싱) | `shots/img100-hung-win.png`, `shots/huge-after-crash-win.png`, `ev/one-img10/20/50/100.json`, `logs/img*.log`, `ev/huge-view.json`, `ev/huge.events.jsonl` |
| T7-02 | 높음 | 가져오기 · 미디어 이름 충돌 | 덱 A(`common1.png` = "CONTENT-FROM-DECK-A")를 가져온 뒤 덱 B(같은 이름, 다른 내용) 가져오기 | 내용이 다르면 새 이름으로 저장하고 B 노트의 참조를 바꾼다 (Anki 동작) 또는 최소한 경고 | B 결과 "media=1" · 경고 없음. 파일은 A 내용 그대로, B 카드가 **A 의 그림을 보여 준다**. DB `media` 행도 A 것 하나뿐. B 를 다시 .apkg 로 내보내면 A 그림이 들어간다 | `src-tauri/src/import.rs:1487-1496` (`if !file.exists()` 면 쓰지 않고 `INSERT OR IGNORE`) | `ev/sec-collide.json`, `data-sec/media/common1.png` |
| T7-03 | 중간 | 카드 HTML 살균 · 화면 덮기 | 필드에 `<div style="position:fixed;top:0;left:0;width:100vw;height:100vh;z-index:2147483647;background:rgba(255,0,0,.35)">OVERLAY</div>` → 학습, E(편집) | 카드 내용은 카드 상자 안에만 그려진다 | 창 전체를 덮는다. `elementFromPoint` 로 그만하기 · Again · Good · 되돌리기 자리 모두 그 div. 실제 마우스로 [그만하기]를 눌러도 아무 일 없음. 편집 모달에서도 모달 전체(카드 삭제 · 저장 포함)를 덮는다. 키보드(Esc · 1-4 · Ctrl+Enter)는 동작. 가짜 안내 문구를 앱 위에 띄우는 데도 쓸 수 있다. 스크립트 실행은 없음 | `src/cardHtml.ts:19` (`style` 속성을 CSS 속성 거르기 없이 허용) · 카드 상자에 `contain`/`overflow` 가둠 없음 (`src/screens/Study.tsx` `.study__card-content`, 편집기 `src/screens/AddCard.tsx:660`) | `shots/sec-study-T7XSS-14-a.png`, `shots/sec-overlay-editor.png`, `ev/sec-overlay.json` |
| T7-04 | 중간 | 가져오기 · 압축 폭탄 | ① 99 KB `.colpkg` (collection.anki21b = zstd 로 3 GB 0) ② 2 MB `.apkg` (collection.anki2 = deflate 로 2 GB) 를 고르기만 해도(inspect_anki) 같은 일 | 압축을 푸는 크기에 상한, 넘으면 바로 오류 | 두 경우 모두 앱 프로세스 사설 메모리 **최대 6.2 GB**(1.3초 · 2.5초) 뒤에 오류. 이 PC(34 GB)는 버텼지만 8 GB PC 면 앱이 죽거나 시스템이 느려질 크기. zstd 상한이 4 GB 이고, deflate 항목과 zstd 가 아닌 미디어는 상한이 없다 | `src-tauri/src/import.rs:745` (`4 << 30`), `import.rs:707-712` (`read_entry` 무제한 `read_to_end`), `import.rs:1480` (미디어 zstd 만 512 MB 상한) | `ev/huge-zbomb.json`, `ev/huge-dbomb.json` |
| T7-05 | 중간 | 가져오기 · 임시 파일 | ① T7-04 ② 의 deflate 폭탄 가져오기 ② `p_perf20k.apkg` 가져오는 중 PID 로 강제 종료 → 다시 실행 | 임시 컬렉션은 실패해도 지우고, 남은 것은 다음 실행 때 치운다 | 데이터 폴더에 `import-<pid>.sqlite` 가 **영구히 남는다**: ① 2,147,483,664 B (2 MB 파일 하나로 2 GB) ② 5.4 MB, 재실행 · 재가져오기 뒤에도 그대로. 컬렉션을 쓴 뒤 열기 · 첫 쿼리에서 실패하면(`file is not a database`, `malformed`, `no such table`) `Package` 가 만들어지기 전이라 Drop 정리가 안 된다. 이름이 pid 라 같은 이름이 다시 쓰이지도 않는다 | `src-tauri/src/import.rs:748-760` (tmp 를 쓴 뒤 `?` 로 빠져나감), 시작 시 정리 없음 (`src-tauri/src/lib.rs` setup) | `ev/huge-dbomb.json` + `data-dbomb\import-43528.sqlite`, `ev/kill-kill1.json` (`after_relaunch_dir`) |
| T7-06 | 중간 | 가져오기 · 미디어 이름 | 미디어 이름에 `<>?*"\|`, 제어 문자(`\x07`), NUL, 300자 이름 중 하나가 든 .apkg (`h_dev10~14,18~20`). `h_devmix`: 정상 4개 + `q?.png` 1개 | 그 파일만 건너뛰고 "이름이 올바르지 않아 건너뛰었어요"(media_name)로 알림 | **덱 전체 가져오기 실패**. 문구가 OS 원문: "파일 이름, 디렉터리 이름 또는 볼륨 레이블 구문이 잘못되었습니다. (os error 123)", NUL 은 "strings passed to WinAPI cannot contain NULs". 되돌리기는 정상(노트 · 파일 남지 않음) | `src-tauri/src/commands.rs:2161-2166` (`safe_media_name` 이 `/ \ :` 와 앞 `.` 만 거름), `src-tauri/src/import.rs:1489` (쓰기 오류를 `?` 로 올림) | `ev/sec-import.json` |
| T7-07 | 중간 | 설정 · 백업 복구 | 설정 > 모든 데이터 삭제 (백업 후 삭제) → 같은 백업으로 되돌리려 함 | 설정의 "이 기기 백업"에서 백업 파일로 복구할 수 있다 | 화면에 복구 버튼이 없다. `restore_backup` 은 DB 를 열지 못했을 때만 나오는 Recover 화면에서만 부른다. 지운 직후 사용자는 앱 안에서 되돌릴 방법이 없다 (zip 을 직접 풀어 데이터 폴더에 넣어야 함). 백엔드 복구 자체는 정상(아래 "통과") | `src/screens/Settings.tsx:87-110` (백업 만들기 · 폴더 열기만), `src/screens/Recover.tsx:18` | `shots/safe-wipe-modal.png`, `shots/recover-screen.png`, `ev/safe-*.json` |
| T7-08 | 중간 | 가져오기 성능 (.apkg) | 노트 2만 · 복습 5만 .apkg 를 빈 DB 에, 그리고 노트 2만이 있는 DB 에 하나 더 | 노트 수에 비례 | 빈 DB 17.5초(그중 cards 단계 15.6초), **2만 장 있는 DB 에서는 76.8초** — 제곱으로 늘어난다. 같은 2만 장 JSON 은 2.3초. 노트마다 `SELECT count(*) FROM note WHERE anki_guid = ?` 를 하는데 `note.anki_guid` 색인이 없어 `EXPLAIN` = `SCAN note`. 가져오는 동안 DB 잠금을 쥐고 있다 | `src-tauri/src/import.rs:1338`, `db/schema.sql:83-97` (anki_guid 색인 없음), `import.rs:1613` (`import_revlog` 의 guid 조회도 같은 열) | `ev/prog-prog1.json`, `ev/prog-prog2.json`, `ev/perf-import-second20k.json`, `ev/perf-import-apkg.json` |
| T7-09 | 낮음 | 미디어 이름 · Windows 예약 이름 | `CON`, `NUL`, `COM1`, `LPT1`, `CONOUT$`, `conin$`, `CON.png`, `aux.mp3`, `nul.txt`, `COM¹.png`, RTL 문자 이름(`\u202egnp.exe`) 미디어 | 예약 이름 · 실행 파일 확장자 · 방향 제어 문자는 거르거나 이름을 바꾼다 | 이 Win11 빌드에선 보통 파일로 써짐(`NUL` 만 null 장치로 가서 DB 행만 있고 파일 없음). Windows 10 이하에서는 같은 경로가 콘솔 · 직렬 · 프린터 장치로 열린다(이 PC 로는 재현 불가). `.exe` 등 아무 확장자나 media 폴더에 들어가고, 탐색기에서 `‮gnp.exe` 가 `exe.png` 처럼 보인다 | `src-tauri/src/commands.rs:2161-2166` | `ev/sec-import.json`, `data-sec/media/` 목록 |
| T7-10 | 낮음 | 가져오기 · 조용히 빠지는 것 | `b_badjson.apkg`(models JSON 깨짐), `b_ver99.apkg`(없는 노트타입 · 없는 덱), `b_badmedia.apkg`(media 목록 깨짐), `b_mediamissing.apkg` | 빠진 노트 · 카드 · 미디어마다 이유를 알린다 | badjson: 미리보기는 노트 1 · 카드 1 인데 결과는 0장 "완료", 문제 0건. ver99: 노트타입 없는 노트는 알림 없이 빠짐, 덱 없는 카드는 건너뛰었다고 알리지만 **그 노트는 카드 없이 DB 에 남는다**(note id 46, 화면에 안 보임). media 목록이 깨지거나 항목이 없으면 경고 없이 그림이 "[그림 없음]" | `src-tauri/src/import.rs:820-860` (`if let Ok` 로 오류 무시), 카드 루프의 노트/카드 따로 처리 | `ev/sec-broken.json` |
| T7-11 | 낮음 | 오류 문구 | 잘린 SQLite · 표 없는 컬렉션 · DB 파일 머리 손상 | 한국어 안내 | "database disk image is malformed", "no such table: cards", "file is not a database"(가져오기와 Recover 화면 하단), os error 123 원문 | `src-tauri/src/import.rs:25` (`err`), `src/screens/Recover.tsx:42` (`startup_error` 그대로) | `ev/sec-broken.json`, `shots/recover-screen.png` |
| T7-12 | 낮음 | 가져오기 중 강제 종료 · 미디어 | 미디어 3,000개 .apkg 를 가져오다 media 1000/3000 에서 PID 로 종료 → 재실행 | 되돌린 가져오기의 파일은 남지 않거나 다음 실행에 치운다 | DB 는 깔끔히 되돌아갔지만 **DB 에 없는 파일 1,822개(≈117 MB)** 가 media 폴더에 남음. 다시 가져오면 이미 있는 파일은 다시 쓰지 않으므로, 끊긴 순간 쓰던 파일이 잘린 채 남으면 그대로 쓰일 수 있다(이번엔 잘린 파일 없음). 남은 파일은 백업 zip 에도 들어간다 | `src-tauri/src/import.rs:1455-1497` (`written` 정리는 Err 반환 때만) | `ev/kill-kill2.json` |
| T7-13 | 낮음 | CSV 내보내기 · 수식 주입 | 앞면 `=HYPERLINK("http://…"&A1,"click")`, 뒷면 `+SUM(1,2)`, `@cmd`, `-2+3` 카드 → CSV 내보내기 | `= + - @` 로 시작하는 칸은 `'` 를 붙이는 등 무력화 | 그대로 `"=HYPERLINK(…)"` 로 써짐 → Excel 에서 열면 수식으로 실행 | `src-tauri/src/export.rs:309` | `ev/csv_injection.csv` |
| T7-14 | 낮음 | 카드 목록 · 검색 | XSS 덱 카드 목록, Perf20k 에서 `b>` 검색 | 목록 글자는 보이는 글자만, 검색은 보이는 글자에서만 | 목록에 `<script>`/`<style>` 안의 글자가 나옴("Q0 window.__t7_x1=1", "@import url(…)"), `srcdoc` 속 `<` 때문에 `parent.__t7_x4b=1">` 같은 조각. 검색이 HTML 태그 · 속성까지 찾음: `b>` · `</b` 가 20,000장 전부, `Q1</b` 1장 | `src-tauri/src/render.rs:256` (`strip_tags`), `src-tauri/src/commands.rs:1125, 1178` (원본 필드에 LIKE) | `shots/sec-cards-xss.png`, `ev/search-markup.json` |
| T7-15 | 낮음 | 깊게 중첩된 HTML | `<b><i>` 50,000번 중첩(1.3 MB) 필드 → 학습 | 몇 초 안에 보이거나 잘라서 보인다 | 렌더러가 약 **35초 멈춘 뒤** 표시 (20,000단 `<div>` 는 0.7초) | `src/cardHtml.ts:245-266` (살균 · 직렬화 · 재파싱 반복) | `logs/nest.log`, `ev/one-nest.json` |
| T7-16 | 낮음 | 홈 · 긴 덱 이름 | JSON `deck` 에 공백 없는 5,000자 | 말줄임 또는 이름 길이 제한 | 덱 이름이 오른쪽 "1장" · [학습] 버튼 밑으로 겹쳐 지나감. 가져올 때 길이 제한 없음 | 홈 덱 행 CSS, `src-tauri/src/import.rs:253` (`split_path`) | `shots/sec-home-longdeck.png` |
| T7-17 | 낮음 | 가져오기 결과 문구 | 스크립트를 쓰는 노트타입이 든 .apkg | "스크립트를 써요" | "노트타입 'T7 TPL'이 **스크립트을** 써요" (조사 고정) | `src-tauri/src/import.rs:1540` | `ev/sec-xss.json` |
| T7-18 | 낮음 | 읽어주기 엔진 메모리 | `{{tts en_US:Front}}` 카드를 한 번 띄움 | 쓰지 않으면 풀리거나, 쓸 때만 올린다 | 칩이 보이기만 해도 미리 올리고(재생 안 해도), 앱 프로세스가 38 MB → 약 486 MB 로 늘어 앱을 끌 때까지 그대로 | `src/cardHtml.ts:133` (칩마다 `tts_preload`), `src-tauri/src/tts.rs:101` (`static OnceLock`) | `ev/perf-tts-ui.json`, `ev/perf-perf-tts.json` |

## 2. 성능

데이터: `Perf20k` 덱(하위 덱 10개) 노트 · 카드 20,000 (복습 12,000 · 학습 중 1,000 · 신규 7,000), 복습 기록 50,000, mp3 50개. 시작 시간은 같은 데이터로 연속 3번 (OS 캐시가 데워진 상태 — 재부팅 직후 값 아님).

| 항목 | 측정값 | 메모리 (WS) |
|---|---|---|
| .apkg 가져오기 (2만 + 복습 5만, 빈 DB) | 17.5초 (cards 단계 15.6초, 복습 기록 2.2초) | 앱 최대 45 MB |
| .apkg 가져오기 (2만, 이미 2만 있는 DB) | **76.8초** (T7-08) | — |
| JSON 가져오기 (같은 2만, 3.8 MB) | 2.3초 (읽기 + 가져오기) | 앱 최대 사설 84 MB |
| JSON 300,000 항목 (3만 통과 · 27만 건너뜀) 붙여넣기 → 미리보기 → 가져오기 | 미리보기 1.1초, 가져오기 + 결과 화면 4.9초, 결과 화면 DOM 165개 | WebView2 676 MB |
| 시작 (프로세스 → CDP) | 286 / 291 / 295 ms | — |
| 시작 (프로세스 → 홈에 덱 목록) | 655 / 656 / 704 ms | 앱 38–40 MB, WebView2 359–372 MB, 합 397–412 MB |
| 홈 다시 그리기 (reload) | 165–174 ms | — |
| 덱 상세 열기 | 213–230 ms | — |
| 카드 목록 열기 (20,000 중 100) | 517–524 ms | — |
| 검색 "abc" (15,759장) | 301–319 ms (입력 대기 150 ms 포함), IPC `list_cards` 135 ms | — |
| 검색 "mp3" (15,818장, 소리 파일 이름은 제외됨) | 300–306 ms, IPC 138 ms | — |
| 검색 결과 없음 | 279 ms | — |
| IPC 단독 | home 94 · deck_detail 198 · queue_preview 49 · heatmap 67 · list_cards(빈 검색) 112 · app_info 4 ms | — |
| 학습 시작 (홈 → 학습 전 요약) | 154 ms | — |
| 학습 시작 (요약 → 첫 카드) | 100 ms | — |
| 답 보기 / 평가 → 다음 카드 | 3–7 ms / 27–43 ms (5장) | 탐색 · 학습 뒤 합 ≈530 MB (WebView2 ≈490 MB) |
| 읽어주기 엔진 첫 로드 (`tts_preload`) | 779 ms, 31개 언어 | 앱 40 → 463 MB (사설 10 → 426 MB) |
| 첫 문장 합성 / 두 번째 | 859 ms / 647 ms | 앱 488 MB |
| 화면에서 첫 재생 (새 프로세스, 칩 표시 직후 누름 → 소리 시작) | 1.57초 (칩 표시 72 ms) | 앱 38 → 486 MB, 끌 때까지 유지 |
| 큰 필드 카드 (5 MB 글자) | 약 0.9초 | WebView2 768 MB |
| 큰 그림 카드 10 / 20 / 50 / 100 / 600 MB | ≈2초 / ≈3초 / ≈5초 / 11분 넘게 멈춤 / 렌더러 충돌 (T7-01) | 600 MB 때 앱 최대 사설 6.4 GB |
| 압축 폭탄 (zstd 3 GB / deflate 2 GB) | 1.3초 / 2.5초 만에 오류 | 앱 최대 사설 6.2 GB (T7-04) |

## 3. CSP · 콘솔

- **정상 흐름 (perf · perfjson · safe · kill1 · kill2 · prog · jui): CSP 위반 0, console.error 0, 예외 0.** DevTools Issue `FormEmptyIdAndNameAttributesForInputError`(id/name 없는 입력칸 — 카드 검색칸 · 삭제 확인칸 등)만 있음. 오류가 아닌 린트.
- 악성 덱(T7XSS)에서 나온 것 — 모두 **막힌 시도**, 기록 서버(127.0.0.1:9461) 도달 0건:
  - `img-src` 위반 ×8: 필드 인라인 `style="background:url(http://127.0.0.1:9461/…)"`. 살균기가 `style` 속성을 남기므로 이 경로는 CSP 한 겹만 막고 있다 (CSP 에 외부 img 를 열면 바로 추적 픽셀이 된다)
  - `base-uri 'none'` 위반 ×24: 필드의 `<base href>` 가 살균용 파싱 단계에서 걸림 (DOM 에는 남지 않음)
  - `StylesheetLoadingIssue` ×12: 필드 `<style>@import url(…)</style>` 가 파싱 중 요청 시작 → `net::ERR_ABORTED` (기록 서버에 안 옴, 결과 DOM 에 style 없음)
  - JS 경고 1: "@import rules are not allowed here" (노트타입 CSS 의 `@import` 를 `CSSStyleSheet.replaceSync` 가 버림)
- 큰 미디어: `console.warning` "IPC custom protocol failed, Tauri will now use the postMessage interface instead SyntaxError: Unexpected end of JSON input" (600 MB 그림 응답, T7-01)
- 원본 이벤트: `ev/*.events.jsonl` (Runtime · Log · Audits · Network 차단 기록)

## 4. 통과한 것

- **경로 탈출 없음**: `..\..\`, `../../`, `..`, 절대 경로, `C:x`, `\\?\` UNC, `/root`, `x:ads`, `:evil` ADS, 전각 `＼` `／`, protobuf media 목록 속 탈출 이름, .colpkg 탈출 이름 전부 "이름이 올바르지 않아 건너뛰었어요". 데이터 폴더 밖(`%TEMP%`, `na-qa-t7`, exe 폴더, 사용자 폴더, 바탕화면, `C:\`, `C:\Windows`)에 표식 파일 0개 (`ev/canary-final.json` — 걸린 4개는 모두 data-sec\media 안의 정상 이름). `%2e%2e%5c…` 는 글자 그대로 media 안에 저장
- `[sound:../../x.mp3]`, `[sound:C:\Windows\win.ini]`, `<img src="../../../../Windows/win.ini">`, `file:///`, `\\127.0.0.1\c$\…`(SMB) 모두 살균 · 이름 검사에서 제거, `read_media` 도 같은 검사
- **스크립트 실행 0**: 18가지 필드 페이로드(`<script>`, `onerror`, `javascript:` 링크, iframe/srcdoc, `<svg onload>`, svg 안 script, mXSS(math/mglyph/style, noscript), form/object/embed/base/meta refresh/link, details ontoggle, video/audio onerror, input autofocus onfocus, KaTeX `\href{javascript:}` · `\url` · `\htmlId` · `\htmlData`, data:image/svg+xml 속 스크립트, media 의 svg 파일), 템플릿 안 `<script>` · `onerror` · iframe, 노트타입 이름 · 덱 이름 · 태그 속 HTML, 목록 · 선택형 항목, `{{tts}}` 필드 · 언어 옵션 속 `"><img onerror>` — 학습 앞/뒤, 편집 모달, 카드 목록, 덱 상세, 홈, 가져오기 결과에서 `window.__t7_*` 0개, 카드 DOM 에 위험 태그 · `on*` 속성 · 외부 URL 0개 (`ev/sec-study-T7XSS.json`, `ev/sec-overlay.json`, `ev/sec-study-T7RECALL.json`). 읽어주기 표시는 `data-text`/`data-tts` 에 escape 되어 들어감 (`ev/sec-tts.json`)
- 노트타입 CSS 의 `}` 로 빠져나가기 · `@import` · `@font-face` 외부 주소 · `.card` 밖 `body{background:url()}`: 요청 0건 (scopeCss 가 통째로 버림)
- 깨진 파일: 임의 바이트 · 빈 파일 · 반쯤 잘린 zip → "Anki 패키지(zip) 형식이 아니에요.", 컬렉션 없음 → "패키지 안에 컬렉션이 없어요.", SQLite 아님 → "컬렉션이 SQLite 파일이 아니에요." 모두 기존 데이터 영향 없음
- JSON: `__proto__` / `constructor.prototype` 키 (최상위 · 항목 · importance) — 프런트(`({}).polluted` 등) 오염 없음, 건너뛴 항목 "JSON으로 복사" 뒤에도 없음. 100,000단 중첩 배열은 "JSON 형식이 올바르지 않아요"(재귀 상한)로 끝. 5 MB 필드 0.4초 가져오기. 30만 항목 배열도 UI 가 앞부분만 그려 멈추지 않음
- **백업 → 모두 삭제(UI, '삭제' 입력) → 복구**: 삭제 0.5초, 사용자 표 전부 0(연쇄 삭제 포함), media · sources 폴더 삭제, 백업 zip 6.4 MB 생성. `restore_backup` 뒤 표 행 수 전부 같고, note · card(state/due/interval/reps/lapses) · review_log · deck · media 지문 같고, media/sources 파일 53개 크기 · 해시 같음 (`ev/safe-before/wiped/restored.json`). DB 머리를 망가뜨린 뒤 실행 → Recover 화면 "백업에서 복구" 0.2초, 같은 비교 통과 (`ev/safe-recovered.json`, `shots/recover-*.png`). 예약 이름 미디어(CON · COM1 · LPT1 …)도 백업 · 삭제 · 복구 왕복 그대로 (`ev/sec-before-wipe.json` / `sec-after-restore.json`)
- **가져오는 중 강제 종료**(cards 15000/40000, media 1000/3000 두 번): `PRAGMA integrity_check` ok, `foreign_key_check` 0, 이전 데이터 그대로, 재실행 0.5초에 홈, 같은 파일 다시 가져오기 성공 (`ev/kill-kill1.json`, `ev/kill-kill2.json`)
- 5 MB 필드 · 20,000단 div 카드는 1초 안에 표시. 20,000장 덱의 홈 · 덱 상세 · 목록 · 검색 · 학습 시작 모두 0.6초 안

## 5. 못 해 본 것 (이유)

- Windows 10 에서의 예약 장치 이름 동작(T7-09) — 이 PC 는 Win11 뿐
- 실제 스키마 18 `.anki21b`(protobuf 노트타입) 악성 변형 — 만들 도구가 없어 legacy 스키마 + protobuf media 목록 + zstd 폭탄만 확인 (스키마 18 정상 파일은 T2 몫)
- 파일 선택 창 · 끌어놓기 경로 — 같은 명령(`inspect_anki`/`import_anki`/`read_json_file`)을 IPC 로 직접 불렀다. JSON 은 붙여넣기 칸으로 UI 경로도 확인
- 편집기에 악성 HTML 붙여넣기(클립보드) — 미실시
- 백업 만들기 · 복구 도중 강제 종료 — 미실시 (복구는 DB 를 옮긴 뒤 새 파일을 쓰므로 그 사이에 끊기면 빈 DB 로 열릴 수 있는지 따로 볼 만함)
- 8 GB PC 에서 압축 폭탄 · 큰 미디어가 실제로 앱을 죽이는지 — 이 PC 34 GB 라 메모리 수치로만 추정
- 재부팅 직후(디스크 캐시 없는) 시작 시간
- 큰 미디어가 멈추기 시작하는 정확한 크기 (50 MB 는 됨, 100 MB 는 멈춤)
- AI · Drive 경로 — release 빌드라 범위 밖

참고: 건너뛴 항목 "JSON으로 복사" 를 눌러 이 PC 클립보드에 테스트 JSON 이 한 번 들어갔다. 실행한 앱은 모두 PID 로 껐고 남은 프로세스 없음. 스크립트 `na-qa-t7\scripts\`, 생성 입력 `na-qa-t7\gen\`, 데이터 폴더 `na-qa-t7\data-*`(지워도 됨, data-dbomb 2.1 GB).
