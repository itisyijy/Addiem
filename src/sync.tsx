// K Google Drive 보관·동기화 (Figma Row K: K1–K10). 일은 Rust(src-tauri/src/sync)가 하고, 상태는 "sync-status" 이벤트로 온다.
// 이 파일: 상태 공유(useSyncStatus) · 상단 바 버튼(K5) · 설정 칸(K1 / K1-n / K4) · 빈 홈 입구(K6) · 창들(K2 K3 K6-f K6-m K7 K8 K9 K10)
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import {
  api, errorText, syncApi,
  type DriveBackup, type RestorePreview, type SyncAuthEvent, type SyncConflict, type SyncJoinMode, type SyncProbe, type SyncStatus,
} from "./api";
import { Badge, Button, Checkbox, Icon, Input, Segment, Spinner, Toggle, type IconName } from "./ui";
import { useToast } from "./toast";
import { n, useKey, useModalScope, useNav, useOverlayScope } from "./nav";
import { useFocusTrap } from "./screens/NewDeck";
import "./styles/sync.css";

// ---------------------------------------------------------------- 상태 (화면 여러 곳이 같은 값을 본다)

let current: SyncStatus | null = null;
const subs = new Set<(s: SyncStatus) => void>();
let started = false;
const publish = (s: SyncStatus) => {
  current = s;
  subs.forEach((f) => f(s));
};
export const refreshSync = () => syncApi.status().then(publish, () => {});

export function useSyncStatus() {
  const [s, setS] = useState(current);
  useEffect(() => {
    subs.add(setS);
    if (!started) {
      started = true;
      refreshSync();
      listen<SyncStatus>("sync-status", (e) => publish(e.payload));
    } else if (current) setS(current);
    return () => {
      subs.delete(setS);
    };
  }, []);
  return s;
}

/** 창 하나를 연다 (SyncHost 가 그린다) */
type Flow =
  | { kind: "connect"; from?: "home" | "settings" | "login" }
  | { kind: "setup"; probe: SyncProbe; note?: string }
  | { kind: "found"; probe: SyncProbe }
  | { kind: "choice"; probe: SyncProbe | null }
  | { kind: "conflicts" }
  | { kind: "restore"; backup: DriveBackup }
  | { kind: "disconnect" }
  /** restart: 잠근 기기가 지운 Drive 를 이 기기 데이터로 다시 시작 (같은 암호로 잠근 채, T3-11) */
  | { kind: "unlock"; restart?: boolean };
let openFlow: (f: Flow | null) => void = () => {};
export const syncFlow = (f: Flow) => openFlow(f);

// ---------------------------------------------------------------- 글자

const DAY = 86_400_000;
const WEEKDAY = ["일", "월", "화", "수", "목", "금", "토"];
const hm = (d: Date) => `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
const md = (d: Date) => `${d.getMonth() + 1}월 ${d.getDate()}일`;
const dayStart = (ms: number) => new Date(new Date(ms).toDateString()).getTime();

/** 오늘 09:12 · 어제 22:10 · 10월 12일 22:40. secs: 같은 분에 고친 두 버전을 가리려고 초까지 (K7) */
export function when(ms: number, secs = false) {
  const d = new Date(ms);
  const t = secs ? `${hm(d)}:${String(d.getSeconds()).padStart(2, "0")}` : hm(d);
  const days = Math.round((dayStart(Date.now()) - dayStart(ms)) / DAY);
  return days === 0 ? `오늘 ${t}` : days === 1 ? `어제 ${t}` : `${md(d)} ${t}`;
}
/** 맞추지 못했을 때: 변경은 이 기기에 남아 있다는 것을 먼저 (T3-14) */
const failText = (s: SyncStatus) =>
  s.pending > 0 ? `아직 올리지 못한 변경 ${n(s.pending)}건은 이 기기에 저장돼 있어요. 다음 동기화 때 올려요.` : "이 기기 데이터는 그대로예요. 다음 동기화 때 다시 맞춰요.";

/** 오류 원문(OS 오류 등)은 접어 두고 [자세히]를 눌렀을 때만 보인다 */
function Detail({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      {" "}
      <button type="button" className="sync-detail t-caption" aria-expanded={open} onClick={() => setOpen(!open)}>
        {open ? "접기" : "자세히"}
      </button>
      {open && <span className="sync-detail__text"> {text}</span>}
    </>
  );
}
/** 방금 · 2분 전 · 3시간 전 · 어제 22:10 */
export function ago(ms: number) {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "방금";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}분 전`;
  if (diff < 12 * 3_600_000) return `${Math.floor(diff / 3_600_000)}시간 전`;
  return when(ms);
}
const backupDate = (ms: number) => {
  const d = new Date(ms);
  return `${md(d)} (${WEEKDAY[d.getDay()]}) ${hm(d)}`;
};
// 1MB 아래는 KB (처음 쓰는 계정의 백업 17KB 가 "0.0MB" 로 보였다)
export const size = (b: number) =>
  b >= 1e9 ? `${(b / 1e9).toFixed(1)}GB` : b >= 1e8 ? `${Math.round(b / 1e6)}MB` : b >= 1e6 ? `${(b / 1e6).toFixed(1)}MB` : `${Math.ceil(b / 1e3)}KB`;
const counts = (c: { decks: number; cards: number; reviews: number } | null | undefined) =>
  c ? `덱 ${n(c.decks)}개 · 카드 ${n(c.cards)}장 · 복습 기록 ${n(c.reviews)}건` : "덱 · 카드 수를 아직 몰라요";
/** 은/는 */
const eun = (w: string) => {
  const c = w.charCodeAt(w.length - 1);
  return c >= 0xac00 && c <= 0xd7a3 && (c - 0xac00) % 28 ? "은" : "는";
};
const others = (s: SyncStatus | null) => (s?.devices ?? []).filter((d) => !d.me && !d.stale).map((d) => d.name);

/** 상단 바(K5)와 설정(K4)이 같은 말을 쓴다 */
function line(s: SyncStatus): { title: string; icon: IconName; tone: "ok" | "plain" | "warn" } {
  switch (s.state) {
    case "syncing": {
      const verb = s.phase === "push" || s.phase === "snapshot" ? "올리는 중" : s.phase === "media" ? "그림 맞추는 중" : s.phase === "backup" ? "Drive에 백업하는 중" : s.phase === "restore" ? "백업으로 복구하는 중" : "맞추는 중";
      return { title: s.total > 1 ? `${verb} · ${n(s.done)} / ${n(s.total)}` : verb, icon: "cloud", tone: "plain" };
    }
    case "offline":
      return { title: s.pending > 0 ? `오프라인 · 아직 올리지 못한 변경 ${n(s.pending)}건 · 이 기기에 저장됨` : "오프라인 · 연결되면 저절로 맞춰요", icon: "cloud-off", tone: "plain" };
    case "login":
      return { title: "다시 로그인이 필요해요", icon: "alert", tone: "warn" };
    case "quota":
      return { title: "Drive 공간이 부족해요", icon: "alert", tone: "warn" };
    case "paused":
      return { title: "동기화가 꺼져 있어요", icon: "cloud-off", tone: "plain" };
    case "choice":
      return { title: "다른 기기에서 Drive 데이터를 바꿨어요", icon: "cloud", tone: "plain" };
    case "gone":
      return { title: "Drive의 Neo-Anki 데이터가 없어졌어요", icon: "cloud-off", tone: "plain" };
    case "locked":
      return { title: "Drive 데이터가 암호로 잠겨 있어요", icon: "lock", tone: "plain" };
    case "update":
      return { title: "앱을 업데이트해 주세요", icon: "info", tone: "plain" };
    case "error":
      return { title: "이번에는 맞추지 못했어요", icon: "cloud-off", tone: "plain" };
    default:
      return { title: s.last_sync ? `동기화됨 · ${ago(s.last_sync)}` : "곧 맞춰요", icon: "cloud-check", tone: "ok" };
  }
}

// ---------------------------------------------------------------- 창 틀

/** 머리 없는 창 (K2 · K6-f · K8 · K9 · K10 · ai.tsx 의 L2 · L3). 초점을 가두고 Esc 로 닫는다 */
export function Sheet({ label, onClose, className = "", children }: { label: string; onClose: () => void; className?: string; children: ReactNode }) {
  useModalScope();
  useKey("Escape", onClose, true, { modal: true });
  const box = useRef<HTMLDivElement>(null);
  useFocusTrap(box);
  // 바깥을 눌러도 닫지 않는다 (X · 취소 · Esc 만). 브라우저에서 로그인하고 앱 창을 눌러 돌아오면 Windows 는 그 클릭을
  // 웹뷰에도 보내서, 방금 뜬 K3 가 닫히고 K2 는 로그인 기다리기를 취소했다
  return (
    <div className="scrim">
      <div ref={box} tabIndex={-1} className={`modal sync-modal ${className}`} role="dialog" aria-modal="true" aria-label={label}>
        {children}
      </div>
    </div>
  );
}

export function Head({ title, sub, onClose }: { title: string; sub?: ReactNode; onClose?: () => void }) {
  return (
    <header className="sync-head">
      <span className="sync-head__text">
        <h2 className="t-title-m">{title}</h2>
        {sub && <span className="t-body-s c-secondary">{sub}</span>}
      </span>
      {onClose && (
        <button className="modal__close c-tertiary" aria-label="닫기" onClick={onClose}>
          <Icon name="x" size={20} />
        </button>
      )}
    </header>
  );
}

function Option({ on, onPick, icon, title, badge, text }: { on: boolean; onPick: () => void; icon?: IconName; title: string; badge?: string; text: string }) {
  return (
    <label className={`import__option ${on ? "is-on" : ""}`}>
      <input type="radio" checked={on} onChange={onPick} />
      <span className="import__radio" />
      <span className="import__option-text">
        <span className="import__option-title">
          {icon && <Icon name={icon} size={16} />}
          <span className="t-body-m-strong">{title}</span>
          {badge && <Badge tone="accent">{badge}</Badge>}
        </span>
        <span className="t-body-s c-secondary">{text}</span>
      </span>
    </label>
  );
}

// ---------------------------------------------------------------- K5 상단 바

/** TopNav 의 설정 버튼 왼쪽. 연결하지 않았으면 없다. 할 일이 있을 때만 노란 알약, 빨강은 쓰지 않는다 */
export function SyncButton() {
  const s = useSyncStatus();
  const [open, setOpen] = useState(false);
  useOverlayScope(open);
  useKey("Escape", () => setOpen(false), open, { modal: true });
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);
  if (!s?.connected) return null;
  const l = line(s);
  const pill = s.state === "login" ? "다시 로그인" : s.state === "quota" ? "Drive 공간 부족" : null;
  return (
    <span className="menu" onClick={(e) => e.stopPropagation()}>
      {pill ? (
        <button className="sync-pill t-label-m" aria-expanded={open} onClick={() => setOpen(!open)}>
          <Icon name="alert" size={16} />
          {pill}
        </button>
      ) : (
        <button
          className={`topnav__settings sync-btn ${s.state === "syncing" ? "is-syncing" : ""}`}
          aria-label={`Google Drive · ${l.title}`}
          title={l.title}
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          <Icon name={l.icon === "alert" ? "cloud" : l.icon} size={18} className="c-secondary" />
          {s.state === "offline" && s.pending > 0 && <span className="sync-btn__count t-label-s">{s.pending > 99 ? "99+" : s.pending}</span>}
        </button>
      )}
      {open && <SyncPopover s={s} close={() => setOpen(false)} />}
    </span>
  );
}

function SyncPopover({ s, close }: { s: SyncStatus; close: () => void }) {
  const nav = useNav();
  const toast = useToast();
  const l = line(s);
  const act = (f: () => unknown) => () => {
    close();
    f();
  };
  let text: ReactNode = "열 때 받고, 학습을 마치면 올리고, 닫을 때 마저 올려요.";
  let actions: ReactNode = (
    <>
      <Button variant="ghost" size="M" label="설정 열기" onClick={act(() => nav({ name: "settings" }))} />
      <Button
        variant="secondary"
        size="M"
        icon="rotate"
        label="지금 동기화"
        disabled={s.state === "syncing" || s.studying}
        onClick={act(() => syncApi.now().catch((e) => toast({ title: errorText(e) })))}
      />
    </>
  );
  if (s.state === "login") {
    text = `Google 로그인이 만료됐어요. 아직 올리지 못한 변경 ${n(s.pending)}건은 이 기기에 저장돼 있어요.`;
    actions = (
      <>
        <Button variant="ghost" size="M" label="나중에" onClick={close} />
        <Button size="M" icon="external" label="다시 로그인" onClick={act(() => syncFlow({ kind: "connect", from: "login" }))} />
      </>
    );
  } else if (s.state === "quota") {
    text = s.quota
      ? `${size(s.quota[1])} 중 ${size(s.quota[0])}를 쓰고 있어요. Neo-Anki는 ${size(s.usage ?? 0)}를 써요. 공간이 생기면 이어서 올려요.`
      : "Drive 공간이 모자라 백업을 멈췄어요. 공간이 생기면 이어서 올려요.";
    actions = (
      <>
        <Button
          variant="ghost"
          size="M"
          label="오래된 백업 정리"
          onClick={act(() => syncApi.pruneBackups().then((k) => toast({ title: k ? `오래된 백업 ${k}개를 지웠어요` : "지울 백업이 없어요" }), (e) => toast({ title: errorText(e), tone: "error" })))}
        />
        <Button variant="secondary" size="M" icon="external" label="Drive 열기" onClick={act(() => syncApi.openFolder())} />
      </>
    );
  } else if (s.state === "paused") {
    text = "연결은 되어 있어요. 동기화를 시작하면 이 기기 데이터를 Drive에 올려요.";
    actions = <Button size="M" icon="cloud" label="동기화 시작" onClick={act(startFromProbe)} />;
  } else if (s.state === "choice") {
    text = "Drive 데이터를 어떻게 할지 고르면 다시 맞춰요. 그때까지 이 기기 데이터는 그대로예요.";
    actions = <Button size="M" label="고르기" onClick={act(() => syncFlow({ kind: "choice", probe: null }))} />;
  } else if (s.state === "gone") {
    text = "다른 기기나 Drive 설정에서 지워졌어요. 이 기기 데이터는 그대로예요.";
    actions = (
      <>
        <Button variant="ghost" size="M" label="연결 끊기" onClick={act(() => syncFlow({ kind: "disconnect" }))} />
        <Button
          size="M"
          label="이 기기 데이터로 다시 시작"
          onClick={act(() =>
            syncApi.join({ mode: "fresh" }).catch((e) =>
              // 잠근 기기: 이 기기에 남긴 잠금이 없으면 암호를 물어 잠근 채 다시 시작한다 (잠금을 말없이 풀지 않는다)
              s.lock ? syncFlow({ kind: "unlock", restart: true }) : toast({ title: errorText(e), tone: "error" }),
            ),
          )}
        />
      </>
    );
  } else if (s.state === "locked") {
    text = "이 기기에서 Drive 데이터를 열려면 암호가 필요해요.";
    actions = <Button size="M" icon="lock" label="암호 입력" onClick={act(() => syncFlow({ kind: "unlock" }))} />;
  } else if (s.state === "error") {
    text = (
      <>
        {failText(s)}
        {s.error && <Detail text={s.error} />}
      </>
    );
  } else if (s.state === "update") {
    text = s.error ?? "";
  }
  const devices = s.state === "idle" || s.state === "syncing" || s.state === "offline" ? s.devices.slice(0, 4) : [];
  return (
    <div className="sync-pop" role="dialog" aria-label={l.title}>
      <span className="sync-pop__title">
        <Icon name={l.icon} size={18} className={l.tone === "ok" ? "c-success" : l.tone === "warn" ? "c-warn" : "c-secondary"} />
        <span className="t-body-m-strong">{l.title}</span>
      </span>
      <p className="t-caption c-tertiary">{text}</p>
      {devices.map((d) => (
        <span key={d.uid} className="sync-pop__device">
          <Icon name="laptop" size={16} className="c-secondary" />
          <span className="t-body-s">{d.me ? `${d.name} (이 기기)` : d.name}</span>
          <span className="t-caption c-tertiary">{d.stale ? "오래 안 씀" : d.at ? (d.me ? ago(d.at) : when(d.at)) : "아직"}</span>
        </span>
      ))}
      <span className="sync-pop__actions">{actions}</span>
    </div>
  );
}

/** 연결은 했지만 합류하지 않았을 때 ([나중에] 뒤) */
async function startFromProbe() {
  try {
    const probe = await syncApi.probe();
    syncFlow(nextFlow(probe));
  } catch (e) {
    syncFlow({ kind: "connect", from: "settings" });
    void e;
  }
}

function nextFlow(probe: SyncProbe, from?: string): Flow {
  if (!probe.remote) return { kind: "setup", probe, note: from === "home" ? "이 계정에는 아직 Neo-Anki 데이터가 없어요" : undefined };
  return probe.local.empty ? { kind: "found", probe } : { kind: "choice", probe };
}

// ---------------------------------------------------------------- 창을 그리는 곳 (App 에 하나)

export function SyncHost({ studying }: { studying: boolean }) {
  const s = useSyncStatus();
  const toast = useToast();
  const [flow, setFlow] = useState<Flow | null>(null);
  // 렌더마다 다시 건다 (개발 중 모듈을 바꿔 끼워도 연결이 끊기지 않게)
  useEffect(() => {
    openFlow = setFlow;
  });
  // 학습 화면이 동기화를 멈춘다. 나오면(학습을 마치면) 받기 -> 올리기
  useEffect(() => {
    syncApi.pause(studying).catch(() => {});
  }, [studying]);
  // K7 토스트: 새 기록이 생기면 한 번. 학습 중이면 끝난 뒤에 (5초)
  const shown = useRef<number | null>(null);
  useEffect(() => {
    if (!s || studying) return;
    if (shown.current === null) shown.current = s.conflicts > 0 ? 0 : s.conflict_seq;
    if (s.conflicts > 0 && s.conflict_seq > shown.current) {
      shown.current = s.conflict_seq;
      toast({
        title: "다른 기기와 맞췄어요",
        text: `카드 ${n(s.conflicts)}장은 두 기기에서 고쳐서 나중에 고친 쪽 내용으로 맞췄어요`,
        icon: "cloud-check",
        ms: 5000,
        action: { label: "보기", onClick: () => setFlow({ kind: "conflicts" }) },
      });
      syncApi.conflictsSeen().catch(() => {});
    }
  }, [s, studying, toast]);
  // 다른 기기가 되돌리기 · 새로 시작을 했다: 몰래 덮지 않고 고르게 한다 (학습이 끝난 뒤)
  const asked = useRef(0);
  useEffect(() => {
    if (s?.state === "choice" && s.choice && !studying && asked.current !== s.choice.gen && !flow) {
      asked.current = s.choice.gen;
      setFlow({ kind: "choice", probe: null });
    }
  }, [s, studying, flow]);
  const close = () => setFlow(null);
  return (
    <>
      {s?.closing && <CloseModal s={s} />}
      {flow?.kind === "connect" && <ConnectModal from={flow.from} onDone={setFlow} />}
      {flow?.kind === "setup" && <SetupModal probe={flow.probe} note={flow.note} onClose={close} />}
      {flow?.kind === "found" && <FoundModal probe={flow.probe} onClose={close} />}
      {flow?.kind === "choice" && <ChoiceModal probe={flow.probe} onClose={close} />}
      {flow?.kind === "conflicts" && <ConflictsModal onClose={close} />}
      {flow?.kind === "restore" && <RestoreModal backup={flow.backup} onClose={close} />}
      {flow?.kind === "disconnect" && <DisconnectModal onClose={close} />}
      {flow?.kind === "unlock" && <UnlockModal restart={flow.restart} onClose={close} />}
    </>
  );
}

// ---------------------------------------------------------------- K2 브라우저 로그인 기다림

function ConnectModal({ from, onDone }: { from?: "home" | "settings" | "login"; onDone: (f: Flow | null) => void }) {
  const [url, setUrl] = useState<string | null>(null);
  const [fail, setFail] = useState<SyncAuthEvent | null>(null);
  const [copied, setCopied] = useState(false);
  const waiting = useRef(true);
  const toast = useToast();
  const start = useCallback(async () => {
    setFail(null);
    setCopied(false);
    waiting.current = true;
    try {
      setUrl(await syncApi.connect());
    } catch (e) {
      waiting.current = false;
      setFail({ ok: false, reason: "error", message: errorText(e), probe: null });
    }
  }, []);
  useEffect(() => {
    let alive = true;
    let un: (() => void) | undefined;
    listen<SyncAuthEvent>("sync-auth", (e) => {
      if (!alive) return;
      waiting.current = false;
      const r = e.payload;
      if (r.ok && r.probe) {
        // 다시 로그인: 이어서 맞춘다. 처음 연결: Drive 에 무엇이 있는지 보고 K3 · K6-f · K6-m
        if (current?.connected) {
          syncApi.now().catch(() => {});
          toast({ title: "다시 로그인했어요", text: "이어서 맞춰요" });
          onDone(null);
        } else onDone(nextFlow(r.probe, from));
      } else if (r.reason === "cancelled") onDone(null);
      else setFail(r);
    }).then((f) => {
      un = f;
      if (alive) start();
    });
    return () => {
      alive = false;
      un?.();
      if (waiting.current) syncApi.cancelConnect().catch(() => {});
    };
  }, [from, onDone, start, toast]);
  const cancel = () => {
    syncApi.cancelConnect().catch(() => {});
    onDone(null);
  };
  const copy = async () => {
    if (!url) return;
    await navigator.clipboard.writeText(url).then(() => setCopied(true), () => {});
  };
  if (fail) {
    const head =
      fail.reason === "scopes" ? "권한 2개가 모두 필요해요"
      : fail.reason === "timeout" ? "시간이 지났어요"
      : fail.reason === "denied" ? "로그인을 취소했어요"
      : "연결하지 못했어요";
    const text =
      fail.reason === "scopes" ? "Google 화면에서 두 권한을 모두 허용해야 동기화할 수 있어요. 하나만으로는 동작하지 않아요."
      : fail.reason === "timeout" ? "5분 안에 로그인을 마치지 않아 기다리기를 멈췄어요."
      : fail.message ?? "";
    return (
      <Sheet label={head} onClose={cancel} className="sync-k2">
        <span className="sync-k2__icon">
          <Icon name="alert" size={24} className="c-warn" />
        </span>
        <h2 className="t-title-m">{head}</h2>
        <p className="t-body-s c-secondary sync-center">{text}</p>
        <div className="sync-foot sync-foot--end">
          <Button variant="ghost" size="M" label="취소" onClick={cancel} />
          <Button size="M" icon="external" label={fail.reason === "scopes" ? "다시 연결" : "다시 시도"} onClick={start} />
        </div>
      </Sheet>
    );
  }
  return (
    <Sheet label="브라우저에서 Google 로그인을 마쳐 주세요" onClose={cancel} className="sync-k2">
      <Spinner size={36} />
      <h2 className="t-title-m">{url === null && current?.folder ? "개발용 폴더에 연결하는 중" : "브라우저에서 Google 로그인을 마쳐 주세요"}</h2>
      <p className="t-body-s c-secondary sync-center">기본 브라우저에 로그인 창을 열었어요. 계정을 고르고 권한을 허용하면 이 창으로 자동으로 돌아와요.</p>
      <div className="sync-box">
        <span className="t-label-s c-tertiary">Neo-Anki가 요청하는 권한 2개</span>
        <span className="sync-perm">
          <Icon name="folder" size={16} className="c-secondary" />
          <span className="sync-perm__text">
            <span className="t-body-s">Neo-Anki가 만든 파일만 보고 관리하기</span>
            <span className="t-caption c-tertiary">"Neo-Anki 백업" 폴더</span>
          </span>
        </span>
        <span className="sync-perm">
          <Icon name="lock" size={16} className="c-secondary" />
          <span className="sync-perm__text">
            <span className="t-body-s">앱 전용 숨김 공간 쓰기</span>
            <span className="t-caption c-tertiary">동기화 데이터 · Drive 화면에는 안 보여요</span>
          </span>
        </span>
        <span className="t-caption c-secondary">Drive의 다른 파일은 보지도, 고치지도 못해요.</span>
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

// ---------------------------------------------------------------- K3 처음 설정

type Backup = "daily" | "weekly" | "off";
const BACKUPS: { value: Backup; label: string }[] = [
  { value: "daily", label: "매일" },
  { value: "weekly", label: "매주" },
  { value: "off", label: "끄기" },
];

/** 암호 두 칸 + 잊으면 복구할 수 없다는 확인. 맞으면 비밀번호, 아니면 null */
function usePassword() {
  const [pw, setPw] = useState("");
  const [again, setAgain] = useState("");
  const [agree, setAgree] = useState(false);
  const problem = pw.length < 8 ? "8자 이상으로 정해 주세요" : pw !== again ? "두 칸이 같지 않아요" : !agree ? "복구할 수 없다는 것을 확인해 주세요" : null;
  const fields = (
    <div className="sync-pw">
      <Input type="password" label="암호" value={pw} autoComplete="new-password" onChange={(e) => setPw(e.target.value)} helper="8자 이상" />
      <Input type="password" label="암호 확인" value={again} autoComplete="new-password" onChange={(e) => setAgain(e.target.value)} />
      <Checkbox checked={agree} onChange={setAgree} label="잊으면 Drive 데이터를 복구할 수 없어요 (이 기기 데이터는 그대로)" />
    </div>
  );
  return { pw, problem, fields };
}

function SetupModal({ probe, note, onClose }: { probe: SyncProbe; note?: string; onClose: () => void }) {
  const toast = useToast();
  const [name, setName] = useState(probe.device_name);
  const [backup, setBackup] = useState<Backup>("daily");
  const [lock, setLock] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [dbBytes, setDbBytes] = useState(0);
  const pw = usePassword();
  useEffect(() => {
    api.appInfo().then((i) => setDbBytes(i.db_bytes), () => {});
  }, []);
  const go = async (later: boolean) => {
    if (!later && lock && pw.problem) return setError(pw.problem);
    setBusy(true);
    setError(undefined);
    try {
      await syncApi.join({ mode: "new", device_name: name, backup, password: lock && !later ? pw.pw : undefined, later });
      await refreshSync();
      onClose();
      if (!later) toast({ title: "Google Drive에 연결했어요", text: "올리는 동안 계속 써도 돼요", icon: "cloud-check" });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet label="동기화 설정" onClose={onClose} className="sync-k3">
      <Head
        title="동기화 설정"
        onClose={onClose}
        sub={
          <span className="sync-account">
            <span className="sync-avatar t-label-s">{probe.account.slice(0, 1).toUpperCase()}</span>
            <span className="t-body-s c-secondary">{probe.account} 에 연결했어요</span>
          </span>
        }
      />
      {note && <p className="sync-note t-body-s c-secondary">{note} · 이 기기 데이터로 시작해요</p>}
      <DeviceName value={name} onChange={setName} />
      <Uploads title="올릴 것" db={dbBytes} media={probe.local.media_bytes} />
      <BackupPick value={backup} onPick={setBackup} />
      <LockBox on={lock} onChange={setLock}>
        {lock && pw.fields}
      </LockBox>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="sync-foot">
        <span className="t-caption c-tertiary">처음 올릴 양 약 {size(dbBytes + probe.local.media_bytes)} · 올리는 동안 계속 써도 돼요</span>
        <span className="sync-foot__actions">
          <Button variant="ghost" size="M" label="나중에" disabled={busy} onClick={() => go(true)} />
          <Button size="M" icon="cloud" label="동기화 시작" busy={busy} busyLabel="올리는 중…" onClick={() => go(false)} />
        </span>
      </div>
    </Sheet>
  );
}

function Upload({ icon, tone, title, text, muted }: { icon: IconName; tone: string; title: string; text: string; muted?: boolean }) {
  return (
    <span className={`sync-list__row ${muted ? "c-tertiary" : ""}`}>
      <Icon name={icon} size={18} className={tone} />
      <span className="sync-row__text">
        <span className="t-body-m">{title}</span>
        <span className="t-caption c-tertiary">{text}</span>
      </span>
    </span>
  );
}

/** K3 "올릴 것" · K4 "올리는 것". 고를 수 있는 것이 없어 스위치를 그리지 않는다 (K20) */
function Uploads({ title, db, media }: { title: string; db: number | null; media: number | null }) {
  const always = (b: number | null) => (b === null ? "항상" : `항상 · ${size(b)}`);
  return (
    <div className="sync-field">
      <span className="t-label-m">{title}</span>
      <div className="sync-list">
        <Upload icon="cloud-check" tone="c-success" title="덱 · 카드 · 복습 기록 · 설정" text={always(db)} />
        <Upload icon="cloud-check" tone="c-success" title="그림 · 미디어" text={always(media)} />
        <Upload icon="file" tone="c-secondary" title="원자료 파일 (PDF · PPTX)" text="올리지 않아요 · 고르는 것은 다음 버전에서" />
        <Upload icon="cloud-off" tone="c-tertiary" title="분석 모델" text="올리지 않아요 · 새 기기에서 다시 받으면 돼요" muted />
      </div>
    </div>
  );
}

function BackupPick({ value, onPick }: { value: Backup; onPick: (v: Backup) => void }) {
  return (
    <div className="sync-row">
      <span className="sync-row__text">
        <span className="t-label-m">Drive에 자동 백업</span>
        <span className="t-caption c-tertiary">최근 7일 하루 하나 · 그 전 주 하나 · 최대 12개</span>
      </span>
      <Segment boxed label="Drive에 자동 백업" options={BACKUPS} value={value} onChange={onPick} />
    </div>
  );
}

function LockBox({ on, onChange, disabled, busy, children }: { on: boolean; onChange: (v: boolean) => void; disabled?: boolean; busy?: boolean; children?: ReactNode }) {
  return (
    <div className="sync-lock">
      <Icon name="lock" size={18} className="c-secondary" />
      <span className="sync-row__text">
        <span className="t-body-m-strong">암호로 잠그기</span>
        <span className="t-caption c-tertiary">
          켜면 이 기기에서 암호화한 뒤 올려요. Google도 내용을 볼 수 없어요. 암호를 잊으면 Drive에 올린 데이터는 열 수 없어요 (이 기기 데이터는 그대로).
        </span>
        {children}
      </span>
      <span className="sync-lock__end">
        {busy && <span className="t-caption c-secondary">다시 올리는 중</span>}
        <Toggle on={on} onChange={onChange} label="암호로 잠그기" disabled={disabled} />
      </span>
    </div>
  );
}

/** onSave 가 있으면 K4: Enter · 칸 밖에서 저장, Esc 는 되돌림 (K20) */
function DeviceName({ value, onChange, onSave, onRevert }: { value: string; onChange: (v: string) => void; onSave?: () => void; onRevert?: () => void }) {
  return (
    <label className="sync-field">
      <span className="t-label-m">이 기기 이름</span>
      <span className="sync-name">
        <Icon name="laptop" size={16} className="c-secondary" />
        <input
          className="t-body-m"
          value={value}
          maxLength={40}
          onChange={(e) => onChange(e.target.value)}
          onBlur={onSave}
          onKeyDown={(e) => {
            if (!onSave) return;
            // 한글 조합 중 Enter 는 조합만 끝낸다 (마지막 글자가 아직 value 에 없다)
            if (e.key === "Enter" && !e.nativeEvent.isComposing) e.currentTarget.blur();
            else if (e.key === "Escape") {
              e.preventDefault();
              onRevert?.();
            }
          }}
        />
      </span>
      <span className="t-caption c-tertiary">다른 기기에서 "어디서 마지막으로 공부했는지" 보여줄 때 써요</span>
    </label>
  );
}

// ---------------------------------------------------------------- K6 빈 홈 입구 · K6-f 찾은 데이터

/** 빈 홈(A1) 두 카드 아래 한 줄. Google 연결이 없는 빌드나 이미 연결했으면 숨긴다 (K1-n) */
export function DriveLoadLink() {
  const s = useSyncStatus();
  if (!s?.configured || s.connected) return null;
  return (
    <div className="sync-load">
      <span className="t-body-s c-tertiary">다른 컴퓨터에서 쓰던 데이터가 있나요?</span>
      <button className="sync-load__btn t-label-m c-accent" onClick={() => syncFlow({ kind: "connect", from: "home" })}>
        <Icon name="cloud" size={16} />
        Google Drive에서 불러오기
        <Icon name="chevron-right" size={14} scaled />
      </button>
    </div>
  );
}

function FoundModal({ probe, onClose }: { probe: SyncProbe; onClose: () => void }) {
  const nav = useNav();
  const toast = useToast();
  const [name, setName] = useState(probe.device_name);
  const [password, setPassword] = useState("");
  const [wrong, setWrong] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const r = probe.remote!;
  const join = async (mode: SyncJoinMode) => {
    setBusy(true);
    setError(undefined);
    try {
      await syncApi.join({ mode, device_name: name, password: r.locked && mode === "take" ? password : undefined });
      await refreshSync();
      onClose();
      nav({ name: "home" });
      toast({ title: mode === "take" ? "Drive 데이터를 받았어요" : "이 기기 데이터로 새로 시작했어요", text: "그림은 쓰는 동안 받아요", icon: "cloud-check" });
    } catch (e) {
      const msg = errorText(e);
      if (msg.includes("암호")) setWrong((w) => w + 1);
      setError(msg);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet label="Drive에서 Neo-Anki 데이터를 찾았어요" onClose={onClose} className="sync-k6f">
      <span className="sync-found__head">
        <span className="sync-tile sync-tile--accent">
          <Icon name="cloud-check" size={22} className="c-accent" />
        </span>
        <span className="sync-head__text">
          <h2 className="t-title-m">Drive에서 Neo-Anki 데이터를 찾았어요</h2>
          <span className="t-caption c-tertiary">{probe.account}</span>
        </span>
      </span>
      <div className="sync-table">
        <span>
          <span className="t-body-s c-secondary">마지막으로 맞춘 기기</span>
          <span className="t-body-m-strong">{r.device ? `${r.device} · ${r.at ? when(r.at) : ""}` : "알 수 없음"}</span>
        </span>
        <span>
          <span className="t-body-s c-secondary">덱 · 카드</span>
          <span className="t-body-m-strong">{r.counts ? `${n(r.counts.decks)}개 · ${n(r.counts.cards)}장` : "—"}</span>
        </span>
        <span>
          <span className="t-body-s c-secondary">복습 기록</span>
          <span className="t-body-m-strong">{r.counts ? `${n(r.counts.reviews)}개` : "—"}</span>
        </span>
        <span>
          <span className="t-body-s c-secondary">그림 · 미디어</span>
          <span className="t-body-m-strong">{r.counts ? `${size(r.counts.media_bytes)} · 쓰는 동안 받아요` : "—"}</span>
        </span>
        <span>
          <span className="t-body-s c-secondary">원자료 파일</span>
          <span className="t-body-m-strong">올리지 않음</span>
        </span>
      </div>
      <DeviceName value={name} onChange={setName} />
      {r.locked && (
        <Input type="password" label="암호" value={password} autoFocus onChange={(e) => setPassword(e.target.value)} helper="Drive 데이터를 잠근 암호예요" />
      )}
      {wrong >= 3 ? (
        <div className="sync-note sync-note--row">
          <span className="t-body-s c-secondary">암호를 잊었다면 Drive 쪽 데이터는 열 수 없어요</span>
          <Button variant="secondary" size="M" label="이 기기에서 새로 시작" disabled={busy} onClick={() => join("fresh")} />
        </div>
      ) : (
        error && <p className="t-body-s c-danger">{error}</p>
      )}
      <div className="sync-foot">
        <span className="t-caption c-tertiary">카드와 기록을 먼저 받아 바로 공부할 수 있어요</span>
        <span className="sync-foot__actions">
          <Button variant="ghost" size="M" label="취소" onClick={onClose} />
          <Button size="M" icon="download" label="이 데이터로 시작" busy={busy} busyLabel="받는 중…" disabled={r.locked && !password} onClick={() => join("take")} />
        </span>
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- K6-m 이 기기에도 데이터가 있을 때 (처음 연결 · 다른 기기의 되돌리기 뒤)

function ChoiceModal({ probe: given, onClose }: { probe: SyncProbe | null; onClose: () => void }) {
  const s = useSyncStatus();
  const nav = useNav();
  const toast = useToast();
  const [probe, setProbe] = useState(given);
  const choice = given ? null : s?.choice ?? null;
  // 다른 기기가 잠그기만 바꿨으면 내용은 같은 데이터에 그 기기 변경까지 받은 것이라 합치기가 잃는 것이 없다 (T3-09).
  // 되돌리기 · 새로 시작 뒤라면 기본은 "Drive 데이터로 바꾸기"
  const lockKind = choice?.header?.reason?.kind === "lock" || choice?.header?.reason?.kind === "unlock";
  const suggest: SyncJoinMode = choice && !lockKind ? "take" : "merge";
  const [mode, setMode] = useState<SyncJoinMode>(suggest);
  const [name, setName] = useState(given?.device_name ?? s?.device_name ?? "");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!given) syncApi.probe().then(setProbe, (e) => setError(errorText(e)));
  }, [given]);
  const remote = probe?.remote;
  const header = choice?.header;
  const dev = header?.reason?.device_name ?? header?.device_name ?? remote?.device ?? "다른 기기";
  const kind = header?.reason?.kind;
  const title = !choice
    ? "이 기기에도 덱이 있어요"
    : kind === "restore"
      ? `${dev}에서 데이터를 ${header?.reason?.backup_at ? md(new Date(header.reason.backup_at)) : "예전"} 백업으로 복구했어요`
      : kind === "lock"
        ? "다른 기기에서 Drive 데이터를 암호로 잠갔어요"
        : kind === "unlock"
          ? "다른 기기에서 Drive 데이터의 암호 잠금을 풀었어요"
          : `${dev}에서 Drive 데이터를 새로 시작했어요`;
  const named = header?.reason?.device_name ?? header?.device_name;
  const sub = !choice
    ? `Drive에는 ${dev}에서 맞춘 데이터가 있어요. 어떻게 할까요?`
    : kind === "unlock"
      ? `${named ? `${named}에서` : "다른 기기에서"} 잠금을 풀고 Drive에 다시 올렸어요. 이 기기에 저장된 암호는 더 이상 쓰지 않아요. 어떻게 할까요?`
      : `이 기기에는 ${kind === "restore" ? "복구" : kind === "lock" ? "잠그기" : "바뀌기"} 전 데이터가 있어요. 어떻게 할까요?`;
  // 이 기기 잠금으로 새로 시작하면 암호가 필요 없다. 다른 기기가 잠근 Drive 를 새로 시작하면 그 암호로 잠근 채 (T3-11)
  const locked = (choice?.locked || remote?.locked) && !(mode === "fresh" && s?.lock);
  // 다시 연결하면 기기 목록 기억이 없다: Drive 의 기기 파일 이름 (T3-18)
  const others_ = (remote?.names?.length ? remote.names : others(s ?? null)).filter((n) => n !== probe?.device_name);
  const who = others_.length ? others_.join(", ") : dev;
  const labels: Record<SyncJoinMode, [string, IconName]> = { merge: ["합치기", "merge"], take: ["Drive 데이터로 바꾸기", "download"], fresh: ["Drive 새로 시작", "upload"], new: ["시작", "cloud"] };
  const go = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await syncApi.join({ mode, password: locked ? password : undefined, device_name: choice ? undefined : name });
      await refreshSync();
      onClose();
      nav({ name: "home" });
      toast({ title: mode === "merge" ? "두 데이터를 합쳤어요" : mode === "take" ? "Drive 데이터로 바꿨어요" : "이 기기 데이터로 Drive를 새로 시작했어요", text: "바꾸기 전 데이터는 이 기기 백업에 있어요", icon: "cloud-check" });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet label={title} onClose={onClose} className="sync-k6m">
      <Head title={title} sub={sub} onClose={onClose} />
      <div className="sync-sides">
        <span className="sync-side">
          <Icon name="laptop" size={18} className="c-secondary" />
          <span className="sync-row__text">
            <span className="t-label-m">이 기기 · {probe?.device_name ?? s?.device_name}</span>
            <span className="t-caption c-tertiary">{counts(probe?.local)}</span>
          </span>
        </span>
        <span className="sync-side">
          <Icon name="cloud" size={18} className="c-secondary" />
          <span className="sync-row__text">
            <span className="t-label-m">Drive · {dev}에서 {remote?.at ? when(remote.at) : header?.at ? when(header.at) : ""}</span>
            <span className="t-caption c-tertiary">{counts(remote?.counts ?? header?.counts)}</span>
          </span>
        </span>
      </div>
      <div className="import__choice">
        <Option
          on={mode === "merge"}
          onPick={() => setMode("merge")}
          icon="merge"
          title="두 데이터 합치기"
          badge={suggest === "merge" ? "권장" : undefined}
          text="덱·카드·복습 기록을 합쳐요. 같은 Anki 노트의 카드는 하나로 합치고, 내용이 다르면 나중에 고친 버전을 써요. 다른 버전은 30일 동안 ‘두 기기에서 고친 카드’에서 볼 수 있어요."
        />
        <Option on={mode === "take"} onPick={() => setMode("take")} icon="download" title="Drive 데이터로 바꾸기" badge={suggest === "take" ? "권장" : undefined} text="이 기기 데이터는 백업 파일로 남기고, Drive의 데이터를 받아요." />
        <Option
          on={mode === "fresh"}
          onPick={() => setMode("fresh")}
          icon="upload"
          title="이 기기 데이터로 Drive 새로 시작"
          text={`Drive의 동기화 데이터를 이 기기 것으로 바꿔요. ${who}${eun(who)} 다음에 동기화할 때 고르는 창을 봐요.`}
        />
      </div>
      {/* 처음 연결: K3 · K6-f 처럼 이 기기 이름을 정한다 (같은 PC 이름끼리 구분, T3-16) */}
      {!choice && <DeviceName value={name} onChange={setName} />}
      {locked && (
        <>
          <Input
            type="password"
            label="Drive 데이터 암호"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            // 이 기기가 잠근 채라면 [Drive 새로 시작]은 이 기기 키로 잠금을 이어 가 암호가 필요 없다
            helper={s?.lock ? "잠근 기기에서 정한 암호예요" : "잠근 기기에서 정한 암호예요 · 세 방법 모두 필요해요"}
          />
          <p className="t-caption c-tertiary">암호를 모르면 [취소] 뒤 설정에서 연결을 끊고 이 기기만 쓸 수 있어요</p>
        </>
      )}
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="sync-foot">
        <span className="t-caption c-tertiary">어느 쪽이든 바꾸기 전에 이 기기 백업을 먼저 만들어요</span>
        <span className="sync-foot__actions">
          <Button variant="ghost" size="M" label="취소" onClick={onClose} />
          <Button size="M" icon={labels[mode][1]} label={labels[mode][0]} busy={busy} busyLabel="맞추는 중…" disabled={!probe || (locked && !password)} onClick={go} />
        </span>
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- K7 두 기기에서 고친 카드

/** 다른 쪽과 다른 글자만 표시 (앞뒤 같은 부분을 뺀 가운데) */
function Diff({ text, other }: { text: string; other: string }) {
  if (text === other) return <>{text}</>;
  let a = 0;
  while (a < text.length && a < other.length && text[a] === other[a]) a++;
  let b = 0;
  while (b < text.length - a && b < other.length - a && text[text.length - 1 - b] === other[other.length - 1 - b]) b++;
  return (
    <>
      {text.slice(0, a)}
      <b className="sync-diff">{text.slice(a, text.length - b)}</b>
      {text.slice(text.length - b)}
    </>
  );
}

function Version({ v, other, current, picked, onUse, busy }: { v: SyncConflict["versions"][0]; other: SyncConflict["versions"][0]; current: boolean; picked?: boolean; onUse?: () => void; busy?: boolean }) {
  const secs = when(v.at) === when(other.at);
  const lines = v.fields.slice(1).join("\n").split("\n");
  const otherLines = other.fields.slice(1).join("\n").split("\n");
  return (
    <div className={`sync-version ${current ? "is-current" : ""}`}>
      <span className="sync-version__who">
        <Icon name="laptop" size={16} className="c-secondary" />
        <span className="t-label-m">{v.device}</span>
        <span className="t-caption c-tertiary">{when(v.at, secs)}</span>
        {current && <Badge tone="accent">지금 내용</Badge>}
      </span>
      {v.fields[0] !== other.fields[0] && (
        <p className="t-body-s">
          <Diff text={v.fields[0] ?? ""} other={other.fields[0] ?? ""} />
        </p>
      )}
      <div className="t-body-s sync-version__lines">
        {lines.map((l, i) => (
          <p key={i}>
            <Diff text={l} other={otherLines[i] ?? ""} />
          </p>
        ))}
      </div>
      {current ? (
        // 저절로 맞춘 쪽(이긴 버전, 고른 적 없음)만 "나중에 고친 쪽". 시각 비교로 고르지 않는다 (T3-02)
        <span className="t-caption c-accent">{v.winner && !picked ? "나중에 고친 쪽이라 이 내용으로 맞췄어요" : "직접 고른 내용이에요"}</span>
      ) : (
        <Button variant="secondary" size="M" icon="rotate" label="이 버전으로 바꾸기" busy={busy} onClick={onUse} />
      )}
    </div>
  );
}

function ConflictsModal({ onClose }: { onClose: () => void }) {
  const [list, setList] = useState<SyncConflict[] | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [busy, setBusy] = useState<number>();
  const [error, setError] = useState<string>();
  const load = useCallback(() => syncApi.conflicts().then((l) => {
    setList(l);
    setOpen((o) => o ?? l[0]?.id ?? null);
  }, (e) => setError(errorText(e))), []);
  useEffect(() => {
    load();
    syncApi.conflictsSeen().catch(() => {});
  }, [load]);
  const use = async (id: number) => {
    setBusy(id);
    try {
      await syncApi.useConflict(id);
      await load();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(undefined);
    }
  };
  return (
    <Sheet label="두 기기에서 고친 카드" onClose={onClose} className="sync-k7">
      <Head
        title={`두 기기에서 고친 카드 ${list ? n(list.length) : ""}장`}
        sub={
          // 저절로 맞춘 그대로인 카드만 있으면 규칙을, 직접 고른(또는 다른 기기에서 고른) 카드가 있으면 그 말을 빼고
          list?.some((c) => c.picked || !c.versions.some((v) => v.current && v.winner))
            ? "내용이 다른 카드예요. 지금 내용은 바꿀 수 있어요. 복습 기록은 양쪽 모두 남았어요."
            : "나중에 고친 쪽 내용을 지금 내용으로 썼어요. 다른 쪽 내용이 맞다면 바꿀 수 있어요. 복습 기록은 양쪽 모두 남았어요."
        }
        onClose={onClose}
      />
      <div className="sync-k7__list">
        {list?.length === 0 && <p className="t-body-s c-tertiary">지난 30일 동안 두 기기에서 같이 고친 카드가 없어요.</p>}
        {list?.map((c) => {
          const [cur, other] = c.versions[0].current ? c.versions : [c.versions[1], c.versions[0]];
          return open === c.id ? (
            <div key={c.id} className="sync-conflict">
              <span className="sync-row__text">
                <span className="t-body-m-strong">{c.question}</span>
                <span className="t-caption c-tertiary">{c.deck_path.join(" › ")}</span>
              </span>
              <div className="sync-versions">
                <Version v={cur} other={other} current picked={c.picked} />
                <Version v={other} other={cur} current={false} onUse={() => use(c.id)} busy={busy === c.id} />
              </div>
            </div>
          ) : (
            <div key={c.id} className="sync-conflict sync-conflict--folded">
              <span className="sync-row__text">
                <span className="t-body-m-strong">{c.question}</span>
                <span className="t-caption c-tertiary">
                  {cur.device} {when(cur.at)}  ↔  {other.device} {when(other.at)}
                </span>
              </span>
              <Button variant="ghost" size="M" icon="chevron-down" label="펼치기" onClick={() => setOpen(c.id)} />
            </div>
          );
        })}
      </div>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="sync-foot">
        <span className="t-caption c-tertiary">설정 › Google Drive에서 지난 30일 동안 다시 볼 수 있어요</span>
        <Button size="M" label="확인" onClick={onClose} />
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- K8 앱을 닫을 때

function CloseModal({ s }: { s: SyncStatus }) {
  const now = () => syncApi.closeNow().catch(() => {});
  const pct = s.total > 0 ? Math.min(100, Math.round((s.done / s.total) * 100)) : 66;
  return (
    <Sheet label="Drive에 올리고 닫을게요" onClose={now} className="sync-k8">
      <span className="sync-k8__title">
        <Spinner size={24} />
        <span className="t-title-s">Drive에 올리고 닫을게요</span>
      </span>
      <span className="sync-bar">
        <span style={{ width: `${pct}%` }} />
      </span>
      <span className="t-caption c-tertiary">
        {s.pending_reviews > 0 && `오늘 학습한 복습 기록 ${n(s.pending_reviews)}건 · `}
        {s.pending_notes > 0 && `카드 변경 ${n(s.pending_notes)}건 · `}몇 초면 끝나요
      </span>
      <div className="sync-foot">
        <span className="t-caption c-secondary">남은 변경은 다음에 열 때 올려요</span>
        <Button variant="ghost" size="M" label="기다리지 않고 닫기" onClick={now} />
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- K9 Drive 백업으로 되돌리기

function RestoreModal({ backup, onClose }: { backup: DriveBackup; onClose: () => void }) {
  const nav = useNav();
  const toast = useToast();
  const [p, setP] = useState<RestorePreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    syncApi.restorePreview(backup.name).then(setP, (e) => setError(errorText(e)));
  }, [backup.name]);
  const stamp = (() => {
    const d = new Date();
    const z = (v: number) => String(v).padStart(2, "0");
    return `${d.getFullYear()}${z(d.getMonth() + 1)}${z(d.getDate())}-${z(d.getHours())}${z(d.getMinutes())}`;
  })();
  const run = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await syncApi.restore(backup.name);
      await refreshSync();
      onClose();
      nav({ name: "home" });
      toast({ title: `${backupDate(backup.at)} 백업으로 복구했어요`, text: "복구 전 데이터는 이 기기 백업에 있어요", icon: "rotate" });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  const who = p?.others.length ? p.others.join(", ") : "다른 기기";
  return (
    <Sheet label="Drive 백업으로 복구" onClose={onClose} className="sync-k9">
      <h2 className="t-title-m">{backupDate(backup.at)} 백업으로 복구할까요?</h2>
      {!p ? (
        error ? <p className="t-body-s c-danger">{error}</p> : (
          <span className="sync-loading t-body-s c-secondary">
            <Spinner /> 백업을 살펴보는 중…
          </span>
        )
      ) : (
        <>
          <span className="t-caption c-tertiary">
            {p.device ?? "어느 기기"}에서 만든 백업 · 카드 {n(p.cards)}장 · 복습 기록 {n(p.reviews)}건
          </span>
          <div className="sync-box sync-box--rows">
            <span className="sync-perm">
              <Icon name="archive" size={18} className="c-secondary" />
              <span className="sync-perm__text">
                <span className="t-body-m">지금 데이터는 이 기기에 백업 파일로 남겨요</span>
                <span className="t-caption c-tertiary">neoanki-before-restore-{stamp}.zip · 설정 › 이 기기 백업</span>
              </span>
            </span>
            {p.lost_reviews + p.lost_changes > 0 ? (
              <span className="sync-perm">
                <Icon name="alert" size={18} className="c-warn" />
                <span className="sync-perm__text">
                  <span className="t-body-m">
                    이 백업 뒤의{" "}
                    {[p.lost_reviews && `복습 기록 ${n(p.lost_reviews)}건`, p.lost_changes && `카드 변경 ${n(p.lost_changes)}건`].filter(Boolean).join(" · ")}은 빠져요
                  </span>
                  <span className="t-caption c-tertiary">방금 남긴 백업 파일에는 그대로 있어요</span>
                </span>
              </span>
            ) : (
              <span className="sync-perm">
                <Icon name="check-circle" size={18} className="c-tertiary" />
                <span className="sync-perm__text">
                  <span className="t-body-m">이 백업 뒤에 바뀐 것은 없어요</span>
                </span>
              </span>
            )}
            <span className="sync-perm">
              <Icon name="rotate" size={18} className="c-secondary" />
              <span className="sync-perm__text">
                <span className="t-body-m">이 기기를 백업 상태로 바꾸고, Drive 동기화 기준도 이 상태로 바꿔요</span>
                <span className="t-caption c-tertiary">
                  {who}
                  {eun(who)} 다음에 동기화할 때 처리 방법을 골라요
                </span>
              </span>
            </span>
          </div>
          {error && <p className="t-body-s c-danger">{error}</p>}
        </>
      )}
      <div className="sync-foot sync-foot--end">
        <Button variant="ghost" size="M" label="취소" disabled={busy} onClick={onClose} />
        <Button size="M" icon="rotate" label="이 백업으로 복구" busy={busy} busyLabel="복구하는 중…" disabled={!p} onClick={run} />
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- K10 연결 끊기

function DisconnectModal({ onClose }: { onClose: () => void }) {
  const s = useSyncStatus();
  const toast = useToast();
  const [remove, setRemove] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [count, setCount] = useState<number | null>(null);
  useEffect(() => {
    syncApi.backups().then((b) => setCount(b.length), () => {});
  }, []);
  const who = others(s ?? null);
  const whoText = who.length ? who.join(", ") : "";
  const run = async () => {
    setBusy(true);
    try {
      await syncApi.disconnect(remove);
      await refreshSync();
      onClose();
      toast({ title: remove ? "연결을 끊고 Drive에서 지웠어요" : "Google Drive 연결을 끊었어요", text: "이 기기 데이터는 그대로예요" });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Sheet label="Google Drive 연결을 끊을까요?" onClose={onClose} className="sync-k10">
      <h2 className="t-title-m">Google Drive 연결을 끊을까요?</h2>
      <p className="t-body-s c-secondary">
        이 기기의 덱·카드·복습 기록은 그대로예요.{whoText && !remove && ` ${whoText}${eun(whoText)} 계속 Drive와 맞춰요.`}
      </p>
      <div className="import__choice">
        <Option on={!remove} onPick={() => setRemove(false)} title="Drive 데이터 남기기" text="다시 연결하면 이어서 쓸 수 있어요." />
        <Option
          on={remove}
          onPick={() => setRemove(true)}
          title="Drive에서도 지우기"
          text={`백업${count !== null ? ` ${n(count)}개와` : "과"} 동기화 데이터${s?.usage ? ` ${size(s.usage)}` : ""}를 지워요.${whoText ? ` ${whoText}의 동기화도 멈춰요.` : ""}`}
        />
      </div>
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="sync-foot sync-foot--end">
        <Button variant="ghost" size="M" label="취소" onClick={onClose} />
        <Button variant="danger" size="M" icon="link-off" label={remove ? "끊고 Drive에서 지우기" : "연결 끊기"} busy={busy} onClick={run} />
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- 암호로 잠그기 바꾸기 · 잠긴 Drive 열기

/** K4 암호로 잠그기 토글이 여는 확인 창. 확인하면 닫히고, 다시 올리기는 설정 행이 보여 준다 (K20) */
function LockModal({ on, onClose, onConfirm }: { on: boolean; onClose: () => void; onConfirm: (password: string | null) => void }) {
  const pw = usePassword();
  const [error, setError] = useState<string>();
  const run = () => {
    if (!on && pw.problem) return setError(pw.problem);
    onClose();
    onConfirm(on ? null : pw.pw);
  };
  return (
    <Sheet label="암호로 잠그기" onClose={onClose} className="sync-k10">
      <h2 className="t-title-m">{on ? "잠금을 풀고 Drive에 다시 올릴까요?" : "Drive 데이터를 암호로 잠글까요?"}</h2>
      <p className="t-body-s c-secondary">
        {on
          ? "Drive의 동기화 데이터를 이 기기에서 암호 없이 다시 올려요. 다른 기기는 다음에 맞출 때 고르는 창을 봐요."
          : "이 기기에서 암호화한 뒤 다시 올려요. Google도 내용을 볼 수 없어요. 암호는 Drive에 올리지 않고 이 기기에만 저장해요."}
      </p>
      {!on && pw.fields}
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="sync-foot sync-foot--end">
        <Button variant="ghost" size="M" label="취소" onClick={onClose} />
        <Button size="M" icon={on ? undefined : "lock"} label={on ? "잠금 풀기" : "잠그기"} onClick={run} />
      </div>
    </Sheet>
  );
}

function UnlockModal({ restart, onClose }: { restart?: boolean; onClose: () => void }) {
  const s = useSyncStatus();
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const run = async () => {
    setBusy(true);
    try {
      if (restart) await syncApi.join({ mode: "fresh", password });
      else await syncApi.unlock(password);
      await refreshSync();
      onClose();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Sheet label="Drive 데이터 암호" onClose={onClose} className="sync-k10">
      <h2 className="t-title-m">암호를 넣어 주세요</h2>
      <p className="t-body-s c-secondary">
        {restart
          ? "이 기기는 Drive 데이터를 암호로 잠가 두었어요. 암호를 넣으면 잠근 채 이 기기 데이터로 다시 시작해요. 잊었다면 연결을 끊고 다시 연결해 잠그지 않고 시작할 수 있어요."
          : s?.lock
            ? "이 기기에 저장해 둔 암호를 찾지 못했어요. 암호를 다시 넣으면 이어서 맞춰요."
            : "다른 기기에서 Drive 데이터를 암호로 잠갔어요. 맞는 암호를 넣으면 이 기기에 저장하고 이어서 맞춰요."}
      </p>
      {/* 다시 시작 모드는 위 문장이 같은 말을 한다 */}
      {!restart && <p className="t-caption c-tertiary">암호를 모르면 설정에서 연결을 끊고 이 기기만 쓸 수 있어요</p>}
      <Input type="password" label="암호" value={password} autoFocus error={error} onChange={(e) => setPassword(e.target.value)} onKeyDown={(e) => e.key === "Enter" && password && run()} />
      <div className="sync-foot sync-foot--end">
        <Button variant="ghost" size="M" label="취소" onClick={onClose} />
        <Button size="M" icon="lock" label={restart ? "다시 시작" : "열기"} busy={busy} disabled={!password} onClick={run} />
      </div>
    </Sheet>
  );
}

// ---------------------------------------------------------------- 설정 > Google Drive (K1 · K1-n · K4)

/** 설정 맨 위 칸. 연결 전(K1), Google 연결이 없는 빌드(K1-n), 연결됨(K4) */
export function DriveSection() {
  const s = useSyncStatus();
  if (!s) return null;
  return (
    <section className="prefs__section sync-section" aria-label="Google Drive">
      <p className="prefs__label t-label-s c-tertiary">Google Drive</p>
      {s.connected ? <Connected s={s} /> : <Intro s={s} />}
    </section>
  );
}

function Intro({ s }: { s: SyncStatus }) {
  return (
    <>
      <div className="sync-hero">
        <span className="sync-tile sync-tile--accent">
          <Icon name="cloud" size={22} className="c-accent" />
        </span>
        <span className="prefs__text">
          <span className="t-body-m-strong">Google Drive에 보관하고 기기끼리 맞추기</span>
          <span className="t-caption c-tertiary">내 Drive에 자동으로 백업하고, 다른 컴퓨터에서도 같은 덱·카드·복습 기록으로 이어서 공부해요.</span>
        </span>
        <Button size="M" icon="cloud" label="Google Drive 연결" disabled={!s.configured} className={s.configured ? "" : "sync-off"} onClick={() => syncFlow({ kind: "connect", from: "settings" })} />
      </div>
      {!s.configured && (
        <div className="sync-noclient">
          <Icon name="info" size={16} className="c-secondary" />
          <span className="prefs__text">
            <span className="t-body-m-strong">이 앱 버전에서는 Google Drive 연결을 사용할 수 없어요</span>
            <span className="t-caption c-tertiary" title="직접 빌드한 앱에는 Google 로그인 정보가 들어 있지 않아요">
              Google Drive 연결이 설정된 배포 버전이 필요해요. 지금은 이 기기 백업을 쓸 수 있어요.
            </span>
          </span>
        </div>
      )}
      <div className="sync-facts">
        <span>
          <Icon name="folder" size={16} className="c-secondary" />
          <span className="t-caption c-secondary">내 Drive에 저장 · Neo-Anki 서버 없음</span>
        </span>
        <span>
          <Icon name="lock" size={16} className="c-secondary" />
          <span className="t-caption c-secondary">Neo-Anki가 만든 파일만 접근</span>
        </span>
        <span>
          <Icon name="link-off" size={16} className="c-secondary" />
          <span className="t-caption c-secondary">끊어도 이 기기 데이터는 그대로</span>
        </span>
      </div>
    </>
  );
}

function Connected({ s }: { s: SyncStatus }) {
  const toast = useToast();
  const l = line(s);
  const [backups, setBackups] = useState<DriveBackup[] | null>(null);
  const [all, setAll] = useState(false);
  const [name, setName] = useState(s.device_name);
  const [bytes, setBytes] = useState<[number, number] | null>(null);
  const [asking, setAsking] = useState(false);
  /** 잠그기를 바꿔 다시 올리는 동안 토글이 가리키는 값 (K17) */
  const [relock, setRelock] = useState<boolean | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const [busy, setBusy] = useState<string>();
  const [conflicts, setConflicts] = useState(0);
  const joined = s.enabled;
  useEffect(() => setName(s.device_name), [s.device_name]);
  useEffect(() => {
    api.appInfo().then((i) => setBytes([i.db_bytes, i.media_bytes]), () => {});
  }, [s.last_sync]);
  useOverlayScope(!!menu);
  useKey("Escape", () => setMenu(null), !!menu, { modal: true });
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menu]);
  const loadBackups = useCallback(() => {
    if (!joined) return;
    syncApi.backups().then(setBackups, () => setBackups([]));
    syncApi.conflicts().then((c) => setConflicts(c.length), () => {});
  }, [joined]);
  useEffect(loadBackups, [loadBackups, s.last_sync]);
  const settings = async (opts: { device_name?: string; backup?: Backup }) => {
    try {
      await syncApi.settings(opts);
      await refreshSync();
    } catch (e) {
      toast({ title: "바꾸지 못했어요", text: errorText(e), tone: "error" });
    }
  };
  // 비우면 이전 이름 그대로
  const saveName = () => {
    const v = name.trim();
    if (!v || v === s.device_name) setName(s.device_name);
    else settings({ device_name: v });
  };
  const setLock = async (password: string | null) => {
    setRelock(password !== null);
    try {
      await syncApi.setLock(password);
      await refreshSync();
      toast({ title: password ? "Drive 데이터를 암호로 잠갔어요" : "암호 잠금을 풀었어요", text: "다른 기기는 다음에 맞출 때 고르는 창을 봐요" });
    } catch (e) {
      toast({ title: "바꾸지 못했어요", text: errorText(e), tone: "error" });
    } finally {
      setRelock(null);
    }
  };
  const full = async (b: DriveBackup) => {
    setMenu(null);
    const path = await save({ defaultPath: b.name.replace(/\.zip$/, "-with-media.zip"), filters: [{ name: "Neo-Anki 백업", extensions: ["zip"] }] });
    if (!path) return;
    setBusy(b.name);
    try {
      const saved = await syncApi.backupFull(b.name, path);
      toast({ title: "그림까지 묶어 저장했어요", tone: "undo", icon: "check-circle", action: { label: "폴더 열기", onClick: () => api.revealFile(saved) } });
    } catch (e) {
      toast({ title: "내려받지 못했어요", text: errorText(e), tone: "error" });
    } finally {
      setBusy(undefined);
    }
  };
  const remove = async (b: DriveBackup) => {
    setMenu(null);
    try {
      await syncApi.deleteBackup(b.name);
      toast({ title: "백업을 지웠어요" });
      loadBackups();
    } catch (e) {
      toast({ title: "지우지 못했어요", text: errorText(e), tone: "error" });
    }
  };
  const shown = all ? backups ?? [] : (backups ?? []).slice(0, 3);
  const busyNow = s.state === "syncing";
  return (
    <>
      <div className="sync-hero">
        <span className={`sync-tile ${l.tone === "ok" ? "sync-tile--ok" : l.tone === "warn" ? "sync-tile--warn" : ""}`}>
          {busyNow ? <Spinner size={20} /> : <Icon name={l.icon} size={22} className={l.tone === "ok" ? "c-success" : l.tone === "warn" ? "c-warn" : "c-secondary"} />}
        </span>
        <span className="prefs__text">
          <span className="t-body-m-strong">{l.title}</span>
          <span className="t-caption c-tertiary">
            {s.account}
            {s.usage !== null && ` · Drive에서 Neo-Anki가 쓰는 공간 ${size(s.usage)}`}
          </span>
          {busyNow && s.total > 1 && (
            <span className="sync-bar">
              <span style={{ width: `${Math.round((s.done / s.total) * 100)}%` }} />
            </span>
          )}
        </span>
        {s.state === "login" ? (
          <Button size="M" icon="external" label="다시 로그인" onClick={() => syncFlow({ kind: "connect", from: "login" })} />
        ) : !joined ? (
          <Button size="M" icon="cloud" label="동기화 시작" onClick={startFromProbe} />
        ) : s.state === "choice" ? (
          <Button size="M" label="고르기" onClick={() => syncFlow({ kind: "choice", probe: null })} />
        ) : s.state === "locked" ? (
          <Button size="M" icon="lock" label="암호 입력" onClick={() => syncFlow({ kind: "unlock" })} />
        ) : (
          <Button variant="secondary" size="M" icon="rotate" label="지금 동기화" busy={busyNow} busyLabel="맞추는 중…" disabled={s.studying} onClick={() => syncApi.now().catch((e) => toast({ title: errorText(e) }))} />
        )}
      </div>
      {/* 오프라인은 위 줄이 이미 말한다 (OS 원문 오류는 보이지 않는다) */}
      {s.state === "error" ? (
        <p className="sync-error t-caption c-tertiary">
          {failText(s)}
          {s.error && <Detail text={s.error} />}
        </p>
      ) : (
        s.error && s.state !== "idle" && s.state !== "offline" && <p className="sync-error t-caption c-tertiary">{s.error}</p>
      )}

      <div className="sync-block">
        <p className="t-label-s c-tertiary">기기 {n(s.devices.length)}대</p>
        {s.devices.map((d) => (
          <div key={d.uid} className={`sync-device ${d.stale ? "is-stale" : ""}`}>
            <Icon name="laptop" size={18} className="c-secondary" />
            <span className="sync-device__name">
              <span className="t-body-m">{d.name}</span>
              {d.me && <Badge tone="accent">이 기기</Badge>}
              {d.stale && <Badge>오래 안 씀</Badge>}
            </span>
            <span className="t-caption c-tertiary">
              {d.me
                ? d.at ? `${ago(d.at)} 맞춤` : "아직 맞추지 않음"
                : d.at ? `${when(d.at)}${d.reviews ? ` · 복습 ${n(d.reviews)}개를 올렸어요` : ""}` : "아직"}
            </span>
          </div>
        ))}
      </div>

      {/* K4-r: 행 모양은 그 행이 하는 일을 따른다 (K20) */}
      <div className="sync-rows">
        <DeviceName value={name} onChange={setName} onSave={saveName} onRevert={() => setName(s.device_name)} />
        <Uploads title="올리는 것" db={bytes?.[0] ?? null} media={bytes?.[1] ?? null} />
        <BackupPick value={s.backup} onPick={(v) => settings({ backup: v })} />
        <div>
          <LockBox on={relock ?? s.lock} busy={relock !== null} disabled={!joined || busyNow || relock !== null} onChange={() => setAsking(true)} />
        </div>
        {joined && (
          <button className="sync-sub" onClick={() => syncFlow({ kind: "conflicts" })}>
            <span className="t-body-m">두 기기에서 고친 카드</span>
            <span className={`t-body-s ${conflicts > 0 ? "c-secondary" : "c-tertiary"}`}>지난 30일 · {conflicts > 0 ? `${n(conflicts)}장` : "없음"}</span>
            <Icon name="chevron-right" size={18} className="c-secondary" />
          </button>
        )}
      </div>
      {asking && <LockModal on={s.lock} onClose={() => setAsking(false)} onConfirm={setLock} />}

      {joined && (
        <div className="sync-block sync-block--last">
          <div className="sync-backups__head">
            <span className="prefs__text">
              <span className="t-body-m-strong">Drive 기록 백업 {backups ? `${n(backups.length)}개` : ""}</span>
              <span className="t-caption c-tertiary">카드·복습 기록·설정이 들어 있어요 · 그림·소리는 Drive 동기화 공간에 따로, 원자료 파일은 이 기기에만 있어요</span>
            </span>
            <Button variant="ghost" size="M" icon="external" label="Drive에서 열기" onClick={() => syncApi.openFolder().catch((e) => toast({ title: errorText(e), tone: "error" }))} />
          </div>
          <div className="sync-backups">
            {backups === null && (
              <span className="sync-loading t-body-s c-tertiary">
                <Spinner /> 백업 목록을 불러오는 중…
              </span>
            )}
            {backups?.length === 0 && <span className="t-body-s c-tertiary sync-backups__empty">아직 Drive 백업이 없어요. 그날 처음 올릴 때 하나 만들어요.</span>}
            {shown.map((b) => (
              <div key={b.name} className="sync-backup">
                <span className="sync-row__text">
                  <span className="t-body-m">{backupDate(b.at)}</span>
                  <span className="t-caption c-tertiary">
                    {b.device ? `${b.device} · ` : ""}
                    {size(b.size)}
                  </span>
                </span>
                <Button variant="ghost" size="M" icon="rotate" label="복구" disabled={busyNow || s.studying} onClick={() => syncFlow({ kind: "restore", backup: b })} />
                <span className="menu" onClick={(e) => e.stopPropagation()}>
                  <button className="sync-more c-tertiary" aria-label={`${backupDate(b.at)} 백업 메뉴`} aria-expanded={menu === b.name} onClick={() => setMenu(menu === b.name ? null : b.name)}>
                    {busy === b.name ? <Spinner size={16} /> : <Icon name="more" size={18} scaled />}
                  </button>
                  {menu === b.name && (
                    <div className="menu__list sync-menu" role="menu">
                      <button role="menuitem" className="menu__item t-body-m" onClick={() => full(b)}>
                        <Icon name="download" size={16} />
                        그림까지 묶어 내려받기
                      </button>
                      <button role="menuitem" className="menu__item menu__item--danger t-body-m" onClick={() => remove(b)}>
                        <Icon name="trash" size={16} />
                        이 백업 지우기
                      </button>
                    </div>
                  )}
                </span>
              </div>
            ))}
            {!all && (backups?.length ?? 0) > 3 && (
              <button className="sync-backups__more t-label-m c-secondary" onClick={() => setAll(true)}>
                {n((backups?.length ?? 0) - 3)}개 더 보기
                <Icon name="chevron-down" size={16} />
              </button>
            )}
          </div>
        </div>
      )}

      <div className="sync-bottom">
        <span className="t-caption c-tertiary">끊어도 이 기기 데이터는 그대로예요</span>
        <Button variant="danger" size="M" icon="link-off" label="연결 끊기" onClick={() => syncFlow({ kind: "disconnect" })} />
      </div>
    </>
  );
}
