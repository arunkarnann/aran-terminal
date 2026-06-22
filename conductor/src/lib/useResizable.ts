import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Hook for resizable side panels. Returns [width, handleProps] where handleProps
 * should be spread on a thin drag-handle element on the panel's inner edge.
 *
 * @param side - which edge the handle sits on ("left" or "right")
 * @param storageKey - localStorage key for persistence
 * @param defaultWidth - initial width if nothing is persisted
 * @param min - minimum width in px
 * @param max - maximum width in px
 */
export function useResizable(
  side: "left" | "right",
  storageKey: string,
  defaultWidth: number,
  min: number,
  max: number,
): [number, React.HTMLAttributes<HTMLDivElement>] {
  const [width, setWidth] = useState(() => {
    try {
      const stored = localStorage.getItem(storageKey);
      if (stored) {
        const n = Number(stored);
        if (n >= min && n <= max) return n;
      }
    } catch {
      /* ignore */
    }
    return defaultWidth;
  });

  const widthRef = useRef(width);
  widthRef.current = width;

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      const startX = e.clientX;
      const startWidth = widthRef.current;

      const onMove = (ev: PointerEvent) => {
        const delta = ev.clientX - startX;
        const newWidth = Math.min(
          max,
          Math.max(min, side === "right" ? startWidth - delta : startWidth + delta),
        );
        setWidth(newWidth);
      };

      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        try {
          localStorage.setItem(storageKey, String(widthRef.current));
        } catch {
          /* ignore */
        }
        // Trigger terminal refit.
        window.dispatchEvent(new Event("resize"));
      };

      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [side, min, max, storageKey],
  );

  // Also dispatch resize on width change so the terminal refits live.
  useEffect(() => {
    window.dispatchEvent(new Event("resize"));
  }, [width]);

  return [width, { onPointerDown }];
}
