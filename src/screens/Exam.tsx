// 8.4 기출 표시 창 (N2 여러 장 · I1 자료 구간: 시험 고르기 · 새 시험) · N1 카드 편집의 기출 줄 · N4 덱 상세 한 줄. 사실만 보인다 (점수 없음)
import { useEffect, useRef, useState } from "react";
import type { ExamState } from "../api";
import { Button, Icon } from "../ui";
import { n, useNav } from "../nav";
import { exam } from "../written";
import { Modal } from "./NewDeck";
import "../styles/study.css";

/** 시험 하나를 고르거나 새로 적는다. 같은 시험이 이미 있는 노트는 건너뛴다 (표시는 노트마다 한 번) */
export function ExamDialog({ deckId, notes, cards, title, sub, unit = "노트", onClose, onConfirm }: {
  /** null 이면 모든 덱의 시험 */
  deckId: number | null;
  /** 고른 카드의 노트 (노트마다 하나) */
  notes: ExamState[];
  cards: number;
  /** 자료 구간(I1)처럼 카드가 아닌 것에 표시할 때 */
  title?: string;
  sub?: string;
  /** 버튼 · 건너뛰기 안내의 단위 */
  unit?: string;
  onClose: () => void;
  /** name 을 더할 노트 (건너뛸 노트는 뺐다) */
  onConfirm: (name: string, todo: ExamState[]) => void;
}) {
  const [list, setList] = useState<{ name: string; cards: number }[]>([]);
  const [pick, setPick] = useState<string | null>(null);
  const [fresh, setFresh] = useState("");
  useEffect(() => {
    exam.list(deckId).then((l) => {
      setList(l);
      setPick((p) => p ?? l[0]?.name ?? null);
    }, () => {});
  }, [deckId]);
  const name = (pick ?? fresh).trim();
  const todo = notes.filter((s) => !s.names.includes(name));
  const skipped = notes.length - todo.length;
  const ok = !!name && todo.length > 0;

  return (
    <Modal
      title={title ?? `카드 ${n(cards)}장에 기출 표시`}
      sub={sub ?? `고른 카드의 노트 ${n(notes.length)}개에 같은 시험을 더해요.`}
      onClose={onClose}
      className="exam-dialog"
    >
      <div className="exam__list" role="radiogroup" aria-label="시험">
        {list.map((e) => (
          <button key={e.name} role="radio" aria-checked={pick === e.name} className={`exam__item ${pick === e.name ? "is-selected" : ""}`} onClick={() => setPick(e.name)}>
            <span className="exam__radio" />
            <span className="exam__text">
              <span className="t-body-m-strong">{e.name}</span>
              <span className="t-caption c-tertiary">이 덱 카드 {n(e.cards)}장에 표시됨</span>
            </span>
          </button>
        ))}
        <label className={`exam__item exam__new ${pick === null ? "is-selected" : ""}`}>
          <Icon name="plus" size={16} className="c-tertiary" />
          <input
            className="t-body-m"
            placeholder="새 시험 이름 (예: 2024년 필기 2회)"
            value={fresh}
            autoFocus={list.length === 0}
            onFocus={() => setPick(null)}
            onChange={(e) => setFresh(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && ok && onConfirm(name, todo)}
          />
        </label>
      </div>
      {skipped > 0 && name && (
        <p className="exam__note t-caption c-secondary">
          <Icon name="info" size={14} />
          {n(skipped)}개 {unit}는 이미 "{name}"이 있어서 건너뛰어요. 표시는 {unit}마다 한 번이에요.
        </p>
      )}
      <div className="modal__actions">
        <Button variant="ghost" label="취소" onClick={onClose} />
        <Button icon="target" label={notes.length === 1 ? "표시" : `${n(todo.length)}개 ${unit}에 표시`} disabled={!ok} onClick={() => onConfirm(name, todo)} />
      </div>
    </Modal>
  );
}

/** 9: 본문 · 자료 글자의 기출 흔적. 있으면 점선 "제안" 칩 (눌러야 확정, N2) */
export function examHint(text: string): string | null {
  return /\(기출\)|기출\s*문제/.exec(text)?.[0] ?? null;
}

/** 글자 속 연도 (exam.rs year 와 같다). "마지막" 시험을 고른다 */
const yearOf = (name: string) => Number(/\d{4}/.exec(name)?.[0] ?? 0);

/** 이름 없이 표시한 횟수는 hits − 이름 수 */
const unnamed = (v: ExamState) => Math.max(0, (v.hits ?? 0) - v.names.length);

/**
 * N1 카드 편집의 기출 줄: 시험 칩(×) · 이름 없는 표시 · 본문 제안 칩 · [+ 기출 추가] 팝오버(이 덱에서 쓴 시험 체크 · 새 시험).
 * 바꾼 값은 저장할 때 쓴다 (onChange 는 화면 값만)
 */
export function ExamField({ deckId, value, onChange, hint }: { deckId: number | null; value: ExamState; onChange: (v: ExamState) => void; hint: string | null }) {
  const [open, setOpen] = useState(false);
  const set = (names: string[], extra = unnamed(value)) => onChange({ ...value, names, hits: names.length + extra || null });
  const last = [...value.names].sort((a, b) => yearOf(b) - yearOf(a))[0];
  const facts = value.hits ? [`기출 ${n(value.hits)}회`, last && `마지막 ${last}`].filter(Boolean).join(" · ") : "표시 없음";
  return (
    <div className="exam-field">
      <span className="exam-field__head">
        <span className="t-label-m">기출</span>
        <span className="t-caption c-tertiary">{facts}</span>
      </span>
      <div className="exam-field__chips">
        {value.names.map((name) => (
          <span key={name} className="exam-chip t-label-s">
            <span className="exam-chip__dot" />
            {name}
            <button className="exam-chip__x" aria-label={`${name} 빼기`} onClick={() => set(value.names.filter((x) => x !== name))}>
              <Icon name="x" size={12} />
            </button>
          </span>
        ))}
        {unnamed(value) > 0 && (
          <span className="exam-chip t-label-s">
            <span className="exam-chip__dot" />
            시험 이름 없이 {n(unnamed(value))}회
            <button className="exam-chip__x" aria-label="이름 없는 기출 빼기" onClick={() => set(value.names, 0)}>
              <Icon name="x" size={12} />
            </button>
          </span>
        )}
        {hint && !value.hits && (
          <button className="exam-chip exam-chip--suggest t-label-s" onClick={() => set([], 1)}>
            <span className="exam-chip__dot" />
            글자에 "{hint}" · 제안
            <Icon name="plus" size={12} />
          </button>
        )}
        <span className="exam-add">
          <button className="exam-add__btn t-label-s" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
            <Icon name="plus" size={12} />
            기출 추가
          </button>
          {open && <ExamPop deckId={deckId} names={value.names} onChange={(names) => set(names)} onClose={() => setOpen(false)} />}
        </span>
      </div>
    </div>
  );
}

/** N1 팝오버: 어느 시험에 나왔나요? 체크하면 바로 칩에 더한다 */
function ExamPop({ deckId, names, onChange, onClose }: { deckId: number | null; names: string[]; onChange: (names: string[]) => void; onClose: () => void }) {
  const [list, setList] = useState<string[]>([]);
  const [fresh, setFresh] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const [had] = useState(names); // 열 때 이미 있던 시험 ("이미 표시")
  // 편집 창(Modal) 안에 뜬다. 창의 Esc 보다 먼저 받아 팝오버만 닫는다 (창 Esc 는 defaultPrevented 면 쉰다)
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", k, true);
    return () => window.removeEventListener("keydown", k, true);
  }, [onClose]);
  useEffect(() => {
    exam.list(deckId).then((l) => setList(l.map((e) => e.name)), () => {});
  }, [deckId]);
  useEffect(() => {
    const close = (e: MouseEvent) => !box.current?.parentElement?.contains(e.target as Node) && onClose();
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [onClose]);
  const shown = [...new Set([...list, ...names])];
  const add = () => {
    const name = fresh.trim();
    if (name && !names.includes(name)) onChange([...names, name]);
    setFresh("");
  };
  return (
    <div ref={box} className="exam-pop" role="dialog" aria-label="어느 시험에 나왔나요?">
      <p className="t-label-m">어느 시험에 나왔나요?</p>
      {shown.map((name) => (
        <label key={name} className="exam-pop__item">
          <input type="checkbox" checked={names.includes(name)} onChange={(e) => onChange(e.target.checked ? [...names, name] : names.filter((x) => x !== name))} />
          <span className="t-body-s">{name}</span>
          <span className="t-caption c-tertiary">{had.includes(name) ? "이미 표시" : list.includes(name) ? "이 덱에서 쓴 시험" : "새 시험"}</span>
        </label>
      ))}
      <span className="exam-pop__new">
        <Icon name="plus" size={14} className="c-tertiary" />
        <input
          className="t-body-s"
          placeholder="새 시험 · 예: 2025년 필기 1회"
          value={fresh}
          autoFocus={shown.length === 0}
          onChange={(e) => setFresh(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              e.stopPropagation();
              add();
            }
          }}
          onBlur={add}
        />
      </span>
      <p className="t-caption c-tertiary">횟수는 표시한 시험 수로 세요. 같은 시험을 두 번 세지 않아요.</p>
    </div>
  );
}

/** N4 덱 상세 한 줄 (중요도 줄 아래). 표시가 없으면 그리지 않는다 (홈에는 넣지 않는다). 시험 이름은 최근 3개까지 */
export function ExamLine({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [s, setS] = useState<Awaited<ReturnType<typeof exam.summary>>>();
  const [names, setNames] = useState<string[]>([]);
  useEffect(() => {
    exam.summary(deckId).then(setS, () => {});
    exam.list(deckId).then((l) => setNames(l.map((e) => e.name)), () => {});
  }, [deckId]);
  if (!s || s.cards === 0) return null;
  const exams = s.exams > 0 && `시험 ${n(s.exams)}개${names.length ? ` (${names.slice(0, 3).join(" · ")}${names.length > 3 ? " 외" : ""})` : ""}`;
  const facts = [s.twice > 0 && `2회 이상 ${n(s.twice)}장`, exams, s.unseen > 0 && `아직 안 본 기출 카드 ${n(s.unseen)}장`].filter(Boolean);
  return (
    <section className="exam-line">
      <span className="exam-line__icon">
        <Icon name="target" size={18} />
      </span>
      <span className="exam-line__text">
        <span className="t-body-m-strong">기출 표시 카드 {n(s.cards)}장</span>
        {facts.length > 0 && <span className="t-caption c-tertiary">{facts.join(" · ")}</span>}
      </span>
      <Button variant="ghost" size="M" icon="chevron-right" label="기출 카드만 보기" onClick={() => nav({ name: "cards", deckId, cardIds: s.card_ids })} />
    </section>
  );
}
