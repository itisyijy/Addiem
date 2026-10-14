// S1-6 학습 데이터를 열지 못함 (Figma 26:3850 의 6번). 홈 자리를 빈 상태처럼 쓴다.
// 자동 복구는 하지 않는다. 마지막 백업 시각을 보여주고 사용자가 고른다
import { useState } from "react";
import { api, errorText, type AppInfo } from "../api";
import { Button, Icon } from "../ui";

const when = (ms: number) =>
  new Intl.DateTimeFormat("ko-KR", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }).format(ms);

export function Recover({ info, onRecovered }: { info: AppInfo; onRecovered: () => void }) {
  const [busy, setBusy] = useState<"retry" | "restore" | null>(null);
  const [error, setError] = useState<string>();

  const run = async (kind: "retry" | "restore") => {
    setBusy(kind);
    try {
      if (kind === "retry") await api.retryOpen();
      else await api.restoreBackup();
      onRecovered();
    } catch (e) {
      setError(errorText(e));
      setBusy(null);
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
        <Button variant="secondary" size="M" icon="rotate" label="다시 시도" busy={busy === "retry"} busyLabel="여는 중…" onClick={() => run("retry")} />
        {info.last_backup && (
          <Button size="M" icon="download" label="백업에서 복구" busy={busy === "restore"} busyLabel="복구하는 중…" onClick={() => run("restore")} />
        )}
      </div>
      <p className="recover__detail t-caption c-tertiary">{error ?? info.startup_error}</p>
    </main>
  );
}
