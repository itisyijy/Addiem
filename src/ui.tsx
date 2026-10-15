/*
 * Figma "Components" 프레임의 컴포넌트를 옮긴 것. 이름과 variant 는 Figma 와 맞춘다.
 * 스타일은 styles/components.css.
 */
import { useEffect, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import backdropSvg from "./assets/backdrop.svg";

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
  | "sparkles" | "gauge" | "wifi-off" | "send";

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

export function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button role="switch" aria-checked={on} aria-label={label} className="toggle" onClick={() => onChange(!on)}>
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

export function Logo() {
  return (
    <span className="logo" aria-hidden>
      <span className="logo__back" />
      <span className="logo__front" />
    </span>
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
        <span className="t-title-s">Neo-Anki</span>
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
  return <img className="backdrop" src={backdropSvg} width={1280} height={800} alt="" aria-hidden />;
}
