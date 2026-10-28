// 덱 트리에서 하위 덱 고르기. C3-n 넣을 곳 · C3-e 하위 덱 이동 · C2-r 이동 하위 메뉴 · C2-b 이동 ▾ 가 같이 쓴다
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { api, type DeckDetail, type Level } from "../api";
import { Icon } from "../ui";
import { NewDeckModal } from "./NewDeck";
import { useOverlayScope } from "../nav";
import { composing } from "../keys";
import "../styles/deckpicker.css";

/** label: 뿌리 아래 상대 경로 ("2과목 · 네트워크"), 뿌리 자신은 덱 이름. depth: 뿌리 0 */
export type DeckChoice = { id: number; name: string; label: string; path: string[]; depth: number; importance: Level };

/** 뿌리 덱과 그 아래 모든 하위 덱 (빈 덱 포함, 깊이 우선). 첫 항목이 뿌리다 */
export async function loadDeckChoices(rootDeckId: number, detail?: DeckDetail): Promise<DeckChoice[]> {
  const d = detail ?? (await api.deckDetail(rootDeckId));
  const paths = new Map<number, string[]>([[d.id, []]]);
  const list: DeckChoice[] = [{ id: d.id, name: d.name, label: d.name, path: [], depth: 0, importance: d.own_importance }];
  for (const s of d.tree) {
    const path = [...(paths.get(s.parent_id) ?? []), s.name];
    paths.set(s.id, path);
    list.push({ id: s.id, name: s.name, label: path.join(" · "), path, depth: s.depth, importance: s.importance });
  }
  return list;
}

/** 덱 경로(이름들)의 최상위 덱 id. 덱 id 를 모르는 카드(CardView)나 하위 덱에서 들어온 C3 에 쓴다 */
export async function rootDeckIdOf(path: string[]): Promise<number | undefined> {
  const h = await api.home();
  return [...h.decks, ...h.idle_decks].find((d) => d.name === path[0])?.id;
}

/** 알약에 쓰는 경로. "정보처리기사 실기 › 2과목 · 네트워크" */
export const deckPathLabel = (root: DeckChoice, c: DeckChoice) => (c.depth === 0 ? c.name : `${root.name} › ${c.label}`);

/**
 * 드롭다운 본체: 검색 칸 + 덱 트리(지금 위치 ✓) + 구분선 + "+ 새 하위 덱".
 * 위치는 가장 가까운 position: relative 부모 기준 아래(창 아래가 모자라면 위). 바깥을 누르거나 Esc 로 닫힌다.
 * "+ 새 하위 덱"은 C1-s 창을 열고(넣을 곳 = 지금 고른 덱), 만들면 그 덱을 고른 것으로 onPick 한다
 */
export function DeckMenu({ rootDeckId, value, onPick, onClose, choices, className = "" }: {
  rootDeckId: number;
  /** 지금 위치 (✓) */
  value: number | null;
  onPick: (deck: DeckChoice) => void;
  onClose: () => void;
  /** 이미 불러온 목록. 없으면 열 때 불러온다 */
  choices?: DeckChoice[] | null;
  className?: string;
}) {
  const [list, setList] = useState<DeckChoice[] | null>(choices ?? null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [creating, setCreating] = useState(false);
  const [up, setUp] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const listBox = useRef<HTMLDivElement>(null);
  const opened = useRef(false);
  useOverlayScope(); // 열려 있는 동안 화면 단축키는 꺼진다. 메뉴 안의 키(↑↓ · Enter · 글자 찾기)는 메뉴가 받는다

  useEffect(() => {
    if (!choices) loadDeckChoices(rootDeckId).then(setList, () => setList([]));
  }, [rootDeckId, choices]);

  // 창 아래가 모자라면 위로 펼친다
  useLayoutEffect(() => {
    const r = box.current?.getBoundingClientRect();
    if (r && r.bottom > window.innerHeight - 8) setUp(true);
  }, [list]);

  // 처음 열 때 지금 위치(✓)를 목록 가운데로 보이게 하고 키보드 강조도 거기서 시작한다
  useLayoutEffect(() => {
    if (!list || opened.current) return;
    opened.current = true;
    const i = list.findIndex((c) => c.id === value);
    if (i < 0) return;
    setActive(i);
    const lb = listBox.current;
    const el = lb?.children[i] as HTMLElement | undefined;
    if (lb && el) lb.scrollTop = el.offsetTop - (lb.clientHeight - el.offsetHeight) / 2;
  }, [list, value]);

  // 바깥(알약 · 하위 메뉴를 담은 부모 밖)을 누르면 닫힌다. 하위 덱 만들기 창이 떠 있는 동안은 그 창이 받는다
  useEffect(() => {
    if (creating) return;
    const close = (e: MouseEvent) => !box.current?.parentElement?.contains(e.target as Node) && onClose();
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [creating, onClose]);

  // Esc 는 캡처 단계에서 먼저 받는다. 바깥 화면(C3)이나 바깥 창(C3-e)의 Esc 가 함께 닫히지 않게
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      if (creating) setCreating(false);
      else onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [creating, onClose]);

  const q = query.trim().toLowerCase();
  const rows = (list ?? []).filter((c) => !q || c.label.toLowerCase().includes(q));
  const count = rows.length + 1; // 마지막은 "+ 새 하위 덱"
  const parentId = value ?? rootDeckId;

  const choose = (i: number) => (i < rows.length ? onPick(rows[i]) : setCreating(true));
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation(); // 검색 칸의 키가 화면 단축키 · Ctrl Enter 저장으로 새지 않게
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => {
        const next = (a + (e.key === "ArrowDown" ? 1 : count - 1)) % count;
        (listBox.current?.children[next] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" });
        return next;
      });
    } else if (e.key === "Enter" && !composing(e.nativeEvent)) {
      // 한글 조합을 끝내는 Enter 는 고르지 않는다 (남은 Enter 가 알약 버튼을 다시 눌러 메뉴가 열린다, T8-09)
      e.preventDefault();
      choose(Math.min(active, count - 1));
    }
  };

  return (
    <>
      <div
        ref={box}
        className={`deckmenu ${up ? "is-up" : ""} ${className}`}
        role="dialog"
        aria-label="하위 덱 고르기"
        hidden={creating}
        onKeyDown={onKeyDown}
      >
        <label className="deckmenu__search">
          <Icon name="search" size={14} className="c-tertiary" />
          <input
            className="t-body-s"
            placeholder="덱이나 하위 덱 찾기"
            value={query}
            autoFocus
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
          />
        </label>
        <div ref={listBox} className="deckmenu__list" role="listbox">
          {rows.map((c, i) => (
            <button
              key={c.id}
              type="button"
              role="option"
              aria-selected={c.id === value}
              className={`deckmenu__row ${i === active ? "is-active" : ""}`}
              style={{ paddingLeft: c.depth === 0 ? undefined : 26 + (q ? 0 : (c.depth - 1) * 16) }}
              onMouseEnter={() => setActive(i)}
              onClick={() => choose(i)}
            >
              {c.depth === 0 && <Icon name="layers" size={16} className="c-tertiary" />}
              <span className="deckmenu__text">
                {/* 찾을 때는 위치를 알 수 있게 경로로 */}
                <span className="t-body-m">{c.depth === 0 ? c.name : q ? c.label : c.name}</span>
                {c.depth === 0 && <span className="t-caption c-tertiary">하위 덱 없이 바로 아래</span>}
              </span>
              {c.id === value && <Icon name="check" size={16} className="c-accent" />}
            </button>
          ))}
          {list && rows.length === 0 && <p className="deckmenu__empty t-body-s c-tertiary">찾는 덱이 없어요</p>}
        </div>
        <div className="deckmenu__rule" />
        <button
          type="button"
          className={`deckmenu__row deckmenu__new ${active === count - 1 ? "is-active" : ""}`}
          onMouseEnter={() => setActive(count - 1)}
          onClick={() => setCreating(true)}
        >
          <Icon name="plus" size={16} className="c-accent" />
          <span className="deckmenu__text">
            <span className="t-body-m c-accent">새 하위 덱</span>
            <span className="t-caption c-tertiary">만들면 바로 여기가 선택돼요</span>
          </span>
        </button>
      </div>
      {creating && (
        // body 로 띄운다. C3-e 처럼 transform 이 걸린 창 안에서는 fixed 배경이 창 안에 갇힌다.
        // 창 안의 키가 바깥 화면(C3 Ctrl Enter 저장 등)으로 새지 않게 여기서 멈춘다 (React 이벤트는 포털도 이 div 를 지난다)
        <div className="deckmenu__modal" onKeyDown={(e) => e.stopPropagation()}>
          {createPortal(
          <NewDeckModal
            parentId={parentId}
            onClose={() => setCreating(false)}
            onCreated={async (id) => {
              const fresh = await loadDeckChoices(rootDeckId);
              setList(fresh);
              setCreating(false);
              const made = fresh.find((c) => c.id === id);
              if (made) onPick(made);
            }}
          />,
          document.body,
          )}
        </div>
      )}
    </>
  );
}

/**
 * 덱 경로 알약 + DeckMenu. C3-n "넣을 곳", C3-e 하위 덱 이동.
 * onLoad 는 목록을 불러올 때마다 (처음, 새 하위 덱을 만든 뒤) 불린다. 고른 덱의 중요도 등을 쓰려는 화면용
 */
export function DeckPicker({ rootDeckId, value, onChange, onLoad, relative = false, prefix }: {
  /** 트리의 뿌리 (최상위 덱). 이 덱과 그 아래만 고를 수 있다 */
  rootDeckId: number;
  /** 지금 고른 덱. 모르면 undefined (알약이 비어 보인다) */
  value: number | undefined;
  onChange: (deck: DeckChoice) => void;
  onLoad?: (choices: DeckChoice[]) => void;
  /** C3-e: 알약에 최상위 덱 이름 없이 상대 경로만. 넘치면 앞쪽을 자르고 전체 경로는 툴팁으로 */
  relative?: boolean;
  /** 알약 앞 글자 (R1-c "넣을 곳") */
  prefix?: string;
}) {
  const [choices, setChoices] = useState<DeckChoice[] | null>(null);
  const [open, setOpen] = useState(false);
  const loaded = useRef(onLoad);
  loaded.current = onLoad;

  const reload = (id: number) =>
    loadDeckChoices(id).then((list) => {
      setChoices(list);
      loaded.current?.(list);
    });
  useEffect(() => {
    reload(rootDeckId).catch(() => setChoices([]));
  }, [rootDeckId]);

  const current = choices?.find((c) => c.id === value);
  return (
    <span className="deckpick">
      <button
        type="button"
        className={`deckpick__pill ${open ? "is-open" : ""}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={!choices?.length}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="layers" size={14} className="c-tertiary" />
        {prefix && <span className="t-label-m c-secondary">{prefix} ·</span>}
        {current && choices ? (
          <span
            className={`deckpick__label t-label-m ${relative ? "deckpick__label--tail" : ""}`}
            title={relative ? deckPathLabel(choices[0], current) : undefined}
          >
            {/* 앞쪽 말줄임은 rtl 상자 + ltr 글자로 (끝이 남아야 어느 하위 덱인지 보인다) */}
            {relative ? <bdi>{current.label}</bdi> : deckPathLabel(choices[0], current)}
          </span>
        ) : (
          <span className="deckpick__label" />
        )}
        <Icon name="chevron-down" size={14} scaled className="c-secondary" />
      </button>
      {open && choices && (
        <DeckMenu
          rootDeckId={rootDeckId}
          value={value ?? null}
          choices={choices}
          onClose={() => setOpen(false)}
          onPick={(d) => {
            setOpen(false);
            if (!choices.some((c) => c.id === d.id)) reload(rootDeckId); // 방금 만든 하위 덱
            onChange(d);
          }}
        />
      )}
    </span>
  );
}
