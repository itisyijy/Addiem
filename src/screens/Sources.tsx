// I1 자료 · 강조 구간 (Figma 53:6560, 노트 53:6724). 덱에 붙인 강의 PDF·슬라이드에서 하이라이트·붉은 글씨·★·볼드 구간을 찾아 카드와 잇는다.
// 모델 없이 동작한다 (B-1). 모델이 있으면 자료를 넣은 뒤 카드와 자동으로 잇는다 (B-2, I3)
import "../styles/sources.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { api, errorText, type CardRow } from "../api";
import { Badge, Button, Checkbox, Icon, Segment as SegmentControl, type BadgeTone } from "../ui";
import { AiConnect, useAiStatus } from "../ai";
import { DraftSend } from "./Drafts";
import { eul, n, useKey, useNav } from "../nav";
import { useToast } from "../toast";
import { readPdf, renderPage } from "../pdf";
import { Modal } from "./NewDeck";
import { menuKeys } from "./Deck";
import { ExamDialog, examHint } from "./Exam";
import { Figures } from "./Figures";
import { PdfFigures, pdfItems, useHidden, usePdfScan } from "./PdfFigures";
import { labelOf, occlusionApi } from "../occlusion";
import { exam as examApi } from "../written";
import {
  cardSource,
  isRegion,
  MAX_PICK,
  mb,
  refreshModel,
  useModel,
  sourceApi,
  SIGNAL_LABEL,
  SIGNAL_ORDER,
  SIGNAL_TONE,
  topSignal,
  type AnalyzeProgress,
  type Material,
  type Para,
  type Segment,
  type Signal,
  type SlideObjects,
} from "../sourceApi";

/** 덱 상세 헤더의 [자료 N] (I1 노트). 미연결 강조 구간이 있으면 숫자를 붙이고 그 탭(R1-a)으로 연다 */
export function SourcesButton({ deckId }: { deckId: number }) {
  const nav = useNav();
  const [list, setList] = useState<Material[]>([]);
  useEffect(() => {
    sourceApi
      .list(deckId)
      .then(setList)
      .catch(() => setList([]));
  }, [deckId]);
  const gaps = list.reduce((t, m) => t + m.gaps, 0);
  const label = list.length ? `자료 ${n(list.length)}${gaps ? ` · 미연결 ${n(gaps)}` : ""}` : "자료";
  return <Button variant="secondary" size="M" icon="file" label={label} onClick={() => nav({ name: "sources", deckId, gaps: gaps > 0 })} />;
}

/** R1-a 정렬: ★ · 붉은 글씨 · 기출 구간 > 하이라이트 > 밑줄 > 볼드, 같은 등급은 쪽 순서 */
const GAP_RANK: Record<Signal, number> = { star: 0, red: 0, highlight: 1, underline: 2, bold: 3, figure: 3, table: 3 };
const gapRank = (s: Segment) => (s.exam.hits ? 0 : Math.min(...s.signals.map((g) => GAP_RANK[g] ?? 3)));
/** 보내는 글자 수 어림 (1구간 600자에서 자른다, rag.rs) */
const sentChars = (s: Segment) => Math.min(s.text.trim().length, 600);

export function SignalBadge({ signal }: { signal: Signal }) {
  return (
    <Badge tone={SIGNAL_TONE[signal] as BadgeTone}>
      <span className={signal === "underline" ? "u-line" : undefined}>{SIGNAL_LABEL[signal]}</span>
    </Badge>
  );
}

const rankOf = (s: Segment) => SIGNAL_ORDER.indexOf(topSignal(s));
const metaOf = (m: Material, gaps: boolean) =>
  m.status === "unsupported"
    ? "글자를 읽을 수 없어요"
    : m.status === "pending"
      ? "읽는 중…"
      : gaps
        ? `미연결 ${n(m.gaps)} · 강조 ${n(m.segments)}곳`
        : `${n(m.pages)}${m.kind === "pptx" ? "장" : "쪽"} · 강조 ${n(m.segments)}곳`;

/** 분석 진행 (I3 노트 "분석 중 2/3 · 기출_2023_필기.pdf"). 이벤트를 듣고, 이미 돌고 있으면 그 상태로 시작한다 */
export function useAnalyzeProgress(initial: AnalyzeProgress | null | undefined) {
  const [p, setP] = useState<AnalyzeProgress | null>(initial ?? null);
  useEffect(() => setP(initial ?? null), [initial]);
  useEffect(() => {
    const off = listen<AnalyzeProgress>("source-analyze", (e) => setP(e.payload));
    return () => {
      off.then((f) => f());
    };
  }, []);
  return [p, setP] as const;
}

export function AnalyzeBar({ p }: { p: AnalyzeProgress }) {
  return (
    <div className="src-progress" role="status">
      <span className="t-body-s c-secondary">
        분석 중 {p.step + 1}/{p.steps} · {p.label}
      </span>
      <span className="src-progress__track">
        <span className="src-progress__fill" style={{ width: `${p.total ? Math.round((p.done / p.total) * 100) : 0}%` }} />
      </span>
    </div>
  );
}

/** materialId · segmentId: I2 패널에서 들어올 때 그 구간을 보여준다. link: 카드 잇기 칸을 연 채로 */
export function Sources({ deckId, materialId, segmentId, link, gaps: gapsFirst }: { deckId: number; materialId?: number; segmentId?: number; link?: boolean; gaps?: boolean }) {
  const nav = useNav();
  const toast = useToast();
  const [deckName, setDeckName] = useState("");
  const [materials, setMaterials] = useState<Material[] | null>(null);
  const [sel, setSel] = useState<number | undefined>(materialId);
  const [segs, setSegs] = useState<Segment[]>([]);
  const [page, setPage] = useState(1);
  const [active, setActive] = useState<number | undefined>(segmentId);
  const [linking, setLinking] = useState<number | undefined>(link ? segmentId : undefined);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string>();
  const [progress, setProgress] = useAnalyzeProgress(null);
  const [stale, setStale] = useState(0);
  const model = useModel();
  const reading = useRef(new Set<number>());
  // R1-a 빠진 범위: 고른 구간 · 쪽 지도 거르기 · R1-b 창 · L2 연결 창
  const [gapView, setGapView] = useState(!!gapsFirst);
  const [picked, setPicked] = useState<number[]>([]);
  const [pageOnly, setPageOnly] = useState<number | null>(null);
  const [drafting, setDrafting] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const ai = useAiStatus();
  // 8.4 기출 구간: ⋯ 메뉴 · 시험 고르기 창 · 연결된 카드에 퍼뜨릴지 묻는 한 줄
  const [segMenu, setSegMenu] = useState<number | null>(null);
  const [examSeg, setExamSeg] = useState<Segment | null>(null);
  const [spread, setSpread] = useState<{ seg: number; name: string | null; cards: number[] } | null>(null);
  // 8.4 그림 · 표 (IT1 · IT1-p): 세 번째 탭. PPTX 슬라이드별 그림 · 표, PDF 쪽마다 찾은 그림 · 표 제안은 자료를 고를 때 한 번 읽는다
  const [objView, setObjView] = useState(false);
  const [objs, setObjs] = useState<SlideObjects[] | null>(null);
  const [objErr, setObjErr] = useState<string>();
  const [filesOpen, setFilesOpen] = useState(false);

  const reload = useCallback(async () => {
    const list = await sourceApi.list(deckId);
    setMaterials(list);
    sourceApi.unanalyzed(deckId).then(setStale, () => {});
    setSel((s) => (s && list.some((m) => m.id === s) ? s : list[0]?.id));
    return list;
  }, [deckId]);

  // 모델이 있으면 자료와 카드를 잇고 중요도 제안을 다시 만든다 (I3). 모델이 없으면 아무것도 하지 않는다
  const analyze = useCallback(async () => {
    const st = await refreshModel().catch(() => null);
    if (!st?.ready) return;
    try {
      await sourceApi.analyze(deckId);
    } catch (e) {
      setError(errorText(e));
    }
    setProgress(null);
    await reload();
  }, [deckId, reload, setProgress]);

  useEffect(() => {
    api.deckDetail(deckId).then((d) => setDeckName(d.name));
    reload();
    refreshModel()
      .then((s) => s.analyzing && setProgress(s.analyzing))
      .catch(() => {});
  }, [deckId, reload, setProgress]);

  // 아직 읽지 않은 PDF 는 여기서 pdf.js 로 읽어 구간을 보낸다 (앱을 닫았다 열어도 이어서)
  useEffect(() => {
    const todo = materials?.filter((m) => m.kind === "pdf" && m.status === "pending" && !reading.current.has(m.id)) ?? [];
    if (!todo.length) return;
    (async () => {
      for (const m of todo) {
        reading.current.add(m.id);
        try {
          const r = await readPdf(m.id);
          await sourceApi.savePdf(m.id, r.pages, r.segments, r.readable);
        } catch {
          // 암호가 걸렸거나 깨진 PDF 도 목록에 남겨 "읽을 수 없어요"로 알린다
          await sourceApi.savePdf(m.id, 0, [], false).catch(() => {});
        }
      }
      await reload();
      analyze();
    })();
  }, [materials, reload, analyze]);

  const current = materials?.find((m) => m.id === sel);
  const pptxId = current?.kind === "pptx" && current.status === "ready" ? current.id : undefined;
  // PDF 는 글자를 못 읽는 스캔본도 쪽을 그린다 (IT10). 열지 못한 PDF(쪽 0)는 뺀다
  const pdfId = current?.kind === "pdf" && current.status !== "pending" && current.pages > 0 ? current.id : undefined;
  const figId = pptxId ?? pdfId;
  const scan = usePdfScan(pdfId ? current : undefined);
  const [hidden, hide] = useHidden(pdfId);
  useEffect(() => {
    if (!figId) setObjView(false);
  }, [figId]);
  useEffect(() => {
    setObjs(null);
    setObjErr(undefined);
    if (!pptxId) return;
    let live = true;
    sourceApi.slideObjects(pptxId).then(
      (o) => live && setObjs(o),
      (e) => live && (setObjs([]), setObjErr(errorText(e))),
    );
    return () => {
      live = false;
    };
  }, [pptxId]);
  const loadSegs = useCallback(() => {
    if (!sel) return setSegs([]);
    sourceApi.segments(sel).then(setSegs);
  }, [sel]);
  useEffect(() => {
    loadSegs();
    setDeleting(false);
  }, [loadSegs, current?.status]);
  useEffect(() => {
    setPicked([]);
    setPageOnly(null);
  }, [sel]);
  const jumped = useRef(false);
  useEffect(() => {
    const s = segs.find((x) => x.id === active);
    if (jumped.current || !s || !figId) return;
    jumped.current = true;
    if (isRegion(s)) setObjView(true);
  }, [segs, active, figId]);
  // 다른 자료를 고르면 첫 쪽부터. 구간에서 들어왔으면 그 쪽으로
  useEffect(() => {
    const s = segs.find((x) => x.id === active);
    setPage(s ? s.page : 1);
  }, [sel, segs, active]);

  useKey("Escape", () => nav({ name: "deck", deckId }), linking === undefined && !drafting && !connecting);
  // 검색 칸에 글을 쓰는 중에도 Esc 로 카드 잇기 칸만 닫는다
  useKey("Escape", () => setLinking(undefined), linking !== undefined, { modal: true });
  useKey("ArrowLeft", () => setPage((p) => Math.max(1, p - 1)), !!current && linking === undefined);
  useKey("ArrowRight", () => setPage((p) => Math.min(current?.pages ?? 1, p + 1)), !!current && linking === undefined);

  const add = async () => {
    setError(undefined);
    const picked = await open({ multiple: true, directory: false, filters: [{ name: "강의 자료", extensions: ["pdf", "pptx"] }] });
    const paths = (Array.isArray(picked) ? picked : picked ? [picked] : []) as string[];
    let last: Material | undefined;
    for (const p of paths) {
      try {
        last = await sourceApi.add(deckId, p);
      } catch (e) {
        setError(errorText(e));
      }
    }
    if (!last) return;
    await reload();
    setSel(last.id);
    setActive(undefined);
    // PPTX 는 바로 읽혔다. PDF 는 위의 읽기가 끝난 뒤 분석한다
    if (last.kind === "pptx") analyze();
  };

  const remove = async () => {
    if (!current) return;
    try {
      await sourceApi.remove(current.id);
      toast({ title: `${current.file_name}${eul(current.file_name)} 지웠어요`, text: "카드와 그 학습 기록은 그대로예요" });
      setSel(undefined);
      await reload();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const changed = () => {
    loadSegs();
    reload();
  };

  /** 구간에 기출 표시. 연결된 카드가 있으면 그 카드에도 표시할지 한 줄로 묻는다 (N3) */
  const markSeg = async (s: Segment, name: string | null) => {
    setExamSeg(null);
    setSegMenu(null);
    try {
      await examApi.mark("segment", [s.id], name);
      setSpread(s.links.length ? { seg: s.id, name, cards: s.links.map((l) => l.card_id) } : null);
      loadSegs();
    } catch (e) {
      setError(errorText(e));
    }
  };
  const spreadToCards = async () => {
    if (!spread) return;
    try {
      const done = await examApi.mark("card", spread.cards, spread.name);
      toast({ title: done ? `카드 노트 ${n(done)}개에 기출을 표시했어요` : "연결된 카드에 이미 표시돼 있어요" });
    } catch (e) {
      setError(errorText(e));
    }
    setSpread(null);
  };
  const clearSeg = async (s: Segment) => {
    setSegMenu(null);
    await examApi.set("segment", [{ id: s.id, hits: null, names: [] }]).catch((e) => setError(errorText(e)));
    loadSegs();
  };

  if (!materials) return null;
  // 강조 구간 · 빠진 범위는 글자 강조만. 그림 · 표 영역은 [그림·표 N] 탭에서 (IT13)
  const textSegs = segs.filter((s) => !isRegion(s));
  const sorted = [...textSegs].sort((a, b) => rankOf(a) - rankOf(b) || a.page - b.page || a.ord - b.ord);
  const linkedSegs = textSegs.filter((s) => s.links.length).length;
  const gapSegs = textSegs.filter((s) => !s.links.length).sort((a, b) => gapRank(a) - gapRank(b) || a.page - b.page || a.ord - b.ord);
  const listed = gapView ? gapSegs.filter((s) => pageOnly === null || s.page === pageOnly) : sorted;
  const pick = (id: number, on: boolean) => setPicked((p) => (on ? (p.length < MAX_PICK && !p.includes(id) ? [...p, id] : p) : p.filter((x) => x !== id)));
  const pickedChars = segs.filter((s) => picked.includes(s.id)).reduce((t, s) => t + sentChars(s), 0);
  // 연결 전이면 같은 버튼이 L2 를 먼저 연다. 연결하면 미리보기(R1-b)로 (보내는 것은 그 창에서 누를 때)
  const startDraft = () => (ai?.connected ? setDrafting(true) : setConnecting(true));
  const pdfList = pdfId ? pdfItems(scan, segs, hidden) : [];
  const objImages = objs?.flatMap((o) => o.images) ?? [];
  const objTables = objs?.flatMap((o) => o.tables).length ?? 0;
  const objOff = objImages.filter((x) => !x.supported).length;
  const suggs = pdfList.filter((x) => x.sugg).length;
  const objCount = pdfId ? pdfList.length : objImages.length + objTables;
  const objNote = pdfId
    ? [
        `그림 ${n(pdfList.filter((x) => x.kind === "figure").length)} · 표 ${n(pdfList.filter((x) => x.kind === "table" && !x.sugg).length)} · 제안 ${n(suggs)}`,
        scan && scan.list.length < (current?.pages ?? 0) ? `살피는 중 ${n(scan.list.length)}/${n(current!.pages)}쪽` : "",
      ]
        .filter(Boolean)
        .join(" · ")
    : `그림 ${n(objImages.length)} · 표 ${n(objTables)}${objOff ? ` · 미리 볼 수 없음 ${n(objOff)}` : ""}`;
  // 그림·표 탭은 왼쪽 파일 열 대신 알약에서 자료를 바꾼다 (IT1)
  const pill = current && (
    <span className="menu">
      <button className="src-pill t-label-m" aria-haspopup="menu" aria-expanded={filesOpen} onClick={() => setFilesOpen(!filesOpen)}>
        <Icon name="file" size={16} />
        <span className="src-pill__name">{current.file_name}</span>
        <Icon name="chevron-down" size={14} />
      </button>
      {filesOpen && (
        <div className="menu__list menu--panel src-pill__menu" role="menu" aria-label="자료 고르기" onKeyDown={menuKeys} onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setFilesOpen(false)}>
          {materials?.map((m, i) => (
            <button
              key={m.id}
              role="menuitem"
              autoFocus={m.id === sel || (!sel && i === 0)}
              className={`menu__item ${m.id === sel ? "is-active" : ""}`}
              onClick={() => {
                setFilesOpen(false);
                setSel(m.id);
                setActive(undefined);
                setLinking(undefined);
              }}
            >
              <Icon name={m.status === "unsupported" ? "alert" : "file"} size={16} />
              <span className="menu__text">
                <span className="t-label-m">{m.file_name}</span>
                <span className="t-caption c-tertiary">{metaOf(m, false)}</span>
              </span>
            </button>
          ))}
        </div>
      )}
    </span>
  );
  const head = (
    <div className="src-list__head">
      <SegmentControl
        boxed
        label="구간 보기"
        value={objView ? "objects" : gapView ? "gaps" : "all"}
        onChange={(v) => {
          setGapView(v === "gaps");
          setObjView(v === "objects");
          setLinking(undefined);
        }}
        options={[
          { value: "all", label: `강조 구간 ${n(textSegs.length)}` },
          { value: "gaps", label: `미연결 강조 구간 ${n(gapSegs.length)}` },
          ...(figId ? [{ value: "objects", label: `그림·표 ${n(objCount)}` }] : []),
        ]}
      />
      <span className="t-caption c-tertiary">
        {objView
          ? objNote
          : gapView
            ? "기기 안에서 찾았어요 · 연결 없이 돼요"
            : `연결된 카드 ${n(linkedSegs)} · 연결 없음 ${n(textSegs.length - linkedSegs)}`}
      </span>
    </div>
  );

  return (
    <main className="sources">
      <button className="backlink t-label-m c-secondary" onClick={() => nav({ name: "deck", deckId })}>
        <Icon name="chevron-left" size={18} />
        {deckName}
      </button>
      <div className="deck__head">
        <div className="deck__title">
          <h1 className="t-title-l">학습 자료</h1>
          <p className="t-body-s c-secondary">
            {objView
              ? pdfId
                ? "쪽 안의 그림과 표를 골라 카드로 만들어요. 자동으로 못 찾은 것은 끌어서 영역을 잡아요."
                : "슬라이드마다 그림과 표를 목록에서 골라요. PPTX는 슬라이드를 그림으로 그리지 않아서 끌어서 잡기는 없어요."
              : gapView
                ? "강조 구간 중 연결된 카드가 없는 곳이에요. 이미 만든 카드가 있다면 연결해 주세요."
                : "강의 PDF·슬라이드의 하이라이트, 붉은 글씨, ★ 표시를 찾아 카드와 이어요."}
            {/* 다시 분석은 자동으로 하지 않는다. 지난 분석 뒤에 생긴 카드가 있을 때만 알린다 */}
            {stale > 0 && model?.ready && !progress && (
              <>
                {" "}
                · 분석 뒤에 추가한 카드 {n(stale)}장 ·{" "}
                <button className="src-inline t-label-m c-accent" onClick={analyze}>
                  분석하기
                </button>
              </>
            )}
          </p>
        </div>
        <Button variant="secondary" size="M" icon="plus" label="학습 자료 추가" onClick={add} />
      </div>
      {progress && <AnalyzeBar p={progress} />}
      {error && <p className="t-body-s c-danger">{error}</p>}

      {materials.length === 0 ? (
        <div className="panel src-empty">
          <Icon name="file" size={24} className="c-tertiary" />
          <p className="t-title-s">아직 자료가 없어요</p>
          <p className="t-body-s c-secondary">텍스트를 고를 수 있는 PDF나 PPTX를 넣으면 강조된 곳을 찾아 보여줘요.</p>
          <p className="t-caption c-tertiary">손으로 그린 별표·밑줄과 스캔본(글자를 고를 수 없는 PDF)은 읽지 못해요.</p>
        </div>
      ) : (
        <div className="src-cols">
          <div className="src-files" hidden={objView}>
            {materials.map((m) => {
              const bad = m.status === "unsupported";
              const on = m.id === sel;
              return (
                <div key={m.id} className={`src-file ${on ? "is-selected" : ""}`}>
                  <button
                    className="src-file__main"
                    aria-current={on || undefined}
                    onClick={() => {
                      setSel(m.id);
                      setActive(undefined);
                      setLinking(undefined);
                    }}
                  >
                    <Icon name={bad ? "alert" : "file"} size={18} className={bad ? "c-warn" : "c-secondary"} />
                    <span className="src-file__text">
                      <span className={`t-label-m ${bad ? "c-tertiary" : "c-primary"}`}>{m.file_name}</span>
                      <span className={`t-caption ${bad ? "c-warn" : "c-tertiary"}`}>{metaOf(m, gapView)}</span>
                    </span>
                  </button>
                  {on && !deleting && (
                    <button className="src-file__del" aria-label={`${m.file_name} 지우기`} onClick={() => setDeleting(true)}>
                      <Icon name="trash" size={16} />
                    </button>
                  )}
                  {on && deleting && (
                    <div className="src-file__confirm">
                      <span className="t-caption c-secondary">지워도 카드는 그대로, 연결만 끊겨요</span>
                      <span className="src-file__confirm-btns">
                        <Button variant="ghost" size="M" label="취소" onClick={() => setDeleting(false)} />
                        <Button variant="danger" size="M" label="지우기" onClick={remove} />
                      </span>
                    </div>
                  )}
                </div>
              );
            })}
            <p className="t-caption c-tertiary">손으로 그린 별표·밑줄과 스캔본(글자를 고를 수 없는 PDF)은 읽지 못해요.</p>
            {model?.ready && <p className="src-model t-caption c-tertiary">분석 모델 있음 · 설정에서 지울 수 있어요</p>}
          </div>

          {current && objView && pptxId && (
            <Figures
              m={current}
              objs={objs}
              error={objErr}
              segs={segs}
              deckId={deckId}
              head={head}
              pill={pill}
              linking={linking}
              onLinking={setLinking}
              links={(s) => <LinkEditor deckId={deckId} seg={s} onChange={changed} />}
              onChanged={changed}
            />
          )}
          {current && objView && pdfId && (
            <PdfFigures
              m={current}
              items={pdfList}
              scanning={!scan || scan.list.length < current.pages}
              scanned={current.status === "unsupported"}
              deckId={deckId}
              head={head}
              pill={pill}
              page={page}
              onPage={setPage}
              linking={linking}
              onLinking={setLinking}
              links={(s) => <LinkEditor deckId={deckId} seg={s} onChange={changed} />}
              onChanged={changed}
              hide={hide}
            />
          )}
          {current && !objView && gapView && current.status === "ready" && <PageMap m={current} segs={segs} page={pageOnly} onPage={setPageOnly} />}
          {current && !objView && !gapView && (
            <Preview
              m={current}
              page={page}
              segs={segs.filter((s) => s.page === page)}
              active={active}
              onPage={setPage}
              onPick={(id) => {
                setActive(id);
                document.getElementById(`seg-${id}`)?.scrollIntoView({ block: "nearest" });
              }}
            />
          )}

          {/* 그림·표 탭은 Figures 가 같은 자리에 목록을 그린다 */}
          <section className="panel src-list" aria-label="강조된 구간" hidden={objView}>
            {head}
            {current?.status === "unsupported" && (
              <p className="src-list__note t-body-s c-secondary">
                이 자료에서는 글자를 읽을 수 없어요. 스캔본이나 손글씨 자료의 강조는 아직 찾지 못해요.
                {pdfId ? " [그림·표] 탭에서 끌어서 잡은 영역으로 그림 카드를 만들 수 있어요." : ""}
              </p>
            )}
            {current?.status === "ready" && textSegs.length === 0 && (
              <p className="src-list__note t-body-s c-secondary">하이라이트·붉은 글씨·★·볼드로 강조된 곳을 찾지 못했어요.</p>
            )}
            {gapView && current?.status === "ready" && listed.length === 0 && (
              <p className="src-list__note t-body-s c-secondary">
                {pageOnly === null ? "카드 미연결 강조 구간이 없어요. 강조된 곳마다 연결된 카드가 있어요." : `${n(pageOnly)}쪽에는 카드 미연결 강조 구간이 없어요.`}
              </p>
            )}
            {listed.map((s) => (
              <div key={s.id} id={`seg-${s.id}`} className={`src-seg ${s.id === active && !gapView ? "is-active" : ""} ${gapView && picked.includes(s.id) ? "is-picked" : ""}`}>
                <div className="src-seg__row" onClick={() => (gapView ? pick(s.id, !picked.includes(s.id)) : setActive(s.id))}>
                  {gapView && (
                    <span className="src-seg__check">
                      <Checkbox
                        quiet
                        label={`${s.text} 고르기`}
                        checked={picked.includes(s.id)}
                        disabled={!picked.includes(s.id) && picked.length >= MAX_PICK}
                        onChange={(v) => pick(s.id, v)}
                      />
                    </span>
                  )}
                  <div className="src-seg__body">
                    <span className="src-seg__meta">
                      {s.signals.map((g) => (
                        <SignalBadge key={g} signal={g} />
                      ))}
                      {s.exam.hits !== null && <span className="badge src-seg__exam t-label-s">기출{s.exam.names.length ? ` · ${s.exam.names.join(" · ")}` : ""}</span>}
                      <span className="t-caption c-tertiary">p.{s.page}</span>
                      {s.exam.hits === null && examHint(s.text) && (
                        <button
                          className="exam-chip exam-chip--suggest t-label-s"
                          title="눌러야 기출 구간으로 표시돼요"
                          onClick={(e) => {
                            e.stopPropagation();
                            markSeg(s, null);
                          }}
                        >
                          <span className="exam-chip__dot" />글자에 "{examHint(s.text)}" · 제안
                          <Icon name="plus" size={12} />
                        </button>
                      )}
                    </span>
                    <p className="t-body-s c-primary">{s.text}</p>
                  </div>
                  {s.links.length ? (
                    <button
                      className="badge badge--accent t-label-s src-seg__count"
                      aria-expanded={linking === s.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        setActive(s.id);
                        setLinking(linking === s.id ? undefined : s.id);
                      }}
                    >
                      카드 {n(s.links.length)}장
                    </button>
                  ) : (
                    <Button
                      variant="ghost"
                      size="M"
                      icon="plus"
                      label="기존 카드 연결"
                      aria-expanded={linking === s.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        setActive(s.id);
                        setLinking(linking === s.id ? undefined : s.id);
                      }}
                    />
                  )}
                  <span className="menu" onClick={(e) => e.stopPropagation()}>
                    <button className="src-seg__more" aria-label="구간 메뉴" aria-haspopup="menu" aria-expanded={segMenu === s.id} onClick={() => setSegMenu(segMenu === s.id ? null : s.id)}>
                      <Icon name="more" size={16} />
                    </button>
                    {segMenu === s.id && (
                      <div className="menu__list menu--panel src-seg__menu" role="menu" aria-label="구간 메뉴" onKeyDown={menuKeys}>
                        {s.exam.hits === null ? (
                          <button role="menuitem" autoFocus className="menu__item menu__item--top" onClick={() => (setSegMenu(null), setExamSeg(s))}>
                            <Icon name="target" size={16} scaled />
                            <span className="menu__text">
                              <span className="t-body-m">기출 구간으로 표시</span>
                              <span className="t-caption c-tertiary">미연결 강조 구간에서 ★처럼 먼저 보여요</span>
                            </span>
                          </button>
                        ) : (
                          <button role="menuitem" autoFocus className="menu__item menu__item--top" onClick={() => clearSeg(s)}>
                            <Icon name="target" size={16} scaled />
                            <span className="menu__text">
                              <span className="t-body-m">기출 표시 빼기</span>
                              <span className="t-caption c-tertiary">{s.exam.names.length ? s.exam.names.join(" · ") : `기출 ${n(s.exam.hits)}회`}</span>
                            </span>
                          </button>
                        )}
                      </div>
                    )}
                  </span>
                </div>
                {spread?.seg === s.id && (
                  <div className="src-spread" role="status">
                    <span className="t-body-s">
                      연결된 카드 {n(spread.cards.length)}장에도 {spread.name ? `"${spread.name}" ` : ""}기출을 표시할까요?
                    </span>
                    <Button variant="ghost" size="M" label="아니요" onClick={() => setSpread(null)} />
                    <Button variant="secondary" size="M" label="카드에도 표시" onClick={spreadToCards} />
                  </div>
                )}
                {linking === s.id && <LinkEditor deckId={deckId} seg={s} onChange={changed} />}
              </div>
            ))}
            {gapView && current?.status === "ready" && gapSegs.length > 0 && (
              <div className="src-pick">
                <span className="src-pick__text">
                  <span className="t-label-m">{picked.length ? `${n(picked.length)}구간 고름 · 약 ${n(pickedChars)}자` : "초안을 만들 구간을 골라 주세요"}</span>
                  <span className="t-caption c-tertiary">
                    {picked.length >= MAX_PICK ? `한 번에 ${MAX_PICK}구간까지 고를 수 있어요` : "ChatGPT 요금제 사용 · 보내기 전에 내용을 보여줘요"}
                  </span>
                </span>
                <Button size="M" icon="sparkles" label="카드 초안 만들기" disabled={!picked.length} onClick={startDraft} />
              </div>
            )}
          </section>
        </div>
      )}
      {examSeg && (
        <ExamDialog
          deckId={deckId}
          notes={[examSeg.exam]}
          cards={1}
          title="기출 구간으로 표시"
          sub="이 구간이 나온 시험을 골라요. 이 구간과 이은 카드는 기출 카드로 세요."
          unit="구간"
          onClose={() => setExamSeg(null)}
          onConfirm={(name) => markSeg(examSeg, name)}
        />
      )}
      {connecting && (
        <AiConnect
          onDone={(ok) => {
            setConnecting(false);
            if (ok) setDrafting(true);
          }}
        />
      )}
      {drafting && current && (
        <DraftSend
          picked={picked}
          onClose={() => setDrafting(false)}
          onDone={(draft, ids, amount, cloze) => nav({ name: "drafts", deckId, materialId: current.id, draft, picked: ids, amount, cloze })}
        />
      )}
    </main>
  );
}

/** R1-a 쪽 지도: 한 칸이 한 쪽. 카드 있음(accent) · 강조됐는데 카드 없음(warn) · 강조 없음(회색). 칸을 누르면 그 쪽만 */
function PageMap({ m, segs, page, onPage }: { m: Material; segs: Segment[]; page: number | null; onPage: (p: number | null) => void }) {
  const unit = m.kind === "pptx" ? "장" : "쪽";
  const state = (p: number) => {
    const on = segs.filter((s) => s.page === p && !isRegion(s));
    return !on.length ? "none" : on.some((s) => !s.links.length) ? "gap" : "linked";
  };
  const label = { linked: "카드가 있는 강조 구간", gap: "카드 미연결 강조", none: "강조 없음" } as const;
  return (
    <div className="panel src-map" aria-label="쪽 지도">
      <span className="t-label-m">
        쪽 지도 · {n(m.pages)}
        {unit}
      </span>
      <div className="src-map__pages">
        {Array.from({ length: m.pages }, (_, i) => i + 1).map((p) => {
          const st = state(p);
          return (
            <button
              key={p}
              type="button"
              className={`src-map__page is-${st} ${page === p ? "is-selected" : ""} t-caption`}
              aria-label={`${p}${unit} · ${label[st]}`}
              aria-pressed={page === p}
              onClick={() => onPage(page === p ? null : p)}
            >
              {p}
            </button>
          );
        })}
      </div>
      <div className="src-map__legend">
        {(["linked", "gap", "none"] as const).map((st) => (
          <span key={st} className="t-caption c-secondary">
            <span className={`src-map__swatch is-${st}`} />
            {label[st]}
          </span>
        ))}
      </div>
      <p className="t-caption c-tertiary">
        칸을 누르면 그 {unit}의 구간만 보여요{page !== null ? ` · ${n(page)}${unit} 선택됨` : ""}
      </p>
    </div>
  );
}

/** E1 데이터 칸의 [분석 모델] 행 (Settings.tsx). 모델은 앱 전체에서 하나라 설정에서 지운다. 받았을 때만 보인다 */
export function ModelRow() {
  const model = useModel();
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  if (!model?.ready) return null;
  const remove = async () => {
    setBusy(true);
    try {
      await sourceApi.deleteModel();
      setAsking(false);
      await refreshModel();
    } catch (e) {
      // 분석이나 내려받기 중이면 지우지 않는다 (embed.rs model_delete)
      setError(errorText(e));
    }
    setBusy(false);
  };
  return (
    <>
      <div className="prefs__row">
        <span className="prefs__text">
          <span className="t-body-m-strong">분석 모델</span>
          <span className="t-body-s c-secondary">약 {mb(model.bytes)} · 자료로 중요도를 제안할 때 써요</span>
        </span>
        <Button variant="ghost" size="M" label="지우기" onClick={() => setAsking(true)} />
      </div>
      {asking && (
        <Modal title="분석 모델을 지울까요?" onClose={() => setAsking(false)} variant="confirm" icon="trash">
          <p className="t-body-m c-secondary">이미 만든 연결과 제안은 그대로예요. 새 자료를 분석하려면 다시 받아야 해요.</p>
          {error && <p className="t-body-s c-danger">{error}</p>}
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setAsking(false)} />
            <Button variant="danger" size="M" icon="trash" label="지우기" busy={busy} onClick={remove} />
          </div>
        </Modal>
      )}
    </>
  );
}

/** 가운데 열: 쪽 미리보기와 강조 위치. PDF 는 그대로 그리고, PPTX 는 슬라이드 글을 줄로 보여준다 */
function Preview({
  m,
  page,
  segs,
  active,
  onPage,
  onPick,
}: {
  m: Material;
  page: number;
  segs: Segment[];
  active?: number;
  onPage: (p: number) => void;
  onPick: (id: number) => void;
}) {
  // 스캔본 PDF 도 쪽은 그린다 (IT10). 열지 못한 PDF(쪽 0)와 읽지 못한 PPTX 만 막는다
  const shown = m.status === "ready" || (m.kind === "pdf" && m.status === "unsupported" && m.pages > 0);
  return (
    <div className="src-preview">
      {!shown ? (
        <div className="src-page src-page--empty t-body-s c-tertiary">{m.status === "pending" ? "자료를 읽고 있어요…" : "미리 볼 수 없어요"}</div>
      ) : m.kind === "pdf" ? (
        <PdfPage materialId={m.id} page={page} width={300} segs={segs} active={active} onPick={onPick} />
      ) : (
        <SlidePage materialId={m.id} page={page} />
      )}
      {shown && (
        <span className="src-preview__nav">
          <button className="src-icon-btn" aria-label="이전 쪽" disabled={page <= 1} onClick={() => onPage(page - 1)}>
            <Icon name="chevron-left" size={16} scaled />
          </button>
          <span className="t-caption c-tertiary">
            {n(page)}
            {m.kind === "pptx" ? "장" : "쪽"} / {n(m.pages)}
          </span>
          <button className="src-icon-btn" aria-label="다음 쪽" disabled={page >= m.pages} onClick={() => onPage(page + 1)}>
            <Icon name="chevron-right" size={16} />
          </button>
        </span>
      )}
    </div>
  );
}

/** PDF 한 쪽 + 강조 위치 표시. I2 패널도 쓴다 (height 를 주면 활성 구간이 보이게 잘라 보여준다) */
export function PdfPage({
  materialId,
  page,
  width,
  height,
  segs,
  active,
  onPick,
}: {
  materialId: number;
  page: number;
  width: number;
  height?: number;
  segs: Segment[];
  active?: number;
  onPick?: (id: number) => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState<{ w: number; h: number }>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    let task: { cancel: () => void } | undefined;
    let live = true;
    setFailed(false);
    renderPage(materialId, page, c, width)
      .then((t) => {
        task = t;
        if (!live) return t.cancel();
        setSize({ w: width, h: parseFloat(c.style.height) });
        return t.promise;
      })
      .catch((e) => live && e?.name !== "RenderingCancelledException" && setFailed(true));
    return () => {
      live = false;
      task?.cancel();
    };
  }, [materialId, page, width]);
  const focus = segs.find((s) => s.id === active) ?? segs[0];
  // 잘라 보일 때는 연결 구간이 가운데 오게
  const top = height && size && focus?.rects?.length ? Math.max(0, Math.min(size.h - height, focus.rects[0][1] * size.h - height / 2)) : 0;
  return (
    <div className="src-page" style={{ width, height: height ?? size?.h ?? width * 1.33 }}>
      {failed && <span className="src-page__fail t-body-s c-tertiary">이 기기에는 자료 파일이 없어요</span>}
      <div className="src-page__inner" style={{ transform: `translateY(${-top}px)` }}>
        <canvas ref={canvas} />
        {segs.flatMap((s) =>
          (s.rects ?? []).map((r, i) => (
            <span
              key={`${s.id}-${i}`}
              className={`src-mark src-mark--${topSignal(s)} ${s.id === active ? "is-active" : ""}`}
              style={{ left: `${r[0] * 100}%`, top: `${r[1] * 100}%`, width: `${r[2] * 100}%`, height: `${r[3] * 100}%` }}
              onClick={onPick && (() => onPick(s.id))}
            />
          )),
        )}
      </div>
    </div>
  );
}

/** PPTX 한 장. 그림은 그리지 않고 글만, 강조된 문단은 신호 색으로. cells: false 면 표 칸 문단은 뺀다 (I2 가 표를 따로 그릴 때) */
export function SlidePage({ materialId, page, active, height, cells = true }: { materialId: number; page: number; active?: string; height?: number; cells?: boolean }) {
  const [paras, setParas] = useState<Para[]>();
  useEffect(() => {
    sourceApi
      .slide(materialId, page)
      .then(setParas)
      .catch(() => setParas([]));
  }, [materialId, page]);
  return (
    <div className="src-page src-page--slide" style={height ? { height } : undefined}>
      {paras?.length === 0 && <span className="t-caption c-tertiary">이 슬라이드에는 글이 없어요</span>}
      {paras?.filter((p) => cells || !p.cell).map((p, i) => (
        <p
          key={i}
          className={`src-slide__para ${p.signals.length ? `src-mark--${topSignal(p)}` : ""} ${p.text === active ? "is-active" : ""}`}
        >
          {p.text}
        </p>
      ))}
    </div>
  );
}

/** 카드 한 줄 (연결된 카드 · 검색 결과 공통): 첫 줄은 질문만, 둘째 줄 "답 · … · 직접 연결 · 덱".
 * 그림 가리기는 질문이 모두 같아 "그림 가리기 · n/N · 칸 이름(없으면 칸 n)" (M21) */
function CardSummary({ c, how }: { c: CardRow; how?: string }) {
  const io = c.occlusion;
  const [name, setName] = useState("");
  useEffect(() => {
    if (!io) return;
    let live = true;
    occlusionApi.note(c.note_id).then((o) => live && setName(labelOf(o.shapes, io.ordinal)), () => {});
    return () => {
      live = false;
    };
  }, [c.note_id, io]);
  const deck = c.deck_path.slice(1).join(" › ") || c.deck_path[0];
  const title = io ? `그림 가리기 · ${n(io.n)}/${n(io.count)} · ${name || `칸 ${n(io.ordinal)}`}` : c.question;
  const sub = [!io && c.answer && `답 · ${c.answer}`, how, deck].filter(Boolean).join(" · ");
  return (
    <span className="src-link__text">
      <span className="t-body-s c-primary">{title}</span>
      <span className="t-caption c-tertiary">{sub}</span>
    </span>
  );
}

/** [기존 카드 연결]: 이 덱 카드 검색 → 골라서 연결 (B-1 수동 연결). 연결된 카드는 끊을 수 있다 */
function LinkEditor({ deckId, seg, onChange }: { deckId: number; seg: Segment; onChange: () => void }) {
  const [q, setQ] = useState("");
  const [rows, setRows] = useState<CardRow[]>([]);
  const [linked, setLinked] = useState<CardRow[]>([]);
  const ids = seg.links.map((l) => l.card_id);
  const key = ids.join(",");
  useEffect(() => {
    const t = setTimeout(() => api.listCards(deckId, q, 8).then((r) => setRows(r.rows)), 150);
    return () => clearTimeout(t);
  }, [deckId, q]);
  useEffect(() => {
    if (!key) return setLinked([]);
    api.listCards(null, "", 50, false, key.split(",").map(Number)).then((r) => setLinked(r.rows));
  }, [key]);
  const act = async (fn: Promise<void>, cardId: number) => {
    await fn;
    cardSource(cardId, true); // 학습 화면의 [원자료] 버튼이 바로 따라오게
    onChange();
  };
  const method = (id: number) => {
    const l = seg.links.find((x) => x.card_id === id);
    return l?.method === "auto" ? "자동 연결" : l?.origin === "draft" ? "초안에서 만듦" : "직접 연결";
  };
  return (
    <div className="src-link" onClick={(e) => e.stopPropagation()}>
      {linked.length > 0 && (
        <div className="src-link__group">
          <span className="t-label-s c-secondary">연결된 카드</span>
          {linked.map((c) => (
            <div key={c.id} className="src-link__row">
              <CardSummary c={c} how={method(c.id)} />
              <Button variant="ghost" size="M" label="끊기" onClick={() => act(sourceApi.unlink(seg.id, c.id), c.id)} />
            </div>
          ))}
        </div>
      )}
      <input
        className="field__input t-body-s src-link__search"
        placeholder="이 덱 카드 검색"
        aria-label="이 덱 카드 검색"
        autoFocus
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      <div className="src-link__group">
        {rows.length === 0 && <span className="t-caption c-tertiary">맞는 카드가 없어요</span>}
        {rows.map((c) => (
          <div key={c.id} className="src-link__row">
            <CardSummary c={c} />
            {ids.includes(c.id) ? (
              <span className="t-label-s c-accent src-link__done">
                <Icon name="check" size={14} />
                연결됨
              </span>
            ) : (
              <Button variant="secondary" size="M" label="연결" onClick={() => act(sourceApi.link(seg.id, c.id), c.id)} />
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
