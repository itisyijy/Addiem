// 학습 화면 묶음의 작은 조각: J3 필수 키워드 칩 · H3 압축 회독 안내 줄 (홈 · D1) · G3 [나누기 제안] 칩 (D5)
import { useEffect, useRef, useState } from "react";
import { Button, Icon } from "../ui";
import { n, useNav } from "../nav";
import { cram, splitLater, type CramPlan, type Keyword } from "../written";
import "../styles/study.css";

// ---------------------------------------------------------------- J3 필수 키워드

/**
 * 확정 칩(회색, ✕ 로 빼기) + 제안 칩(점선, 누르면 확정) + [직접 추가].
 * 확정 칩을 누르면 "같은 뜻으로 인정할 말"(동의어)을 쉼표로 적는 칸이 열린다
 */
export function KeywordEditor({ keywords, suggestions, onChange, onDismiss }: {
  keywords: Keyword[];
  /** 모범 답안에서 뽑은 제안 (이미 확정했거나 뺀 말은 없다) */
  suggestions: string[];
  onChange: (k: Keyword[]) => void;
  /** 확정 칩을 뺐다. 같은 말을 다시 제안하지 않게 */
  onDismiss: (word: string) => void;
}) {
  const [active, setActive] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const addBox = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (adding) addBox.current?.focus();
  }, [adding]);

  const add = (word: string, source: string) => {
    const w = word.trim();
    if (w && !keywords.some((k) => k.word === w)) onChange([...keywords, { word: w, synonyms: [], source }]);
  };
  const remove = (i: number) => {
    onDismiss(keywords[i].word);
    onChange(keywords.filter((_, j) => j !== i));
    setActive(null);
  };
  const cur = active !== null ? keywords[active] : undefined;

  return (
    <div className="kw">
      <p className="kw__head">
        <span className="t-label-m">필수 키워드</span>
        <span className="t-caption c-tertiary">점선은 모범 답안에서 뽑은 제안 · 누르면 확정</span>
      </p>
      <div className="kw__chips">
        {keywords.map((k, i) => (
          <span key={k.word} className={`kw-chip ${i === active ? "is-active" : ""}`}>
            <button className="kw-chip__main t-label-s" aria-pressed={i === active} onClick={() => setActive(i === active ? null : i)}>
              {k.word}
              {k.synonyms.length > 0 && <span className="c-tertiary"> +{k.synonyms.length}</span>}
            </button>
            <button className="kw-chip__x" aria-label={`${k.word} 빼기`} onClick={() => remove(i)}>
              <Icon name="x" size={12} />
            </button>
          </span>
        ))}
        {suggestions.map((w) => (
          <button key={w} className="kw-chip kw-chip--suggested t-label-s" aria-label={`${w} 확정`} onClick={() => add(w, "suggested")}>
            {w}
            <Icon name="plus" size={12} />
          </button>
        ))}
        {adding ? (
          <input
            ref={addBox}
            className="kw-chip kw-chip--input t-label-s"
            placeholder="키워드"
            aria-label="키워드 직접 추가 · Enter 로 넣기"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                add(e.currentTarget.value, "user");
                e.currentTarget.value = "";
              } else if (e.key === "Escape") {
                e.preventDefault(); // 편집 창이 닫히지 않게. 입력 칸만 닫는다
                e.stopPropagation();
                setAdding(false);
              }
            }}
            onBlur={(e) => {
              add(e.currentTarget.value, "user");
              setAdding(false);
            }}
          />
        ) : (
          <button className="kw-chip kw-chip--add t-label-s" onClick={() => setAdding(true)}>
            <Icon name="plus" size={12} />
            직접 추가
          </button>
        )}
      </div>
      {cur ? (
        <label className="kw__syn">
          <span className="t-caption c-secondary">'{cur.word}' 대신 같은 뜻으로 인정할 말 · 쉼표로 나눠요</span>
          <input
            key={cur.word}
            className="field__input t-body-s"
            defaultValue={cur.synonyms.join(", ")}
            placeholder="VIF, 분산팽창계수"
            onChange={(e) =>
              onChange(keywords.map((k, i) => (i === active ? { ...k, synonyms: e.target.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean) } : k)))
            }
          />
        </label>
      ) : (
        <p className="t-caption c-tertiary">같은 뜻으로 인정할 말은 칩을 눌러 쉼표로: VIF, 분산팽창계수</p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- H3 압축 회독 안내 (홈 · D1)

/** 목표일 3일 전부터 "압축 회독을 할 수 있어요" 한 줄 + [압축 회독]. 열 수 없으면 아무것도 그리지 않는다 */
export function CramHint({ deckIds }: { deckIds: number[] | null }) {
  const nav = useNav();
  const [plan, setPlan] = useState<CramPlan | null>(null);
  useEffect(() => {
    cram.plan(deckIds).then(setPlan, () => setPlan(null));
  }, [deckIds]);
  if (!plan) return null;
  return (
    <div className="cram-hint">
      <Icon name="target" size={16} className="c-accent" />
      <span className="cram-hint__text">
        <span className="t-label-m">압축 회독을 할 수 있어요</span>
        <span className="t-caption c-tertiary">
          {plan.name} {plan.dday === 0 ? "시험이 오늘이에요" : `시험까지 ${n(plan.dday)}일`} · 불안한 카드 {n(plan.scopes[2])}장
        </span>
      </span>
      <Button variant="secondary" size="M" label="압축 회독" onClick={() => nav({ name: "summary", deckIds, cram: true })} />
    </div>
  );
}

// ---------------------------------------------------------------- G3 [나누기 제안] 칩 (D5 접어둔 카드 행)

/**
 * 학습 중 [나누기 제안]을 누른 카드에만 보인다 (이 기기의 "나중에 볼 제안" 목록).
 * onOpen 에서 G2 나누기 제안 창을 연다. 나누거나 그대로 두기로 정하면 splitLater.remove(cardId)
 */
export function SplitLaterChip({ cardId, onOpen }: { cardId: number; onOpen: () => void }) {
  if (!splitLater.has(cardId)) return null;
  return (
    <button className="split-chip t-label-s" onClick={onOpen}>
      <Icon name="layers" size={14} />
      나누기 제안
    </button>
  );
}
