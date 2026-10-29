// 공통 토스트 (Figma S1 26:3850 의 3번). 화면 아래 가운데, D4 접어두기 토스트와 같은 모양.
// 정보는 3초, 되돌리기는 5초 뒤 사라지고, 오류는 닫거나 다시 시도할 때까지 남는다.
// 새 토스트가 앞의 되돌리기 · 오류를 덮지 않게 위로 쌓는다 (QA T1-12 · T8-14). 정보는 마지막 1개, 되돌리기 · 오류는 최근 2개까지
import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { Icon, type IconName } from "./ui";
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

  return (
    <ToastContext.Provider value={show}>
      {children}
      {items.length > 0 && (
        <div className="toasts">
          {items.map((t) => (
            <Toast key={t.id} toast={t} onClose={close} />
          ))}
        </div>
      )}
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
