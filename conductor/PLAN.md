# Implementation Plan — Git panel write actions

Scoped with exact files and approach. Intended to be handed to an implementation agent.

---

## Git panel write actions (commit / push / fetch, VS Code–style)
**Goal:** turn the read-only Git panel into a basic source-control surface — stage /
unstage files, write a commit message and commit, and fetch / pull / push — like the
VS Code or Zed git pane.

### Current state
`src-tauri/src/git.rs` is **read-only** (`repos`, `status`, `log`, `show`, `diff_file`)
and its `run()` helper returns `Option<String>` — it **swallows stderr** and returns
`None` on failure. The frontend `GitPanel.tsx` ("Changes" tab) only *opens diffs*; there
are no stage/commit/sync controls. `status()` already returns `branch`, `ahead`,
`behind`, `staged`, `unstaged`, `untracked`, so the header has the data it needs to gate
buttons.

### Backend — `src-tauri/src/git.rs`
- Add a fallible runner that **captures stderr** so the UI can show real git errors
  (e.g. "push rejected", "no upstream"):
  ```rust
  fn run_result(cwd: &str, args: &[&str]) -> Result<String, String> {
      let out = Command::new("git").arg("-C").arg(cwd).args(args)
          // Fail fast instead of hanging on a credential/passphrase prompt (no TTY).
          .env("GIT_TERMINAL_PROMPT", "0")
          .output().map_err(|e| e.to_string())?;
      if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into()) }
      else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
  }
  ```
- New functions (all keep the existing **argv discipline** — never a shell; pass file
  paths after a `--` separator; pass the commit message as a single `-m <msg>` arg):
  - `stage(cwd, paths: &[String])` → `git add -- <paths>`
  - `unstage(cwd, paths: &[String])` → `git reset -q HEAD -- <paths>`
  - `commit(cwd, message, all: bool)` → `git commit [-a] -m <message>`; reject an empty
    message before shelling out.
  - `fetch(cwd)` → `git fetch --all --prune`
  - `pull(cwd)` → `git pull --ff-only` (surface a clear error if it can't fast-forward)
  - `push(cwd, set_upstream: bool)` → `git push`; when there's no upstream, retry with
    `push -u origin HEAD` (or expose `set_upstream` so the UI offers a "Publish branch").
  - (optional) `discard(cwd, paths)` → `git restore -- <paths>` / `git checkout -- …`.

### Commands / IPC plumbing
- Add `#[tauri::command]` wrappers in `src-tauri/src/commands.rs`: `git_stage`,
  `git_unstage`, `git_commit`, `git_fetch`, `git_pull`, `git_push`. Each returns
  `Result<(), String>` (or `Result<String, String>` to bubble a success summary).
- **Make the network commands async** (`#[tauri::command] async fn`) and run the
  blocking git call on a worker (e.g. `tauri::async_runtime::spawn_blocking`) so a slow
  fetch/push doesn't freeze the WebView. Consider a wall-clock timeout (kill the child
  after ~30s) so a hung auth attempt can't wedge the panel.
- Register all six in the `lib.rs` `invoke_handler!` list.
- Add typed wrappers in `src/ipc/api.ts` and any shared types in `src/ipc/types.ts`.

### Frontend — `src/components/GitPanel.tsx`
- **Sync bar** in `git-head` (next to branch/ahead-behind, ~line 145–152): three icon
  buttons — **Fetch**, **Pull** (badge `↓behind`, disabled when `behind === 0`),
  **Push** (badge `↑ahead`, disabled when `ahead === 0`). When `status.branch` has no
  upstream, show **Publish** (calls push with `set_upstream`). Show a spinner + disable
  while a sync runs; surface errors in a small `git-status-line`.
- **Stage controls** in the "Changes" tab: a stage (`+`) action on each Unstaged /
  Untracked row, an unstage (`−`) action on each Staged row, plus "Stage all" /
  "Unstage all" group headers. (Keep the existing click-to-open-diff; put the
  stage/unstage control as a separate button so the two don't collide.)
- **Commit box** below the Changes list: a `<textarea>` for the message + a **Commit**
  button (disabled when message is empty or `staged.length === 0`). Optional "Commit all"
  toggle → `commit(all: true)`. On success, clear the box and `refresh()`.
- After **every** mutating action call the existing `refresh()` so status / log / ahead /
  behind update immediately (don't wait for the 5s interval).

### Gotchas
- **Auth/credential hangs** are the main risk: HTTPS or an SSH passphrase will prompt on
  a TTY that doesn't exist here. `GIT_TERMINAL_PROMPT=0` + the timeout make it fail fast
  with a readable error instead of hanging. Document that credential-helper / ssh-agent
  setups are expected (we don't prompt for passwords in-app).
- **Untracked vs unstaged**: untracked files come from `status.untracked` (strings), not
  `GitFileChange` — staging them is still `git add -- <path>`, but the UI list is
  separate (see `GitPanel.tsx:221`).
- Renames in porcelain are tracked as the *new* path (already handled in `status()`),
  so stage/unstage by that path works.

### Decision to confirm before starting
- Commit default: commit staged only (standard, recommended) vs. expose a "Commit all"
  that stages tracked changes too.
