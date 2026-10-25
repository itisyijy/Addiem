// theme.ts 의 initTheme 와 같은 규칙. 앱 코드가 뜨기 전에 첫 화면 색을 맞춘다
try {
  // 예전 이름(Neo-Anki) 시절 저장 키(neoanki.*)를 새 이름(addiem.*)으로 옮긴다. 앱 코드보다 먼저라 한 곳에서 한다
  var old = [];
  for (var i = 0; i < localStorage.length; i++) {
    var k = localStorage.key(i);
    if (k && k.indexOf("neoanki.") === 0) old.push(k);
  }
  old.forEach(function (k) {
    var to = "addiem." + k.slice(8);
    if (localStorage.getItem(to) === null) localStorage.setItem(to, localStorage.getItem(k));
    localStorage.removeItem(k);
  });
  var p = localStorage.getItem("addiem.theme");
  var dark = p ? p === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme = dark ? "dark" : "light";
} catch (e) {}
