// S1-6 학습 데이터를 열지 못함 (Figma 26:3850 의 6번). 홈 자리를 빈 상태처럼 쓴다.
// 자동 복구는 하지 않는다. 마지막 백업 시각을 보여주고 사용자가 고른다.
// 백업이 없거나 쓸 수 있는 백업이 없으면 데이터 폴더를 열거나 (확인 뒤) 빈 데이터로 시작한다. 열지 못한 파일은 지우지 않는다 (QA T8-11 · V1-05)
import { useState } from "react";
import { api, errorText, type AppInfo } from "../api";
import { useToast } from "../toast";
import { Button, Icon } from "../ui";
import { Modal } from "./NewDeck";

const when = (ms: number) =>
  new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }).format(ms);

/** empty: 카드 0장 백업밖에 없다고 확인한 뒤의 복구 (QA V1-01) */
type Act = "retry" | "restore" | "empty" | "fresh";

/** commands.rs ONLY_EMPTY 의 앞부분: 쓸 수 있는 백업이 모두 카드 0장 */
const ONLY_EMPTY = "카드가 있는 백업이 없어요";

export function Recover({ info, onRecovered }: { info: AppInfo; onRecovered: () => void }) {
  const toast = useToast();
  const [busy, setBusy] = useState<Act | null>(null);
  const [error, setError] = useState<string>();
  const [asking, setAsking] = useState<"fresh" | "empty" | null>(null);
  // 백업이 있어도 복구가 실패하면(모두 쓸 수 없음) 빈 데이터로 시작할 수 있게 (QA V1-05)
  const [restoreFailed, setRestoreFailed] = useState(false);

  const run = async (kind: Act) => {
    setBusy(kind);
    try {
      if (kind === "retry") await api.retryOpen();
      else if (kind === "fresh") await api.startFresh();
      else {
        const name = await api.restoreBackup(null, false, kind === "empty");
        // 어느 백업으로 돌아갔는지 알린다 (QA V1-01)
        const at = (await api.localBackups().catch(() => [])).find((b) => b.name === name)?.at;
        toast({ title: at ? `${when(at)} 백업으로 복구했어요` : "백업으로 복구했어요", text: "열지 못한 데이터 파일은 데이터 폴더에 남겨 뒀어요", ms: 5000 });
      }
      onRecovered();
    } catch (e) {
      const msg = errorText(e);
      setBusy(null);
      if (kind === "restore" || kind === "empty") setRestoreFailed(true);
      if (kind === "restore" && msg.startsWith(ONLY_EMPTY)) return setAsking("empty");
      setError(msg);
      setAsking(null);
    }
  };

  return (
    <main className="recover">
      <span className="recover__icon">
        <Icon name="alert" size={22} scaled className="c-warn" />
      </span>
      <h1 className="t-title-s">학습 데이터를 열지 못했어요</h1>
      <p className="t-caption c-tertiary">{info.last_backup ? `마지막 백업 · ${when(info.last_backup)}` : "백업이 아직 없어요"}</p>
      <div className="recover__actions">
        <Button variant="secondary" size="M" icon="rotate" label="다시 시도" busy={busy === "retry"} busyLabel="여는 중…" disabled={!!busy} onClick={() => run("retry")} />
        {info.last_backup && (
          <Button size="M" icon="download" label="백업으로 복구" busy={busy === "restore"} busyLabel="복구하는 중…" disabled={!!busy} onClick={() => run("restore")} />
        )}
        {(!info.last_backup || restoreFailed) && (
          <Button variant={info.last_backup ? "secondary" : "primary"} size="M" icon="plus" label="빈 데이터로 시작" disabled={!!busy} onClick={() => setAsking("fresh")} />
        )}
      </div>
      <Button variant="ghost" size="M" icon="folder" label="데이터 폴더 열기" onClick={() => api.openDataDir().catch((e) => setError(errorText(e)))} />
      <p className="recover__detail t-caption c-tertiary">{error ?? (info.startup_error && errorText(info.startup_error))}</p>
      {asking === "fresh" && (
        <Modal title="빈 데이터로 시작할까요?" onClose={() => busy || setAsking(null)} variant="confirm" icon="alert">
          <p className="t-body-m c-secondary">
            열지 못한 데이터 파일은 지우지 않고 데이터 폴더에 남겨 둬요. 덱과 카드가 없는 상태로 시작하고, Google Drive에 올려 둔 데이터가 있으면 홈에서 다시 불러올 수 있어요.
          </p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" disabled={!!busy} onClick={() => setAsking(null)} />
            <Button variant="danger" size="M" label="빈 데이터로 시작" busy={busy === "fresh"} busyLabel="시작하는 중…" onClick={() => run("fresh")} />
          </div>
        </Modal>
      )}
      {asking === "empty" && (
        <Modal title="카드가 없는 백업으로 복구할까요?" onClose={() => busy || setAsking(null)} variant="confirm" icon="alert">
          <p className="t-body-m c-secondary">
            카드가 있는 백업을 찾지 못했어요. 쓸 수 있는 백업에는 카드가 한 장도 없어요. 복구하면 카드 없이 시작하고, 열지 못한 데이터 파일은 지우지 않고 데이터 폴더에 남겨 둬요.
          </p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" disabled={!!busy} onClick={() => setAsking(null)} />
            <Button variant="danger" size="M" label="이 백업으로 복구" busy={busy === "empty"} busyLabel="복구하는 중…" onClick={() => run("empty")} />
          </div>
        </Modal>
      )}
    </main>
  );
}
