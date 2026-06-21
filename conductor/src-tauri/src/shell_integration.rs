//! Shell-integration injection (PRD §6.1 Signal 1, §12 "never break the user's prompt").
//!
//! We emit OSC 133 prompt/command markers by sourcing the user's real config first, then
//! appending hooks — never editing the user's dotfiles:
//!   - zsh:  a generated `ZDOTDIR` whose rc files source the real ones, then add hooks.
//!   - bash: `--rcfile` that sources `~/.bashrc`, then installs a DEBUG-trap + PROMPT_COMMAND.
//!   - fish: a generated `XDG_CONFIG_HOME` whose `config.fish` sources the real one + events.
//! Anything else falls back to degraded detection (heuristic + pattern), which the engine
//! handles (PRD §6.1 AC5).

use std::fs;
use std::path::PathBuf;

use portable_pty::CommandBuilder;

const ZSHENV: &str = r#"# Conductor shell integration (generated). Sources your real config; never replaces it.
[ -f "${__CONDUCTOR_REAL_ZDOTDIR:-$HOME}/.zshenv" ] && source "${__CONDUCTOR_REAL_ZDOTDIR:-$HOME}/.zshenv"
"#;

const ZSHRC: &str = r#"# Conductor shell integration (generated).
__cdr_real="${__CONDUCTOR_REAL_ZDOTDIR:-$HOME}"
[ -f "$__cdr_real/.zshrc" ] && source "$__cdr_real/.zshrc"
# Restore the user's real ZDOTDIR for the rest of the session.
export ZDOTDIR="$__cdr_real"

__conductor_precmd() {
  local ec=$?
  local __cdr_last="$(fc -ln -1 2>/dev/null)"
  [ -n "$__cdr_last" ] && print -rn -- $'\e]633;E;'"$(printf '%s' "$__cdr_last" | base64 | tr -d '\n')"$'\a'
  print -rn -- $'\e]133;D;'${ec}$'\a'
  print -rn -- $'\e]133;A\a'
  print -rn -- $'\e]7;file://'${HOST}${PWD}$'\a'
}
__conductor_preexec() {
  print -rn -- $'\e]633;E;'"$(printf '%s' "$1" | base64 | tr -d '\n')"$'\a'
  print -rn -- $'\e]133;C\a'
}

autoload -Uz add-zsh-hook 2>/dev/null
if (( $+functions[add-zsh-hook] )); then
  add-zsh-hook precmd __conductor_precmd
  add-zsh-hook preexec __conductor_preexec
fi
"#;

// Works on bash 3.2 (macOS) through 5.x. The DEBUG trap fires for EVERY simple command,
// including those inside PROMPT_COMMAND, so a naive "once per line" flag emits spurious C
// markers when the user already has a PROMPT_COMMAND. Fix: keep an `armed` flag that is set
// only at the very END of PROMPT_COMMAND (via `__conductor_arm`), so nothing run during the
// prompt can fire C — only the user's next real command can. One C per command line.
const BASHRC: &str = r#"# Conductor shell integration (generated). Sources your real config; never replaces it.
[ -f "$HOME/.bashrc" ] && source "$HOME/.bashrc"

__conductor_armed=""
__conductor_preexec() {
  [ -n "$COMP_LINE" ] && return        # skip tab-completion
  [ -z "$__conductor_armed" ] && return # nothing run during PROMPT_COMMAND counts
  __conductor_armed=""
  printf '\e]133;C\a'
}
__conductor_precmd() {
  local ec=$?
  local __cdr_last="$(history 1 2>/dev/null | sed 's/^[ ]*[0-9]*[ ]*//')"
  [ -n "$__cdr_last" ] && printf '\e]633;E;%s\a' "$(printf '%s' "$__cdr_last" | base64 | tr -d '\n')"
  printf '\e]133;D;%s\a\e]133;A\a\e]7;file://%s%s\a' "$ec" "$HOSTNAME" "$PWD"
}
__conductor_arm() { __conductor_armed=1; }
trap '__conductor_preexec' DEBUG
case "$PROMPT_COMMAND" in
  *__conductor_precmd*) ;;
  *) PROMPT_COMMAND="__conductor_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __conductor_arm" ;;
esac
"#;

fn dir() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("conductor-shell-integration");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Wire OSC 133 integration into the spawn command for supported shells.
pub fn apply(cmd: &mut CommandBuilder, shell_path: &str) {
    let base = shell_path.rsplit('/').next().unwrap_or("");
    let Some(dir) = dir() else { return };

    match base {
        "zsh" => {
            if fs::write(dir.join(".zshenv"), ZSHENV).is_ok()
                && fs::write(dir.join(".zshrc"), ZSHRC).is_ok()
            {
                let real = std::env::var("ZDOTDIR")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_default();
                cmd.env("__CONDUCTOR_REAL_ZDOTDIR", real);
                cmd.env("ZDOTDIR", dir.to_string_lossy().to_string());
            }
        }
        "bash" => {
            let rc = dir.join("conductor.bashrc");
            if fs::write(&rc, BASHRC).is_ok() {
                cmd.arg("--rcfile");
                cmd.arg(rc.to_string_lossy().to_string());
            }
        }
        // fish 3.4+ emits OSC 133 natively, so we deliberately do NOT inject (double-firing
        // would corrupt command counts). Older fish falls back to degraded detection.
        _ => { /* degraded mode: heuristic + pattern only */ }
    }
}
