/*
 * Figma "Components" 프레임의 컴포넌트를 옮긴 것. 이름과 variant 는 Figma 와 맞춘다.
 * 스타일은 styles/components.css.
 */
import { useEffect, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

// ---------------------------------------------------------------- Icon

const iconUrls = import.meta.glob("./assets/icons/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
}) as Record<string, string>;

export type IconName =
  | "plus" | "x" | "chevron-right" | "chevron-down" | "chevron-left" | "check" | "settings"
  | "upload" | "download" | "pencil" | "archive" | "alert" | "info" | "image" | "search"
  | "folder" | "calendar" | "file" | "arrow-right" | "layers" | "list" | "more" | "grip"
  | "rotate" | "trash" | "target" | "clock" | "check-circle"
  // K Google Drive (Figma Row K)
  | "cloud" | "cloud-check" | "cloud-off" | "lock" | "laptop" | "merge" | "external" | "link-off"
  // G2 성격별로 묶기 · ChatGPT (Figma Row L)
  | "sparkles" | "gauge" | "wifi-off" | "send"
  // AU 소리 재생 (Figma Row AU)
  | "volume-2" | "volume-x" | "play" | "pause" | "speech";

/**
 * Figma 는 작은 크기에서도 선 두께를 2px 로 유지한다. 그래서 크기별 원본(plus-16.svg)이
 * 있으면 그것을 쓰고, 없으면 24px 원본을 줄인다 (Figma 가 인스턴스를 축소한 경우와 같다).
 * 색은 글자색을 따른다. 원본 파일은 마스크로만 쓰고 수정하지 않는다.
 */
export function Icon({ name, size = 24, className = "", scaled = false }: {
  name: IconName;
  size?: number;
  className?: string;
  /** Figma 가 24px 원본을 축소해 쓴 자리. 크기별 원본을 무시한다 */
  scaled?: boolean;
}) {
  const sized = scaled ? undefined : iconUrls[`./assets/icons/${name}-${size}.svg`];
  const url = sized ?? iconUrls[`./assets/icons/${name}.svg`];
  const mask = `url("${url}") center / 100% 100% no-repeat`;
  return (
    <span
      aria-hidden
      className={`icon ${className}`}
      style={{ width: size, height: size, WebkitMask: mask, mask }}
    />
  );
}

// ---------------------------------------------------------------- Kbd

export function Kbd({ k, tone = "default" }: { k: string; tone?: "default" | "onAccent" }) {
  return <span className={`kbd kbd--${tone} t-mono-key`}>{k}</span>;
}

// ---------------------------------------------------------------- Button

type ButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "style"> & {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "L" | "M";
  icon?: IconName;
  shortcut?: string;
  label: ReactNode;
  /** 오래 걸리는 일 (Figma S1-1). 300ms 가 지나도 끝나지 않으면 글자를 busyLabel 로 바꾸고 스피너를 띄운다 */
  busy?: boolean;
  busyLabel?: ReactNode;
};

/** 300ms 안에 끝나는 일은 아무것도 바꾸지 않는다 (Figma S1-1) */
function useSlow(busy: boolean) {
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (!busy) return setSlow(false);
    const t = setTimeout(() => setSlow(true), 300);
    return () => clearTimeout(t);
  }, [busy]);
  return slow;
}

export function Spinner({ size = 16 }: { size?: number }) {
  return <span className="spinner" style={{ width: size, height: size }} aria-hidden />;
}

export function Button({ variant = "primary", size = "L", icon, shortcut, label, className = "", type = "button", busy = false, busyLabel, disabled, ...rest }: ButtonProps) {
  const slow = useSlow(busy);
  return (
    <button type={type} className={`btn btn--${variant} btn--${size} ${slow ? "is-busy" : ""} ${className}`} disabled={disabled || busy} {...rest}>
      {icon && !slow && <Icon name={icon} size={size === "L" ? 20 : 16} />}
      <span className={size === "L" ? "t-title-s" : "t-label-m"}>{slow && busyLabel ? busyLabel : label}</span>
      {slow ? <Spinner /> : shortcut && <Kbd k={shortcut} tone={variant === "primary" ? "onAccent" : "default"} />}
    </button>
  );
}

// ---------------------------------------------------------------- Badge

export type BadgeTone = "neutral" | "accent" | "warn" | "danger" | "success" | "ink";

export function Badge({ tone = "neutral", children }: { tone?: BadgeTone; children: ReactNode }) {
  return <span className={`badge badge--${tone} t-label-s`}>{children}</span>;
}

// ---------------------------------------------------------------- Importance

export type Level = 0 | 1 | 2; // 0=보충 1=일반 2=핵심 (db/schema.sql 과 같다)
export const LEVEL_LABEL: Record<Level, string> = { 2: "핵심", 1: "일반", 0: "보충" };
const LEVEL_KEY: Record<Level, string> = { 2: "core", 1: "normal", 0: "supp" };

export function Importance({ level }: { level: Level }) {
  return (
    <span className={`importance importance--${LEVEL_KEY[level]} t-label-s`}>
      <span className={`dot dot--6 dot--${LEVEL_KEY[level]}`} />
      {LEVEL_LABEL[level]}
    </span>
  );
}

// ---------------------------------------------------------------- Segment

export function Segment<T extends string | number>({
  options,
  value,
  onChange,
  boxed,
  label,
  disabled,
}: {
  /** disabled: 그 선택지만 끈다 (P1 남은 양보다 큰 분량) */
  options: { value: T; label: string; disabled?: boolean }[];
  value: T;
  onChange: (v: T) => void;
  /** 회색 트랙 (segment--boxed) */
  boxed?: boolean;
  label?: string;
  disabled?: boolean;
}) {
  return (
    <div className={`segment ${boxed ? "segment--boxed" : ""}`} role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={String(o.value)}
          role="radio"
          aria-checked={o.value === value}
          className={`segment__item t-label-m ${o.value === value ? "is-selected" : ""}`}
          disabled={disabled || o.disabled}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- RatingButton

export type Rating = 1 | 2 | 3 | 4;
const RATING_NAME: Record<Rating, string> = { 1: "Again", 2: "Hard", 3: "Good", 4: "Easy" };
const RATING_KEY: Record<Rating, string> = { 1: "again", 2: "hard", 3: "good", 4: "easy" };

export function RatingButton({ rating, hint, shortcut = String(rating), onClick }: { rating: Rating; hint?: string; shortcut?: string; onClick: () => void }) {
  return (
    <button className="rating" aria-keyshortcuts={shortcut} onClick={onClick}>
      <span className="rating__top">
        <span className="rating__name">
          <span className={`dot dot--${RATING_KEY[rating]}`} />
          <span className="t-title-s">{RATING_NAME[rating]}</span>
        </span>
        <Kbd k={shortcut} />
      </span>
      {hint && <span className="t-caption c-tertiary">{hint}</span>}
    </button>
  );
}

// ---------------------------------------------------------------- StatTile

export function StatTile({ label, value, sub }: { label: string; value: ReactNode; sub?: string }) {
  return (
    <div className="stat">
      <p className="t-body-s c-secondary">{label}</p>
      <p className="t-title-l">{value}</p>
      {sub && <p className="t-caption c-tertiary">{sub}</p>}
    </div>
  );
}

// ---------------------------------------------------------------- Input

type InputProps = InputHTMLAttributes<HTMLInputElement> & {
  label?: string;
  helper?: ReactNode;
  error?: string;
};

export function Input({ label, helper, error, className = "", ...rest }: InputProps) {
  return (
    <label className={`field ${className}`}>
      {label && <span className="t-label-m">{label}</span>}
      <input className={`field__input t-body-m ${error ? "is-error" : ""}`} {...rest} />
      {error ? (
        <span className="t-caption c-danger">{error}</span>
      ) : (
        helper && <span className="t-caption c-tertiary">{helper}</span>
      )}
    </label>
  );
}

// ---------------------------------------------------------------- Checkbox

/** quiet: 글자 없이 상자만 (label 은 화면 읽기용). 목록 행의 고르기 */
export function Checkbox({ checked, label, onChange, quiet, disabled }: { checked: boolean; label: string; onChange: (v: boolean) => void; quiet?: boolean; disabled?: boolean }) {
  return (
    <label className={`checkbox ${disabled ? "is-disabled" : ""}`} onClick={(e) => e.stopPropagation()}>
      <input type="checkbox" checked={checked} disabled={disabled} aria-label={quiet ? label : undefined} onChange={(e) => onChange(e.target.checked)} />
      <span className={`checkbox__box ${checked ? "is-on" : ""}`}>{checked && <Icon name="check" size={14} />}</span>
      {!quiet && <span className="t-body-m">{label}</span>}
    </label>
  );
}

// ---------------------------------------------------------------- Toggle

export function Toggle({ on, onChange, label, disabled }: { on: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button role="switch" aria-checked={on} aria-label={label} className="toggle" disabled={disabled} onClick={() => onChange(!on)}>
    </button>
  );
}

// ---------------------------------------------------------------- SubdeckChip

export function SubdeckChip({ name, count, suggested }: { name: string; count: number; suggested?: boolean }) {
  return (
    <span className={`chip ${suggested ? "chip--suggested" : ""}`}>
      <Icon name="grip" size={16} />
      <span className="t-label-m">{name}</span>
      <span className="t-caption c-tertiary">{count.toLocaleString()}장</span>
      {suggested && <span className="t-label-s c-accent">제안</span>}
    </span>
  );
}

// ---------------------------------------------------------------- DeckRow

export function DeckRow({
  name,
  counts,
  total,
  badge,
  onStudy,
  onOpen,
}: {
  name: string;
  counts: string;
  total: number;
  badge?: string;
  onStudy?: () => void;
  onOpen: () => void;
}) {
  return (
    <div className="deckrow" onClick={onOpen} role="button" tabIndex={0} onKeyDown={(e) => e.key === "Enter" && onOpen()}>
      <div className="deckrow__main">
        <div className="deckrow__name">
          <span className="t-body-m-strong">{name}</span>
          {badge && <Badge tone="warn">{badge}</Badge>}
        </div>
        <span className="t-body-s c-secondary">{counts}</span>
      </div>
      <span className="deckrow__total">
        <span className="t-title-m">{total.toLocaleString()}</span>
        <span className="t-body-s c-tertiary">장</span>
      </span>
      {onStudy && (
        <Button
          variant="secondary"
          size="M"
          label="학습"
          onClick={(e) => {
            e.stopPropagation();
            onStudy();
          }}
        />
      )}
      <Icon name="chevron-right" size={18} className="c-tertiary" />
    </div>
  );
}

// ---------------------------------------------------------------- Logo, TopNav, Backdrop

// Addiem 점 격자 심볼 4×4 (design/logo). 대각선 점이 커지며 오른쪽 위 주황 점(그날)에 닿는다
const LOGO_DOTS = Array.from({ length: 16 }, (_, i) => {
  const r = Math.floor(i / 4), k = i % 4, onPath = r + k === 3;
  return { cx: 11 + k * 14, cy: 11 + r * 14, r: onPath ? [8, 6.5, 5, 3.5][r] : 3, fill: onPath ? (r === 0 ? "var(--brand-day)" : "var(--brand-path)") : "var(--brand-grid)" };
});

export function Logo() {
  return (
    <svg className="logo" viewBox="0 0 64 64" aria-hidden>
      {LOGO_DOTS.map((p) => <circle key={`${p.cx}-${p.cy}`} {...p} />)}
    </svg>
  );
}

// Newsreader 400 을 도형으로 바꾼 워드마크 (design/logo/addiem-wordmark.svg, 소수 한 자리로 줄임). i 점은 주황 원
const WORDMARK_D = "M26.9 39.4 27.2 40.8Q23.5 41.9 21.1 42.9Q18.8 43.9 17.5 44.9Q16.3 46 15.8 47.1Q15.3 48.2 15.3 49.6Q15.3 51.9 16.7 53Q18 54.2 19.9 54.2Q21.4 54.2 22.6 53.5Q23.7 52.9 24.4 51.7Q25.1 50.5 25.1 48.9V36.9Q25.1 34.3 23.8 32.9Q22.5 31.5 19.8 31.5Q18.6 31.5 17.3 31.7Q16 31.9 15.1 32.3L16.6 31.2Q16.5 32.1 16.3 33.1Q16.1 34.2 15.9 35Q15.6 35.8 15.3 36.2Q14.9 36.6 14.2 36.9Q13.5 37.1 12.7 37.1Q11.7 37.1 11.2 36.7Q10.6 36.3 10.6 35.5Q10.6 34.4 11.6 33.3Q12.7 32.2 14.3 31.3Q16 30.4 18 29.8Q19.9 29.3 21.8 29.3Q24.7 29.3 26.5 30.1Q28.3 31 29.1 32.6Q29.9 34.2 29.9 36.4V51.9Q29.9 52.8 30.2 53.4Q30.5 53.9 31 54.2Q31.5 54.4 32.2 54.4Q33.1 54.4 34 54.2Q34.9 53.9 35.8 53.2V54.2Q34 56.3 32.5 56.9Q31 57.5 29.5 57.5Q27.9 57.5 26.9 56.8Q26 56.1 25.6 54.8Q25.1 53.5 25.1 51.7L25.2 51.6Q24.7 53.3 23.7 54.7Q22.6 56 21.2 56.8Q19.7 57.6 17.9 57.6Q14.7 57.6 12.5 55.8Q10.3 54 10.3 50.7Q10.3 48.9 11 47.5Q11.6 46.2 13.3 44.9Q15.1 43.7 18.3 42.3Q21.6 41 26.9 39.4Z M57.8 36.9Q57.8 34.2 55.9 32.5Q54 30.8 51 30.8Q48.3 30.8 46.4 32.2Q44.5 33.6 43.6 36.3Q42.6 38.9 42.6 42.8Q42.6 46.5 43.6 49Q44.6 51.5 46.5 52.7Q48.4 54 51.1 54Q53.5 54 55.5 53.2Q57.5 52.4 59.4 50.6L59.6 51.6Q57.8 53.3 56.3 54.5Q54.9 55.7 53.6 56.3Q52.3 57 51.2 57.3Q50.1 57.6 49 57.6Q45.3 57.6 42.7 55.9Q40.1 54.3 38.7 51.4Q37.3 48.5 37.3 44.7Q37.3 41.1 38.4 38.2Q39.5 35.3 41.5 33.3Q43.5 31.3 46.2 30.2Q48.8 29.2 52 29.2Q53.5 29.2 55.1 29.4Q56.7 29.7 58.2 30.2Q59.7 30.6 61.2 31.4L57.8 32.5V20.8Q57.5 20.5 56.8 20.2Q56.1 19.8 55.2 19.4Q54.4 19 53.5 18.6V18.1L62.8 16H63L62.7 20.4V53.8Q62.9 54 63.4 54.2Q63.9 54.5 64.6 54.7Q65.2 55 65.8 55.2Q66.4 55.5 66.8 55.6V56.3L58.2 57.6H57.9L57.8 53.6Z M90 36.9Q90 34.2 88.1 32.5Q86.2 30.8 83.2 30.8Q80.5 30.8 78.6 32.2Q76.8 33.6 75.8 36.3Q74.8 38.9 74.8 42.8Q74.8 46.5 75.8 49Q76.8 51.5 78.7 52.7Q80.6 54 83.4 54Q85.7 54 87.7 53.2Q89.7 52.4 91.6 50.6L91.9 51.6Q90 53.3 88.6 54.5Q87.1 55.7 85.8 56.3Q84.6 57 83.4 57.3Q82.3 57.6 81.2 57.6Q77.6 57.6 75 55.9Q72.4 54.3 71 51.4Q69.6 48.5 69.6 44.7Q69.6 41.1 70.7 38.2Q71.8 35.3 73.7 33.3Q75.7 31.3 78.4 30.2Q81.1 29.2 84.2 29.2Q85.8 29.2 87.3 29.4Q88.9 29.7 90.4 30.2Q92 30.6 93.4 31.4L90 32.5V20.8Q89.7 20.5 89 20.2Q88.3 19.8 87.4 19.4Q86.6 19 85.8 18.6V18.1L95 16H95.2L94.9 20.4V53.8Q95.1 54 95.6 54.2Q96.2 54.5 96.8 54.7Q97.4 55 98 55.2Q98.6 55.5 99.1 55.6V56.3L90.4 57.6H90.2L90 53.6Z M107.2 23.5Q105.5 23.5 104.5 22.4Q103.5 21.3 103.5 19.8Q103.5 18.3 104.5 17.3Q105.5 16.2 107.2 16.2Q108.8 16.2 109.8 17.3Q110.8 18.3 110.8 19.8Q110.8 21.3 109.8 22.4Q108.8 23.5 107.2 23.5Z M110 29 110 34.2V55.2L114.1 56.2V57H101.1V56.2L105.1 55.2V34.4Q104.8 34.2 104.2 33.8Q103.6 33.4 102.8 32.9Q102 32.5 101.1 32V31.4L109.8 29Z M129.8 29.2Q132.7 29.2 134.8 30.4Q137 31.6 138.2 34Q139.5 36.3 139.7 39.8H121.6L121.6 38.1L136.8 37.9L134.7 39Q134.4 36.3 133.7 34.5Q133.1 32.6 131.9 31.7Q130.8 30.8 129.1 30.8Q126.9 30.8 125.4 32.1Q123.8 33.3 122.9 35.7Q122.1 38.1 122.1 41.5Q122.1 45.7 123.3 48.4Q124.5 51.1 126.8 52.4Q129.1 53.7 132.3 53.7Q133.7 53.7 135 53.4Q136.4 53 137.6 52.2Q138.8 51.5 139.7 50.3L140.3 50.8Q138.6 53.3 137 54.7Q135.3 56.2 133.5 56.9Q131.6 57.6 129.5 57.6Q126 57.6 123.1 55.9Q120.3 54.3 118.7 51.2Q117.1 48.2 117.1 43.8Q117.1 39.9 118.6 36.5Q120.1 33.2 123 31.2Q125.8 29.2 129.8 29.2Z M151.9 34.3V55.2L156 56.2V57H143V56.2L147.1 55.2V34.5Q146.7 34.2 145.8 33.7Q145 33.2 143.1 32.2V31.6L151.4 29H151.7Z M168.4 37.2V55.2L172.4 56.2V57H159.5V56.2L163.5 55.2V38.1Q163.5 36.5 162.9 35.4Q162.3 34.3 161.1 33.7Q159.8 33.1 158 33.1Q156.1 33.1 154.2 33.7Q152.4 34.3 151.3 35.1L150.8 34.3Q152.7 32.7 154.1 31.8Q155.5 30.8 156.7 30.2Q157.9 29.6 159 29.4Q160.1 29.2 161.4 29.2Q163.9 29.2 165.4 30.1Q166.9 31.1 167.6 32.8Q168.4 34.6 168.4 37.2Z M184.8 37.7V55.2L188.9 56.2V57H175.9V56.2L180 55.2V38.1Q180 36.5 179.3 35.4Q178.7 34.3 177.5 33.7Q176.3 33.1 174.4 33.1Q172.5 33.1 170.7 33.7Q168.8 34.3 167.8 35.1L167.2 34.3Q169.1 32.7 170.5 31.8Q172 30.8 173.1 30.2Q174.3 29.6 175.4 29.4Q176.6 29.2 177.8 29.2Q181.5 29.2 183.1 31.3Q184.8 33.4 184.8 37.7Z";

export function Wordmark() {
  return (
    <svg className="wordmark" viewBox="0 0 198 73" role="img" aria-label="Addiem">
      <path d={WORDMARK_D} fill="var(--brand-ink)" />
      <circle cx="107.1" cy="19.9" r="6.1" fill="var(--brand-day)" />
    </svg>
  );
}

export function TopNav({ onImport, onNewDeck, onSettings, onHome, sync }: {
  onImport: () => void;
  onNewDeck: () => void;
  onSettings: () => void;
  onHome: () => void;
  /** K5 동기화 버튼 자리 (설정 버튼 바로 왼쪽). 연결하지 않았으면 비어 있다 */
  sync?: ReactNode;
}) {
  return (
    <header className="topnav">
      <button className="topnav__brand" onClick={onHome}>
        <Logo />
        <Wordmark />
      </button>
      <div className="topnav__actions">
        <Button variant="ghost" size="M" icon="upload" label="가져오기" onClick={onImport} />
        <Button variant="secondary" size="M" icon="plus" label="새 덱" onClick={onNewDeck} />
        {sync}
        <button className="topnav__settings" aria-label="설정" onClick={onSettings}>
          <Icon name="settings" size={18} className="c-secondary" />
        </button>
      </div>
    </header>
  );
}

export function Backdrop() {
  // 창이 가려지면(최소화 등) 배경 움직임을 멈춘다
  const [hidden, setHidden] = useState(document.hidden);
  // WebView2 는 최소화해도 visibilityState 가 'visible' 그대로라 창 크기 이벤트로 최소화를 따로 본다 (QA T6-02)
  const [minimized, setMinimized] = useState(false);
  useEffect(() => {
    const onChange = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", onChange);
    const win = getCurrentWindow();
    const unlisten = win.onResized(() => { win.isMinimized().then(setMinimized, () => {}); });
    return () => {
      document.removeEventListener("visibilitychange", onChange);
      unlisten.then((f) => f(), () => {});
    };
  }, []);
  return (
    <div className={hidden || minimized ? "backdrop is-paused" : "backdrop"} aria-hidden>
      <i /><i /><i /><i /><i />
    </div>
  );
}
