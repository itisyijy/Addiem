// I3 자료 기반 중요도 제안 (Figma 53:6875, 노트 53:7256). 중요도 화면(B4/B4-g) 위의 모델 배너와 칩의 출처 태그.
// [내려받기]는 앱의 유일한 외부 연결이라 사용자가 누를 때만 받는다. 받지 않아도 지금까지의 중요도 기능은 그대로
import "../styles/sources.css";
import { useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText, type BoardDeck } from "../api";
import { Button, Icon } from "../ui";
import { n } from "../nav";
import { mb, refreshModel, sourceApi, useModel } from "../sourceApi";
import { AnalyzeBar, useAnalyzeProgress } from "./Sources";

// [나중에]는 앱을 닫을 때까지 (다음에 열면 다시 묻는다)
let later = false;

/**
 * board: 중요도 화면이 이미 불러온 하위 덱 (자료 제안 수를 센다).
 * onChanged: 분석으로 제안이 바뀌면 화면이 다시 불러온다. onShow: [보기] (자료 제안이 보이게)
 */
export function SourceBanner({ deckId, board, onChanged, onShow }: { deckId: number; board: BoardDeck[]; onChanged: () => void; onShow: () => void }) {
  const model = useModel();
  const [hasMaterial, setHasMaterial] = useState(false);
  const [dl, setDl] = useState<{ done: number; total: number } | null>(null);
  const [progress, setProgress] = useAnalyzeProgress(model?.analyzing);
  const [hidden, setHidden] = useState(later);
  const [error, setError] = useState<string>();
  const [result, setResult] = useState<number | null>(null);

  useEffect(() => {
    sourceApi
      .list(deckId)
      .then((m) => setHasMaterial(m.some((x) => x.status === "ready")))
      .catch(() => {});
  }, [deckId]);
  useEffect(() => {
    const off = listen<{ done: number; total: number }>("model-progress", (e) => setDl(e.payload));
    return () => {
      off.then((f) => f());
    };
  }, []);

  const analyze = async () => {
    setError(undefined);
    try {
      const r = await sourceApi.analyze(deckId);
      setResult(r.suggested);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    }
    setProgress(null);
  };

  const download = async () => {
    setError(undefined);
    setDl({ done: 0, total: model?.bytes ?? 0 });
    refreshModel();
    try {
      await sourceApi.downloadModel();
      await refreshModel();
      setDl(null);
      analyze(); // 받았으면 바로 이 덱 자료로 제안을 만든다
    } catch (e) {
      setDl(null);
      setError(errorText(e));
      refreshModel();
    }
  };

  if (!model || !hasMaterial || hidden) return null;
  const material = board.filter((d) => d.source === "material").length;

  let body: ReactNode;
  if (dl || model.downloading) {
    const d = dl ?? { done: 0, total: model.bytes };
    body = (
      <>
        <BannerText title="분석 모델을 내려받고 있어요" sub={`${mb(d.done)} / ${mb(d.total)} · 다 받으면 이 기기 안에서만 분석해요`}>
          <span className="src-progress__track">
            <span className="src-progress__fill" style={{ width: `${d.total ? Math.round((d.done / d.total) * 100) : 0}%` }} />
          </span>
        </BannerText>
        <Button variant="ghost" size="M" label="멈추기" onClick={() => sourceApi.cancelDownload()} />
      </>
    );
  } else if (!model.ready) {
    body = (
      <>
        <BannerText
          title="자료로 중요도를 제안하려면 분석 모델이 필요해요"
          sub={`약 ${mb(model.bytes)} · 한 번 내려받으면 인터넷 없이 동작해요. 분석하려고 자료와 카드를 밖으로 보내지 않아요.`}
        />
        <Button
          variant="ghost"
          size="M"
          label="나중에"
          onClick={() => {
            later = true;
            setHidden(true);
          }}
        />
        <Button size="M" icon="download" label="내려받기" onClick={download} />
      </>
    );
  } else if (progress) {
    body = (
      <BannerText title="자료와 카드를 잇고 있어요" sub="처음 분석은 자료마다 수십 초 걸려요">
        <AnalyzeBar p={progress} />
      </BannerText>
    );
  } else if (material > 0) {
    body = (
      <>
        <BannerText title={`자료로 제안 ${n(material)}개가 생겼어요`} sub="노란 '자료' 태그가 붙은 칩이에요. 마우스를 올리면 근거가 보여요" />
        <Button size="M" label="보기" onClick={onShow} />
      </>
    );
  } else {
    body = (
      <>
        <BannerText
          title={result === 0 ? "자료로 제안할 하위 덱을 찾지 못했어요" : "자료로 중요도 제안을 만들 수 있어요"}
          sub={
            result === 0
              ? "자료와 이어진 하위 덱이 3개보다 적거나, 이미 정해 둔 덱이에요"
              : "이 기기 안에서 자료의 강조 구간과 카드를 이어 하위 덱 순위를 매겨요"
          }
        />
        <Button size="M" variant={result === 0 ? "ghost" : "primary"} label={result === 0 ? "다시 분석" : "분석하기"} onClick={analyze} />
      </>
    );
  }
  return (
    <div className="src-banner" role="region" aria-label="자료 분석">
      <span className="src-banner__icon">
        <Icon name="download" size={18} className="c-accent" />
      </span>
      {body}
      {error && <span className="t-caption c-danger src-banner__error">{error}</span>}
    </div>
  );
}

function BannerText({ title, sub, children }: { title: string; sub: string; children?: ReactNode }) {
  return (
    <span className="src-banner__text">
      <span className="t-body-m-strong c-accent">{title}</span>
      <span className="t-body-s c-secondary">{sub}</span>
      {children}
    </span>
  );
}

const TAG: Record<string, string> = { material: "자료", tag: "태그", rule: "이름 규칙" };

/** 칩의 출처 태그 (I3): 자료(노랑) / 태그 / 이름 규칙. 모델이 있을 때만 붙인다 */
export function SourceTag({ source }: { source: string }) {
  const model = useModel();
  if (!TAG[source] || (source !== "material" && !model?.ready)) return null;
  return <span className={`src-tag src-tag--${source} t-label-s`}>{TAG[source]}</span>;
}
