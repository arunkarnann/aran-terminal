// Per-project (per-folder) color assignment.
//
// Each project gets a deterministic default color derived from its name, so the
// same folder is always the same color across sessions and launches. The user can
// override any project's color; overrides are persisted in localStorage ("memory")
// and shared reactively across all components via a tiny subscribe store.

import { useEffect, useReducer } from "react";

const KEY = "conductor-project-colors";

// Dark-theme-friendly palette; hex so it works directly with <input type="color">.
const PALETTE = [
  "#7aa2f7",
  "#bb9af7",
  "#7dcfff",
  "#9ece6a",
  "#e0af68",
  "#f7768e",
  "#ff9e64",
  "#2ac3de",
  "#73daca",
  "#ff75a0",
  "#c0caf5",
  "#b4f9f8",
];

const NEUTRAL = "#6272a4";

type ColorMap = Record<string, string>;

function load(): ColorMap {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as ColorMap;
  } catch {
    return {};
  }
}

let overrides: ColorMap = load();
const listeners = new Set<() => void>();
function emit() {
  for (const fn of listeners) fn();
}

/** Stable color for a project name when the user hasn't picked one. */
export function defaultProjectColor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return PALETTE[h % PALETTE.length];
}

export function getProjectColor(name: string | null | undefined): string {
  if (!name) return NEUTRAL;
  return overrides[name] ?? defaultProjectColor(name);
}

export function setProjectColor(name: string, color: string) {
  overrides = { ...overrides, [name]: color };
  try {
    localStorage.setItem(KEY, JSON.stringify(overrides));
  } catch {
    /* ignore quota / private-mode errors */
  }
  emit();
}

/** Subscribe a component to color changes; returns get/set helpers. */
export function useProjectColors() {
  const [, force] = useReducer((x) => x + 1, 0);
  useEffect(() => {
    listeners.add(force);
    return () => {
      listeners.delete(force);
    };
  }, []);
  return { get: getProjectColor, set: setProjectColor };
}
