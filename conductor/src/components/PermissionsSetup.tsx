import { useEffect, useState } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  checkFullDiskAccess,
  openFullDiskAccessSettings,
  primeFolderPermissions,
} from "../ipc/api";

// Onboarding for macOS file permissions (issue #3). A terminal spawns arbitrary
// child processes, so macOS attributes their file access to this app — without a
// broad up-front grant, every dropped photo/file triggers its own TCC dialog.
// Two paths, best first:
//   1. Full Disk Access — one switch in System Settings, covers everything.
//   2. Folder priming  — answer the Desktop/Documents/Downloads prompts now,
//      back-to-back, instead of scattered mid-work.

const PRIMED_FOLDERS = ["Desktop", "Documents", "Downloads"] as const;

interface PermissionsSetupProps {
  /** Rendered inside Settings (no intro copy tuned for first launch). */
  embedded?: boolean;
}

export function PermissionsSetup({ embedded }: PermissionsSetupProps) {
  const [fda, setFda] = useState<boolean | null>(null);
  const [granted, setGranted] = useState<Set<string>>(new Set());
  const [priming, setPriming] = useState(false);
  const [settingsOpened, setSettingsOpened] = useState(false);

  // Poll FDA status while visible so the ✓ flips as soon as the grant lands.
  useEffect(() => {
    let active = true;
    const poll = () => {
      checkFullDiskAccess()
        .then((ok) => {
          if (active) setFda(ok);
        })
        .catch(() => {
          if (active) setFda(false);
        });
    };
    poll();
    const t = setInterval(poll, 2000);
    return () => {
      active = false;
      clearInterval(t);
    };
  }, []);

  const prime = () => {
    setPriming(true);
    primeFolderPermissions()
      .then((ok) => setGranted(new Set(ok)))
      .catch(() => {})
      .finally(() => setPriming(false));
  };

  return (
    <div className="perms-setup">
      {!embedded && (
        <p className="dialog-body">
          macOS asks permission for every file dragged into the terminal — one
          dialog at a time. Grant access once here and those prompts go away for
          every tool you run inside Aran Terminal.
        </p>
      )}

      <div className="setting-section-label">
        Option 1 — Full Disk Access (recommended)
      </div>
      <p className="dialog-body">
        One switch covers Desktop, Documents, Downloads, Photos, external drives —
        everything, for the app and all terminals inside it.{" "}
        {fda ? (
          <span className="font-hint font-hint--ok">✓ Granted.</span>
        ) : (
          <>Find “Aran Terminal” in the list and turn it on.</>
        )}
      </p>
      {!fda && (
        <div className="dialog-actions dialog-actions--start">
          <button
            className="btn btn-primary"
            onClick={() => {
              setSettingsOpened(true);
              void openFullDiskAccessSettings().catch(() => {});
            }}
          >
            Open System Settings…
          </button>
          {settingsOpened && (
            <button className="btn" onClick={() => void relaunch().catch(() => {})}>
              Relaunch app
            </button>
          )}
        </div>
      )}
      {!fda && settingsOpened && (
        <p className="dialog-body setting-muted">
          macOS applies Full Disk Access after the app restarts — relaunch once
          you've flipped the switch.
        </p>
      )}

      <div className="setting-section-label">Option 2 — Just the common folders</div>
      <p className="dialog-body">
        Prefer something narrower? Answer the three folder prompts now instead of
        being interrupted later.
      </p>
      <div className="dialog-actions dialog-actions--start">
        <button className="btn" onClick={prime} disabled={priming || fda === true}>
          {priming ? "Requesting…" : "Grant folder access"}
        </button>
        <span className="setting-muted">
          {PRIMED_FOLDERS.map((f) => (
            <span key={f} style={{ marginRight: "0.75em" }}>
              {granted.has(f) || fda ? "✓" : "·"} {f}
            </span>
          ))}
        </span>
      </div>
    </div>
  );
}
