// 8.3 서술형 개념 관계 확인 (llm.rs · 보조 실행 파일 src-tauri/llm). 설정 › 서술형 채점 칸(Figma J5 · J6)과 J2 비교 화면의 한 줄 요약(J4).
// 모델은 크기 · 남은 공간 · 라이선스 · 출처를 보고 사용자가 받을 때만 받는다. 이 PC 의 CPU 가 AVX2 를 지원하지 않거나 엔진이 없으면 쓸 수 없고,
// 설정 칸이 그렇게 알린다 (학습 화면에는 아무것도 보이지 않는다). 결과는 참고용이다: 평가 추천은 핵심어 검사만 쓴다 (J4).
// 기다리는 동안에도 1–4 가 된다 (평가하면 확인을 멈추고 결과는 버린다)
import "../styles/concept.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import { useToast } from "../toast";
import { Badge, Button, Icon, Spinner, type IconName } from "../ui";
import { Modal } from "./NewDeck";
import { llm, written as writtenApi, type Concept, type LlmStatus, type Relation } from "../written";

const gb = (bytes: number) => `${(bytes / 1e9).toFixed(1)}GB`;

// 상태는 설정 칸과 학습 화면이 같이 본다. 받거나 지우면 refreshLlm()
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

/** 이 PC 에서 쓸 수 있는가 (CPU AVX2 · 엔진 파일) */
const usable = (s: LlmStatus) => s.cpu_ok && s.engine;

// ---------------------------------------------------------------- 설정 › 서술형 채점 (J5)

/** 설정의 "서술형 채점" 칸 (Settings.tsx 에서 한 줄로 끼운다) */
export function WrittenSection() {
  return (
    <section className="prefs__section">
      <p className="prefs__label t-label-s c-tertiary">서술형 채점</p>
      <LlmRow />
    </section>
  );
}

function LlmRow() {
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
  const ok = usable(s);
  const sub = !ok
    ? `${s.cpu_ok ? "이 설치에는 개념 관계 확인 엔진이 없어 쓸 수 없어요." : "이 PC의 CPU가 빠른 계산(AVX2)을 지원하지 않아 쓸 수 없어요."} 서술형 핵심어 검사와 ChatGPT 채점 보조는 그대로 쓸 수 있어요.`
    : loading
      ? `내려받고 있어요 · ${gb(loading.done)} / ${gb(loading.total)} · 다 받으면 이 기기 안에서만 돌아요`
      : s.ready
        ? `${s.short} · 약 ${gb(s.bytes)} · 실험 기능${s.last_ms ? ` · 지난 확인은 ${Math.max(1, Math.round(s.last_ms / 1000))}초 걸렸어요` : ""}`
        : "서술형 답이 원인 → 결과, 비교, 순서 같은 관계를 맞게 말했는지 이 기기 안에서 살펴봐요. 받지 않아도 핵심어 검사는 그대로예요.";
  return (
    <>
      <div className="prefs__row llm-row">
        <span className="prefs__text">
          <span className="t-body-m-strong llm-row__title">
            개념 관계 확인 모델 · 실험 기능 {!ok && <Badge>이 PC에서는 쓸 수 없어요</Badge>}
          </span>
          <span className="t-body-s c-secondary">{sub}</span>
          {error && <span className="t-body-s c-danger">{error}</span>}
        </span>
        {!ok ? (
          s.ready && <Button variant="ghost" size="M" icon="trash" label={`지우기 · ${gb(s.bytes)} 비우기`} onClick={() => setAsking("delete")} />
        ) : loading ? (
          <Button variant="ghost" size="M" label="멈추기" onClick={() => llm.cancel()} />
        ) : s.ready ? (
          <Button variant="ghost" size="M" label="지우기" onClick={() => setAsking("delete")} />
        ) : (
          <Button
            variant="secondary"
            size="M"
            icon="download"
            label={`${error ? "다시 받기" : "받기"} · ${gb(s.bytes)}`}
            onClick={() => {
              refreshLlm(); // 남은 공간을 새로 잰다
              setAsking("get");
            }}
          />
        )}
      </div>
      {asking === "get" && ok && <GetModel s={s} onClose={() => setAsking(null)} onGet={download} />}
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

/** 받기 전 확인 (J6): 크기 · 남은 공간 · 모델 · 라이선스 · 출처 · 이 PC. 공간이 모자라면 받지 않는다. 이 PC 에서 쓸 수 없으면 열지 않는다 */
function GetModel({ s, onClose, onGet }: { s: LlmStatus; onClose: () => void; onGet: () => void }) {
  const short = s.free !== null && s.free < s.need;
  return (
    <Modal title="개념 관계 확인 모델을 받을까요?" sub="서술형 답의 관계를 이 기기 안에서 살펴보는 작은 언어 모델이에요 (실험 기능)" onClose={onClose}>
      <dl className="llm-facts">
        <dt className="t-label-s c-tertiary">크기</dt>
        <dd className="t-body-m">약 {gb(s.bytes)} · 받는 데 수 분 걸려요</dd>
        <dt className="t-label-s c-tertiary">남은 공간</dt>
        <dd className={`t-body-m ${short ? "c-danger" : ""}`}>
          {s.free === null ? "확인하지 못했어요" : `${gb(s.free)}${short ? ` · ${gb(s.need)}가 필요해요` : ""}`}
        </dd>
        <dt className="t-label-s c-tertiary">모델</dt>
        <dd className="t-body-m">{s.name}</dd>
        <dt className="t-label-s c-tertiary">라이선스</dt>
        <dd className="t-body-m">
          <Badge>{s.license}</Badge>
        </dd>
        <dt className="t-label-s c-tertiary">출처</dt>
        <dd className="t-body-m">{s.source}</dd>
        <dt className="t-label-s c-tertiary">이 PC</dt>
        <dd className="t-body-m">AVX2 지원 · 쓸 수 있어요</dd>
      </dl>
      <p className="t-body-s c-secondary">
        받은 뒤에는 인터넷 없이 이 기기에서만 돌고, 쓴 답을 밖으로 보내지 않아요. 작은 모델이라 판정이 자주 틀려요: 결과는 비교 화면에 참고로만 보여 주고 평가 추천은 바꾸지 않아요. 받다가 끊기면 받던 파일은 지우고 핵심어 검사는 그대로예요.
      </p>
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
        <Button size="M" icon="download" label={`내려받기 · ${gb(s.bytes)}`} disabled={short} onClick={onGet} />
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- J2 비교 화면 (J4)

/** 판정 칩 (펼친 목록): 단정하지 않는 말 + 연한 색 (빨강은 핵심어 검사에만) */
const CHIP: Record<Relation["verdict"], { label: string; icon: IconName; tone: string }> = {
  ok: { label: "맞아 보여요", icon: "check", tone: "is-ok" },
  reversed: { label: "거꾸로 같아요", icon: "rotate", tone: "is-warn" },
  negated: { label: "반대 같아요", icon: "rotate", tone: "is-warn" },
  wrong: { label: "달라 보여요", icon: "info", tone: "is-warn" },
  missing: { label: "안 보여요", icon: "x", tone: "is-none" },
};
/** 요약 줄: 노랑 셋(거꾸로 · 반대 · 달라)은 "다르게 보여요" 하나로 센다 (J4 재검토) */
const SUMMARY: { label: string; tone: string; of: Relation["verdict"][] }[] = [
  { label: "맞아 보여요", tone: "is-ok", of: ["ok"] },
  { label: "다르게 보여요", tone: "is-warn", of: ["reversed", "negated", "wrong"] },
  { label: "안 보여요", tone: "is-none", of: ["missing"] },
];

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

/** 미리 하기 표. 늘려 가며 준다 (앱을 다시 열어도 줄지 않게 시각에서 시작) */
let prepToken = Date.now() * 1000;

/**
 * J2 의 개념 관계 확인. on: 쓴 답과 비교 화면이 있을 때. writing: 서술형 카드의 질문 면 (답을 쓰는 동안 미리 한다).
 * 돌려주는 것: 비교 블록 바로 아래 한 줄(기다림 · 요약 · 펼침 · 안내 · 실패), 멈춤
 */
export function useConcept({ cardId, typed, on, writing, toastBusy }: { cardId: number | null; typed: string; on: boolean; writing: boolean; toastBusy: boolean }) {
  const s = useLlm();
  const notify = useToast();
  // 이번 학습에서 껐으면(엔진이 다시 띄워도 멈춤) 더 부르지 않는다. 알림은 토스트 한 번 (카드 안 줄 없음)
  const ready = !!s && s.ready && usable(s) && !s.off;
  const [phase, setPhase] = useState<Phase>("idle");
  const [result, setResult] = useState<Concept | null>(null);
  const [fail, setFail] = useState("");
  const [open, setOpen] = useState(false);
  const [slow, setSlow] = useState(false);
  const [hidden, setHidden] = useState(tipHidden);
  const tipCard = useRef<number | null>(null); // 받지 않았을 때 안내는 이 학습의 첫 서술형 카드에서만
  const seq = useRef(0);
  const box = useRef<HTMLElement | null>(null);
  const setBox = (el: HTMLElement | null) => void (box.current = el);
  // 접힌 한 줄은 "확인하는 중" 줄과 높이가 같아 카드를 움직이지 않는다. 펼칠 때만 펼친 만큼 보이게 내린다
  // (학습 화면이 카드 높이를 다시 재고 난 뒤에, 한 박자 늦게)
  useEffect(() => {
    if (!open) return;
    const t = setTimeout(() => box.current?.scrollIntoView({ block: "nearest" }), 120);
    return () => clearTimeout(t);
  }, [open]);
  const offToast = useRef(false);
  // "이번 학습에서는 껐어요" 는 학습 화면의 되돌리기 · 접기 토스트가 닫힌 뒤에 띄운다 (겹치지 않게, 대기열처럼)
  const [offPending, setOffPending] = useState(false);
  useEffect(() => {
    if (!offPending || toastBusy) return;
    setOffPending(false);
    notify({ title: "개념 관계 확인을 이번 학습에서는 껐어요", text: "다음 학습에서 다시 켜져요. 핵심어 검사와 평가는 그대로예요.", icon: "info" });
  }, [offPending, toastBusy, notify]);

  // 서술형 카드가 나오면 답을 쓰는 동안 미리 (엔진 띄우기 · 모델 읽기 · 관계 뽑기 · 판정 앞부분). 학습 화면을 떠나면 엔진을 끝낸다
  const prep = useRef<number | null>(null);
  useEffect(() => {
    if (!writing || !ready || cardId === null) return;
    const token = ++prepToken;
    prep.current = token;
    writtenApi.prepareConcept(cardId, token).catch(() => {});
  }, [writing, ready, cardId]);
  // 카드가 바뀌면(다음 카드 · 되돌리기 · 접어두기) 그 카드의 미리 하기를 멈춘다: 엔진 잠금을 쥔 채 돌아온 카드의 확인을 막지 않게 (QA Q5 L1).
  // 같은 카드에서 답을 비교할 때(질문 면 → 답 면)는 멈추지 않는다 (그 확인이 미리 한 것을 쓴다)
  useEffect(
    () => () => {
      if (prep.current !== null) writtenApi.stopPrepare(prep.current).catch(() => {});
      prep.current = null;
    },
    [cardId],
  );
  useEffect(() => () => void llm.release().catch(() => {}), []);

  const run = useCallback(() => {
    if (cardId === null) return;
    const my = ++seq.current;
    setPhase("busy");
    setResult(null);
    setOpen(false);
    writtenApi.concept(cardId, typed).then(
      (c) => {
        if (seq.current !== my) return; // 그 사이 평가했거나 다음 카드: 버린다
        setResult(c);
        setPhase(c ? "done" : "bad");
        refreshLlm(); // 설정 칸의 "지난 확인은 N초"
      },
      (e) => {
        if (seq.current !== my) return;
        refreshLlm().then((st) => {
          if (seq.current !== my) return;
          if (!st?.off) {
            setFail(errorText(e));
            return setPhase("error");
          }
          setPhase("idle");
          if (offToast.current) return;
          offToast.current = true;
          setOffPending(true);
        });
      },
    );
    // typed 는 비교할 때 정해진다
  }, [cardId, typed]);

  // 비교 화면이 뜨면 바로 (기기 안이라 보내는 것이 없다). 카드가 바뀌면 멈춘다
  useEffect(() => {
    seq.current++;
    setPhase("idle");
    setResult(null);
    setOpen(false);
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

  const hideTip = () => {
    setHidden(true);
    try {
      localStorage.setItem(TIP_HIDDEN, "1");
    } catch {
      /* 저장이 막힌 환경. 다음에 다시 보일 뿐이다 */
    }
  };

  // 받지 않았을 때 (쓸 수 있는 PC 만): 이 학습의 첫 서술형 카드에서 한 줄, ✕ 로 닫으면 다시 안 나온다
  const tip = on && !!s && usable(s) && !s.ready && !hidden && cardId !== null && (tipCard.current ?? cardId) === cardId;
  if (tip) tipCard.current = cardId;
  const ref = <Badge>참고용 · 틀릴 수 있어요</Badge>;
  const counts = result ? SUMMARY.map((g) => [g, result.relations.filter((r) => g.of.includes(r.verdict)).length] as const).filter(([, n]) => n > 0) : [];

  const slot = !on ? null : tip ? (
    <p className="concept concept--line t-caption c-tertiary">
      <Icon name="info" size={14} />
      <span className="concept__grow">설정 › 서술형 채점에서 개념 관계 확인 모델(실험 기능)을 받으면 원인 → 결과 같은 관계를 맞게 썼는지 참고로 볼 수 있어요</span>
      <button className="concept__x" aria-label="개념 관계 안내 숨기기" onClick={hideTip}>
        <Icon name="x" size={12} />
      </button>
    </p>
  ) : !ready ? null : phase === "busy" ? (
    <div className="concept concept--line" role="status" ref={setBox}>
      <Spinner size={14} />
      <span className="concept__grow t-label-s c-secondary">{slow ? "조금 오래 걸려요 · 평가는 지금 해도 돼요" : "개념 관계를 확인하고 있어요 · 평가는 지금 해도 돼요"}</span>
      <button type="button" className="ai-link t-label-s" onClick={stop}>
        멈추기
      </button>
    </div>
  ) : phase === "error" || phase === "bad" ? (
    <p className="concept concept--line t-caption c-tertiary" role="status">
      <Icon name="info" size={14} />
      <span className="concept__grow">
        개념 관계를 확인하지 못했어요 · {phase === "bad" ? "모범 답안에서 관계를 찾지 못했어요." : fail} 핵심어 검사와 평가는 그대로예요.
      </span>
      <button type="button" className="ai-link t-label-s" onClick={run}>
        다시 확인
      </button>
    </p>
  ) : phase === "done" && result ? (
    <section className={`concept ${open ? "is-open" : "concept--line"}`} aria-label="개념 관계 (참고용)" ref={setBox}>
      <div className="concept__head">
        <Icon name="list" size={14} className="c-secondary" />
        <span className="t-label-s">개념 관계</span>
        {ref}
        {!open &&
          counts.map(([g, n]) => (
            <span key={g.label} className={`concept__chip t-label-s ${g.tone}`}>
              {g.label} {n}
            </span>
          ))}
        <button type="button" className="concept__toggle ai-link t-label-s" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          {open ? "접기" : "자세히"}
          <Icon name="chevron-down" size={14} className={open ? "concept__up" : ""} />
        </button>
      </div>
      {open && (
        <>
          <p className="t-caption c-secondary">
            이 기기의 작은 모델이 모범 답안의 관계를 내 답에서 찾아본 결과예요. 자주 틀려서 평가 추천에는 쓰지 않아요. 핵심어 검사에서 빠진 말이 든 관계는 &lsquo;안 보여요&rsquo;로 둬요.
          </p>
          <div className="concept__grid">
            <span className="t-caption c-tertiary">내 답에서</span>
            <span className="t-caption c-tertiary">모범 답안의 관계</span>
            {result.relations.map((r, i) => (
              <div key={i} className="concept__row">
                <span className={`concept__chip t-label-s ${CHIP[r.verdict].tone}`}>
                  <Icon name={CHIP[r.verdict].icon} size={12} />
                  {CHIP[r.verdict].label}
                </span>
                <span className="t-body-s">{r.rel}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  ) : null;

  return { slot, stop };
}
