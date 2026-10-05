import { useEffect, useState, type RefObject } from "react";

/** How much room each tab gets — drives which details a tab can afford to show.
 *  Done in JS (not container queries) because we still support macOS 10.15. */
export type TabDensity = "full" | "compact" | "tight" | "mini";

/** Width-per-tab thresholds (px). Below these, tabs progressively drop
 *  uptime/memory/reveal, then the swatch, then the label. */
const FULL_MIN = 265;
const COMPACT_MIN = 130;
const TIGHT_MIN = 70;

/** Observe an element's content width. */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) setWidth(e.contentRect.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return width;
}

/** Observe the window width (the top bar always spans it). */
export function useWindowWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return width;
}

/** Density for a strip of `count` tabs sharing `stripWidth` px (minus the "+" button). */
export function tabDensity(stripWidth: number, count: number): TabDensity {
  if (count === 0 || stripWidth === 0) return "full";
  const per = (stripWidth - 40) / count;
  if (per >= FULL_MIN) return "full";
  if (per >= COMPACT_MIN) return "compact";
  if (per >= TIGHT_MIN) return "tight";
  return "mini";
}
