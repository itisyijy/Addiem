# Neo-Anki

Anki가 복습 시점을 관리한다면, Neo-Anki는 시험일까지의 완주 전략을 관리합니다.

Neo-Anki는 시험 목표와 카드 품질을 함께 관리해, 사용자가 제한된 시간 안에 핵심 내용을 효율적으로 암기하도록 돕는 Anki 호환 학습 시스템입니다.

## Documentation

- [Anki 장기 사용자 인터뷰](docs/user-research/anki-power-user-interview.md)
- [Neo-Anki 제품 방향](docs/product/product-direction.md)
- [Neo-Anki MVP 범위](docs/product/mvp-scope.md)
- [Neo-Anki MVP 화면 흐름](docs/product/mvp-screen-flow.md)
- [MVP 다음 단계: 원자료 기반 중요도 판정](docs/product/post-mvp-source-analysis.md)

### 설계

- [ADR 0001. 기술 스택과 책임 경계](docs/adr/0001-tech-stack.md)
- [데이터 모델](docs/engineering/data-model.md) — 정본은 [`db/schema.sql`](db/schema.sql)
- [오늘의 학습 큐 명세](docs/engineering/study-queue-spec.md)
- [JSON 일괄 가져오기 형식](docs/engineering/json-import-format.md)

### 스파이크

- [1. Anki 패키지 구조 실측](docs/engineering/spike-01-apkg-parsing.md)
- [5. FSRS 적합성](docs/engineering/spike-05-fsrs.md)

### 검토 대기

- [검토 대기 결정 목록](docs/DECISIONS-PENDING-REVIEW.md)

## 개발 환경

언어마다 프로젝트 안에서 버전을 고정한다. 전역에 설치하는 것은 언어 런타임과 rustup뿐이다.

| 언어 | 고정 장치 | 버전 |
|---|---|---|
| Python | `.venv/` + [`requirements.txt`](requirements.txt) | 3.12 |
| Rust | [`rust-toolchain.toml`](rust-toolchain.toml). 이 폴더 안에서 rustup이 자동으로 이 버전을 쓴다 | 1.99.0 |
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
npm run tauri build     # 설치 파일 생성 (src-tauri/target/release/bundle/)
```

데이터는 `%APPDATA%\io.github.itisyijy.neoanki\` 에 저장된다 (DB, `media/`, `backups/`). 다른 폴더로 시험하려면 환경 변수 `NEOANKI_DATA_DIR` 을 지정한다.

```powershell
$env:NEOANKI_DATA_DIR = "C:\temp\neoanki-test"; npm run tauri dev
```

### 확인

```powershell
cd src-tauri; cargo test; cargo clippy --all-targets; cd ..
npx tsc --noEmit
```

### 구조

| 경로 | 내용 |
|---|---|
| `src/screens/` | 화면. [Figma](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx) 화면 24장과 이름이 같다 (Home = A1·A2, Deck = C1, Study = D1~D4 …) |
| `src/ui.tsx`, `src/styles/` | Figma 컴포넌트와 디자인 토큰 |
| `src-tauri/src/` | 데이터 계층. Rust: `db` 스키마, `queue` 오늘의 큐, `srs` FSRS, `render` Anki 템플릿, `import` 가져오기, `commands` 화면이 부르는 명령 |
| `db/schema.sql` | 데이터 모델 정본 |
| `tools/` | Python 참조 구현. Rust로 옮긴 원본 |

### Python 도구 사용

```powershell
.venv\Scripts\Activate.ps1      # 이후 python 은 가상환경의 것
```

## 도구

```bash
python tools/import_reference.py --self-test     # 스키마 자체 검사
python tools/import_reference.py <파일.colpkg>    # Anki 패키지 -> Neo-Anki 스키마
python tools/import_reference.py <파일.json>      # JSON 일괄 가져오기
python tools/queue_reference.py                  # 큐 알고리즘 테스트
python tools/queue_reference.py <적재된.sqlite>   # 실제 데이터로 큐 구성
```

가상환경을 활성화한 상태에서 실행한다. `tools/` 는 스파이크 산출물이고, 실제 앱은 ADR 0001에 따라 Rust(Tauri)로 만든다.
