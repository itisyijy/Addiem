// 화면 전환과 단축키. 데스크톱 앱이라 URL 라우팅을 쓰지 않는다
import { createContext, useContext, useEffect } from "react";
import type { ImportResult, SessionSummary } from "./api";
import { matchAction, type ActionId } from "./keys";

export type Route =
  | { name: "home" }
  | { name: "newDeck" } // B1. 홈 위 모달
  | { name: "import"; pick?: boolean } // B2. pick: 단축키 I 로 들어오면 파일 선택 창을 바로 연다
  | { name: "importJson"; deckId?: number } // B2-j
  | { name: "importResult"; result: ImportResult } // B3
  | { name: "importance"; deckId: number; next?: number[] } // B4. next: 가져오기 뒤 이어서 볼 최상위 덱
  | { name: "deck"; deckId: number } // C1
  | { name: "cards"; deckId: number | null; buriedOnly?: boolean; cardIds?: number[]; weakOnly?: boolean } // C2. deckId 가 null 이면 모든 덱. cardIds: 이 카드들만. weakOnly: 약한 챕터만 (C1-w)
  | { name: "addCard"; deckId: number } // C3
  | { name: "deckSettings"; deckId: number } // C4
  | { name: "quality"; deckId: number } // G1 카드 점검 (G2 나누기 제안은 그 위 모달)
  | { name: "summary"; deckIds: number[] | null } // D1
  | { name: "study"; deckIds: number[] | null; scale: number; cardIds?: number[]; sessionId?: number } // D2~D4
  | { name: "done"; sessionId: number; summary: SessionSummary; completed: boolean; deckIds: number[] | null; scale: number } // D5
  | { name: "settings"; focus?: "archived" }; // E1. focus: 열자마자 그 칸으로 (홈 "보관한 덱 N")

export const NavContext = createContext<(r: Route) => void>(() => {});
export const useNav = () => useContext(NavContext);

// 떠 있는 겹침 창 수 (모달 · 메뉴 · 팝오버). 떠 있으면 화면 단축키가 모두 꺼지고 그 창이 키를 받는다 (E4 노트 45:5835)
let openOverlays = 0;

/**
 * 모달 · 메뉴 · 팝오버가 부른다. active 인 동안 화면 단축키가 꺼진다 (창 안의 키는 useKey 의 { modal: true }).
 * 훅 규칙 때문에 열고 닫는 메뉴는 늘 부르고 active 로 켠다
 */
export function useOverlayScope(active = true) {
  useEffect(() => {
    if (!active) return;
    openOverlays++;
    return () => {
      openOverlays--;
    };
  }, [active]);
}
/** 예전 이름 (Modal) */
export const useModalScope = () => useOverlayScope();

/** 바꿀 수 있는 동작 (keys.ts). KeyboardEvent.code 로 맞춘다. 평가처럼 키가 여럿이면 handler 의 index 로 몇 번째인지 */
export type KeySpec = string | string[] | { action: ActionId };

/**
 * 입력 중이 아닐 때만 동작하는 단축키.
 * - 글자 key (예: "Escape", "ArrowDown", "F2"): KeyboardEvent.key 로 맞춘다. 고정 키와 화면 안 메뉴 키용
 * - { action }: 설정 > 단축키(E4)에서 바꿀 수 있는 동작. 키 이름(code)으로 맞춰 한/영과 무관하다. Ctrl 조합도 된다
 * 겹침 창이 떠 있으면 modal: true 로 등록한 것만 동작한다. 입력 중 Esc 도 창만 받는다
 * (화면 Esc 는 다른 화면으로 넘어가 입력한 글을 날린다)
 */
export function useKey(
  key: KeySpec,
  handler: (e: KeyboardEvent, index: number) => void,
  enabled = true,
  { modal = false } = {},
) {
  const spec = typeof key === "string" ? key : Array.isArray(key) ? key.join("\u0000") : `@${key.action}`;
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      // 창 Esc 가 먼저 처리해 창이 닫힌 뒤라도 화면이 같은 키를 또 받지 않게
      if (e.defaultPrevented) return;
      if (openOverlays > 0 && !modal) return;
      const t = e.target instanceof HTMLElement ? e.target : null;
      const typing =
        t && ((t.tagName === "INPUT" && (t as HTMLInputElement).type !== "checkbox") || t.tagName === "TEXTAREA" || t.isContentEditable);
      if (typing && !(modal && e.key === "Escape")) return;
      // Enter·Space 는 키보드로 초점을 옮긴 버튼·메뉴 항목이 먼저 받는다 (덱 상세에서 버튼에 초점을 두고 Enter 를 누르면 학습이 시작되던 문제).
      // 글자 · 숫자 단축키는 버튼에 초점이 있어도 동작한다 (마우스로 [답 보기]를 누른 뒤에도 1–4 가 되게)
      const control = "button, a[href], select, summary, input[type=checkbox], [role=button], [role=menuitem], [role=option], [role=checkbox], [role=switch], [role=radio], [role=tab]";
      if ((e.key === "Enter" || e.key === " ") && t?.closest(control)) return;
      let index: number;
      if (spec.startsWith("@")) {
        index = matchAction(spec.slice(1) as ActionId, e);
      } else {
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        // 글자 키는 키 이름(code)으로도 맞춘다. 한글 입력 상태에서 B 가 "ㅠ" 로 들어와도 동작하게
        index = spec
          .split("\u0000")
          .findIndex((k) => k.toLowerCase() === e.key.toLowerCase() || (/^[a-z]$/i.test(k) && e.code === `Key${k.toUpperCase()}`));
      }
      if (index < 0) return;
      e.preventDefault();
      handler(e, index);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [spec, handler, enabled, modal]);
}

/** 가져오는 동안(B2-p) 화면 전환을 막는다. 잠근 화면이 끝날 때 푼다 */
let navLocked = false;
export const lockNav = (v: boolean) => {
  navLocked = v;
};
export const isNavLocked = () => navLocked;

/** 하루 시작 시각 (E1). 앱을 열 때 Rust 에서 받아 둔다 */
let cutoffHour = 4;
export const setCutoff = (h: number) => {
  cutoffHour = h;
};
export const getCutoff = () => cutoffHour;
export const hourLabel = (h: number) => (h === 0 ? "자정" : `새벽 ${h}시`);

/** 학습일. 하루 시작 시각 이전은 전날 (Rust db::study_day 와 같은 규칙) */
export function studyDate(): Date {
  return new Date(Date.now() - cutoffHour * 3600 * 1000);
}

export function dateLabel(d = studyDate()) {
  return new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric", weekday: "long" }).format(d);
}

export const n = (v: number) => v.toLocaleString("ko-KR");

/**
 * 마지막 글자를 읽은 소리의 받침 번호: 0 없음, 8 ㄹ, 그 밖은 받침 있음.
 * 영문·숫자는 읽는 소리로: L(엘) R(알) 1(일) 7(칠) 8(팔) → ㄹ, M(엠) N(엔) 0(영) 3(삼) 6(육) → 받침. 괄호·기호는 건너뛴다
 */
function jong(word: string) {
  const ch = [...word.replace(/[\s)\]}"'.·…-]+$/, "")].pop() ?? "";
  const c = ch.charCodeAt(0);
  if (c >= 0xac00 && c <= 0xd7a3) return (c - 0xac00) % 28;
  return /[lr178]/i.test(ch) ? 8 : /[mn036]/i.test(ch) ? 1 : 0;
}
/** 을/를 */
export const eul = (word: string) => (jong(word) ? "을" : "를");
/** 으로/로 (ㄹ 받침은 "로") */
export const ro = (word: string) => {
  const j = jong(word);
  return j && j !== 8 ? "으로" : "로";
};
