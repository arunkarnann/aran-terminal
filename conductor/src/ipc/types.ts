// FROZEN IPC CONTRACT (Phase 0.5) — TS mirror of `src-tauri/src/ipc.rs`.
// Keep in lockstep with the Rust side. Changing this is a blocking event for all
// agents (IMPLEMENTATION-PLAN.md §5).

export type SessionId = string;

export type AttentionState = "RUNNING" | "IDLE" | "WAITING";
export type StateSource = "osc133" | "heuristic" | "pattern";

export interface SessionMeta {
  id: SessionId;
  name: string | null;
  project: string | null;
  cwd: string | null;
  taskLabel: string | null;
  shell: string;
  createdAt: number;
  openCount: number;
  commandCount: number;
  state: AttentionState;
  rssKb: number | null;
  lastCommand: string | null;
}

// Event channel names (Rust -> frontend).
export const EVENTS = {
  PTY_OUTPUT: "pty://output",
  SESSION_STATE: "session://state",
  SESSION_META: "session://meta",
  SESSION_CLOSED: "session://closed",
  CAP_REACHED: "cap://reached",
} as const;

export interface PtyOutputEvent {
  id: SessionId;
  base64Bytes: string;
}

export interface SessionClosedEvent {
  id: SessionId;
  exitCode: number | null;
}

export interface StateEventPayload {
  id: SessionId;
  state: AttentionState;
  source: StateSource;
  at: number;
}

export interface CapReachedEvent {
  limit: number;
  current: number;
}

export interface ProjectTime {
  project: string;
  activeMs: number;
}

export type FocusKind = "focus" | "break";
export type FocusStatus = "active" | "completed" | "abandoned";

export interface FocusBlock {
  id: string;
  sessionId: SessionId | null;
  taskLabel: string | null;
  kind: FocusKind;
  startedAt: number;
  plannedMs: number;
  endedAt: number | null;
  status: FocusStatus;
}

export interface FocusDay {
  date: number;
  focusMs: number;
  blocksCompleted: number;
  goalMs: number;
  streakDays: number;
}

export interface HistoryEntry {
  cmdline: string;
  project: string | null;
  cwd: string | null;
  finishedAt: number | null;
  durationMs: number | null;
  exitCode: number | null;
}

// ---- Session restore (PRD §8.1) ----

export interface SessionSnapshot {
  sessionId: SessionId;
  name: string | null;
  projectPath: string | null;
  projectName: string | null;
  taskLabel: string | null;
  shell: string;
  cwd: string | null;
  tabOrder: number;
  isActive: boolean;
  updatedAt: number;
}

export interface SessionSnapshotWithScrollback {
  snapshot: SessionSnapshot;
  scrollbackBase64: string | null;
}

export interface Summary {
  since: number;
  until: number;
  sessionsOpened: number;
  commandsRun: number;
  agentBlockedMs: number;
  capOverrides: number;
  perProject: ProjectTime[];
}

// ---- Git inspection (active session's working directory) ----

export interface GitRepo {
  path: string;
  name: string;
}

export interface GitFileChange {
  path: string;
  /** Single-letter git status (M, A, D, R, C, U…). */
  code: string;
  insertions: number;
  deletions: number;
}

export interface GitStatus {
  isRepo: boolean;
  branch: string | null;
  ahead: number;
  behind: number;
  staged: GitFileChange[];
  unstaged: GitFileChange[];
  untracked: string[];
}

export interface GitCommit {
  hash: string;
  short: string;
  subject: string;
  author: string;
  relative: string;
  timestamp: number;
}

// ---- Command --help flag explorer ----

export interface HelpFlag {
  /** Option tokens, e.g. "-h, --help" or "--model <MODEL>". */
  flags: string;
  description: string;
}

export interface CommandHelp {
  ok: boolean;
  command: string;
  flags: HelpFlag[];
  raw: string;
  error: string | null;
}
