// 날짜 · 시각 · 남은 시간 · 크기 글자. 여러 화면이 같이 쓴다 (node tools/check_dates.mjs).
// 날짜 문자열은 로컬 날짜 "2026-10-07" (toISOString 은 UTC 라 하루 어긋난다)
export const WEEKDAY = "일월화수목금토";
const DAY = 86_400_000;
const pad = (n: number) => String(n).padStart(2, "0");

/** "2026-10-07" -> 그날 로컬 0시 */
export const parse = (s: string) => {
  const [y, m, d] = s.split("-").map(Number);
  return new Date(y, m - 1, d);
};
export const iso = (t: Date) => `${t.getFullYear()}-${pad(t.getMonth() + 1)}-${pad(t.getDate())}`;
export const addDays = (s: string, days: number) => {
  const t = parse(s);
  t.setDate(t.getDate() + days);
  return iso(t);
};
/** "10/7" */
export const md = (s: string) => `${+s.slice(5, 7)}/${+s.slice(8, 10)}`;
/** "10월 7일" */
export const monthDay = (s: string) => `${+s.slice(5, 7)}월 ${+s.slice(8, 10)}일`;
/** 시각(ms)의 "10월 7일" */
export const monthDayAt = (ms: number) => monthDay(iso(new Date(ms)));
/** "10월 18일 (토)" */
export const dateWithDay = (s: string) => `${monthDay(s)} (${WEEKDAY[parse(s).getDay()]})`;
/** "2026. 10. 28. (수)" */
export const longDate = (s: string) => {
  const d = parse(s);
  return `${d.getFullYear()}. ${d.getMonth() + 1}. ${d.getDate()}. (${WEEKDAY[d.getDay()]})`;
};
/** "10. 16. (금)" */
export const dotDate = (s: string) => {
  const d = parse(s);
  return `${d.getMonth() + 1}. ${d.getDate()}. (${WEEKDAY[d.getDay()]})`;
};
/** "09:05", secs 면 "09:05:03" */
export const hm = (d: Date, secs = false) => `${pad(d.getHours())}:${pad(d.getMinutes())}${secs ? `:${pad(d.getSeconds())}` : ""}`;
/** "10월 12일 22:40" */
export const stamp = (ms: number, secs = false) => `${monthDayAt(ms)} ${hm(new Date(ms), secs)}`;
const dayStart = (ms: number) => new Date(new Date(ms).toDateString()).getTime();
/** 오늘 09:12 · 어제 22:10 · 10월 12일 22:40. secs: 같은 분에 고친 두 버전을 가리려고 초까지 (K7) */
export function when(ms: number, secs = false) {
  const days = Math.round((dayStart(Date.now()) - dayStart(ms)) / DAY);
  return days === 0 ? `오늘 ${hm(new Date(ms), secs)}` : days === 1 ? `어제 ${hm(new Date(ms), secs)}` : stamp(ms, secs);
}
/** 진행 카드의 "약 12초 남음". 지금까지 걸린 시간에 비례한 어림값. 처음 5% 는 너무 흔들려서 null (B2-p · C1-xp) */
export function eta(ratio: number, started: number) {
  if (!(ratio >= 0.05 && ratio < 1)) return null;
  const elapsed = (Date.now() - started) / 1000;
  const left = Math.max(1, Math.ceil((elapsed * (1 - ratio)) / ratio));
  return `약 ${left < 60 ? `${left}초` : `${Math.ceil(left / 60)}분`} 남음`;
}
/** 파일 크기. 0.1MB 미만은 KB (0.0MB 로 보이지 않게) */
export const mb = (bytes: number) => (bytes < 0.1 * 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))}KB` : `${(bytes / 1024 / 1024).toFixed(1)}MB`);
