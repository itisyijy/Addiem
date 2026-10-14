// I2 학습 중 원자료 보기 (Figma 53:6730, 노트 53:6869). 카드 오른쪽 420px 패널: 연결 구간을 앞뒤 문단과 함께,
// 신호 배지와 연결 방식, 쪽 축소 미리보기, [자료에서 열기] [연결 바꾸기].
// 학습 화면(Study.tsx)이 답을 본 뒤(D3)에만 연다. [원자료] 버튼은 useCardSource(cardId) 가 null 이 아닐 때만
import "../styles/sources.css";
import { Button, Icon } from "../ui";
import { n, useNav } from "../nav";
import { useCardSource } from "../sourceApi";
import { PdfPage, SignalBadge, SlidePage } from "./Sources";

export function SourcePanel({ cardId, onClose }: { cardId: number; onClose: () => void }) {
  const nav = useNav();
  const src = useCardSource(cardId);
  if (!src) return null;
  const { material: m, segment: s, method } = src;
  // [연결 바꾸기]는 그 구간의 카드 잇기 칸을 연 채로 들어간다
  const open = (link: boolean) => nav({ name: "sources", deckId: m.deck_id, materialId: m.id, segmentId: s.id, link });
  return (
    <aside className="src-panel" aria-label="원자료">
      <div className="src-panel__head">
        <span className="src-panel__title">
          <span className="t-title-s">원자료</span>
          <span className="t-caption c-tertiary">
            {m.file_name} · {n(s.page)}
            {m.kind === "pptx" ? "번째 슬라이드" : "쪽"}
          </span>
        </span>
        <button className="src-icon-btn" aria-label="원자료 닫기" onClick={onClose}>
          <Icon name="x" size={18} />
        </button>
      </div>
      <div className="src-panel__text">
        {s.before && <p className="t-body-s c-secondary">{s.before}</p>}
        <p className="src-panel__hit t-body-s c-primary">{s.text}</p>
        {s.after && <p className="t-body-s c-secondary">{s.after}</p>}
      </div>
      <span className="src-panel__meta">
        <SignalBadge signal={s.signals[0] ?? "bold"} />
        <span className="t-caption c-tertiary">이 카드와 연결된 구간 · {method === "manual" ? "직접 연결함" : "자동으로 연결됨"}</span>
      </span>
      {m.kind === "pdf" ? (
        <PdfPage materialId={m.id} page={s.page} width={376} height={180} segs={[s]} active={s.id} />
      ) : (
        <SlidePage materialId={m.id} page={s.page} active={s.text} height={180} />
      )}
      <span className="src-panel__actions">
        <Button variant="ghost" size="M" icon="file" label="자료에서 열기" onClick={() => open(false)} />
        <Button variant="ghost" size="M" icon="pencil" label="연결 바꾸기" onClick={() => open(true)} />
      </span>
    </aside>
  );
}
