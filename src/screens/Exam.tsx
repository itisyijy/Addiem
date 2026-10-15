// 8.4 기출 표시 창 (N1 한 장 · N2 여러 장: 시험 고르기 · 새 시험) · N4 덱 상세 한 줄. 사실만 보인다 (점수 없음)
import { useEffect, useState } from "react";
import type { ExamState } from "../api";
import { Button, Icon } from "../ui";
import { n, useNav } from "../nav";
import { exam } from "../written";
import { Modal } from "./NewDeck";
import "../styles/study.css";

/** 시험 하나를 고르거나 새로 적는다. 같은 시험이 이미 있는 노트는 건너뛴다 (표시는 노트마다 한 번) */
export function ExamDialog({ deckId, notes, cards, onClose, onConfirm }: {
  /** null 이면 모든 덱의 시험 */
  deckId: number | null;
  /** 고른 카드의 노트 (노트마다 하나) */
  notes: ExamState[];
  cards: number;
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
    <Modal title={`카드 ${n(cards)}장에 기출 표시`} sub={`고른 카드의 노트 ${n(notes.length)}개에 같은 시험을 더해요.`} onClose={onClose} className="exam-dialog">
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
          {n(skipped)}개 노트는 이미 "{name}"이 있어서 건너뛰어요. 표시는 노트마다 한 번이에요.
        </p>
      )}
      <div className="modal__actions">
        <Button variant="ghost" label="취소" onClick={onClose} />
        <Button icon="target" label={`${n(todo.length)}개 노트에 표시`} disabled={!ok} onClick={() => onConfirm(name, todo)} />
      </div>
    </Modal>
  );
}

/** N4 덱 상세 한 줄. 표시가 없으면 그리지 않는다 (홈에는 넣지 않는다) */
export function ExamLine({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [s, setS] = useState<Awaited<ReturnType<typeof exam.summary>>>();
  useEffect(() => {
    exam.summary(deckId).then(setS, () => {});
  }, [deckId]);
  if (!s || s.cards === 0) return null;
  const facts = [`기출 표시 카드 ${n(s.cards)}장`, s.twice > 0 && `2회 이상 ${n(s.twice)}장`, s.exams > 0 && `시험 ${n(s.exams)}개`, s.unseen > 0 && `아직 안 본 기출 ${n(s.unseen)}장`];
  return (
    <div className="exam-line">
      <Icon name="target" size={16} className="c-accent" />
      <span className="t-body-s c-secondary">{facts.filter(Boolean).join(" · ")}</span>
      <Button variant="ghost" size="M" label="기출 카드만 보기" onClick={() => nav({ name: "cards", deckId, cardIds: s.card_ids })} />
    </div>
  );
}
