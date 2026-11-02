// CSV 가져오기의 덱 경로 검사 (src/csv.ts).
//   node tools/check_csv.mjs
import assert from "node:assert/strict";
import { csvDeckRoot, parseCsv } from "../src/csv.ts";

// T4-15: 내보낸 파일(덱 열이 최상위 덱부터의 전체 경로)은 같은 구조로 돌아온다. 파일 이름은 뿌리가 되지 않는다
const exported = parseCsv("앞면,뒷면,하위 덱\na,b,QA 네트워크\nc,d,QA 네트워크::1장 기초::TCP\ne,f,QA 네트워크::2장\n", ",");
assert.deepEqual(csvDeckRoot(exported.slice(1).map((r) => r[2]), "QA 네트워크_2026-11-01.csv"), { root: "QA 네트워크", full: true });
assert.deepEqual(csvDeckRoot(["formula"], "formula-rt.csv"), { root: "formula", full: true }, "하위 덱 없는 덱");
// 직접 만든 표: 칸이 여러 이름이면 파일 이름 아래, 비었으면 파일 이름이 덱
assert.deepEqual(csvDeckRoot(["Day 1", "", "Day 2"], "토익 영단어.csv"), { root: "토익 영단어", full: false });
assert.deepEqual(csvDeckRoot(["", ""], "토익 영단어.csv"), { root: "토익 영단어", full: false });
assert.deepEqual(csvDeckRoot(["A ::x", "A::y"], "f.csv"), { root: "A", full: true }, "앞뒤 공백은 덱 이름처럼 무시");
console.log("check_csv: ok");
