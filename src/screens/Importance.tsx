// B4 중요도 정하기. 하위 덱을 세 칸으로 옮긴다. 하위 덱이 많으면 과목(경로 첫 단계)별로 묶는다 (B4-g)
import "../styles/board.css";
import { useCallback, useEffect, useRef, useState, type DragEvent, type ReactNode } from "react";
import { api, errorText, type BoardDeck, type DeckDetail, type Level } from "../api";
import { Badge, Button, Icon, Importance as ImportanceBadge, LEVEL_LABEL } from "../ui";
import { n, useKey, useNav, useOverlayScope } from "../nav";
import { menuKeys } from "./Deck";
import { SourceBanner, SourceTag, sourcesOf } from "./SourceBanner";

const LANES: Level[] = [2, 1, 0];
const GROUP_OVER = 12; // 하위 덱이 이보다 많으면 과목별로 묶어서 연다
const suggested = (d: BoardDeck) => d.source !== "user" && d.source !== "default";
const cardsOf = (ds: BoardDeck[]) => ds.reduce((s, d) => s + d.cards, 0);

/** 제안 근거 툴팁. 근거는 줄마다 하나 ("기출 · …", "자료 · …", "목차 · …"). 사실만 쓴다 (N3) */
function ReasonTip({ level, reason }: { level: Level; reason: string }) {
  const lines = reason.split("\n").filter(Boolean);
  return (
    <span className="board__tip" role="tooltip">
      <span className="t-label-s">
        왜 {LEVEL_LABEL[level]}인가요?{lines.length > 1 && ` · 근거 ${lines.length}가지`}
      </span>
      {lines.map((l) => (
        <span key={l} className="board__tip-line t-caption">
          {lines.length > 1 ? `· ${l}` : l}
        </span>
      ))}
    </span>
  );
}

export function Importance({ deckId, next = [] }: { deckId: number; next?: number[] }) {
  const nav = useNav();
  const [deck, setDeck] = useState<DeckDetail | null>(null);
  const [board, setBoard] = useState<BoardDeck[] | null>(null);
  const [dragging, setDragging] = useState<number[] | null>(null);
  const [over, setOver] = useState<Level | null>(null);
  const [menu, setMenu] = useState<number | null>(null);
  const [grouped, setGrouped] = useState<boolean>();
  const [open, setOpen] = useState<string | null>(null); // "줄:과목". 트레이는 한 번에 하나만
  const [error, setError] = useState<string>();

  const load = useCallback(
    () => api.importanceBoard(deckId).then((b) => setBoard(b.sort((x, y) => x.label.localeCompare(y.label, "ko", { numeric: true })))),
    [deckId],
  );
  // 불러오지 못하면 빈 화면 대신 이유를 보인다 (QA T3-14). 돌아갈 길과 [다시 시도]도 (QA V3-07)
  const [loadError, setLoadError] = useState<string>();
  const [tries, setTries] = useState(0);
  useEffect(() => {
    const fail = (e: unknown) => setLoadError(errorText(e));
    api.deckDetail(deckId).then(setDeck, fail);
    load().catch(fail);
  }, [deckId, load, tries]);

  const leave = useCallback(() => nav(next.length ? { name: "importance", deckId: next[0], next: next.slice(1) } : { name: "deck", deckId }), [nav, next, deckId]);
  // 칩 메뉴가 열려 있으면 Esc 는 메뉴만 닫고 초점을 칩으로 돌려준다
  const chipRef = useRef<HTMLButtonElement | null>(null);
  // 가져온 뒤 여러 덱을 차례로 정하는 중이면 Esc 는 다음 덱으로 (남은 덱을 건너뛰지 않게, QA T3-13)
  useKey("Escape", leave, menu === null);
  // 칩 메뉴가 떠 있는 동안 화면 단축키는 꺼지고 메뉴가 키를 받는다 (E4 노트)
  useOverlayScope(menu !== null);
  useKey(
    "Escape",
    () => {
      setMenu(null);
      chipRef.current?.focus();
    },
    menu !== null,
    { modal: true },
  );

  const move = async (ids: number[], level: Level) => {
    setBoard((b) => b && b.map((d) => (ids.includes(d.id) ? { ...d, importance: level, source: "user", reason: null } : d)));
    try {
      await Promise.all(ids.map((id) => api.setDeckImportance(id, level)));
    } catch (e) {
      setError(errorText(e));
      load();
    }
  };

  const confirm = async () => {
    try {
      await api.confirmImportance(board?.filter(suggested).map((d) => d.id) ?? []);
      leave();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const confirmAll = async () => {
    try {
      await api.confirmImportance(board?.filter(suggested).map((d) => d.id) ?? []);
      await load();
    } catch (e) {
      setError(errorText(e));
    }
  };

  // 뒤로 링크는 Esc 와 같다: 가져온 뒤 여러 덱을 차례로 정하는 중이면 다음 덱으로 (QA T3-13)
  const backLink = (
    <button className="backlink t-label-m c-secondary" onClick={leave}>
      <Icon name="chevron-left" size={18} />
      {next.length ? `다음 덱 · ${n(next.length)}개 남음` : (deck?.name ?? "덱")}
    </button>
  );
  if (!deck || !board)
    return loadError ? (
      <main className="board">
        {backLink}
        <p className="t-body-m c-danger" role="alert">{loadError}</p>
        <div>
          <Button
            variant="secondary"
            icon="rotate"
            label="다시 시도"
            onClick={() => {
              setLoadError(undefined);
              setTries((t) => t + 1);
            }}
          />
        </div>
      </main>
    ) : null;

  // 과목 = 경로 첫 단계. 덱 자신(빈 경로)은 "" 로 모여 칩 하나로 남는다
  const groups = new Map<string, BoardDeck[]>();
  for (const d of board) {
    const k = d.path[0] ?? "";
    groups.set(k, [...(groups.get(k) ?? []), d]);
  }
  const groupable = [...groups].some(([k, v]) => k && v.length > 1);
  const isGrouped = groupable && (grouped ?? board.length > GROUP_OVER);
  const pending = board.filter(suggested);

  const drag = (e: DragEvent, ids: number[]) => {
    e.dataTransfer.setData("text/plain", ids.join(","));
    setDragging(ids);
  };

  // 덱 자신에 바로 든 카드(빈 경로)는 하위 덱으로 오해하지 않게 따로 부른다
  const chip = (d: BoardDeck, label = d.path.length ? d.label : `${d.label} · 하위 덱 없는 카드`) => (
    <span key={d.id} className="board__chip-wrap menu">
      <button
        draggable
        className={`chip ${suggested(d) ? "chip--suggested" : ""}`}
        aria-label={`${label} · ${n(d.cards)}장 · ${LEVEL_LABEL[d.importance]}${suggested(d) ? " 제안" : ""} · 중요도 바꾸기`}
        aria-haspopup="menu"
        aria-expanded={menu === d.id}
        onDragStart={(e) => drag(e, [d.id])}
        onDragEnd={() => setDragging(null)}
        onClick={(e) => {
          e.stopPropagation();
          chipRef.current = e.currentTarget;
          setMenu(menu === d.id ? null : d.id);
        }}
      >
        <Icon name="grip" size={16} />
        <span className="t-label-m">{label}</span>
        <span className="t-caption c-tertiary">{n(d.cards)}장</span>
        {suggested(d) && sourcesOf(d.source, d.reason).map((s) => <SourceTag key={s} source={s} />) /* I3 · N3 출처 태그 */}
        {suggested(d) && <span className="t-label-s c-accent">제안</span>}
      </button>
      {suggested(d) && d.reason && menu !== d.id && <ReasonTip level={d.importance} reason={d.reason} />}
      {menu === d.id && (
        <div className="menu__list board__menu" role="menu" aria-label="중요도" onKeyDown={menuKeys} onClick={(e) => e.stopPropagation()}>
          {LANES.map((l, i) => (
            <button
              key={l}
              role="menuitem"
              aria-current={l === d.importance || undefined}
              autoFocus={i === 0}
              className="menu__item t-body-m"
              onClick={() => {
                setMenu(null);
                move([d.id], l);
              }}
            >
              {LEVEL_LABEL[l]}
              {l === d.importance && <Icon name="check" size={14} className="c-accent" />}
            </button>
          ))}
        </div>
      )}
    </span>
  );

  const group = (level: Level, name: string, all: BoardDeck[]): ReactNode[] => {
    const kids = all.filter((d) => d.importance === level);
    if (!kids.length) return [];
    if (!name || all.length === 1) return kids.map((d) => chip(d));
    const key = `${level}:${name}`;
    const split = kids.length < all.length;
    const sug = kids.some(suggested);
    const caption = `${split ? `하위 덱 ${all.length}개 중 ${LEVEL_LABEL[level]} ${kids.length}개` : `하위 덱 ${all.length}개`} · ${n(cardsOf(kids))}장`;
    const handle = {
      draggable: true,
      onDragStart: (e: DragEvent) => drag(e, all.map((d) => d.id)),
      onDragEnd: () => setDragging(null),
      onClick: () => setOpen(open === key ? null : key),
    };
    if (open === key)
      return [
        <div key={key} className="board__tray">
          <button className="board__tray-head" {...handle}>
            <Icon name="grip" size={16} className="c-tertiary" />
            <Icon name="chevron-down" size={16} scaled className="c-secondary" />
            <span className="t-label-m">{name}</span>
            <span className="t-caption c-tertiary">{caption}</span>
            {split && <Badge>중요도 혼합</Badge>}
            {sug && <span className="t-label-s c-accent">제안</span>}
          </button>
          <div className="board__tray-chips">{kids.map((d) => chip(d, d.path.slice(1).join(" · ") || d.label))}</div>
        </div>,
      ];
    return [
      // 끌기 손잡이는 하위 덱 칩(B4)처럼 맨 앞에. 과목 칩과 하위 덱 칩이 같은 자리를 잡고 끈다
      <button key={key} className={`board__group ${sug ? "board__group--suggested" : ""}`} {...handle}>
        <Icon name="grip" size={16} />
        <Icon name="chevron-right" size={16} scaled className="c-secondary" />
        <span className="board__group-text">
          <span className="t-label-m c-primary">{name}</span>
          <span className="t-caption">{caption}</span>
        </span>
        {[...new Set(kids.filter(suggested).flatMap((d) => sourcesOf(d.source, d.reason)))].filter((s) => s === "exam" || s === "material" || s === "outline").map((s) => (
          <SourceTag key={s} source={s} />
        ))}
        {sug && <span className="t-label-s c-accent">제안</span>}
        {split && <Badge>중요도 혼합</Badge>}
      </button>,
    ];
  };

  return (
    <main className="board" onClick={() => setMenu(null)}>
      {backLink}
      <div className="board__head">
        <div className="deck__title">
          {/* 여러 덱을 차례로 정하는 중이면 뒤로 링크가 다음 덱을 말하니, 지금 덱 이름은 제목에 (QA V9-02) */}
          <h1 className="t-title-l">{next.length ? `「${deck.name}」 중요도 정하기` : "중요도 정하기"}</h1>
          <p className="t-body-l c-secondary">
            태그와 하위 덱 이름으로 정한 중요도가 지금 학습에 적용돼요.
            <br />
            신규 카드는 핵심 → 일반 → 보충 순으로 배워요(보충도 학습 범위예요).
          </p>
        </div>
        <span className="board__legend">
          <span className="chip">
            <Icon name="grip" size={16} />
            <span className="t-label-m">확정</span>
          </span>
          <span className="chip chip--suggested">
            <Icon name="grip" size={16} />
            <span className="t-label-m">시스템 제안</span>
            <span className="t-label-s c-accent">제안</span>
          </span>
        </span>
      </div>

      {(groupable || pending.length > 0) && (
        <div className="board__bar">
          <span className="board__bar-left">
            {groupable && (
            <span className="segment segment--boxed">
              {[true, false].map((g) => (
                <button key={String(g)} className={`segment__item t-label-m ${isGrouped === g ? "is-selected" : ""}`} onClick={() => setGrouped(g)}>
                  {g ? "과목별로 묶기" : "하위 덱 모두 보기"}
                </button>
              ))}
            </span>
            )}
            <span className="t-body-s c-tertiary">
              하위 덱 {n(board.length)}개{pending.length > 0 && ` · 제안 ${n(pending.length)}개는 아직 확인 전`}
            </span>
          </span>
          {pending.length > 0 && <Button variant="ghost" size="M" icon="check" label="제안 모두 확정" onClick={confirmAll} />}
        </div>
      )}

      {/* I3 자료 기반 제안: 모델 배너 · 분석 진행 · 결과 (screens/SourceBanner.tsx) */}
      <SourceBanner deckId={deckId} board={board} onChanged={load} onShow={() => setGrouped(false)} />
      <div className="board__lanes">
        {LANES.map((level) => {
          const items = board.filter((d) => d.importance === level);
          const showDrop = items.length === 0 || (dragging?.some((id) => board.find((d) => d.id === id)?.importance !== level) ?? false);
          return (
            <section
              key={level}
              className={`board__lane board__lane--${level} ${over === level ? "is-over" : ""}`}
              onDragOver={(e) => {
                e.preventDefault();
                setOver(level);
              }}
              onDragLeave={() => setOver(null)}
              onDrop={(e) => {
                e.preventDefault();
                setOver(null);
                const ids = e.dataTransfer.getData("text/plain").split(",").map(Number).filter(Boolean);
                if (ids.length) move(ids, level);
              }}
            >
              <div className="board__lane-label">
                <ImportanceBadge level={level} />
                <span className="t-title-m">{n(cardsOf(items))}장</span>
              </div>
              <div className="board__chips">
                {isGrouped ? [...groups].flatMap(([name, all]) => group(level, name, all)) : items.map((d) => chip(d))}
                {showDrop && (
                  <span className={`board__drop t-label-m c-tertiary ${isGrouped ? "board__drop--group" : ""}`}>
                    {isGrouped ? "과목이나 하위 덱을 여기로 끌어 놓기" : "여기로 끌어 놓기"}
                  </span>
                )}
              </div>
            </section>
          );
        })}
      </div>

      <div className="board__foot">
        <span className="t-body-s c-tertiary">
          {isGrouped
            ? "과목을 옮기면 그 안의 하위 덱이 모두 옮겨져요 · 하나만 옮기려면 펼쳐서"
            : "정하지 않은 하위 덱은 일반으로 학습해요 · 카드 한 장만 바꾸려면 카드 목록에서"}
          {" · 확정한 제안은 자동 제안으로 덮어쓰지 않아요"}
        </span>
        <span className="import__foot-right">
          {error && <span className="t-body-s c-danger">{error}</span>}
          <Button variant="ghost" label="현재 중요도로 계속" onClick={leave} />
          <Button icon="check" label={next.length ? "제안 확정하고 다음 덱" : "제안 확정하고 덱 열기"} onClick={confirm} />
        </span>
      </div>
    </main>
  );
}
