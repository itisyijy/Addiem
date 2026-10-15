// C1-xe 내보내기 확인 / C1-xc CSV / C1-xp 내보내는 중 / C1-xr 내보내기 결과 (미디어 누락). 덱 ⋯ 메뉴에서 연다
import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { api, errorText, type ExportPreview, type ExportProgress, type ExportResult } from "../api";
import { Button, Icon, Spinner, Toggle } from "../ui";
import { n, useKey, useNav, eul } from "../nav";
import { useToast } from "../toast";
import { Modal } from "./NewDeck";
import "../styles/export.css";

const iso = (t: Date) => `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")}`;
/** 저장 경로는 마지막 폴더와 파일 이름만 (…\Documents\덱 2026-10-14.apkg) */
const shortPath = (p: string) => p.replace(/^.*([\\/][^\\/]+[\\/][^\\/]+)$/, "…$1");

/** C1-xc 넣을 열. 다음 복습일만 기본 꺼짐. 순서는 CSV 열 순서와 같다 */
type Col = "front" | "back" | "deck" | "tags" | "importance" | "due";
const COLS: { key: Col; label: string }[] = [
  { key: "front", label: "앞면" },
  { key: "back", label: "뒷면" },
  { key: "deck", label: "하위 덱" },
  { key: "tags", label: "태그" },
  { key: "importance", label: "중요도" },
  { key: "due", label: "다음 복습일" },
];

export function ExportDeckDialog({ deckId, onClose }: { deckId: number; onClose: () => void }) {
  const nav = useNav();
  const toast = useToast();
  const [info, setInfo] = useState<ExportPreview | null>(null);
  const [history, setHistory] = useState(true); // C1-xe 기본 켜짐
  const [format, setFormat] = useState<"apkg" | "csv">("apkg");
  const [cols, setCols] = useState<Col[]>(["front", "back", "deck", "tags", "importance"]);
  const [plain, setPlain] = useState(true); // C1-xc 서식 빼고 글자만, 기본 켜짐
  const csv = format === "csv";
  const [file, setFile] = useState<string | null>(null); // 고른 저장 경로. 있는 동안 내보내는 중
  const [slow, setSlow] = useState(false); // 300ms 를 넘긴 내보내기만 진행 상태를 띄운다 (S1 로딩)
  const [prog, setProg] = useState<ExportProgress | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [result, setResult] = useState<ExportResult | null>(null);
  const started = useRef(0);
  const cancelled = useRef(false);
  const picking = useRef(false); // 저장 창이 두 번 뜨지 않게

  useEffect(() => {
    api.exportPreview(deckId).then(setInfo, (e) => {
      toast({ tone: "error", title: "내보내지 못했어요", text: errorText(e) });
      onClose();
    });
  }, [deckId]);

  const name = info?.path[info.path.length - 1] ?? "";
  // 파일 이름에 쓸 수 없는 글자는 _ 로 (덱 이름 "C++/Java" 가 폴더로 읽히지 않게)
  const fileName = `${name.replace(/[\\/:*?"<>|]/g, "_")} ${iso(new Date())}.${format}`;

  const reveal = (p: string) =>
    api.revealFile(p).catch((e) => toast({ tone: "error", title: "폴더를 열지 못했어요", text: errorText(e) }));

  const run = async () => {
    if (!info || file || picking.current || (csv && cols.length === 0)) return;
    picking.current = true;
    const filter = csv ? { name: "CSV (쉼표로 분리)", extensions: ["csv"] } : { name: "Anki 덱 패키지", extensions: ["apkg"] };
    const path = await save({ defaultPath: fileName, filters: [filter] }).finally(() => (picking.current = false));
    if (!path) return; // 저장 창에서 취소하면 이 창으로 돌아온다
    if (csv) return runCsv(path);
    setFile(path);
    setProg(null);
    setCancelling(false);
    cancelled.current = false;
    started.current = Date.now();
    const timer = setTimeout(() => setSlow(true), 300);
    const unlisten = await listen<ExportProgress>("export-progress", (e) => setProg(e.payload));
    try {
      const r = await api.exportApkg(deckId, path, history);
      if (r.missing_media > 0) setResult(r);
      else {
        // C1-xr 경고가 없으면 결과 창 없이 토스트만
        onClose();
        toast({
          tone: "undo",
          icon: "check-circle",
          title: `${name}${eul(name)} 내보냈어요 · 카드 ${n(r.cards)}장`,
          action: { label: "폴더에서 보기", onClick: () => reveal(r.path) },
        });
      }
    } catch (e) {
      // 멈추면 백엔드가 만들던 파일만 지운다. 실패도 확인 단계로 돌아가 다른 위치를 고르게 한다
      if (cancelled.current) toast({ title: "내보내기를 멈췄어요", text: "덱은 그대로예요" });
      else toast({ tone: "error", title: "저장하지 못했어요", text: `${errorText(e)} · 다른 위치를 골라 다시 시도해 주세요` });
    } finally {
      clearTimeout(timer);
      unlisten();
      setFile(null);
      setSlow(false);
    }
  };

  // C1-xc CSV 는 한 번에 써서 진행 단계가 없다. 결과는 .apkg 와 같은 토스트
  const runCsv = async (path: string) => {
    setFile(path);
    try {
      const r = await api.exportCsv(deckId, path, cols, plain);
      onClose();
      toast({
        tone: "undo",
        icon: "check-circle",
        title: `${name}${eul(name)} 내보냈어요 · ${n(r.rows)}줄`,
        action: { label: "폴더에서 보기", onClick: () => reveal(r.path) },
      });
    } catch (e) {
      toast({ tone: "error", title: "저장하지 못했어요", text: `${errorText(e)} · 다른 위치를 골라 다시 시도해 주세요` });
      setFile(null);
    }
  };

  const cancel = () => {
    cancelled.current = true;
    setCancelling(true);
    api.cancelExport().catch(() => {});
  };

  useKey("Enter", run, !!info && !file && !result, { modal: true });
  useKey("Enter", onClose, !!result, { modal: true });

  if (!info) return null;

  if (result) {
    const r = result;
    const tiles: [string, number][] = [["노트", r.notes], ["카드", r.cards], ["미디어", r.media]];
    if (history) tiles.push(["복습 기록", r.reviews]); // 끈 경우 복습 기록 칸 없음
    return (
      <Modal title={`내보냈지만 미디어 파일 ${n(r.missing_media)}개가 빠졌어요`} variant="confirm" icon="check" className="modal--export is-result" onClose={onClose}>
        <p className="export__path c-tertiary" title={r.path}>
          {shortPath(r.path)}
        </p>
        <div className="export__tiles">
          {tiles.map(([label, v]) => (
            <span key={label} className="export__tile">
              <span className="t-caption c-tertiary">{label}</span>
              <span className="t-title-s">{n(v)}</span>
            </span>
          ))}
        </div>
        <div className="export__warn">
          <Icon name="alert" size={18} className="c-warn" />
          <span className="export__warn-text">
            <span className="t-body-m-strong c-warn">미디어 {n(r.missing_media)}개를 찾지 못했어요</span>
            <span className="t-body-s c-secondary">
              저장한 .apkg 파일에서 카드 {n(r.missing_cards.length)}장의 그림이나 소리가 나오지 않아요. 이 앱의 카드는 그대로예요. 덱 안의 원래 파일이 지워졌거나 옮겨진 경우예요.
            </span>
          </span>
          <Button variant="ghost" size="M" label="카드 보기" onClick={() => nav({ name: "cards", deckId, cardIds: r.missing_cards })} />
        </div>
        <div className="modal__actions">
          <Button variant="secondary" size="M" icon="folder" label="폴더에서 보기" onClick={() => reveal(r.path)} />
          <Button size="M" label="닫기" shortcut="Enter" onClick={onClose} />
        </div>
      </Modal>
    );
  }

  // 내보내는 동안 바깥을 눌러도 닫히지 않는다 (C1-xp). Modal 은 Esc·닫기 버튼에서만 이벤트를 넘기므로 그때만 멈춘다
  const close = file ? (e?: unknown) => e && !cancelling && cancel() : onClose;

  if (slow && file && !csv) {
    return (
      <Modal title="내보내는 중" sub={file.split(/[\\/]/).pop()} className="modal--export is-progress" onClose={close}>
        <Progress info={info} history={history} prog={prog} started={started.current} />
        <div className="export__foot">
          <span className="t-caption c-tertiary">멈추면 만들던 파일만 지워져요 · 덱은 그대로</span>
          <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={cancel} busy={cancelling} busyLabel="멈추는 중…" />
        </div>
      </Modal>
    );
  }

  const counts = [
    info.subdecks > 0 && `하위 덱 ${n(info.subdecks)}개 포함`,
    `노트 ${n(info.notes)}개`,
    `카드 ${n(info.cards)}장`,
    info.media > 0 && `미디어 ${n(info.media)}개`,
  ];
  return (
    <Modal title="덱 내보내기" sub="형식을 고르고 저장할 곳을 정해요" className="modal--export" onClose={close}>
      <div className="segment segment--boxed">
        {(["apkg", "csv"] as const).map((f) => (
          <button key={f} className={`segment__item t-label-m ${format === f ? "is-selected" : ""}`} onClick={() => setFormat(f)} disabled={!!file}>
            {f === "apkg" ? "Anki 덱 (.apkg)" : "CSV (표)"}
          </button>
        ))}
      </div>
      <div className="export__deck">
        <span className="export__deck-icon">
          <Icon name="layers" size={18} className="c-accent" />
        </span>
        <span className="export__deck-text">
          <span className="t-body-m-strong">{info.path.join(" › ")}</span>
          <span className="t-caption c-secondary">{counts.filter(Boolean).join(" · ")}</span>
        </span>
      </div>
      {csv ? (
        <>
          <div className="export__cols">
            <span className="t-body-m-strong">넣을 열</span>
            <div className="export__chips">
              {COLS.map((c) => {
                const on = cols.includes(c.key);
                return (
                  <button
                    key={c.key}
                    role="checkbox"
                    aria-checked={on}
                    className={`export__chip t-label-m ${on ? "is-on" : ""}`}
                    // 순서를 COLS 순서로 유지한다 (CSV 열 순서)
                    onClick={() => setCols(COLS.map((x) => x.key).filter((k) => (k === c.key ? !on : cols.includes(k))))}
                  >
                    <span className="export__chip-box">{on && <Icon name="check" size={12} />}</span>
                    {c.label}
                  </button>
                );
              })}
            </div>
          </div>
          <div className="export__switch">
            <span className="export__switch-text">
              <span className="t-body-m-strong">서식 빼고 글자만</span>
              <span className="t-body-s c-secondary">굵게·색·줄바꿈 태그를 지우고 글자만 남겨요. 끄면 HTML 그대로 들어가요.</span>
            </span>
            <Toggle on={plain} onChange={setPlain} label="서식 빼고 글자만" />
          </div>
          <div className="export__note">
            <Icon name="info" size={16} className="c-tertiary" />
            <span className="t-body-s c-secondary">
              CSV에는 그림·소리·복습 기록이 들어가지 않아요. 다른 기기나 Anki로 옮기려면 .apkg가 나아요. 엑셀에서 한글이 깨지지 않게 UTF-8(BOM)으로 저장해요.
            </span>
          </div>
        </>
      ) : (
        <>
          <div className="export__switch">
            <span className="export__switch-text">
              <span className="t-body-m-strong">복습 기록 포함</span>
              <span className="t-body-s c-secondary">
                켜면 간격·기한과 복습 기록 {n(info.reviews)}건이 함께 나가요. 끄면 모든 카드가 새 카드로 나가고, 일시 중지한 카드만 그대로예요.
              </span>
            </span>
            <Toggle on={history} onChange={setHistory} label="복습 기록 포함" />
          </div>
          <div className="export__note">
            <Icon name="info" size={16} className="c-tertiary" />
            <span className="t-body-s c-secondary">이 파일을 Neo-Anki로 다시 가져오면 이미 있는 노트는 건너뛰어요. 백업은 설정 &gt; 백업을 쓰세요.</span>
          </div>
          {info.recall_cards > 0 && (
            <div className="export__note">
              <Icon name="info" size={16} className="c-tertiary" />
              <span className="t-body-s c-secondary">
                목록·선택형 카드 {n(info.recall_cards)}장 · Anki에서는 목록을 하나씩 공개하거나 보기를 선택할 수 없어요. 질문·정답은 볼 수 있어요.
              </span>
            </div>
          )}
        </>
      )}
      <p className="export__file c-tertiary">
        <Icon name="file" size={14} />
        <span className="export__file-name">{fileName}</span>
        <span className="t-caption">· 저장할 곳은 다음 창에서 골라요</span>
      </p>
      <div className="modal__actions">
        <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={onClose} disabled={!!file} />
        <Button size="M" icon="download" label="저장할 곳 고르기…" shortcut="Enter" onClick={run} busy={!!file} disabled={csv && cols.length === 0} />
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- C1-xp

const STEPS = [
  { key: "notes", label: "노트와 카드" },
  { key: "reviews", label: "복습 기록" },
  { key: "media", label: "미디어" },
  { key: "zip", label: "압축해서 저장" },
] as const;

function Progress({ info, history, prog, started }: { info: ExportPreview; history: boolean; prog: ExportProgress | null; started: number }) {
  const steps = STEPS.filter((s) => history || s.key !== "reviews"); // 끄면 복습 기록 줄이 없다
  const at = prog ? steps.findIndex((s) => s.key === prog.step) : 0;
  const frac = prog && prog.total > 0 ? prog.done / prog.total : 0;
  const ratio = Math.min(1, (at + frac) / steps.length);
  // 남은 시간은 지금까지 걸린 시간에 비례한 어림값. 처음 5% 는 너무 흔들려서 보이지 않는다 (B2-p 와 같다)
  const elapsed = (Date.now() - started) / 1000;
  const left = ratio >= 0.05 && ratio < 1 ? Math.max(1, Math.ceil((elapsed * (1 - ratio)) / ratio)) : null;
  const done = { notes: `${n(info.cards)}장`, reviews: `${n(info.reviews)}건`, media: `${n(info.media)}개`, zip: "완료" };

  return (
    <div className="export__progress">
      <div className="export__progress-head">
        <span className="t-title-s c-accent">{Math.floor(ratio * 100)}%</span>
        {left !== null && <span className="t-caption c-tertiary">약 {left < 60 ? `${left}초` : `${Math.ceil(left / 60)}분`} 남음</span>}
      </div>
      <div className="progress__track">
        <span className="progress__fill export__fill" style={{ width: `${ratio * 100}%` }} />
      </div>
      {steps.map((s, i) => {
        const state = i < at || ratio === 1 ? "done" : i === at ? "active" : "wait";
        const live = state === "active" && (s.key === "notes" || s.key === "media");
        const value =
          state === "done"
            ? done[s.key]
            : state === "wait"
              ? "대기"
              : s.key === "notes" // 이벤트는 노트와 카드를 합쳐 세므로 비율만 쓰고 숫자는 카드 수로
                ? `${n(Math.round(info.cards * frac))} / ${n(info.cards)}`
                : s.key === "media"
                  ? `${n(prog?.done ?? 0)} / ${n(prog?.total ?? info.media)}`
                  : s.key === "zip" ? "저장 중" : done[s.key];
        return (
          <div key={s.key} className="export__step">
            {state === "done" ? (
              <Icon name="check" size={18} className="c-success" />
            ) : state === "active" ? (
              <span className="export__step-icon c-accent">
                <Spinner size={18} />
              </span>
            ) : (
              <span className="export__wait" />
            )}
            <span className={`t-body-m ${state === "wait" ? "c-tertiary" : ""}`}>{s.label}</span>
            <span className={`t-body-s ${live ? "c-accent" : "c-secondary"}`}>{value}</span>
          </div>
        );
      })}
    </div>
  );
}
