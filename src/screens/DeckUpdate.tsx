// 8.7 덱 업데이트 가져오기 (Figma Row DU, docs/engineering/deck-update.md 9장)
// B2-u0 업데이트로 알아보기 (가져오기 화면 안) · B2-u 업데이트 요약 · B2-k 충돌 검토 · B2-kt 카드 모양 비교 · B2-p 업데이트 적용 중 · B3-u 업데이트 결과
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorText,
  syncApi,
  type AnkiPreview,
  type LookConflict,
  type UpdateChoices,
  type UpdateConflict,
  type UpdateProgress,
  type UpdateSummary,
  type UpdateTypeChange,
} from "../api";
import { renderCardHtml } from "../cardHtml";
import { scopeCss } from "../cardCss";
import { Badge, Button, Icon, Kbd, Segment, Spinner, StatTile, type IconName } from "../ui";
import { eul, eun, lockNav, n, ro, useKey, useNav } from "../nav";
import { useSyncStatus } from "../sync";
import { IssueList } from "./Import";
import "../styles/update.css";

const day = (ms: number) => new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric" }).format(ms);
const clock = (ms: number) => new Intl.DateTimeFormat("ko-KR", { hour: "numeric", minute: "2-digit" }).format(ms);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** 덱 "이름" (업데이트한 덱의 최상위 덱) */
const deckOf = (s: UpdateSummary) => s.hint.known_decks[0];

// ---------------------------------------------------------------- 다른 기기 변경 먼저 받기 (DU15 · DU26)

type Pull = "off" | "pulling" | "done" | "failed";
const STUCK = ["offline", "error", "login", "quota", "update", "choice", "gone", "locked"];

/** 동기화를 켰으면 받기 → 올리기를 한 바퀴 돌리고 마칠 때까지 기다린다. 마지막 동기화 시각이 바뀌면 받은 것이다 */
async function pullFirst(onStart: () => void): Promise<Exclude<Pull, "pulling">> {
  const first = await syncApi.status().catch(() => null);
  if (!first?.connected || !first.enabled) return "off";
  onStart();
  const before = first.last_sync ?? 0;
  try {
    await syncApi.now();
  } catch {
    return "failed";
  }
  let busy = false;
  for (const end = Date.now() + 60_000; Date.now() < end; ) {
    await sleep(400);
    const s = await syncApi.status().catch(() => null);
    if (!s) continue;
    if ((s.last_sync ?? 0) > before) return "done";
    if (s.state === "syncing") busy = true;
    else if (STUCK.includes(s.state) || (busy && s.state === "idle")) return "failed";
  }
  return "failed";
}

// ---------------------------------------------------------------- B2-u0 (가져오기 화면 안, 파일 카드 아래)

export function UpdateChoice({ path, info, onAdd }: { path: string; info: AnkiPreview; onAdd: () => void }) {
  const nav = useNav();
  const first = info.update!;
  const [mode, setMode] = useState<"update" | "add">(first.suggested ? "update" : "add");
  const [pull, setPull] = useState<Pull | null>(null);
  const [summary, setSummary] = useState<UpdateSummary | null>(null);
  const [error, setError] = useState<string>();
  const check = useCallback(async () => {
    setSummary(null);
    setError(undefined);
    setPull(null);
    setPull(await pullFirst(() => setPull("pulling")));
    try {
      setSummary(await api.inspectUpdate(path));
    } catch (e) {
      setError(errorText(e));
    }
  }, [path]);
  useEffect(() => {
    check();
  }, [check]);

  // 다른 기기 변경을 받은 뒤의 근거 (사전 검사가 받은 뒤에 센다, DU-09). 그 전에는 파일을 열 때의 근거
  const hint = summary?.hint ?? first;
  const deck = hint.known_decks[0];
  const half = hint.existing_notes * 2 >= hint.notes;
  const adds = hint.notes - hint.existing_notes;
  const ready = !!summary && pull !== "pulling" && pull !== null;
  const counts = summary
    ? [
        summary.changed > 0 && `새 버전으로 바뀌는 노트 ${n(summary.changed)}개`,
        summary.merged > 0 && `합친 노트 ${n(summary.merged)}개`,
        summary.new_notes > 0 && `새 노트 ${n(summary.new_notes)}개`,
        summary.conflicts.length > 0 && `충돌 ${n(summary.conflicts.length)}개`,
      ].filter(Boolean)
    : [];
  const go = () => {
    if (mode === "add") return onAdd();
    if (summary) nav({ name: "deckUpdate", path, info, summary, pulled: pull === "done" });
  };
  useKey("Enter", go, mode === "add" || ready);

  return (
    <>
      <div className="du-banner">
        <Icon name="rotate" size={20} className="c-accent" />
        <span className="du-banner__text">
          <span className="t-body-m-strong">{half || deck ? "이미 가져온 덱의 새 버전 같아요" : "이미 가져온 노트가 있어요"}</span>
          <span className="t-body-s c-secondary">
            이 파일 노트 {n(hint.notes)}개 중 {n(hint.existing_notes)}개가 이미 있어요{deck ? ` · 덱 "${deck}"` : ""}
          </span>
        </span>
      </div>
      <div className="import__checks">
        <div className="import__check">
          <Icon name="check" size={18} className="c-success" />
          <span className="t-body-m">덱과 하위 덱 구조</span>
          <span className="t-body-s c-secondary">
            최상위 {n(info.decks_top)}개 · 하위 {n(info.decks_sub)}개 · 내 덱 이름 그대로
          </span>
        </div>
        <div className="import__check">
          <Icon name="check" size={18} className="c-success" />
          <span className="t-body-m">노트 유형</span>
          <span className="t-body-s c-secondary">{n(info.note_types)}종 · 지난번에 받은 모양과 비교해요</span>
        </div>
        {pull && pull !== "off" && (
          <div className="import__check">
            {pull === "pulling" ? (
              <span className="import__step-icon c-accent">
                <Spinner size={18} />
              </span>
            ) : (
              <Icon name={pull === "done" ? "check" : "alert"} size={18} className={pull === "done" ? "c-success" : "c-warn"} />
            )}
            <span className="t-body-m">다른 기기 변경 먼저 받기</span>
            <span className="du-check__right">
              <span className="t-body-s c-secondary">
                {pull === "pulling" ? "받는 중…" : pull === "done" ? "방금 받았어요 · 판정에 반영해요" : "다른 기기 변경을 받지 못했어요 · 이 기기 기준으로 비교해요"}
              </span>
              {pull === "failed" && <Button variant="ghost" size="M" label="다시 받기" onClick={check} />}
            </span>
          </div>
        )}
        <div className="import__check">
          <Icon name="check" size={18} className="c-success" />
          <span className="t-body-m">미디어</span>
          <span className="t-body-s c-secondary">
            {n(info.media)}개{summary && summary.media_compared > 0 ? ` · 이름이 같은 ${n(summary.media_compared)}개는 내용을 비교해요` : ""}
          </span>
        </div>
      </div>
      <div className="import__choice" role="radiogroup" aria-label="가져오는 방식">
        <p className="t-label-m">어떻게 가져올까요?</p>
        <label className={`import__option ${mode === "update" ? "is-on" : ""}`}>
          <input type="radio" name="du-mode" checked={mode === "update"} onChange={() => setMode("update")} />
          <span className="import__radio" />
          <span className="import__option-text">
            <span className="import__option-title">
              <span className="t-body-m-strong">업데이트로 가져오기</span>
              {hint.suggested && <Badge tone="accent">권장</Badge>}
            </span>
            <span className="t-body-s c-secondary">
              만든 사람이 고친 노트는 새 버전으로 바꾸고, 내가 고친 필드와 복습 기록 · 일정은 지켜요. 적용하기 전에 바뀌는 것을 모두 보여 드려요.
            </span>
            <span className="du-chip">
              <Icon name="list" size={14} className="c-secondary" />
              <span className={`t-caption ${error ? "c-danger" : "c-secondary"}`}>
                {error ?? (!summary ? "바뀌는 것을 세는 중…" : counts.length ? `${counts.join(" · ")} (요약에서 확인)` : "새 버전에서 바뀐 노트가 없어요")}
              </span>
            </span>
          </span>
        </label>
        <label className={`import__option ${mode === "add" ? "is-on" : ""}`}>
          <input type="radio" name="du-mode" checked={mode === "add"} onChange={() => setMode("add")} />
          <span className="import__radio" />
          <span className="import__option-text">
            <span className="import__option-title">
              <span className="t-body-m-strong">새 노트만 더하기</span>
              {!hint.suggested && <Badge tone="accent">권장</Badge>}
            </span>
            <span className="t-body-s c-secondary">
              이미 있는 노트 {n(hint.existing_notes)}개는 그대로 두고 새 노트 {n(adds)}개만 넣어요. 만든 사람이 고친 내용은 들어오지 않아요.
            </span>
          </span>
        </label>
      </div>
      <div className="import__foot">
        <span className="t-body-s c-tertiary">원본 파일은 읽기만 하고 수정하지 않아요</span>
        <Button
          icon="arrow-right"
          label={mode === "update" ? "바뀌는 것 보기" : "새 노트 고르기"}
          onClick={go}
          disabled={mode === "update" && !ready}
        />
      </div>
    </>
  );
}

// ---------------------------------------------------------------- 고르기 (B2-k · B2-kt)

type Pick = "mine" | "new";
const lookKey = (t: number, ord: number | null) => `${t}:${ord ?? "css"}`;

/** 화면의 고르기 → 엔진 choices. 카드 종류를 새 모양으로 고르면 그 유형의 CSS 충돌도 새 모양 (B2-kt "앞면 · 뒷면 · CSS를 모두"), CSS 만 따로 내 것으로 고르면 지킨다 */
function toChoices(picks: Map<number, Pick>, looks: Map<string, Pick>, s: UpdateSummary): UpdateChoices {
  const take_new = [...picks].filter(([, v]) => v === "new").map(([k]) => k);
  const new_looks: [number, number | null][] = [];
  for (const t of s.types) {
    const anyNew = t.looks.some((l) => l.ord !== null && looks.get(lookKey(t.note_type_id, l.ord)) === "new");
    for (const l of t.looks) {
      const v = looks.get(lookKey(t.note_type_id, l.ord));
      if (v === "new" || (l.ord === null && anyNew && v !== "mine")) new_looks.push([t.note_type_id, l.ord]);
    }
  }
  return { take_new, new_looks };
}

type Step = "summary" | "conflicts" | "looks" | "apply";

export function DeckUpdate({ path, info, summary: first }: { path: string; info: AnkiPreview; summary: UpdateSummary; pulled: boolean }) {
  const nav = useNav();
  const [summary, setSummary] = useState(first);
  const [step, setStep] = useState<Step>("summary");
  const [picks, setPicks] = useState<Map<number, Pick>>(new Map());
  const [looks, setLooks] = useState<Map<string, Pick>>(new Map());
  const [reviewed, setReviewed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const choices = useMemo(() => toChoices(picks, looks, summary), [picks, looks, summary]);

  // 고른 것을 반영해 요약을 다시 센다 (사전 검사 = 적용 결과, DU20)
  const back = async (c: UpdateChoices = choices) => {
    setStep("summary");
    setBusy(true);
    try {
      setSummary(await api.inspectUpdate(path, c));
      setError(undefined);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const allNewLooks = () => {
    const next = new Map(looks);
    summary.types.forEach((t) => t.looks.forEach((l) => next.set(lookKey(t.note_type_id, l.ord), "new")));
    setLooks(next);
    back(toChoices(picks, next, summary));
  };

  if (step === "conflicts")
    return (
      <ConflictReview
        summary={summary}
        picks={picks}
        onPick={setPicks}
        onBack={() => {
          setReviewed(true);
          back();
        }}
      />
    );
  if (step === "looks") return <LookCompare summary={summary} looks={looks} onPick={setLooks} onBack={() => back()} />;
  if (step === "apply") return <ApplyProgress path={path} info={info} summary={summary} choices={choices} onBack={() => setStep("summary")} />;
  return (
    <UpdateSummaryView
      s={summary}
      reviewed={reviewed}
      busy={busy}
      error={error}
      onMode={() => nav({ name: "import", path })}
      onReview={() => setStep("conflicts")}
      onLooks={() => setStep("looks")}
      onAllNewLooks={allNewLooks}
      onApply={() => setStep("apply")}
    />
  );
}

// ---------------------------------------------------------------- B2-u 업데이트 요약

type Row = { key: string; icon: IconName; title: string; desc: string; right?: string; warn?: boolean; actions?: { label: string; onClick: () => void; ink?: boolean }[] };

/** 노트 유형 줄의 설명 한 문장: 새 필드 · 이름 바뀐 필드 · 새 카드 종류 · 새 모양 · 없어진 것 ([이어 쓰는 꼴, 끝내는 꼴]) */
function typeDesc(t: UpdateTypeChange) {
  const q = (xs: string[]) => xs.map((x) => `"${x}"`).join(", ");
  // 카드 종류 이름은 따옴표, CSS 는 그대로
  const looks = (xs: string[]) => xs.map((x) => (x === "CSS" ? x : `"${x}"`)).join(", ");
  const gone = [...t.removed_fields, ...t.removed_templates];
  const skipped = [t.skipped_fields.length > 0 && `필드 ${q(t.skipped_fields)}`, t.skipped_templates.length > 0 && `카드 종류 ${q(t.skipped_templates)}`].filter(Boolean) as string[];
  const clauses = [
    t.created && ["새 노트 유형으로 넣고", "새 노트 유형으로 넣어요"],
    t.new_fields.length > 0 && [`${q(t.new_fields)} 필드를 끝에 더하고`, `${q(t.new_fields)} 필드를 끝에 더해요`],
    ...t.renamed_fields.map(([a, b]) => [`필드 "${a}" 이름을 "${b}"${ro(b)} 바꾸고`, `필드 "${a}" 이름을 "${b}"${ro(b)} 바꿔요`]),
    t.new_templates.length > 0 && [
      `새 카드 종류 ${q(t.new_templates)}${ro(t.new_templates.slice(-1)[0])} 신규 카드를 만들고`,
      `새 카드 종류 ${q(t.new_templates)}${ro(t.new_templates.slice(-1)[0])} 신규 카드를 만들어요`,
    ],
    t.updated_looks.length > 0 && [`${looks(t.updated_looks)}${eun(t.updated_looks.slice(-1)[0])} 새 모양으로 바꾸고`, `${looks(t.updated_looks)}${eun(t.updated_looks.slice(-1)[0])} 새 모양으로 바꿔요`],
    gone.length > 0 && [`새 버전에서 없어진 ${q(gone)}${eun(gone.slice(-1)[0])} 그대로 두고`, `새 버전에서 없어진 ${q(gone)}${eun(gone.slice(-1)[0])} 그대로 둬요`],
    skipped.length > 0 && [
      `비교할 원본이 없는 예전 유형이라 새 버전의 ${skipped.join(" · ")}${eun(skipped.slice(-1)[0])} 넣지 않고 (내가 지운 것과 구별할 수 없어요)`,
      `비교할 원본이 없는 예전 유형이라 새 버전의 ${skipped.join(" · ")}${eun(skipped.slice(-1)[0])} 넣지 않아요 (내가 지운 것과 구별할 수 없어요)`,
    ],
  ].filter(Boolean) as [string, string][];
  const sentence = clauses.map((c, i) => c[i === clauses.length - 1 ? 1 : 0]).join(", ");
  return `"${t.name}"${sentence ? `: ${sentence}` : ""}`;
}

function UpdateSummaryView({ s, reviewed, busy, error, onMode, onReview, onLooks, onAllNewLooks, onApply }: {
  s: UpdateSummary;
  reviewed: boolean;
  busy: boolean;
  error?: string;
  onMode: () => void;
  onReview: () => void;
  onLooks: () => void;
  onAllNewLooks: () => void;
  onApply: () => void;
}) {
  const nav = useNav();
  useKey("Escape", onMode);
  const conflicts = s.conflicts.length;
  const takeNew = s.conflicts.filter((c) => c.take_new).length;
  const typeRows = s.types.filter(
    (t) =>
      t.created ||
      t.new_fields.length ||
      t.renamed_fields.length ||
      t.new_templates.length ||
      t.updated_looks.length ||
      t.removed_fields.length ||
      t.removed_templates.length ||
      t.skipped_fields.length ||
      t.skipped_templates.length,
  );
  const newFields = typeRows.reduce((a, t) => a + t.new_fields.length, 0);
  const newTpls = typeRows.reduce((a, t) => a + t.new_templates.length, 0);
  const tplCards = s.types.reduce((a, t) => a + t.new_template_cards, 0);
  // 모양 충돌: 지키는 것(고르지 않음 · 내 모양)과 새 모양으로 고른 것 (DU-03). 원본 없는 유형은 내가 고친 것이 아니다 (DU-08)
  const looks = s.types.flatMap((t) => t.looks.map((l) => ({ t, l, label: `"${t.name}" ${l.name}` })));
  const kept = looks.filter((x) => !x.l.take_new);
  const chosen = looks.filter((x) => x.l.take_new);
  const editedKept = kept.filter((x) => !x.t.no_base);
  const baseless = kept.filter((x) => x.t.no_base);
  const sides = [...new Set(editedKept.flatMap((x) => x.l.mine_changed))];
  const keptDesc = [
    editedKept.length > 0 && `${editedKept.map((x) => x.label).join(", ")}의 ${sides.join(" · ") || "모양"}${eul(sides.join(" · ") || "모양")} 나도 고쳐서 내 모양을 지켜요`,
    baseless.length > 0 && `${baseless.map((x) => x.label).join(", ")}${eun(baseless.slice(-1)[0].label)} 비교할 원본이 없어 새 버전과 다른 모양을 내 것으로 지켜요`,
  ]
    .filter(Boolean)
    .join(" · ");
  // 빈칸 · 새 카드 종류 밖에서 생기거나 비는 카드 (필드가 새로 차거나 비었다)
  const otherNew = s.new_cards - s.cloze.new_cards - tplCards;
  const otherEmpty = s.emptied_cards - s.cloze.emptied_cards;
  const noBase = s.conflicts.filter((c) => c.no_base).length;
  const nothing = s.changed + s.merged + conflicts + s.new_notes + s.new_cards + typeRows.length + s.media_new + chosen.length === 0;

  const all: Row[] = [
    { key: "changed", icon: "rotate", title: "새 버전으로 바뀌는 노트", desc: "만든 사람만 고친 노트예요. 카드 기록과 일정은 그대로예요", right: `${n(s.changed)}개` },
    { key: "merged", icon: "layers", title: "합친 노트", desc: "한 노트 안에서 만든 사람이 고친 필드는 새 버전, 내가 고친 필드는 내 것으로 합쳐요", right: `${n(s.merged)}개` },
    {
      key: "conflicts",
      icon: "alert",
      title: "충돌 · 같은 필드를 둘 다 고친 노트",
      desc: "검토하지 않으면 내 것을 지켜요. 고른 것은 충돌한 필드에만 적용되고, 같은 노트의 다른 변경은 합쳐요",
      right: reviewed ? `${n(conflicts)}개 · 내 것 ${n(conflicts - takeNew)} · 새 버전 ${n(takeNew)}${ro(String(takeNew))} 골랐어요` : `${n(conflicts)}개`,
      warn: !reviewed,
      actions: [{ label: reviewed ? "다시 검토" : "검토", onClick: onReview, ink: !reviewed }],
    },
    {
      key: "new",
      icon: "plus",
      title: "새 노트",
      desc: `새 카드는 신규로 시작해요 · 하위 덱 자리는 새 버전을 따라요${s.new_decks > 0 ? ` · 이 기기에 없는 덱 ${n(s.new_decks)}개를 새로 만들어요` : ""}`,
      right: `${n(s.new_notes)}개 · 카드 ${n(s.new_note_cards)}장`,
    },
    ...(chosen.length > 0
      ? [{
          key: "newlooks",
          icon: "layers" as IconName,
          title: "새 모양으로 바꾸는 카드 종류",
          desc: `${chosen.map((x) => x.label).join(", ")}${eun(chosen.slice(-1)[0].label)} 고른 대로 새 모양으로 바꿔요`,
          right: `${n(chosen.length)}개`,
          actions: [{ label: "새 모양 보기", onClick: onLooks }],
        }]
      : []),
    ...(typeRows.length
      ? [{
          key: "types",
          icon: "file" as IconName,
          title: ["노트 유형", newFields > 0 && `새 필드 ${n(newFields)}개`, newTpls > 0 && `새 카드 종류 ${n(newTpls)}개`].filter(Boolean).join(" · "),
          desc: typeRows.map(typeDesc).join(" / "),
          right: tplCards > 0 ? `새 카드 ${n(tplCards)}장` : undefined,
        }]
      : []),
    ...(s.cloze.renumbered_notes + s.cloze.new_cards + s.cloze.emptied_cards > 0
      ? [{
          key: "cloze",
          icon: "list" as IconName,
          title: s.cloze.renumbered_notes > 0 ? "빈칸 번호가 바뀐 것 같은 노트" : "빈칸 번호가 바뀌는 노트",
          desc: `카드는 번호를 따라가요. 새 번호 카드 ${n(s.cloze.new_cards)}장은 신규로 시작하고, 없어진 번호 카드 ${n(s.cloze.emptied_cards)}장은 학습에서 빠져요 (기록 · 일정 보존)`,
          right: s.cloze.renumbered_notes > 0 ? `${n(s.cloze.renumbered_notes)}개` : undefined,
        }]
      : []),
    ...(otherNew + otherEmpty > 0
      ? [{
          key: "fronts",
          icon: "layers" as IconName,
          title: "앞면이 바뀌는 카드",
          desc: `필드가 새로 차서 생기는 카드 ${n(otherNew)}장은 신규로 시작하고, 앞면이 비는 카드 ${n(otherEmpty)}장은 학습에서 빠져요 (기록 · 일정 보존)`,
        }]
      : []),
    ...(s.media_new > 0
      ? [{
          key: "media",
          icon: "image" as IconName,
          title: "새 미디어 파일",
          desc: s.media_renamed > 0 ? `이름은 같은데 내용이 다른 ${n(s.media_renamed)}개는 기존 파일을 두고 새 이름으로 저장해요` : "이 파일의 그림 · 소리를 저장해요",
          right: `${n(s.media_new)}개`,
        }]
      : []),
  ];
  const changes = all.filter((r) => !["changed", "merged", "conflicts", "new"].includes(r.key) || { changed: s.changed, merged: s.merged, conflicts, new: s.new_notes }[r.key as "changed"] > 0);

  const stays: Row[] = [
    ...(s.mine_kept > 0 ? [{ key: "mine", icon: "pencil" as IconName, title: "내가 고쳐 지키는 노트", desc: "내가 고친 필드만 있고 만든 사람은 고치지 않았어요", right: `${n(s.mine_kept)}개` }] : []),
    ...(s.missing_notes > 0
      ? [{
          key: "missing",
          icon: "info" as IconName,
          title: "새 버전에 없는 노트",
          desc: "지우지 않아요. 카드 · 기록 · 일정 그대로 학습에도 나와요",
          right: `${n(s.missing_notes)}개`,
          actions: [{ label: "카드 목록에서 보기", onClick: () => nav({ name: "cards", deckId: null, cardIds: s.missing_card_ids }) }],
        }]
      : []),
    ...(s.gone > 0 ? [{ key: "gone", icon: "trash" as IconName, title: "내가 지운 노트", desc: "다시 넣지 않아요", right: `${n(s.gone)}개` }] : []),
    ...(kept.length > 0
      ? [{
          key: "looks",
          icon: "lock" as IconName,
          title: "내 모양을 지킨 노트 유형",
          desc: keptDesc,
          right: `${n(kept.length)}개`,
          actions: [
            { label: "새 모양 보기", onClick: onLooks },
            { label: "새 모양으로", onClick: onAllNewLooks },
          ],
        }]
      : []),
    ...(s.duplicate_notes > 0
      ? [{
          key: "dups",
          icon: "alert" as IconName,
          title: "같은 노트가 두 벌 있어요",
          desc: "다른 기기 변경을 받기 전에 두 기기에서 같은 파일을 적용했을 수 있어요. 첫 번째 것만 고쳐요 · 하나를 지워 주세요",
          right: `${n(s.duplicate_notes)}개`,
          actions: [{ label: "카드 목록에서 보기", onClick: () => nav({ name: "cards", deckId: null, cardIds: s.duplicate_card_ids }) }],
        }]
      : []),
    ...(noBase > 0
      ? [{ key: "nobase", icon: "info" as IconName, title: "비교할 원본이 없는 노트", desc: "예전에 가져온 노트예요. 가져온 뒤 고치지 않았으면 새 버전으로, 고쳤으면 충돌로 보고 내 것을 지켜요", right: `${n(noBase)}개` }]
      : []),
    ...(s.other_type > 0
      ? [{ key: "other", icon: "alert" as IconName, title: "노트 유형이 달라 고치지 못한 노트", desc: "만든 사람이 노트 유형을 바꿨어요. 이 노트는 그대로 둬요", right: `${n(s.other_type)}개` }]
      : []),
  ];

  return (
    <main className="import">
      <button className="backlink t-label-m c-secondary" onClick={onMode}>
        <Icon name="chevron-left" size={18} />
        가져오기
      </button>
      <div className="import__head">
        <h1 className="t-title-l">업데이트 요약</h1>
      </div>
      <div className="import__file">
        <span className="import__file-icon">
          <Icon name="file" size={22} className="c-accent" />
        </span>
        <span className="import__file-text">
          <span className="t-body-m-strong">{s.file_name}</span>
          <span className="t-body-s c-secondary">{deckOf(s) ? `덱 "${deckOf(s)}" · 지난번에 받은 버전 → 이 파일` : "지난번에 받은 버전 → 이 파일"}</span>
        </span>
        <Button variant="ghost" size="M" label="방식 바꾸기" onClick={onMode} />
      </div>
      <p className="t-body-m c-secondary">적용하기 전에 바뀌는 것을 모두 보여 드려요. 내 것은 지금 내 덱, 새 버전은 이 파일이에요.</p>
      <RowGroup accent title="바뀌는 것" caption="적용하면 이렇게 돼요" rows={changes} empty={s.changed + s.merged + conflicts === 0 ? "새 버전에서 바뀐 노트가 없어요" : undefined} />
      {stays.length > 0 && <RowGroup title="그대로 두는 것" rows={stays} />}
      {s.issues.length > 0 && (
        <div className="du-group">
          <div className="du-group__head">
            <span className="t-label-m">참고할 항목</span>
          </div>
          <div className="du-row">
            <IssueList issues={s.issues} />
          </div>
        </div>
      )}
      <div className="du-fixed">
        <span className="du-line">
          <Icon name="check" size={16} className="c-success" />
          <span className="t-label-m">복습 기록과 일정은 그대로예요 · 중요도 · 덱 구조 · 목표일도 내 것 그대로</span>
        </span>
        <span className="du-line">
          <Icon name="check" size={16} className="c-success" />
          <span className="t-label-m">적용하기 전에 백업을 만들어요 · 문제가 생기면 설정 › 이 기기 백업에서 [백업으로 복구]</span>
        </span>
        {s.reviews_skipped > 0 && (
          <span className="du-line">
            <Icon name="info" size={16} className="c-secondary" />
            <span className="t-body-s c-secondary">가져오지 않는 복습 기록 {n(s.reviews_skipped)}건 · 이 파일의 복습 기록은 만든 사람 것이에요</span>
          </span>
        )}
      </div>
      <div className="import__foot">
        <span className={`t-caption ${error ? "c-danger" : "c-secondary"}`}>
          {error ??
            (busy
              ? "고른 것으로 다시 세는 중…"
              : conflicts === 0
                ? "충돌 없음 · 바로 적용할 수 있어요"
                : reviewed
                  ? `충돌 ${n(conflicts)}개 · 내 것 ${n(conflicts - takeNew)} · 새 버전 ${n(takeNew)}`
                  : `충돌 ${n(conflicts)}개 · 검토 전이라 내 것을 지켜요`)}
        </span>
        <span className="import__foot-right">
          <Button variant="ghost" size="M" label="취소" onClick={() => nav({ name: "import" })} />
          {nothing ? (
            <Button label="닫기" onClick={() => nav({ name: "home" })} />
          ) : (
            <Button label="업데이트 적용" onClick={onApply} disabled={busy || !!error} />
          )}
        </span>
      </div>
    </main>
  );
}

function RowGroup({ title, caption, rows, empty, accent }: { title: string; caption?: string; rows: Row[]; empty?: string; accent?: boolean }) {
  return (
    <div className="du-group">
      <div className="du-group__head">
        <span className="t-label-m">{title}</span>
        {caption && <span className="t-caption c-tertiary">{caption}</span>}
      </div>
      {empty && (
        <div className="du-row">
          <Icon name="info" size={16} className="c-secondary" />
          <span className="du-row__text">
            <span className="t-body-m">{empty}</span>
          </span>
        </div>
      )}
      {rows.map((r) => (
        <div key={r.key} className={`du-row ${r.warn ? "du-row--warn" : ""}`}>
          <Icon name={r.icon} size={16} className={r.warn ? "c-warn" : accent ? "c-accent" : "c-secondary"} />
          <span className="du-row__text">
            <span className="t-body-m">{r.title}</span>
            <span className="t-caption c-secondary">{r.desc}</span>
          </span>
          <span className="du-row__right">
            {r.right && <span className="t-label-m">{r.right}</span>}
            {r.actions?.map((a) => <Button key={a.label} variant={a.ink ? "secondary" : "ghost"} size="M" label={a.label} onClick={a.onClick} />)}
          </span>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- B2-k 충돌 검토

/** 빈칸 표시를 학습 화면처럼 칩으로 (번호를 붙인다) */
const showCloze = (html: string) =>
  html.replace(/\{\{c(\d+)::(.*?)(?:::(.*?))?\}\}/gs, (_, num, ans) => `<span class="du-cloze"><span class="du-cloze__n">c${num}</span>${ans}</span>`);

/** 두 HTML 의 다른 낱말에 밑줄 (글자 노드의 낱말 LCS). 태그 · 그림의 차이는 보지 않는다.
 * ponytail: 낱말이 아주 많으면(곱이 25만 넘게) 밑줄 없이 보인다 */
function underlineDiff(a: string, b: string): [string, string] {
  const parse = (h: string) => Object.assign(document.createElement("template"), { innerHTML: h });
  const [ta, tb] = [parse(a), parse(b)];
  type Tok = { node: Text; start: number; end: number; text: string };
  const tokens = (t: HTMLTemplateElement) => {
    const out: Tok[] = [];
    const walk = document.createTreeWalker(t.content, NodeFilter.SHOW_TEXT);
    for (let node = walk.nextNode() as Text | null; node; node = walk.nextNode() as Text | null) {
      for (const m of node.data.matchAll(/\S+/g)) out.push({ node, start: m.index!, end: m.index! + m[0].length, text: m[0] });
    }
    return out;
  };
  const [wa, wb] = [tokens(ta), tokens(tb)];
  if (wa.length * wb.length > 250_000) return [a, b];
  const w = wb.length + 1;
  const dp = new Uint32Array((wa.length + 1) * w);
  for (let i = wa.length - 1; i >= 0; i--)
    for (let j = wb.length - 1; j >= 0; j--)
      dp[i * w + j] = wa[i].text === wb[j].text ? dp[(i + 1) * w + j + 1] + 1 : Math.max(dp[(i + 1) * w + j], dp[i * w + j + 1]);
  const [ka, kb] = [new Set<number>(), new Set<number>()];
  for (let i = 0, j = 0; i < wa.length && j < wb.length; ) {
    if (wa[i].text === wb[j].text) (ka.add(i++), kb.add(j++));
    else if (dp[(i + 1) * w + j] >= dp[i * w + j + 1]) i++;
    else j++;
  }
  const mark = (toks: Tok[], keep: Set<number>, t: HTMLTemplateElement) => {
    const byNode = new Map<Text, Tok[]>();
    toks.forEach((tok, i) => !keep.has(i) && byNode.set(tok.node, [...(byNode.get(tok.node) ?? []), tok]));
    for (const [node, changed] of byNode) {
      const frag = document.createDocumentFragment();
      let at = 0;
      for (const tok of changed) {
        frag.append(node.data.slice(at, tok.start), Object.assign(document.createElement("u"), { className: "du-diff", textContent: tok.text }));
        at = tok.end;
      }
      frag.append(node.data.slice(at));
      node.replaceWith(frag);
    }
    return t.innerHTML;
  };
  return [mark(wa, ka, ta), mark(wb, kb, tb)];
}

/** 그림 이름 (src) */
const srcs = (html: string) => [...html.matchAll(/src=["']?([^"' >]+)/g)].map((m) => m[1]);

function FieldCompare({ name, mine, next, pick, edited, preview }: {
  name: string;
  mine: string;
  next: string;
  pick: Pick | undefined;
  edited: number;
  preview: Record<string, string>;
}) {
  const [html, setHtml] = useState<[string, string] | null>(null);
  const [full, setFull] = useState(false);
  useEffect(() => {
    let live = true;
    // 적용 전이라 아직 저장하지 않은 새 그림은 사전 검사가 준 data URL 로 (I85 새 이름)
    const withPreview = next.replace(/src=(["']?)([^"' >]+)/g, (m, q, src) => (preview[src] ? `src=${q}${preview[src]}` : m));
    Promise.all([renderCardHtml(showCloze(mine)), renderCardHtml(showCloze(withPreview))]).then(([a, b]) => live && setHtml(underlineDiff(a, b)));
    return () => {
      live = false;
    };
  }, [mine, next, preview]);
  // 같은 이름에 내용만 다른 그림 (새 버전은 이름-<sha8> 로 저장한다, DU10)
  const mineSrcs = srcs(mine);
  const sameName = srcs(next).some((x) => mineSrcs.includes(x.replace(/-[0-9a-f]{8}(\.[^.]+)?$/, "$1")) && !mineSrcs.includes(x));
  const side = (who: Pick) => {
    const chosen = pick === who;
    const fallback = !pick && who === "mine";
    return (
      <div className={`du-side ${chosen ? "is-chosen" : fallback ? "is-default" : ""} ${full ? "is-full" : ""}`}>
        <span className="du-side__head">
          <Kbd k={who === "mine" ? "1" : "2"} />
          <span className="t-label-m">{who === "mine" ? "내 것" : "새 버전"}</span>
          <span className="t-caption c-tertiary">{who === "mine" ? `${day(edited)}에 고침` : "이 파일"}</span>
          {fallback && <Badge>고르지 않으면 이것</Badge>}
          {who === "new" && sameName && <span className="t-caption c-warn">이름은 같고 그림이 달라요</span>}
        </span>
        <div className="du-side__body t-body-l card-html" dangerouslySetInnerHTML={{ __html: html?.[who === "mine" ? 0 : 1] ?? "" }} />
      </div>
    );
  };
  const long = Math.max(mine.length, next.length) > 600;
  return (
    <>
      <span className="du-field">
        <span className="t-label-m">{name}</span>
        <span className="t-caption c-tertiary">다른 부분에 밑줄을 그어요</span>
        {long && <Button variant="ghost" size="M" label={full ? "접기" : "전체 보기"} onClick={() => setFull(!full)} />}
      </span>
      <div className="du-compare">
        {side("mine")}
        {side("new")}
      </div>
    </>
  );
}

function ConflictReview({ summary, picks, onPick, onBack }: {
  summary: UpdateSummary;
  picks: Map<number, Pick>;
  onPick: (m: Map<number, Pick>) => void;
  onBack: () => void;
}) {
  const list = summary.conflicts;
  const [cursor, setCursor] = useState(0);
  const [open, setOpen] = useState<number | null>(0);
  const rows = useRef<(HTMLDivElement | null)[]>([]);
  useEffect(() => {
    rows.current[cursor]?.scrollIntoView({ block: "nearest" });
  }, [cursor]);
  const choose = (i: number, v: Pick) => {
    onPick(new Map(picks).set(list[i].note_id, v));
    // 고르면 다음 노트가 펼쳐진다
    if (i + 1 < list.length) (setCursor(i + 1), setOpen(i + 1));
  };
  const all = (v: Pick) => onPick(new Map(list.map((c) => [c.note_id, v])));
  useKey("Escape", onBack);
  useKey("ArrowDown", () => setCursor((c) => Math.min(list.length - 1, c + 1)));
  useKey("ArrowUp", () => setCursor((c) => Math.max(0, c - 1)));
  useKey("Enter", () => setOpen((o) => (o === cursor ? null : cursor)));
  useKey(["1", "2"], (_, i) => choose(cursor, i === 0 ? "mine" : "new"));
  const picked = list.filter((c) => picks.has(c.note_id)).length;

  return (
    <main className="import du-wide">
      <button className="backlink t-label-m c-secondary" onClick={onBack}>
        <Icon name="chevron-left" size={18} />
        업데이트 요약
      </button>
      <div className="import__head">
        <h1 className="t-title-l">충돌 검토</h1>
      </div>
      <p className="t-body-m c-secondary">
        나와 만든 사람이 같은 필드를 둘 다 고친 노트 {n(list.length)}개예요. 고르지 않으면 내 것을 지켜요. 고른 것은 충돌한 필드에만 적용돼요.
      </p>
      <div className="du-toolbar">
        <span className="t-label-m">
          {n(picked)} / {n(list.length)}개 골랐어요
        </span>
        <span>
          <Button variant="ghost" size="M" label="모두 내 것" onClick={() => all("mine")} />
          <Button variant="ghost" size="M" label="모두 새 버전" onClick={() => all("new")} />
        </span>
      </div>
      <div className="du-list">
        {list.map((c, i) => (
          <ConflictRow
            key={c.note_id}
            ref={(el) => {
              rows.current[i] = el;
            }}
            c={c}
            pick={picks.get(c.note_id)}
            cursor={cursor === i}
            open={open === i}
            onToggle={() => (setCursor(i), setOpen(open === i ? null : i))}
            onPick={(v) => choose(i, v)}
            preview={summary.media_preview}
          />
        ))}
      </div>
      <span className="du-keys">
        <Kbd k="↑↓" />
        <span className="t-caption c-secondary">노트 이동</span>
        <Kbd k="Enter" />
        <span className="t-caption c-secondary">펼치기</span>
        <Kbd k="1" />
        <span className="t-caption c-secondary">내 것</span>
        <Kbd k="2" />
        <span className="t-caption c-secondary">새 버전</span>
      </span>
      <div className="import__foot">
        <span className="t-caption c-secondary">{picked < list.length ? `고르지 않은 ${n(list.length - picked)}개는 내 것을 지켜요` : "모두 골랐어요"}</span>
        <Button label="요약으로 돌아가기" onClick={onBack} />
      </div>
    </main>
  );
}

function ConflictRow({ c, pick, cursor, open, onToggle, onPick, ref, preview }: {
  c: UpdateConflict;
  preview: Record<string, string>;
  pick: Pick | undefined;
  cursor: boolean;
  open: boolean;
  onToggle: () => void;
  onPick: (v: Pick) => void;
  ref: (el: HTMLDivElement | null) => void;
}) {
  const others = c.auto_new_fields;
  return (
    <div ref={ref} className={`du-item ${cursor ? "is-cursor" : ""}`}>
      <button className="du-item__head" aria-expanded={open} onClick={onToggle}>
        <Icon name={open ? "chevron-down" : "chevron-right"} size={16} className="c-secondary" />
        <span className="du-row__text">
          <span className="t-body-m-strong du-ellipsis">{c.title.replace(/\{\{c\d+::.*?\}\}/g, "[...]") || "(첫 필드가 비어 있어요)"}</span>
          <span className="t-caption c-secondary du-ellipsis">
            {c.deck} · 충돌 필드: {c.fields.map((f) => f.name).join(", ")}
          </span>
        </span>
        {pick === "new" ? <Badge tone="accent">새 버전</Badge> : pick === "mine" ? <Badge>내 것</Badge> : <Badge tone="warn">고르지 않음 · 내 것</Badge>}
      </button>
      {open && (
        <>
          {c.fields.map((f) => (
            <FieldCompare key={f.slot} name={f.name} mine={f.mine} next={f.new} pick={pick} edited={c.edited_at} preview={preview} />
          ))}
          {(others.length > 0 || c.auto_mine > 0) && (
            <span className="du-other">
              <Icon name="chevron-right" size={14} className="c-secondary" />
              <span className="t-caption c-secondary">
                {[
                  others.length > 0 && `만든 사람이 고친 필드 ${n(others.length)}개(${others.join(", ")})는 새 버전으로 바뀌어요`,
                  c.auto_mine > 0 && `내가 고친 필드 ${n(c.auto_mine)}개는 내 것으로 남아요`,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </span>
          )}
          {c.no_base && <span className="t-caption c-tertiary">예전에 가져온 노트라 비교할 원본이 없어 충돌로 봤어요</span>}
          <span className="du-actions">
            <Button variant="secondary" size="M" label="내 것 유지" onClick={() => onPick("mine")} />
            <Button variant="secondary" size="M" label="새 버전으로" onClick={() => onPick("new")} />
          </span>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- B2-kt 카드 모양 비교

type LookItem = { t: UpdateTypeChange; l: LookConflict; key: string };

function LookCompare({ summary, looks, onPick, onBack }: { summary: UpdateSummary; looks: Map<string, Pick>; onPick: (m: Map<string, Pick>) => void; onBack: () => void }) {
  const items: LookItem[] = summary.types.flatMap((t) => t.looks.map((l) => ({ t, l, key: lookKey(t.note_type_id, l.ord) })));
  const [at, setAt] = useState(0);
  const [face, setFace] = useState<"front" | "back">("front");
  useKey("Escape", onBack);
  const item = items[Math.min(at, items.length - 1)];
  if (!item) return null;
  const pick = looks.get(item.key);
  const set = (v: Pick) => onPick(new Map(looks).set(item.key, v));
  const label = (x: LookItem) => `${x.t.name} · ${x.l.ord === null ? "CSS" : x.l.name}`;
  const mineSides = item.l.mine_changed.join(" · ");
  return (
    <main className="import du-wide">
      <button className="backlink t-label-m c-secondary" onClick={onBack}>
        <Icon name="chevron-left" size={18} />
        업데이트 요약
      </button>
      <div className="import__head">
        <h1 className="t-title-l">카드 모양 비교</h1>
      </div>
      <p className="t-body-m c-secondary">나와 만든 사람이 같은 카드 종류의 모양을 둘 다 고쳤어요. 고르지 않으면 내 모양을 지켜요. 노트 내용 · 복습 기록은 어느 쪽이든 그대로예요.</p>
      {items.length > 1 && (
        <div className="du-chips">
          {items.map((x, i) => {
            const v = looks.get(x.key);
            return (
              <button key={x.key} className={`du-chipbtn t-label-m ${i === at ? "is-on" : ""}`} onClick={() => setAt(i)}>
                <span className={`du-dot du-dot--${v ?? "none"}`} aria-hidden />
                {label(x)}
                <span className="t-caption c-tertiary">{v === "new" ? "새 모양" : v === "mine" ? "내 모양" : "고르지 않음"}</span>
              </button>
            );
          })}
        </div>
      )}
      <div className="du-typehead">
        <span className="du-line">
          <Icon name="file" size={16} className="c-secondary" />
          <span className="t-label-m">{label(item)}</span>
          <Badge>이 유형 노트 {n(item.t.notes)}개</Badge>
        </span>
        <Segment boxed label="미리볼 면" options={[{ value: "front", label: "앞면" }, { value: "back", label: "뒷면" }]} value={face} onChange={setFace} />
      </div>
      <div className="du-looks">
        <LookSide item={item} who="mine" face={face} pick={pick} />
        <LookSide item={item} who="new" face={face} pick={pick} />
      </div>
      <span className="du-actions">
        <Button variant="secondary" size="M" label="내 모양 유지" onClick={() => set("mine")} />
        <Button variant="secondary" size="M" label="새 모양으로" onClick={() => set("new")} />
        <span className="t-caption c-tertiary">
          {item.t.no_base ? "새 모양으로 바꾸면 지금 모양은 사라져요" : `새 모양으로 바꾸면 내가 고친 ${mineSides || "모양"}${eun(mineSides || "모양")} 사라져요`} · 적용 전 백업에서 되돌릴 수 있어요
        </span>
      </span>
      <div className="import__foot">
        <span className="t-caption c-secondary">{pick === "new" ? "새 모양으로 바꿔요" : pick === "mine" ? "내 모양을 지켜요" : "고르지 않았어요 · 내 모양을 지켜요"}</span>
        <Button label="요약으로 돌아가기" onClick={onBack} />
      </div>
    </main>
  );
}

/** 학습 화면과 같은 렌더 (템플릿 미리보기 · 살균 · 소리 칩 · CSS 가두기). 이 유형의 가장 최근 노트로 그린다 */
function LookSide({ item, who, face, pick }: { item: LookItem; who: Pick; face: "front" | "back"; pick: Pick | undefined }) {
  const { t, l } = item;
  const [html, setHtml] = useState<{ front: string; back: string } | null>(null);
  const [error, setError] = useState(false);
  const [raw, setRaw] = useState(false);
  const [tpl, setTpl] = useState<{ q: string; a: string; ord: number } | null>(null);
  // 새 모양은 적용 전이라 새 필드 · 바뀐 필드 이름이 아직 없다 (빈 칸이 점처럼 그려진다). 미리보기에서만 그 자리에 새 필드는 이름, 바뀐 이름은 옛 이름의 값
  const needFill = who === "new" && t.new_fields.length + t.renamed_fields.length > 0;
  const [fill, setFill] = useState<{ names: string[]; values: string[] } | null>(null);
  useEffect(() => {
    if (!needFill) return;
    Promise.all([api.noteTypeDetail(t.note_type_id), api.noteTypeSamples(t.note_type_id)]).then(
      ([d, samples]) => {
        const slots = samples[0]?.slots;
        const names = d.fields.map((f) => f.name);
        const values = d.fields.map((f) => slots?.[f.slot] ?? `(${f.name})`);
        for (const [a, b] of t.renamed_fields) {
          const i = names.indexOf(a);
          if (i >= 0 && !names.includes(b)) (names.push(b), values.push(values[i]));
        }
        for (const f of t.new_fields) if (!names.includes(f)) (names.push(f), values.push(f));
        setFill({ names, values });
      },
      () => setError(true),
    );
  }, [needFill, t]);
  // CSS 충돌이면 카드 종류는 바뀌지 않으니 첫 카드 종류로 그린다
  useEffect(() => {
    if (l.ord !== null) return setTpl(who === "mine" ? { q: l.mine_front, a: l.mine_back, ord: l.ord } : { q: l.new_front, a: l.new_back, ord: l.ord });
    api.noteTypeDetail(t.note_type_id).then(
      (d) => {
        const x = d.templates[0];
        setTpl(x ? { q: x.qfmt, a: x.afmt, ord: x.ord ?? 0 } : null);
      },
      () => setError(true),
    );
  }, [t.note_type_id, l, who]);
  useEffect(() => {
    if (!tpl || (needFill && !fill)) return;
    let live = true;
    setError(false);
    api
      .previewTemplate(t.note_type_id, tpl.q, tpl.a, tpl.ord, fill?.values ?? null, fill?.names)
      .then(async (p) => {
        const [front, back] = await Promise.all([renderCardHtml(p.front), renderCardHtml(p.back)]);
        if (live) setHtml({ front, back });
      })
      .catch(() => live && setError(true));
    return () => {
      live = false;
    };
  }, [t.note_type_id, tpl, needFill, fill]);
  const css = who === "mine" ? t.css_mine : t.css_new;
  const scope = `.du-look__content--${who}`;
  const scoped = useMemo(() => scopeCss(css, false, scope), [css, scope]);
  const fallback = !pick && who === "mine";
  const changed = who === "mine" ? l.mine_changed : l.new_changed;
  // 새 필드는 적용한 뒤에 생겨서 미리보기에서는 비어 보인다
  const later = who === "new" ? t.new_fields.filter((f) => `${l.new_front}${l.new_back}`.includes(`${f}}}`)) : [];
  const source = l.ord === null ? (who === "mine" ? l.mine_front : l.new_front) : face === "front" ? (who === "mine" ? l.mine_front : l.new_front) : who === "mine" ? l.mine_back : l.new_back;
  return (
    <div className={`du-look ${pick === who ? "is-chosen" : fallback ? "is-default" : ""}`}>
      <span className="du-side__head">
        <span className="t-label-m">{who === "mine" ? "내 모양" : "새 모양"}</span>
        <span className="t-caption c-tertiary">{who === "mine" ? (l.edited_at ? `${day(l.edited_at)}에 고침` : "이 기기") : "이 파일"}</span>
        {fallback && <Badge>고르지 않으면 이것</Badge>}
      </span>
      <div className="du-look__card study">
        {scoped && <style>{scoped}</style>}
        {error || raw ? (
          <div className="du-look__raw">
            {error && <span className="t-body-s c-secondary">미리 볼 수 없어요</span>}
            {(raw || error) && <pre className="t-mono-key">{source}</pre>}
          </div>
        ) : (
          <div className={`study__card-content du-look__content--${who}`}>
            {html && <div className={`${face === "front" ? "study__q" : "study__a"} card-html`} dangerouslySetInnerHTML={{ __html: face === "front" ? html.front : html.back }} />}
          </div>
        )}
      </div>
      {later.length > 0 && <span className="t-caption c-tertiary">새 필드 {later.map((f) => `"${f}"`).join(", ")}{eun(later.slice(-1)[0])} 적용한 뒤에 채워져요</span>}
      <span className="du-css">
        <span className="t-label-s c-tertiary">CSS</span>
        <span className="t-mono-key c-secondary du-ellipsis">{css.replace(/\s+/g, " ").trim() || "(비어 있어요)"}</span>
      </span>
      <span className="du-look__foot">
        <span className="t-caption c-secondary">
          {who === "mine" && t.no_base ? "비교할 원본이 없어요 · 새 버전과 달라서 내 모양으로 지켜요" : `${who === "mine" ? "내가 고친 것" : "만든 사람이 고친 것"}: ${changed.join(" · ") || "—"}`}
        </span>
        <Button variant="ghost" size="M" label={raw ? "미리보기" : "HTML 보기"} onClick={() => setRaw(!raw)} />
      </span>
    </div>
  );
}

// ---------------------------------------------------------------- B2-p 업데이트 적용 중

const STEPS = [
  { key: "backup", label: "백업 만들기" },
  { key: "types", label: "노트 유형" },
  { key: "notes", label: "노트" },
  { key: "cards", label: "새 카드" },
  { key: "media", label: "미디어" },
] as const;
const NO_BACKUP = "적용 전 백업을 만들지 못해";

function ApplyProgress({ path, info, summary: s, choices, onBack }: { path: string; info: AnkiPreview; summary: UpdateSummary; choices: UpdateChoices; onBack: () => void }) {
  const nav = useNav();
  const [prog, setProg] = useState<UpdateProgress | null>(null);
  const [backupAt, setBackupAt] = useState<number | null>(null);
  const [failed, setFailed] = useState<string>();
  const [cancelling, setCancelling] = useState(false);
  const [round, setRound] = useState(0);
  const started = useRef(Date.now());
  const running = !failed;
  // 진행 이벤트 (StrictMode 가 효과를 두 번 돌려도 구독은 하나)
  useEffect(() => {
    const un = listen<UpdateProgress>("update-progress", (e) => {
      setProg(e.payload);
      if (e.payload.step === "backup" && e.payload.done === e.payload.total) setBackupAt(Date.now());
    });
    return () => {
      un.then((f) => f());
      lockNav(false);
    };
  }, []);
  // 적용은 한 번만 (StrictMode 의 두 번째 효과가 업데이트를 또 적용하지 않게). 결과는 화면이 아직 있을 때만 받는다
  const ran = useRef(-1);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    if (ran.current === round) return;
    ran.current = round;
    started.current = Date.now();
    lockNav(true);
    api.applyUpdate(path, choices).then(
      (result) => {
        lockNav(false);
        if (mounted.current) nav({ name: "updateResult", summary: result, info });
      },
      (e) => {
        lockNav(false);
        if (mounted.current) (setFailed(errorText(e)), setCancelling(false));
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- round 가 [다시 시도]
  }, [round]);
  const cancel = () => {
    setCancelling(true);
    api.cancelImport().catch(() => {});
  };
  useKey("Escape", cancel, running && !cancelling);

  const at = !prog ? 0 : prog.step === "done" ? STEPS.length : STEPS.findIndex((x) => x.key === prog.step);
  const ended = !!prog && prog.step !== "done" && prog.done >= prog.total;
  const active = at + (ended ? 1 : 0);
  const frac = prog && !ended && prog.step !== "done" && prog.total > 0 ? prog.done / prog.total : 0;
  const ratio = Math.min(1, (active + frac) / STEPS.length);
  const elapsed = (Date.now() - started.current) / 1000;
  const left = ratio >= 0.05 && ratio < 1 ? Math.max(1, Math.ceil((elapsed * (1 - ratio)) / ratio)) : null;
  const fields = s.types.reduce((a, t) => a + t.new_fields.length, 0);
  const tpls = s.types.reduce((a, t) => a + t.new_templates.length, 0);
  const touched = s.changed + s.merged + s.conflicts.length;
  // 노트 단계의 수: 파일 줄과 같은 바뀌는 노트 + 새 노트. 엔진 이벤트는 파일의 노트를 모두 세므로(그대로 · 내가 지운 노트 포함) 비율만 쓴다
  const writes = touched + s.new_notes;
  const cards = s.new_cards + s.new_note_cards;
  const value = (key: (typeof STEPS)[number]["key"], state: "done" | "active" | "wait") => {
    const wait = state === "wait" ? " · 대기" : "";
    switch (key) {
      case "backup":
        return state === "done" ? `완료${backupAt ? ` · ${clock(backupAt)}` : ""}` : state === "active" ? "만드는 중…" : "대기";
      case "types":
        return `${fields + tpls > 0 ? `새 필드 ${n(fields)} · 새 카드 종류 ${n(tpls)}` : "바뀐 구조 없음"}${wait}`;
      case "notes":
        return state === "active" && prog?.step === "notes" ? `${n(Math.round((writes * prog.done) / Math.max(1, prog.total)))} / ${n(writes)}` : `${n(writes)}개${wait}`;
      case "cards":
        return `${n(cards)}장${wait}`;
      case "media":
        return `${[s.media_renamed > 0 && `바뀐 것 ${n(s.media_renamed)}개`, s.media_new - s.media_renamed > 0 && `새 것 ${n(s.media_new - s.media_renamed)}개`].filter(Boolean).join(" · ") || "새 파일 없음"}${wait}`;
    }
  };
  const noBackup = failed?.startsWith(NO_BACKUP);
  // [취소]로 멈춘 것은 오류가 아니다 (엔진 STOPPED 와 같은 글)
  const stopped = failed?.startsWith("업데이트를 멈췄어요");

  return (
    <main className="import">
      <button className="backlink t-label-m c-secondary" disabled={!failed} onClick={() => nav({ name: "home" })}>
        <Icon name="chevron-left" size={18} />홈
      </button>
      <div className="import__head">
        <h1 className="t-title-l">{stopped ? "업데이트를 멈췄어요" : noBackup ? "업데이트하지 않았어요" : failed ? "업데이트하지 못했어요" : "업데이트 적용 중"}</h1>
      </div>
      <div className="import__file">
        <span className="import__file-icon">
          <Icon name="file" size={22} className="c-accent" />
        </span>
        <span className="import__file-text">
          <span className="t-body-m-strong">{s.file_name}</span>
          <span className="t-body-s c-secondary">
            {[
              deckOf(s) && `덱 "${deckOf(s)}"`,
              touched > 0 && `바뀌는 노트 ${n(touched)}개`,
              s.new_notes > 0 && `새 노트 ${n(s.new_notes)}개`,
              cards > 0 && `새 카드 ${n(cards)}장`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </span>
        </span>
      </div>
      {failed ? (
        <div className={`import__fail ${stopped ? "du-stopped" : ""}`}>
          <Icon name={stopped ? "info" : noBackup ? "alert" : "x"} size={20} className={stopped ? "c-secondary" : "c-danger"} />
          <span className="import__fail-text">
            <span className={`t-body-m-strong ${stopped ? "" : "c-danger"}`}>
              {stopped ? "업데이트를 멈췄어요 · 아무것도 바뀌지 않았어요" : noBackup ? "백업을 만들 수 없어요 · 백업 없이는 적용하지 않아요" : "업데이트하지 못했어요 · 아무것도 바뀌지 않았어요"}
            </span>
            {!stopped && <span className="t-body-s c-secondary">{failed}</span>}
            <span className="du-actions">
              <Button variant="secondary" size="M" label="요약으로" onClick={onBack} />
              {noBackup ? (
                <Button variant="secondary" size="M" label="설정 › 이 기기 백업 열기" onClick={() => nav({ name: "settings" })} />
              ) : (
                <Button variant="secondary" size="M" label="다시 시도" onClick={() => (setFailed(undefined), setProg(null), setBackupAt(null), setRound((r) => r + 1))} />
              )}
            </span>
          </span>
        </div>
      ) : (
        <div className="import__progress">
          <div className="import__progress-head">
            <span className="t-title-s">적용하는 중</span>
            <span className="t-label-m c-accent">
              {Math.floor(ratio * 100)}%{left !== null && ` · 약 ${left < 60 ? `${left}초` : `${Math.ceil(left / 60)}분`} 남음`}
            </span>
          </div>
          <div className="progress__track">
            <span className="progress__fill progress__fill--accent" style={{ width: `${ratio * 100}%` }} />
          </div>
          {STEPS.map((x, i) => {
            const state = i < active ? "done" : i === active ? "active" : "wait";
            return (
              <div key={x.key} className="import__step">
                {state === "done" ? (
                  <Icon name="check" size={18} className="c-success" />
                ) : state === "active" ? (
                  <span className="import__step-icon c-accent">
                    <Spinner size={18} />
                  </span>
                ) : (
                  <span className="import__wait" />
                )}
                <span className={`t-body-m ${state === "wait" ? "c-tertiary" : ""}`}>{x.label}</span>
                <span className={`t-body-s ${state === "active" && x.key === "notes" ? "c-accent" : "c-secondary"}`}>{value(x.key, state)}</span>
              </div>
            );
          })}
        </div>
      )}
      {!failed && (
        <div className="import__foot">
          <span className="t-body-s c-tertiary">중간에 멈추면 아무것도 바뀌지 않아요 · 만든 백업은 남아요</span>
          <span className="import__foot-right">
            <Button variant="ghost" label="취소" shortcut="Esc" onClick={cancel} busy={cancelling} busyLabel="멈추는 중…" />
            <Button label="적용하는 중…" disabled />
          </span>
        </div>
      )}
    </main>
  );
}

// ---------------------------------------------------------------- B3-u 업데이트 결과

function report(s: UpdateSummary) {
  const takeNew = s.conflicts.filter((c) => c.take_new).length;
  const lines = [
    "Neo-Anki 덱 업데이트 결과",
    `파일: ${s.file_name}${deckOf(s) ? ` → 덱 "${deckOf(s)}"` : ""}`,
    `시각: ${new Date().toLocaleString("ko-KR")}`,
    "",
    `새 버전으로 바꾼 노트 ${n(s.changed)}개 · 합친 노트 ${n(s.merged)}개 · 충돌 ${n(s.conflicts.length)}개 (내 것 ${n(s.conflicts.length - takeNew)} · 새 버전 ${n(takeNew)})`,
    `새 노트 ${n(s.new_notes)}개 · 새 카드 ${n(s.new_note_cards + s.new_cards)}장 · 지킨 노트 ${n(s.mine_kept)}개`,
    `새 버전에 없어 그대로 둔 노트 ${n(s.missing_notes)}개 · 내가 지운 노트 ${n(s.gone)}개 (다시 넣지 않음)`,
    `빈칸: 새 번호 카드 ${n(s.cloze.new_cards)}장 · 학습에서 빠진 옛 번호 카드 ${n(s.cloze.emptied_cards)}장`,
    `내 모양을 지킨 노트 유형 ${n(s.kept_looks)}개 · 새 미디어 ${n(s.media_new)}개 · 가져오지 않은 복습 기록 ${n(s.reviews_skipped)}건`,
    s.backup ? `업데이트 전 백업: ${s.backup}` : "",
    "",
    "항목",
    ...s.issues.map((i) => `[${{ skipped: "건너뜀", warning: "경고", info: "참고" }[i.severity]}] ${i.detail}`),
  ];
  return lines.join("\n");
}

export function UpdateResultScreen({ summary: s }: { summary: UpdateSummary; info: AnkiPreview }) {
  const nav = useNav();
  const sync = useSyncStatus();
  const [saved, setSaved] = useState<string>();
  const [error, setError] = useState<string>();
  const synced = !!sync?.connected && !!sync.enabled;
  const root = s.roots[0] ?? null;
  const openDeck = () => nav(root !== null ? { name: "deck", deckId: root } : { name: "home" });
  useKey("Escape", openDeck);
  const takeNew = s.conflicts.filter((c) => c.take_new).length;
  const tiles = [
    s.changed + s.merged > 0 && { label: "바꾼 노트", value: n(s.changed + s.merged), sub: `새 버전 ${n(s.changed)} · 합친 노트 ${n(s.merged)}` },
    s.conflicts.length > 0 && { label: "충돌", value: n(s.conflicts.length), sub: `내 것 ${n(s.conflicts.length - takeNew)} · 새 버전 ${n(takeNew)}` },
    s.new_notes > 0 && { label: "새 노트", value: n(s.new_notes), sub: `새 카드 ${n(s.new_note_cards + s.new_cards)}장 · 신규` },
    s.mine_kept > 0 && { label: "지킨 노트", value: n(s.mine_kept), sub: "내가 고친 노트" },
  ].filter(Boolean) as { label: string; value: string; sub: string }[];
  const looks = s.kept_looks > 0 ? ` 내가 지킨 모양 ${n(s.kept_looks)}종은 그대로예요.` : "";
  const save_ = async () => {
    try {
      const path = await save({ defaultPath: `업데이트 결과 ${s.file_name}.txt`, filters: [{ name: "텍스트", extensions: ["txt"] }] });
      if (!path) return;
      await api.saveText(path, report(s));
      setError(undefined);
      setSaved("저장했어요");
    } catch (e) {
      setError(errorText(e));
    }
  };
  return (
    <main className="result">
      <div className="result__head">
        <span className="result__icon">
          <Icon name="check" size={26} className="c-success" />
        </span>
        <span className="result__title">
          <h1 className="t-title-l">업데이트를 마쳤어요</h1>
          <span className="t-body-s c-secondary">
            {s.file_name}
            {deckOf(s) ? ` → 덱 "${deckOf(s)}"` : ""} · 원본 파일은 그대로예요
          </span>
        </span>
      </div>
      {tiles.length > 0 ? (
        <div className="result__tiles">
          {tiles.map((t) => (
            <StatTile key={t.label} label={t.label} value={t.value} sub={t.sub} />
          ))}
        </div>
      ) : (
        <p className="t-body-m c-secondary">새 버전에서 바뀐 노트가 없었어요</p>
      )}
      <div className="result__list">
        {s.missing_notes > 0 && (
          <div className="result__row">
            <Icon name="info" size={20} className="c-warn" />
            <span className="result__row-text">
              <span className="t-body-m-strong">새 버전에 없어 그대로 둔 노트 {n(s.missing_notes)}개</span>
              <span className="t-body-s c-secondary">지우지 않았어요. 카드 · 기록 · 일정 그대로 학습에 나와요.</span>
            </span>
            <Button variant="ghost" size="M" label="카드 목록에서 보기" onClick={() => nav({ name: "cards", deckId: null, cardIds: s.missing_card_ids })} />
          </div>
        )}
        {s.cloze.renumbered_notes + s.cloze.new_cards + s.cloze.emptied_cards > 0 && (
          <div className="result__row">
            <Icon name="list" size={20} className="c-secondary" />
            <span className="result__row-text">
              <span className="t-body-m-strong">
                빈칸 번호를 따라간 노트{s.cloze.renumbered_notes > 0 ? ` ${n(s.cloze.renumbered_notes)}개` : ""}
              </span>
              <span className="t-body-s c-secondary">
                새 번호 카드 {n(s.cloze.new_cards)}장은 신규로 시작하고, 옛 번호 카드 {n(s.cloze.emptied_cards)}장은 학습에서 빠졌어요 (기록 · 일정 보존).
              </span>
            </span>
          </div>
        )}
        {s.backup && (
          <div className="result__row">
            <Icon name="download" size={20} className="c-success" />
            <span className="result__row-text">
              <span className="t-body-m-strong">업데이트 전 백업을 만들었어요</span>
              <span className="t-body-s c-secondary">되돌리려면 설정 › 이 기기 백업에서 [백업으로 복구] · 그 뒤의 학습도 함께 돌아가요.{synced ? "" : looks}</span>
            </span>
            <Button variant="ghost" size="M" label="백업으로 복구" onClick={() => nav({ name: "settings", focus: "restore", backup: s.backup!.split(/[\\/]/).pop() })} />
          </div>
        )}
        {synced && (
          <div className="result__row">
            <Icon name="rotate" size={20} className="c-secondary" />
            <span className="result__row-text">
              <span className="t-body-m-strong">다른 기기에도 다음 동기화 때 반영돼요</span>
              <span className="t-body-s c-secondary">복습 기록과 일정은 바꾸지 않았어요.{looks}</span>
            </span>
          </div>
        )}
        {s.issues.length > 0 && (
          <div className="result__row">
            <Icon name="info" size={20} scaled className="c-secondary" />
            <span className="result__row-text">
              <span className="t-body-m-strong">참고할 항목</span>
              <IssueList issues={s.issues} />
            </span>
          </div>
        )}
      </div>
      <div className="result__foot">
        <Button variant="ghost" size="M" icon="download" label={saved ?? "결과를 파일로 저장"} onClick={save_} />
        <span className="import__foot-right">
          {error && <span className="t-body-s c-danger">{error}</span>}
          {root !== null && <Button variant="secondary" label="학습 시작" onClick={() => nav({ name: "summary", deckIds: [root] })} />}
          <Button icon="arrow-right" label="덱 열기" onClick={openDeck} />
        </span>
      </div>
    </main>
  );
}
