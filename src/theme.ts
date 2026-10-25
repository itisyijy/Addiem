// 화면 테마 (Figma F 다크 모드). 설정 > 학습의 [시스템 따름 / 밝게 / 어둡게], 기본은 시스템 따름.
// 색은 tokens.css 의 변수만 바뀌므로 <html data-theme> 하나만 바꾸면 된다 (학습 세션도 그대로 유지)
export type ThemePref = "system" | "light" | "dark";

const KEY = "addiem.theme";
const media = window.matchMedia("(prefers-color-scheme: dark)");

export function getThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark") return v;
  } catch {
    // 저장소를 못 쓰면 시스템을 따른다
  }
  return "system";
}

function apply() {
  const p = getThemePref();
  document.documentElement.dataset.theme = p === "dark" || (p === "system" && media.matches) ? "dark" : "light";
}

export function setThemePref(p: ThemePref) {
  try {
    if (p === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, p);
  } catch {
    // 저장이 안 돼도 이번 실행에는 적용한다
  }
  apply();
}

/** main.tsx 가 첫 렌더 전에 부른다. OS 테마가 바뀌면 "system" 일 때만 따라간다 (apply 가 설정을 다시 읽는다) */
export function initTheme() {
  apply();
  media.addEventListener("change", apply);
}
