// 날짜 글자 검사 (src/dates.ts). 기대값은 화면마다 따로 있던 옛 함수들이 낸 글자 그대로다 (KST · UTC 같음).
//   node tools/check_dates.mjs
import assert from "node:assert/strict";
import { addDays, dateWithDay, dayRuns, dotDate, eta, hm, iso, longDate, md, monthDay, monthDayAt, parse, stamp, when, WEEKDAY } from "../src/dates.ts";

const ISOS = ["2026-10-04", "2026-10-05", "2026-10-06", "2026-10-07", "2026-10-08", "2026-10-09", "2026-10-10", "2026-01-01", "2025-12-31", "2026-02-28", "2024-02-29", "2026-11-30"];
const DAYS = ["일", "월", "화", "수", "목", "금", "토", "목", "수", "토", "목", "월"];
const MD = ["10/4", "10/5", "10/6", "10/7", "10/8", "10/9", "10/10", "1/1", "12/31", "2/28", "2/29", "11/30"];
const MONTH_DAY = MD.map((s) => s.replace(/(\d+)\/(\d+)/, "$1월 $2일"));
assert.deepEqual(ISOS.map(md), MD);
assert.deepEqual(ISOS.map(monthDay), MONTH_DAY);
assert.deepEqual(ISOS.map(dateWithDay), MONTH_DAY.map((s, i) => `${s} (${DAYS[i]})`));
assert.deepEqual(ISOS.map(longDate), ["2026. 10. 4. (일)", "2026. 10. 5. (월)", "2026. 10. 6. (화)", "2026. 10. 7. (수)", "2026. 10. 8. (목)", "2026. 10. 9. (금)", "2026. 10. 10. (토)", "2026. 1. 1. (목)", "2025. 12. 31. (수)", "2026. 2. 28. (토)", "2024. 2. 29. (목)", "2026. 11. 30. (월)"]);
assert.deepEqual(ISOS.map(dotDate), ["10. 4. (일)", "10. 5. (월)", "10. 6. (화)", "10. 7. (수)", "10. 8. (목)", "10. 9. (금)", "10. 10. (토)", "1. 1. (목)", "12. 31. (수)", "2. 28. (토)", "2. 29. (목)", "11. 30. (월)"]);
assert.deepEqual([...WEEKDAY], ["일", "월", "화", "수", "목", "금", "토"]);
// H2 넘는 날: 이어진 날끼리 (QA T3-22, 7 · 8일은 넘지 않는다)
assert.deepEqual(
  [["2026-11-09"], ["2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05", "2026-11-06", "2026-11-09"], ["2026-10-30", "2026-10-31", "2026-11-01", "2026-11-05"]].map(dayRuns),
  ["9일", "2–6 · 9일", "10월 30일–11월 1일 · 11월 5일"],
);
const adds = [["2025-12-31", 1], ["2026-01-01", -1], ["2026-02-28", 1], ["2024-02-28", 1], ["2026-10-07", 6], ["2026-10-31", 30], ["2026-03-01", -1]];
assert.deepEqual(adds.map(([s, d]) => addDays(s, d)), ["2026-01-01", "2025-12-31", "2026-03-01", "2024-02-29", "2026-10-13", "2026-11-30", "2026-02-28"]);
const dates = [new Date(2026, 0, 5, 0, 0), new Date(2026, 11, 31, 23, 59, 59), new Date(2026, 9, 7, 3, 59), new Date(2026, 9, 7, 4, 0)];
assert.deepEqual(dates.map(iso), ["2026-01-05", "2026-12-31", "2026-10-07", "2026-10-07"]);

// 2024 · 2026 의 모든 날: 문자열 <-> 로컬 Date 가 한 바퀴 돌고, 옛 Intl 판(설정 · 덱 업데이트)과 같은 "10월 7일"
const intlDay = new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric" });
for (const y of [2024, 2026])
  for (let t = new Date(y, 0, 1); t.getFullYear() === y; t.setDate(t.getDate() + 1)) {
    const s = iso(t);
    assert.equal(parse(s).getTime(), t.getTime(), s);
    assert.equal(monthDayAt(t.getTime() + 13 * 3600e3), intlDay.format(t), s);
  }

// 시각: 0시 · 새벽 4시(학습일 경계) 앞뒤 · 해 바뀜
const T = (y, mo, d, h, mi, s = 0) => new Date(y, mo - 1, d, h, mi, s).getTime();
const times = [T(2026, 10, 7, 0, 0, 0), T(2026, 10, 7, 0, 5, 9), T(2026, 10, 7, 3, 59, 59), T(2026, 10, 7, 4, 0, 0), T(2026, 10, 7, 9, 5, 3), T(2026, 10, 7, 12, 0, 0), T(2026, 10, 7, 23, 59, 59), T(2026, 1, 1, 0, 30, 1), T(2025, 12, 31, 23, 50, 7), T(2026, 10, 6, 23, 59, 59), T(2026, 10, 5, 8, 1, 2)];
const HMS = ["00:00:00", "00:05:09", "03:59:59", "04:00:00", "09:05:03", "12:00:00", "23:59:59", "00:30:01", "23:50:07", "23:59:59", "08:01:02"];
const DAY_OF = ["10월 7일", "10월 7일", "10월 7일", "10월 7일", "10월 7일", "10월 7일", "10월 7일", "1월 1일", "12월 31일", "10월 6일", "10월 5일"];
assert.deepEqual(times.map((t) => hm(new Date(t))), HMS.map((s) => s.slice(0, 5)));
assert.deepEqual(times.map((t) => hm(new Date(t), true)), HMS);
assert.deepEqual(times.map(monthDayAt), DAY_OF);
assert.deepEqual(times.map((t) => stamp(t)), DAY_OF.map((d, i) => `${d} ${HMS[i].slice(0, 5)}`));
assert.deepEqual(times.map((t) => stamp(t, true)), DAY_OF.map((d, i) => `${d} ${HMS[i]}`));
// 옛 Intl 판(설정 · 복구 화면의 when)과 하루의 모든 분이 같다 (0시가 "24:00" 이 아니다)
const intlStamp = (ms, secs) => new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", second: secs ? "2-digit" : undefined, hour12: false }).format(ms);
for (let m = 0; m < 24 * 60; m++) {
  const t = T(2026, 10, 7, 0, m, 7);
  assert.equal(stamp(t), intlStamp(t, false));
  assert.equal(stamp(t, true), intlStamp(t, true));
}

// when / eta 는 지금 시각을 고정해 본다
const realNow = Date.now;
// times 마다 "오늘" · "어제" (나머지는 날짜). 미래 시각도 날짜로 쓴다
const today7 = ["오늘", "오늘", "오늘", "오늘", "오늘", "오늘", "오늘", null, null, "어제", null];
const rels = [[T(2026, 10, 7, 10, 0), today7], [T(2026, 10, 7, 0, 0), today7], [T(2026, 1, 1, 0, 30), [null, null, null, null, null, null, null, "오늘", "어제", null, null]]];
for (const [now, labels] of rels) {
  Date.now = () => now;
  const expect = labels.map((day, i) => [`${day ?? DAY_OF[i]} ${HMS[i].slice(0, 5)}`, `${day ?? DAY_OF[i]} ${HMS[i]}`]);
  assert.deepEqual(times.map((t) => [when(t), when(t, true)]), expect, String(now));
}
Date.now = () => 1_000_000;
const etas = [[0, 5], [0.04, 5], [0.05, 1], [0.05, 3], [0.1, 100], [0.5, 59], [0.5, 60], [0.5, 61], [0.25, 1000], [0.99, 10], [1, 10]];
assert.deepEqual(etas.map(([r, s]) => eta(r, 1_000_000 - s * 1000)), [null, null, "약 19초 남음", "약 57초 남음", "약 15분 남음", "약 59초 남음", "약 1분 남음", "약 2분 남음", "약 50분 남음", "약 1초 남음", null]);
Date.now = realNow;

console.log("check_dates: ok");
