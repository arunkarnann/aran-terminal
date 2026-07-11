import { useEffect, useMemo, useState } from "react";
import { getWaitThreshold, setWaitThreshold } from "../ipc/api";
import { PermissionsSetup } from "./PermissionsSetup";
import { useTheme } from "../themes/useTheme";
import { buildStack, isFontAvailable, primaryFamily } from "../lib/fonts";

const MIN_FONT_SIZE = 8;
const MAX_FONT_SIZE = 32;
const DEFAULT_FONT_SIZE = 13;

// Candidate fonts for the dropdown. The first group ships with macOS (so it's
// always available); the rest are popular dev fonts shown only if installed.
// Every candidate is rendered, with an "· not installed" tag on missing ones, so
// the list always shows plenty of choices and is honest about which work.
const FONT_CANDIDATES = [
  // macOS built-ins (reliably present)
  "Menlo",
  "Monaco",
  "SF Mono",
  "Andale Mono",
  "PT Mono",
  "Courier New",
  "Courier",
  // Popular programming fonts (if the user installed them)
  "Fira Code",
  "JetBrains Mono",
  "Cascadia Code",
  "Source Code Pro",
  "Hack",
  "IBM Plex Mono",
  "Roboto Mono",
  "Ubuntu Mono",
  "Iosevka",
  "MesloLGS NF",
  "Inconsolata",
];

interface SettingsProps {
  cap: number;
  fontFamily: string;
  fontSize: number;
  onApplyCap: (cap: number) => void;
  onFontChange: (family: string, size: number) => void;
  onSave: (family: string, size: number) => void;
  onClose: () => void;
}

function themeBg(colors: { bg: string }): string {
  return colors.bg;
}

export function Settings({ cap, fontFamily, fontSize, onApplyCap, onFontChange, onSave, onClose }: SettingsProps) {
  const [draftCap, setDraftCap] = useState(cap);
  const [waitSecs, setWaitSecs] = useState(3);
  const { themeName, setTheme, availableThemes } = useTheme();

  // The font field edits a single family name; the stored value is a full stack.
  const fontName = useMemo(() => primaryFamily(fontFamily), [fontFamily]);
  const [fontAvail, setFontAvail] = useState<boolean | null>(null);
  // Names from FONT_CANDIDATES that are actually installed on this machine.
  const [installed, setInstalled] = useState<Set<string>>(new Set());

  // The selectable list: all candidates, plus the current font if it's custom.
  const fontOptions = useMemo(() => {
    const list = [...FONT_CANDIDATES];
    if (fontName && !list.includes(fontName)) list.unshift(fontName);
    return list;
  }, [fontName]);

  useEffect(() => {
    getWaitThreshold()
      .then(setWaitSecs)
      .catch(() => {});
  }, []);

  // Detect which candidate fonts are installed (once, on open).
  useEffect(() => {
    let active = true;
    Promise.all(
      FONT_CANDIDATES.map(async (n) => [n, await isFontAvailable(n)] as const),
    ).then((pairs) => {
      if (active) setInstalled(new Set(pairs.filter(([, ok]) => ok).map(([n]) => n)));
    });
    return () => {
      active = false;
    };
  }, []);

  // Live "is this font installed?" check for the current selection.
  useEffect(() => {
    let active = true;
    if (!fontName) {
      setFontAvail(null);
      return;
    }
    isFontAvailable(fontName).then((ok) => {
      if (active) setFontAvail(ok);
    });
    return () => {
      active = false;
    };
  }, [fontName]);

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog dialog--wide" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">Settings</h2>

        <div className="setting-row">
          <label htmlFor="cap">Max concurrent sessions</label>
          <input
            id="cap"
            type="number"
            min={1}
            max={50}
            value={draftCap}
            onChange={(e) => setDraftCap(Math.max(1, Number(e.target.value) || 1))}
          />
        </div>
        <p className="dialog-body">
          The focus cap blocks opening more terminals than this. Raising it during a
          block is counted as an override in the daily summary.
        </p>

        <div className="setting-row">
          <label htmlFor="wait">Waiting threshold (seconds)</label>
          <input
            id="wait"
            type="number"
            min={1}
            max={120}
            value={waitSecs}
            onChange={(e) => setWaitSecs(Math.max(1, Number(e.target.value) || 1))}
          />
        </div>
        <p className="dialog-body">
          Once a session prints a prompt (e.g. a “(y/n)” question), how long it stays
          quiet before it's flagged as needing you. Lower = more responsive; higher =
          fewer false alarms.
        </p>

        <div className="setting-section-label">Terminal Font</div>

        <div className="setting-row">
          <label htmlFor="font-family">Font family</label>
          <select
            id="font-family"
            className="theme-select"
            value={fontName}
            onChange={(e) => onFontChange(buildStack(e.target.value), fontSize)}
          >
            {fontOptions.map((name) => {
              const missing = installed.size > 0 && !installed.has(name);
              return (
                <option key={name} value={name} style={{ fontFamily: buildStack(name) }}>
                  {name}
                  {missing ? " · not installed" : ""}
                </option>
              );
            })}
          </select>
        </div>

        <div className="setting-row">
          <label htmlFor="font-custom">Or type any font</label>
          <input
            id="font-custom"
            className="theme-select"
            type="text"
            placeholder="Exact family name from Font Book…"
            value={fontName}
            onChange={(e) => onFontChange(buildStack(e.target.value), fontSize)}
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
          />
        </div>

        <p className="dialog-body">
          Pick from the list, or type any font installed on your Mac.{" "}
          {fontName &&
            (fontAvail === false ? (
              <span className="font-hint font-hint--warn">
                ⚠ “{fontName}” isn't installed under that name — check Font Book for
                the exact family name.
              </span>
            ) : fontAvail ? (
              <span className="font-hint font-hint--ok">✓ “{fontName}” is installed.</span>
            ) : null)}
        </p>

        <div className="setting-row">
          <label htmlFor="font-size">Font size</label>
          <input
            id="font-size"
            type="number"
            min={MIN_FONT_SIZE}
            max={MAX_FONT_SIZE}
            value={fontSize}
            onChange={(e) => {
              const v = Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, Number(e.target.value) || DEFAULT_FONT_SIZE));
              onFontChange(fontFamily, v);
            }}
          />
        </div>

        <p className="dialog-body">
          <kbd>⌘=</kbd> / <kbd>⌘-</kbd> on macOS, <kbd>Ctrl+=</kbd> / <kbd>Ctrl+-</kbd> on Linux to zoom the entire UI. <kbd>⌘0</kbd> / <kbd>Ctrl+0</kbd> resets.
        </p>

        <div className="setting-section-label">Theme</div>
        <div className="theme-grid">
          {availableThemes.map((t) => {
            const bg = themeBg(t.colors);
            const active = t.name === themeName;
            return (
              <button
                key={t.name}
                className={`theme-card ${active ? "theme-card--active" : ""}`}
                onClick={() => setTheme(t.name)}
                title={t.name}
              >
                <span
                  className="theme-swatch"
                    style={{ background: bg }}
                />
                <span className="theme-swatch-name">{t.name}</span>
              </button>
            );
          })}
        </div>
        <p className="dialog-body">
          Themes are JSON files in <code>src/themes/</code>. Drop a new{" "}
          <code>.json</code> file matching the schema to add your own.
        </p>

        <div className="setting-section-label">macOS Permissions</div>
        <PermissionsSetup embedded />

        <div className="setting-row setting-row--static">
          <span>Shell integration</span>
          <span className="setting-muted">zsh · bash · fish (others: degraded)</span>
        </div>

        <div className="dialog-actions">
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => {
              if (draftCap !== cap) onApplyCap(draftCap);
              void setWaitThreshold(waitSecs);
              onSave(fontFamily, fontSize);
            }}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
