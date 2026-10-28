// B1 새 덱 만들기 / C1-s 하위 덱 만들기. 이름 하나만 받고 바로 통과시킨다
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorText, type DeckDetail, type Level } from "../api";
import { Button, Icon, Input, LEVEL_LABEL, type IconName } from "../ui";
import { useModalScope, useNav } from "../nav";
import { composing } from "../keys";

/** 창 안의 [취소] 버튼이 X · Esc · 바깥 클릭과 같은 닫기(고친 것이 있으면 묻기)를 쓰게 */
const CloseContext = createContext<() => void>(() => {});
export const useModalClose = () => useContext(CloseContext);

export function Modal({ title, sub, onClose, children, wide, variant, icon = "alert", className = "", dirty = false }: {
  title: string;
  /** 제목 아래 한 줄 설명 (C1-s). 카드 편집은 노트 유형 칩도 (NT13) */
  sub?: ReactNode;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
  /** confirm: 되돌릴 수 없는 일을 묻는 창 (E1-b). 경고 아이콘, 닫기 버튼 없음 */
  variant?: "confirm";
  /** confirm 창의 아이콘 (C2-d 는 휴지통) */
  icon?: IconName;
  /** 화면마다 다른 폭·위치 */
  className?: string;
  /** 고친 것이 있다. 바깥 클릭 · X · Esc · useModalClose 로 닫을 때 버릴지 먼저 묻는다 (QA T1-02 · T1-14 · T8-05) */
  dirty?: boolean;
}) {
  useModalScope();
  const box = useRef<HTMLDivElement>(null);
  const [asking, setAsking] = useState(false);
  const close = () => (dirty ? setAsking(true) : onClose());
  useFocusTrap(box, close);
  return (
    <>
      {/* 작은 창에서 scrim 이 스크롤되면 그 스크롤 막대를 누른 것은 바깥 클릭이 아니다 */}
      <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && e.clientX < e.currentTarget.clientWidth && close()}>
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
              <button className="modal__close c-tertiary" aria-label="닫기" onClick={close}>
                <Icon name="x" size={20} />
              </button>
            </header>
          )}
          <CloseContext.Provider value={close}>{children}</CloseContext.Provider>
        </div>
      </div>
      {asking && (
        <DiscardModal
          title="고친 내용이 있어요"
          text="저장하지 않고 닫을까요?"
          keep="계속 고치기"
          discard="버리고 닫기"
          onKeep={() => setAsking(false)}
          onDiscard={onClose}
        />
      )}
    </>
  );
}

const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';
/** 열린 모달. 맨 위 것만 Tab 을 가둔다 (C3-e 위의 새 하위 덱 창처럼 겹칠 때) */
const openBoxes: HTMLElement[] = [];

/**
 * Tab 이 모달 밖으로 나가지 않게 한다. onEscape 가 있으면 맨 위 창만 Esc 를 받는다 (창 위에 뜬 확인 창의 Esc 가 아래 창까지 닫지 않게).
 * 닫힌 뒤 연 버튼으로 초점을 돌려주는 것은 useModalScope (nav.tsx)
 */
export function useFocusTrap(ref: React.RefObject<HTMLDivElement | null>, onEscape?: () => void) {
  const esc = useRef(onEscape);
  esc.current = onEscape;
  useEffect(() => {
    const box = ref.current;
    if (!box) return;
    openBoxes.push(box);
    // autoFocus 한 입력이 있으면 그대로, 없으면 창 자체에 (스크린리더가 제목을 읽는다)
    if (!box.contains(document.activeElement)) box.focus();
    const onKey = (e: KeyboardEvent) => {
      if (openBoxes[openBoxes.length - 1] !== box) return;
      if (composing(e)) return; // 한글 조합 중 Esc 는 조합만 끝낸다
      if (e.key === "Escape" && esc.current) {
        if (e.defaultPrevented) return; // 창 안의 팝오버 · 입력 칸이 먼저 받았다
        e.preventDefault();
        return esc.current();
      }
      if (e.key !== "Tab") return;
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
    };
  }, [ref]);
}

/** 쓰던 글 · 고친 내용을 버리기 전에 묻는 창 (C3 카드 추가 · R1-c 초안 · G2 나누기 제안이 같이 쓴다). Esc 는 계속 쓰기 */
export function DiscardModal({ title, text, keep, discard, onKeep, onDiscard }: {
  title: string;
  text: string;
  keep: string;
  discard: string;
  onKeep: () => void;
  onDiscard: () => void;
}) {
  return (
    <Modal title={title} onClose={onKeep} variant="confirm" icon="trash">
      <p className="t-body-m c-secondary">{text}</p>
      <div className="modal__actions">
        <Button variant="ghost" size="M" label={keep} shortcut="Esc" onClick={onKeep} />
        <Button variant="danger" size="M" icon="trash" label={discard} onClick={onDiscard} />
      </div>
    </Modal>
  );
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
