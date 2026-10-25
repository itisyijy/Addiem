// E1 백업과 기본 설정 / E1-b 데이터 삭제 확인 / E2 학습 지표 (E2-0 데이터 모자람) / E3 보관한 덱
import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { api, errorText, type AppInfo, type ArchivedDeck, type HeatDay, type LocalBackup, type MvpMetrics } from "../api";
import { Badge, Button, Checkbox, Icon, Input, Kbd, Toggle } from "../ui";
import { useToast } from "../toast";
import { hourLabel, n, setCutoff, studyDate, useKey, useNav, eul, ro, useOverlayScope } from "../nav";
import {
  ACTIONS, FIXED, FIXED_ROWS, SCOPE_LABEL, actionOf, chordOf, exportKeys, importKeys, isChanged, keyLabel, reservedLabel, resetAllKeys, resetKeys, setKeys, useKeymap,
  type ActionId,
} from "../keys";
import { Modal } from "./NewDeck";
import { HIDDEN as FIGURES_HIDDEN } from "./PdfFigures";
import { DeleteDeckConfirm } from "./Deck";
import { NO_AUTOPLAY, SKIP_SUMMARY, TTS_AUTOPLAY, pref, setPref } from "./Study";
import { getThemePref, setThemePref, type ThemePref } from "../theme";
import { ModelRow } from "./Sources";
import { WrittenSection } from "./ConceptCheck";
import { DriveSection, useSyncStatus } from "../sync";
import { AiSection, useAiStatus } from "../ai";
import { NoteTypesEntry } from "./NoteTypes";
import { LAST_TYPE } from "./AddCard";
import type { Undoable } from "../api";
import { UndoConfirm } from "./DeckUpdate";

const HOURS = [0, 1, 2, 3, 4, 5, 6];
const THEMES: [ThemePref, string][] = [
  ["system", "시스템 따름"],
  ["light", "밝게"],
  ["dark", "어둡게"],
];

function when(ms: number, secs = false) {
  return new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", second: secs ? "2-digit" : undefined, hour12: false }).format(ms);
}

/** 긴 경로는 끝의 세 단계만. C:\Users\me\AppData\Roaming\앱\backups -> …\Roaming\앱\backups */
const shortPath = (p: string) => {
  const parts = p.split(/[\\/]/);
  return parts.length > 3 ? `…\\${parts.slice(-3).join("\\")}` : p;
};

export function Settings({ focus, backup }: { focus?: "archived" | "restore"; backup?: string }) {
  const nav = useNav();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [hourMenu, setHourMenu] = useState(false);
  const [wiping, setWiping] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string>();
  const [error, setError] = useState<string>();
  const [skipSummary, setSkipSummary] = useState(() => pref(SKIP_SUMMARY));
  const [autoplay, setAutoplay] = useState(() => !pref(NO_AUTOPLAY));
  const [ttsAutoplay, setTtsAutoplay] = useState(() => pref(TTS_AUTOPLAY));
  const [theme, setTheme] = useState(getThemePref);
  const [version, setVersion] = useState<string>();
  // [백업으로 복구] 창. 문자열이면 그 백업을 미리 골라 연다 (B3-u 업데이트 결과에서)
  const [restoring, setRestoring] = useState<string | boolean>(focus === "restore" ? (backup ?? true) : false);

  const load = useCallback(() => api.appInfo().then(setInfo), []);
  useEffect(() => {
    load().catch((e) => setError(errorText(e)));
    getVersion().then(setVersion, () => {});
  }, [load]);
  const back = useCallback(() => nav({ name: "home" }), [nav]);
  useKey("Escape", back, !wiping && !restoring);
  // 메뉴가 떠 있는 동안 화면 단축키는 꺼지고 Esc 는 메뉴만 닫는다 (E4 노트)
  useOverlayScope(hourMenu);
  useKey("Escape", () => setHourMenu(false), hourMenu, { modal: true });

  const act = async (work: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await work();
      setError(undefined);
      setMsg(done);
      await load();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  if (!info) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;

  return (
    <main className="prefs" onClick={() => setHourMenu(false)}>
      <button className="backlink t-label-m c-secondary" onClick={back}>
        <Icon name="chevron-left" size={18} />홈
      </button>
      <h1 className="t-title-l">설정</h1>

      {/* K1 · K4 Google Drive (맨 위). 기존 "백업"은 "이 기기 백업"으로 그 아래 */}
      <DriveSection />
      {/* L1 · L1-c ChatGPT 요금제 (G2 성격별로 묶기) */}
      <AiSection />

      <section className="prefs__section">
        <p className="prefs__label t-label-s c-tertiary">이 기기 백업</p>
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">지금 백업 만들기</span>
            <span className="t-body-s c-secondary">
              {info.last_backup ? `마지막 백업 · ${when(info.last_backup)} · ` : "아직 백업이 없어요 · "}
              덱, 카드, 복습 기록, 미디어, 연결한 자료가 파일 하나에 들어가요.
            </span>
          </span>
          <Button size="M" icon="download" label={busy ? "만드는 중…" : "백업 만들기"} disabled={busy} onClick={() => act(api.createBackup, "백업을 만들었어요")} />
        </div>
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">백업 위치</span>
            {/* 사용자 결정 2026-10-20 (commands.rs prune_auto_backups) */}
            <span className="t-body-s c-secondary">자동 백업은 최근 10개와 7일 안에 만든 것을 남기고, 그보다 오래된 것은 지워요 · 직접 만든 백업은 지우지 않아요</span>
          </span>
          <span className="prefs__path">
            <span className="prefs__mono c-secondary" title={info.backup_dir}>
              {shortPath(info.backup_dir)}
            </span>
            <Button variant="ghost" size="M" icon="folder" label="폴더 열기" onClick={() => act(api.openBackupDir)} />
          </span>
        </div>
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">백업으로 복구</span>
            <span className="t-body-s c-secondary">고른 백업을 만든 때로 덱 · 카드 · 복습 기록을 되돌려요. 덱 업데이트를 되돌릴 때도 여기서 해요.</span>
          </span>
          <Button variant="secondary" size="M" icon="rotate" label="백업 고르기" disabled={!info.last_backup} onClick={() => setRestoring(true)} />
        </div>
        {(msg || error) && <p className={`prefs__msg t-body-s ${error ? "c-danger" : "c-success"}`}>{error ?? msg}</p>}
      </section>
      {restoring && <RestoreBackup initial={typeof restoring === "string" ? restoring : undefined} onClose={() => setRestoring(false)} />}

      <Metrics />
      <Archived focus={focus === "archived"} />
      {/* NT1 노트 유형 (입구. 목록은 따로) */}
      <NoteTypesEntry />

      <section className="prefs__section">
        <p className="prefs__label t-label-s c-tertiary">학습</p>
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">하루 시작 시각</span>
            <span className="t-body-s c-secondary">이 시각을 기준으로 "오늘"이 바뀌어요. 밤늦게 공부해도 하루가 끊기지 않게.</span>
          </span>
          <span className="menu">
            <button
              className="prefs__select t-label-m"
              onClick={(e) => {
                e.stopPropagation();
                setHourMenu((v) => !v);
              }}
            >
              {hourLabel(info.cutoff_hour)}
              <Icon name="chevron-down" size={16} scaled className="c-secondary" />
            </button>
            {hourMenu && (
              <div className="menu__list" onClick={(e) => e.stopPropagation()}>
                {HOURS.map((h) => (
                  <button
                    key={h}
                    className="menu__item t-body-m prefs__hour"
                    onClick={() => {
                      setHourMenu(false);
                      act(async () => {
                        await api.setCutoffHour(h);
                        setCutoff(h);
                      });
                    }}
                  >
                    {hourLabel(h)}
                    {h === info.cutoff_hour && <Icon name="check" size={14} className="c-accent" />}
                  </button>
                ))}
              </div>
            )}
          </span>
        </div>
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">학습 전 요약 건너뛰기</span>
            <span className="t-body-s c-secondary">켜면 학습 시작을 누르자마자 첫 카드로 가요. 분량은 권장대로예요.</span>
          </span>
          <Toggle
            on={skipSummary}
            label="학습 전 요약 건너뛰기"
            onChange={(v) => {
              setPref(SKIP_SUMMARY, v);
              setSkipSummary(v);
            }}
          />
        </div>
        {/* AU4 앱 설정 한 줄 (이 기기). 덱마다 다르게 하기 · Anki 덱 옵션 가져오기는 하지 않는다 */}
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">소리 자동 재생</span>
            <span className="t-body-s c-secondary">앞면 소리는 카드를 열 때, 뒷면 소리는 답을 볼 때 재생해요. 끄면 눌렀을 때만 들려요.</span>
          </span>
          <Toggle
            on={autoplay}
            label="소리 자동 재생"
            onChange={(v) => {
              setPref(NO_AUTOPLAY, !v);
              setAutoplay(v);
            }}
          />
        </div>
        {/* AU21 읽어주기({{tts}}) 자동 재생은 따로, 기본 꺼짐. 꺼도 칩이나 R 로 읽는다 */}
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">읽어주기 자동 재생</span>
            <span className="t-body-s c-secondary">켜면 카드의 읽어주기도 소리처럼 저절로 읽어요. 끄면 칩을 누르거나 다시 듣기 키로 들어요.</span>
          </span>
          <Toggle
            on={ttsAutoplay}
            label="읽어주기 자동 재생"
            onChange={(v) => {
              setPref(TTS_AUTOPLAY, v);
              setTtsAutoplay(v);
            }}
          />
        </div>
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">화면 테마</span>
            <span className="t-body-s c-secondary">시스템 따름이면 Windows의 밝게·어둡게 설정을 따라가요.</span>
          </span>
          <span className="segment segment--boxed" role="radiogroup" aria-label="화면 테마">
            {THEMES.map(([v, label]) => (
              <button
                key={v}
                role="radio"
                aria-checked={theme === v}
                className={`segment__item t-label-m ${theme === v ? "is-selected" : ""}`}
                onClick={() => {
                  setThemePref(v);
                  setTheme(v);
                }}
              >
                {label}
              </button>
            ))}
          </span>
        </div>
      </section>

      <KeysTable />

      <WrittenSection /> {/* 8.3 서술형 채점: 개념 관계 확인 모델 (받기 · 지우기 · 이 PC 에서 쓸 수 있는지, screens/ConceptCheck.tsx) */}

      <section className="prefs__section">
        <p className="prefs__label t-label-s c-tertiary">데이터</p>
        <ModelRow /> {/* I3 분석 모델 (받았을 때만, screens/Sources.tsx) */}
        <div className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">모든 데이터 삭제</span>
            <span className="t-body-s c-secondary">덱, 카드, 복습 기록, 미디어, 연결한 자료를 이 기기에서 지워요. 삭제 전에 백업을 만들어요.</span>
          </span>
          <Button variant="danger" size="M" icon="trash" label="모든 데이터 삭제" onClick={() => setWiping(true)} />
        </div>
      </section>

      <Licenses />
      {version && <p className="prefs__version t-caption c-tertiary">Addiem {version}</p>}

      {wiping && <WipeConfirm info={info} onClose={() => setWiping(false)} onDone={() => nav({ name: "home" })} />}
    </main>
  );
}

/** 라이선스 원문은 80–100자에서 줄을 바꿔 두었다. 문단 안의 줄바꿈만 이어 상자 너비로 감싸게 한다 (빈 줄 · 목록 (a) · 1. · 구분선은 둔다) */
const reflow = (t: string) =>
  t.replace(/\r\n/g, "\n").replace(/^(.{60,})\n(?!\s|\([a-z0-9]{1,4}\)|[•*-]|\d+\.)/gim, (m, line: string) => (/[a-z]/i.test(line) ? `${line} ` : m));

/** 읽어주기 음성(tts.rs)과 함께 설치되는 것의 라이선스. 음성 파일이 없는 빌드에서는 보이지 않는다 */
function Licenses() {
  const [shown, setShown] = useState(false);
  const [texts, setTexts] = useState<Record<string, string>>({});
  useEffect(() => {
    api.ttsLangs().then((l) => setShown(l.length > 0), () => setShown(false));
  }, []);
  if (!shown) return null;
  // 전문(특히 ONNX Runtime 의 ThirdPartyNotices 340KB)은 펼칠 때 읽는다
  const open = (which: "supertonic" | "onnxruntime") =>
    texts[which] === undefined &&
    api.ttsLicense(which).then(
      (t) => setTexts((x) => ({ ...x, [which]: reflow(t) })),
      (e) => setTexts((x) => ({ ...x, [which]: errorText(e) })),
    );
  const items = [
    {
      which: "supertonic" as const,
      name: "Supertonic 3 음성 모델 · Supertone",
      license: "OpenRAIL-M",
      desc: "읽어주기 음성이에요. 라이선스 부록 A의 용도(법 위반, 남을 해치거나 속이는 일, 사칭, 기계가 만든 내용임을 밝히지 않고 퍼뜨리기 등)로는 쓸 수 없어요. 읽어 준 소리는 이 기기에서 재생만 하고 노트나 내보내기 파일에 넣지 않아요.",
    },
    { which: "onnxruntime" as const, name: "ONNX Runtime 1.31.0 · Microsoft", license: "MIT", desc: "음성 모델을 이 기기의 CPU에서 돌려요. 함께 쓰는 오픈소스의 고지도 전문에 있어요." },
  ];
  return (
    <section className="prefs__section">
      <p className="prefs__label t-label-s c-tertiary">오픈소스 라이선스</p>
      {items.map((it) => (
        <div key={it.which} className="prefs__row">
          <span className="prefs__text">
            <span className="t-body-m-strong">
              {it.name} <Badge>{it.license}</Badge>
            </span>
            <span className="t-body-s c-secondary">{it.desc}</span>
            <details className="prefs__license" onToggle={(e) => e.currentTarget.open && open(it.which)}>
              <summary className="t-label-s c-secondary">라이선스 전문</summary>
              <pre className="t-caption c-secondary">{texts[it.which] ?? "읽는 중…"}</pre>
            </details>
          </span>
        </div>
      ))}
    </section>
  );
}

/** 복구한 뒤 다시 불러온 홈의 토스트 (값: 그 백업의 시각 글) */
export const RESTORED_KEY = "addiem.restored";
const BACKUP_KIND: Record<LocalBackup["kind"], string> = {
  manual: "직접 만든 백업",
  update: "덱 업데이트 적용 전 자동 백업",
  restore: "복구 전 자동 백업",
  move: "노트 유형 옮기기 전 자동 백업",
  undo: "업데이트 되돌리기 전 자동 백업",
  notetype: "노트 유형 저장 전 자동 백업",
  deck: "덱 삭제 전 자동 백업",
  wipe: "모든 데이터 삭제 전 자동 백업",
  sync: "동기화 전 자동 백업",
  cleanup: "노트 정리 전 자동 백업",
};
const size = (b: number) => (b < 0.1 * 1024 * 1024 ? `${Math.max(1, Math.round(b / 1024))}KB` : `${(b / 1024 / 1024).toFixed(1)}MB`);

/**
 * 설정 › 이 기기 백업 [백업으로 복구] (DU16 의 되돌리기, DU42). 백업을 골라 DB 전체를 그때로 바꾼다.
 * 바꾸기 전에 지금 상태도 백업하고(-restore), 복구한 뒤 앱을 다시 불러온다. 동기화 중이면 다음 받기에서 Drive 와 다시 맞춘다 (K35)
 */
function RestoreBackup({ initial, onClose }: { initial?: string; onClose: () => void }) {
  const [list, setList] = useState<LocalBackup[] | null>(null);
  const [pick, setPick] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const drive = useSyncStatus();
  // DU2-u 설정 쪽 진입점: 업데이트 전 백업을 고르면 그 업데이트만 되돌리는 띠 (백업으로 복구는 DB 전체)
  const [updates, setUpdates] = useState<Undoable[]>([]);
  const [undo, setUndo] = useState<number | null>(null);
  useEffect(() => {
    api.undoableUpdates().then(setUpdates, () => {});
  }, []);
  useEffect(() => {
    api.localBackups().then(
      (l) => {
        setList(l);
        setPick((p) => (p && l.some((b) => b.name === p) ? p : l[0]?.name));
      },
      (e) => setError(errorText(e)),
    );
  }, []);
  const run = async () => {
    if (!pick || busy) return;
    setBusy(true);
    try {
      await api.restoreBackup(pick, true);
      // 다시 불러온 홈이 토스트로 알린다 (DU49). 저장이 막혀도 복구는 됐다
      try {
        sessionStorage.setItem(RESTORED_KEY, when(list!.find((b) => b.name === pick)!.at));
      } catch {
        /* 토스트만 빠진다 */
      }
      window.location.reload(); // 화면이 들고 있던 덱 · 카드를 모두 새로 읽는다
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  const update = updates.find((x) => x.backup === pick && !x.blocked);
  if (undo !== null) return <UndoConfirm batch={undo} onClose={() => setUndo(null)} />;
  return (
    <Modal title="백업으로 복구할까요?" onClose={onClose} variant="confirm" icon="rotate" className="restore">
      <p className="t-body-m c-secondary">지금 데이터가 이 백업 때로 바뀌어요 · 이 백업 뒤에 한 학습은 빠져요. 복구하기 전에 지금 상태를 백업해 두니 다시 되돌릴 수 있어요.</p>
      {list && list.length === 0 && <p className="t-body-s c-tertiary">이 기기 백업이 아직 없어요</p>}
      {list && list.length > 0 && (
        <div className="restore__list" role="radiogroup" aria-label="복구할 백업">
          {list.map((b) => (
            <label key={b.name} className={`import__option ${pick === b.name ? "is-on" : ""}`}>
              <input type="radio" name="restore" checked={pick === b.name} onChange={() => setPick(b.name)} />
              <span className="import__radio" />
              <span className="import__option-text">
                {/* 같은 분에 만든 백업끼리만 초까지 (DU51) */}
                <span className="t-body-m-strong">{when(b.at, list.some((o) => o !== b && when(o.at) === when(b.at)))}</span>
                <span className="t-caption c-tertiary">
                  {BACKUP_KIND[b.kind]} · {size(b.bytes)}
                </span>
              </span>
            </label>
          ))}
        </div>
      )}
      {update && (
        <div className="du-undo-strip">
          <Icon name="rotate" size={16} className="c-accent" />
          <span className="t-body-s">
            「{update.file_name}」 업데이트 전 백업이에요 · 그 업데이트로 바뀐 것만 되돌리면 그 뒤의 학습과 수정은 그대로예요
          </span>
          <Button variant="secondary" size="M" label="이 업데이트 되돌리기" onClick={() => setUndo(update.batch)} />
        </div>
      )}
      {drive?.connected && (
        <p className="t-body-s c-secondary">Google Drive 동기화를 켜 두었으면 다음 동기화 때 Drive와 다시 맞춰요. 이미 다른 기기로 올라간 변경은 다시 들어와요.</p>
      )}
      {error && <p className="t-body-s c-danger">{error}</p>}
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
        <Button variant="danger" size="M" icon="rotate" label={busy ? "복구하는 중…" : "이 백업으로 복구"} disabled={!pick || busy} onClick={run} />
      </div>
    </Modal>
  );
}

function WipeConfirm({ info, onClose, onDone }: { info: AppInfo; onClose: () => void; onDone: () => void }) {
  const [backup, setBackup] = useState(true);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const ok = typed.trim() === "삭제";
  const drive = useSyncStatus();
  const ai = useAiStatus();
  // 함께 지우는 것과 남는 것 (Q2-10). 학습 데이터가 아닌 ChatGPT 연결은 남긴다
  const also = [
    drive?.connected &&
      `Google Drive 연결도 끊어요. 이 기기에 저장한 Google 로그인 정보${drive.lock ? "와 Drive 잠금 키를" : "를"} 지우고, Drive의 백업·동기화 데이터는 남겨요.`,
    ai?.connected && "ChatGPT 연결은 그대로예요.",
  ].filter(Boolean);

  const run = async () => {
    if (!ok || busy) return;
    setBusy(true);
    try {
      await api.wipeAll(backup);
      localStorage.removeItem(FIGURES_HIDDEN); // 새 자료가 지운 자료의 id 를 다시 쓴다
      try {
        localStorage.removeItem(LAST_TYPE); // 새 덱 · 유형이 지운 것의 id 를 다시 쓴다 (NT35)
      } catch {
        // 못 지워도 틀린 기본 유형일 뿐이다
      }
      onDone();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal title="모든 데이터를 지울까요?" onClose={onClose} variant="confirm">
      <p className="t-body-m c-secondary">
        덱 {n(info.decks)}개 · 카드 {n(info.cards)}장 · 복습 기록 {n(info.reviews)}건이 이 기기에서 사라져요. 되돌릴 수 없어요.
      </p>
      {also.length > 0 && <p className="t-body-s c-secondary">{also.join(" ")}</p>}
      <Checkbox checked={backup} label="지우기 전에 백업 파일 만들기 (권장)" onChange={setBackup} />
      <Input
        label="확인을 위해 '삭제'를 입력해 주세요"
        value={typed}
        autoFocus
        onChange={(e) => setTyped(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && run()}
        error={error}
      />
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} />
        <Button variant="danger" size="M" icon="trash" label={busy ? "지우는 중…" : backup ? "백업 후 모두 삭제" : "모두 삭제"} disabled={!ok || busy} onClick={run} />
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- E2 학습 지표

const DAY = 86_400_000;
const dayNum = (iso: string) => Date.UTC(+iso.slice(0, 4), +iso.slice(5, 7) - 1, +iso.slice(8, 10)) / DAY;
const isoOf = (day: number) => new Date(day * DAY).toISOString().slice(0, 10);
const md = (iso: string) => `${+iso.slice(5, 7)}/${+iso.slice(8, 10)}`;
const longDate = (iso: string) => `${+iso.slice(5, 7)}월 ${+iso.slice(8, 10)}일`;
const pct = (v: number) => `${Math.round(v * 100)}%`;
/** 계획 완료율을 보는 기간 (오늘 제외 최근 14일, mvp_metrics 와 같다) */
const WINDOW = 14;
const CHART_H = 132;

function Metrics() {
  const nav = useNav();
  const [m, setM] = useState<MvpMetrics | null>(null);
  const [error, setError] = useState<string>();
  useEffect(() => {
    api.mvpMetrics().then(setM, (e) => setError(errorText(e)));
  }, []);
  if (!m) return error ? <p className="t-body-s c-danger">{error}</p> : null;

  const d = studyDate();
  const today = Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / DAY;
  // 오늘은 아직 끝나지 않았으니 지난 날만 센다
  const past = (iso: string | null) => (iso === null ? null : today - dayNum(iso));
  // 첫 학습 뒤 지난 날 (기록이 있는 날 수가 아니라 달력 날짜)
  const sessionDays = past(m.first_session_day);
  const byDay = new Map(m.plan_days.map((p) => [p.day, p]));
  const days = Array.from({ length: WINDOW }, (_, i) => isoOf(today - WINDOW + i));

  const setTarget = async () => {
    // 덱이 하나면 그 덱의 목표 설정으로, 여럿이면 홈에서 고른다
    const h = await api.home();
    const ids = [...h.decks, ...h.idle_decks].map((d) => d.id);
    nav(ids.length === 1 ? { name: "deckSettings", deckId: ids[0] } : ids.length ? { name: "home" } : { name: "newDeck" });
  };

  return (
    <section className="prefs__section prefs__section--card">
      <header className="prefs__head">
        <span className="prefs__text">
          <span className="t-title-s">학습 지표</span>
          <span className="t-body-s c-secondary">
            {sessionDays === null
              ? "아직 학습 기록이 없어요"
              : sessionDays >= WINDOW
                ? `최근 ${WINDOW}일 학습 기록 (${md(days[0])} – ${md(days[WINDOW - 1])})`
                : `첫 학습 뒤 ${sessionDays + 1}일째 · 아직 기록이 적어요. 지금까지의 추이예요`}
          </span>
        </span>
        <Badge>이 기기의 학습 기록으로만 계산</Badge>
      </header>

      <div className="metrics">
        <div className="metric">
          <span className="metric__title">
            <span className="t-body-m-strong">목표 덱 일일 계획 완료율</span>
            <span className="t-caption c-tertiary">
              {m.target_decks > 0
                ? `목표일이 있는 덱 ${n(m.target_decks)}개 · 최근 ${WINDOW}일 중 계획이 있던 ${n(m.plan_days.length)}일`
                : "목표일까지 계획한 분량을 매일 끝내는지"}
            </span>
          </span>
          {m.target_decks > 0 ? (
            <>
              <span className="metric__value">
                <span className={`metric__num ${m.plan_median === null ? "c-tertiary" : ""}`}>{m.plan_median === null ? "—" : pct(m.plan_median)}</span>
                <span className="t-body-s c-secondary">
                  {m.plan_median === null ? "아직 계획이 끝난 날이 없어요" : "하루 완료율의 중앙값"}
                </span>
              </span>
              <div className="metric__chart">
                <div className="metric__bars">
                  {days.map((day) => {
                    const p = byDay.get(day);
                    return p ? (
                      <span
                        key={day}
                        className="metric__bar"
                        style={{ height: Math.max(2, Math.round(Math.min(1, p.done / p.planned) * CHART_H)) }}
                        title={`${longDate(day)} · 계획 ${n(p.planned)}장 중 ${n(p.done)}장`}
                      />
                    ) : (
                      <span key={day} className="metric__bar metric__bar--none" title={`${longDate(day)} · 계획 없음`} />
                    );
                  })}
                </div>
                <span className="metric__axis t-caption c-tertiary">
                  <span>{md(days[0])}</span>
                  <span>{md(days[7])}</span>
                  <span>{md(days[WINDOW - 1])}</span>
                </span>
              </div>
              <p className="t-caption c-tertiary">막대 = 그날 계획 중 끝낸 비율 · 점선 칸은 계획이 없던 날</p>
            </>
          ) : (
            <>
              <span className="metric__value">
                <span className="metric__num c-tertiary">—</span>
                <span className="t-body-s c-secondary">목표일이 있는 덱이 생기면 계산해요</span>
              </span>
              <Button variant="ghost" size="M" icon="target" label="목표 정하기" onClick={setTarget} />
            </>
          )}
        </div>

        <div className="metric">
          <span className="metric__title">
            <span className="t-body-m-strong">접어둔 카드가 있었던 학습의 완료 비율</span>
            <span className="t-caption c-tertiary">카드를 접은 뒤에도 그날 예정을 끝까지 마친 학습</span>
          </span>
          <span className="metric__value">
            <span className={`metric__num ${m.bury_rate === null ? "c-tertiary" : ""}`}>{m.bury_rate === null ? "—" : pct(m.bury_rate)}</span>
            <span className="t-body-s c-secondary">
              {m.bury_sessions > 0 ? `학습 ${n(m.bury_sessions)}번 중 ${n(m.bury_completed)}번 끝까지 마침` : "아직 카드를 접은 학습이 없어요"}
            </span>
          </span>
          {m.bury_sessions === 0 && (
            <p className="t-body-s c-secondary">학습 중 카드를 접으면(자동 또는 직접), 그 학습을 끝까지 마쳤는지 여기에 점으로 쌓여요.</p>
          )}
          <span className="metric__dots">
            {m.sessions.length > 0
              ? m.sessions.map((done, i) => <span key={i} className={`metric__dot ${done ? "is-done" : "is-quit"}`} />)
              : Array.from({ length: 12 }, (_, i) => <span key={i} className="metric__dot is-empty" />)}
          </span>
          <span className="metric__legend t-caption c-secondary">
            <span>
              <span className="metric__key is-done" />
              끝까지 마침
            </span>
            <span>
              <span className="metric__key is-quit" />
              중간에 그만둠
            </span>
          </span>
        </div>
      </div>
      <Heatmap />
    </section>
  );
}

// ---------------------------------------------------------------- E2-h 학습 기록 히트맵

const WEEKS = 52;
const CELL = 17; // 13px 칸 + 4px 간격
const LABEL_W = 28; // 요일 글자 칸
/** 칸 색 단계: 0 / 1–49 / 50–99 / 100–199 / 200+ (고정 장수, E2-h 노트) */
const heatLevel = (n: number) => (n === 0 ? 0 : n < 50 ? 1 : n < 100 ? 2 : n < 200 ? 3 : 4);
const WEEKDAY = ["일", "월", "화", "수", "목", "금", "토"];

function Heatmap() {
  const [deck, setDeck] = useState<{ id: number; name: string } | null>(null);
  const [decks, setDecks] = useState<{ id: number; name: string }[]>([]);
  const [days, setDays] = useState<Map<string, HeatDay> | null>(null);
  const [menu, setMenu] = useState(false);
  const [hover, setHover] = useState<{ w: number; d: number; iso: string } | null>(null);
  const [table, setTable] = useState(false);
  // 창이 좁아 격자가 넘치면 오래된 주부터 자른다 (오른쪽 끝은 늘 이번 주)
  const [fit, setFit] = useState(WEEKS);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setFit(Math.max(1, Math.min(WEEKS, Math.floor((e.contentRect.width - LABEL_W + 4) / CELL)))));
    ro.observe(el);
    return () => ro.disconnect();
  }, [days]);

  useEffect(() => {
    api.home().then((h) => setDecks([...h.decks, ...h.idle_decks].map(({ id, name }) => ({ id, name }))), () => {});
  }, []);
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menu]);
  useOverlayScope(menu);
  useKey("Escape", () => setMenu(false), menu, { modal: true });
  useEffect(() => {
    api.heatmap(deck?.id ?? null).then((rows) => setDays(new Map(rows.map((r) => [r.day, r]))), () => setDays(new Map()));
  }, [deck]);
  if (!days) return null;

  // 월요일에 시작하는 52주 (넘치면 보이는 만큼). 아직 오지 않은 날은 그리지 않는다
  const t = studyDate();
  const today = Date.UTC(t.getFullYear(), t.getMonth(), t.getDate()) / DAY;
  const start = today - ((new Date(today * DAY).getUTCDay() + 6) % 7) - (fit - 1) * 7;
  const weeks = Array.from({ length: fit }, (_, w) =>
    Array.from({ length: 7 }, (_, d) => start + w * 7 + d)
      .filter((x) => x <= today)
      .map(isoOf),
  );
  const count = (iso: string) => days.get(iso)?.cards ?? 0;
  const month = (iso: string) => +iso.slice(5, 7);
  const tip = hover && days.get(hover.iso);
  // 달이 바뀌는 주에 월 이름. 맨 앞 주는 다음 달 이름과 3주 안으로 붙으면 뺀다 (글자가 겹치지 않게, 52주면 12개)
  const changes = weeks.flatMap((w, i) => (i > 0 && month(w[0]) !== month(weeks[i - 1][0]) ? [i] : []));
  const monthStarts = changes[0] === undefined || changes[0] >= 3 ? [0, ...changes] : changes;

  return (
    <div className="heat">
      <header className="heat__head">
        <span className="metric__title">
          <span className="t-body-m-strong">학습 기록</span>
          <span className="t-caption c-tertiary">최근 {fit}주 · 하루 학습한 카드 수 · 같은 카드는 하루에 한 번 세요 · {deck?.name ?? "모든 덱"}</span>
        </span>
        <span className="menu" onClick={(e) => e.stopPropagation()}>
          <button className="heat__select t-label-m" aria-expanded={menu} onClick={() => setMenu(!menu)}>
            {deck?.name ?? "모든 덱"}
            <Icon name="chevron-down" size={14} scaled className="c-secondary" />
          </button>
          {menu && (
            <div className="menu__list heat__menu">
              {[null, ...decks].map((d) => (
                <button
                  key={d?.id ?? 0}
                  className="menu__item t-body-m prefs__hour"
                  onClick={() => {
                    setDeck(d);
                    setMenu(false);
                  }}
                >
                  {d?.name ?? "모든 덱"}
                  {(d?.id ?? null) === (deck?.id ?? null) && <Icon name="check" size={14} className="c-accent" />}
                </button>
              ))}
            </div>
          )}
        </span>
      </header>

      <div ref={box} className="heat__grid" onMouseLeave={() => setHover(null)}>
        {monthStarts.map((i) => (
          <span key={`m${i}`} className="heat__month t-caption c-tertiary" style={{ left: LABEL_W + i * CELL }}>
            {month(weeks[i][0])}월
          </span>
        ))}
        {[0, 2, 4].map((d) => (
          <span key={d} className="heat__weekday t-caption c-tertiary" style={{ top: 17 + d * CELL }}>
            {WEEKDAY[(d + 1) % 7]}
          </span>
        ))}
        {weeks.map((w, i) =>
          w.map((iso, d) => (
            <span
              key={iso}
              className={`heat__cell heat--${heatLevel(count(iso))} ${hover?.iso === iso ? "is-hover" : ""}`}
              style={{ left: LABEL_W + i * CELL, top: 18 + d * CELL }}
              aria-label={`${longDate(iso)} · 학습한 카드 ${n(count(iso))}장`}
              onMouseEnter={() => setHover({ w: i, d, iso })}
            />
          )),
        )}
        {hover && (
          <span className="heat__tip t-caption" style={{ left: Math.min(LABEL_W + hover.w * CELL + 20, LABEL_W + fit * CELL - 190), top: 18 + hover.d * CELL - 34 }}>
            <b>
              {longDate(hover.iso)} ({WEEKDAY[new Date(dayNum(hover.iso) * DAY).getUTCDay()]})
            </b>
            {tip ? `학습한 카드 ${n(tip.cards)}장${tip.decks ? ` · 덱 ${n(tip.decks)}개` : ""}` : "학습 없음"}
          </span>
        )}
      </div>

      <footer className="heat__foot">
        <span className="t-caption c-tertiary">한 칸 = 하루 · 기준: 1–49 / 50–99 / 100–199 / 200장 이상</span>
        <span className="heat__legend t-caption c-tertiary">
          <button className="heat__table-btn t-caption c-secondary" aria-expanded={table} onClick={() => setTable(!table)}>
            {table ? "표 닫기" : "표로 보기"}
          </button>
          적게
          {[0, 1, 2, 3, 4].map((l) => (
            <span key={l} className={`heat__key heat--${l}`} />
          ))}
          많이
        </span>
      </footer>
      {table && (
        <table className="heat__table t-caption">
          <thead>
            <tr>
              <th>주 (월요일)</th>
              <th>학습한 카드</th>
            </tr>
          </thead>
          <tbody>
            {weeks
              .map((w) => [w[0], w.reduce((sum, iso) => sum + count(iso), 0)] as const)
              .reverse()
              .map(([iso, sum]) => (
                <tr key={iso}>
                  <td>{longDate(iso)} 주</td>
                  <td>{n(sum)}장</td>
                </tr>
              ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- E3 보관한 덱

const monthDay = (ms: number) => new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric" }).format(ms);

function Archived({ focus }: { focus: boolean }) {
  const nav = useNav();
  const toast = useToast();
  const ref = useRef<HTMLElement>(null);
  const [list, setList] = useState<ArchivedDeck[] | null>(null);
  const [busy, setBusy] = useState<number>();
  const [deleting, setDeleting] = useState<ArchivedDeck>();
  const load = useCallback(() => api.archivedDecks().then(setList), []);
  useEffect(() => {
    load().catch((e) => toast({ title: "보관한 덱을 불러오지 못했어요", text: errorText(e), tone: "error" }));
  }, [load, toast]);
  useEffect(() => {
    if (focus && list) ref.current?.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [focus, list]);
  if (!list) return null;

  const restore = async (d: ArchivedDeck) => {
    setBusy(d.id);
    try {
      const backlog = await api.unarchiveDeck(d.id);
      await load();
      toast({
        title: `${d.name} 보관을 해제했어요${backlog > 0 ? ` · 밀린 복습 ${n(backlog)}장` : ""}`,
        tone: "undo",
        icon: "check-circle",
        action: { label: "덱 열기", onClick: () => nav({ name: "deck", deckId: d.id }) },
      });
    } catch (e) {
      toast({ title: "보관을 해제하지 못했어요", text: errorText(e), tone: "error" });
    } finally {
      setBusy(undefined);
    }
  };

  return (
    <section ref={ref} className="prefs__section prefs__section--list">
      <header className="prefs__head prefs__head--stack">
        <span className="prefs__title">
          <span className="t-title-s">보관한 덱</span>
          {list.length > 0 && <Badge>{n(list.length)}</Badge>}
        </span>
        <span className="t-body-s c-secondary">
          보관한 덱은 홈과 오늘의 학습에서 빠져요. 보관을 해제하면 다시 학습에 포함되고, 그동안 쌓인 복습은 밀린 복습으로 올라와요 (앱 전체에서 하루 200장까지).
        </span>
      </header>
      {list.length === 0 && <p className="prefs__empty t-body-s c-tertiary">보관한 덱이 없어요. 시험이 끝난 덱은 덱 메뉴(⋯)에서 보관하면 여기에 모여요.</p>}
      {list.map((d) => (
        <div key={d.id} className="prefs__row">
          <span className="prefs__icon">
            <Icon name="archive" size={18} scaled className="c-secondary" />
          </span>
          <span className="prefs__text">
            <span className="prefs__title">
              <span className="t-body-m-strong">{d.name}</span>
              {d.target_passed && d.target_date && <Badge>목표일 지남 · {longDate(d.target_date)}</Badge>}
            </span>
            <span className="t-body-s c-tertiary">
              {monthDay(d.archived_at)} 보관 · 카드 {n(d.cards)}장{d.subdecks > 0 ? ` · 하위 덱 ${n(d.subdecks)}개` : ""}
            </span>
          </span>
          <Button variant="ghost" size="M" icon="trash" label="완전히 삭제" className="btn--danger-text" onClick={() => setDeleting(d)} />
          <Button variant="secondary" size="M" icon="rotate" label="보관 해제" busy={busy === d.id} busyLabel="해제하는 중…" onClick={() => restore(d)} />
        </div>
      ))}
      {deleting && (
        <DeleteDeckConfirm
          id={deleting.id}
          name={deleting.name}
          counts={[deleting.subdecks > 0 && `하위 덱 ${n(deleting.subdecks)}개`, `카드 ${n(deleting.cards)}장`].filter(Boolean).join(" · ")}
          onClose={() => setDeleting(undefined)}
          onDone={() => {
            toast({ title: `${deleting.name}${eul(deleting.name)} 지웠어요` });
            setDeleting(undefined);
            load();
          }}
        />
      )}
    </section>
  );
}

// ---------------------------------------------------------------- E4 단축키 바꾸기 (Figma 45:5593)

const eun = (w: string) => (eul(w) === "을" ? "은" : "는");
const wa = (w: string) => (eul(w) === "을" ? "과" : "와");
const RATE_NAMES = ["Again", "Hard", "Good", "Easy"];

/** 행을 눌러 새 키를 누르면 바뀐다. 한 번에 한 행만 녹음. 겹침은 같은 범위 안에서만 막는다 */
function KeysTable() {
  const map = useKeymap();
  const toast = useToast();
  /** 녹음 중인 행. 평가는 키 4개를 차례로 모은다 */
  const [rec, setRec] = useState<{ id: ActionId; got: string[] } | null>(null);
  /** 겹침 확인을 기다리는 행 */
  const [clash, setClash] = useState<{ id: ActionId; chord: string; other: ActionId } | null>(null);
  const [note, setNote] = useState<string>();
  const got = useRef<string[]>([]); // 평가 행에서 모은 키. 빠르게 이어 눌러도 놓치지 않게 상태가 아니라 ref 로
  useOverlayScope(!!rec); // 녹음 중에는 다른 단축키(N · I 등)가 끼어들지 않게

  const finish = (id: ActionId, keys: string[]) => {
    setKeys(id, keys);
    setRec(null);
    setNote(undefined);
  };

  // 녹음: 캡처 단계에서 먼저 받아 화면 단축키보다 앞선다. Esc 는 취소
  useEffect(() => {
    if (!rec) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
        setRec(null);
        setNote(undefined);
        return;
      }
      const chord = chordOf(e);
      if (!chord) return; // Ctrl · Shift 만 누른 상태
      if (FIXED.has(chord)) return setNote("Esc · Enter · Tab 은 바꿀 수 없어요");
      const fixed = reservedLabel(rec.id, chord);
      if (fixed) return setNote(`${keyLabel(chord)}${eun(keyLabel(chord))} '${fixed}'에 고정된 키예요`);
      const me = actionOf(rec.id);
      const other = ACTIONS.find((a) => a.id !== rec.id && map[a.id].includes(chord));
      if (rec.id === "rate") {
        // 평가 4개는 한 행에서 차례로. 같은 범위의 다른 동작 키나 이미 고른 키는 받지 않는다
        if (got.current.includes(chord)) return setNote(`${keyLabel(chord)}${eun(keyLabel(chord))} 이미 골랐어요`);
        if (other && other.scope === me.scope) return setNote(`${keyLabel(chord)}${eun(keyLabel(chord))} '${other.label}'에 쓰고 있어요`);
        got.current = [...got.current, chord];
        if (got.current.length === 4) return finish("rate", got.current);
        setRec({ id: "rate", got: got.current });
        setNote(undefined);
        return;
      }
      if (other) {
        setClash({ id: rec.id, chord, other: other.id });
        setRec(null);
        setNote(undefined);
        return;
      }
      finish(rec.id, [chord]);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [rec, map]);

  /** [바꿔 쓰기]: 같은 범위면 상대 동작의 키를 비운다 (미지정), 범위가 다르면 둘 다 쓴다 */
  const overwrite = () => {
    if (!clash) return;
    const other = actionOf(clash.other);
    if (other.scope === actionOf(clash.id).scope) setKeys(other.id, map[other.id].map((k) => (k === clash.chord ? "" : k)));
    setKeys(clash.id, [clash.chord]);
    setClash(null);
  };

  const resetAll = () => {
    const before = exportKeys();
    resetAllKeys();
    setRec(null);
    setClash(null);
    toast({ title: "단축키를 모두 기본값으로 돌렸어요", tone: "undo", action: { label: "되돌리기", onClick: () => importKeys(before) } });
  };

  const chips = (keys: string[]) =>
    keys.map((k, i) => (
      <span key={i} className="keys__chord">
        {k ? keyLabel(k).split(" ").map((p, j) => <Kbd key={j} k={p} />) : <span className="t-caption c-tertiary">미지정</span>}
      </span>
    ));

  const row = (id: ActionId) => {
    const a = actionOf(id);
    const recording = rec?.id === id;
    const conflict = clash?.id === id ? clash : null;
    const other = conflict && actionOf(conflict.other);
    const key = conflict ? keyLabel(conflict.chord) : "";
    return (
      <div key={id} className={`keys__row ${recording ? "is-recording" : ""} ${conflict ? "is-clash" : ""}`}>
        <span className="keys__name">
          <span className="t-body-m">{a.label}</span>
          {conflict && other ? (
            <span className="t-caption c-warn">
              {other.scope === a.scope
                ? `${key}${eun(key)} '${other.label}'에 쓰고 있어요. 바꿔 쓰면 '${other.label}'${eun(other.label)} 미지정이 돼요.`
                : `${key}${eun(key)} 이 화면에서 쓰지 않지만 '${other.label}'(${SCOPE_LABEL[other.scope]})${wa(other.label)} 같아요. ${SCOPE_LABEL[a.scope]}에서는 '${a.label}'${ro(a.label)} 쓸까요?`}
            </span>
          ) : recording && id === "rate" ? (
            <span className="t-caption c-accent">
              {RATE_NAMES[rec!.got.length]} 키를 누르세요 ({rec!.got.length + 1} / 4){note ? ` · ${note}` : ""}
            </span>
          ) : recording && note ? (
            <span className="t-caption c-accent">{note}</span>
          ) : id === "rate" ? (
            <span className="t-caption c-tertiary">기본값 1 2 3 4</span>
          ) : null}
        </span>
        {recording ? (
          <>
            <span className="keys__chips">
              {id === "rate" && chips(rec!.got)}
              <span className="keys__wait t-label-s c-accent">새 키를 누르세요…</span>
            </span>
            <span className="t-caption c-tertiary">Esc 취소</span>
          </>
        ) : conflict ? (
          <>
            <span className="keys__chips">{chips([conflict.chord])}</span>
            <span className="keys__acts">
              <Button variant="secondary" size="M" label="바꿔 쓰기" onClick={overwrite} />
              <Button variant="ghost" size="M" label="취소" onClick={() => setClash(null)} />
            </span>
          </>
        ) : (
          <>
            <span className="keys__chips">{chips(map[id])}</span>
            <span className="keys__acts">
              {isChanged(id) && (
                <>
                  <span className="t-caption c-accent">바꿈</span>
                  <Button variant="ghost" size="M" icon="rotate" label="기본값" onClick={() => resetKeys(id)} />
                </>
              )}
              <Button
                variant="ghost"
                size="M"
                label="바꾸기"
                aria-label={`${a.label} 단축키 바꾸기`}
                onClick={() => {
                  setClash(null);
                  setNote(undefined);
                  got.current = [];
                  setRec({ id, got: [] });
                }}
              />
            </span>
          </>
        )}
      </div>
    );
  };

  return (
    <section className="prefs__section keys">
      <div className="keys__head">
        <span className="prefs__text">
          <span className="t-title-s">단축키</span>
          <span className="t-body-s c-secondary">행을 눌러 새 키를 누르면 바뀌어요. 같은 화면 안에서만 겹치는지 확인해요.</span>
        </span>
        <Button variant="ghost" size="M" icon="rotate" label="모두 기본값으로" onClick={resetAll} />
      </div>
      {(["study", "app", "occlusion"] as const).map((scope) => (
        <div key={scope} className="keys__group" role="group" aria-label={SCOPE_LABEL[scope]}>
          <p className="keys__label t-label-s c-tertiary">{SCOPE_LABEL[scope]}</p>
          {ACTIONS.filter((a) => a.scope === scope).map((a) => row(a.id))}
          {FIXED_ROWS.filter((f) => f.scope === scope).map((f) => (
            <div key={f.key} className="keys__row">
              <span className="keys__name">
                <span className="t-body-m">{f.label}</span>
                <span className="t-caption c-tertiary">{f.caption}</span>
              </span>
              <span className="keys__chips">
                <Kbd k={f.key} />
              </span>
              <span className="keys__fixed t-caption c-tertiary">바꿀 수 없음</span>
            </div>
          ))}
        </div>
      ))}
    </section>
  );
}
