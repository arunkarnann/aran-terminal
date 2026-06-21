import type { AttentionState } from "../ipc/types";

export const STATE_META: Record<
  AttentionState,
  { color: string; label: string }
> = {
  RUNNING: { color: "var(--running)", label: "Running" },
  IDLE: { color: "var(--idle)", label: "Idle" },
  WAITING: { color: "var(--waiting)", label: "Waiting for you" },
};

/** "1h 4m", "3m 12s", "8s" — compact, at most two units. */
export function formatDuration(ms: number): string {
  if (ms < 0) ms = 0;
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${sec}s`;
  return `${sec}s`;
}

/** RSS in KB → "142 MB", "1.2 GB", "—". */
export function formatMem(kb: number | null | undefined): string {
  if (kb == null) return "—";
  const mb = kb / 1024;
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

/** "25:00", "04:09", "1:02:30" — countdown/stopwatch clock, never negative. */
export function formatCountdown(ms: number): string {
  if (ms < 0) ms = 0;
  const total = Math.ceil(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/** Whole minutes, "90m" / "1h 30m" style for goals/totals. */
export function formatMinutes(ms: number): string {
  const m = Math.round(ms / 60000);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  return rem === 0 ? `${h}h` : `${h}h ${rem}m`;
}

/** How long a session has been open, escalating with age:
 *  fresh (<30m) → stale (≥30m) → old (≥4h, "full red"). */
export type AgeTier = "fresh" | "stale" | "old";
export const AGE_STALE_MS = 30 * 60_000; // 30 minutes
export const AGE_OLD_MS = 4 * 60 * 60_000; // 4 hours

export function ageTier(uptimeMs: number): AgeTier {
  if (uptimeMs >= AGE_OLD_MS) return "old";
  if (uptimeMs >= AGE_STALE_MS) return "stale";
  return "fresh";
}

/** Local-midnight epoch ms for the day containing `now` (drives the focus-day window). */
export function startOfDay(now: number): number {
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "just now", "12s ago", "4m ago". */
export function formatRelative(fromMs: number, nowMs: number): string {
  const d = nowMs - fromMs;
  if (d < 3000) return "just now";
  return `${formatDuration(d)} ago`;
}
