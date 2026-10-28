// S1-6 학습 데이터를 열지 못함 (Figma 26:3850 의 6번). 홈 자리를 빈 상태처럼 쓴다.
// 자동 복구는 하지 않는다. 마지막 백업 시각을 보여주고 사용자가 고른다.
// 백업이 없으면 데이터 폴더를 열거나 (확인 뒤) 빈 데이터로 시작한다. 열지 못한 파일은 지우지 않는다 (QA T8-11)
import { useState } from "react";
import { api, errorText, type AppInfo } from "../api";
import { Button, Icon } from "../ui";
import { Modal } from "./NewDeck";

const when = (ms: number) =>
  new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }).format(ms);

type Act = "retry" | "restore" | "fresh";

export function Recover({ info, onRecovered }: { info: AppInfo; onRecovered: () => void }) {
  const [busy, setBusy] = useState<Act | null>(null);
  const [error, setError] = useState<string>();
  const [asking, setAsking] = useState(false);

  const run = async (kind: Act) => {
    setBusy(kind);
    try {
      if (kind === "retry") await api.retryOpen();
      else if (kind === "restore") await api.restoreBackup();
      else await api.startFresh();
      onRecovered();
    } catch (e) {
      setError(errorText(e));
      setBusy(null);
      setAsking(false);
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
        {info.last_backup ? (
          <Button size="M" icon="download" label="백업으로 복구" busy={busy === "restore"} busyLabel="복구하는 중…" disabled={!!busy} onClick={() => run("restore")} />
        ) : (
          <Button size="M" icon="plus" label="빈 데이터로 시작" disabled={!!busy} onClick={() => setAsking(true)} />
        )}
      </div>
      <Button variant="ghost" size="M" icon="folder" label="데이터 폴더 열기" onClick={() => api.openDataDir().catch((e) => setError(errorText(e)))} />
      <p className="recover__detail t-caption c-tertiary">{error ?? (info.startup_error && errorText(info.startup_error))}</p>
      {asking && (
        <Modal title="빈 데이터로 시작할까요?" onClose={() => busy || setAsking(false)} variant="confirm" icon="alert">
          <p className="t-body-m c-secondary">
            열지 못한 데이터 파일은 지우지 않고 데이터 폴더에 남겨 둬요. 덱과 카드가 없는 상태로 시작하고, Google Drive에 올려 둔 데이터가 있으면 홈에서 다시 불러올 수 있어요.
          </p>
          <div className="modal__actions">
            <Button variant="ghost" size="M" label="취소" shortcut="Esc" disabled={!!busy} onClick={() => setAsking(false)} />
            <Button variant="danger" size="M" label="빈 데이터로 시작" busy={busy === "fresh"} busyLabel="시작하는 중…" onClick={() => run("fresh")} />
          </div>
        </Modal>
      )}
    </main>
  );
}
