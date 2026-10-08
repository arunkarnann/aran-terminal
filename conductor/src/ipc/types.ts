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

export interface GitBranch {
  name: string;
  current: boolean;
  remote: boolean;
  upstream: string | null;
}

export interface GitRemote {
  name: string;
  url: string;
  webUrl: string | null;
}

export interface GitStash {
  index: number;
  message: string;
  relative: string;
  timestamp: number;
}

export interface GitOpResult {
  ok: boolean;
  output: string;
  conflicts: string[];
  inProgress: boolean;
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

// ---- Tasks sidebar ----

export type TaskStatus = "backlog" | "todo" | "in_progress" | "review" | "blocked" | "done";
/** 0 = urgent … 3 = low. */
export type TaskPriority = 0 | 1 | 2 | 3;

export interface Task {
  id: string;
  title: string;
  notes: string;
  status: TaskStatus;
  priority: TaskPriority;
  project: string | null;
  /** Epoch ms (local end-of-day). */
  dueAt: number | null;
  createdAt: number;
  updatedAt: number;
  /** Backend-owned: first entry into in_progress. */
  startedAt: number | null;
  /** Backend-owned: set on entering done, cleared on leaving it. */
  completedAt: number | null;
}

export interface TaskEvent {
  fromStatus: TaskStatus | null;
  toStatus: TaskStatus;
  at: number;
}

// ---- GitHub issues sidebar ----

export interface GhAccount {
  login: string;
  /** "gh" = token held by the GitHub CLI; "token" = pasted PAT in the Keychain. */
  via: "gh" | "token";
  scopes: string[] | null;
  canProjects: boolean;
}

export interface GhRepoInfo {
  fullName: string;
  url: string;
  private: boolean;
  description: string | null;
  pushedAt: number | null;
}

export interface GhProjectInfo {
  id: string;
  title: string;
  number: number;
  owner: string;
  url: string;
}

export interface GhProjectList {
  projects: GhProjectInfo[];
  notices: string[];
}

export type GhSourceKind = "repo" | "project";

export interface GhSourceInput {
  kind: GhSourceKind;
  /** "owner/name" for repos, node id for projects. */
  key: string;
  title: string;
  url: string | null;
}

export interface GhSource extends GhSourceInput {
  id: string;
  account: string;
  statusOptions: string[];
  syncedAt: number | null;
  error: string | null;
}

export interface GhLabel {
  name: string;
  /** Validated "#rrggbb" or null. */
  color: string | null;
}

export interface GhItem {
  sourceId: string;
  itemKey: string;
  kind: "issue" | "pr" | "draft";
  repo: string | null;
  number: number | null;
  title: string;
  state: string;
  url: string | null;
  author: string | null;
  assignees: string[];
  labels: GhLabel[];
  comments: number;
  updatedAt: number;
  status: string | null;
  fields: { name: string; value: string }[];
  /** Plain text — render as text, never HTML. */
  body: string;
  /** Epoch ms (0 for items cached before this field existed). */
  createdAt: number;
  /** Last title/body edit, if ever edited. */
  editedAt: number | null;
  /** Newest comment and its author. */
  lastCommentAt: number | null;
  lastCommentBy: string | null;
}
