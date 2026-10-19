// 8.3 서술형 개념 관계 확인 (로컬 모델, llm.rs). 설정 > 데이터의 [받기 · 지우기] 행과 J2 비교 화면의 결과 자리.
// 모델은 크기 · 남은 공간 · 라이선스 · 출처를 보고 사용자가 받을 때만 받는다. 없거나 실패해도 핵심어 검사와 평가는 그대로다.
// 결과는 비교 화면에 참고로만 보인다 (작은 모델이라 자주 틀려 추천은 바꾸지 않는다, DECISIONS J). 기다리는 동안에도 1–4 가 된다 (평가하면 확인을 멈추고 결과는 버린다)
import "../styles/concept.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import { Badge, Button, Icon, Spinner, type IconName } from "../ui";
import { Modal } from "./NewDeck";
import { llm, written as writtenApi, type Concept, type LlmStatus, type Relation } from "../written";

const gb = (bytes: number) => `${(bytes / 1e9).toFixed(1)}GB`;
const mb = (bytes: number) => `${Math.round(bytes / 1e6)}MB`;

// 상태는 설정 행과 학습 화면이 같이 본다. 받거나 지우면 refreshLlm()
let status: LlmStatus | null = null;
const subs = new Set<() => void>();
export function refreshLlm() {
  return llm.status().then(
    (s) => {
      status = s;
      subs.forEach((f) => f());
      return s;
    },
    () => null,
  );
}
export function useLlm() {
  const [, tick] = useState(0);
  useEffect(() => {
    const f = () => tick((x) => x + 1);
    subs.add(f);
    refreshLlm();
    return () => {
      subs.delete(f);
    };
  }, []);
  return status;
}

// ---------------------------------------------------------------- 설정 > 데이터 행

/** E1 데이터 칸의 [개념 관계 확인 모델] 행 (Settings.tsx 에서 한 줄로 끼운다) */
export function LlmRow() {
  const s = useLlm();
  const [asking, setAsking] = useState<"get" | "delete" | null>(null);
  const [dl, setDl] = useState<{ done: number; total: number } | null>(null);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const off = listen<{ done: number; total: number }>("llm-progress", (e) => setDl(e.payload));
    return () => {
      off.then((f) => f());
    };
  }, []);
  if (!s) return null;

  const download = async () => {
    setAsking(null);
    setError(undefined);
    setDl({ done: 0, total: s.bytes });
    try {
      await llm.download();
    } catch (e) {
      setError(errorText(e));
    }
    setDl(null);
    refreshLlm();
  };
  const remove = async () => {
    setBusy(true);
    try {
      await llm.remove();
      setAsking(null);
      setError(undefined);
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
    refreshLlm();
  };

  const loading = dl ?? (s.downloading ? { done: 0, total: s.bytes } : null);
  return (
    <>
      <div className="prefs__row llm-row">
        <span className="prefs__text">
          <span className="t-body-m-strong">개념 관계 확인 모델</span>
          {loading ? (
            <>
              <span className="t-body-s c-secondary">
                내려받고 있어요 · {mb(loading.done)} / {mb(loading.total)}
              </span>
              <span className="llm-progress" role="progressbar" aria-valuenow={loading.total ? Math.round((loading.done / loading.total) * 100) : 0}>
                <span className="llm-progress__fill" style={{ width: `${loading.total ? Math.round((loading.done / loading.total) * 100) : 0}%` }} />
              </span>
            </>
          ) : s.ready ? (
            <>
              <span className="t-body-s c-secondary">
                실험 기능 · {s.name} · 약 {gb(s.bytes)} · 서술형 답의 관계를 이 기기 안에서 살펴봐요
              </span>
              <span className="t-caption c-tertiary">{speedLine(s)}</span>
            </>
          ) : (
            <span className="t-body-s c-secondary">실험 기능 · 서술형 답이 원인 → 결과, 비교, 순서 같은 관계를 맞게 말했는지 이 기기 안에서 살펴봐요. 받지 않아도 핵심어 검사는 그대로예요.</span>
          )}
          {error && <span className="t-body-s c-danger">{error}</span>}
        </span>
        {loading ? (
          <Button variant="ghost" size="M" label="멈추기" onClick={() => llm.cancel()} />
        ) : s.ready ? (
          <Button variant="ghost" size="M" label="지우기" onClick={() => setAsking("delete")} />
        ) : (
          <Button
            variant="secondary"
            size="M"
            icon="download"
            label={error ? "다시 받기" : "받기"}
            onClick={() => {
              refreshLlm(); // 남은 공간을 새로 잰다
              setAsking("get");
            }}
          />
        )}
      </div>
      {asking === "get" && <GetModel s={s} onClose={() => setAsking(null)} onGet={download} />}
      {asking === "delete" && (
        <Modal title="개념 관계 확인 모델을 지울까요?" onClose={() => setAsking(null)} variant="confirm" icon="trash">
          <p className="t-body-m c-secondary">약 {gb(s.bytes)}를 비워요. 서술형 핵심어 검사와 평가는 그대로예요. 다시 쓰려면 다시 받아야 해요.</p>
          {error && <p className="t-body-s c-danger">{error}</p>}
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setAsking(null)} />
            <Button variant="danger" size="M" icon="trash" label="지우기" busy={busy} onClick={remove} />
          </div>
        </Modal>
      )}
    </>
  );
}

/** 왜 느린지: candle 의 양자화 계산은 CPU 와 앱 빌드가 모두 AVX2 일 때만 빠르다 (llm.rs LlmStatus) */
function speedLine(s: LlmStatus) {
  const why = !s.avx2_cpu
    ? "이 CPU는 빠른 계산(AVX2)을 지원하지 않아 확인이 느려요"
    : s.avx2_build
      ? "빠른 계산(AVX2)을 써요"
      : "이 앱 빌드는 CPU의 빠른 계산(AVX2)을 쓰지 않아 확인이 느려요";
  return s.ms_per_token ? `${why} · 지난 확인 토큰당 약 ${s.ms_per_token}ms` : why;
}

/** 받기 전 확인: 크기 · 남은 공간 · 라이선스 · 출처. 공간이 모자라면 받지 않는다 */
function GetModel({ s, onClose, onGet }: { s: LlmStatus; onClose: () => void; onGet: () => void }) {
  const short = s.free !== null && s.free < s.need;
  return (
    <Modal title="개념 관계 확인 모델을 받을까요?" sub="서술형 답의 관계를 이 기기 안에서 살펴보는 작은 언어 모델이에요 (실험 기능)" onClose={onClose}>
      <dl className="llm-facts">
        <dt className="t-label-s c-tertiary">크기</dt>
        <dd className="t-body-m">약 {gb(s.bytes)}</dd>
        <dt className="t-label-s c-tertiary">남은 공간</dt>
        <dd className={`t-body-m ${short ? "c-danger" : ""}`}>
          {s.free === null ? "확인하지 못했어요" : `${gb(s.free)}${short ? ` · ${gb(s.need)} 이상 필요해요` : ""}`}
        </dd>
        <dt className="t-label-s c-tertiary">모델</dt>
        <dd className="t-body-m">{s.name}</dd>
        <dt className="t-label-s c-tertiary">라이선스</dt>
        <dd className="t-body-m">
          <Badge>{s.license}</Badge>
        </dd>
        <dt className="t-label-s c-tertiary">출처</dt>
        <dd className="t-body-m">{s.source}</dd>
      </dl>
      <p className="t-body-s c-secondary">
        받은 뒤에는 인터넷 없이 이 기기에서만 돌고, 쓴 답을 밖으로 보내지 않아요. 작은 모델이라 판정이 자주 틀려요: 결과는 비교 화면에 참고로만 보여 주고 평가 추천은 바꾸지 않아요. 받다가 끊기면 받던 파일은 지우고 핵심어 검사는 그대로예요.
      </p>
      {short && <p className="t-body-s c-danger">저장 공간이 모자라요. {gb(s.need)} 이상 비운 뒤 다시 해 주세요.</p>}
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
        <Button size="M" icon="download" label={`내려받기 · ${gb(s.bytes)}`} disabled={short} onClick={onGet} />
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- J2 비교 화면

const VERDICT: Record<Relation["verdict"], { label: string; icon: IconName; tone: string }> = {
  ok: { label: "맞음", icon: "check", tone: "is-ok" },
  missing: { label: "빠짐", icon: "x", tone: "is-miss" },
  reversed: { label: "거꾸로", icon: "rotate", tone: "is-bad" },
  negated: { label: "부정", icon: "alert", tone: "is-bad" },
  wrong: { label: "틀림", icon: "alert", tone: "is-bad" },
};
/** 받지 않았을 때의 안내를 숨겼다 (이 기기에서만) */
const TIP_HIDDEN = "neo.conceptTipHidden";
const tipHidden = () => {
  try {
    return localStorage.getItem(TIP_HIDDEN) === "1";
  } catch {
    return false;
  }
};

type Phase = "idle" | "busy" | "done" | "bad" | "error";

/**
 * J2 의 개념 관계 확인. on: 쓴 답과 비교 화면이 있을 때. rules: 핵심어 추천 (합친 추천과 견준다).
 * 돌려주는 것: 비교 칸 아래 자리, 합친 추천(결과가 있을 때만), 추천 줄 문구(추천을 바꿨을 때만), 멈춤
 */
export function useConcept({ cardId, typed, on, writing, rules }: { cardId: number | null; typed: string; on: boolean; writing: boolean; rules: 1 | 2 | 3 | null }) {
  const s = useLlm();
  const ready = !!s?.ready;
  const [phase, setPhase] = useState<Phase>("idle");
  const [result, setResult] = useState<Concept | null>(null);
  const [fail, setFail] = useState("");
  const [slow, setSlow] = useState(false);
  const [hidden, setHidden] = useState(tipHidden);
  const seq = useRef(0);

  // 서술형 카드가 나오면 답을 쓰는 동안 미리 (모델 읽기 · 관계 뽑기 · 판정 앞부분). 학습 화면을 떠나면 메모리를 돌려준다
  useEffect(() => {
    if (writing && ready && cardId !== null) writtenApi.prepareConcept(cardId).catch(() => {});
  }, [writing, ready, cardId]);
  useEffect(() => () => void llm.release().catch(() => {}), []);

  const run = useCallback(() => {
    if (cardId === null) return;
    const my = ++seq.current;
    setPhase("busy");
    setResult(null);
    writtenApi.concept(cardId, typed).then(
      (c) => {
        if (seq.current !== my) return; // 그 사이 평가했거나 다음 카드: 버린다
        setResult(c);
        setPhase(c ? "done" : "bad");
      },
      (e) => {
        if (seq.current !== my) return;
        setFail(errorText(e));
        setPhase("error");
      },
    );
    // typed 는 비교할 때 정해진다
  }, [cardId, typed]);

  // 비교 화면이 뜨면 바로 (로컬이라 보내는 것이 없다). 카드가 바뀌면 멈춘다
  useEffect(() => {
    seq.current++;
    setPhase("idle");
    setResult(null);
    if (!on || !ready || cardId === null) return;
    run();
    return () => {
      seq.current++;
      writtenApi.stopConcept().catch(() => {});
    };
  }, [cardId, on, ready]);
  useEffect(() => {
    if (phase !== "busy") return setSlow(false);
    const t = setTimeout(() => setSlow(true), 15_000);
    return () => clearTimeout(t);
  }, [phase]);

  const stop = useCallback(() => {
    if (phase !== "busy") return;
    seq.current++;
    writtenApi.stopConcept().catch(() => {});
    setPhase("idle");
  }, [phase]);

  const rating = phase === "done" && result ? result.suggest : undefined;
  const changed = rating !== undefined && rating !== rules;
  const bad = result?.relations.filter((r) => r.verdict !== "ok" && r.verdict !== "missing") ?? [];
  const missing = result?.relations.filter((r) => r.verdict === "missing") ?? [];
  const reason = !result
    ? ""
    : bad.length
      ? `관계가 ${VERDICT[bad[0].verdict].label}예요 (${bad[0].rel})`
      : missing.length && missing.length === result.relations.length
        ? "모범 답안의 관계를 말하지 않았어요"
        : missing.length
          ? `빠진 관계가 있어요 (${missing[0].rel})`
          : "관계를 모두 맞게 말했어요";
  /** 추천 줄. 개념 관계가 추천을 바꿨을 때만 (같으면 핵심어 문구 그대로) */
  const tip = changed && rating !== null ? `${reason}${rules !== null ? " · 핵심어 기준보다 낮춰 추천해요" : ""}` : null;

  const hideTip = () => {
    setHidden(true);
    try {
      localStorage.setItem(TIP_HIDDEN, "1");
    } catch {
      /* 저장이 막힌 환경. 다음에 다시 보일 뿐이다 */
    }
  };

  const slot = !on ? null : !ready ? (
    s && !hidden && (
      <p className="concept concept--tip t-caption c-tertiary">
        <Icon name="info" size={14} />
        <span>설정 &gt; 데이터에서 개념 관계 확인 모델(실험 기능)을 받으면 원인 → 결과 같은 관계를 맞게 썼는지 참고로 볼 수 있어요</span>
        <button className="concept__x" aria-label="개념 관계 안내 숨기기" onClick={hideTip}>
          <Icon name="x" size={12} />
        </button>
      </p>
    )
  ) : phase === "busy" ? (
    <div className="concept concept--wait" role="status">
      <Spinner size={14} />
      <span className="t-label-s c-secondary">{slow ? "조금 오래 걸려요 · 평가는 지금 해도 돼요" : "개념 관계를 확인하고 있어요 · 평가는 지금 해도 돼요"}</span>
      <button type="button" className="ai-link t-label-s" onClick={stop}>
        멈추기
      </button>
    </div>
  ) : phase === "error" || phase === "bad" ? (
    <p className="concept concept--tip t-caption c-tertiary" role="status">
      <Icon name="info" size={14} />
      <span>
        개념 관계를 확인하지 못했어요 · {phase === "bad" ? "모델 답의 형식이 맞지 않아 쓰지 않았어요" : fail} 핵심어 검사와 평가는 그대로예요.
      </span>
      <button type="button" className="ai-link t-label-s" onClick={run}>
        다시 확인
      </button>
    </p>
  ) : phase === "done" && result ? (
    <section className="concept" aria-label="개념 관계 확인">
      <div className="concept__head">
        <span className="t-label-m">개념 관계</span>
        <Badge tone={result.verdict === "correct" ? "success" : result.verdict === "partial" ? "warn" : "danger"}>
          {result.verdict === "correct" ? "모두 맞음" : result.verdict === "partial" ? "일부 빠짐" : result.verdict === "wrong" ? "틀린 관계 있음" : "관계를 말하지 않음"}
        </Badge>
        <span className="t-caption c-tertiary">이 기기의 작은 모델이 본 결과예요 · 자주 틀려 추천에는 쓰지 않아요</span>
      </div>
      <ul className="concept__list">
        {result.relations.map((r, i) => (
          <li key={i} className={`concept__rel ${VERDICT[r.verdict].tone}`}>
            <span className="concept__verdict t-label-s">
              <Icon name={VERDICT[r.verdict].icon} size={12} />
              {VERDICT[r.verdict].label}
            </span>
            <span className="t-body-s">{r.rel}</span>
          </li>
        ))}
      </ul>
    </section>
  ) : null;

  return { slot, rating, changed, tip, stop };
}
