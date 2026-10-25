# Addiem 데이터 모델

## 문서 상태

- 버전: v0.2
- 상태: **결정 완료.** 정본은 [`db/schema.sql`](../../db/schema.sql) 이다
- 검증: 실제 컬렉션 4,694장 / 복습기록 7,745건을 **손실 0**으로 적재 (`tools/import_reference.py`)
- 선행 문서: [ADR 0001](../adr/0001-tech-stack.md), 후속 문서: [학습 큐 명세](study-queue-spec.md)

> **스키마 파일이 정본이다.** 이 문서는 왜 그렇게 생겼는지를 설명한다.
> 컬럼 목록을 여기 베껴두면 반드시 어긋나므로 적지 않는다.

## 1. 설계 원칙과 그 결과

| 원칙 | 스키마에 남은 흔적 |
|---|---|
| 로컬 우선 저장 | 단일 SQLite 파일로 전체 상태를 복원한다 |
| 원본 `.apkg` 보존 | 가져온 데이터는 복사본이다. 원본 파일을 참조하는 컬럼이 없다 |
| 되돌리기와 변경 이력 | `review_log` 는 append-only. 카드가 지워져도 남는다 |
| 설명 가능한 자동화 | `importance_source` 와 `importance_reason` 이 모든 자동 값에 붙는다 |
| 덱별 설정 | `deck_config` 가 덱에 붙는다. 전역 설정 테이블이 없다 |
| 가져오기 실패 시 롤백 | `import_batch` 가 단위를 식별한다 |

## 2. 용어

Anki의 노트/카드 구분을 그대로 가져간다.

| 용어 | 정의 |
|---|---|
| 노트 | 내용의 단위. 필드 값의 집합 |
| 카드 | 학습의 단위. 노트 하나에서 템플릿 수만큼 생긴다 |
| 덱 | 카드를 담는 계층 구조. `parent_id` 로 표현한다 |
| 세션 | 학습 시작부터 이탈까지. 하루 경계를 넘으면 종료 |
| 오늘 | 새벽 4시 경계로 계산한 학습일 |

**분리하는 이유.** MVP 6.4.2가 `Basic and Reversed` 를 지원 범위에 두고, 스파이크 1에서 노트 하나가 템플릿 2개를 갖는 노트타입이 실제로 존재함을 확인했다. 지금 합치면 나중에 갈아엎게 된다.

## 3. 엔티티

| 엔티티 | 목적 | 관련 범위 |
|---|---|---|
| `deck` | 덱 계층 + 중요도 | 6.2, 6.5.1 |
| `deck_config` | 목표일, 당일 포함, 신규 상한, 오늘만 신규 분량(`today_new`, 그날에만 유효), 잠시 쉬기(`paused`) | 6.5, 7장 |
| `note_type` | 필드 목록, CSS, 지원 여부 | 6.3, 6.4.2 |
| `card_template` | 앞면/뒷면 템플릿 | 6.4.2 |
| `note` | 필드 값, 태그 | 6.3 |
| `card` | 학습 단위, SRS 상태, 중요도 덮어쓰기 | 6.6, 6.7 |
| `card_effective` (뷰) | 중요도 상속을 해결한 카드 | 6.5.1, 6.6 |
| `review_log` | 평가 기록 (append-only) | 6.7, 6.9 |
| `study_session` | 세션 | 6.8 |
| `session_card_state` | 세션 안의 Again 횟수와 보류 | 6.8 |
| `daily_plan` | 그날 처음 본 최상위 덱별 계획 (합격선 ①의 분모) | 12장 |
| `media` | 이미지와 첨부 | 6.3, 6.4 |
| `import_batch`, `import_issue` | 가져오기 단위와 보고 | 6.4 |
| `schema_meta` | 스키마 버전, 하루 경계 | - |

## 4. 눈여겨볼 설계 결정

### 4.1 중요도를 정수로 두고 뷰로 상속한다

```sql
deck.importance   INTEGER  -- 0=보충 1=일반 2=핵심
card.importance   INTEGER  -- NULL이면 덱에서 상속
```

큐가 이 값으로 정렬한다. 텍스트면 정렬할 때마다 `CASE` 가 필요하다.

`card_effective` 뷰가 `COALESCE(card.importance, deck.importance)` 를 대신 처리한다. 큐 쿼리가 상속 로직을 반복하지 않는다.

### 4.2 자동으로 정한 값에는 출처와 이유가 따라붙는다

```sql
importance_source  -- 'default' | 'user' | 'rule' | 'tag' | 'material'
importance_reason  -- "태그 '중요도::최우선'"
```

`user` 로 한 번 바뀌면 자동 배치가 덮어쓰지 않는다. 제품 방향의 `Explainable changes` 와 MVP 6.5.1의 조건이 이 두 컬럼에 걸려 있다.

`material` 은 [MVP 다음 단계](../product/post-mvp-source-analysis.md)를 위해 미리 열어둔 값이다.

### 4.3 복습 기록은 카드보다 오래 산다

```sql
card_id INTEGER REFERENCES card(id) ON DELETE SET NULL
```

카드를 지워도 "그날 공부했다"는 사실은 남아야 한다. 실측 컬렉션에서 **7,745건 중 1,310건(17%)이 이미 삭제된 카드의 기록**이었다. 이걸 버리면 학습 히트맵이 17% 틀린다.

### 4.4 `fed_to_scheduler` — 반복 루프 방지의 기록

```sql
review_log.fed_to_scheduler INTEGER NOT NULL DEFAULT 1
```

보류를 유발한 2회차 Again은 기록하되 SRS 스케줄러에 넘기지 않는다. 자세한 근거는 [큐 명세 6.3](study-queue-spec.md)에 있다. **ADR 0001 7장에서 정식 승인이 필요하다.**

### 4.5 `anki_*` 원본 식별자

`deck`, `note`, `card`, `note_type`, `media`, `review_log` 에 각각 있다.

지금 넣으면 컬럼 몇 개다. 나중에 넣으면 이미 가져온 덱은 추적이 불가능해 재가져오기밖에 답이 없다. `.apkg` 재내보내기(Should)와 버전 관리(Later)가 전부 여기에 의존한다.

### 4.6 노트 타입을 코드가 아니라 데이터로 둔다

스파이크 1에서 실측한 결과 **75%가 커스텀 노트타입**이었다. 이름으로 화이트리스트하는 방식은 성립하지 않는다.

`note_type.fields` 는 JSON 배열, `card_template` 은 별도 테이블이다. `is_supported` 와 `unsupported_note` 가 판정 결과를 담는다. 판정 기준은 이름이 아니라 **템플릿이 쓰는 기능**이다.

### 4.7 학습일을 저장하지 않는다

`study_session.study_day` 하나만 예외다. 나머지는 전부 UTC epoch 밀리초로 저장하고 학습일은 파생한다.

하루 경계 설정을 나중에 바꾸면 저장된 학습일이 전부 틀어진다. `schema_meta.day_cutoff_hour` 가 그 설정이다.

## 5. 관계

```text
deck ──┬─< deck (하위 덱)
       ├─── deck_config          1:1, 선택
       └─< card

note_type ──┬─< card_template
            └─< note ──< card ──┬─< review_log        카드 삭제 시 card_id=NULL
                                └─< session_card_state

study_session ──< session_card_state
import_batch  ──┬─< note
                └─< import_issue
media                                  파일명으로 카드 HTML에서 참조
```

## 6. 카드 상태 전이

```text
          ┌──────────────────────────────┐
          ↓                              │
  new → learning → review ⇄ relearning ──┘
          │           │
          └───────────┴──→ suspended  (사용자 수동. 큐에 절대 안 나옴)
```

| 상태 | 큐에 나오는가 |
|---|---|
| `new` | 예산 덱 상한 안에서 |
| `learning` | 항상. 가장 앞 |
| `review`, `relearning` | 예정일이 오늘 이하면 |
| `suspended` | **절대 나오지 않는다** |

보류(접어두기)는 상태가 아니라 기간이다. `state` 는 그대로 두고 `card.buried_until` 과 `buried_reason` (`two_again` | `user`) 만 채운다. 상태로 덮으면 보류 전이 `review` 였는지 `learning` 이었는지 잃는다 ([스파이크 5](spike-05-fsrs.md) 결함 2).

큐는 상태보다 `buried_until` 을 먼저 본다. 그 날이 오기 전에는 상태와 상관없이 나오지 않고, 그 날부터 보류 복귀 그룹으로 나온다. 평가하면 비운다. Anki의 bury와 이름은 같지만 의미가 다르다. 여기서는 반복 루프 방지(6.8)와 사용자 접어두기의 결과다.

## 7. Anki 매핑

전체 매핑과 함정은 [스파이크 1 보고서](spike-01-apkg-parsing.md)에 있고, 실행 가능한 형태는 `tools/import_reference.py` 다.

요약하면 이렇다.

| Anki | Addiem | 주의 |
|---|---|---|
| `decks.name` (`\x1f` 구분) | `deck` 계층 | 부모부터 만든다 |
| `notetypes` + `fields` + `templates` | `note_type` + `card_template` | 템플릿은 protobuf. field 1=앞면, 2=뒷면 |
| `notes.flds` (`\x1f` 구분) | `note.fields` (JSON) | |
| `cards.did` | `card.deck_id` | **`odid`가 0이 아니면 `odid`를 쓴다** |
| `cards.type` | `card.state` | 0=new 1=learning 2=review 3=relearning |
| `cards.queue = -1` | `state = 'suspended'` | |
| `revlog.ease` | `review_log.rating` | 1~4 외의 값은 평가가 아니다 |
| `media` (protobuf) | `media.filename` | `{1:이름, 2:크기, 3:sha1}` 반복. 순서가 zip 번호 |

## 8. 주요 쿼리

큐 구성이 요구하는 것은 두 가지다.

```sql
-- 오늘의 후보. card_effective 뷰가 중요도 상속을 이미 처리했다
SELECT id, deck_id, state, eff_importance, due_at
FROM card_effective
WHERE state <> 'suspended';

-- 덱 트리. 예산 덱 탐색에 쓴다
SELECT id, parent_id, name FROM deck WHERE archived_at IS NULL;
```

`idx_card_due (state, due_at)` 가 첫 번째를 받친다.

## 9. 마이그레이션 정책

| 항목 | 결정 |
|---|---|
| 버전 관리 | `schema_meta.version` 정수. 마이그레이션 파일은 `db/migrations/NNNN_*.sql` |
| 앱 실행 시 자동 적용 | **한다.** 사용자가 수동으로 할 일이 아니다 |
| 적용 전 자동 백업 | **한다.** 실패 시 되돌릴 수단이 이것뿐이다 |
| 하위 호환 | 앞으로만 간다. 구버전 앱으로 여는 것은 지원하지 않는다 |

MVP에는 마이그레이션이 아직 없다. v1이 최초 스키마다.

## 10. 남은 미결정

| 질문 | 영향 | 언제 |
|---|---|---|
| Anki SRS 상태(`ivl`, `factor`)를 FSRS 상태로 어떻게 매핑하는가 | 복습 이력 이전 품질 | FSRS 스파이크 후 |
| 카드가 참조하지 않는 미디어를 가져올 것인가 | 실측에서 227개 중 179개가 미참조 | 가져오기 구현 시 |
| 태그를 테이블로 정규화할 것인가 | 태그 검색 기능이 생기면 | Should 단계 |
| `graves`(Anki 삭제 기록)를 가져올 것인가 | 동기화가 생기기 전에는 불필요 | Later |
