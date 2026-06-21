// Flag palette for the command typed at the prompt. Runs `<command> --help` in
// the backend (see src-tauri/src/cmdhelp.rs), parses the options, and shows them
// searchable. Clicking a flag inserts it into the terminal.

import { useEffect, useMemo, useRef, useState } from "react";
import { commandHelp } from "../ipc/api";
import type { CommandHelp as CommandHelpData } from "../ipc/types";

interface CommandHelpProps {
  cwd: string | null;
  command: string;
  onClose: () => void;
  onPick: (flagToken: string) => void;
}

/** The token to actually insert: prefer the long `--flag`, else the first `-x`. */
function insertToken(flags: string): string {
  const tokens = flags.split(/[\s,]+/).filter(Boolean);
  return tokens.find((t) => t.startsWith("--")) ?? tokens.find((t) => t.startsWith("-")) ?? "";
}

export function CommandHelp({ cwd, command, onClose, onPick }: CommandHelpProps) {
  const [data, setData] = useState<CommandHelpData | null>(null);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [showRaw, setShowRaw] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setData(null);
    const cmd = command.trim();
    if (!cmd) {
      setLoading(false);
      return;
    }
    commandHelp(cwd ?? "", cmd)
      .then((d) => active && setData(d))
      .catch((e) => active && setData({ ok: false, command: cmd, flags: [], raw: "", error: String(e) }))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [cwd, command]);

  useEffect(() => {
    setTimeout(() => searchRef.current?.focus(), 0);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const flags = data?.flags ?? [];
    if (!q) return flags;
    return flags.filter(
      (f) =>
        f.flags.toLowerCase().includes(q) || f.description.toLowerCase().includes(q),
    );
  }, [data, query]);

  const title = command.trim() || "command";

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog ch-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="ch-head">
          <h2 className="dialog-title">
            <span className="ch-cmd">{title}</span> <span className="ch-dim">--help</span>
          </h2>
          {data?.flags && data.flags.length > 0 && (
            <button className="ch-rawbtn" onClick={() => setShowRaw((v) => !v)}>
              {showRaw ? "flags" : "raw"}
            </button>
          )}
        </div>

        {!command.trim() ? (
          <div className="ch-msg">Type a command at the prompt first, then open this.</div>
        ) : loading ? (
          <div className="ch-msg">Running {title} --help…</div>
        ) : data && !data.ok ? (
          <div className="ch-msg ch-msg--err">
            {data.error ?? "Couldn't get help for this command."}
          </div>
        ) : showRaw ? (
          <pre className="ch-raw">{data?.raw}</pre>
        ) : (
          <>
            <input
              ref={searchRef}
              className="ch-search"
              placeholder="Filter flags…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div className="ch-list">
              {filtered.length === 0 ? (
                <div className="ch-msg">
                  {data?.flags.length ? "No matching flags." : "No flags found in --help."}
                </div>
              ) : (
                filtered.map((f, i) => {
                  const token = insertToken(f.flags);
                  return (
                    <button
                      key={`${f.flags}:${i}`}
                      className="ch-flag"
                      onClick={() => token && onPick(token)}
                      title={token ? `Insert ${token}` : f.flags}
                    >
                      <span className="ch-flag-name">{f.flags}</span>
                      {f.description && <span className="ch-flag-desc">{f.description}</span>}
                    </button>
                  );
                })
              )}
            </div>
          </>
        )}

        <div className="ch-foot">
          {data?.ok && !showRaw
            ? `${data.flags.length} flags · click to insert · esc to close`
            : "esc to close"}
        </div>
      </div>
    </div>
  );
}
