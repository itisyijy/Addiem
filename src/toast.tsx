// 공통 토스트 (Figma S1 26:3850 의 3번). 화면 아래 가운데, D4 접어두기 토스트와 같은 모양.
// 정보는 3초, 되돌리기는 5초 뒤 사라지고, 오류는 닫거나 다시 시도할 때까지 남는다.
// 새 토스트가 앞의 되돌리기 · 오류를 덮지 않게 위로 쌓는다 (QA T1-12 · T8-14). 정보는 마지막 1개, 되돌리기 · 오류는 최근 2개까지
// 겹침 창이 떠 있으면 맨 위 창 안, 버튼 줄 바로 위에 그린다 (I191). 창을 덮지 않고 ✕ · [되돌리기]가 창의 Tab 안에 든다.
// 창이 화면보다 길면 토스트와 버튼 줄이 보이게 스크롤한다. 토스트가 사라지면 창이 짧아져 스크롤이 줄어 창이 들어가면 제목이 다시 보인다 (QA G5-02)
import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Icon, type IconName } from "./ui";
import { store } from "./store";
import "./styles/toast.css";

export type ToastTone = "info" | "undo" | "error";
export type ToastInput = {
  title: string;
  text?: string;
  tone?: ToastTone;
  icon?: IconName;
  /** 오른쪽 버튼 하나 (되돌리기, 다시 시도) */
  action?: { label: string; onClick: () => void };
  /** 머무는 시간(ms). 없으면 종류의 기본값 (K7 정보 토스트는 [보기]가 있어 5초) */
  ms?: number;
};

const LIFETIME: Record<ToastTone, number | null> = { info: 3000, undo: 5000, error: null };
const ICON: Record<ToastTone, IconName> = { info: "check", undo: "rotate", error: "alert" };

/** 열린 겹침 창 (useFocusTrap 이 넣고 뺀다). 마지막이 맨 위 */
export const dialogs = store<HTMLElement[]>([]);
/** 창의 버튼 줄. 토스트는 마지막 줄 바로 위에, 없으면 창 끝에 */
const ROWS = ".modal__actions, .edit__actions, .sync-foot, .export__foot, .fig-foot, .split__foot, .draft-send__foot, .ai-l3__foot";

const ToastContext = createContext<(t: ToastInput) => void>(() => {});
/** const toast = useToast(); toast({ title: "보관했어요", tone: "undo", action: { label: "되돌리기", onClick } }) */
export const useToast = () => useContext(ToastContext);

type Item = ToastInput & { id: number };
let seq = 0;
const sticky = (t: Item) => (t.tone ?? "info") !== "info";

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Item[]>([]);
  const show = useCallback(
    (t: ToastInput) =>
      setItems((prev) => {
        const next = [...prev, { ...t, id: ++seq }];
        const keep = new Set([...next.filter((x) => !sticky(x)).slice(-1), ...next.filter(sticky).slice(-2)]);
        return next.filter((x) => keep.has(x));
      }),
    [],
  );
  const close = useCallback((id: number) => setItems((prev) => prev.filter((x) => x.id !== id)), []);

  // 맨 위 창 안에 자리(slot)를 끼운다. 창 내용이 바뀌어 버튼 줄이 새로 그려져도 그 위로 다시 옮긴다
  const top = dialogs.use().slice(-1)[0];
  const shown = items.length > 0;
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (!top || !shown) return;
    const el = document.createElement("div");
    el.style.display = "contents";
    const place = () => {
      const rows = top.querySelectorAll(ROWS);
      let row = rows[rows.length - 1];
      const outer = row?.parentElement?.closest(ROWS); // .fig-foot 안의 .modal__actions 는 바깥 줄 위로
      if (outer && top.contains(outer)) row = outer;
      if (row ? el.nextElementSibling !== row : el.parentElement !== top) row ? row.before(el) : top.append(el);
    };
    place();
    const watch = new MutationObserver(place);
    watch.observe(top, { childList: true, subtree: true });
    setSlot(el);
    return () => {
      watch.disconnect();
      el.remove();
      setSlot(null);
    };
  }, [top, shown]);
  // 창 안에 토스트가 뜨면 토스트와 바로 아래 버튼 줄이 보이게 창을 스크롤한다 (작은 창에서 화면 아래로 밀리지 않게, QA G5-02)
  useEffect(() => {
    if (slot) [slot.firstElementChild, slot.nextElementSibling].forEach((e) => e?.scrollIntoView({ block: "nearest" }));
  }, [slot, items]);

  const list = shown && (
    <div className="toasts">
      {items.map((t) => (
        <Toast key={t.id} toast={t} onClose={close} />
      ))}
    </div>
  );
  return (
    <ToastContext.Provider value={show}>
      {children}
      {slot && list ? createPortal(list, slot) : list}
    </ToastContext.Provider>
  );
}

function Toast({ toast, onClose }: { toast: Item; onClose: (id: number) => void }) {
  const tone = toast.tone ?? "info";
  const close = () => onClose(toast.id);
  useEffect(() => {
    const life = toast.ms ?? LIFETIME[tone];
    if (!life) return;
    const t = setTimeout(() => onClose(toast.id), life);
    return () => clearTimeout(t);
  }, [toast, tone, onClose]);

  return (
    <div className={`toast toast--${tone}`} role={tone === "error" ? "alert" : "status"}>
      <span className="toast__icon">
        <Icon name={toast.icon ?? ICON[tone]} size={16} />
      </span>
      <span className="toast__text">
        <span className="t-body-m-strong">{toast.title}</span>
        {toast.text && <span className="t-caption c-on-inverse">{toast.text}</span>}
      </span>
      {toast.action && (
        <button
          className="toast__btn t-label-m"
          onClick={() => {
            toast.action!.onClick();
            close();
          }}
        >
          {toast.action.label}
        </button>
      )}
      {tone === "error" && (
        <button className="toast__close" aria-label="닫기" onClick={close}>
          <Icon name="x" size={16} />
        </button>
      )}
    </div>
  );
}
