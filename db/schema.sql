-- Neo-Anki 로컬 스키마 v1
--
-- 앱은 열 때마다 이 파일 전체를 적용한다. 그래서 모든 CREATE 는 IF NOT EXISTS 다.
-- 새 테이블은 여기 더하면 기존 DB 에도 생긴다. 기존 테이블에 열을 더할 때는 db.rs 의 열 목록에 함께 적는다
--
-- 이 파일이 데이터 모델의 정본이다. docs/engineering/data-model.md 는 이 파일을 설명한다.
-- 검증: tools/import_reference.py 가 실제 Anki 컬렉션을 이 스키마에 적재한다.
--
-- 규칙
--   시각   : UTC epoch milliseconds (INTEGER)
--   학습일 : 시각에서 파생한다. 하루 경계는 새벽 4시. 컬럼으로 저장하지 않는다
--   중요도 : 0=보충 1=일반 2=핵심. 큐 정렬에 쓰므로 정수로 둔다
--   anki_* : 원본 Anki 식별자. 재내보내기와 재가져오기 추적에 필요하다 (지금 넣지 않으면 나중에 복구 불가)

PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------- 덱

CREATE TABLE IF NOT EXISTS deck (
    id          INTEGER PRIMARY KEY,
    parent_id   INTEGER REFERENCES deck(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,              -- 이 단계의 이름만. 전체 경로가 아니다
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    archived_at INTEGER,                       -- 보관. NULL이면 활성 (MVP 6.2)

    -- 중요도 (MVP 6.5.1). 카드는 소속 덱에서 상속받는다
    importance        INTEGER NOT NULL DEFAULT 1 CHECK (importance BETWEEN 0 AND 2),
    importance_source TEXT    NOT NULL DEFAULT 'default'
                      CHECK (importance_source IN ('default','user','rule','tag','material')),
    importance_reason TEXT,                    -- 설명 가능성. "하위 덱 이름에 '요약' 포함"

    anki_deck_id INTEGER,
    UNIQUE (parent_id, name)
);
CREATE INDEX IF NOT EXISTS idx_deck_parent ON deck(parent_id);

-- 덱별 학습 설정 (MVP 6.5). 덱과 1:1이지만 대부분 비어 있어 분리한다
CREATE TABLE IF NOT EXISTS deck_config (
    deck_id          INTEGER PRIMARY KEY REFERENCES deck(id) ON DELETE CASCADE,
    target_date      TEXT,                     -- 'YYYY-MM-DD'. NULL이면 목표일 없는 장기 덱
    include_target_day INTEGER NOT NULL DEFAULT 0 CHECK (include_target_day IN (0,1)),
    daily_new_limit  INTEGER,                  -- NULL이면 무제한
    -- C4 "오늘만 신규 분량 조정". today_new_day 가 오늘(학습일)일 때만 유효하다. 다음 날이면 저절로 사라진다
    today_new        INTEGER,
    today_new_day    TEXT,
    -- 잠시 쉬는 덱. 하위 덱까지 오늘의 큐에서 빠진다. 카드와 일정은 그대로 둔다
    paused           INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0,1)),
    paused_until     TEXT,                     -- 'YYYY-MM-DD' 까지 쉰다 (그날 포함). NULL 이면 다시 시작할 때까지
    updated_at       INTEGER NOT NULL
);

-- ---------------------------------------------------------------- 노트

-- 노트 타입을 코드에 고정하지 않고 데이터로 둔다.
-- 스파이크 1: 실제 컬렉션의 75%가 커스텀 노트타입이었다. 화이트리스트는 성립하지 않는다
CREATE TABLE IF NOT EXISTS note_type (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    fields     TEXT    NOT NULL,               -- JSON 배열. ["Front","Back"]
    css        TEXT    NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,

    -- 지원 판정 결과 (MVP 6.4.2). 이름이 아니라 템플릿이 쓰는 기능으로 판정한다
    is_supported     INTEGER NOT NULL DEFAULT 1 CHECK (is_supported IN (0,1)),
    unsupported_note TEXT,                     -- 'JavaScript 사용' 등

    anki_model_id INTEGER,
    anki_raw      TEXT                         -- 원본 JSON. 재내보내기 대비
);

CREATE TABLE IF NOT EXISTS card_template (
    id           INTEGER PRIMARY KEY,
    note_type_id INTEGER NOT NULL REFERENCES note_type(id) ON DELETE CASCADE,
    ord          INTEGER NOT NULL,             -- 노트 타입 안에서의 순서
    name         TEXT    NOT NULL,
    qfmt         TEXT    NOT NULL,             -- 앞면 템플릿
    afmt         TEXT    NOT NULL,             -- 뒷면 템플릿
    UNIQUE (note_type_id, ord)
);

CREATE TABLE IF NOT EXISTS note (
    id           INTEGER PRIMARY KEY,
    note_type_id INTEGER NOT NULL REFERENCES note_type(id),
    fields       TEXT    NOT NULL,             -- JSON 배열. note_type.fields 와 같은 순서
    tags         TEXT    NOT NULL DEFAULT '',  -- 공백 구분. MVP에 태그 검색이 없어 정규화하지 않는다
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,

    anki_note_id INTEGER,
    anki_guid    TEXT,

    import_batch_id INTEGER REFERENCES import_batch(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_note_type ON note(note_type_id);
CREATE INDEX IF NOT EXISTS idx_note_batch ON note(import_batch_id);
-- .apkg 가져오기가 노트마다 guid 로 이미 있는지 찾는다 (T2-11 · T7-08: 없으면 노트 수의 제곱으로 느려진다). 기기 안 색인이라 동기화와 무관
CREATE INDEX IF NOT EXISTS idx_note_guid ON note(anki_guid) WHERE anki_guid IS NOT NULL;

-- ---------------------------------------------------------------- 카드

CREATE TABLE IF NOT EXISTS card (
    id          INTEGER PRIMARY KEY,
    note_id     INTEGER NOT NULL REFERENCES note(id) ON DELETE CASCADE,
    deck_id     INTEGER NOT NULL REFERENCES deck(id),
    template_ord INTEGER NOT NULL,             -- card_template.ord

    -- 상태 (docs/engineering/study-queue-spec.md 와 일치해야 한다)
    --
    -- 'buried'를 여기 넣지 않는다. 보류는 상태가 아니라 기간이다.
    -- 상태로 덮어쓰면 보류 전이 review였는지 learning이었는지 잃어버린다
    state       TEXT    NOT NULL DEFAULT 'new'
                CHECK (state IN ('new','learning','review','relearning','suspended')),

    -- 보류 (MVP 6.8 반복 루프 방지). 이 시각 전까지 큐에 올리지 않는다
    buried_until  INTEGER,
    buried_reason TEXT CHECK (buried_reason IN ('two_again','user')),

    -- SRS. 소유권은 ADR 0001 7장에서 확정한다. 큐는 이 값을 읽기만 한다
    due_at      INTEGER,                       -- NULL이면 아직 예정 없음 (신규)
    interval_days REAL    NOT NULL DEFAULT 0,
    stability   REAL,                          -- FSRS
    difficulty  REAL,                          -- FSRS
    srs_step    INTEGER,                       -- FSRS의 learning/relearning 단계 위치.
                                               -- 스파이크 5에서 발견: 없으면 학습 단계가 복원되지 않는다
    reps        INTEGER NOT NULL DEFAULT 0,
    lapses      INTEGER NOT NULL DEFAULT 0,
    last_reviewed_at INTEGER,

    -- 중요도 개별 지정. NULL이면 덱에서 상속한다 (MVP 6.5.1)
    importance        INTEGER CHECK (importance BETWEEN 0 AND 2),
    importance_source TEXT CHECK (importance_source IN ('user','rule','tag','material')),
    importance_reason TEXT,

    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,

    anki_card_id INTEGER,
    UNIQUE (note_id, template_ord)
);
CREATE INDEX IF NOT EXISTS idx_card_deck ON card(deck_id);
CREATE INDEX IF NOT EXISTS idx_card_due  ON card(state, due_at);
CREATE INDEX IF NOT EXISTS idx_card_buried ON card(buried_until) WHERE buried_until IS NOT NULL;

-- 큐 정렬용 뷰. 상속을 매번 CASE로 쓰지 않기 위해 둔다
CREATE VIEW IF NOT EXISTS card_effective AS
SELECT c.*,
       COALESCE(c.importance, d.importance)               AS eff_importance,
       COALESCE(c.importance_source, d.importance_source) AS eff_importance_source
FROM card c JOIN deck d ON d.id = c.deck_id;

-- ---------------------------------------------------------------- 복습 기록

-- append-only. 절대 갱신하거나 삭제하지 않는다.
-- 카드가 삭제되어도 남긴다 (통계가 소급해서 바뀌면 안 된다) → ON DELETE SET NULL
CREATE TABLE IF NOT EXISTS review_log (
    id           INTEGER PRIMARY KEY,
    card_id      INTEGER REFERENCES card(id) ON DELETE SET NULL,
    session_id   INTEGER REFERENCES study_session(id) ON DELETE SET NULL,
    reviewed_at  INTEGER NOT NULL,
    rating       INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 4),  -- 1=Again 2=Hard 3=Good 4=Easy
    elapsed_ms   INTEGER,
    prev_interval_days REAL,
    next_interval_days REAL,
    prev_state   TEXT,

    -- 이 평가를 SRS 스케줄러에 전달했는가.
    -- 반복 루프 방지(MVP 6.8)로 보류된 2회차 Again은 기록하되 스케줄러에는 넘기지 않는다
    fed_to_scheduler INTEGER NOT NULL DEFAULT 1 CHECK (fed_to_scheduler IN (0,1)),

    anki_revlog_id INTEGER,

    -- 밀린 복습(예정일이 어제 이전)이었던 카드를 평가했는가. 하루 상한 200장에서 오늘 이미 한 만큼 뺀다
    overdue INTEGER,

    -- NULL = 학습 평가. 'cram' = H3 압축 회독 (SRS 에 넘기지 않음). 오늘의 계획 · 지표는 NULL 만 센다, 히트맵은 둘 다
    kind TEXT
);
CREATE INDEX IF NOT EXISTS idx_revlog_card ON review_log(card_id, reviewed_at);
CREATE INDEX IF NOT EXISTS idx_revlog_time ON review_log(reviewed_at);

-- ---------------------------------------------------------------- 세션

-- 세션 = 학습 시작부터 이탈까지. 하루 경계를 넘으면 종료한다 (인터뷰 6라운드 Q3)
CREATE TABLE IF NOT EXISTS study_session (
    id          INTEGER PRIMARY KEY,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    end_reason  TEXT CHECK (end_reason IN ('completed','user_exit','app_exit','day_rollover')),
    study_day   TEXT NOT NULL                  -- 'YYYY-MM-DD'. 새벽 4시 경계로 계산한 학습일
);
CREATE INDEX IF NOT EXISTS idx_session_day ON study_session(study_day);

-- 그날 처음 계산한 오늘의 계획. 최상위 덱 단위 (MVP 12장 합격선 ① 일일 계획 완료율의 분모).
-- 학습하기 전의 계획을 남겨야 완료율을 낼 수 있다. 하루에 한 번만 쓴다 (INSERT OR IGNORE)
CREATE TABLE IF NOT EXISTS daily_plan (
    study_day      TEXT    NOT NULL,           -- 'YYYY-MM-DD'
    deck_id        INTEGER NOT NULL REFERENCES deck(id) ON DELETE CASCADE,
    planned_review INTEGER NOT NULL,           -- 복습 + 학습 중 + 밀린 + 접어둔 카드 복귀
    planned_new    INTEGER NOT NULL,
    has_target     INTEGER NOT NULL CHECK (has_target IN (0,1)),
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (study_day, deck_id)
);

-- 세션 안에서만 의미 있는 카드 상태 (MVP 6.8 반복 루프 방지)
CREATE TABLE IF NOT EXISTS session_card_state (
    session_id   INTEGER NOT NULL REFERENCES study_session(id) ON DELETE CASCADE,
    card_id      INTEGER NOT NULL REFERENCES card(id) ON DELETE CASCADE,
    again_count  INTEGER NOT NULL DEFAULT 0,
    is_buried    INTEGER NOT NULL DEFAULT 0 CHECK (is_buried IN (0,1)),
    buried_at    INTEGER,
    buried_reason TEXT,                        -- 'two_again' | 'user'
    shown_count  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session_id, card_id)
);

-- ---------------------------------------------------------------- 미디어

CREATE TABLE IF NOT EXISTS media (
    id         INTEGER PRIMARY KEY,
    filename   TEXT    NOT NULL UNIQUE,        -- 카드 HTML이 참조하는 이름
    rel_path   TEXT    NOT NULL,               -- 앱 미디어 디렉터리 기준 상대 경로
    sha256     TEXT    NOT NULL,               -- 중복 제거
    byte_size  INTEGER NOT NULL,
    mime_type  TEXT,
    created_at INTEGER NOT NULL,
    anki_media_key TEXT
);
CREATE INDEX IF NOT EXISTS idx_media_hash ON media(sha256);

-- ---------------------------------------------------------------- 가져오기

-- 가져오기 단위. 롤백과 결과 보고에 쓴다 (MVP 6.4)
CREATE TABLE IF NOT EXISTS import_batch (
    id           INTEGER PRIMARY KEY,
    source_kind  TEXT    NOT NULL CHECK (source_kind IN ('apkg','colpkg','json')),
    source_name  TEXT    NOT NULL,
    started_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    status       TEXT    NOT NULL DEFAULT 'running'
                 CHECK (status IN ('running','ok','partial','failed','rolled_back')),
    deck_count   INTEGER NOT NULL DEFAULT 0,
    note_count   INTEGER NOT NULL DEFAULT 0,
    card_count   INTEGER NOT NULL DEFAULT 0,
    media_count  INTEGER NOT NULL DEFAULT 0,
    skipped_count INTEGER NOT NULL DEFAULT 0
);

-- 건너뛰거나 확인이 필요한 항목. "조용히 삭제하지 않는다" 원칙의 근거 (MVP 6.4)
CREATE TABLE IF NOT EXISTS import_issue (
    id          INTEGER PRIMARY KEY,
    batch_id    INTEGER NOT NULL REFERENCES import_batch(id) ON DELETE CASCADE,
    severity    TEXT    NOT NULL CHECK (severity IN ('skipped','warning','info')),
    kind        TEXT    NOT NULL,              -- 'unsupported_template' | 'missing_media' | ...
    detail      TEXT    NOT NULL,
    ref_label   TEXT                           -- 사용자가 찾아볼 수 있는 표식
);
CREATE INDEX IF NOT EXISTS idx_issue_batch ON import_issue(batch_id, severity);

-- ---------------------------------------------------------------- 메타

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO schema_meta (key, value) VALUES
    ('version', '1'),
    ('day_cutoff_hour', '4'),
    ('srs', 'fsrs'),               -- 스파이크 5: Rating 1..4가 우리 review_log.rating과 정확히 일치
    ('fsrs_learning_steps', '1m,10m');

-- ---- G · 카드 품질
-- G2 나누기 같은 카드 변경 한 번 = 한 줄. [되돌리기]가 이 줄로 새 카드를 지우고 원래 카드를 되살린다.
-- 이 기기 안에서만 쓴다 (payload 가 로컬 id 를 가리킨다. 카드 자체는 card · note 로 동기화된다)
CREATE TABLE IF NOT EXISTS card_change (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('split','draft')), -- draft: R1 자료에서 카드 초안 승인
    deck_id    INTEGER,
    card_id    INTEGER,                        -- 원래 카드
    summary    TEXT    NOT NULL,               -- "3장으로 나누기"
    payload    TEXT    NOT NULL,               -- JSON. {prev_state, new_notes}
    created_at INTEGER NOT NULL,
    undone_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_card_change_deck ON card_change(deck_id, created_at);

-- ---- H · 시험 계획
-- 사용자 설정 (쓸 수 있는 시간 등). 키-값. 동기화한다 (uid)
CREATE TABLE IF NOT EXISTS plan_setting (
    key   TEXT PRIMARY KEY,                    -- 'weekday_min' | 'weekend_min'
    value TEXT NOT NULL,
    uid   TEXT
);
-- H1 [N일로 당겨 맞추기]. 예산 덱(deck_id)의 그날 신규 몫에 더하거나 뺀다. 큐가 따른다
CREATE TABLE IF NOT EXISTS plan_shift (
    id         INTEGER PRIMARY KEY,
    deck_id    INTEGER NOT NULL REFERENCES deck(id) ON DELETE CASCADE,
    day        TEXT    NOT NULL,               -- 'YYYY-MM-DD'
    delta      INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    uid        TEXT
);
CREATE INDEX IF NOT EXISTS idx_plan_shift_day ON plan_shift(day);
-- H2 목표일 충돌에서 고른 것. deck_a 의 시험이 먼저다. 목표일은 바꾸지 않는다
CREATE TABLE IF NOT EXISTS plan_conflict (
    id         INTEGER PRIMARY KEY,
    deck_a     INTEGER NOT NULL REFERENCES deck(id) ON DELETE CASCADE,
    deck_b     INTEGER NOT NULL REFERENCES deck(id) ON DELETE CASCADE,
    mode       TEXT    NOT NULL CHECK (mode IN ('spread','first','keep')),
    created_at INTEGER NOT NULL,
    uid        TEXT,
    UNIQUE (deck_a, deck_b)
);

-- ---------------------------------------------------------------- I · 원자료
-- 덱에 붙인 강의 자료 (I1, post-mvp B-1). 원본은 <data_dir>/sources/ 에 복사해 두고 고치지 않는다.
-- 자료·구간·연결은 사용자 데이터라 동기화 대상(uid, db::SYNC_TABLES). 자료 파일 자체는 기본으로 올리지 않는다

CREATE TABLE IF NOT EXISTS source_material (
    id         INTEGER PRIMARY KEY,
    deck_id    INTEGER NOT NULL REFERENCES deck(id) ON DELETE CASCADE,
    file_name  TEXT    NOT NULL,             -- 사용자가 고른 파일 이름 (보여주기용)
    rel_path   TEXT    NOT NULL,             -- data_dir 기준 복사본 경로 (sources/12.pdf)
    kind       TEXT    NOT NULL CHECK (kind IN ('pdf','pptx')),
    pages      INTEGER NOT NULL DEFAULT 0,   -- PDF 쪽 수 / PPTX 슬라이드 수
    status     TEXT    NOT NULL DEFAULT 'pending'
               CHECK (status IN ('pending','ready','unsupported')), -- unsupported: 글자를 읽을 수 없음 (스캔본)
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    uid        TEXT
);
CREATE INDEX IF NOT EXISTS idx_source_material_deck ON source_material(deck_id);

-- 강조된 구간. signals 는 'highlight,red' 처럼 신뢰도 순. weight 는 신호 가중치 합 (post-mvp 5장)
CREATE TABLE IF NOT EXISTS source_segment (
    id          INTEGER PRIMARY KEY,
    material_id INTEGER NOT NULL REFERENCES source_material(id) ON DELETE CASCADE,
    page        INTEGER NOT NULL,            -- 1부터
    ord         INTEGER NOT NULL,            -- 쪽 안 순서
    text        TEXT    NOT NULL,
    signals     TEXT    NOT NULL,
    weight      REAL    NOT NULL,
    rects       TEXT,                        -- 쪽 위 위치 JSON [[x,y,w,h],..] 0~1 비율. PPTX 는 없음
    before_text TEXT,                        -- 앞뒤 문단 (I2 패널)
    after_text  TEXT,
    uid         TEXT
);
CREATE INDEX IF NOT EXISTS idx_source_segment_material ON source_segment(material_id, page, ord);

-- 구간 ↔ 카드. manual: [카드 잇기] / auto: 임베딩 유사도가 기준 이상 (I3) /
-- rejected: 사용자가 끊은 자동 연결 (다시 분석해도 되살리지 않는다). 카드나 자료를 지우면 연결만 사라진다 (CASCADE).
-- origin 열(db.rs migrate): 'draft' 면 R1 초안에서 만든 카드의 연결 (method 는 manual). NULL 허용 (예전 앱이 만든 행)
CREATE TABLE IF NOT EXISTS source_link (
    id         INTEGER PRIMARY KEY,
    segment_id INTEGER NOT NULL REFERENCES source_segment(id) ON DELETE CASCADE,
    card_id    INTEGER NOT NULL REFERENCES card(id) ON DELETE CASCADE,
    method     TEXT    NOT NULL CHECK (method IN ('manual','auto','rejected')),
    score      REAL,                         -- auto 일 때 코사인 유사도
    created_at INTEGER NOT NULL,
    uid        TEXT,
    UNIQUE (segment_id, card_id)
);
CREATE INDEX IF NOT EXISTS idx_source_link_card ON source_link(card_id);

-- 임베딩 캐시. 이 기기에서만 쓴다 (모델만 있으면 다시 만들 수 있다). 글이 바뀌면 text_hash 가 달라져 다시 계산
CREATE TABLE IF NOT EXISTS embedding_cache (
    kind      TEXT    NOT NULL CHECK (kind IN ('card','segment')),
    ref_id    INTEGER NOT NULL,
    model     TEXT    NOT NULL,
    text_hash TEXT    NOT NULL,
    vec       BLOB    NOT NULL,              -- f32 little-endian
    PRIMARY KEY (kind, ref_id, model)
);

-- ---------------------------------------------------------------- J · 서술형 (학습 화면 J1~J3)

-- 서술형 설정. 노트 단위 (서술형 노트는 카드가 하나다). 행이 없으면 기본값: 앞면에 {{type:필드}} 가 있으면 써서 답하기, 키워드 없음
CREATE TABLE IF NOT EXISTS note_written (
    note_id    INTEGER PRIMARY KEY REFERENCES note(id) ON DELETE CASCADE,
    write_mode INTEGER CHECK (write_mode IN (0,1)),  -- 공부할 때 직접 써서 답하기. NULL 이면 템플릿대로
    keywords   TEXT    NOT NULL DEFAULT '[]',        -- 필수 키워드 JSON [{"word","synonyms":[..],"source":"user"|"suggested"}]
    dismissed  TEXT    NOT NULL DEFAULT '[]',        -- 뺀 제안 JSON ["..."]. 다시 제안하지 않는다
    updated_at INTEGER NOT NULL,
    uid        TEXT
);

-- J1 "지난번에 쓴 답". 카드마다 마지막 하나 (자동 저장이 덮어쓴다)
CREATE TABLE IF NOT EXISTS written_answer (
    card_id    INTEGER PRIMARY KEY REFERENCES card(id) ON DELETE CASCADE,
    answer     TEXT    NOT NULL,
    written_at INTEGER NOT NULL,
    uid        TEXT
);

-- Q9 목록 회상 · 선택형: 마지막 평가 때 빠뜨린 항목 · 고른 보기 (번호 JSON 배열). 카드마다 하나.
-- 이 기기 안에서만 쓴다 (동기화하지 않는다. Anki 에는 평가만 간다)
CREATE TABLE IF NOT EXISTS recall_mark (
    card_id   INTEGER PRIMARY KEY REFERENCES card(id) ON DELETE CASCADE,
    marks     TEXT    NOT NULL,
    marked_at INTEGER NOT NULL
);
