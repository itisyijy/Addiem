// 파일 끌어놓기 (가져오기 · 카드 추가 · 학습 자료가 같이 쓴다)
import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

/** 놓은 파일 경로 전부와 놓은 곳 (CSS 픽셀이라 document.elementFromPoint 에 바로 쓴다) */
export type FileDrop = { paths: string[]; x: number; y: number };

/**
 * Tauri 는 파일 드롭을 HTML drop 이 아니라 웹뷰 이벤트(경로 · 물리 픽셀 위치)로 준다 (dragDropEnabled 기본값, QA T1-05).
 * 화면이 보이는 동안(enabled)만 듣는다. accept 가 있으면 맞는 경로만 넘기고, 하나도 맞지 않으면 drag 가 "wrong" 이 된다.
 * accept 는 모듈 상수로 넘긴다 (바뀌면 다시 구독)
 */
export function useFileDrop(enabled: boolean, onDrop: (d: FileDrop) => void, accept?: RegExp) {
  const [drag, setDrag] = useState<"over" | "wrong" | null>(null);
  const handler = useRef(onDrop);
  handler.current = onDrop;
  useEffect(() => {
    if (!enabled) return;
    const un = getCurrentWebview().onDragDropEvent(({ payload: p }) => {
      if (p.type === "enter" || p.type === "over") return setDrag("over");
      if (p.type === "leave") return setDrag(null);
      const paths = accept ? p.paths.filter((x) => accept.test(x)) : p.paths;
      if (!paths.length) return setDrag("wrong");
      setDrag(null);
      const r = window.devicePixelRatio || 1;
      handler.current({ paths, x: p.position.x / r, y: p.position.y / r });
    });
    return () => void un.then((f) => f());
  }, [enabled, accept]);
  return [drag, setDrag] as const;
}
