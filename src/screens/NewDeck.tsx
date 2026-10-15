// B1 새 덱 만들기 / C1-s 하위 덱 만들기. 이름 하나만 받고 바로 통과시킨다
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorText, type DeckDetail, type Level } from "../api";
import { Button, Icon, Input, LEVEL_LABEL, type IconName } from "../ui";
import { useKey, useModalScope, useNav } from "../nav";

export function Modal({ title, sub, onClose, children, wide, variant, icon = "alert", className = "" }: {
  title: string;
  /** 제목 아래 한 줄 설명 (C1-s) */
  sub?: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
  /** confirm: 되돌릴 수 없는 일을 묻는 창 (E1-b). 경고 아이콘, 닫기 버튼 없음 */
  variant?: "confirm";
  /** confirm 창의 아이콘 (C2-d 는 휴지통) */
  icon?: IconName;
  /** 화면마다 다른 폭·위치 */
  className?: string;
}) {
  useModalScope();
  useKey("Escape", onClose, true, { modal: true });
  const box = useRef<HTMLDivElement>(null);
  useFocusTrap(box);
  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={box}
        tabIndex={-1}
        className={`modal ${wide ? "modal--wide" : ""} ${variant ? `modal--${variant}` : ""} ${className}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        {variant === "confirm" ? (
          <>
            <span className="modal__alert">
              <Icon name={icon} size={24} className="c-danger" />
            </span>
            <h2 className="t-title-m">{title}</h2>
          </>
        ) : (
          <header className="modal__head">
            {sub ? (
              <div className="modal__titles">
                <h2 className="t-title-m">{title}</h2>
                <p className="t-caption c-tertiary">{sub}</p>
              </div>
            ) : (
              <h2 className="t-title-m">{title}</h2>
            )}
            <button className="modal__close c-tertiary" aria-label="닫기" onClick={onClose}>
              <Icon name="x" size={20} />
            </button>
          </header>
        )}
        {children}
      </div>
    </div>
  );
}

const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';
/** 열린 모달. 맨 위 것만 Tab 을 가둔다 (C3-e 위의 새 하위 덱 창처럼 겹칠 때) */
const openBoxes: HTMLElement[] = [];

/** Tab 이 모달 밖으로 나가지 않게 하고, 닫히면 열기 전 초점(연 버튼)으로 돌려준다 */
export function useFocusTrap(ref: React.RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    const box = ref.current;
    if (!box) return;
    const opener = document.activeElement as HTMLElement | null;
    openBoxes.push(box);
    // autoFocus 한 입력이 있으면 그대로, 없으면 창 자체에 (스크린리더가 제목을 읽는다)
    if (!box.contains(document.activeElement)) box.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || openBoxes[openBoxes.length - 1] !== box) return;
      const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !el.hasAttribute("disabled") && el.offsetParent !== null);
      if (!items.length) return e.preventDefault();
      const [first, last] = [items[0], items[items.length - 1]];
      const at = document.activeElement;
      if (!box.contains(at) || (e.shiftKey ? at === first || at === box : at === last)) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      openBoxes.splice(openBoxes.indexOf(box), 1);
      if (opener?.isConnected) opener.focus();
    };
  }, [ref]);
}

const LEVELS: Level[] = [2, 1, 0];

/**
 * parentId 가 있으면 C1-s 하위 덱 만들기. 넣을 곳은 그 덱이 기본이고 더 깊은 하위 덱도 고를 수 있다.
 * onCreated 가 없으면 만든 덱으로 간다
 */
export function NewDeckModal({ parentId = null, onClose, onCreated }: {
  parentId?: number | null;
  onClose?: () => void;
  onCreated?: (deckId: number) => void;
}) {
  const nav = useNav();
  const [name, setName] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [parent, setParent] = useState<DeckDetail | null>(null);
  const [target, setTarget] = useState(parentId);
  const [level, setLevel] = useState<Level>(1); // 고르지 않으면 일반 (상속 없음)
  const close = useCallback(() => (onClose ? onClose() : nav({ name: "home" })), [nav, onClose]);
  const isSub = parentId !== null;

  useEffect(() => {
    if (parentId !== null) api.deckDetail(parentId).then(setParent, (e) => setError(errorText(e)));
  }, [parentId]);

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const id = await api.createDeck(name, target);
      // 고른 중요도로 확정해 둔다. B4 에서 제안이 아닌 실선 칩으로 바로 보인다
      if (isSub) await api.setDeckImportance(id, level);
      if (onCreated) onCreated(id);
      else nav({ name: "deck", deckId: id });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={isSub ? "하위 덱 만들기" : "새 덱 만들기"}
      sub={isSub ? "과목이나 챕터처럼 덱을 폴더로 나눠요" : undefined}
      className={isSub ? "modal--subdeck" : ""}
      onClose={close}
    >
      <form
        className="modal__body"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        {isSub && (
          <label className="field">
            <span className="t-label-m">넣을 곳</span>
            <span className="newdeck__select">
              <Icon name="layers" size={16} className="c-tertiary" />
              <select className="t-body-m" value={target ?? ""} onChange={(e) => setTarget(Number(e.target.value))}>
                {parent && <option value={parent.id}>{parent.name}</option>}
                {/* 깊이 우선 트리. 깊이만큼 전각 공백으로 들여 쓴다 */}
                {parent?.tree.map((s) => (
                  <option key={s.id} value={s.id}>
                    {`${"　".repeat(s.depth)}${s.name}`}
                  </option>
                ))}
              </select>
              <Icon name="chevron-down" size={16} scaled className="c-secondary" />
            </span>
          </label>
        )}
        <Input
          label={isSub ? "이름" : "덱 이름"}
          placeholder={isSub ? undefined : "예: 정보처리기사 실기"}
          value={name}
          autoFocus
          error={error}
          helper={isSub ? "같은 곳에 같은 이름은 만들 수 없어요" : "목표일과 학습 설정은 나중에 덱 안에서 정할 수 있어요."}
          onChange={(e) => {
            setName(e.target.value);
            setError(undefined);
          }}
        />
        {isSub && (
          <div className="newdeck__row">
            <span className="newdeck__label">
              <span className="t-label-m">기본 중요도</span>
              <span className="t-caption c-tertiary">이 하위 덱에 넣는 카드에 적용돼요</span>
            </span>
            {/* ui 의 Segment 는 type 이 없는 button 이라 form 안에서 제출해 버린다 */}
            <span className="segment segment--boxed" role="radiogroup" aria-label="기본 중요도">
              {LEVELS.map((l) => (
                <button
                  key={l}
                  type="button"
                  role="radio"
                  aria-checked={level === l}
                  className={`segment__item t-label-m ${level === l ? "is-selected" : ""}`}
                  onClick={() => setLevel(l)}
                >
                  {LEVEL_LABEL[l]}
                </button>
              ))}
            </span>
          </div>
        )}
        <div className="modal__actions">
          <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={close} />
          <Button type="submit" size="M" label="만들기" shortcut="Enter" disabled={!name.trim() || busy} />
        </div>
      </form>
    </Modal>
  );
}
