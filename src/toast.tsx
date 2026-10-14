// 공통 토스트 (Figma S1 26:3850 의 3번). 화면 아래 가운데 한 자리, D4 접어두기 토스트와 같은 모양.
// 정보는 3초, 되돌리기는 5초 뒤 사라지고, 오류는 닫거나 다시 시도할 때까지 남는다
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
};

const LIFETIME: Record<ToastTone, number | null> = { info: 3000, undo: 5000, error: null };
const ICON: Record<ToastTone, IconName> = { info: "check", undo: "rotate", error: "alert" };

const ToastContext = createContext<(t: ToastInput) => void>(() => {});
/** const toast = useToast(); toast({ title: "보관했어요", tone: "undo", action: { label: "되돌리기", onClick } }) */
export const useToast = () => useContext(ToastContext);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toast, setToast] = useState<(ToastInput & { id: number }) | null>(null);
  const show = useCallback((t: ToastInput) => setToast({ ...t, id: Date.now() }), []);
  const close = useCallback(() => setToast(null), []);

  useEffect(() => {
    const life = toast && LIFETIME[toast.tone ?? "info"];
    if (!life) return;
    const t = setTimeout(close, life);
    return () => clearTimeout(t);
  }, [toast, close]);

  const tone = toast?.tone ?? "info";
  return (
    <ToastContext.Provider value={show}>
      {children}
      {toast && (
        <div key={toast.id} className={`toast toast--${tone}`} role={tone === "error" ? "alert" : "status"}>
          <span className="toast__icon">
            <Icon name={toast.icon ?? ICON[tone]} size={16} />
          </span>
          <span className="toast__text">
            <span className="t-body-m-strong">{toast.title}</span>
            {toast.text && <span className="t-caption c-tertiary">{toast.text}</span>}
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
      )}
    </ToastContext.Provider>
  );
}
