// B2 Anki 덱 가져오기 (B2-0 고르기 전, B2-r 기록 이전 선택, B2-p 진행, S1-5 실패) / B2-j JSON 가져오기 / B3 가져오기 결과
import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { parseCsv } from "../csv";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, errorText, type AnkiPreview, type ImportIssue, type ImportProgress, type ImportResult, type JsonPreview } from "../api";
import { Badge, Button, Icon, Spinner, StatTile, Toggle, type IconName } from "../ui";
import { eul, lockNav, n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { Modal } from "./NewDeck";
import { UpdateChoice } from "./DeckUpdate";
import "../styles/import.css";

// String.raw: JSON 문자열 안의 수식은 역슬래시를 두 번 쓴다 (\\( … \\)). 그대로 보여주고 그대로 붙여넣게
const EXAMPLE = String.raw`{ "deck": "정보처리기사",
  "cards": [
    { "front": "TCP와 UDP의 차이는?", "back": "TCP는 연결 지향, UDP는 비연결",
      "deck": "정보처리기사::2과목::네트워크", "tags": ["중요도::최우선", "기출"] },
    { "front": "OSI 7계층 중 4계층은?", "back": "전송 계층" },
    { "front": "TCP는 {{c1::연결 지향}} 프로토콜이다", "importance": "핵심" },
    { "front": "\\(x^2 + y^2 = r^2\\) 는 무엇의 식인가?", "back": "원" }
  ]
}`;

/** 파일 크기. 0.1MB 미만은 KB (0.0MB 로 보이지 않게) */
const mb = (bytes: number) => (bytes < 0.1 * 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))}KB` : `${(bytes / 1024 / 1024).toFixed(1)}MB`);
const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;
const JSON_FILE = /\.json$/i;

/**
 * 결과 화면(B3)에 넘길 것 중 ImportResult 에 없는 값. 라우트(nav.tsx)를 넓히지 않으려고
 * 가져오기 직후 결과 객체에 붙여 둔다. 라우트는 메모리에만 있어 객체가 그대로 전달된다
 */
type Carry = { roots?: string[]; skipped?: { json: string; count: number } | null; overdue?: number };
const carried = new WeakMap<ImportResult, Carry>();

/** 밀린 복습이 하루에 올라오는 상한 (큐 명세의 backlog 200) */
const BACKLOG_PER_DAY = 200;

type Tab = "anki" | "json" | "csv";

export function Import({ tab: initial, deckId = null, pick = false, path }: { tab: "anki" | "json"; deckId?: number | null; pick?: boolean; path?: string }) {
  const nav = useNav();
  const [tab, setTab] = useState<Tab>(initial);
  // 단축키 I 로 들어오면 파일 선택 창을 바로 연다 (B2-0). 탭을 오가며 다시 열리지 않게 한 번만
  const [autoPick, setAutoPick] = useState(pick && initial === "anki");
  const switchTab = (t: Tab) => {
    setTab(t);
    setAutoPick(false);
  };
  // 가져오는 동안에는 화면을 떠나지 않는다. 떠나도 가져오기는 계속돼 결과 화면이 갑자기 뜬다
  const [locked, setLocked] = useState(false);
  const back = useCallback(() => nav(deckId !== null ? { name: "deck", deckId } : { name: "home" }), [nav, deckId]);
  useKey("Escape", back, !locked);
  return (
    <main className="import">
      <button className="backlink t-label-m c-secondary" onClick={back} disabled={locked}>
        <Icon name="chevron-left" size={18} />
        {deckId !== null ? "덱" : "홈"}
      </button>
      <div className="import__head">
        <h1 className="t-title-l">가져오기</h1>
        <div className="segment segment--boxed">
          <button className={`segment__item t-label-m ${tab === "anki" ? "is-selected" : ""}`} onClick={() => switchTab("anki")} disabled={locked}>
            Anki 덱 (.apkg · .colpkg)
          </button>
          <button className={`segment__item t-label-m ${tab === "json" ? "is-selected" : ""}`} onClick={() => switchTab("json")} disabled={locked}>
            JSON
          </button>
          <button className={`segment__item t-label-m ${tab === "csv" ? "is-selected" : ""}`} onClick={() => switchTab("csv")} disabled={locked}>
            CSV
          </button>
        </div>
      </div>
      {tab === "anki" ? (
        <AnkiImport onLock={setLocked} autoPick={autoPick} initialPath={path} />
      ) : tab === "json" ? (
        <JsonImport deckId={deckId} onCancel={back} />
      ) : (
        <CsvImport deckId={deckId} onCancel={back} />
      )}
    </main>
  );
}

/**
 * 파일 끌어놓기. Tauri 는 파일 드롭을 HTML drop 이 아니라 웹뷰 이벤트(경로)로 준다.
 * 탭마다 그 탭이 보이는 동안만 듣는다. accept 는 모듈 상수로 넘긴다 (바뀌면 다시 구독)
 */
function useFileDrop(enabled: boolean, accept: RegExp, onFile: (path: string) => void) {
  const [drag, setDrag] = useState<"over" | "wrong" | null>(null);
  const handler = useRef(onFile);
  handler.current = onFile;
  useEffect(() => {
    if (!enabled) return;
    const un = getCurrentWebview().onDragDropEvent(({ payload: p }) => {
      if (p.type === "enter" || p.type === "over") setDrag("over");
      else if (p.type === "leave") setDrag(null);
      else if (p.paths[0] && accept.test(p.paths[0])) {
        setDrag(null);
        handler.current(p.paths[0]);
      } else setDrag("wrong");
    });
    return () => void un.then((f) => f());
  }, [enabled, accept]);
  return [drag, setDrag] as const;
}

/** B2-0 끌어놓기 영역. 탭마다 문구만 다르다 (B2-c "CSV 파일을 여기로 끌어오세요") */
function DropZone({ drag, idle, wrong, hint, help, onPick, busy = false }: {
  drag: "over" | "wrong" | null;
  idle: string;
  wrong: ReactNode;
  hint: string;
  help: ReactNode;
  onPick: () => void;
  busy?: boolean;
}) {
  return (
    <>
      <div className={`import__drop ${drag === "over" ? "is-over" : ""}`}>
        <span className="import__drop-icon">
          <Icon name="upload" size={28} className="c-accent" />
        </span>
        <p className="t-title-m">{drag === "over" ? "놓으면 바로 검사해요" : drag === "wrong" ? wrong : idle}</p>
        <p className="t-caption c-tertiary">또는</p>
        <Button variant="secondary" icon="folder" label="파일 선택" onClick={onPick} busy={busy} busyLabel="살펴보는 중…" />
        <p className="import__drop-hint t-caption c-tertiary">{hint}</p>
      </div>
      <div className="import__help">
        <Icon name="info" size={18} className="c-tertiary" />
        <span className="t-body-s c-secondary">{help}</span>
      </div>
    </>
  );
}

// ---------------------------------------------------------------- B2

const ANKI_FILE = /\.(apkg|colpkg)$/i;

/** S1 오류 문구 공식: 무엇이 안 됐는지 + 기존 데이터는 그대로 */
const safe = (msg: string) => (msg.includes("그대로") ? msg : `${msg} 기존 덱과 카드는 그대로예요.`);

function AnkiImport({ onLock, autoPick, initialPath }: { onLock: (v: boolean) => void; autoPick: boolean; initialPath?: string }) {
  const nav = useNav();
  const toast = useToast();
  const [path, setPath] = useState<string | null>(null);
  const [info, setInfo] = useState<AnkiPreview | null>(null);
  const [checking, setChecking] = useState(false);
  const [history, setHistory] = useState(true); // B2-r 기본은 "그대로 옮기기"
  // B2-s 고른 덱 (deck_tree id). 기본은 전부 — 지금처럼 한 번에 다 가져오는 흐름 그대로
  const [sel, setSel] = useState<Set<number>>(new Set());
  const tree = useMemo(() => (info ? deckTree(info.deck_tree) : []), [info]);
  const picked = useMemo(() => info?.deck_tree.filter((d) => sel.has(d.id)) ?? [], [info, sel]);
  const all = !!info && picked.length === info.deck_tree.length;
  const none = !!info && info.deck_tree.length > 0 && picked.length === 0;
  const total = (key: "cards" | "new_cards" | "reviews" | "overdue" | "latex_cards" | "latex_raw_cards") =>
    picked.reduce((a, d) => a + d[key], 0);
  // 실제로 들어갈 카드. 이미 있는 노트의 카드는 건너뛰므로 버튼 · 기록 · 밀린 복습 모두 이것 기준
  const newCards = total("new_cards");
  const nothingNew = !none && !!info && info.deck_tree.length > 0 && newCards === 0;
  // 지운 카드의 기록: 어느 덱에도 속하지 않아 덱을 모두 고를 때만 함께 들어간다 (히트맵의 지난 기록이 줄어드는 이유).
  // deck_tree 의 reviews 는 새로 들어갈 카드의 기록만 세므로 파일 전체 수에서 빼면 안 되고 백엔드 값을 쓴다
  const orphanReviews = info?.orphan_reviews ?? 0;
  const reviews = total("reviews") + (all ? orphanReviews : 0);
  const [failed, setFailed] = useState<string>();
  const [importing, setImporting] = useState(false);
  const [slow, setSlow] = useState(false); // 300ms 를 넘긴 가져오기만 진행 카드를 띄운다 (S1 로딩)
  const [prog, setProg] = useState<ImportProgress | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const started = useRef(0);
  const cancelled = useRef(false);
  // B2-u0: 이미 가져온 노트가 있으면 먼저 방식을 고른다 (업데이트 · 새 노트만). adding = 지금의 가져오기
  const [adding, setAdding] = useState(false);

  const inspect = async (p: string) => {
    setPath(p);
    setInfo(null);
    setFailed(undefined);
    setChecking(true);
    try {
      const i = await api.inspectAnki(p);
      setAdding(false);
      setInfo(i);
      // 업데이트로 알아본 파일(B2-u0)의 기록은 덱을 만든 사람 것일 수 있어 기본은 신규로 (DU52). 화면에서 바꿀 수 있다
      setHistory(i.reviews > 0 && !i.update);
      setSel(new Set(i.deck_tree.map((d) => d.id)));
    } catch (e) {
      setFailed(safe(errorText(e)));
    } finally {
      setChecking(false);
    }
  };

  const pick = async () => {
    const p = await open({ multiple: false, directory: false, filters: [{ name: "Anki 덱", extensions: ["apkg", "colpkg"] }] });
    if (typeof p === "string") inspect(p);
  };
  const [drag] = useFileDrop(!importing, ANKI_FILE, inspect);

  // StrictMode 는 마운트 효과를 두 번 돌린다. 창이 두 번 뜨지 않게 ref 로 막는다
  const autoPicked = useRef(false);
  useEffect(() => {
    if (autoPick && !autoPicked.current) {
      autoPicked.current = true;
      pick();
    }
    // B2-u [방식 바꾸기]: 그 파일을 다시 살펴본다
    if (initialPath && !autoPicked.current) {
      autoPicked.current = true;
      inspect(initialPath);
    }
  }, [autoPick, initialPath]);

  // 가져오는 동안 앱 전체의 화면 전환을 막는다 (TopNav, N · I 단축키 포함). 결과로 넘어가기 전에 꼭 푼다
  const lock = (v: boolean) => {
    lockNav(v);
    onLock(v);
  };
  useEffect(() => () => lockNav(false), []);

  const run = async () => {
    if (!path || !info || importing || none || nothingNew) return;
    setImporting(true);
    lock(true);
    setProg(null);
    setCancelling(false);
    cancelled.current = false;
    started.current = Date.now();
    const timer = setTimeout(() => setSlow(true), 300);
    const unlisten = await listen<ImportProgress>("import-progress", (e) => setProg(e.payload));
    try {
      // 고른 덱에 기록이 없으면 고를 것이 없어 새 카드로 (기록 묶음도 숨겨져 있다)
      const result = await api.importAnki(path, history && reviews > 0, all ? null : [...sel]);
      carried.set(result, { overdue: total("overdue") });
      lock(false); // 잠긴 채로 nav 하면 결과 화면 전환까지 막힌다
      nav({ name: "importResult", result });
    } catch (e) {
      lock(false);
      // 직접 멈춘 것은 실패가 아니다. 백엔드가 모두 되돌렸으니 조용히 사전 검사로 돌아간다
      if (cancelled.current) toast({ title: "가져오기를 멈췄어요", text: "기존 덱과 카드는 그대로예요" });
      else setFailed(safe(errorText(e)));
    } finally {
      clearTimeout(timer);
      unlisten();
      setImporting(false);
      setSlow(false);
      lock(false);
    }
  };

  const cancel = () => {
    cancelled.current = true;
    setCancelling(true);
    api.cancelImport().catch(() => {});
  };
  useKey("Escape", cancel, slow && !cancelling);

  // S1-5 실패: 사전 검사/진행 카드 자리가 빨간 안내로 바뀐다
  const failure = failed && (
    <div className="import__fail">
      <Icon name="alert" size={20} className="c-danger" />
      <span className="import__fail-text">
        <span className="t-body-m-strong c-danger">가져오지 못했어요</span>
        <span className="t-body-s c-secondary">{failed}</span>
        <Button variant="secondary" size="M" icon="folder" label="다른 파일 고르기" onClick={pick} />
      </span>
    </div>
  );

  if (!info) {
    if (failure) return failure;
    return (
      <DropZone
        drag={drag}
        idle="Anki 덱 파일을 여기로 끌어오세요"
        wrong={
          <>
            Anki 덱 파일(.apkg, .colpkg)만 가져올 수 있어요.
            <br />
            JSON · CSV는 오른쪽 위 탭에서.
          </>
        }
        hint=".apkg · .colpkg   ·   원본 파일은 읽기만 하고 수정하지 않아요"
        help='Anki에서 파일 만들기: 파일 → 내보내기 → "Anki 덱 패키지 (.apkg)" 또는 "Anki 컬렉션 패키지 (.colpkg)"'
        onPick={pick}
        busy={checking}
      />
    );
  }

  // B2-s 사전 검사는 고른 덱만으로 다시 센다. 전부 골랐으면 파일 전체 문구 그대로
  const nodes = tree.flatMap((t) => [t, ...descendants(t)]);
  const types = [...new Set(picked.flatMap((d) => d.types))];
  const typesOk = types.filter((t) => !info.degraded_types.includes(t)).length;
  const degradedIn = (feature: string) => {
    const hit = picked.flatMap((d) => d.degraded.filter((x) => x.feature === feature));
    return { types: new Set(hit.flatMap((x) => x.note_types)).size, cards: hit.reduce((a, x) => a + x.cards, 0) };
  };
  const latex = total("latex_cards");
  const latexRaw = total("latex_raw_cards");
  const overdue = total("overdue");
  const rows: { icon: IconName; tone: string; label: string; value: string }[] = [
    {
      icon: "check",
      tone: "c-success",
      label: "덱과 하위 덱 구조",
      value: all
        ? `최상위 ${n(info.decks_top)}개 · 하위 ${n(info.decks_sub)}개`
        : `고른 덱 ${n(tree.filter((t) => pickOf(t, sel) !== "off").length)}개 · 하위 ${n(nodes.filter((d) => d.depth > 0 && pickOf(d, sel) !== "off").length)}개`,
    },
    {
      icon: "check",
      tone: "c-success",
      label: "노트 유형",
      value: all
        ? info.note_types_ok === info.note_types
          ? `${n(info.note_types)}종 모두 지원 (필드 치환 템플릿)`
          : `${n(info.note_types)}종 중 ${n(info.note_types_ok)}종 지원 (필드 치환 템플릿)`
        : typesOk === types.length
          ? `고른 덱이 쓰는 ${n(types.length)}종 모두 지원`
          : `고른 덱이 쓰는 ${n(types.length)}종 중 ${n(typesOk)}종 지원`,
    },
    ...info.degraded.flatMap((d) => {
      const s = degradedIn(d.feature);
      // AU23 읽어주기는 이 기기에 그 언어 음성이 없을 때만 오는 참고 한 줄 ("읽어주기(프랑스어)")
      const lang = d.feature.match(/^읽어주기\((.+)\)$/)?.[1];
      if (lang)
        return s.cards === 0
          ? []
          : [{ icon: "info" as IconName, tone: "c-secondary", label: "읽어주기", value: `이 기기에 ${lang} 음성이 없어 ${n(s.cards)}장은 소리 없이 보여요` }];
      return s.cards === 0
        ? [{ icon: "check" as IconName, tone: "c-success", label: `${d.feature}${eul(d.feature)} 쓰는 템플릿`, value: "고른 덱에는 없어요" }]
        : [{
            icon: "alert" as IconName,
            tone: "c-warn",
            label: `${d.feature}${eul(d.feature)} 쓰는 템플릿`,
            value: `${n(s.types)}종 · ${n(s.cards)}장은 ${d.feature === "빈칸 입력칸" ? "써 넣은 답을 본문 전체와 비교해요" : "일부가 다르게 보여요"}`,
          }];
    }),
    // D3-m: [latex] 는 수식만 들었으면 KaTeX 로 그리고, LaTeX 문서 전체면 원문으로 둔다
    ...(latex > 0
      ? [{
          icon: (latexRaw > 0 ? "alert" : "check") as IconName,
          tone: latexRaw > 0 ? "c-warn" : "c-success",
          label: "LaTeX 태그",
          value: `${n(latex)}장${latexRaw > 0 ? " · 일부는 원문으로 보여요" : ""}`,
        }]
      : []),
    ...(info.filtered_decks > 0
      ? [{ icon: "check" as IconName, tone: "c-success", label: "Anki 필터 덱", value: `${n(info.filtered_decks)}개 · 카드는 원래 덱으로 들어가요` }]
      : []),
  ];

  return (
    <>
      <div className="import__file">
        <span className="import__file-icon">
          <Icon name="file" size={22} className="c-accent" />
        </span>
        <span className="import__file-text">
          <span className="t-body-m-strong">{info.file_name}</span>
          <span className="t-body-s c-secondary">
            {mb(info.bytes)} · 덱 {n(info.decks_top)}개 · 카드 {n(info.cards)}장 · 미디어 {n(info.media)}개 발견
          </span>
        </span>
        {!importing && !failed && <Button variant="ghost" size="M" label="다른 파일" onClick={pick} />}
      </div>
      {info.update && !adding && !failed && path && <UpdateChoice key={path} path={path} info={info} onAdd={() => setAdding(true)} />}
      {info.update && adding && !importing && (
        <div className="du-adding">
          <Icon name="plus" size={16} className="c-secondary" />
          <span className="t-body-s c-secondary">
            새 노트만 더해요 · 이미 있는 노트 {n(info.update.existing_notes)}개는 그대로예요
            {info.update.gone_notes > 0 && ` · 내가 지운 노트 ${n(info.update.gone_notes)}개는 다시 넣지 않아요`}
          </span>
          <Button variant="ghost" size="M" label="방식 바꾸기" onClick={() => setAdding(false)} />
        </div>
      )}
      {(!info.update || adding || failed) && (
        <>

      {failure ||
        (slow ? (
          <ProgressCard
            prog={prog}
            started={started.current}
            cards={all ? info.cards : total("cards")}
            decks={all ? info.decks_top + info.decks_sub : nodes.filter((d) => pickOf(d, sel) !== "off").length}
            media={all ? info.media : null}
          />
        ) : (
          <div className="b2s">
            <DeckTree tree={tree} sel={sel} onChange={setSel} total={info.deck_tree} newOnly={!!info.update} />
            <div className="b2s__side">
            <div className="import__checks">
              {rows.map((r) => (
                <div key={r.label} className="import__check">
                  <Icon name={r.icon} size={18} className={r.tone} />
                  <span className="t-body-m">{r.label}</span>
                  <span className="t-body-s c-secondary">{r.value}</span>
                </div>
              ))}
            </div>
            {/* B2-r 고른 덱에 기록이 없으면 고를 것이 없어 묶음째 숨기고 새 카드로 가져온다 */}
            {reviews > 0 && (
              <div className="import__choice" role="radiogroup" aria-label="Anki의 복습 기록">
                <p className="t-label-m">Anki의 복습 기록</p>
                <label className={`import__option ${history ? "is-on" : ""}`}>
                  <input type="radio" name="history" checked={history} onChange={() => setHistory(true)} />
                  <span className="import__radio" />
                  <span className="import__option-text">
                    <span className="import__option-title">
                      <span className="t-body-m-strong">복습 기록과 다음 복습 일정 가져오기</span>
                      {info.update ? <Badge tone="neutral">{n(reviews)}건</Badge> : <Badge tone="accent">권장 · {n(reviews)}건</Badge>}
                    </span>
                    <span className="t-body-s c-secondary">
                      카드마다 간격·기한·기억 상태를 이어받아 시작해요. 이후 평가는 Addiem의 일정 계산을 따르고, 학습 순서·하루 분량은 Addiem 설정을 써요.
                    </span>
                    {!all && orphanReviews > 0 && (
                      <span className="t-caption c-tertiary">지운 카드의 기록 {n(orphanReviews)}건은 덱을 모두 고를 때만 함께 들어가요</span>
                    )}
                    {history && overdue > 0 && (
                      <span className="import__overdue">
                        <Icon name="clock" size={14} />
                        <span className="t-caption">
                          {all ? "오늘" : "고른 덱"} 기준 밀린 복습 {n(overdue)}장 · 하루 {BACKLOG_PER_DAY}장씩 올라와요 (약 {Math.ceil(overdue / BACKLOG_PER_DAY)}일)
                        </span>
                      </span>
                    )}
                  </span>
                </label>
                <label className={`import__option ${history ? "" : "is-on"}`}>
                  <input type="radio" name="history" checked={!history} onChange={() => setHistory(false)} />
                  <span className="import__radio" />
                  <span className="import__option-text">
                    <span className="import__option-title">
                      <span className="t-body-m-strong">신규 카드로 시작</span>
                      {info.update && <Badge tone="accent">권장</Badge>}
                    </span>
                    <span className="t-body-s c-secondary">
                      Anki에서 학습한 카드도 복습 기록 없이 신규 카드로 시작해요. 목표일이 있으면 신규로 다시 나눠요. 원본 기록은 Anki에 그대로 남아요.
                    </span>
                    {info.update && <span className="t-caption c-tertiary">이미 가져온 덱의 새 버전이라, 이 기록은 덱을 만든 사람의 것일 수 있어요. 내가 Anki에서 공부한 기록이면 위를 골라 주세요.</span>}
                  </span>
                </label>
              </div>
            )}
            </div>
          </div>
        ))}

      {!failed && (
        <div className="import__foot">
          <span className="t-body-s c-tertiary">
            {slow
              ? "중간에 멈추면 아무것도 바뀌지 않아요 · 기존 데이터 그대로"
              : none
                ? "가져올 덱을 하나 이상 골라 주세요"
                : nothingNew
                  ? "고른 덱의 노트는 이미 모두 있어요 · 다른 덱을 골라 주세요"
                : "이미 있는 노트는 추가하거나 갱신하지 않아요 · 고르지 않은 덱은 나중에 같은 파일로 다시 가져올 수 있어요"}
          </span>
          <span className="import__foot-right">
            {slow && <Button variant="ghost" label="취소" shortcut="Esc" onClick={cancel} busy={cancelling} busyLabel="멈추는 중…" />}
            <Button
              icon={slow ? undefined : "download"}
              label={slow ? "가져오는 중…" : all ? `${n(newCards)}장 가져오기` : `고른 ${n(newCards)}장 가져오기`}
              onClick={run}
              disabled={importing || none || nothingNew}
            />
          </span>
        </div>
      )}
        </>
      )}
    </>
  );
}

// ---------------------------------------------------------------- B2-s

type DeckStat = AnkiPreview["deck_tree"][number];
/** 트리 한 줄. units: 이 덱과 하위 덱 중 카드가 있는 실제 덱 (고르는 단위) */
type DeckNode = { key: string; name: string; depth: number; units: DeckStat[]; children: DeckNode[] };
type Pick = "on" | "off" | "mixed";

const byName = new Intl.Collator("ko", { numeric: true }).compare;

/** 실제 덱 경로들로 트리를 만든다. 카드 없는 중간 덱도 줄이 된다 */
function deckTree(stats: DeckStat[]): DeckNode[] {
  const top: DeckNode[] = [];
  const nodes = new Map<string, DeckNode>();
  for (const s of stats) {
    let siblings = top;
    s.path.forEach((name, depth) => {
      const key = s.path.slice(0, depth + 1).join("\x1f");
      let node = nodes.get(key);
      if (!node) {
        node = { key, name, depth, units: [], children: [] };
        nodes.set(key, node);
        siblings.push(node);
      }
      node.units.push(s);
      siblings = node.children;
    });
  }
  const sort = (ns: DeckNode[]) => ns.sort((a, b) => byName(a.name, b.name)).forEach((n) => sort(n.children));
  sort(top);
  return top;
}

const pickOf = (node: DeckNode, sel: Set<number>): Pick => {
  const k = node.units.filter((u) => sel.has(u.id)).length;
  return k === 0 ? "off" : k === node.units.length ? "on" : "mixed";
};
const sumOf = (units: DeckStat[], key: "cards" | "notes" | "existing_notes" | "gone_cards" | "new_cards") => units.reduce((a, u) => a + u[key], 0);
const descendants = (n: DeckNode): DeckNode[] => n.children.flatMap((c) => [c, ...descendants(c)]);

/** B2-s 왼쪽 "가져올 덱". 3상태 체크박스, 최상위만 펼친 채 시작, 고르는 단위는 하위 덱 전체 */
function DeckTree({ tree, sel, onChange, total, newOnly = false }: {
  tree: DeckNode[];
  sel: Set<number>;
  onChange: (update: (s: Set<number>) => Set<number>) => void;
  total: DeckStat[];
  /** 업데이트로 알아본 파일을 [새 노트만 더하기]로: 머리를 들어갈 새 카드 기준으로 ([n장 가져오기]와 같은 수) */
  newOnly?: boolean;
}) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const rows: DeckNode[] = [];
  const walk = (ns: DeckNode[]) => ns.forEach((n) => (rows.push(n), open.has(n.key) && walk(n.children)));
  walk(tree);
  // 이전 선택에서 계산한다 (연달아 눌러도 앞의 변경을 잃지 않게)
  const toggle = (n: DeckNode) =>
    onChange((prev) => {
      const next = new Set(prev);
      const on = pickOf(n, prev) !== "on";
      n.units.forEach((u) => (on ? next.add(u.id) : next.delete(u.id)));
      return next;
    });
  const picked = total.filter((u) => sel.has(u.id));
  return (
    <div className="b2s__tree">
      <div className="b2s__tree-head">
        <span className="b2s__tree-title">
          <span className="t-title-s">가져올 덱</span>
          <span className="t-caption c-tertiary">
            {newOnly
              ? `새 카드 ${n(sumOf(picked, "new_cards"))}장 · 이미 있는 ${n(sumOf(picked, "cards") - sumOf(picked, "new_cards") - sumOf(picked, "gone_cards"))}장${sumOf(picked, "gone_cards") > 0 ? ` · 내가 지운 ${n(sumOf(picked, "gone_cards"))}장` : ""} 제외`
              : `${n(sumOf(total, "cards"))}장 중 ${n(sumOf(picked, "cards"))}장 · 덱 ${n(tree.length)}개 중 ${n(tree.filter((t) => pickOf(t, sel) !== "off").length)}개`}
          </span>
        </span>
        <span>
          <Button variant="ghost" size="M" label="모두 선택" onClick={() => onChange(() => new Set(total.map((u) => u.id)))} />
          <Button variant="ghost" size="M" label="모두 해제" onClick={() => onChange(() => new Set())} />
        </span>
      </div>
      {rows.map((node) => {
        const state = pickOf(node, sel);
        const cards = sumOf(node.units, "cards");
        const chosen = sumOf(node.units.filter((u) => sel.has(u.id)), "cards");
        const notes = sumOf(node.units, "notes");
        const existing = sumOf(node.units, "existing_notes");
        const subs = descendants(node);
        const caption =
          existing > 0 && existing === notes
            ? `이미 있는 노트 ${n(existing)}개 · 추가하거나 갱신하지 않아요`
            : state === "mixed" && subs.length > 0
              ? `하위 덱 ${n(subs.length)}개 중 ${n(subs.filter((s) => pickOf(s, sel) !== "off").length)}개`
              : null;
        return (
          <div key={node.key} className={`b2s__row ${state === "off" ? "is-off" : ""}`} style={{ paddingLeft: 16 + node.depth * 28 }}>
            {node.children.length > 0 ? (
              <button
                className="b2s__fold"
                aria-label={open.has(node.key) ? "접기" : "펼치기"}
                aria-expanded={open.has(node.key)}
                onClick={() =>
                  setOpen((o) => {
                    const next = new Set(o);
                    if (!next.delete(node.key)) next.add(node.key);
                    return next;
                  })
                }
              >
                <Icon name={open.has(node.key) ? "chevron-down" : "chevron-right"} size={14} scaled />
              </button>
            ) : (
              <span className="b2s__fold" />
            )}
            <button role="checkbox" aria-checked={state === "mixed" ? "mixed" : state === "on"} className="b2s__pick" onClick={() => toggle(node)}>
              <span className={`b2s__box is-${state}`}>{state === "on" && <Icon name="check" size={14} />}</span>
              <span className="b2s__name">
                <span className={node.depth === 0 ? "t-body-m-strong" : "t-body-m"}>{node.name}</span>
                {caption && <span className="t-caption c-tertiary">{caption}</span>}
              </span>
              <span className="t-body-s c-secondary">{state === "mixed" ? `${n(chosen)} / ${n(cards)}장` : `${n(cards)}장`}</span>
            </button>
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------- B2-p

const STEPS = [
  { key: "unzip", label: "압축 풀기" },
  { key: "decks", label: "덱과 하위 덱" },
  { key: "cards", label: "카드" },
  { key: "media", label: "미디어" },
] as const;

/** cards · decks · media: 고른 덱 기준 수 (B2-s). media 가 null 이면 고른 노트가 쓰는 수를 아직 모른다 */
function ProgressCard({ prog, started, cards, decks, media }: {
  prog: ImportProgress | null;
  started: number;
  cards: number;
  decks: number;
  media: number | null;
}) {
  // 단계 이벤트는 단계가 끝날 때도 온다 (done === total). 그 사이에는 다음 단계가 돌고 있다
  const at = !prog ? 0 : prog.step === "done" ? STEPS.length : STEPS.findIndex((s) => s.key === prog.step);
  const ended = !!prog && prog.step !== "done" && prog.done >= prog.total;
  const active = at + (ended ? 1 : 0);
  const frac = prog && !ended && prog.step !== "done" && prog.total > 0 ? prog.done / prog.total : 0;
  const ratio = Math.min(1, (active + frac) / STEPS.length);
  // 남은 시간은 지금까지 걸린 시간에 비례한 어림값. 처음 5% 는 너무 흔들려서 보이지 않는다
  const elapsed = (Date.now() - started) / 1000;
  const left = ratio >= 0.05 && ratio < 1 ? Math.max(1, Math.ceil((elapsed * (1 - ratio)) / ratio)) : null;
  // 미디어는 고른 노트가 쓰는 것만 들어가서, 덱을 골랐으면 미디어 단계 이벤트가 알려 주는 수를 쓴다
  const mediaTotal = media ?? (prog?.step === "media" || prog?.step === "done" ? prog.total : null);
  const totals = { unzip: 0, decks, cards, media: mediaTotal ?? 0 };

  return (
    <div className="import__progress">
      <div className="import__progress-head">
        <span className="t-title-s">가져오는 중</span>
        <span className="t-label-m c-accent">
          {Math.floor(ratio * 100)}%{left !== null && ` · 약 ${left < 60 ? `${left}초` : `${Math.ceil(left / 60)}분`} 남음`}
        </span>
      </div>
      <div className="progress__track">
        <span className="progress__fill progress__fill--accent" style={{ width: `${ratio * 100}%` }} />
      </div>
      {STEPS.map((s, i) => {
        const state = i < active ? "done" : i === active ? "active" : "wait";
        const total = totals[s.key];
        const unit = s.key === "cards" ? "장" : "개";
        // 카드 단계 이벤트는 노트와 카드를 합쳐 세므로 비율만 쓰고 숫자는 카드 수로 보여준다
        const live = state === "active" && (s.key === "cards" || s.key === "media");
        const value =
          s.key === "unzip" || (s.key === "media" && mediaTotal === null)
            ? state === "done" ? "완료" : state === "active" ? (s.key === "unzip" ? "푸는 중" : "옮기는 중") : "대기"
            : state === "wait"
              ? `${n(total)}${unit} · 대기`
              : live
                ? `${n(Math.round(total * (prog?.step === s.key ? frac : 0)))} / ${n(total)}`
                : `${n(total)}${unit}`;
        return (
          <div key={s.key} className="import__step">
            {state === "done" ? (
              <Icon name="check" size={18} className="c-success" />
            ) : state === "active" ? (
              <span className="import__step-icon c-accent">
                <Spinner size={18} />
              </span>
            ) : (
              <span className="import__wait" />
            )}
            <span className={`t-body-m ${state === "wait" ? "c-tertiary" : ""}`}>{s.label}</span>
            <span className={`t-body-s ${live ? "c-accent" : "c-secondary"}`}>{value}</span>
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------- B2-j

export function IssueList({ issues, limit = 30 }: { issues: ImportIssue[]; limit?: number }) {
  if (!issues.length) return null;
  return (
    <div className="import__issues">
      {issues.slice(0, limit).map((i, k) => (
        <div key={k} className="import__issue">
          <Badge tone={i.severity === "skipped" ? "danger" : i.severity === "warning" ? "warn" : "neutral"}>
            {i.index !== null ? `${i.index}번째` : i.severity === "skipped" ? "건너뜀" : i.severity === "warning" ? "경고" : "참고"}
          </Badge>
          <span className="t-body-s c-secondary">{i.detail}</span>
        </div>
      ))}
      {issues.length > limit && <span className="t-caption c-tertiary">외 {n(issues.length - limit)}건</span>}
    </div>
  );
}

/**
 * B3-j [건너뛴 N개 JSON으로 복사]. 건너뛴 항목만 원문 그대로 다시 묶는다 (번호는 1부터).
 * 최상위 deck 같은 설정은 남겨, 고쳐서 다시 붙여넣어도 같은 덱으로 들어가게 한다
 */
function skippedJson(text: string, issues: ImportIssue[]) {
  const idx = [...new Set(issues.filter((i) => i.severity === "skipped" && i.index !== null).map((i) => i.index!))].sort((a, b) => a - b);
  if (!idx.length) return null;
  try {
    const doc = JSON.parse(text);
    const items: unknown[] = Array.isArray(doc) ? doc : doc.cards;
    const picked = idx.map((i) => items[i - 1]).filter((v) => v !== undefined);
    if (!picked.length) return null;
    return { json: JSON.stringify(Array.isArray(doc) ? picked : { ...doc, cards: picked }, null, 2), count: picked.length };
  } catch {
    return null;
  }
}

function JsonImport({ deckId, onCancel }: { deckId: number | null; onCancel: () => void }) {
  const nav = useNav();
  const [mode, setMode] = useState<"paste" | "file">("paste");
  const [text, setText] = useState("");
  const [source, setSource] = useState("붙여넣기");
  const [preview, setPreview] = useState<JsonPreview | null>(null);
  const [example, setExample] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const file = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!text.trim()) return setPreview(null);
    const t = setTimeout(
      () =>
        api.previewJson(text, deckId).then(
          (p) => {
            setPreview(p);
            setError(undefined); // 앞선 미리보기 실패 문구를 지운다
          },
          (e) => setError(errorText(e)),
        ),
      250,
    );
    return () => clearTimeout(t);
  }, [text, deckId]);

  const readFile = (f: File) => {
    const r = new FileReader();
    r.onload = () => {
      setText(String(r.result));
      setSource(f.name);
    };
    r.readAsText(f);
  };

  // .json 끌어놓기. 웹뷰 이벤트는 경로만 주므로 내용은 백엔드에서 읽는다
  const [drag] = useFileDrop(!busy, JSON_FILE, (path) =>
    api.readJsonFile(path).then(
      (t) => {
        setText(t);
        setSource(baseName(path));
        setMode("file");
        setError(undefined);
      },
      (e) => setError(errorText(e)),
    ),
  );
  const shownError = drag === "wrong" ? "JSON 파일(.json)만 넣을 수 있어요. Anki 덱과 CSV는 다른 탭에서 가져와요." : error;

  const run = async () => {
    if (!preview || preview.ok === 0 || busy) return;
    setBusy(true);
    try {
      const result = await api.importJson(text, source, deckId);
      // 결과 화면에는 원문이 없어서 복사할 항목과 덱 이름을 여기서 만들어 넘긴다
      carried.set(result, { roots: preview.roots, skipped: skippedJson(text, result.issues) });
      nav({ name: "importResult", result });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <>
      <div className="import__json">
        {drag === "over" && <div className="import__json-drop t-title-m">놓으면 바로 검사해요</div>}
        <div className="import__json-head">
          <span className="import__tabs t-label-m">
            <button className={mode === "paste" ? "c-primary" : "c-tertiary"} onClick={() => setMode("paste")}>
              붙여넣기
            </button>
            <button
              className={mode === "file" ? "c-primary" : "c-tertiary"}
              onClick={() => {
                setMode("file");
                file.current?.click();
              }}
            >
              파일 선택
            </button>
            <input ref={file} type="file" accept=".json,application/json" hidden onChange={(e) => e.target.files?.[0] && readFile(e.target.files[0])} />
          </span>
          <Button variant="ghost" size="M" icon="info" label="형식 예시 보기" onClick={() => setExample(true)} />
        </div>
        <textarea
          className="import__text"
          spellCheck={false}
          value={text}
          placeholder={EXAMPLE}
          onChange={(e) => {
            setText(e.target.value);
            setSource("붙여넣기");
          }}
        />
      </div>

      {preview && (
        <div className="import__result">
          {preview.error ? (
            <p className="t-body-m c-danger">{preview.error}</p>
          ) : (
            <>
              <div className="import__counts">
                <span>
                  <span className="t-title-l">{n(preview.ok)}</span>
                  <span className="t-body-s c-secondary">검사를 통과한 항목</span>
                </span>
                <span>
                  <span className="t-title-l c-danger">{n(preview.skipped)}</span>
                  <span className="t-body-s c-secondary">건너뜀</span>
                </span>
                <span>
                  <span className="t-title-l c-warn">{n(preview.warnings)}</span>
                  <span className="t-body-s c-secondary">경고</span>
                </span>
              </div>
              <IssueList issues={preview.issues} />
            </>
          )}
        </div>
      )}

      <div className="import__foot">
        <span className="t-body-s c-tertiary">
          {/* JSON 의 한 덩어리는 "항목", 학습 단위는 "카드" (B3-j). new_decks 는 새 하위 덱만 센다 */}
          {preview && !preview.error && preview.roots.length > 0
            ? `들어갈 덱 · ${preview.roots.join(", ")}${preview.new_decks > 0 ? ` · 하위 덱 ${n(preview.new_decks)}개 새로` : ""}`
            : "카드 배열만 있어도 돼요. 덱을 적지 않으면 '가져온 카드' 덱에 들어가요"}
        </span>
        <span className="import__foot-right">
          {shownError && <span className="t-body-s c-danger">{shownError}</span>}
          <Button variant="secondary" label="전체 취소" onClick={onCancel} />
          <Button
            label={preview && preview.skipped > 0 ? `검사를 통과한 ${n(preview.ok)}개 항목 가져오기` : `${n(preview?.ok ?? 0)}개 항목 가져오기`}
            onClick={run}
            busy={busy}
            busyLabel="가져오는 중…"
            disabled={!preview || !!preview.error || preview.ok === 0}
          />
        </span>
      </div>

      {example && (
        <Modal title="JSON 형식 예시" onClose={() => setExample(false)} wide>
          <pre className="import__example">{EXAMPLE}</pre>
          <p className="t-body-s c-secondary">
            front 와 back 만 있어도 돼요. deck 은 :: 로 하위 덱을 나눠요. importance 는 핵심 · 일반 · 보충 중 하나예요. 빈칸 카드는{" "}
            {"{{c1::...}}"} 를 쓰면 back 없이도 돼요. 수식은 {String.raw`\\( … \\)`} 가 인라인, {String.raw`\\[ … \\]`} 가 블록이에요. JSON 안에서는 역슬래시를 두 번 써요.
          </p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="닫기" onClick={() => setExample(false)} />
            <Button
              size="M"
              label="붙여넣기 칸에 넣기"
              onClick={() => {
                setText(EXAMPLE);
                setSource("예시");
                setExample(false);
              }}
            />
          </div>
        </Modal>
      )}
    </>
  );
}

// ---------------------------------------------------------------- B2-c

const TABLE_FILE = /\.(csv|tsv|txt)$/i;
type Enc = "utf-8" | "euc-kr";
type Role = "front" | "back" | "deck" | "tags" | "importance" | "none";
const ROLES: { value: Role; label: string }[] = [
  { value: "front", label: "앞면" },
  { value: "back", label: "뒷면" },
  { value: "deck", label: "하위 덱" },
  { value: "tags", label: "태그" },
  { value: "importance", label: "중요도" },
  { value: "none", label: "사용 안 함" },
];
/** 열 제목 → 역할. 내보내기(C1-xc)의 제목 "앞면,뒷면,하위 덱,태그,중요도" 도 손대지 않고 맞는다 */
const ALIASES: [Role, string[]][] = [
  ["front", ["앞면", "front", "질문", "단어", "문제", "question", "word", "term"]],
  ["back", ["뒷면", "back", "답", "정답", "뜻", "의미", "answer", "meaning", "definition"]],
  ["deck", ["하위 덱", "하위덱", "덱", "deck", "subdeck", "day"]],
  ["tags", ["태그", "tags", "tag"]],
  ["importance", ["중요도", "importance"]],
];
const roleOf = (head: string) => ALIASES.find(([, names]) => names.includes(head.trim().toLowerCase()))?.[0];
const DELIMS = [
  { value: ",", label: "쉼표" },
  { value: "\t", label: "탭" },
  { value: ";", label: "세미콜론" },
];


/** 인코딩: 자동이면 UTF-8 로 엄격하게 읽어 보고, 깨지면 EUC-KR (한국어 엑셀 CSV 에 흔하다) */
function decode(bytes: ArrayBuffer, enc: Enc | null): { text: string; enc: Enc } {
  if (enc !== "euc-kr") {
    try {
      return { text: new TextDecoder("utf-8", { fatal: enc === null }).decode(bytes).replace(/^﻿/, ""), enc: "utf-8" };
    } catch {
      // UTF-8 이 아니다
    }
  }
  return { text: new TextDecoder("euc-kr").decode(bytes), enc: "euc-kr" };
}

/** 첫 줄에 가장 많이 나오는 구분자. .tsv 는 탭 */
function detectDelim(text: string, name: string) {
  if (/\.tsv$/i.test(name)) return "\t";
  const first = text.slice(0, text.search(/\r?\n|$/));
  return DELIMS.map((d) => ({ d: d.value, k: first.split(d.value).length })).sort((a, b) => b.k - a.k)[0].d;
}

/** 열 제목으로 역할을 고른다. 제목이 없으면 앞 두 열을 앞면 · 뒷면으로. 같은 역할은 한 열만 */
function autoRoles(first: string[], named: boolean): Role[] {
  const used = new Set<Role>();
  return first.map((h, i) => {
    const r = named ? roleOf(h) : i === 0 ? "front" : i === 1 ? "back" : undefined;
    if (!r || used.has(r)) return "none";
    used.add(r);
    return r;
  });
}

/** 백엔드 문구는 JSON 키 이름이라 CSV 에서는 열 이름으로 바꾼다. 열 개수가 다른 행은 null 로 보내 건너뛰게 한다 */
const csvIssues = (issues: ImportIssue[]) =>
  issues.map((i) => ({
    ...i,
    detail:
      i.kind === "bad_item"
        ? "열 개수가 달라요"
        : i.detail
            .replace(/^질문\(front\)/, "앞면")
            .replace(/^답\(back\)/, "뒷면")
            .replace(/^중요도\(importance\)/, "중요도"),
  }));

/** 줄바꿈이 있는 글자 칸은 카드에서도 줄을 나눈다. 이미 HTML 이면 그대로 */
const cellHtml = (s: string) => (/<[a-z][^>]*>/i.test(s) ? s : s.replace(/\r?\n/g, "<br>"));
const size = (bytes: number) => (bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))}KB` : mb(bytes));
const colName = (i: number) => (i < 26 ? String.fromCharCode(65 + i) : `${i + 1}`);

function PillSelect<T extends string>({ label, value, options, onChange, accent = false, muted = false }: {
  label?: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  accent?: boolean;
  muted?: boolean;
}) {
  return (
    <label className={`csv__pill ${accent ? "is-accent" : ""} ${muted ? "is-muted" : ""}`}>
      {label && <span className="t-caption c-tertiary">{label}</span>}
      <select className="t-label-m" value={value} onChange={(e) => onChange(e.target.value as T)}>
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      <Icon name="chevron-down" size={14} scaled />
    </label>
  );
}

function CsvImport({ deckId, onCancel }: { deckId: number | null; onCancel: () => void }) {
  const nav = useNav();
  const [file, setFile] = useState<{ name: string; bytes: ArrayBuffer } | null>(null);
  const [failed, setFailed] = useState<string>();
  const [enc, setEnc] = useState<Enc | null>(null); // null = 자동
  const [delim, setDelim] = useState<string | null>(null);
  const [header, setHeader] = useState<boolean | null>(null);
  const [roles, setRoles] = useState<Role[]>([]);
  const [preview, setPreview] = useState<JsonPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const input = useRef<HTMLInputElement>(null);

  const load = (name: string, bytes: ArrayBuffer) => {
    setFile({ name, bytes });
    setFailed(undefined);
    setEnc(null);
    setDelim(null);
    setHeader(null);
    setPreview(null);
  };
  const [drag] = useFileDrop(!busy, TABLE_FILE, (path) =>
    api.readTableFile(path).then(
      (b) => load(baseName(path), b),
      (e) => setFailed(errorText(e)),
    ),
  );

  // 자동 감지는 파일마다 한 번. 사용자가 고른 인코딩이 자동과 다를 때만 다시 읽는다
  const auto = useMemo(() => (file ? decode(file.bytes, null) : null), [file]);
  const table = useMemo(() => {
    if (!file || !auto) return null;
    const d = enc === null || enc === auto.enc ? auto : decode(file.bytes, enc);
    const autoDelim = detectDelim(d.text, file.name);
    return { ...d, autoDelim, rows: parseCsv(d.text, delim ?? autoDelim) };
  }, [file, auto, enc, delim]);
  const first = table?.rows[0] ?? [];
  const autoHeader = first.some((h) => roleOf(h));
  const named = header ?? autoHeader;
  const width = first.length;
  const data = useMemo(() => table?.rows.slice(named ? 1 : 0) ?? [], [table, named]);
  useEffect(() => setRoles(autoRoles(first, named)), [table, named]);
  const ready = roles.includes("front") && roles.includes("back");

  // JSON 가져오기와 같은 검증 · 결과를 쓰려고 행을 JSON 카드 모양으로 바꾼다. 행 번호(#N)가 그대로 맞는다
  const json = useMemo(() => {
    if (!file || !ready) return null;
    const at = (r: string[], role: Role) => (r[roles.indexOf(role)] ?? "").trim();
    // "하위 덱" 열은 최상위 덱 아래 경로다 (Day 12 → 토익 영단어::Day 12). 덱에서 열었으면 그 덱이, 아니면 파일 이름이 최상위
    const root = deckId === null ? file.name.replace(/\.[^.]+$/, "") : null;
    const sub = (v: string) => (v && root ? `${root}::${v}` : v || undefined);
    const cards = data.map((r) =>
      r.length > width || roles.indexOf("front") >= r.length || roles.indexOf("back") >= r.length
        ? null
        : {
            front: cellHtml(at(r, "front")),
            back: cellHtml(at(r, "back")),
            deck: sub(at(r, "deck")),
            tags: at(r, "tags") || undefined,
            importance: at(r, "importance") || undefined,
          },
    );
    return JSON.stringify(root ? { deck: root, cards } : { cards });
  }, [file, ready, data, roles, width, deckId]);

  useEffect(() => {
    if (!json) return setPreview(null);
    const t = setTimeout(
      () =>
        api.previewJson(json, deckId).then(
          (p) => {
            setPreview({ ...p, issues: csvIssues(p.issues) });
            setError(undefined);
          },
          (e) => setError(errorText(e)),
        ),
      150,
    );
    return () => clearTimeout(t);
  }, [json, deckId]);

  const run = async () => {
    if (!file || !json || !preview || preview.ok === 0 || busy) return;
    setBusy(true);
    try {
      const result = await api.importJson(json, file.name, deckId);
      result.issues = csvIssues(result.issues);
      carried.set(result, { roots: preview.roots, skipped: skippedJson(json, result.issues) });
      nav({ name: "importResult", result });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  const picker = (
    <input
      ref={input}
      type="file"
      accept=".csv,.tsv,.txt,text/csv"
      hidden
      onChange={(e) => {
        const f = e.target.files?.[0];
        e.target.value = ""; // 같은 파일을 다시 골라도 onChange 가 오게
        if (f) f.arrayBuffer().then((b) => load(f.name, b));
      }}
    />
  );
  const pick = () => input.current?.click();

  if (!file || !table) {
    return (
      <>
        {picker}
        {failed ? (
          <div className="import__fail">
            <Icon name="alert" size={20} className="c-danger" />
            <span className="import__fail-text">
              <span className="t-body-m-strong c-danger">가져오지 못했어요</span>
              <span className="t-body-s c-secondary">{safe(failed)}</span>
              <Button variant="secondary" size="M" icon="folder" label="다른 파일 고르기" onClick={pick} />
            </span>
          </div>
        ) : (
          <DropZone
            drag={drag}
            idle="CSV 파일을 여기로 끌어오세요"
            wrong={
              <>
                CSV 파일(.csv, .tsv, .txt)만 가져올 수 있어요.
                <br />
                Anki 덱과 JSON은 왼쪽 탭에서.
              </>
            }
            hint=".csv · .tsv   ·   원본 파일은 읽기만 하고 수정하지 않아요"
            help='엑셀에서 파일 만들기: 파일 → 다른 이름으로 저장 → "CSV UTF-8 (쉼표로 분리)"'
            onPick={pick}
          />
        )}
      </>
    );
  }

  const issues = preview?.issues.filter((i) => i.severity !== "info") ?? [];
  const delimOptions = DELIMS.map((d) => ({ ...d, label: d.value === table.autoDelim ? `${d.label} (자동)` : d.label }));
  const encOptions = (["utf-8", "euc-kr"] as Enc[]).map((e) => ({ value: e, label: `${e.toUpperCase()}${e === auto?.enc ? " (자동)" : ""}` }));
  const eucNote = enc === null && table.enc === "euc-kr";

  return (
    <>
      {picker}
      <div className="import__file">
        <span className="import__file-icon">
          <Icon name="file" size={22} className="c-accent" />
        </span>
        <span className="import__file-text">
          <span className="t-body-m-strong">{file.name}</span>
          <span className="t-body-s c-secondary">
            {size(file.bytes.byteLength)} · {n(data.length)}행 · 열 {n(width)}개
          </span>
        </span>
        <Button variant="ghost" size="M" label="다른 파일" onClick={pick} />
      </div>

      <div className="csv__options">
        <PillSelect label="구분자" value={delim ?? table.autoDelim} options={delimOptions} onChange={setDelim} />
        <PillSelect label="인코딩" value={enc ?? table.enc} options={encOptions} onChange={(v) => setEnc(v)} accent={eucNote} />
        <span className="csv__header">
          <Toggle on={named} onChange={setHeader} label="첫 줄은 열 제목" />
          <span className="t-body-s c-secondary">첫 줄은 열 제목</span>
        </span>
      </div>
      {eucNote && (
        <p className="csv__note t-caption">
          <Icon name="info" size={14} />
          엑셀에서 저장한 한글 파일이라 EUC-KR로 읽었어요. 글자가 깨져 보이면 인코딩을 바꿔 보세요.
        </p>
      )}

      <div className="csv__map" style={{ "--cols": Math.max(width, 1) } as CSSProperties}>
        <div className="csv__row csv__row--head">
          {first.map((h, i) => (
            <span key={i} className="csv__cell">
              <span className="t-caption c-tertiary">
                열 {colName(i)}
                {named && h.trim() ? ` · ${h.trim()}` : ""}
              </span>
              <PillSelect
                value={roles[i] ?? "none"}
                options={ROLES}
                muted={(roles[i] ?? "none") === "none"}
                onChange={(v) =>
                  // 같은 역할을 다른 열에서 고르면 그 열은 "사용 안 함"으로 (역할마다 한 열)
                  setRoles((rs) => rs.map((r, k) => (k === i ? v : v !== "none" && r === v ? "none" : r)))
                }
              />
            </span>
          ))}
        </div>
        {data.slice(0, 3).map((r, k) => (
          <div key={k} className="csv__row">
            {first.map((_, i) => (
              <span key={i} className={`csv__cell t-body-s ${(roles[i] ?? "none") === "none" || !r[i]?.trim() ? "c-tertiary" : ""}`}>
                {r[i]?.trim() || "—"}
              </span>
            ))}
          </div>
        ))}
        <p className="csv__more t-caption c-tertiary">
          처음 {n(Math.min(3, data.length))}행 미리보기 · 전체 {n(data.length)}행
        </p>
      </div>

      <div className="csv__check">
        {!ready ? (
          <span className="t-body-s c-secondary">앞면과 뒷면으로 쓸 열을 골라 주세요</span>
        ) : preview?.error ? (
          <span className="t-body-s c-danger">{preview.error}</span>
        ) : (
          preview && (
            <>
              <span className="csv__count">
                <span className="t-title-m">{n(preview.ok)}</span>
                <span className="t-body-s c-secondary">검사를 통과한 항목</span>
              </span>
              <span className="csv__count">
                <span className="t-title-m c-danger">{n(preview.skipped)}</span>
                <span className="t-body-s c-secondary">건너뜀</span>
              </span>
              {preview.warnings > 0 && (
                <span className="csv__count">
                  <span className="t-title-m c-warn">{n(preview.warnings)}</span>
                  <span className="t-body-s c-secondary">경고</span>
                </span>
              )}
              {issues.slice(0, 3).map((i, k) => (
                <Badge key={k} tone={i.severity === "skipped" ? "danger" : "warn"}>
                  {i.index !== null ? `#${i.index} ` : ""}
                  {i.detail}
                </Badge>
              ))}
              {issues.length > 3 && <span className="t-caption c-tertiary">외 {n(issues.length - 3)}건</span>}
            </>
          )
        )}
      </div>

      <div className="import__foot">
        <span className="t-body-s c-tertiary">
          {preview && !preview.error && preview.roots.length > 0
            ? `들어갈 덱 · ${preview.roots.join(", ")}${preview.new_decks > 0 ? ` · 하위 덱 ${n(preview.new_decks)}개 새로` : ""}`
            : ""}
        </span>
        <span className="import__foot-right">
          {error && <span className="t-body-s c-danger">{error}</span>}
          <Button variant="secondary" label="전체 취소" onClick={onCancel} />
          <Button
            label={preview && preview.skipped > 0 ? `검사를 통과한 ${n(preview.ok)}개 항목 가져오기` : `${n(preview?.ok ?? 0)}개 항목 가져오기`}
            onClick={run}
            busy={busy}
            busyLabel="가져오는 중…"
            disabled={!ready || !preview || !!preview.error || preview.ok === 0}
          />
        </span>
      </div>
    </>
  );
}

// ---------------------------------------------------------------- B3

function report(r: ImportResult) {
  const lines = [
    `Addiem 가져오기 결과`,
    `파일: ${r.source}`,
    `시각: ${new Date().toLocaleString("ko-KR")}`,
    ``,
    `가져온 덱 ${n(r.decks)}개 (새 하위 덱 ${n(r.subdecks)}개)`,
    `가져온 카드 ${n(r.cards)}장 · 노트 ${n(r.notes)}개 · 미디어 ${n(r.media)}개`,
    `건너뛴 항목 ${n(r.skipped)}개 · 확인이 필요한 카드 ${n(r.check_cards)}장`,
  ];
  if (r.kind !== "json")
    lines.push(
      `필터 덱 ${n(r.filtered_decks)}개 (카드 ${n(r.filtered_cards)}장은 원래 덱으로) · ${r.history ? "옮긴" : "옮기지 않은"} 복습 기록 ${n(r.reviews)}건`,
    );
  lines.push("", "항목");
  for (const i of r.issues) lines.push(`[${i.severity}] ${i.index !== null ? `#${i.index} ` : ""}${i.detail}`);
  return lines.join("\n");
}

export function ImportResultScreen({ result: r }: { result: ImportResult }) {
  const nav = useNav();
  const toast = useToast();
  const [saved, setSaved] = useState<string>();
  const [error, setError] = useState<string>();
  const isJson = r.kind === "json";
  const carry = carried.get(r) ?? {};
  const degraded = r.issues.filter((i) => i.kind === "degraded_template");
  const others = r.issues.filter((i) => i.kind !== "degraded_template");
  const openDeck = () => nav(r.root_deck_id !== null ? { name: "deck", deckId: r.root_deck_id } : { name: "home" });
  useKey("Escape", openDeck);

  const saveReport = async () => {
    try {
      const path = await save({ defaultPath: `가져오기 결과 ${r.source}.txt`, filters: [{ name: "텍스트", extensions: ["txt"] }] });
      if (!path) return;
      await api.saveText(path, report(r));
      setError(undefined);
      setSaved("저장했어요");
    } catch (e) {
      setError(errorText(e));
    }
  };

  const copySkipped = async () => {
    if (!carry.skipped) return;
    try {
      await navigator.clipboard.writeText(carry.skipped.json);
      toast({ title: `건너뛴 ${n(carry.skipped.count)}개를 복사했어요`, text: "고친 뒤 JSON 탭에 다시 붙여넣으면 돼요" });
    } catch {
      toast({ tone: "error", title: "복사하지 못했어요", text: "가져온 카드는 그대로예요", action: { label: "다시 시도", onClick: copySkipped } });
    }
  };

  const overdue = carry.overdue ?? 0;
  const rows: { icon: IconName; scaled?: boolean; tone: string; title: string; desc: string; action?: () => void }[] = [
    ...degraded.map((i) => ({
      icon: "alert" as IconName,
      tone: "c-warn",
      title: "확인이 필요한 카드",
      desc: i.detail,
      action: r.root_deck_id !== null ? () => nav({ name: "cards", deckId: r.root_deck_id! }) : undefined,
    })),
    ...(r.filtered_decks > 0
      ? [{
          icon: "layers" as IconName,
          scaled: true,
          tone: "c-secondary",
          title: `Anki 필터 덱 ${n(r.filtered_decks)}개는 가져오지 않았어요`,
          desc: r.filtered_cards > 0 ? `필터 덱에 올라가 있던 ${n(r.filtered_cards)}장은 원래 덱으로 들어갔어요.` : "",
        }]
      : []),
    ...(r.reviews > 0
      ? [
          r.history
            ? {
                // B3-r 옮긴 기록은 지원한 것이라 초록. 아이콘은 Figma 의 20px 원본
                icon: "clock" as IconName,
                tone: "c-success",
                title: `복습 기록 ${n(r.reviews)}건을 옮겼어요`,
                desc: `Anki 일정 그대로 이어서 복습해요.${
                  overdue > 0 ? ` 오늘 밀린 복습 ${n(overdue)}장은 하루 ${BACKLOG_PER_DAY}장씩 올라와요.` : ""
                }`,
              }
            : {
                icon: "clock" as IconName,
                scaled: true,
                tone: "c-secondary",
                title: `복습 기록 ${n(r.reviews)}건은 옮기지 않았어요`,
                desc: "고르신 대로 모든 카드가 새 일정으로 시작해요.",
              },
        ]
      : []),
  ];
  // 지원하지 않은 항목: 필터 덱, 옮기지 않은 복습 기록 (B3-r: 옮겼으면 빠진다)
  const unsupported = (r.filtered_decks > 0 ? 1 : 0) + (!r.history && r.reviews > 0 ? 1 : 0);

  // B3-j 아래 목록은 건너뛴 것(가져오지 않음)과 경고(가져왔지만 확인)로 나눈다
  const groups = [
    { title: "건너뛴 항목", tone: "c-danger", issues: r.issues.filter((i) => i.severity === "skipped") },
    { title: "경고", tone: "c-warn", issues: r.issues.filter((i) => i.severity === "warning") },
    { title: "참고", tone: "c-secondary", issues: r.issues.filter((i) => i.severity === "info") },
  ].filter((g) => g.issues.length > 0);
  const warnN = groups.find((g) => g.title === "경고")?.issues.length ?? 0;
  const deckSub = isJson
    ? [carry.roots?.join(", "), r.subdecks > 0 && `하위 덱 ${n(r.subdecks)}개 새로`].filter(Boolean).join(" · ") || "새 하위 덱 없음"
    : `새 하위 덱 ${n(r.subdecks)}개`;

  return (
    <main className="result">
      <div className="result__head">
        <span className="result__icon">
          <Icon name="check" size={26} className="c-success" />
        </span>
        <span className="result__title">
          <h1 className="t-title-l">가져오기를 마쳤어요</h1>
          <span className="t-body-s c-secondary">
            {r.source} ·{" "}
            {!isJson
              ? "원본 파일은 그대로예요"
              : r.skipped > 0
                ? `항목 ${n(r.notes + r.skipped)}개 중 ${n(r.notes)}개를 가져왔어요 · 기존 카드는 그대로예요`
                : `항목 ${n(r.notes)}개를 모두 가져왔어요 · 기존 카드는 그대로예요`}
          </span>
        </span>
      </div>
      <div className="result__tiles">
        <StatTile label="가져온 덱" value={n(r.decks)} sub={deckSub} />
        <StatTile
          label="가져온 카드"
          value={n(r.cards)}
          sub={isJson ? `항목 ${n(r.notes)}개${r.cards > r.notes ? " · 빈칸 카드 포함" : ""}` : `미디어 ${n(r.media)}개`}
        />
        {isJson ? (
          <>
            <div className={`stat ${r.skipped > 0 ? "stat--danger" : ""}`}>
              <p className="t-body-s c-secondary">건너뛴 항목</p>
              <p className="t-title-l">{n(r.skipped)}</p>
              <p className="t-caption c-tertiary">{r.skipped > 0 ? "가져오지 않았어요" : "없음"}</p>
            </div>
            <div className={`stat ${warnN > 0 ? "stat--warn" : ""}`}>
              <p className="t-body-s c-secondary">경고</p>
              <p className="t-title-l">{n(warnN)}</p>
              <p className="t-caption c-tertiary">{warnN > 0 ? "가져왔지만 확인해 보세요" : "없음"}</p>
            </div>
          </>
        ) : (
          <>
            <div className={`stat ${r.check_cards > 0 ? "stat--warn" : ""}`}>
              <p className="t-body-s c-secondary">확인이 필요한 카드</p>
              <p className="t-title-l">{n(r.check_cards)}</p>
              <p className="t-caption c-tertiary">{r.check_cards > 0 ? "템플릿 기능" : "없음"}</p>
            </div>
            <StatTile label="지원하지 않은 항목" value={`${n(unsupported)}종`} sub={unsupported > 0 ? "아래에서 확인" : "없음"} />
          </>
        )}
      </div>

      {isJson
        ? groups.length > 0 && (
            <div className="result__groups">
              {groups.map((g, k) => (
                <Fragment key={g.title}>
                  {k > 0 && <hr className="result__divider" />}
                  <div className="result__group-head">
                    <span className={`t-body-m-strong ${g.tone}`}>
                      {g.title} {n(g.issues.length)}
                    </span>
                    {g.title === "건너뛴 항목" && carry.skipped && (
                      <Button variant="ghost" size="M" icon="file" label={`건너뛴 ${n(carry.skipped.count)}개 JSON으로 복사`} onClick={copySkipped} />
                    )}
                  </div>
                  <IssueList issues={g.issues} />
                </Fragment>
              ))}
            </div>
          )
        : (rows.length > 0 || others.length > 0) && (
            <div className="result__list">
              {rows.map((row, k) => (
                <div key={k} className="result__row">
                  <Icon name={row.icon} size={20} scaled={row.scaled} className={row.tone} />
                  <span className="result__row-text">
                    <span className="t-body-m-strong">{row.title}</span>
                    {row.desc && <span className="t-body-s c-secondary">{row.desc}</span>}
                  </span>
                  {row.action && <Button variant="ghost" size="M" label="목록 보기" onClick={row.action} />}
                </div>
              ))}
              {others.length > 0 && (
                <div className="result__row">
                  <Icon name="info" size={20} scaled className="c-secondary" />
                  <span className="result__row-text">
                    <span className="t-body-m-strong">참고할 항목</span>
                    <IssueList issues={others} />
                  </span>
                </div>
              )}
            </div>
          )}

      <div className="result__foot">
        <Button variant="ghost" size="M" icon="download" label={saved ?? "결과를 파일로 저장"} onClick={saveReport} />
        <span className="import__foot-right">
          {error && <span className="t-body-s c-danger">{error}</span>}
          <Button variant="secondary" label="건너뛰고 덱 열기" onClick={openDeck} />
          {r.roots.length > 0 && (
            <Button
              icon="arrow-right"
              label="중요도 확인하기"
              onClick={() => nav({ name: "importance", deckId: r.roots[0], next: r.roots.slice(1) })}
            />
          )}
        </span>
      </div>
    </main>
  );
}
