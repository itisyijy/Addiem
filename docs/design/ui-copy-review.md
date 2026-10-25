# Addiem UI 문구 이해도 검토

검토일: 2026-10-15 · 대상: [Addiem MVP UI / MVP Screens](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=0-1) · 구현 대조 기준: 작업 트리 HEAD `66e34ac`.

## 1. 검토 결과와 읽는 법

가장 큰 문제는 문장이 부자연스럽다는 것보다 **문구에서 예상한 행동·범위와 실제 기능이 어긋나는 것**이다. 특히 신규 카드 학습 완료와 시험 준비 완료, 접어두기와 학습 중지, 데이터 합치기와 모든 버전 보존, 연결과 외부 전송을 명확히 구분해야 한다.

피드백은 **59개: 높음 19개, 중간 38개, 낮음 2개**다. MVP 1~5와 추가 기능군으로 묶었으며, 동일 문구가 반복되는 화면은 대표 항목에 통합했다.

Figma의 최상위 프레임 327개를 분류하고, 제품 화면·상태 예시·호환 설명 **173개를 모두 이미지로 확인**했다. 기본·설명 프레임 130개의 원문을 수집했고, 다크 변형 43개도 텍스트를 대조했다. 같은 이름의 기본 화면이 있는 다크 변형 42개는 원문에 차이가 없었다. 나머지 1개는 가져온 덱의 밝은 카드 면을 설명하는 별도 예시다. 설계 결정 노트 130개, 행 제목 20개, 컴포넌트·토큰·흐름 보드 4개는 제품 문구 검토 대상에서 제외했다. 전체 검토 목록은 9장에 있다.

MCP의 연결 계정 식별자 오류를 해결한 뒤 파일을 조회했다. `get_screenshot`은 Education 호출 한도에 도달했으나, 공식 읽기 전용 Plugin API의 `node.screenshot()`은 작동하여 나머지 이미지 확인을 완료했다. Figma와 앱 코드는 변경하지 않았다.

### 평가 기준

- **높음:** 잘못된 행동·결과를 유도할 가능성이 있다.
- **중간:** 기능 이해나 선택을 지연시킨다.
- **낮음:** 자연스러움·일관성을 개선한다.

아래의 오해 가능성은 **전문가 검토 가설**이며 사용자 테스트에서 확인한 사실이 아니다. 각 표의 원문은 Figma에 실제로 존재하는 문자열이다. 화면명 링크는 해당 원문의 프레임을 가리킨다. 화면에 함께 있는 긴 보조문구는 필요한 부분만 인용했다. 수정안은 검토 제안이며 승인·반영된 최종 문구가 아니다.

상태 표기는 `문구 수정`(현재 동작을 설명), `설계 확인`(동작·지표 정의를 먼저 확인), `구현과 차이`(Figma와 코드가 다름)다. 숫자와 날짜는 화면 예시이며, 실제 적용 시 현재 데이터로 치환해야 한다. S1·K5·L4-s·R2-s·M5·P0·Q3·AU2 등 설명 보드는 **사용자에게 노출될 예시 문구만** 평가했다. 제작자용 주석 자체를 앱 문구 오류로 세지 않았다.

### 우선 수정 항목

| 순서 | ID | 먼저 고칠 이유 |
|---|---|---|
| 1 | M3-01 | ‘다음 학습’으로 미룬 카드가 다음번 실행에 나오리라 예상하지만 실제로는 3일 뒤 복귀한다. |
| 2 | M5-01·02 | 학습 시작 비율과 신규 카드 소진 계획을 숙달·시험 준비 완료로 오해할 수 있다. |
| 3 | X2-01·02 | 동기화 병합·복구의 적용 대상과 현재 내용이 바뀌는 범위를 명확히 해야 한다. |
| 4 | X3-01·02 | 자료를 보내지 않는다는 요약과 실제 자료 구간·질문 전송 범위가 일치해야 한다. |
| 5 | M2-02 | ‘제안’을 확정하지 않아도 현재 중요도로 학습한다는 점을 밝혀야 한다. |
| 6 | X1-01 | Anki 일정이 ‘그대로’라는 표현은 Addiem의 큐 규칙과 이후 일정 계산 차이를 가린다. |
| 7 | M4-01 | ‘그대로 두기’가 30일간 목록에서 숨기기라는 결과를 클릭 전에 알려야 한다. |
| 8 | M5-03 | 개인 학습 화면의 ‘합격선’이 실제 시험의 합격 기준으로 읽힐 수 있다. |

## 2. MVP 1 — 시험 목표와 스케줄링

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| M1-01 · 높음 · 문구 수정 | [C4 목표 설정](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1559) 제목 아래: ‘이 덱에만 적용돼요.’ | 초보: 하위 덱은 영향이 없다고 생각한다. 기존: 하위 덱의 설정 상속 여부를 알 수 없다. | ‘이 덱과 이 설정을 따르는 하위 덱에 적용돼요.’ 상한 아래에는 ‘별도 설정이 없는 하위 덱끼리 하루 신규 상한을 나눠 써요.’ 가장 가까운 설정을 따르는 구현에 맞춘다. |
| M1-02 · 중간 · 문구 수정 | [C4](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1559): ‘바꾸면 다음 학습부터 계획을 다시 계산해요.’ / ‘오늘만 신규 분량 조정’ | 초보: 오늘 설정인지 내일부터인지 불분명하다. 기존: 진행 중인 세션의 큐가 즉시 바뀌는지 궁금하다. | ‘저장한 설정으로 이후에 시작하는 학습 계획을 계산해요.’ 오늘 조정에는 ‘오늘 시작하는 학습의 신규 분량만 바꿔요.’를 붙인다. 진행 중 세션의 적용 시점은 별도 확인 후 보조문구에 명시한다. |
| M1-03 · 중간 · 설계 확인 | [C4-b 상한 부족](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=22-1849): ‘목표 완료 예상’ → ‘11월 6일’, ‘이대로면 목표일을 12일 넘겨요’, ‘목표일 11월 7일로’ | 두 사용자 모두: 예상 완료일과 조정 날짜가 달라 계산을 신뢰하기 어렵다. | 라벨을 ‘신규 카드 첫 학습 완료 예상’으로 바꾼다. 당일 제외 때문에 다음 날을 목표일로 제안한다면 ‘11월 6일까지 학습 · 당일 제외 기준 목표일 11월 7일’로 설명한다. 초과 일수는 같은 날짜 기준으로 재계산한다. |
| M1-04 · 중간 · 구현과 차이 | [H1 학습 계획](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6121): ‘최근 14일 동안 카드 1장에 평균 9초’ / ‘10월 23일 (금) 하루가 시간을 넘겨요’ | 초보: 공부 시간을 정확히 예측한다고 여긴다. 기존: 카드 난이도나 재학습에 따른 변동을 알 수 없다. | ‘최근 기록 기준 카드당 보통 약 {초}초 · 예상 분량이에요.’ / ‘10월 23일은 공부 가능한 시간을 넘길 것으로 예상돼요.’ 실제 구현은 시간을 제한한 뒤 중앙값을 사용하므로 ‘평균’은 피한다. 기록 부족 시 ‘학습 기록이 적어 카드당 10초로 예상했어요.’ |
| M1-05 · 중간 · 문구 수정 | [H2 목표일 충돌](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6299): ‘빅데이터분석기사 먼저’ / ‘이렇게 나누기’ / ‘정처기’ | 초보: 복습까지 한 과목만 하는지, 무엇이 저장되는지 모른다. 기존: 두 덱의 목표일 변경으로 오해할 수 있다. | 선택명 ‘빅데이터분석기사 신규 카드 먼저’, 버튼 ‘선택한 분량 배분 적용’. 버튼 위에 ‘두 덱의 신규 카드 배분을 바꿔요. 목표일과 예정 복습은 그대로예요.’ 과목 약칭은 정식 이름으로 통일한다. |

## 3. MVP 2 — 우선순위 기반 학습

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| M2-01 · 중간 · 문구 수정 | [B4 중요도](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-745): ‘핵심’ / ‘일반’ / ‘보충’ | 초보: 포함·제외 등급으로 여긴다. 기존: 복습 카드도 항상 핵심부터 나오는 것으로 예상한다. | 제목 아래 ‘신규 카드는 핵심 → 일반 → 보충 순으로 배워요. 보충도 학습 범위에 포함돼요.’ 복습은 예정일 우선이라는 설명은 도움말로 제공한다. |
| M2-02 · 높음 · 문구 수정 | [B4](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-745): ‘시스템 제안’ / ‘나중에 하기’ / ‘확정하고 덱 열기’ | 초보: 제안을 승인하기 전에는 중요도에 반영되지 않는다고 생각한다. 기존: 확정이 저장인지 자동 변경 방지인지 모른다. | 제안 안내 ‘태그와 덱 이름으로 정한 중요도가 현재 학습에 적용돼요. 확인한 제안은 이후 자동 제안으로 덮어쓰지 않아요.’ 보조 버튼 ‘현재 중요도로 계속’, 주 버튼 ‘제안 확정하고 덱 열기’. 현재의 출처 변경 동작을 설명한다. |
| M2-03 · 중간 · 설계 확인 | [C3 카드 추가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1236): ‘비워두면 하위 덱 설정을 따라요 (일반)’ / [C2-m](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2864): ‘개별’ | 초보: 3개 중 하나가 선택된 UI에서 무엇을 비우라는지 모른다. 기존: 일반 직접 지정과 상속 일반을 구분하기 어렵다. | 상속 선택을 ‘덱 설정 따르기 · 현재 일반’으로 명시하고, 직접 지정 후에도 돌아갈 선택지를 보인다. ‘개별’ 배지는 ‘직접 지정’으로 바꾼다. 선택 상태 표현을 먼저 맞춘 뒤 보조문구를 확정한다. |
| M2-04 · 낮음 · 문구 수정 | [B4-g 과목별 묶음](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=24-1828): ‘나뉨’ / ‘12개 중 3개 · 118장’ | 초보: 덱이 복제되거나 카드가 나뉜 것으로 읽는다. 기존: 하위 덱 일부가 다른 등급이라는 의미를 추측해야 한다. | 배지 ‘중요도 혼합’. 설명 ‘하위 덱 12개 중 핵심 3개 · 118장’. 펼치기 도움말 ‘하위 덱마다 중요도가 달라요.’ |
| M2-05 · 높음 · 문구 수정 | [N2 여러 장 기출 표시](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=112-11692): ‘카드 3장에 기출 표시’ / ‘2개 노트에 표시’ | 초보: 노트가 무엇인지 모른다. 기존: 선택한 카드 외에 같은 노트의 역방향·빈칸 카드까지 표시가 공유되는지 확인하기 어렵다. | 제목 ‘선택한 카드에 기출 시험 표시’. 안내 ‘같은 내용에서 만들어진 카드들은 기출 표시를 함께 써요(Anki 노트 기준). 3개 중 1개는 이미 표시돼 있어요.’ 버튼 ‘새 시험 표시 2개 추가’. 관련 카드 수를 확보할 수 있으면 함께 표시한다. |

## 4. MVP 3 — 어려운 카드 관리와 평가

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| M3-01 · 높음 · 문구 수정 | [D5 학습 완료](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=18-1358): ‘다음 학습으로 넘기기’ / ‘3일 뒤 학습 맨 앞에 다시 나와요’ | 초보: 오늘 다시 시작하면 나온다고 생각한다. 기존: Anki의 하루 미루기와 혼동한다. | 버튼 ‘3일 뒤 다시 보기’. 설명 ‘{복귀일}부터 오늘의 학습에 다시 포함돼요.’ 실제 학습일 경계를 반영한 날짜를 쓴다. 복귀 시점과 그날 큐 순서를 하나의 약속으로 묶지 않는다. |
| M3-02 · 중간 · 구현과 차이 | [D5-b](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=18-1492): ‘같은 학습에서 두 번 막혀서 잠시 빼둔 카드예요.’ | 직접 접어둔 사용자에게는 사실과 다르다. 기존 사용자도 자동 판정과 수동 제외를 구분하지 못한다. | 자동일 때 ‘두 번 기억나지 않아 자동으로 접어둔 카드예요.’, 수동일 때 ‘직접 접어둔 카드예요.’, 혼합일 때 ‘자동으로 또는 직접 접어둔 카드예요.’ 현재 Done 구현은 원인을 분기하므로 Figma를 맞춘다. |
| M3-03 · 중간 · 문구 수정 | [C1 덱 상세](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-800): ‘어려운 카드’ / ‘접어둔 카드 · 3일 뒤 복귀’ | 초보: 앱이 난이도를 판정한 카드 수로 여긴다. 기존: 수동 보류·복귀 날짜가 다른 카드도 같은 상태로 뭉친다. | 지표명 ‘접어둔 카드’. 상세에는 ‘자동 {n}장 · 직접 {n}장’과 ‘가장 빠른 복귀 {날짜}’를 표시한다. 원인·날짜를 아직 제공할 수 없다면 ‘복귀일은 카드 목록에서 확인해요.’로 제한한다. |
| M3-04 · 중간 · 문구 수정 | [D5-c](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2294): ‘막힌 카드 없이 끝냈어요’ | 초보: 한 번도 틀리지 않았다는 칭찬으로 읽는다. 기존: 접었다가 다시 푼 카드가 있어도 같은 문구가 나오는 이유를 알 수 없다. | ‘남겨둔 접어둔 카드가 없어요.’ 보조문구 ‘오늘 예정 학습을 마쳤어요. 내일 예상 분량을 확인해 보세요.’ 접어둔 카드가 없다는 사실만 말한다. |
| M3-05 · 중간 · 문구 수정 | [D3 답변과 평가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=17-1351): ‘Again’ / ‘Hard’ / ‘Good’ / ‘Easy’ / ‘다음 학습부터는 숨겨요.’ | 초보: 영어와 난이도 평가를 기억 여부 평가로 연결하지 못한다. 기존: 익숙한 버튼명은 유용하지만 Addiem의 ‘일부만 기억함’ 기준을 놓칠 수 있다. | 원래 영문은 유지하고 한국어 기준을 항상 다시 펼칠 수 있게 한다. 안내 ‘기억한 정도를 고르면 다음 복습 일정을 정해요.’ 숨기는 것은 첫 사용 안내로 제한한다. 평가 기준 자체를 바꾸려면 서술형·목록 추천 규칙과 함께 별도로 결정한다. |

## 5. MVP 4 — 카드 품질 진단

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| M4-01 · 높음 · 문구 수정 | [G1 점검 목록](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-5650): ‘그대로 두기’ | 초보: 단순히 수정을 취소한다고 생각한다. 기존: 다시 점검되지 않는 기간을 알 수 없다. | G1 버튼 ‘30일간 점검에서 숨기기’. G2·L4 창의 같은 행동도 ‘나누지 않고 30일 숨기기’로 명시한다. 학습·카드 내용은 유지된다는 설명을 붙인다. 30일은 quality 구현에서 확인했다. |
| M4-02 · 중간 · 문구 수정 | [G2 나누기 제안](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-5871): ‘원래 카드는 보관해요’ | 초보: 어디에서 다시 찾는지 모른다. 기존: 덱 보관과 카드 학습 중지가 같은 기능이라고 생각한다. | 체크 항목 ‘원래 카드의 학습을 중지해요’. 설명 ‘내용과 복습 기록은 남아요. 나누기를 되돌리면 원래 상태로 돌아가요.’ 변경 기록을 여는 실제 동선을 바로 제공한다. 내부 suspended 상태를 덱 보관과 구분한다. |
| M4-03 · 중간 · 문구 수정 | [G2](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-5871): ‘조직 문화의 10가지 특성 중 ①–③은?’ | 두 사용자 모두: 원래 목록 순서를 외우지 않으면 질문만으로 답의 범위를 정할 수 없다. 나눴지만 독립적으로 학습하기 어려워진다. | 예시 ‘조직 문화 특성 중 구성원·집단·사람을 중심으로 보는 세 가지는?’처럼 범위가 드러나는 질문으로 검토·수정한다. 번호 분할 초안에는 ‘질문만 읽고 답할 수 있는지 확인해 주세요.’를 표시한다. 모든 자동 초안을 의미별 분류로 단정하지 않는다. |
| M4-04 · 중간 · 문구 수정 | [L4 나누기 결과](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=73-8640): ‘성격별로 묶기’ / [G2](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-5871): ‘모델이 필요해요’ | 초보: 성격이 사람의 성향인지 내용 분류인지 모른다. 기존: 로컬 분석 모델 다운로드와 ChatGPT 연결을 혼동한다. | ‘비슷한 내용끼리 나누기 · ChatGPT’로 통일한다. 연결 전 안내 ‘ChatGPT 연결이 필요해요.’ 로컬 중요도 분석 모델은 별도 명칭을 쓴다. |
| M4-05 · 중간 · 문구 수정 | [R1-c 초안 검토](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=80-9793): ‘근거와 다름 1’ / ‘근거 구간에 없는 말이 있어요’ | 초보: 근거에 없다는 이유만으로 틀린 내용으로 판단한다. 기존: 외부 지식 추가인지 원문과 모순인지 구분할 수 없다. | 필터 ‘근거에서 확인되지 않은 내용 1’. 경고 ‘선택한 자료 구간에서 “L1 벌점”을 확인하지 못했어요. 내용을 검토해 주세요.’ 실제 모순을 검증한 경우에만 ‘근거와 다름’을 쓴다. |

## 6. MVP 5 — 시험 준비도와 학습 현황

현재 UI가 제공하는 것은 학습 진도·예상 분량·자기 평가 기록이다. 원래 제품 방향의 ‘시험 준비도’는 문서 분류명으로만 사용하고, 새 점수나 합격 확률은 만들지 않는다.

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| M5-01 · 높음 · 문구 수정 | [C1](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-800): ‘핵심 카드 학습률’ / ‘본 카드’ | 초보: 82%를 숙달률·정답률로 읽는다. 기존: 복습 안정도에 따른 수치인지 신규 상태를 벗어난 비율인지 모른다. | ‘학습을 시작한 핵심 카드’ / ‘학습을 시작한 카드’. 도움말 ‘신규 상태가 아닌 카드의 비율이에요. 정답률이나 숙달도를 뜻하지 않아요.’ 구현은 state가 new가 아닌 카드를 센다. |
| M5-02 · 높음 · 문구 수정 | [C1](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-800): ‘목표일까지 완주 가능’ | 초보: 시험 범위를 충분히 암기할 수 있다는 보장으로 받아들인다. 기존: 예정 복습까지 완료하는 계산으로 오해한다. | ‘현재 분량이면 신규 카드 첫 학습을 목표일까지 마칠 수 있어요.’ 작은 배지는 ‘신규 첫 학습 완료 예상’. 학습 속도와 설정에 따라 달라지는 추정임을 상세에 표시한다. on_track은 남은 신규와 일일 분량을 기준으로 한다. |
| M5-03 · 높음 · 문구 수정 | [E2 학습 지표](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-2882): ‘합격선 70% 넘음’ / ‘합격선 80% 넘음’ / ‘MVP 범위 문서 12장’ | 초보: 실제 시험 합격 기준이라고 읽는다. 기존: 개인 성과 화면에 제품 검증용 문서·기준이 나오는 이유를 모른다. | 사용자 화면은 ‘일일 계획 완료율’과 ‘접어둔 카드가 있었던 학습의 완료 비율’만 설명한다. 제품 가설의 70%·80% 판정과 MVP 문서 참조는 내부 검증 문서에 둔다. 개인 목표값으로 임의 대체하지 않는다. |
| M5-04 · 중간 · 설계 확인 | [E2-0 데이터 부족](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-3008): ‘판정에는 14일 기록이 필요해요 · 지금 3일째’ / ‘판정 전 · 12일 남음’ | 두 사용자 모두: 경과일·기록일·관측 시작일을 구분할 수 없고 14일과 3일의 차이도 맞지 않아 보인다. | ‘기록이 있는 날 {n}일 / 기준 14일’처럼 계산에 쓰는 단위를 명시한다. 사용자 화면에서 내부 판정을 제거하면 ‘아직 기록이 적어요. 현재까지의 추이예요.’로 바꾼다. 3일·12일 예시를 같은 집계 정의로 정합화한다. |
| M5-05 · 중간 · 문구 수정 | [C1-t 오늘 진행](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-3271): ‘오늘 남은 48장’ / ‘복습 32’ / ‘신규 40’ | 두 사용자 모두: 남은 양의 하위 합계가 72장이라 48장의 의미를 이해할 수 없다. | 남은 분량을 나누어 표시하거나, 기존 32·40 위에 ‘처음 계획 72장’을 명시한다. ‘완료 24 / 계획 72 · 남음 48’과 남은 복습·신규를 같은 기준으로 표시한다. 홈의 큰 ‘오늘’도 ‘오늘 남은 카드’로 맞춘다. |
| M5-06 · 중간 · 문구 수정 | [E2-h 학습 기록](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=35-4254): ‘하루에 평가한 카드 수’ / ‘카드 64장’ | 초보: 평가 횟수와 카드 수를 구분하기 어렵다. 기존: 같은 카드의 반복 평가가 중복 집계되는지 모른다. | ‘하루 학습한 카드 수 · 같은 카드는 하루에 한 번 집계’. 툴팁 ‘학습 카드 64장 · 덱 3개’. 현재 heatmap 구현은 카드 ID를 하루별 집합으로 중복 제거하므로 평가 횟수로 바꾸지 않는다. 삭제된 카드의 ID가 사라진 기록은 시각으로 구분하는 예외가 있어 별도 집계 검증이 필요하다. |
| M5-07 · 중간 · 문구 수정 | [C1-w 약한 챕터](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=49-5376): ‘약해 보여요?’ / ‘약함으로’ / ‘최근 14일 Again’ | 초보: 무엇을 판단하고 확정하는지 불분명하다. 기존: 수동 지정과 자동 판정을 구분하기 어렵다. | ‘취약 챕터 제안’, 버튼 ‘취약 챕터로 지정’ / ‘제안 숨기기’. 지표 ‘최근 14일 기억 못함(Again) 비율’. 제안 근거와 기록 부족 표시는 유지한다. |

## 7. 추가 기능군

### 7.1 가져오기·내보내기

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| X1-01 · 높음 · 문구 수정 | [B2-r 기록 이전](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3175): ‘복습 기록과 일정 그대로 옮기기’ / ‘Anki에서 하던 그대로 이어서 복습해요.’ | 초보: 일정 이전의 의미를 알기 어렵다. 기존: Anki의 모든 일정·스케줄러 설정까지 동일하다고 예상한다. | 선택명 ‘복습 기록과 다음 복습 일정 가져오기’. 설명 ‘가져온 일정으로 시작하고, 이후 평가는 Addiem의 일정 계산을 따라요. 학습 순서·일일 분량은 Addiem 설정을 적용해요.’ 원본을 수정하지 않는다는 안내는 유지한다. |
| X1-02 · 높음 · 구현과 차이 | [B2](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-367): ‘12장은 앞면만 표시돼요’ / [B3](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-615): ‘입력칸 없이 앞면·뒷면만 보여요.’ | 두 사용자 모두: 답변이 사라지는 것인지입력칸만 달라지는 것인지 알 수 없다. | ‘Anki의 답 입력 템플릿을 쓰는 카드 12장 · 질문과 답은 표시돼요.’ 현재 render 구현은 앞면의 type 입력 자리는 비우고 뒷면에 정답 필드를 표시한다. 실제 카드의 직접 쓰기 모드 적용은 별도 확인해 안내한다. B2 기본 경로의 ‘아직 옮기지 않아요’도 기록 이전 선택과 공존시키지 않는다. |
| X1-03 · 중간 · 문구 수정 | [B2-j JSON](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-491): ‘성공한 312장만 가져오기’ / [B3-j](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=27-2852): ‘항목 5개 · 빈칸 항목이 카드 2장’ | 초보: 검사 통과와 가져오기 완료를 혼동한다. 기존: 입력 노트 수와 생성 카드 수가 섞인다. | 실행 전 ‘검사를 통과한 312개 항목 가져오기’. 수량 요약은 ‘입력 항목 {n}개 → 생성 예정 카드 {m}장’. 완료 뒤에만 ‘가져온 카드’를 쓴다. 건너뛴 항목의 JSON 복사는 유지한다. |
| X1-04 · 중간 · 문구 수정 | [B2-j](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-491): ‘back 이 비어 있어요’ / ‘front 키가 없어요’ / ‘importance 값’ | 초보: 자신의 질문·답 중 무엇을 고쳐야 하는지 모른다. 기존: 필드명은 유용하나 위치를 다시 찾아야 한다. | ‘47번째 항목: 답변(back)이 비어 있어요.’ / ‘128번째 항목: 질문(front) 항목이 없어요.’ / ‘88번째 항목: 중요도(importance)를 읽지 못해 태그 기준으로 정했어요.’ 기술 키는 오류 수정에 필요하므로 괄호로 유지한다. |
| X1-05 · 중간 · 문구 수정 | [B2-s 덱 선택](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=49-5963): ‘이미 있는 노트 780개 · 가져와도 건너뛰어요’ / ‘고른 3,018장 가져오기’ | 초보: 버튼 숫자만큼 새 카드가 추가된다고 생각한다. 기존: 재가져오기로 수정 내용까지 갱신된다고 예상한다. | 버튼 옆에 ‘선택 {n}장 · 이미 있는 노트는 추가·갱신하지 않아요.’를 표시한다. 중복을 제외한 생성량을 계산할 수 있으면 ‘새로 추가할 카드 {m}장’을 별도로 보여준다. |
| X1-06 · 중간 · 문구 수정 | [C1-xr 미디어 누락](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=38-4947): ‘내보냈어요’ / ‘카드 3장은 그림 없이 들어갔어요.’ | 초보: 현재 앱의 카드에서 그림이 삭제되었다고 생각한다. 기존: 내보내기 파일의 누락인지 원본 데이터 변형인지 다시 확인해야 한다. | ‘내보냈지만 그림 파일 2개가 빠졌어요.’ / ‘저장한 .apkg 파일에서 카드 3장의 그림이 표시되지 않아요.’ 미디어가 음성을 포함하면 ‘미디어 파일’과 실제 종류를 함께 표시한다. |
| X1-07 · 중간 · 문구 수정 | [Q3 Anki 호환](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=128-13246): ‘Anki에서는 고르지 못하고 보기만 해요’ / [C1-xe](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=38-4677): ‘Anki 덱 패키지(.apkg) 파일로 저장해요’ | 초보: 같은 상호작용이 Anki에서도 된다고 생각한다. 기존: 내보내기 직전에는 목록·선택형의 기능 손실을 볼 수 없다. | 해당 유형 포함 시 내보내기 확인창에 ‘Anki에서는 목록을 하나씩 공개하거나 보기를 선택할 수 없어요. 질문·정답은 볼 수 있어요.’를 표시한다. Q3 설명 보드에만 두지 않는다. |

### 7.2 Google Drive 동기화·백업

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| X2-01 · 높음 · 문구 수정 | [K6-m 처음 합치기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=63-7341): ‘둘 다 남기고 합치기’ / ‘내용이 다르면 한쪽으로 맞춘 뒤 다른 쪽은 30일 동안 기록에 남겨요.’ | 초보: 두 내용이 현재 카드에 모두 남는다고 생각한다. 기존: 같은 노트의 충돌 버전도 영구 보존된다고 오해한다. | 선택명 ‘두 데이터 합치기 · 충돌 내용 확인 가능’. 설명 ‘덱·카드·복습 기록을 합쳐요. 같은 카드의 내용이 다르면 한 버전을 현재 내용으로 쓰고, 다른 버전은 30일 동안 변경 내역에 보관해요.’ 충돌 기준과 접근 위치를 바로 보여준다. |
| X2-02 · 높음 · 문구 수정 | [K9 백업 복구](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7577): ‘동기화도 이 상태로 새로 시작해요’ / ‘되돌리기’ | 초보: 이 기기만 과거 상태로 돌아간다고 생각한다. 기존: 다른 기기의 이후 병합·선택에도 영향이 있다는 점을 놓친다. | 버튼 ‘이 백업으로 복구’. 설명 ‘이 기기를 선택한 백업 상태로 바꾸고, Drive 동기화 기준도 이 상태로 바꿔요. 다른 기기는 다음 동기화 때 처리 방법을 선택해요.’ 복구로 빠지는 기록 수와 사전 로컬 백업 안내는 버튼 가까이에 유지한다. |
| X2-03 · 높음 · 구현과 차이 | [K10 연결 끊기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7776): ‘Drive에서도 지우기’ 선택과 실행 버튼 ‘연결 끊기’ | 초보: 연결만 해제한다고 생각할 수 있다. 기존: 삭제 선택 상태의 버튼명이 Figma에 없어 결과를 확인하기 어렵다. | 데이터 유지 선택 시 ‘연결만 끊기’, 삭제 선택 시 ‘Drive 데이터 삭제하고 연결 끊기’. Figma에 삭제 선택 상태를 추가해 버튼 변화를 명시한다. 구현의 DisconnectModal은 이미 삭제 선택 시 ‘끊고 Drive에서 지우기’로 분기한다. |
| X2-04 · 중간 · 문구 수정 | [K4-r 설정](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=101-10734): ‘Drive 백업 9개’ / ‘카드 · 기록 · 설정만 담은 파일 · 그림은 동기화 공간에 한 번만 보관’ | 초보: 백업 하나에 그림과 자료도 모두 들어 있다고 생각한다. 기존: 내려받은 백업 파일만으로 완전 복구가 가능한지 모른다. | 이름 ‘Drive 기록 백업’. 설명 ‘카드·복습 기록·설정이 들어 있어요. 그림은 Drive 동기화 공간에 별도로, 원자료 파일은 이 기기에만 있어요.’ 로컬 백업과 포함 항목을 나란히 비교해 보여준다. |
| X2-05 · 중간 · 문구 수정 | [K5 동기화 상태](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=62-7207): 오프라인 아이콘의 ‘12’ / ‘변경 12개’ / [K7](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7406): ‘나중에 동기화한 쪽’ | 초보: 12개가 남은 카드인지 오류 수인지 모른다. 기존: 최신 수정 시각과 서버 반영 순서를 혼동한다. | 상태 텍스트 ‘전송 대기 변경 12건 · 이 기기에 저장됨’. 충돌 설명 ‘Drive에 나중에 반영된 버전을 현재 내용으로 사용했어요.’ ‘수정 시각이 더 최근’이라는 의미로 바꾸지 않는다. |
| X2-06 · 중간 · 문구 수정 | [K1-n 연결 불가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=67-7910): ‘이 빌드에는 Google 연결이 설정되지 않았어요’ / ‘직접 빌드한 앱이라’ | 초보: 자신의 Google 계정이나 로그인 설정 문제로 여긴다. 기존: 해결 행동을 알 수 없다. | ‘이 앱 버전에서는 Google Drive 연결을 사용할 수 없어요.’ / ‘설정된 배포 버전이 필요해요. 지금은 이 기기 백업을 사용할 수 있어요.’ 개발자 대상 빌드 설명은 상세 도움말로 옮긴다. |

### 7.3 AI 연결·자료 기반 생성·서술형

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| X3-01 · 높음 · 문구 수정 | [L1 AI 연결](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=71-8023): ‘직접 고른 카드와 자료 구간만 OpenAI로 보내요’ / ‘덱·기록·원자료·그림은 안 보내요’ | 초보: 자료의 내용은 전혀 전송되지 않는다고 이해한다. 기존: 자동 연결·유사 구간도 ‘직접 고른’ 것으로 볼 수 있는지 모호하다. | ‘실행할 때 표시되는 카드 글자와 자료 구간을 OpenAI로 보내요.’ / ‘원자료 파일 전체·그림·복습 기록은 보내지 않아요.’ 자료 발췌문과 파일 전체를 구분하고, 직접·자동 선택 여부를 전송 목록에 표시한다. |
| X3-02 · 높음 · 구현과 차이 | [R2 채점 보조](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=81-9865): ‘보낸 것: 내 답 · 모범 답 · 자료 2구간’ / [R2-s](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=81-10044): ‘OpenAI로 보낼 내용’ | 두 사용자 모두: 질문도 전송되는지 알 수 없다. 세션의 두 번째 카드부터 보내는 범위가 바뀌어도 놓칠 수 있다. | ‘질문 · 내 답 · 모범 답안 · 자료 2구간을 보내요.’ 전송 전후 요약과 상세 목록을 같은 항목으로 맞춘다. 현재 GradeAssist의 목록과 rag의 요청 본문 생성 모두 질문을 포함한다. |
| X3-03 · 중간 · 문구 수정 | [L1-c 연결됨](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=71-8205): ‘끊으면 바로 멈춰요.’ | 초보: 이미 전송된 내용도 회수되거나 삭제된다고 읽을 수 있다. 기존: 진행 중 요청·향후 요청·로컬 카드 중 무엇이 멈추는지 모른다. | ‘연결을 끊으면 이후 AI 요청을 보내지 않아요. 이미 만든 카드는 남아요.’ 진행 중 요청 중단 가능 여부는 구현으로 확인해 별도로 설명한다. 외부에 이미 보낸 내용의 삭제를 약속하지 않는다. |
| X3-04 · 중간 · 문구 수정 | [J2 서술형 비교](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-7329): ‘정답’ / ‘핵심어 5개 중 3개’ / ‘Hard 추천’ | 초보: 표현이 다르면 오답이거나 AI가 정답을 확정했다고 생각한다. 기존: 핵심어 일치 검사와 의미 기반 채점 보조를 구분하기 어렵다. | 비교 열 ‘모범 답안’, 집계 ‘등록 핵심어 5개 중 3개 확인’. 추천 ‘핵심어 기준 Hard 제안 · 답의 의미를 확인하고 직접 평가해 주세요.’ R2는 ‘핵심어 검사’와 ‘ChatGPT 검토’를 나누어 표시한다. |
| X3-05 · 중간 · 문구 수정 | [R1-a 자료](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=78-9379): ‘빠진 범위 18’ / ‘강조됐는데 아직 카드가 없는 곳’ | 초보: 시험 범위를 자동으로 파악해 누락 개념을 찾았다고 믿는다. 기존: 미연결 카드가 있어도 실제 카드가 없다는 뜻으로 오해한다. | ‘카드 미연결 강조 구간 18곳’. 설명 ‘강조 구간 중 연결된 카드가 없는 곳이에요. 이미 만든 카드가 있다면 연결해 주세요.’ 범위 분석의 한계를 정확히 표현한다. |
| X3-06 · 중간 · 문구 수정 | [I1 자료](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6560): ‘카드 잇기’ / [I2](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6730): ‘원자료’ / ‘연결 바꾸기’ | 초보: 자료에서 새 카드를 생성하는 것과 기존 카드를 연결하는 것을 구분하지 못한다. 기존: ‘자료’와 ‘원자료’가 다른 저장 대상인지 헷갈린다. | 버튼 ‘기존 카드 연결’, 새 생성은 ‘카드 초안 만들기’. 탐색 이름은 ‘학습 자료’, 카드 옆 열기는 ‘연결 자료 보기’로 맞춘다. ‘원자료 파일’은 파일 전체 전송·백업 설명에서 사용한다. |
| X3-07 · 중간 · 설계 확인 | [L2 연결 시트](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=72-8183): ‘ChatGPT 요금제 사용량이 Codex와 함께 줄어요’ / ‘이 앱의 한도를 정할 수 있어요’ | 초보: 월 구독료가 추가 차감된다고 생각할 수 있다. 기존: 자신에게 적용되는 한도와 실제 관리 화면을 확인하기 어렵다. | 과금 보장 문구와 로그인 브랜드 버튼을 임의로 바꾸지 않는다. 기능 제공 조건·공유 한도·앱별 한도 설정의 실제 계정 동작을 출시 전에 검증한다. 확인된 경우 안내 ‘요청할 때 연결한 계정의 사용 한도를 사용해요.’를 쓰고 실제 관리 화면으로 연결한다. 본 검토에서는 외부 요금제 정책을 검증하지 않았다. |

### 7.4 카드 작성·그림 가리기·목록·소리

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| X4-01 · 중간 · 문구 수정 | [A1 빈 홈](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=11-48): ‘아직 학습할 덱이 없어요’ / ‘새 덱 만들기’ | 초보: 덱이 카드인지 과목 폴더인지 모르고 첫 행동을 고르기 어렵다. 기존: 익숙한 용어라 문제없다. | ‘덱은 학습 카드를 모아 두는 묶음이에요. 과목이나 시험별로 만들어 보세요.’를 빈 홈에 한 번만 추가한다. 기본 용어 ‘덱’은 유지한다. |
| X4-02 · 높음 · 문구 수정 | [K6 빈 홈](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=63-7167): ‘모든 데이터는 이 기기에만 저장돼요’와 ‘Google Drive에서 불러오기’ | 초보: 연결 후에도 외부 저장이 없다고 믿는다. 기존: 로컬 기본 저장과 선택적 동기화를 모순으로 인식한다. | 연결 전 ‘기본 저장 위치는 이 기기예요.’ 연결 후에는 ‘이 기기에 저장 · Google Drive 동기화 사용 중’. AI·동기화 여부에 따라 달라지는 사실을 고정된 ‘모든’ 문구로 표현하지 않는다. |
| X4-03 · 중간 · 문구 수정 | [C3-r 앞뒤 둘 다](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-4398): ‘앞뒤 둘 다’ / [C3-b 빈칸](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1411): ‘빈칸마다 카드 1장이 생겨요.’ | 초보: 한 카드에 양면을 쓰는 일반 카드와 역방향 카드 생성을 구분하지 못한다. 기존: 같은 빈칸 번호의 묶음 처리와 단순 빈칸 수를 혼동한다. | 유형 ‘양방향 · 카드 2장’. 설명 ‘앞→뒤, 뒤→앞으로 묻는 카드가 각각 생겨요.’ 빈칸은 ‘빈칸 번호마다 카드 1장이 생겨요. 같은 번호는 한 카드에서 함께 물어요.’ 생성 예정 장수는 저장 버튼 가까이에 표시한다. |
| X4-04 · 높음 · 문구 수정 | [Q1-a 목록 회상](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=124-13041): ‘연 항목은 기억한 것으로 세요. 못 떠올렸으면 X’ / [Q1-b](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=124-12869): ‘7개 중 5개 기억’ | 초보: 앱이 기억 여부를 판정했다고 생각한다. 기존: 공개만 했는데 기억 수와 평가 추천이 높아질 수 있다. | ‘답을 연 뒤 못 떠올렸던 항목을 “기억 못함”으로 표시해 주세요.’ 결과는 ‘직접 표시한 결과: 7개 중 5개 기억’. 항목 클릭·X는 ‘기억 여부 바꾸기’로 명시한다. 추천이 사용자 표시 기준임을 유지한다. |
| X4-05 · 중간 · 문구 수정 | [M2 가리기 편집기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=107-11368): ‘완료’ / [M1](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=107-11027): ‘칸 6개 · 카드 5장’ | 초보: 편집기 완료로카드까지 저장됐다고 생각한다. 기존: 칸 수와 카드 수가 다른 이유, 같은 그림의 다른 카드도 수정되는 범위를 놓친다. | 신규 작성 경로의 편집기 버튼 ‘가리기 적용’. 안내 ‘가릴 영역 6개를 5개 질문으로 묶었어요. 카드 추가 화면에서 저장해 주세요.’ 기존 노트 편집에서는 완료 시 save가 호출되므로 버튼 ‘변경 저장’, 안내 ‘같은 그림에서 만든 카드에 함께 반영돼요.’로 구분한다. |
| X4-06 · 낮음 · 문구 수정 | [M5 오류](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=108-11457): ‘칸이 없으면 [완료]가 꺼져 있어요.’ / ‘카드 42장이 생겨요’ | 초보: 버튼이 꺼졌다는 표현이 기능 불가 원인을 설명하지 못한다. 기존: 영역을 묶었을 때 생성 수가 달라지는 점을 안다. | ‘가릴 영역을 하나 이상 그려야 적용할 수 있어요.’ 많은 영역 경고는 실제 그룹 기준으로 ‘영역 {n}개 · 생성할 카드 {m}장’으로 표시한다. |
| X4-07 · 중간 · 문구 수정 | [AU1 소리](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=134-13664): ‘파일 없음 · abandon_ex.mp3’ | 초보: 원인과 해결 방법을 알 수 없다. 기존: 가져오기 누락인지 재생 장치 문제인지 구분하고 싶다. | ‘소리 파일을 찾을 수 없어요 · abandon_ex.mp3’. 오류 위치에서 ‘소리 파일 확인’ 동선을 제공한다. 자동 재생·다시 듣기 설명은 유지하고, 해당 파일의 부재를 재생 실패 전반으로 확대하지 않는다. |

### 7.5 시험 전 회독과 공통 상태

| ID · 우선순위 · 상태 | 화면·위치 / 원문 | 사용자별 예상 오해 | 바로 쓸 수정안과 표시 방식 |
|---|---|---|---|
| X5-01 · 높음 · 문구 수정 | [P1 시험 전 회독](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12126): ‘압축’ / ‘혼합’ / [H3](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6439): ‘범위 전체를 한 번 훑되’ | 초보: 학습량 차이만 있다고 생각한다. 기존: 한쪽만 복습 일정에 반영된다는 차이를 놓칠 수 있다. | 선택 옆에 ‘압축 · 취약 카드만, 일정 유지’ / ‘혼합 · 배운 카드와 취약 카드, 일정 반영’을 함께 보여준다. 압축 설명 ‘설정한 취약 조건에 맞는 카드를 모아 봐요.’ 버튼도 선택 방식을 포함한다. |
| X5-02 · 높음 · 문구 수정 | [P0 회독 입구](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12050): ‘전체를 한 번 훑어볼까요?’ / [P1](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12126): ‘전체 범위’ / ‘아직 안 본 배운 카드’ | 초보: 미학습 신규 카드까지 모두 포함한다고 이해한다. 기존: 전체 카드 수와 이번 회독 분모가 다르면 누락을 의심한다. | 입구 ‘배운 카드를 다시 훑어볼까요?’ 범위 ‘이미 학습한 카드 전체’. 진행률 ‘이번 회독에서 확인한 배운 카드 {n}/{m}장’. 신규 카드는 오늘의 학습으로 진행한다는 설명을 선택 가까이에 둔다. |
| X5-03 · 중간 · 문구 수정 | [P1](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12126): ‘취약 끼워 넣기’ / 선택 ‘적게’, ‘보통’, ‘많이’ / ‘1/5 · 1/3 · 1/2’ | 두 사용자 모두: 선택된 ‘보통’이 어느 비율인지, 비율이 전체 기준인지 추가 카드 기준인지 계산해야 한다. | ‘취약 카드 비율’, 선택 ‘20% · 약 33% · 50%’. 설명 ‘일반 2장마다 취약 1장’처럼 현재 선택의 관계를 한 줄로 표시한다. 전체/취약 카드의 중복 제거 설명을 유지한다. |
| X5-04 · 중간 · 문구 수정 | [S1 저장 실패](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-3850): ‘디스크 공간이 부족하거나 파일이 잠겨 있어요.’ | 초보: 두 원인 중 무엇을 고쳐야 하는지 알 수 없다. 기존: 원인을 확인하지 않고 단정했다고 여길 수 있다. | 원인을 알면 해당 원인을 구체적으로 표시한다. 불명일 때 ‘저장 중 오류가 발생했어요. 입력한 내용은 유지돼요.’ 확인 가능한 상세 오류와 ‘다시 시도’를 함께 제공한다. |
| X5-05 · 중간 · 문구 수정 | [E3 보관한 덱](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-3133): ‘되살리기’ / ‘새 카드도 들어오지 않아요’ | 초보: 삭제된 덱의 복원으로 읽거나 카드 추가 자체가 막힌다고 생각한다. 기존: 덱의 학습 제외와 신규 투입 중지를 구분하고 싶다. | 버튼 ‘보관 해제’. 설명 ‘보관한 덱은 홈과 오늘의 학습에서 제외돼요. 보관을 해제하면 학습에 다시 포함돼요.’ 밀린 복습의 200장 상한은 ‘앱 전체 하루 기준’임을 상세에 명시한다. |

## 8. 공통 용어와 적용 원칙

| 개념 | 권장 표기 | 사용 규칙 |
|---|---|---|
| Deck | 덱 | 첫 빈 홈에서만 ‘학습 카드 묶음’으로 설명한다. 과목·폴더를 무조건 덱과 동의어로 쓰지 않는다. |
| Note / Card | 노트 / 카드 | 초보에게는 ‘같은 내용에서 만들어진 카드들(Anki 노트)’로 설명한다. 입력 항목 수·노트 수·생성 카드 수의 단위를 바꾸지 않는다. |
| New | 신규 카드 / 아직 학습하지 않은 카드 | 짧은 목록은 ‘신규’, 첫 안내는 풀어서 쓴다. 다시 가져와 신규로 시작하는 경우 원래 처음 본 자료라는 뜻으로 오해하지 않게 설명한다. |
| Review | 복습 | 예정일이 지난 복습은 ‘밀린 복습’. 앞면 공개를 ‘복습 완료’로 단정하지 않는다. |
| Importance | 중요도 · 핵심 / 일반 / 보충 | 우선순위와 숙달 정도를 구분한다. 보충을 학습 제외로 표현하지 않는다. |
| Buried | 접어두기 | 실제 복귀 날짜를 함께 쓴다. ‘다음’이나 ‘나중’만으로 기간을 설명하지 않는다. |
| Suspended | 카드 학습 중지 | 덱 보관·기간 있는 접어두기와 구분한다. |
| Paused deck | 덱 학습 쉬기 / 다시 시작 | 기간과 자동 재개일을 함께 표시한다. 목표일이 유지된다는 설명을 가까이 둔다. |
| Archived deck | 덱 보관 / 보관 해제 | ‘되살리기’ 대신 저장 상태의 변화를 직접 표현한다. |
| Weak chapter | 취약 챕터 / 취약 챕터 제안 | 사용자 확정과 통계 기반 제안을 구분한다. |
| Mastery / progress | 학습 시작 비율 / 회독 진행률 | 실제 측정값만 이름에 넣는다. 숙달·정답률·시험 준비도와 섞지 않는다. |
| Source | 학습 자료 / 연결 자료 / 원자료 파일 | 탐색·카드 옆 열기·파일 전체의 전송 설명에 각각 쓴다. |
| Keyword | 핵심어 | ‘필수 키워드’와 ‘핵심어’ 혼용을 줄이고 등록·제안·확인 상태를 구분한다. |
| AI splitting | 비슷한 내용끼리 나누기 · ChatGPT | 로컬 규칙 분할·로컬 분석 모델·외부 AI 요청을 같은 ‘모델’로 부르지 않는다. |
| Sync / Backup / Restore | 동기화 / 백업 / 백업으로 복구 | 현재 상태 맞추기·시점 보관·시점 복구를 구분한다. 평가 취소는 ‘마지막 평가 되돌리기’로 유지한다. |
| Counts | 카드 n장 / 노트 n개 / 평가·변경 n회·n건 / 자료 구간 n곳 | 합계와 세부 항목의 분모·집계 기간을 일치시킨다. |

문장 톤은 현재의 해요체를 유지한다. ‘허용되지 않아요’보다 할 수 있는 다음 행동을 구체적으로 쓰고, ‘그대로·모두·바로·언제든’은 실제 적용 범위가 확인될 때만 쓴다. 설명을 늘릴 때는 짧은 버튼명, 바로 아래의 결과 설명, 필요할 때 여는 상세 도움말 순서로 배치한다. 중요한 기간·영향 범위는 툴팁에만 숨기지 않는다.

이미 이해를 돕는 표현도 유지한다. B1의 덱 이름 입력과 나중에 목표를 정할 수 있다는 안내, B2-0의 Anki 내보내기 경로, D3-u의 ‘복습 일정도 원래대로예요’, C1 메뉴의 하위 덱 동반 삭제 안내, 수식 오류의 위치·저장 가능 여부 안내, AI 초안의 승인 전 미생성 안내가 이에 해당한다.

## 9. 전체 프레임 검토 목록

모든 아래 항목은 **이미지·원문 검토 완료**다. 다크 변형은 같은 원문의 기본 화면 지적을 함께 적용한다. 관련 항목은 그 프레임에서 검토한 문제를 묶은 것으로, 표의 모든 문구가 각 프레임에 반복된다는 뜻은 아니다. ‘추가 지적 없음’은 이 검토에서 별도 문구 문제를 발견하지 못했다는 뜻이며, 사용성 검증을 통과했다는 뜻은 아니다.

| 화면·상태 (Figma 원래 이름) | 관련 피드백 |
|---|---|
| [A1 빈 홈](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=11-48) | X4-01 |
| [A2 사용 중인 홈](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=11-126) | M5-05 |
| [A2-b 오늘 학습 끝](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=11-302) | M3-01 |
| [B1 새 덱 만들기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-293) | 추가 지적 없음 |
| [B2 Anki 덱 가져오기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-367) | X1-02 |
| [B2-j JSON 가져오기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-491) | X1-03·04 |
| [B3 가져오기 결과](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-615) | X1-02 |
| [B4 중요도 정하기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=12-745) | M2-01·02 |
| [C1-a 빈 덱 상세](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-691) | 추가 지적 없음 |
| [C1 덱 상세](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-800) | M3-03, M5-01·02 |
| [C2 카드 목록](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-982) | M2-03 |
| [C3 카드 추가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1236) | M2-03 |
| [C3-b 카드 추가 · 빈칸](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1411) | M2-03, X4-03 |
| [C4 목표 및 학습 설정](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=14-1559) | M1-01·02 |
| [D1 학습 시작 전 요약](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=17-1156) | M2-01, M3-03 |
| [D2 질문](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=17-1289) | 추가 지적 없음 |
| [D3 답변과 평가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=17-1351) | M3-05 |
| [D4 어려운 카드 접어두기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=18-1268) | M3-01·02 |
| [D5 학습 완료](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=18-1358) | M3-01·02 |
| [D5-b 중간에 그만둔 경우](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=18-1492) | M3-01·02 |
| [E1 백업과 기본 설정](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=19-1431) | X5-04 |
| [E1-b 데이터 삭제 확인](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=19-1593) | 추가 지적 없음 |
| [C1-b 덱 상세 · 목표일 없음](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=22-1662) | M3-03 |
| [C4-b 목표 설정 · 상한 부족](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=22-1849) | M1-01·02·03 |
| [B4-g 중요도 · 과목별 묶음 (실데이터)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=24-1828) | M2-01·02·04 |
| [C3-e 카드 편집 (학습 중 E · 카드 목록 편집)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=24-2204) | 추가 지적 없음 |
| [D3-l 긴 답변 · 그림 카드](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=24-2332) | M3-05 |
| [D4-m 접어둔 카드 목록 (칩 클릭)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2160) | M3-01·02 |
| [D5-c 학습 완료 · 접어둔 카드 없음](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2294) | M3-04 |
| [B2-0 가져오기 · 파일 고르기 전](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2449) | 추가 지적 없음 |
| [B2-p 가져오는 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2560) | X1-03 |
| [C2-m 카드 목록 · 중요도 메뉴](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-2864) | M2-03 |
| [C2-d 카드 삭제 확인](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-3120) | 추가 지적 없음 |
| [C1-m 덱 메뉴 (⋯)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-3306) | M3-03, M5-01·02, X5-05 |
| [C1-s 하위 덱 만들기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-3468) | 추가 지적 없음 |
| [S1 공통 상태 · 로딩 · 완료 · 오류](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=26-3850) | X5-04 |
| [B3-j 가져오기 결과 · JSON](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=27-2852) | X1-03·04 |
| [E2 설정 · 학습 지표](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-2882) | M5-03 |
| [E2-0 학습 지표 · 아직 데이터가 모자람](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-3008) | M5-03·04 |
| [E3 설정 · 보관한 덱](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-3133) | X5-05 |
| [C1-t 덱 상세 · 오늘 완료 + 하위 덱 목록](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=29-3271) | M5-01·02·05 |
| [B2-r 가져오기 · 복습 기록 이전 선택](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3175) | X1-01·02 |
| [B3-r 가져오기 결과 · 기록 옮김](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3307) | X1-01·02 |
| [D2-u 평가 직후 · 되돌리기 토스트](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3442) | 추가 지적 없음 |
| [D3-u 되돌린 뒤 · 다시 평가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3510) | 추가 지적 없음 |
| [C4-p 목표 설정 · 잠시 쉬기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3610) | M1-01·02 |
| [C1-p 덱 상세 · 쉬는 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3805) | M3-03, M5-01·02 |
| [A2-p 홈 · 쉬는 덱이 있을 때](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=31-3994) | M5-05 |
| [C2-r 카드 행 ⋯ 메뉴 · 하위 덱 이동](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-3649) | M3-01 |
| [C1-r 덱 ⋯ 메뉴 · 잠시 쉬기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-3963) | M1-01, X5-05 |
| [C3-n 카드 추가 · 넣을 하위 덱 고르기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-4229) | M2-03 |
| [C3-r 카드 추가 · 앞뒤 둘 다](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-4398) | M2-03, X4-03 |
| [D5-d 중간에 그만둠 · 접어둔 카드 없음](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-4665) | M3-04 |
| [C2-b 카드 목록 · 일괄 선택](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=34-4781) | M3-01 |
| [E2-h 학습 지표 · 학습 기록 히트맵](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=35-4254) | M5-03·06 |
| [Dark · A2 사용 중인 홈](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=35-4594) | M5-05 |
| [Dark · C1 덱 상세](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=35-4729) | M3-03, M5-01·02 |
| [Dark · D3 답변과 평가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=35-4885) | M3-05 |
| [C1-x 덱 ⋯ 메뉴 · 내보내기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=38-4498) | X1-07 |
| [C1-xe 내보내기 확인](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=38-4677) | X1-07 |
| [C1-xp 내보내는 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=38-4823) | 추가 지적 없음 |
| [C1-xr 내보내기 결과 · 미디어 누락](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=38-4947) | X1-06 |
| [Dark · D3-s 덱 스타일 카드 (밝은 카드 면)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=40-4911) | M3-05 |
| [C3-m 카드 추가 · 수식](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=43-4963) | M2-03 |
| [D3-m 답변 · 수식 렌더링](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=43-5198) | M3-05 |
| [B2-c 가져오기 · CSV](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=45-5105) | X1-03·04 |
| [C1-xc 내보내기 · CSV](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=45-5330) | X1-07 |
| [E4 설정 · 단축키 바꾸기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=45-5593) | 추가 지적 없음 |
| [C1-w 덱 상세 · 약한 챕터](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=49-5376) | M5-01·02·05·07 |
| [B2-s 가져오기 · 덱 골라 가져오기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=49-5963) | X1-01·05 |
| [G1 카드 점검 목록](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-5650) | M4-01 |
| [G2 카드 나누기 제안](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-5871) | M4-01·02·03·04 |
| [G3 학습 중 · 긴 카드 신호](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6032) | M3-02, M4-03 |
| [H1 학습 계획 · 날짜별 분량과 시간](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6121) | M1-04 |
| [H2 목표일 충돌 조정](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6299) | M1-05 |
| [H3 시험 직전 · 압축 회독](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6439) | X5-01 |
| [I1 자료 · 강조 구간](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6560) | X3-06 |
| [I2 학습 중 · 원자료 보기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6730) | X3-06 |
| [I3 자료 기반 중요도 제안](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-6875) | M2-01·02·04, M4-04 |
| [J1 서술형 · 직접 써보기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-7265) | X3-04 |
| [J2 서술형 · 비교와 평가 추천](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-7329) | M3-05, X3-04 |
| [J3 서술형 · 필수 키워드 정하기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=53-7470) | X3-04 |
| [K1 설정 · Google Drive 연결 전](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=61-6616) | X2-04 |
| [K2 브라우저에서 Google 로그인 기다림](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=61-6782) | 추가 지적 없음 |
| [K3 Google Drive 처음 설정](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=61-6937) | X2-04 |
| [K4 설정 · Google Drive 연결됨](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=62-6896) | X2-04·05 |
| [K5 상단 바 · 동기화 상태](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=62-7207) | X2-05 |
| [K6 빈 홈 · Drive에서 불러오기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=63-7167) | X4-01·02 |
| [K6-f Drive에서 찾은 데이터](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=63-7238) | X2-04, X4-02 |
| [K6-m 이 기기에도 데이터가 있을 때](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=63-7341) | X2-01 |
| [K7 두 기기에서 고친 카드](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7406) | X2-01·05 |
| [K8 앱을 닫을 때 남은 변경 올리기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7527) | 추가 지적 없음 |
| [K9 Drive 백업으로 되돌리기 확인](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7577) | X2-02·04 |
| [K10 Google Drive 연결 끊기 확인](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=64-7776) | X2-03 |
| [K1-n 설정 · 이 빌드에는 Google 연결 없음](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=67-7910) | X2-06 |
| [L1 설정 · AI 연결 (꺼짐)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=71-8023) | X3-01·07 |
| [L1-c 설정 · AI 연결 (연결됨)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=71-8205) | X3-03·07 |
| [L2 ChatGPT 연결 시트](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=72-8183) | X3-01·07 |
| [L2-w 브라우저에서 ChatGPT 로그인 기다림](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=72-8377) | X3-07 |
| [L2-x ChatGPT 연결 실패 2가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=72-8549) | X3-07 |
| [L3 첫 연결 환영](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=73-8522) | X3-07 |
| [L4 카드 나누기 · ChatGPT로 묶은 결과](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=73-8640) | M4-01·02·04 |
| [L4-s [성격별로 묶기] 상태 3가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=73-8812) | M4-04, X3-01 |
| [L4-e [성격별로 묶기] 오류 4가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=73-8919) | M4-04 |
| [Dark · L1 설정 · AI 연결 (꺼짐)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-8691) | X3-01·07 |
| [Dark · L1-c 설정 · AI 연결 (연결됨)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-8808) | X3-03·07 |
| [Dark · L2 ChatGPT 연결 시트](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-8926) | X3-01·07 |
| [Dark · L2-w 브라우저에서 ChatGPT 로그인 기다림](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-9083) | X3-07 |
| [Dark · L2-x ChatGPT 연결 실패 2가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-9224) | X3-07 |
| [Dark · L3 첫 연결 환영](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-9247) | X3-07 |
| [Dark · L4 카드 나누기 · ChatGPT로 묶은 결과](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-9374) | M4-01·02·04 |
| [Dark · L4-s [성격별로 묶기] 상태 3가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-9452) | M4-04, X3-01 |
| [Dark · L4-e [성격별로 묶기] 오류 4가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=74-9514) | M4-04 |
| [R1-a 자료 · 빠진 범위](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=78-9379) | X3-05·06 |
| [R1-b 카드 초안 만들기 · 보낼 내용 미리보기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=80-9464) | X3-01 |
| [R1-c 카드 초안 검토](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=80-9793) | M4-05 |
| [R1-e 카드 초안 만들기 오류 4가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=81-9717) | X3-06 |
| [R2 서술형 · ChatGPT 채점 보조 결과](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=81-9865) | M3-05, X3-02·04 |
| [R2-s 채점 보조 · 누르기 전 · 처음 보낼 때 · 기다리는 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=81-10044) | X3-01·02 |
| [R3 성격별로 묶기 · 자료 근거](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-9900) | M4-01·02·04, X3-01 |
| [R3-s 성격별로 묶기 · 보내는 범위 3가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10071) | X3-01 |
| [Dark · R1-a 자료 · 빠진 범위](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10263) | X3-05·06 |
| [Dark · R1-b 카드 초안 만들기 · 보낼 내용 미리보기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10454) | X3-01 |
| [Dark · R1-c 카드 초안 검토](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10714) | M4-05 |
| [Dark · R1-e 카드 초안 만들기 오류 4가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10848) | X3-06 |
| [Dark · R2 서술형 · ChatGPT 채점 보조 결과](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10893) | M3-05, X3-02·04 |
| [Dark · R2-s 채점 보조 · 누르기 전 · 처음 보낼 때 · 기다리는 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-10986) | X3-01·02 |
| [Dark · R3 성격별로 묶기 · 자료 근거](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-11061) | M4-01·02·04, X3-01 |
| [Dark · R3-s 성격별로 묶기 · 보내는 범위 3가지](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=82-11153) | X3-01 |
| [K4-r 설정 · Google Drive 연결됨 (행 개편)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=101-10734) | X2-04·05 |
| [Dark · K4-r 설정 · Google Drive 연결됨 (행 개편)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=101-10992) | X2-04·05 |
| [M1 카드 추가 · 그림 가리기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=107-11027) | M2-03, X4-05 |
| [M2 가리기 편집기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=107-11368) | X4-05 |
| [M3-q 학습 · 그림 가리기 질문](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=108-11142) | X4-05 |
| [M3-a 학습 · 그림 가리기 답](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=108-11222) | M3-05 |
| [M4 카드 목록 · 그림 가리기 미리보기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=108-11319) | X4-05 |
| [M5 그림 가리기 · 빈 상태와 오류](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=108-11457) | X4-06 |
| [Dark · M1 카드 추가 · 그림 가리기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=109-11282) | M2-03, X4-05 |
| [Dark · M2 가리기 편집기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=109-11464) | X4-05 |
| [Dark · M3-q 학습 · 그림 가리기 질문](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=109-11591) | X4-05 |
| [Dark · M3-a 학습 · 그림 가리기 답](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=109-11648) | M3-05 |
| [Dark · M4 카드 목록 · 그림 가리기 미리보기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=109-11714) | X4-05 |
| [Dark · M5 그림 가리기 · 빈 상태와 오류](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=109-11832) | X4-06 |
| [N1 카드 편집 · 기출 표시](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=112-11537) | M2-05 |
| [N2 카드 목록 · 여러 장 기출 표시](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=112-11692) | M2-05 |
| [N3 중요도 제안 · 기출 근거](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=112-11946) | M2-01·02·04 |
| [N4 덱 상세 · 기출 요약](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=112-12313) | M5-01·02 |
| [P0 시험 전 회독 · 입구 3곳](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12050) | X5-02 |
| [P1 시험 전 회독 · 시작 전 요약 (혼합)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12126) | X5-01·02·03 |
| [P2 시험 전 회독 · 학습 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12285) | X5-01·03 |
| [P3 시험 전 회독 · 끝난 뒤](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=114-12347) | X5-02·03 |
| [Dark · N1 카드 편집 · 기출 표시](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12199) | M2-05 |
| [Dark · N2 카드 목록 · 여러 장 기출 표시](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12305) | M2-05 |
| [Dark · N3 중요도 제안 · 기출 근거](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12526) | M2-01·02·04 |
| [Dark · N4 덱 상세 · 기출 요약](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12640) | M5-01·02 |
| [Dark · P0 시험 전 회독 · 입구 3곳](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12720) | X5-02 |
| [Dark · P1 시험 전 회독 · 시작 전 요약 (혼합)](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12750) | X5-01·02·03 |
| [Dark · P2 시험 전 회독 · 학습 중](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12836) | X5-01·03 |
| [Dark · P3 시험 전 회독 · 끝난 뒤](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=115-12867) | X5-02·03 |
| [Q1-b 학습 · 목록 회상 · 다 본 뒤 평가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=124-12869) | M3-05, X4-04 |
| [Q1-a 학습 · 목록 회상 · 하나씩 공개](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=124-13041) | X4-04 |
| [Q2 학습 · 선택형 · 고른 뒤](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=125-13035) | M3-05 |
| [Q0 카드 추가 · 목록 회상 · 선택형](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=126-13108) | M2-03, X4-03·04 |
| [Q3 Anki 호환 · 노트 유형과 필드](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=128-13246) | X1-07 |
| [Dark · Q0 카드 추가 · 목록 회상 · 선택형](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=129-13287) | M2-03, X4-03·04 |
| [Dark · Q1-a 학습 · 목록 회상 · 하나씩 공개](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=129-13417) | X4-04 |
| [Dark · Q1-b 학습 · 목록 회상 · 다 본 뒤 평가](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=129-13455) | M3-05, X4-04 |
| [Dark · Q2 학습 · 선택형 · 고른 뒤](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=129-13500) | M3-05 |
| [Dark · Q3 Anki 호환 · 노트 유형과 필드](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=129-13542) | X1-07 |
| [AU1 학습 · 카드 안의 소리](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=134-13664) | X4-07 |
| [AU2 소리 · 설정 · 단축키 · 가져오기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=135-13704) | X4-07 |
| [Dark · AU1 학습 · 카드 안의 소리](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=136-13729) | X4-07 |
| [Dark · AU2 소리 · 설정 · 단축키 · 가져오기](https://www.figma.com/design/xZsmRhJ4rvkjwYHfQyM5jx?node-id=136-13772) | X4-07 |

## 10. 근거 대조·한계·확인 과제

### 구현에서 대조한 사항

- [commands.rs](../../src-tauri/src/commands.rs)의 덱 지표는 핵심 카드 중 신규 상태가 아닌 카드를 core_seen으로 센다. on_track은 남은 신규와 일일 신규 분량을 대조한다. 따라서 숙달률이나 시험 합격 가능성으로 명명하지 않는다.
- [Done.tsx](../../src/screens/Done.tsx)와 [queue.rs](../../src-tauri/src/queue.rs)는 ‘다음 학습으로’ 경로에서 3일 뒤 복귀를 사용한다. 완료 화면의 접어두기 원인 분기는 구현에 이미 있다.
- [quality.rs](../../src-tauri/src/quality.rs)의 그대로 두기는 30일 숨기기이고, 원래 카드 보관은 suspended로 학습에서 제외한다. 나누기를 되돌리면 새 카드를 지우고 원래 상태로 되돌린다.
- [plan.rs](../../src-tauri/src/plan.rs)는 과거 기록으로 학습 시간을 추정하고 미래 복습은 근사한다. 학습 시간 대표값은 최근 14일 기록을 제한한 뒤 얻은 중앙값이다. 차트의 미래 수치를 확정 일정으로 표현하지 않는다.
- [PreExam.tsx](../../src/screens/PreExam.tsx)와 [Study.tsx](../../src/screens/Study.tsx)는 압축은 일정 미반영, 혼합은 일정 반영으로 구분한다. 전체 회독의 분모는 배운 카드다.
- [GradeAssist.tsx](../../src/screens/GradeAssist.tsx)의 전송 목록과 [rag.rs](../../src-tauri/src/rag.rs)의 grade_input에는 질문·내 답·모범 답안·자료 구간이 있다. Figma의 짧은 범위 요약과 다르다.
- [Recall.tsx](../../src/screens/Recall.tsx)는 공개한 목록 항목을 기본 기억 상태로 세고, 사용자가 빠뜨림 표시를 수정한다. 기억 여부 자동 측정이라고 설명해서는 안 된다.
- [sync.tsx](../../src/sync.tsx)의 병합·복구·연결 해제 선택을 대조했다. 문구 검토는 실계정에서 데이터 병합·삭제·복구를 실행한 검증을 대신하지 않는다.

### 적용 전에 확인할 사항

| 항목 | 남은 확인 | 완료 기준 |
|---|---|---|
| M1-02 | 설정 저장 시 진행 중 세션에 미치는 영향 | ‘오늘·다음 학습’ 문구를 실제 큐 갱신 시점과 맞춘다. |
| M1-03·M5-04 | 예상 날짜·초과 일수·관측 기간 | 같은 분모와 당일 포함 규칙에서 숫자와 문장이 일치한다. |
| M2-03 | 상속 해제·복귀 UI | 직접 지정과 덱 설정 따르기를 화면에서 구분해 선택할 수 있다. |
| M5-06 | 히트맵의 삭제된 카드 기록 집계 예외 | 카드 삭제 이후에도 ‘하루 한 번’ 설명과 통계가 일치한다. |
| X1-02 | 가져온 type-in 카드의 직접 쓰기 모드 | 질문·답 표시와 직접 쓰기 지원 안내가 실제 학습 동작과 일치한다. |
| X2-01~04 | 동기화 병합·복구·삭제의 실계정 동작 | 현재 내용·30일 대체 버전·백업·다른 기기 영향의 설명이 실제 결과와 일치한다. |
| X3-02·03 | 연결 해제 중 진행 중인 요청 처리·전송 안내 반영 | 질문을 포함한 요약·상세 목록·실제 payload가 일치하고, 중단을 회수로 표현하지 않는다. |
| X3-07 | 외부 계정·요금제 정책 | 대상 계정으로 로그인·권한·한도 관리 동선을 확인하고 보장 문구를 확정한다. |
| X4-05 | 작성·기존 편집의 버튼 분기 반영 | 신규는 적용 후 별도 저장, 기존은 변경 저장이라는 동작과 공유 영향 범위가 문구에 드러난다. |

이번 검토는 Figma에 그려진 상태 전체를 다뤘으며, 프로토타입의 모든 클릭 전이·실계정 OAuth·모든 오류의 재현을 수행한 것은 아니다. 원문·이미지 확인과 코드 정적 대조를 완료했지만 사용자 이해도 향상 효과, 화면에 없는 상태, 외부 서비스 정책은 검증되지 않았다. 이를 ‘문제 없음’으로 처리하지 않는다.

### 문구 적용 후 확인할 이해도 시나리오

Anki 초보자와 기존 사용자가 각각 다음 질문에 화면만 보고 답할 수 있는지 확인한다. 정답을 먼저 설명하지 않고, 선택 이유와 예상 결과를 듣는다.

1. 중요도를 확정하지 않고 나가면 어떤 순서로 학습하는가? 보충 카드는 제외되는가?
2. ‘3일 뒤 다시 보기’를 누르면 오늘 다시 앱을 켰을 때 나오는가? 복귀일은 언제인가?
3. ‘신규 첫 학습 완료 예상’이 높으면 시험 준비나 암기가 끝났다는 뜻인가?
4. 카드 점검에서 ‘30일간 숨기기’를 누르면 카드 학습도 중지되는가?
5. 압축과 혼합 중 어느 쪽의 평가가 복습 일정을 바꾸는가? 신규 카드도 회독에 포함되는가?
6. 노트 하나에서 만든 카드 중 하나에 기출 표시를 추가하면 무엇이 함께 바뀌는가?
7. 자료 기반 채점 보조에서 질문·답·자료 중 무엇이 OpenAI로 전송되는가?
8. Drive 데이터를 합칠 때 다른 내용은 어디에 얼마나 남는가? 백업 복구는 다른 기기에 어떤 영향을 주는가?
9. 목록 항목을 공개하기만 했을 때 ‘기억’ 수는 어떻게 계산되는가?

수정 후에는 각 문구를 실제 버튼 너비와 다크 모드에서 다시 확인하고, 자동 줄바꿈·잘림·중요한 기간의 누락을 점검한다. UI 구현·테스트 코드는 이번 문서 작업에서 변경하지 않았다.
