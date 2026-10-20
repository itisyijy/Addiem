# 실제 Anki(pylib) 검증 하네스

Neo-Anki 의 .apkg 가져오기 · 덱 업데이트 · 내보내기 · 왕복을 **실제 Anki 의 Rust 백엔드(pylib `anki` 26.9.3)** 와 맞대어 본다. Anki 가 기준이다 (특수 필드 · 렌더 의미).

## 한 명령으로 다시 돌리기

```powershell
$repo = "C:\path\to\Neo-Anki-worktree"   # 합친 브랜치의 worktree (메인 체크아웃 말고)
& "$env:TEMP\na-anki-venv\Scripts\python.exe" -I "$repo\tools\qa-real-anki\real_anki.py" all --repo $repo --work "$env:TEMP\na-qr-work"
```

- `--target` 로 CARGO_TARGET_DIR 를 고른다 (기본 `%TEMP%\na-qr-target`). `CARGO_BUILD_JOBS` 는 없으면 4.
- `--work` 폴더는 **비우고** 시작한다. 결과: `report.md` (판정표), `checks_*.json` (판정 · 근거), `du_summary.json` · `rt_update_summary.json` (덱 업데이트 요약 원문), 만든 패키지 `v1_*.apkg` · `v2_modern.apkg` · `neo_export.apkg` · `anki_rt.apkg`, Neo 스크래치 DB `neo_*\`.
- FAIL 이 하나라도 있으면 종료 코드 1. 판정은 PASS · FAIL · INFO(설계상 다른 것, 판정 없음) 셋이다.
- 단계만 따로: `make` · `anki` · `report` (`--work` 만), Rust 단계는 `cargo test --lib qa_real_anki::phase1 -- --ignored --exact --nocapture` (`NA_QR_WORK` 필요).

## venv

`%TEMP%\na-anki-venv` (Python 3.12, `pip install anki==26.9.3`, PyPI). 2026-10-19 사용자 허락으로 만들었다. 없어졌으면 다시 만들기 전에 사용자에게 묻는다.
설치한 패키지는 믿지 않는다: 스크립트는 venv 밖에 두고 `python.exe -I` 로, 경로는 인자로 넘긴다. 개인 컬렉션은 쓰지 않는다 (모든 데이터를 여기서 만든다).

## 흐름

| 단계 | 누가 | 하는 일 |
|---|---|---|
| make | pylib | 컬렉션을 만들고 (FSRS 켬) Basic · 앞뒤 · Cloze(c1 여러 개 · 힌트) · type-in · Image Occlusion · 사용자 정의 유형(`{{Tags}}` `{{Deck}}` `{{Subdeck}}` `{{Card}}` `{{Type}}` `{{text:}}` `{{hint:}}` 조건부, 카드 종류 둘) · 미디어(그림 · 소리) · 덱 3단 · 태그 · 7번 공부 · 일시 정지 1장 → `v1_modern`(anki21b) · `v1_legacy`(anki21) · `v1_anki2`(anki2 하나로 다시 묶음). Anki 렌더 기준 `ref_v1.json`. 만든 사람의 편집(값 · 태그 · 충돌 · 합침 · 노트 삭제 · 새 필드 · 이름 바뀐 필드 · 새 카드 종류 · 빈칸 번호 바꿈 · CSS · 템플릿 · 새 노트) → `v2_modern` |
| phase1 | Rust (`src-tauri/src/qa_real_anki.rs`) | 세 형식 가져오기(기록 옮기기) · 같은 파일 다시 가져오기, v1 위에 내 편집 · 노트 삭제 · CSS · 공부(앱 함수) → `inspect_update` · `apply_update` v2 판정 · 일정 · 기록 · 빈칸 번호 · 멱등, 앱에서 만든 노트 하나 더해 내보내기 (`neo_state.json`) |
| anki | pylib | 1: Neo 상태와 Anki 기준 비교 (노트 · 필드 · 태그 · 유형 · 미디어 sha1 · 상태 · 기한 · FSRS · 기록 · 덱 · 렌더). 3: `neo_export.apkg` 를 새 컬렉션에 가져와 같은 비교. 4: Anki 에서 노트 2개 고치고 2장 공부 · 새 노트 → `anki_rt.apkg` |
| phase2 | Rust | `anki_rt.apkg` 를 같은 시작점 두 벌에: 새 노트만 더하기(기록 옮기기, I79) · 업데이트로 가져오기 |
| report | | `report.md` |

## 고칠 때

- 렌더는 `qa_real_anki.rs` 의 `neo_render` 한 곳만 부른다 (`render_fields_with` + `render::special_fields`, 학습 화면 card_view 와 같다). 모양이 바뀌면 거기만 고친다.
- 특수 필드 탐침(`qa-tags` … `qa-text`)은 Anki 값과 같으면 PASS. 힌트는 값이 있고 · 처음엔 숨겨지고 · 눌러 여는 것이 있으면 같다고 본다 (문구 · id 는 달라도 된다).
- 덱 업데이트 2단계는 phase1 에서 본다: `DU-p2-field` (복사본에서 충돌 둘 중 한 필드만 새 버전, 적용 = 사전 검사), `DU-p2-orig` (가져온 노트의 원본 글 `"v"` = Anki v1 값, 충돌 화면의 원본, 적용 뒤 기준 = v2), `DU-p2-undo` (적용한 DB 복사본에서 새 카드 하나를 공부한 뒤 `deck_update::undo`: 이미 있던 노트 · 카드 · 기록 = 적용 전 + 그 공부, 공부한 새 카드는 남고 나머지는 보관). Anki 복습 기록을 업데이트에 합치는 것은 미뤘다 (DU56).
