// G2 성격별로 묶기 · ChatGPT 요금제 (Figma Row L: L1–L4). 일은 Rust(src-tauri/src/ai.rs)가 하고, 로그인 결과는 "ai-auth" 이벤트로 온다.
// 이 파일: 상태 공유(useAiStatus) · 설정 칸(L1 · L1-c) · 연결 창(L2 · L2-w · L2-x · L3) · 나누기 창의 오류 상자(L4-e)
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { aiApi, errorText, type AiAuthEvent, type AiError, type AiStatus } from "./api";
import { Button, Icon, Spinner, type IconName } from "./ui";
import { useToast } from "./toast";
import { n } from "./nav";
import { Head, Sheet, when } from "./sync";
// OpenAI 공식 로그인 버튼 로고 (developers.openai.com/assets/siwc/sign-in-buttons, 고치지 않고 쓴다)
import logoWhite from "./assets/brand/chatgpt-logo-white.svg";
import logoBlack from "./assets/brand/chatgpt-logo-black.svg";
import "./styles/sync.css";
import "./styles/ai.css";

// ---------------------------------------------------------------- 상태 (설정과 나누기 창이 같은 값을 본다)

let current: AiStatus | null = null;
const subs = new Set<(s: AiStatus) => void>();
export const refreshAi = () =>
  aiApi.status().then((s) => {
    current = s;
    subs.forEach((f) => f(s));
  }, () => {});

export function useAiStatus() {
  const [s, setS] = useState(current);
  useEffect(() => {
    subs.add(setS);
    refreshAi();
    return () => {
      subs.delete(setS);
    };
  }, []);
  return s;
}

/** 처음 보낼 때 한 번만 묻는다 (L4-s). 연결을 끊으면 지워서 다시 연결한 뒤 다시 묻는다 */
const SEND_OK = "neoanki.ai.sendConfirmed";
export const sendConfirmed = () => {
  try {
    return localStorage.getItem(SEND_OK) === "1";
  } catch {
    return false;
  }
};
export const setSendConfirmed = (on: boolean) => {
  try {
    if (on) localStorage.setItem(SEND_OK, "1");
    else localStorage.removeItem(SEND_OK);
  } catch {
    /* 저장하지 못하면 다음에 다시 묻는다 */
  }
};

const openUsage = () => aiApi.openUsage().catch(() => {});

// ---------------------------------------------------------------- 로그인 버튼 (OpenAI 지침: "Continue with ChatGPT")

/** 바탕은 text/primary, 글자는 bg/canvas (밝게: 검정 버튼 + 흰 로고 · 어둡게: 흰 버튼 + 검정 로고). 로고는 테마에 따라 CSS 로 바꾼다 */
export function ChatGptButton({ onClick, disabled }: { onClick: () => void; disabled?: boolean }) {
  return (
    <button type="button" className="btn btn--M ai-brand" disabled={disabled} onClick={onClick}>
      <img className="ai-brand__logo is-white" src={logoWhite} alt="" width={18} height={18} />
      <img className="ai-brand__logo is-black" src={logoBlack} alt="" width={18} height={18} />
      <span className="t-label-m">Continue with ChatGPT</span>
    </button>
  );
}

// ---------------------------------------------------------------- L2 · L2-w · L2-x · L3 연결

type Stage = { kind: "intro" } | { kind: "wait"; reconsent: boolean; newAccount: boolean } | { kind: "fail"; ev: AiAuthEvent } | { kind: "welcome" };

/**
 * ChatGPT 연결. start=intro 는 설명 창(L2)부터, wait 는 바로 브라우저(다시 로그인 · 권한 다시 허락).
 * 끝나면 onDone(true). 나누기 창에서 열었어도 돌아간 뒤 저절로 보내지 않는다
 */
export function AiConnect({ start = "intro", reconsent = false, onDone }: { start?: "intro" | "wait"; reconsent?: boolean; onDone: (ok: boolean) => void }) {
  const [stage, setStage] = useState<Stage>(start === "wait" ? { kind: "wait", reconsent, newAccount: false } : { kind: "intro" });
  const toast = useToast();
  const close = () => onDone(false);
  const finished = useCallback(
    (ev: AiAuthEvent) => {
      refreshAi();
      if (ev.first) return setStage({ kind: "welcome" });
      toast({ title: "ChatGPT에 연결했어요", text: ev.email ?? undefined, icon: "check-circle" });
      onDone(true);
    },
    [onDone, toast],
  );
  if (stage.kind === "intro") return <Intro onClose={close} onGo={() => setStage({ kind: "wait", reconsent: false, newAccount: false })} />;
  if (stage.kind === "wait")
    return (
      <Wait
        key={`${stage.reconsent}-${stage.newAccount}`}
        reconsent={stage.reconsent}
        newAccount={stage.newAccount}
        onClose={close}
        onDone={finished}
        onFail={(ev) => setStage({ kind: "fail", ev })}
      />
    );
  if (stage.kind === "fail")
    return (
      <Fail
        ev={stage.ev}
        onClose={close}
        onRetry={(o) => setStage({ kind: "wait", reconsent: o.reconsent, newAccount: o.newAccount })}
      />
    );
  return <Welcome onClose={() => onDone(true)} />;
}

/** L2 무엇을 보내고 무엇을 보내지 않는지. 동의 체크는 없다 (브라우저의 OpenAI 동의 화면이 동의다) */
function Intro({ onClose, onGo }: { onClose: () => void; onGo: () => void }) {
  return (
    <Sheet label="ChatGPT를 연결할까요?" onClose={onClose} className="ai-l2">
      <Head title="ChatGPT를 연결할까요?" sub="카드를 성격별로 묶어 나눌 때만 써요. 내 ChatGPT 요금제로 요청하고, Neo-Anki에는 추가 요금이 없어요." onClose={onClose} />
      <div className="ai-two">
        <div className="ai-two__col">
          <span className="ai-two__head">
            <Icon name="send" size={16} className="c-accent" />
            <span className="t-label-m">OpenAI로 보내는 것</span>
          </span>
          <span className="t-body-s c-secondary">나누기 창에서 직접 누른 카드 한 장의 앞·뒷면 글자</span>
        </div>
        <div className="ai-two__col">
          <span className="ai-two__head">
            <Icon name="lock" size={16} className="c-secondary" />
            <span className="t-label-m">보내지 않는 것</span>
          </span>
          <span className="t-body-s c-secondary">덱 전체 · 다른 카드</span>
          <span className="t-body-s c-secondary">복습 기록 · 일정</span>
          <span className="t-body-s c-secondary">원자료 파일 · 그림</span>
        </div>
      </div>
      <div className="ai-notes">
        <Note icon="gauge" title="ChatGPT 요금제 사용량이 Codex와 함께 줄어요" text="ChatGPT 설정의 사용량 관리에서 이 앱의 한도를 정할 수 있어요" />
        <Note icon="link-off" title="연결을 끊으면 바로 멈춰요" text="이미 나눈 카드는 그대로예요" />
        <Note icon="info" title="ChatGPT Plus · Pro 같은 대상 요금제에서 쓸 수 있어요" />
      </div>
      <div className="sync-foot sync-foot--end">
        <Button variant="ghost" size="M" label="취소" onClick={onClose} />
        <ChatGptButton onClick={onGo} />
      </div>
    </Sheet>
  );
}

function Note({ icon, title, text }: { icon: IconName; title: string; text?: string }) {
  return (
    <span className="ai-note">
      <Icon name={icon} size={18} className="c-secondary" />
      <span className="ai-note__text">
        <span className="t-body-m">{title}</span>
        {text && <span className="t-caption c-tertiary">{text}</span>}
      </span>
    </span>
  );
}

/** L2-w 브라우저 로그인 기다림 (K2 와 같은 틀). 사용량 권한은 따로, 없으면 연결할 수 없다고 먼저 말한다 */
function Wait({ reconsent, newAccount, onClose, onDone, onFail }: {
  reconsent: boolean;
  newAccount: boolean;
  onClose: () => void;
  onDone: (ev: AiAuthEvent) => void;
  onFail: (ev: AiAuthEvent) => void;
}) {
  const [url, setUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const waiting = useRef(true);
  // 부모가 다시 그려져도(상태 갱신) 기다리기를 다시 시작하지 않게 콜백은 ref 로
  const cb = useRef({ onClose, onDone, onFail });
  useEffect(() => {
    cb.current = { onClose, onDone, onFail };
  });
  useEffect(() => {
    let alive = true;
    let un: (() => void) | undefined;
    listen<AiAuthEvent>("ai-auth", (e) => {
      if (!alive) return;
      waiting.current = false;
      if (e.payload.ok) cb.current.onDone(e.payload);
      else if (e.payload.reason === "cancelled") cb.current.onClose();
      else cb.current.onFail(e.payload);
    }).then((f) => {
      un = f;
      if (!alive) return;
      aiApi.connect(reconsent, newAccount).then(
        (u) => alive && setUrl(u),
        (err) => {
          waiting.current = false;
          if (alive) cb.current.onFail({ ok: false, reason: "error", message: errorText(err), email: null, first: false });
        },
      );
    });
    return () => {
      alive = false;
      un?.();
      if (waiting.current) aiApi.cancelConnect().catch(() => {});
    };
  }, [reconsent, newAccount]);
  const cancel = () => {
    aiApi.cancelConnect().catch(() => {});
    waiting.current = false;
    onClose();
  };
  const copy = () => url && navigator.clipboard.writeText(url).then(() => setCopied(true), () => {});
  return (
    <Sheet label="브라우저에서 ChatGPT 로그인을 마쳐 주세요" onClose={cancel} className="sync-k2">
      <Spinner size={36} />
      <h2 className="t-title-m">브라우저에서 ChatGPT 로그인을 마쳐 주세요</h2>
      <p className="t-body-s c-secondary sync-center">기본 브라우저에 OpenAI 로그인 창을 열었어요. 계정을 고르고 권한을 허용하면 이 창으로 자동으로 돌아와요.</p>
      <div className="sync-box">
        <span className="t-label-s c-tertiary">Neo-Anki가 요청하는 권한</span>
        <span className="sync-perm">
          <Icon name="check" size={16} className="c-secondary" />
          <span className="sync-perm__text">
            <span className="t-body-s">계정 기본 정보</span>
            <span className="t-caption c-tertiary">이메일 · 연결된 계정 표시에만 써요</span>
          </span>
        </span>
        <span className="sync-perm">
          <Icon name="gauge" size={16} className="c-secondary" />
          <span className="sync-perm__text">
            <span className="t-body-s">ChatGPT 요금제로 요청 보내기 (사용량)</span>
            <span className="t-caption c-tertiary">이 권한이 없으면 연결할 수 없어요</span>
          </span>
        </span>
      </div>
      <div className="sync-foot">
        <span className="sync-foot__left">
          <span className="t-caption c-tertiary">브라우저가 안 열렸나요?</span>
          <Button variant="ghost" size="M" icon="external" label={copied ? "복사했어요" : "링크 복사"} disabled={!url} onClick={copy} />
        </span>
        <Button variant="secondary" size="M" label="취소" onClick={cancel} />
      </div>
    </Sheet>
  );
}

/** L2-x 연결하지 못했다. 사용량 권한이 빠졌으면 [다시 연결], 요금제가 안 되면 [다른 계정으로]. 업그레이드는 권하지 않는다 */
function Fail({ ev, onClose, onRetry }: { ev: AiAuthEvent; onClose: () => void; onRetry: (o: { reconsent: boolean; newAccount: boolean }) => void }) {
  const [head, text] =
    ev.reason === "scope" ? ["사용량 권한이 필요해요", `Neo-Anki는 내 ChatGPT 요금제로만 요청해요. 로그인할 때 "사용량" 권한을 허락해 주세요.`]
    : ev.reason === "not_eligible" ? ["이 요금제에서는 쓸 수 없어요", "지금 계정의 ChatGPT 요금제는 다른 앱에서 요금제로 요청하는 기능을 지원하지 않아요."]
    : ev.reason === "timeout" ? ["시간이 지났어요", "5분 안에 로그인을 마치지 않아 기다리기를 멈췄어요."]
    : ev.reason === "denied" ? ["로그인을 취소했어요", "브라우저에서 연결을 허락하지 않았어요."]
    : ev.reason === "offline" ? ["인터넷에 연결되어 있지 않아요", "연결되면 다시 시도해 주세요."]
    : ["연결하지 못했어요", ev.message ?? ""];
  return (
    <Sheet label={head} onClose={onClose} className="ai-l2x">
      <h2 className="t-title-m ai-l2x__title">
        <Icon name="alert" size={22} className="c-warn" />
        {head}
      </h2>
      <p className="t-body-s c-secondary">{text}</p>
      <p className="t-caption c-tertiary">연결하지 않았어요. 규칙 나누기는 그대로 쓸 수 있어요.</p>
      <div className="sync-foot sync-foot--end">
        {ev.reason === "not_eligible" ? (
          <>
            <Button variant="ghost" size="M" label="다른 계정으로" onClick={() => onRetry({ reconsent: false, newAccount: true })} />
            <Button size="M" label="닫기" onClick={onClose} />
          </>
        ) : (
          <>
            <Button variant="ghost" size="M" label="취소" onClick={onClose} />
            <Button size="M" icon="rotate" label={ev.reason === "scope" ? "다시 연결" : "다시 시도"} onClick={() => onRetry({ reconsent: ev.reason === "scope", newAccount: false })} />
          </>
        )}
      </div>
    </Sheet>
  );
}

/** L3 처음 연결했을 때 한 번만 (OpenAI 지침의 환영 창). 로고 40px: 밝게 검정 · 어둡게 흰색 */
function Welcome({ onClose }: { onClose: () => void }) {
  return (
    <Sheet label="내 ChatGPT 요금제를 쓰고 있어요" onClose={onClose} className="sync-k2 ai-l3">
      <img className="ai-mark is-black" src={logoBlack} alt="ChatGPT" width={40} height={40} />
      <img className="ai-mark is-white" src={logoWhite} alt="ChatGPT" width={40} height={40} />
      <h2 className="t-title-m">내 ChatGPT 요금제를 쓰고 있어요</h2>
      <p className="t-body-s c-secondary sync-center">
        카드를 성격별로 묶을 때마다 ChatGPT 요금제 사용량에서 줄어요.
        <br />
        Neo-Anki에는 추가 요금이 없어요.
      </p>
      <p className="t-caption c-tertiary sync-center">한도는 ChatGPT 설정의 사용량 관리에서 정할 수 있어요.</p>
      <div className="ai-l3__foot">
        <Button variant="ghost" size="M" icon="external" label="사용량 관리" onClick={openUsage} />
        <Button size="M" label="알겠어요" autoFocus onClick={onClose} />
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- 설정 > AI 연결 (L1 · L1-c)

/** Google Drive 칸 아래. 연결 전(L1) · 연결됨(L1-c) */
export function AiSection() {
  const s = useAiStatus();
  const toast = useToast();
  const [connecting, setConnecting] = useState(false);
  const [busy, setBusy] = useState(false);
  if (!s) return null;
  // 확인 없이 바로 끊는다. 진행 중인 묶기도 멈춘다 (L1-c)
  const disconnect = async () => {
    setBusy(true);
    try {
      const revoked = await aiApi.disconnect();
      setSendConfirmed(false);
      await refreshAi();
      toast({ title: "ChatGPT 연결을 끊었어요", text: revoked ? "이미 나눈 카드는 그대로예요." : "ChatGPT 설정에서도 이 앱의 연결을 끊을 수 있어요." });
    } catch (e) {
      toast({ title: "연결을 끊지 못했어요", text: errorText(e), tone: "error" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="prefs__section" aria-label="AI 연결">
      <p className="prefs__label t-label-s c-tertiary">AI 연결</p>
      {s.connected ? (
        <>
          <div className="sync-hero">
            <span className="sync-tile sync-tile--ok">
              <Icon name="check" size={22} className="c-success" />
            </span>
            <span className="prefs__text">
              <span className="t-body-m-strong">ChatGPT 연결됨</span>
              <span className="t-caption c-tertiary">{s.email} · ChatGPT 요금제 사용 · Neo-Anki 추가 요금 없음</span>
            </span>
            <Button variant="secondary" size="M" icon="external" label="사용량 관리" onClick={openUsage} />
          </div>
          <div className="ai-rows">
            <Row label="쓰는 곳" value="카드 나누기 · 성격별로 묶기" />
            <Row label="보내는 것" value="직접 누른 카드 한 장의 앞·뒷면 글자" />
            <Row label="보낸 카드" value={`이번 달 ${n(s.sent_month)}장${s.sent_at ? ` · 마지막 ${when(s.sent_at)}` : ""}`} />
          </div>
          <div className="ai-foot">
            <span className="t-caption c-tertiary">끊으면 바로 멈춰요. 이미 나눈 카드는 그대로예요.</span>
            <Button variant="danger" size="M" icon="link-off" label="연결 끊기" busy={busy} onClick={disconnect} />
          </div>
        </>
      ) : (
        <>
          <div className="sync-hero">
            <span className="sync-tile">
              <Icon name="sparkles" size={22} className="c-secondary" />
            </span>
            <span className="prefs__text">
              <span className="t-body-m-strong">ChatGPT로 카드를 성격별로 묶어 나누기</span>
              <span className="t-caption c-tertiary">카드 나누기에서 "성격별로 묶기"를 누를 때만 내 ChatGPT 요금제를 써요. Neo-Anki에는 추가 요금이 없어요.</span>
            </span>
            <ChatGptButton onClick={() => setConnecting(true)} />
          </div>
          <div className="sync-facts">
            <span>
              <Icon name="send" size={16} className="c-secondary" />
              <span className="t-caption c-secondary">직접 누른 카드 한 장만 OpenAI로 보내요</span>
            </span>
            <span>
              <Icon name="lock" size={16} className="c-secondary" />
              <span className="t-caption c-secondary">덱·기록·원자료·그림은 안 보내요</span>
            </span>
            <span>
              <Icon name="layers" size={16} className="c-secondary" />
              <span className="t-caption c-secondary">연결하지 않아도 규칙 나누기는 그대로</span>
            </span>
          </div>
        </>
      )}
      {connecting && <AiConnect onDone={() => setConnecting(false)} />}
    </section>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="ai-row">
      <span className="t-body-m">{label}</span>
      <span className="t-body-s c-secondary">{value}</span>
    </div>
  );
}

// ---------------------------------------------------------------- L4-e 나누기 창 오른쪽 목록 칸의 오류

/** 어떤 오류든 [규칙으로 나누기]로 돌아갈 수 있다 (깨진 응답이면 아래에 규칙 제안이 이미 보인다) */
export function AiErrorBox({ e, onRule, onRetry, onReconnect }: {
  e: AiError;
  onRule: () => void;
  onRetry: () => void;
  /** 다시 로그인 · 사용량 권한 다시 허락 */
  onReconnect: (reconsent: boolean) => void;
}) {
  const grey = e.kind === "offline" || e.kind === "unavailable";
  const [icon, title, text]: [IconName, string, string] =
    e.kind === "limit" ? ["gauge", "사용량 한도에 도달했어요", "ChatGPT 설정에서 요금제나 이 앱의 한도를 확인해 주세요."]
    : e.kind === "expired" ? ["alert", "ChatGPT 로그인이 만료됐어요", "다시 로그인하면 이어서 쓸 수 있어요. 이 카드는 아직 보내지 않았어요."]
    : e.kind === "offline" ? ["wifi-off", "인터넷에 연결되어 있지 않아요", "규칙으로 나눈 제안은 인터넷 없이도 그대로 쓸 수 있어요."]
    : e.kind === "bad_output" ? ["alert", "ChatGPT 답을 카드로 바꾸지 못했어요", "형식이 맞지 않아 쓰지 않았어요. 아래는 규칙으로 나눈 제안이에요."]
    : e.kind === "scope" ? ["alert", "사용량 권한이 필요해요", `Neo-Anki는 내 ChatGPT 요금제로만 요청해요. 다시 연결할 때 "사용량" 권한을 허락해 주세요.`]
    : e.kind === "not_eligible" ? ["alert", "이 요금제에서는 쓸 수 없어요", "지금 계정의 ChatGPT 요금제는 다른 앱에서 요금제로 요청하는 기능을 지원하지 않아요."]
    : e.kind === "unavailable" ? ["alert", "지금은 ChatGPT 요금제를 쓸 수 없어요", "잠시 뒤 다시 해 보세요. 규칙으로 나눈 제안은 그대로 쓸 수 있어요."]
    : ["alert", "ChatGPT로 묶지 못했어요", e.message];
  const rule = <Button variant="ghost" size="M" label="규칙으로 나누기" onClick={onRule} />;
  return (
    <div className={`ai-error ${grey ? "is-grey" : ""}`} role="alert">
      <span className="ai-error__title">
        <Icon name={icon} size={18} />
        <span className="t-body-m-strong">{title}</span>
      </span>
      <p className="t-body-s c-secondary">{text}</p>
      <span className="ai-error__actions">
        {e.kind === "limit" ? (
          <>
            <Button size="M" icon="external" label="사용량 관리" onClick={openUsage} />
            {rule}
          </>
        ) : e.kind === "expired" || e.kind === "scope" ? (
          <>
            <Button size="M" icon="external" label={e.kind === "scope" ? "다시 연결" : "다시 로그인"} onClick={() => onReconnect(e.kind === "scope")} />
            {rule}
          </>
        ) : e.kind === "bad_output" ? (
          <Button variant="secondary" size="M" icon="rotate" label="다시 묶기" onClick={onRetry} />
        ) : e.kind === "not_eligible" ? (
          rule
        ) : (
          <>
            <Button variant="secondary" size="M" icon="rotate" label="다시 시도" onClick={onRetry} />
            {rule}
          </>
        )}
      </span>
    </div>
  );
}

/** L4 "ChatGPT 요금제 사용 · 사용량 관리 ↗" (OpenAI 지침: 쓰는 곳 가까이에 늘 보인다) */
export function UsingPlan() {
  return (
    <span className="ai-hint t-caption c-tertiary">
      <Icon name="gauge" size={14} />
      ChatGPT 요금제 사용 ·
      <button type="button" className="ai-link t-label-s" onClick={openUsage}>
        사용량 관리
        <Icon name="external" size={12} />
      </button>
    </span>
  );
}
