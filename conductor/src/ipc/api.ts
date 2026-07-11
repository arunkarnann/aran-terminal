// Typed wrappers over the Tauri boundary. UI code should import from here, never
// call `invoke`/`listen` with raw strings — that keeps the frozen contract honest
// and gives Agent C (dashboard) one place to point at a mock during Phase 4.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  EVENTS,
  type CapReachedEvent,
  type FocusBlock,
  type FocusDay,
  type FocusKind,
  type CommandHelp,
  type GitCommit,
  type GitRepo,
  type GitStatus,
  type HistoryEntry,
  type PtyOutputEvent,
  type SessionClosedEvent,
  type SessionId,
  type SessionMeta,
  type StateEventPayload,
  type Summary,
} from "./types";

// ---- Commands (frontend -> Rust) ----

export function createSession(opts?: {
  shell?: string;
  cwd?: string;
}): Promise<SessionId> {
  return invoke<SessionId>("create_session", {
    shell: opts?.shell ?? null,
    cwd: opts?.cwd ?? null,
  });
}

export function writeStdin(id: SessionId, data: string): Promise<void> {
  return invoke("write_stdin", { id, data });
}

export function resizePty(
  id: SessionId,
  cols: number,
  rows: number,
): Promise<void> {
  return invoke("resize_pty", { id, cols, rows });
}

export function closeSession(id: SessionId): Promise<void> {
  return invoke("close_session", { id });
}

export function listSessions(): Promise<SessionMeta[]> {
  return invoke<SessionMeta[]>("list_sessions");
}

/** Whole-app resident memory in KB (main process + WKWebView helpers), or null. */
export function getAppMem(): Promise<number | null> {
  return invoke<number | null>("get_app_mem");
}

/** TEMP diagnostic: send a message to the dev terminal's stdout. */
export function devLog(msg: string): void {
  void invoke("dev_log", { msg });
}

export function renameSession(id: SessionId, name: string): Promise<void> {
  return invoke("rename_session", { id, name });
}

export function setSessionCap(cap: number): Promise<void> {
  return invoke("set_session_cap", { cap });
}

export function setTaskLabel(id: SessionId, label: string): Promise<void> {
  return invoke("set_task_label", { id, label });
}

export function getWaitThreshold(): Promise<number> {
  return invoke<number>("get_wait_threshold");
}

export function setWaitThreshold(secs: number): Promise<void> {
  return invoke("set_wait_threshold", { secs });
}

export function getDailySummary(since: number, until: number): Promise<Summary> {
  return invoke<Summary>("get_daily_summary", { since, until });
}

export function resetStats(): Promise<void> {
  return invoke("reset_stats");
}

export function getCommandHistory(
  search?: string,
  limit?: number,
): Promise<HistoryEntry[]> {
  return invoke<HistoryEntry[]>("get_command_history", {
    search: search ?? null,
    limit: limit ?? null,
  });
}

export function getCommandSuggestions(
  prefix: string,
  cwd?: string | null,
  limit?: number,
): Promise<string[]> {
  return invoke<string[]>("get_command_suggestions", {
    prefix,
    cwd: cwd ?? null,
    limit: limit ?? null,
  });
}

// ---- Focus View (deep-work timers) ----

export function startFocusBlock(opts: {
  sessionId?: SessionId | null;
  plannedMs: number;
  taskLabel?: string | null;
  kind?: FocusKind;
}): Promise<FocusBlock> {
  return invoke<FocusBlock>("start_focus_block", {
    sessionId: opts.sessionId ?? null,
    plannedMs: opts.plannedMs,
    taskLabel: opts.taskLabel ?? null,
    kind: opts.kind ?? "focus",
  });
}

export function completeFocusBlock(id: string): Promise<void> {
  return invoke("complete_focus_block", { id });
}

export function abandonFocusBlock(id: string): Promise<void> {
  return invoke("abandon_focus_block", { id });
}

export function extendFocusBlock(id: string, addMs: number): Promise<void> {
  return invoke("extend_focus_block", { id, addMs });
}

export function getActiveFocusBlock(): Promise<FocusBlock | null> {
  return invoke<FocusBlock | null>("get_active_focus_block");
}

export function getFocusDay(dayStart: number): Promise<FocusDay> {
  return invoke<FocusDay>("get_focus_day", { dayStart });
}

export function getFocusGoal(): Promise<number> {
  return invoke<number>("get_focus_goal");
}

export function setFocusGoal(ms: number): Promise<void> {
  return invoke("set_focus_goal", { ms });
}

export function setNotificationFocus(
  sessionId: SessionId | null,
): Promise<void> {
  return invoke("set_notification_focus", { sessionId });
}

// ---- Git inspection (active session's working directory) ----

export function gitRepos(cwd: string): Promise<GitRepo[]> {
  return invoke<GitRepo[]>("git_repos", { cwd });
}

export function gitStatus(cwd: string): Promise<GitStatus> {
  return invoke<GitStatus>("git_status", { cwd });
}

export function gitLog(cwd: string, limit?: number): Promise<GitCommit[]> {
  return invoke<GitCommit[]>("git_log", { cwd, limit: limit ?? null });
}

export function gitShow(cwd: string, hash: string): Promise<string> {
  return invoke<string>("git_show", { cwd, hash });
}

export function gitDiff(cwd: string, path: string, staged: boolean): Promise<string> {
  return invoke<string>("git_diff", { cwd, path, staged });
}

export function gitStage(cwd: string, paths: string[]): Promise<void> {
  return invoke("git_stage", { cwd, paths });
}

export function gitUnstage(cwd: string, paths: string[]): Promise<void> {
  return invoke("git_unstage", { cwd, paths });
}

export function gitCommit(cwd: string, message: string, all: boolean): Promise<string> {
  return invoke<string>("git_commit", { cwd, message, all });
}

export function gitFetch(cwd: string): Promise<string> {
  return invoke<string>("git_fetch", { cwd });
}

export function gitPull(cwd: string): Promise<string> {
  return invoke<string>("git_pull", { cwd });
}

export function gitPush(cwd: string, setUpstream: boolean): Promise<string> {
  return invoke<string>("git_push", { cwd, setUpstream });
}

// ---- macOS permissions (issue #3: stop repeated TCC prompts) ----

/** True when the app currently has Full Disk Access. Never triggers a prompt. */
export function checkFullDiskAccess(): Promise<boolean> {
  return invoke<boolean>("check_full_disk_access");
}

/** Deep-link System Settings → Privacy & Security → Full Disk Access. */
export function openFullDiskAccessSettings(): Promise<void> {
  return invoke("open_full_disk_access_settings");
}

/**
 * Fire the Desktop/Documents/Downloads permission prompts up front (instead of
 * scattered mid-work). Resolves with the folder names currently readable.
 */
export function primeFolderPermissions(): Promise<string[]> {
  return invoke<string[]>("prime_folder_permissions");
}

// ---- Command --help flag explorer ----

export function commandHelp(cwd: string, command: string): Promise<CommandHelp> {
  return invoke<CommandHelp>("command_help", { cwd, command });
}

// ---- Events (Rust -> frontend) ----

export function onPtyOutput(
  cb: (e: PtyOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<PtyOutputEvent>(EVENTS.PTY_OUTPUT, (ev) => cb(ev.payload));
}

export function onSessionClosed(
  cb: (e: SessionClosedEvent) => void,
): Promise<UnlistenFn> {
  return listen<SessionClosedEvent>(EVENTS.SESSION_CLOSED, (ev) => cb(ev.payload));
}

export function onSessionState(
  cb: (e: StateEventPayload) => void,
): Promise<UnlistenFn> {
  return listen<StateEventPayload>(EVENTS.SESSION_STATE, (ev) => cb(ev.payload));
}

export function onCapReached(
  cb: (e: CapReachedEvent) => void,
): Promise<UnlistenFn> {
  return listen<CapReachedEvent>(EVENTS.CAP_REACHED, (ev) => cb(ev.payload));
}

/** Decode base64 PTY bytes into a Uint8Array for `term.write`. */
export function decodePtyBytes(base64Bytes: string): Uint8Array {
  const bin = atob(base64Bytes);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
