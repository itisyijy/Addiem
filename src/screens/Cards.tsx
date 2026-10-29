// C2 카드 목록 / C2-m 중요도 메뉴 / C2-d 카드 삭제 확인 / C2-r 행 ⋯ 메뉴 / C2-b 일괄 선택
import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
import { api, errorText, qualityApi, type CardList, type CardRow, type DeckDetail, type ExamState, type Level } from "../api";
import { Badge, Button, Icon, Importance, Kbd, LEVEL_LABEL, type BadgeTone } from "../ui";
import { n, studyDate, useKey, useNav, ro, useOverlayScope } from "../nav";
import { actionKey } from "../keys";
import { useToast, type ToastInput } from "../toast";
import { Modal, NewDeckModal } from "./NewDeck";
import { EditNote } from "./Study";
import { buryReturnDay, menuKeys } from "./Deck";
import { DeckMenu, loadDeckChoices, type DeckChoice } from "./DeckPicker";
import { ExamDialog } from "./Exam";
import { OcclusionRow } from "./Occlusion";
import { exam as examApi } from "../written";
import { ChangeTypeDialog } from "./NoteTypeEdit";

const PAGE = 100;
/** C2-m 메뉴 순서. 숫자키 1/2/3 이 이 순서다 */
const LEVELS: Level[] = [2, 1, 0];
// Figma 의 8px 점. 데이터 표시라 중요도 글자색을 쓴다 (보충도 border/strong 이 아니라 supp-fg)
const DOT: Record<Level, string> = { 2: "dot--core", 1: "dot--normal", 0: "dot--supp" };
/** AU6 목록은 소리 칩 대신 작은 스피커 (재생 없음). 글자 앞에 두어 말줄임에 가려지지 않게 */
const Snd = () => <Icon name="volume-2" size={14} scaled className="cards__snd c-tertiary" />;

const STATE: Record<CardRow["state"], [string, BadgeTone]> = {
  new: ["신규", "accent"],
  learning: ["학습 중", "neutral"],
  relearning: ["학습 중", "neutral"],
  review: ["복습", "neutral"],
  suspended: ["학습 중지", "neutral"],
};
/** 접어둔 카드 배지는 상태 이름 먼저, 그다음 복귀일 (copy-guide 접어두기). 툴팁에 무슨 날짜인지 적는다 */
const buriedDay = (until: string | null) =>
  !until ? null : until <= studyDate().toLocaleDateString("sv-SE") ? "오늘" : `${+until.slice(5, 7)}/${+until.slice(8, 10)}`;
const key = (path: string[]) => path.join("\u0000");
/** "…으로/로". 받침이 있으면 으로, 없거나 ㄹ 받침이면 로 */

/** 목록 범위. deckId 가 null 이면 모든 덱 (홈의 "접어둔 카드 보기") */
type Scope = Pick<DeckDetail, "name" | "path" | "total" | "subdecks">;

export function Cards({ deckId, buriedOnly = false, cardIds, weakOnly = false, edit, query: start }: {
  deckId: number | null;
  buriedOnly?: boolean;
  /** 이 카드들만 본다 (C1-xr [카드 보기]: 내보내며 미디어가 빠진 카드) */
  cardIds?: number[];
  /** 약함으로 확정한 하위 덱 아래만 (C1-w [약한 챕터 카드 보기]) */
  weakOnly?: boolean;
  /** 노트 유형 편집기에서 돌아왔다: 이 카드의 편집 창(E)을 다시 연다 (NT53) */
  edit?: number;
  /** 처음 검색 글자 (되돌리기 결과 [보관한 카드 보기] 의 "tag:되돌린_업데이트") */
  query?: string;
}) {
  const nav = useNav();
  const toast = useToast();
  const [deck, setDeck] = useState<Scope | null>(null);
  // 이 덱과 그 아래 하위 덱. 이동 메뉴 · "하위 덱 설정 따르기" 캡션 · 이동 되돌리기(카드 행에 덱 id 가 없어 경로로 찾는다)에 쓴다
  const [choices, setChoices] = useState<DeckChoice[]>([]);
  const [scope, setScope] = useState<number | null>(deckId);
  const [only, setOnly] = useState(cardIds ?? null);
  const [weak, setWeak] = useState(weakOnly); // C1-w [약한 챕터 카드 보기]
  const [susp, setSusp] = useState(false); // [학습 중지 n장 보기]
  const [query, setQuery] = useState(start ?? "");
  const [list, setList] = useState<CardList | null>(null);
  // Ctrl A · 머리 체크박스로 받은, 아직 불러오지 않은 뒤쪽 카드 (질문 · 답 없이, P0-7)
  const [extra, setExtra] = useState<CardRow[]>([]);
  const shown = useRef(PAGE); // 다시 불러올 때 받을 장수: 지금까지 이어 붙인 만큼
  const gen = useRef(0); // 거르기가 바뀐 뒤 늦게 온 응답을 버린다
  const loadingMore = useRef(false);
  const [scopeMenu, setScopeMenu] = useState(false);
  const [impMenu, setImpMenu] = useState<number | null>(null);
  const [rowMenu, setRowMenu] = useState<number | null>(null);
  const [rowMove, setRowMove] = useState(false);
  const [barMenu, setBarMenu] = useState<"imp" | "move" | "more" | null>(null);
  const [changing, setChanging] = useState<number[] | null>(null); // NT9 다른 유형으로 옮길 노트
  const [selected, setSelected] = useState<Set<number>>(() => new Set());
  const anchor = useRef<number | null>(null); // Shift 클릭 범위의 시작 카드 id (목록이 바뀌면 그 카드가 있는 자리, T1-11)
  const [editing, setEditing] = useState<CardRow | null>(null);
  useEffect(() => {
    if (edit !== undefined) api.listCards(null, "", 1, false, [edit]).then((l) => l.rows[0] && setEditing(l.rows[0]), () => {});
  }, [edit]);
  const [deleting, setDeleting] = useState<CardRow | null>(null);
  const [bulkDelete, setBulkDelete] = useState(false);
  const [examFor, setExamFor] = useState<CardRow[] | null>(null); // N2 기출 표시 창
  const [newSub, setNewSub] = useState(false);
  const [error, setError] = useState<string>();
  const foot = useRef<HTMLDivElement>(null);
  const lastTrigger = useRef<HTMLElement | null>(null); // Esc 로 메뉴를 닫으면 초점을 여기로 돌려준다
  const moveTrigger = useRef<HTMLButtonElement>(null);

  const loadDeck = useCallback(() => {
    if (deckId === null) return setDeck({ name: "모든 덱", path: [], total: 0, subdecks: [] });
    api.deckDetail(deckId).then(
      (d) => {
        setDeck(d);
        loadDeckChoices(deckId, d).then(setChoices);
      },
      (e) => setError(errorText(e)),
    );
  }, [deckId]);
  useEffect(loadDeck, [loadDeck]);
  // G1 진입 칩 "고칠 만한 카드 N" (숨긴 카드는 빼고 센다). 편집 · 목록에서 바꾼 뒤에도 다시 센다
  const [fixable, setFixable] = useState(0);
  const loadFixable = useCallback(() => {
    if (deckId !== null && !buriedOnly) qualityApi.check(deckId).then((c) => setFixable(c.filter((x) => !x.hidden).length), () => {});
  }, [deckId, buriedOnly]);
  useEffect(loadFixable, [loadFixable]);

  const page = useCallback(
    (limit: number, after: number | null = null, bare = false) => api.listCards(scope, query, limit, buriedOnly, only, weak, susp, after, bare),
    [scope, query, buriedOnly, only, weak, susp],
  );
  const load = useCallback(() => {
    const g = ++gen.current;
    return page(shown.current).then((l) => void (g === gen.current && setList(l)), (e) => setError(errorText(e)));
  }, [page]);
  useEffect(() => {
    const t = setTimeout(load, 150); // 입력 중에는 기다렸다가 찾는다
    return () => clearTimeout(t);
  }, [load]);

  const rows = list?.rows ?? [];
  // 끝에 닿으면 마지막 카드 다음 100장을 이어 붙인다 (처음부터 다시 받지 않는다, T9-04)
  const more = !!list && rows.length < list.total;
  useEffect(() => {
    const el = foot.current;
    const last = rows[rows.length - 1]?.id;
    if (!el || !more || last === undefined) return;
    const io = new IntersectionObserver((e) => {
      if (!e[0].isIntersecting || loadingMore.current) return;
      loadingMore.current = true;
      const g = gen.current;
      page(PAGE, last)
        .then((l) => {
          if (g !== gen.current) return;
          shown.current = rows.length + l.rows.length;
          setList((p) => p && { ...l, rows: [...p.rows, ...l.rows] });
        }, (e) => setError(errorText(e)))
        .finally(() => (loadingMore.current = false));
    });
    io.observe(el);
    return () => io.disconnect();
  }, [more, list, page]);

  const loaded = new Set(rows.map((r) => r.id));
  const picked = [...rows.filter((c) => selected.has(c.id)), ...extra.filter((c) => selected.has(c.id) && !loaded.has(c.id))];
  const selecting = picked.length > 0;
  const clearSelection = () => {
    setSelected(new Set());
    setExtra([]);
  };
  /** 지금 검색 · 거르기에 걸린 카드 전부. 아직 불러오지 않은 카드는 그리지 않은 행으로 받는다 (P0-7) */
  const selectAll = () => {
    if (!list) return;
    const ids = rows.map((r) => r.id);
    if (rows.length >= list.total) return setSelected(new Set(ids));
    const g = gen.current;
    page(list.total, ids[ids.length - 1] ?? null, true).then((l) => {
      if (g !== gen.current) return;
      setExtra(l.rows);
      setSelected(new Set([...ids, ...l.rows.map((r) => r.id)]));
    }, (e) => setError(errorText(e)));
  };
  const headCheck = useRef<HTMLButtonElement>(null);
  /** 선택 바의 일을 마치면 바가 사라지므로 초점을 머리 체크박스로 옮긴다 (모달이 초점을 돌려준 다음에) */
  const endSelection = () => {
    clearSelection();
    if (selecting) setTimeout(() => headCheck.current?.focus());
  };
  const dialog = editing !== null || !!deleting || bulkDelete || newSub || !!examFor || !!changing;

  // 선택 바가 토스트 자리를 쓰는 동안 토스트를 바 위로 올린다 (toast.css 의 --toast-lift)
  useEffect(() => {
    if (!selecting) return;
    document.body.style.setProperty("--toast-lift", "64px");
    return () => {
      document.body.style.removeProperty("--toast-lift");
    };
  }, [selecting]);

  // Ctrl A: 지금 검색 · 거르기 결과의 카드 전부 (스크롤해 불러오지 않은 카드도, P0-7). 검색 칸에서는 글자 전체 선택 그대로
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "a" || dialog) return;
      const t = e.target as HTMLElement;
      if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable) return;
      e.preventDefault();
      selectAll();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const back = useCallback(() => nav(deckId === null ? { name: "home" } : { name: "deck", deckId }), [nav, deckId]);
  const menuOpen = impMenu !== null || scopeMenu || rowMenu !== null || barMenu !== null;
  const closeMenus = () => {
    setImpMenu(null);
    setScopeMenu(false);
    setRowMenu(null);
    setRowMove(false);
    setBarMenu(null);
  };
  useKey({ action: "addCard" }, () => deckId !== null && nav({ name: "addCard", deckId }), !dialog);
  // 메뉴가 떠 있는 동안 화면 단축키는 꺼지고 메뉴 안의 키만 받는다 (E4 노트). Esc 는 메뉴 → 선택 → 화면 순서로 하나씩 닫는다
  useOverlayScope(menuOpen);
  useKey(
    "Escape",
    () => {
      closeMenus();
      lastTrigger.current?.focus();
    },
    menuOpen,
    { modal: true },
  );
  useKey("Escape", clearSelection, selecting && !menuOpen && !dialog);
  useKey("Escape", back, !dialog && !menuOpen && !selecting);
  useKey("Delete", () => setBulkDelete(true), selecting && !menuOpen && !dialog);

  const act = async (work: Promise<unknown>) => {
    try {
      await work;
      setError(undefined);
      loadFixable();
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };
  /** 하고 나서 5초 되돌리기 토스트. 되돌리기는 반대 명령을 부른다 */
  const undoable = (msg: Omit<ToastInput, "tone" | "action">, work: Promise<unknown>, undo: () => Promise<unknown>) => {
    closeMenus();
    endSelection();
    act(work.then(() => toast({ ...msg, tone: "undo", action: { label: "되돌리기", onClick: () => act(undo()) } })));
  };

  /** C2-m 한 장. 고르는 즉시 저장하고 메뉴를 닫는다. null 은 하위 덱 설정 따르기 */
  const pick = (c: CardRow, level: Level | null) => {
    setImpMenu(null);
    act(api.setCardImportance(c.id, level));
  };
  /** C2-b 여러 장. 되돌리기는 카드마다 원래 값(개별 지정이 없으면 null)으로, 같은 값끼리 묶어서 */
  const setLevels = (cards: CardRow[], level: Level | null) => {
    const before = new Map<Level | null, number[]>();
    for (const c of cards) {
      const was = c.own_importance ? c.importance : null;
      before.set(was, [...(before.get(was) ?? []), c.id]);
    }
    undoable(
      { title: level === null ? `${n(cards.length)}장을 하위 덱 설정으로 되돌렸어요` : `${n(cards.length)}장을 ${LEVEL_LABEL[level]}으로 바꿨어요` },
      api.setCardsImportance(cards.map((c) => c.id), level),
      () => Promise.all([...before].map(([was, ids]) => api.setCardsImportance(ids, was))),
    );
  };
  const impCard = rows.find((c) => c.id === impMenu);
  const impPick = impCard ? (l: Level) => pick(impCard, l) : barMenu === "imp" ? (l: Level) => setLevels(picked, l) : null;
  useKey(["1", "2", "3"], (e) => impPick?.(LEVELS[Number(e.key) - 1]), !!impPick, { modal: true });

  /** 학습 밖 접어두기 (3일 뒤 다시). 이미 접어둔 카드는 되돌릴 때 꺼내지 않게 뺀다 */
  const bury = (cards: CardRow[]) => {
    const ids = cards.filter((c) => !c.buried).map((c) => c.id);
    if (!ids.length) return (closeMenus(), endSelection(), toast({ title: "이미 접어둔 카드예요" }));
    undoable(
      { title: ids.length > 1 ? `${n(ids.length)}장을 접어뒀어요` : "접어뒀어요", text: `${buryReturnDay()}부터 오늘의 학습에 다시 포함돼요` },
      api.buryCards(ids),
      () => api.unburyCards(ids),
    );
  };
  /** 학습 중지 풀기 (Q4 M2: 되돌리기가 보관한 카드 · 가져온 학습 중지 카드) */
  const unsuspend = (cards: CardRow[]) => {
    const ids = cards.filter((c) => c.state === "suspended").map((c) => c.id);
    closeMenus();
    endSelection();
    act(api.unsuspendCards(ids).then((k) => toast({ title: k > 1 ? `${n(k)}장의 학습 중지를 풀었어요` : "학습 중지를 풀었어요", text: "다음에 시작하는 학습부터 포함돼요" })));
  };
  const unbury = (c: CardRow) => {
    closeMenus();
    act(api.unburyCards([c.id]).then(() => toast({ title: "접어두기를 풀었어요", text: "다음에 시작하는 학습부터 다시 포함돼요" })));
  };

  const deckIdOf = new Map(choices.map((c) => [key(c.path), c.id]));
  const rel = (p: string[]) => p.slice(deck?.path.length ?? 0);
  /** 다른 하위 덱으로. 되돌리기는 원래 덱별로 묶어 돌려보낸다 */
  const move = (cards: CardRow[], to: DeckChoice) => {
    const from = new Map<number, number[]>();
    for (const c of cards) {
      const id = deckIdOf.get(key(rel(c.deck_path)));
      if (id !== undefined) from.set(id, [...(from.get(id) ?? []), c.id]);
    }
    if (!choices.some((c) => c.id === to.id)) loadDeck(); // 방금 만든 하위 덱. 필터와 목록에 넣는다
    undoable(
      { title: `${cards.length > 1 ? `${n(cards.length)}장을 ` : ""}${to.label}${ro(to.label)} 옮겼어요` },
      api.moveCards(cards.map((c) => c.id), to.id),
      () => Promise.all([...from].map(([id, ids]) => api.moveCards(ids, id))),
    );
  };

  /** N2 기출 표시 (노트마다 한 번). 되돌리기는 표시한 노트를 원래 값으로 */
  const markExam = (name: string, todo: ExamState[]) => {
    setExamFor(null);
    undoable({ title: `노트 ${n(todo.length)}개에 "${name}" 기출을 표시했어요` }, examApi.mark("note", todo.map((s) => s.id), name), () => examApi.set("note", todo));
  };
  const clearExam = (c: CardRow) =>
    undoable({ title: "기출 표시를 뺐어요" }, examApi.set("note", [{ id: c.note_id, hits: null, names: [] }]), () => examApi.set("note", [c.exam]));

  const remove = (c: CardRow) => {
    setDeleting(null);
    act(api.deleteCard(c.id).then(() => toast({ title: "카드를 지웠어요" })));
  };
  const removeMany = (cards: CardRow[]) => {
    setBulkDelete(false);
    endSelection();
    act(api.deleteCards(cards.map((c) => c.id)).then(() => toast({ title: `${n(cards.length)}장을 지웠어요` })));
  };

  // C2-r 행 메뉴가 열려 있을 때 E 편집, Del 삭제 (메뉴에 적힌 키)
  const menuRow = rows.find((c) => c.id === rowMenu);
  useKey(
    ["e", "Delete"],
    (e) => {
      if (!menuRow) return;
      closeMenus();
      if (e.key === "Delete") setDeleting(menuRow);
      else setEditing(menuRow);
    },
    !!menuRow && !rowMove,
    { modal: true },
  );

  if (!deck) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;
  const scopeName = scope === deckId ? "전체" : (deck.subdecks.find((s) => s.id === scope)?.name ?? "");
  const relPath = (p: string[]) => rel(p).join(" · ") || deck.name;
  const levelOf = (p: string[]) => choices.find((c) => key(c.path) === key(rel(p)))?.importance;
  const canMove = deckId !== null && choices.length > 1;

  /** Shift 클릭이면 앞서 누른 카드부터 여기까지 고른다. 그 카드가 지금 목록에 없으면 이 행만 */
  const toggle = (i: number, e: MouseEvent) => {
    const at = e.shiftKey && anchor.current !== null ? rows.findIndex((r) => r.id === anchor.current) : -1;
    const from = at < 0 ? null : at;
    anchor.current = rows[i].id;
    setSelected((prev) => {
      const next = new Set(prev);
      if (from !== null) {
        const [a, b] = [from, i].sort((x, y) => x - y);
        rows.slice(a, b + 1).forEach((r) => next.add(r.id));
      } else if (!next.delete(rows[i].id)) next.add(rows[i].id);
      return next;
    });
  };
  const all = !!list && list.total > 0 && picked.length >= list.total;
  const resetFilter = () => {
    shown.current = PAGE;
    anchor.current = null;
    clearSelection(); // 보이지 않게 된 카드를 고른 채로 두지 않는다
  };
  const sameLevel = picked.every((c) => c.importance === picked[0]?.importance) ? picked[0]?.importance : undefined;
  // NT9 다른 유형으로 옮기기는 한 유형의 노트만. 섞였으면 메뉴에서 미리 막고 몇 종인지 알린다 (T1-35)
  const pickedTypes = new Set(picked.map((c) => c.note_type_id)).size;
  const ownCount = picked.filter((c) => c.own_importance).length;

  return (
    <main className={`cards ${selecting ? "cards--selecting" : ""}`} onClick={closeMenus}>
      <button className="backlink t-label-m c-secondary" onClick={back}>
        <Icon name="chevron-left" size={18} />
        {deckId === null ? "홈" : deck.name}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">{buriedOnly ? "접어둔 카드" : "카드"} {n(list?.total ?? deck.total)}장</h1>
          <p className="t-body-s c-secondary">
            {/* 결과에 맞게: 접어둔 카드 화면만 "접어둔 카드" (Q4 디자인 검토) */}
            {deckId === null
              ? only
                ? "모든 덱 · 고른 카드만 보여요"
                : buriedOnly
                  ? "모든 덱 · 자동으로 또는 직접 접어둔 카드"
                  : susp
                    ? "모든 덱 · 학습 중지 카드만 보여요"
                    : query.trim()
                      ? "모든 덱 · 찾은 카드"
                      : "모든 덱"
              : `${deck.name} · 중요도는 하위 덱 설정을 따르고, 카드마다 직접 지정할 수 있어요`}
          </p>
        </div>
        {deckId !== null && <Button size="M" icon="plus" label="카드 추가" shortcut={actionKey("addCard")} onClick={() => nav({ name: "addCard", deckId })} />}
      </div>

      <div className="cards__tools">
        <label className="cards__search">
          <Icon name="search" size={18} className="c-tertiary" />
          <input
            className="t-body-m"
            placeholder="질문이나 답변으로 검색 · tag:태그"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              resetFilter();
            }}
          />
        </label>
        {/* 하위 덱이 없어도 연다. 맨 아래 "새 하위 덱"이 C1-s 진입점이다 */}
        {deckId !== null && (
          <div className="menu">
            <button
              className="cards__scope t-body-m"
              aria-haspopup="menu"
              aria-expanded={scopeMenu}
              onClick={(e) => {
                e.stopPropagation();
                lastTrigger.current = e.currentTarget;
                closeMenus();
                setScopeMenu(!scopeMenu);
              }}
            >
              하위 덱 · {scopeName}
              <Icon name="chevron-down" size={16} scaled className="c-secondary" />
            </button>
            {scopeMenu && (
              <div className="menu__list cards__scope-list" role="menu" aria-label="하위 덱" onKeyDown={menuKeys} onClick={(e) => e.stopPropagation()}>
                {[{ id: deckId, name: "전체" }, ...deck.subdecks].map((s, i) => (
                  <button
                    key={s.id}
                    role="menuitem"
                    autoFocus={i === 0}
                    className="menu__item t-body-m"
                    onClick={() => {
                      setScope(s.id);
                      resetFilter();
                      setScopeMenu(false);
                    }}
                  >
                    {s.name}
                  </button>
                ))}
                <div className="menu__sep" role="separator" />
                <button
                  role="menuitem"
                  className="menu__item t-body-m"
                  onClick={() => {
                    setScopeMenu(false);
                    setNewSub(true);
                  }}
                >
                  <Icon name="plus" size={16} />
                  새 하위 덱
                </button>
              </div>
            )}
          </div>
        )}
        {only && (
          <span className="cards__only t-label-m">
            카드 {n(only.length)}장만 보는 중
            <button
              className="c-accent"
              onClick={() => {
                setOnly(null);
                resetFilter();
              }}
            >
              모두 보기
            </button>
          </span>
        )}
        {weak && (
          <span className="cards__only t-label-m">
            취약 챕터만 보는 중
            <button
              className="c-accent"
              onClick={() => {
                setWeak(false);
                resetFilter();
              }}
            >
              모두 보기
            </button>
          </span>
        )}
        {susp ? (
          <span className="cards__only t-label-m">
            학습 중지 카드만 보는 중
            <button
              className="c-accent"
              onClick={() => {
                setSusp(false);
                resetFilter();
              }}
            >
              모두 보기
            </button>
          </span>
        ) : (
          !buriedOnly &&
          (list?.suspended ?? 0) > 0 && (
            // 가져온 Anki 일시 정지 카드 · 되돌리기가 보관한 카드 (태그가 없는 것도) 를 거른다
            <button
              className="cards__fix t-label-m"
              onClick={() => {
                setSusp(true);
                resetFilter();
              }}
            >
              <Icon name="pause" size={16} scaled />
              학습 중지 {n(list?.suspended ?? 0)}장 보기
            </button>
          )
        )}
        {error && <span className="t-body-s c-danger">{error}</span>}
        {deckId !== null && fixable > 0 && !selecting && (
          <button className="cards__fix t-label-m" onClick={() => nav({ name: "quality", deckId })}>
            <Icon name="layers" size={16} />
            고칠 만한 카드 {n(fixable)}
          </button>
        )}
        {selecting && <span className="cards__hint t-caption c-tertiary">Shift 클릭으로 범위 선택 · Ctrl A 모두 선택</span>}
      </div>

      {/* 고른 장수를 읽어 준다. 바가 생기기 전부터 있어야 첫 선택도 읽힌다 */}
      <span className="cards__live" aria-live="polite">
        {selecting ? `${n(picked.length)}장 선택됨` : ""}
      </span>
      {/* C2-b 선택 바. 토스트 자리에 뜨고, 메뉴는 위로 펼친다.
          DOM 은 표 앞에 둔다: 키보드로 카드 수백 장을 지나지 않고 바에 닿게 */}
      {/* 창이 떠 있으면 숨기기만 한다. 빼면 창을 연 바 버튼이 사라져 닫은 뒤 초점이 body 로 빠진다 (QA V9-07) */}
      {selecting && (
        <div className="cards__bar" role="toolbar" aria-label="고른 카드" style={dialog ? { display: "none" } : undefined} onClick={(e) => e.stopPropagation()}>
          <span className="t-label-m">{n(picked.length)}장 선택됨</span>
          <span className="cards__bar-sep" />
          <span className="menu">
            <button
              className="cards__bar-btn t-label-m"
              aria-haspopup="menu"
              aria-expanded={barMenu === "imp"}
              onClick={(e) => {
                lastTrigger.current = e.currentTarget;
                setBarMenu(barMenu === "imp" ? null : "imp");
              }}
            >
              <Icon name="layers" size={16} />
              중요도
              <Icon name="chevron-down" size={14} />
            </button>
            {barMenu === "imp" && (
              <ImportanceMenu
                className="cards__bar-menu"
                cap={`고른 ${n(picked.length)}장을 바꿔요`}
                current={sameLevel}
                canFollow={ownCount > 0}
                follow={ownCount > 0 ? `직접 지정한 ${n(ownCount)}장이 하위 덱 설정으로` : "직접 지정한 카드가 없어요"}
                onPick={(l) => setLevels(picked, l)}
              />
            )}
          </span>
          {canMove && (
            <span className="menu">
              <button
                className="cards__bar-btn t-label-m"
                aria-haspopup="dialog"
                aria-expanded={barMenu === "move"}
                onClick={(e) => {
                  lastTrigger.current = e.currentTarget;
                  setBarMenu(barMenu === "move" ? null : "move");
                }}
              >
                <Icon name="folder" size={16} />
                이동
                <Icon name="chevron-down" size={14} />
              </button>
              {barMenu === "move" && (
                <DeckMenu
                  className="cards__bar-deckmenu"
                  rootDeckId={deckId}
                  value={null}
                  choices={choices}
                  onClose={() => {
                    setBarMenu(null);
                    lastTrigger.current?.focus();
                  }}
                  onPick={(to) => move(picked, to)}
                />
              )}
            </span>
          )}
          <button className="cards__bar-btn t-label-m" onClick={() => (closeMenus(), setExamFor(picked))}>
            <Icon name="target" size={16} />
            기출 표시
          </button>
          <button className="cards__bar-btn t-label-m" onClick={() => bury(picked)}>
            <Icon name="archive" size={16} />
            접어두기
          </button>
          <button className="cards__bar-btn cards__bar-btn--danger t-label-m" onClick={() => setBulkDelete(true)}>
            <Icon name="trash" size={16} />
            삭제
          </button>
          {/* NT9 [⋯] › 다른 유형으로 옮기기 (노트 단위: 고른 카드의 노트) */}
          <span className="menu">
            <button
              className="cards__bar-btn t-label-m"
              aria-label="더 보기"
              aria-haspopup="menu"
              aria-expanded={barMenu === "more"}
              onClick={(e) => {
                lastTrigger.current = e.currentTarget;
                setBarMenu(barMenu === "more" ? null : "more");
              }}
            >
              <Icon name="more" size={16} />
            </button>
            {barMenu === "more" && (
              <div className="menu__list menu--panel cards__bar-menu cards__bar-more" role="menu" onKeyDown={menuKeys} onClick={(e) => e.stopPropagation()}>
                <button
                  role="menuitem"
                  className="menu__item t-body-m"
                  autoFocus
                  disabled={pickedTypes > 1}
                  onClick={() => {
                    closeMenus();
                    setChanging([...new Set(picked.map((c) => c.note_id))]);
                  }}
                >
                  <Icon name="layers" size={16} />
                  <span className="menu__text">
                    <span className="t-body-m">다른 유형으로 옮기기</span>
                    <span className="t-caption c-tertiary">
                      {pickedTypes > 1
                        ? `노트 유형 ${n(pickedTypes)}종이 섞여 있어요 · 한 유형만 골라 주세요`
                        : `고른 카드의 노트 ${n(new Set(picked.map((c) => c.note_id)).size)}개`}
                    </span>
                  </span>
                </button>
                {picked.some((c) => c.state === "suspended") && (
                  <button role="menuitem" className="menu__item t-body-m" onClick={() => unsuspend(picked)}>
                    <Icon name="play" size={16} scaled />
                    <span className="menu__text">
                      <span className="t-body-m">학습 중지 풀기</span>
                      <span className="t-caption c-tertiary">학습 중지 카드 {n(picked.filter((c) => c.state === "suspended").length)}장</span>
                    </span>
                  </button>
                )}
              </div>
            )}
          </span>
          <button className="cards__bar-clear t-label-s" onClick={clearSelection}>
            <Icon name="x" size={14} />
            선택 해제 Esc
          </button>
        </div>
      )}

      <div className="cards__table">
        <div className="cards__row cards__row--head t-label-s c-tertiary">
          <span className="cards__check">
            <button
              ref={headCheck}
              role="checkbox"
              aria-checked={all ? true : selecting ? "mixed" : false}
              aria-label="카드 모두 선택"
              className={`cards__cb ${selecting ? "is-on" : ""}`}
              onClick={(e) => {
                e.stopPropagation();
                if (all) clearSelection();
                else selectAll();
              }}
            >
              {all ? <Icon name="check" size={14} /> : selecting && <span className="cards__cb-mixed" />}
            </button>
          </span>
          <span>질문</span>
          <span>답변</span>
          <span>하위 덱</span>
          <span>중요도</span>
          <span>상태</span>
          <span />
        </div>
        {rows.map((c, i) => {
          const day = c.buried ? buriedDay(c.buried_until) : null;
          const [label, tone] = c.buried ? ([day ? `접어둠 · ${day}` : "접어둠", "warn"] as const) : STATE[c.state];
          const inherited = levelOf(c.deck_path);
          const on = selected.has(c.id);
          return (
            <div
              key={c.id}
              className={`cards__row ${on ? "is-selected" : ""} ${rowMenu === c.id ? "is-menu" : ""}`}
              onDoubleClick={(e) => {
                if (selecting) return;
                // 행은 초점을 받지 않는다. 늘 보이는 ⋯ 를 연 곳으로 삼아 닫은 뒤 초점이 body 로 빠지지 않게 (QA V4-08 ④ · V9-07)
                e.currentTarget.querySelector<HTMLElement>(".cards__more")?.focus();
                setEditing(c);
              }}
            >
              <span className="cards__check">
                <button
                  role="checkbox"
                  aria-checked={on}
                  aria-label="카드 선택"
                  className={`cards__cb ${on ? "is-on" : ""}`}
                  onMouseDown={(e) => e.shiftKey && e.preventDefault()} // Shift 클릭이 글자를 긁지 않게
                  onClick={(e) => {
                    e.stopPropagation();
                    closeMenus();
                    toggle(i, e);
                  }}
                >
                  {on && <Icon name="check" size={14} />}
                </button>
              </span>
              {c.occlusion ? (
                <span>
                  <OcclusionRow noteId={c.note_id} pos={c.occlusion} question={c.question} />
                </span>
              ) : (
                <span className="t-body-m">
                  {c.sounds[0] && <Snd />}
                  {c.question || (c.sounds[0] ? "" : "(내용 없음)")}
                </span>
              )}
              <span className="t-body-s c-secondary">
                {c.sounds[1] && <Snd />}
                {c.answer}
              </span>
              <span className="t-body-s c-secondary">{relPath(c.deck_path)}</span>
              <span className="cards__imp menu">
                <button
                  aria-label={`중요도 ${LEVEL_LABEL[c.importance]} · 바꾸기`}
                  aria-haspopup="menu"
                  aria-expanded={impMenu === c.id}
                  onClick={(e) => {
                    e.stopPropagation();
                    lastTrigger.current = e.currentTarget;
                    closeMenus();
                    setImpMenu(impMenu === c.id ? null : c.id);
                  }}
                >
                  <Importance level={c.importance} />
                </button>
                {c.own_importance && <span className="t-caption c-accent">직접 지정</span>}
                {impMenu === c.id && (
                  <ImportanceMenu
                    cap="이 카드만 바꿔요"
                    current={c.importance}
                    canFollow={c.own_importance}
                    follow={`${relPath(c.deck_path)}${inherited !== undefined ? ` → ${LEVEL_LABEL[inherited]}` : ""}`}
                    onPick={(l) => pick(c, l)}
                  />
                )}
              </span>
              {c.front_empty ? (
                // NT49 앞면이 빈 카드는 상태와 상관없이 학습에 나오지 않는다
                <span title="앞면이 비어 학습에 나오지 않아요 · 필드를 채우면 다시 나와요">
                  <Badge tone="warn">앞면 빔</Badge>
                </span>
              ) : (
                <span title={day ? (day === "오늘" ? "오늘 복습으로 돌아와요" : `${day}에 복습으로 돌아와요`) : undefined}>
                  <Badge tone={tone}>{label}</Badge>
                </span>
              )}
              <span className="cards__actions">
                <span className="menu">
                  <button
                    className="cards__more"
                    aria-label="카드 메뉴"
                    title="카드 메뉴"
                    aria-haspopup="menu"
                    aria-expanded={rowMenu === c.id}
                    onClick={(e) => {
                      e.stopPropagation();
                      lastTrigger.current = e.currentTarget;
                      const open = rowMenu === c.id;
                      closeMenus();
                      if (!open) setRowMenu(c.id);
                    }}
                  >
                    <Icon name="more" size={16} />
                  </button>
                  {rowMenu === c.id && (
                    <div
                      className="menu__list menu--panel cards__row-menu"
                      role="menu"
                      aria-label="카드 메뉴"
                      onKeyDown={menuKeys}
                      onClick={(e) => e.stopPropagation()}
                    >
                      <button
                        role="menuitem"
                        className="menu__item t-body-m"
                        autoFocus
                        onMouseEnter={() => setRowMove(false)}
                        onClick={() => (closeMenus(), setEditing(c))}
                      >
                        <Icon name="pencil" size={16} scaled />
                        <span className="menu__text">편집</span>
                        <Kbd k="E" />
                      </button>
                      {canMove && (
                        <div className="cards__move">
                          <button
                            ref={moveTrigger}
                            role="menuitem"
                            aria-haspopup="dialog"
                            aria-expanded={rowMove}
                            className="menu__item t-body-m"
                            onMouseEnter={() => setRowMove(true)}
                            onClick={() => setRowMove(true)}
                            onKeyDown={(e) => e.key === "ArrowRight" && (e.preventDefault(), setRowMove(true))}
                          >
                            <Icon name="layers" size={16} scaled />
                            <span className="menu__text">다른 하위 덱으로 이동</span>
                            <Icon name="chevron-right" size={14} className="c-tertiary" />
                          </button>
                          {rowMove && (
                            <DeckMenu
                              className="cards__move-menu"
                              rootDeckId={deckId}
                              value={deckIdOf.get(key(rel(c.deck_path))) ?? null}
                              choices={choices}
                              onClose={() => {
                                setRowMove(false);
                                moveTrigger.current?.focus();
                              }}
                              onPick={(to) => move([c], to)}
                            />
                          )}
                        </div>
                      )}
                      {c.state === "suspended" && (
                        <button role="menuitem" className="menu__item menu__item--top" onMouseEnter={() => setRowMove(false)} onClick={() => unsuspend([c])}>
                          <Icon name="play" size={16} scaled />
                          <span className="menu__text">
                            <span className="t-body-m">학습 중지 풀기</span>
                            <span className="t-caption c-tertiary">다음에 시작하는 학습부터 포함</span>
                          </span>
                        </button>
                      )}
                      {/* 학습 중지 카드는 학습에 나오지 않아 접어두기가 없다 (접어두기 풀기는 그대로) */}
                      {c.buried ? (
                        <button role="menuitem" className="menu__item menu__item--top" onMouseEnter={() => setRowMove(false)} onClick={() => unbury(c)}>
                          <Icon name="rotate" size={16} scaled />
                          <span className="menu__text">
                            <span className="t-body-m">접어두기 풀기</span>
                            <span className="t-caption c-tertiary">다음에 시작하는 학습부터 포함</span>
                          </span>
                        </button>
                      ) : c.state === "suspended" ? null : (
                        <button role="menuitem" className="menu__item menu__item--top" onMouseEnter={() => setRowMove(false)} onClick={() => bury([c])}>
                          <Icon name="archive" size={16} scaled />
                          <span className="menu__text">
                            <span className="t-body-m">접어두기</span>
                            <span className="t-caption c-tertiary">{buryReturnDay()}부터 다시 포함</span>
                          </span>
                        </button>
                      )}
                      {c.exam.hits !== null && (
                        <button role="menuitem" className="menu__item menu__item--top" onMouseEnter={() => setRowMove(false)} onClick={() => clearExam(c)}>
                          <Icon name="target" size={16} scaled />
                          <span className="menu__text">
                            <span className="t-body-m">기출 표시 빼기</span>
                            <span className="t-caption c-tertiary">{c.exam.names.length ? c.exam.names.join(" · ") : `기출 ${n(c.exam.hits)}회`}</span>
                          </span>
                        </button>
                      )}
                      <div className="menu__sep" role="separator" />
                      <button
                        role="menuitem"
                        className="menu__item menu__item--danger t-body-m"
                        onMouseEnter={() => setRowMove(false)}
                        onClick={() => (closeMenus(), setDeleting(c))}
                      >
                        <Icon name="trash" size={16} />
                        <span className="menu__text">삭제</span>
                        <Kbd k="Del" />
                      </button>
                    </div>
                  )}
                </span>
                <button className="cards__act" aria-label="편집" title="편집" onClick={() => setEditing(c)}>
                  <Icon name="pencil" size={16} />
                </button>
                <button className="cards__act c-danger" aria-label="삭제" title="삭제" onClick={() => setDeleting(c)}>
                  <Icon name="trash" size={16} />
                </button>
              </span>
            </div>
          );
        })}
        {list && rows.length === 0 && <p className="cards__empty t-body-m c-tertiary">{query ? "찾는 카드가 없어요." : "카드가 없어요."}</p>}
        <div ref={foot} className="cards__foot t-caption c-tertiary">
          <span>{rows.length > 0 && list ? `1–${n(rows.length)} / ${n(list.total)}` : ""}</span>
          <span>{more ? "스크롤하면 더 불러와요" : ""}</span>
        </div>
      </div>

      {editing !== null && (
        <EditNote
          noteId={editing.note_id}
          card={editing}
          back={{ name: "cards", deckId, buriedOnly, cardIds: only ?? undefined, weakOnly: weak, edit: editing.id, query }}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            loadFixable();
            load();
          }}
          onDeleted={() => {
            setEditing(null);
            toast({ title: "카드를 지웠어요" });
            loadFixable();
            load();
          }}
        />
      )}
      {deleting && (
        // 카드 한 장은 '삭제' 입력 없이 한 번만 묻는다 (데이터 전체 삭제 E1-b 와 무게를 다르게)
        <Modal title="이 카드를 지울까요?" onClose={() => setDeleting(null)} variant="confirm" icon="trash" className="cards__delete">
          <div className="cards__preview">
            <p className="t-body-m">{deleting.question || "(내용 없음)"}</p>
            {/* 답이 길어도 위치는 보이게 둘로 나눈다 */}
            <p className="cards__preview-meta t-caption c-tertiary">
              {deleting.answer && <span className="cards__preview-answer">{deleting.answer}</span>}
              <span>
                {deleting.answer && "· "}
                {relPath(deleting.deck_path)}
              </span>
            </p>
          </div>
          <p className="t-body-s c-secondary">되돌릴 수 없어요. 이 카드의 복습 기록은 학습 통계에만 남아요.</p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setDeleting(null)} />
            {/* 열리면 삭제 버튼에 초점이 있어 Enter 가 곧 삭제다. Tab 으로 취소에 옮기면 Enter 는 취소 */}
            <Button variant="danger" size="M" icon="trash" label="삭제" shortcut="Enter" autoFocus onClick={() => remove(deleting)} />
          </div>
        </Modal>
      )}
      {examFor && (
        <ExamDialog
          deckId={deckId}
          cards={examFor.length}
          notes={[...new Map(examFor.map((c) => [c.note_id, c.exam])).values()]}
          onClose={() => setExamFor(null)}
          onConfirm={markExam}
        />
      )}
      {changing && (
        <ChangeTypeDialog
          noteIds={changing}
          onClose={() => setChanging(null)}
          onDone={(to) => {
            const count = changing.length;
            setChanging(null);
            endSelection();
            act(Promise.resolve().then(() => toast({ title: `노트 ${n(count)}개를 「${to}」${ro(to)} 옮겼어요`, text: "옮기기 전 백업은 설정 › 이 기기 백업에 있어요" })));
          }}
        />
      )}
      {bulkDelete && (
        <Modal title={`${n(picked.length)}장을 지울까요?`} onClose={() => setBulkDelete(false)} variant="confirm" icon="trash" className="cards__delete">
          <div className="cards__preview">
            {picked.slice(0, 3).map((c) => (
              <p key={c.id} className="cards__preview-line t-body-m">
                {c.question || "(내용 없음)"}
              </p>
            ))}
            {picked.length > 3 && <p className="t-caption c-tertiary">외 {n(picked.length - 3)}장</p>}
          </div>
          <p className="t-body-s c-secondary">되돌릴 수 없어요. 이 카드들의 복습 기록은 학습 통계에만 남아요.</p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setBulkDelete(false)} />
            <Button variant="danger" size="M" icon="trash" label={`${n(picked.length)}장 삭제`} shortcut="Enter" autoFocus onClick={() => removeMany(picked)} />
          </div>
        </Modal>
      )}
      {newSub && deckId !== null && <NewDeckModal parentId={deckId} onClose={() => setNewSub(false)} />}
    </main>
  );
}

/** C2-m 중요도 메뉴. 행의 칩(한 장)과 선택 바(여러 장)가 같이 쓴다 */
function ImportanceMenu({ cap, current, canFollow, follow, onPick, className = "" }: {
  cap: string;
  /** ✓ 를 붙일 값. 여러 장이 서로 다르면 없음 */
  current?: Level;
  canFollow: boolean;
  /** "하위 덱 설정 따르기" 아래 캡션 */
  follow: string;
  onPick: (level: Level | null) => void;
  className?: string;
}) {
  return (
    <div
      className={`menu__list menu--panel cards__imp-list ${className}`}
      role="menu"
      aria-label={cap}
      onKeyDown={menuKeys}
      onClick={(e) => e.stopPropagation()}
    >
      <p className="menu__cap t-caption c-tertiary" aria-hidden>
        {cap}
      </p>
      {LEVELS.map((lvl, i) => (
        <button
          key={lvl}
          role="menuitem"
          aria-current={current === lvl || undefined}
          className="menu__item t-body-m"
          autoFocus={i === 0}
          onClick={() => onPick(lvl)}
        >
          <span className={`dot ${DOT[lvl]}`} />
          <span className="cards__imp-label">{LEVEL_LABEL[lvl]}</span>
          {current === lvl && <Icon name="check" size={16} className="c-accent" />}
          <Kbd k={String(i + 1)} />
        </button>
      ))}
      <div className="menu__sep" role="separator" />
      {/* 개별 지정한 카드에서만 켜진다. 어디서 무엇을 물려받는지 캡션으로 */}
      <button role="menuitem" className="menu__item cards__imp-follow" disabled={!canFollow} onClick={() => onPick(null)}>
        <Icon name="rotate" size={16} scaled />
        <span className="menu__text">
          <span className="t-body-m">하위 덱 설정 따르기</span>
          <span className="t-caption c-tertiary">{follow}</span>
        </span>
      </button>
    </div>
  );
}
