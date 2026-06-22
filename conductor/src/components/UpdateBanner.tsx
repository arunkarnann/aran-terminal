// OTA update prompt. On launch it asks the GitHub Releases endpoint (see
// tauri.conf.json → plugins.updater) whether a newer signed bundle exists. If
// so it shows a banner; the user opts in, we download + verify + install, then
// relaunch into the new version.

import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type Phase = "idle" | "available" | "downloading" | "ready" | "error";

export function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState(0); // 0..1
  const [error, setError] = useState<string | null>(null);

  // Check once on mount. Failures are silent — a missing endpoint or offline
  // launch should never block the app.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const u = await check();
        if (!cancelled && u) {
          setUpdate(u);
          setPhase("available");
        }
      } catch {
        /* offline / no release / dev build — ignore */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const install = async () => {
    if (!update) return;
    setPhase("downloading");
    setError(null);
    let total = 0;
    let received = 0;
    try {
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? 0;
            break;
          case "Progress":
            received += event.data.chunkLength;
            if (total > 0) setProgress(received / total);
            break;
          case "Finished":
            setProgress(1);
            break;
        }
      });
      setPhase("ready");
      await relaunch();
    } catch (e) {
      setPhase("error");
      setError(String(e));
    }
  };

  if (phase === "idle" || !update) return null;

  return (
    <div className={`update-banner update-banner--${phase}`}>
      {phase === "available" && (
        <>
          <span className="update-banner-text">
            Update available — <strong>v{update.version}</strong>
          </span>
          <button className="update-banner-btn" onClick={() => void install()}>
            Update &amp; Restart
          </button>
          <button className="update-banner-dismiss" onClick={() => setPhase("idle")} title="Dismiss">
            ✕
          </button>
        </>
      )}
      {phase === "downloading" && (
        <span className="update-banner-text">
          Downloading update… {Math.round(progress * 100)}%
        </span>
      )}
      {phase === "ready" && <span className="update-banner-text">Restarting…</span>}
      {phase === "error" && (
        <>
          <span className="update-banner-text">Update failed: {error}</span>
          <button className="update-banner-btn" onClick={() => void install()}>
            Retry
          </button>
          <button className="update-banner-dismiss" onClick={() => setPhase("idle")} title="Dismiss">
            ✕
          </button>
        </>
      )}
    </div>
  );
}
