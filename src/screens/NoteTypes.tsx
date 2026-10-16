// NT1 설정 › 노트 유형 목록 · [새 노트 유형](복제로 시작, NT3) · 이름 바꾸기 · 삭제. 설정 칸(NoteTypesEntry)도 여기
import { useCallback, useEffect, useState } from "react";
import { api, errorText, type NoteTypeRow as TypeRow } from "../api";
import { Button, Icon, Input, type IconName } from "../ui";
import { n, eul, useKey, useNav, useOverlayScope, type Route } from "../nav";
import { useToast } from "../toast";
import { Modal } from "./NewDeck";
import { menuKeys } from "./Deck";
import { TYPE_LABEL, rememberType } from "./AddCard";
import "../styles/notetypes.css";

/** 앱 기본 유형 행 순서. 목록 · 선택형 · 그림 가리기는 한 행으로 묶는다 (앱이 직접 그리는 유형) */
const APP_ORDER = ["basic", "reversed", "cloze", "written"];
const DRAWN = ["io", "list", "choice"];

/** 화면에 쓰는 이름. 앱 기본 유형은 카드 추가 세그먼트 이름 (DB 이름은 Anki 의 "Basic" 등) */
export const typeLabel = (t: { name: string; native_kind: string | null }) => (t.native_kind ? (TYPE_LABEL[t.native_kind] ?? t.name) : t.name);

/** "필드 2개 · 카드 종류 1개 · 토익 영단어에서 씀" */
export function typeSub(r: TypeRow) {
  const parts = [`필드 ${n(r.fields)}개`, r.cloze ? "빈칸 수만큼 카드" : `카드 종류 ${n(r.templates)}개`];
  if (!r.native_kind && r.decks.length) parts.push(`${r.decks[0]}${r.decks.length > 1 ? ` 외 ${n(r.decks.length - 1)}곳` : ""}에서 씀`);
  return parts.join(" · ");
}

/** 겹치지 않는 이름: "질문과 답변 복사본", 있으면 " 2", " 3" … (엔진 clone_type 과 같은 규칙) */
function uniqueName(base: string, rows: TypeRow[]) {
  const taken = new Set(rows.map((r) => r.name));
  let name = base;
  for (let i = 2; taken.has(name); i++) name = `${base} ${i}`;
  return name;
}

const groups = (rows: TypeRow[]) => ({
  app: APP_ORDER.flatMap((k) => rows.filter((r) => r.native_kind === k)),
  drawn: DRAWN.flatMap((k) => rows.filter((r) => r.native_kind === k)),
  imported: rows.filter((r) => !r.native_kind && !r.user_made),
  mine: rows.filter((r) => r.user_made),
});

// ---------------------------------------------------------------- 설정 칸 (입구)

export function NoteTypesEntry() {
  const nav = useNav();
  const [rows, setRows] = useState<TypeRow[] | null>(null);
  useEffect(() => {
    api.noteTypes().then(setRows, () => setRows([]));
  }, []);
  const g = rows && groups(rows);
  return (
    <section className="prefs__section">
      <p className="prefs__label t-label-s c-tertiary">노트 유형</p>
      <div className="prefs__row">
        <span className="prefs__text">
          <span className="t-body-m-strong">카드를 만드는 틀이에요</span>
          <span className="t-body-s c-secondary">
            어떤 필드(입력 칸)를 받아 앞면 · 뒷면에 어떻게 놓을지 정해요.
            {g && ` 앱 기본 ${n(g.app.length + g.drawn.length)}개 · Anki에서 가져옴 ${n(g.imported.length)}개 · 내가 만든 것 ${n(g.mine.length)}개`}
          </span>
        </span>
        <Button variant="secondary" size="M" label="노트 유형 관리" onClick={() => nav({ name: "noteTypes" })} />
      </div>
    </section>
  );
}

// ---------------------------------------------------------------- NT1 목록

type MenuItem = { icon: IconName; label: string; onClick: () => void; why?: string; danger?: boolean; sep?: boolean };

export function NoteTypes({ back }: { back?: Route }) {
  const nav = useNav();
  const toast = useToast();
  const [rows, setRows] = useState<TypeRow[] | null>(null);
  const [error, setError] = useState<string>();
  const [menu, setMenu] = useState<string | null>(null); // 메뉴를 연 행 (id, 묶은 행은 "drawn")
  const [cloning, setCloning] = useState<TypeRow | "new" | null>(null);
  const [renaming, setRenaming] = useState<TypeRow | null>(null);
  const [deleting, setDeleting] = useState<TypeRow | null>(null);
  const load = useCallback(() => api.noteTypes().then(setRows, (e) => setError(errorText(e))), []);
  useEffect(() => {
    load();
  }, [load]);
  const to = back ?? { name: "settings" };
  const goBack = useCallback(() => nav(to), [nav, to]);
  const busy = !!(cloning || renaming || deleting);
  useKey("Escape", goBack, !busy && !menu);
  useOverlayScope(menu !== null);
  useKey("Escape", () => setMenu(null), menu !== null, { modal: true });
  const open = (r: TypeRow) => nav({ name: "noteTypeEdit", id: r.id, back: { name: "noteTypes", back } });

  if (!rows) return error ? <p className="load-error t-body-m c-danger">{error}</p> : null;
  const g = groups(rows);

  const items = (r: TypeRow): MenuItem[] =>
    r.editable
      ? [
          { icon: "pencil", label: "편집", onClick: () => open(r) },
          { icon: "layers", label: "복제", onClick: () => setCloning(r) },
          { icon: "pencil", label: "이름 바꾸기", onClick: () => setRenaming(r) },
          {
            icon: "trash",
            label: "삭제",
            sep: true,
            danger: true,
            onClick: () => setDeleting(r),
            why: r.notes > 0 ? `이 유형을 쓰는 노트 ${n(r.notes)}개가 있어 지울 수 없어요. 노트를 모두 지우면 지울 수 있어요.` : undefined,
          },
        ]
      : [{ icon: "file", label: "보기", onClick: () => open(r) }, ...(r.cloneable ? [{ icon: "layers" as const, label: "복제", onClick: () => setCloning(r) }] : [])];

  const row = (key: string, r: TypeRow, icon: IconName, title: string, sub: string, counts: { notes: number; cards: number }, menuItems: MenuItem[]) => (
    <div key={key} className={`ntl__row ${menu === key ? "is-open" : ""}`}>
      <button className="ntl__main" onClick={() => open(r)}>
        <span className="ntl__icon">
          <Icon name={icon} size={16} className="c-secondary" />
        </span>
        <span className="ntl__text">
          <span className="t-body-m-strong">{title}</span>
          <span className="t-caption c-secondary">{sub}</span>
        </span>
        <span className={`ntl__counts ${counts.notes ? "" : "c-tertiary"}`}>
          <span className="t-label-m">노트 {n(counts.notes)}개</span>
          <span className="t-caption c-tertiary">카드 {n(counts.cards)}장</span>
        </span>
      </button>
      <span className="menu">
        <button
          className="ntl__more"
          aria-label={`${title} 메뉴`}
          aria-haspopup="menu"
          aria-expanded={menu === key}
          onClick={(e) => {
            e.stopPropagation();
            setMenu(menu === key ? null : key);
          }}
        >
          <Icon name="more" size={18} />
        </button>
        {menu === key && (
          <div
            className="menu__list ntl__menu"
            role="menu"
            aria-label={`${title} 메뉴`}
            onKeyDown={menuKeys}
            onClick={(e) => e.stopPropagation()}
            onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setMenu(null)}
          >
            {menuItems.map((m, i) => (
              <div key={m.label + i} className="ntl__menu-item">
                {m.sep && <div className="menu__sep" role="separator" />}
                <button
                  role="menuitem"
                  autoFocus={i === 0}
                  className={`menu__item menu__item--top ${m.danger && !m.why ? "menu__item--danger" : ""}`}
                  disabled={!!m.why}
                  onClick={() => (setMenu(null), m.onClick())}
                >
                  <Icon name={m.icon} size={16} />
                  <span className="menu__text">
                    <span className={`t-label-m ${m.why ? "c-tertiary" : ""}`}>{m.label}</span>
                    {m.why && <span className="t-caption c-secondary">{m.why}</span>}
                  </span>
                </button>
              </div>
            ))}
          </div>
        )}
      </span>
    </div>
  );

  const drawnSum = g.drawn.reduce((a, r) => ({ notes: a.notes + r.notes, cards: a.cards + r.cards }), { notes: 0, cards: 0 });
  return (
    <main className="ntl" onClick={() => setMenu(null)}>
      <button className="backlink t-label-m c-secondary" onClick={goBack}>
        <Icon name="chevron-left" size={18} />
        {back?.name === "addCard" ? "카드 추가" : back ? "돌아가기" : "설정"}
      </button>
      <h1 className="t-title-l">노트 유형</h1>
      <section className="ntl__section">
        <div className="ntl__head">
          <div className="ntl__title">
            <p className="ntl__heading">카드를 만드는 틀이에요</p>
            <p className="t-body-s c-secondary">어떤 필드(입력 칸)를 받아 앞면·뒷면에 어떻게 놓을지 정해요. 고치면 그 유형을 쓰는 카드가 모두 바뀌어요.</p>
          </div>
          <Button variant="secondary" size="M" icon="plus" label="새 노트 유형" onClick={() => setCloning("new")} />
        </div>

        <p className="t-label-s c-tertiary">앱 기본 · 고칠 때는 복제해요</p>
        <div className="ntl__group">
          {g.app.map((r) => row(String(r.id), r, "lock", typeLabel(r), typeSub(r), r, items(r)))}
          {g.drawn.length > 0 &&
            row(
              "drawn",
              g.drawn[0],
              "lock",
              g.drawn.map(typeLabel).join(" · "),
              "앱이 직접 그리는 유형 · 복제할 수 없어요",
              drawnSum,
              g.drawn.map((r) => ({ icon: "file" as const, label: `${typeLabel(r)} 보기`, onClick: () => open(r) })),
            )}
        </div>
        {g.imported.length > 0 && (
          <>
            <p className="t-label-s c-tertiary">Anki에서 가져옴</p>
            <div className="ntl__group">{g.imported.map((r) => row(String(r.id), r, "download", r.name, typeSub(r), r, items(r)))}</div>
          </>
        )}
        <p className="t-label-s c-tertiary">내가 만든 것</p>
        <div className="ntl__group">
          {g.mine.map((r) => row(String(r.id), r, "pencil", r.name, typeSub(r), r, items(r)))}
          {g.mine.length === 0 && <p className="ntl__empty t-body-s c-tertiary">아직 없어요. [새 노트 유형]으로 앱 기본이나 가져온 유형을 복제해 만들어요.</p>}
        </div>
      </section>

      {cloning && <CloneDialog rows={rows} from={cloning === "new" ? undefined : cloning} back={{ name: "noteTypes", back }} onClose={() => setCloning(null)} />}
      {renaming && (
        <RenameDialog
          row={renaming}
          onClose={() => setRenaming(null)}
          onDone={() => {
            setRenaming(null);
            toast({ title: "이름을 바꿨어요" });
            load();
          }}
        />
      )}
      {deleting && (
        <Modal title={`「${deleting.name}」 노트 유형을 지울까요?`} onClose={() => setDeleting(null)} variant="confirm" icon="trash">
          <p className="t-body-s c-secondary">이 유형을 쓰는 노트가 없어 카드는 지워지지 않아요. 되돌릴 수 없어요.</p>
          {error && <p className="t-body-s c-danger">{error}</p>}
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" onClick={() => setDeleting(null)} />
            <Button
              variant="danger"
              size="M"
              icon="trash"
              label="삭제"
              autoFocus
              onClick={() =>
                api.deleteNoteType(deleting.id).then(
                  () => {
                    setDeleting(null);
                    toast({ title: "노트 유형을 지웠어요" });
                    load();
                  },
                  (e) => setError(errorText(e)),
                )
              }
            />
          </div>
        </Modal>
      )}
    </main>
  );
}

// ---------------------------------------------------------------- 복제 (새 노트 유형 · NT4 [복제해서 고치기])

/** 이 경로를 거슬러 카드 추가(C3)가 있으면 그 덱 */
const addCardDeck = (r: Route | undefined): number | undefined => (!r ? undefined : r.name === "addCard" ? r.deckId : "back" in r ? addCardDeck(r.back) : undefined);

/**
 * from 이 없으면 "어떤 유형에서 시작할까요?" (복제할 수 있는 유형만, 빈 유형 없음 NT3). 만들면 그 사본을 편집기로 연다 (back: 편집기에서 돌아갈 곳).
 * 카드 추가에서 왔으면 그 덱의 마지막 유형(NT12)을 사본으로 바꿔, 돌아가면 사본이 골라져 있다 (NT52)
 */
export function CloneDialog({ rows: given, from, back, onClose }: {
  rows?: TypeRow[];
  from?: Pick<TypeRow, "id" | "name" | "native_kind">;
  back: Route;
  onClose: () => void;
}) {
  const nav = useNav();
  const [rows, setRows] = useState<TypeRow[]>(given ?? []);
  const [src, setSrc] = useState<Pick<TypeRow, "id" | "name" | "native_kind"> | undefined>(from);
  const [name, setName] = useState("");
  const [edited, setEdited] = useState(false);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!given) api.noteTypes().then(setRows, (e) => setError(errorText(e)));
  }, [given]);
  const choices = (() => {
    const g = groups(rows.filter((r) => r.cloneable));
    return [...g.app, ...g.imported, ...g.mine];
  })();
  const pick = src ?? choices[0];
  // 고른 유형이 바뀌면 이름도 따라간다 (직접 고쳤으면 그대로)
  useEffect(() => {
    // 앱 유형은 "내 질문과 답변" (세그먼트에서 앞부분으로 구분된다, NT55), 그 밖은 "X 복사본" (엔진 clone_type 과 같다)
    if (pick && !edited) setName(uniqueName(pick.native_kind ? `내 ${typeLabel(pick)}` : `${pick.name} 복사본`, rows));
  }, [pick, edited, rows]);

  const submit = async () => {
    if (!pick || busy) return;
    setBusy(true);
    try {
      const id = await api.cloneNoteType(pick.id, name.trim());
      const deck = addCardDeck(back);
      if (deck !== undefined) rememberType(deck, id);
      nav({ name: "noteTypeEdit", id, back }, true);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={from ? "복제해서 고치기" : "새 노트 유형"}
      sub={from ? `${typeLabel(from)}${eul(typeLabel(from))} 복제해 내 유형으로 만들어요. 이 유형을 쓰던 카드는 그대로예요.` : "어떤 유형에서 시작할까요?"}
      className="ntl__clone"
      onClose={onClose}
    >
      <form
        className="modal__body"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        {!from && (
          <div className="ntl__pick" role="radiogroup" aria-label="시작할 유형">
            {choices.map((r) => (
              <button
                key={r.id}
                type="button"
                role="radio"
                aria-checked={pick?.id === r.id}
                className={`ntl__pick-row ${pick?.id === r.id ? "is-on" : ""}`}
                onClick={() => setSrc(r)}
              >
                <Icon name={r.native_kind ? "lock" : r.user_made ? "pencil" : "download"} size={16} className="c-secondary" />
                <span className="ntl__text">
                  <span className="t-label-m">{typeLabel(r)}</span>
                  <span className="t-caption c-tertiary">{typeSub(r)}</span>
                </span>
                {pick?.id === r.id && <Icon name="check" size={16} className="c-accent" />}
              </button>
            ))}
          </div>
        )}
        <Input
          label="이름"
          value={name}
          autoFocus={!!from}
          error={error}
          onChange={(e) => {
            setName(e.target.value);
            setEdited(true);
            setError(undefined);
          }}
        />
        <div className="modal__actions">
          <Button type="button" variant="ghost" size="M" label="취소" onClick={onClose} />
          <Button type="submit" size="M" icon="layers" label="복제해서 만들기" disabled={!pick || !name.trim()} busy={busy} />
        </div>
      </form>
    </Modal>
  );
}

function RenameDialog({ row, onClose, onDone }: { row: TypeRow; onClose: () => void; onDone: () => void }) {
  const [name, setName] = useState(row.name);
  const [error, setError] = useState<string>();
  const same = name.trim() === row.name || !name.trim();
  return (
    <Modal title="노트 유형 이름 바꾸기" onClose={onClose}>
      <form
        className="modal__body"
        onSubmit={(e) => {
          e.preventDefault();
          if (!same) api.renameNoteType(row.id, name).then(onDone, (e) => setError(errorText(e)));
        }}
      >
        <Input
          label="이름"
          value={name}
          autoFocus
          error={error}
          onChange={(e) => {
            setName(e.target.value);
            setError(undefined);
          }}
        />
        <div className="modal__actions">
          <Button type="button" variant="ghost" size="M" label="취소" onClick={onClose} />
          <Button type="submit" size="M" label="바꾸기" disabled={same || !!error} />
        </div>
      </form>
    </Modal>
  );
}
