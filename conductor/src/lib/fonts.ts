// Font-family helpers shared by Settings (the picker) and TerminalView (the
// renderer). The app stores a full CSS font stack string (e.g.
// `'Fira Code', Menlo, Monaco, monospace`); the user only cares about the first
// name, so we extract / rebuild around that.

const FALLBACKS = "Menlo, Monaco, 'Courier New', monospace";

/** First family in a CSS stack, with surrounding quotes stripped. */
export function primaryFamily(stack: string): string {
  const first = (stack.split(",")[0] ?? "").trim();
  return first.replace(/^['"]|['"]$/g, "");
}

/** Build a CSS stack from a single family name, quoting it and appending the
 *  monospace fallbacks so a typo never leaves the terminal with no font. */
export function buildStack(name: string): string {
  const n = name.trim();
  if (!n) return FALLBACKS;
  const quoted = /\s/.test(n) ? `'${n}'` : n;
  return `${quoted}, ${FALLBACKS}`;
}

/**
 * CSS font-spec string ready for document.fonts.load / check.
 * Font names containing spaces are quoted so the CSS parser treats them
 * as a single family token — without quotes "13px JetBrains Mono"
 * is ambiguous and produce different results across engines.
 */
export function fontProbe(size: number, family: string): string {
  const name = family.trim();
  if (!name) return "";
  const quoted = /\s/.test(name) ? `'${name}'` : name;
  return `${size}px ${quoted}`;
}

// ---- font availability detection -------------------------------------------

/**
 * Canvas-based font-availability check.  Renders a distinctive test
 * character at a fixed size with the target family and compares the
 * measured width against the same text rendered with the generic
 * "monospace" fallback.  If the browser substituted a different
 * (wider/narrower) glyph the widths diverge, which means the named
 * family was actually used.  This works even when the Font Loading API
 * is unavailable (WKWebView quirk) or when system fonts aren't tracked
 * by FontFaceSet.
 */
function canvasFontAvailable(family: string): boolean {
  try {
    const canvas = document.createElement("canvas");
    canvas.width = 64;
    canvas.height = 32;
    const ctx = canvas.getContext("2d");
    if (!ctx) return false;
    const testChar = "\u2502"; // U+2502 BOX DRAWINGS LIGHT VERTICAL — distinctive per face
    const probe = `16px '${family}', monospace`;
    const fallback = "16px monospace";
    // For font names with embedded spaces, the CSS value must already be
    // quoted; we wrap in double-quotes for the inner family token.
    ctx.font = probe;
    const w = ctx.measureText(testChar).width;
    ctx.font = fallback;
    const wMono = ctx.measureText(testChar).width;
    return Math.abs(w - wMono) > 0.5; // small epsilon to ignore rounding
  } catch {
    return false;
  }
}

/** Whether the named family is installed/loadable in this WebView. Resolves
 *  after asking the Font Loading API to load it (so freshly-installed system
 *  fonts report correctly instead of "not found" before first use).
 *  Falls back to a Canvas-measurement heuristic when the API is unavailable
 *  or the probe cannot be resolved synchronously. */
export async function isFontAvailable(name: string): Promise<boolean> {
  const n = name.trim();
  if (!n) return false;
  const probe = fontProbe(16, n);
  if (!probe) return false;

  // 1. Quick synchronous check via FontFaceSet (most engines).
  try {
    if (document.fonts && typeof document.fonts.check === "function") {
      if (document.fonts.check(probe)) return true;
    }
  } catch {
    /* API may be present but throw on invalid probes */
  }

  // 2. Async load — triggers font discovery; on some platforms (WebKit)
  //    system fonts only appear in FontFaceSet after first use.
  try {
    if (document.fonts && typeof document.fonts.load === "function") {
      await document.fonts.load(probe);
      if (typeof document.fonts.check === "function" && document.fonts.check(probe)) {
        return true;
      }
    }
  } catch {
    /* load/check failed — system fonts not tracked by FontFaceSet (common in WKWebView) */
  }

  // 3. Canvas heuristic — the most reliable cross-engine fallback.
  return canvasFontAvailable(n);
}
