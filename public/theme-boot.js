// theme.ts 의 initTheme 와 같은 규칙. 앱 코드가 뜨기 전에 첫 화면 색을 맞춘다
try {
  var p = localStorage.getItem("neoanki.theme");
  var dark = p ? p === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme = dark ? "dark" : "light";
} catch (e) {}
