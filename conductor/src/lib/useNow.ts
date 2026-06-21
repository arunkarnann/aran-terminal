import { useEffect, useState } from "react";

/** A shared wall-clock that re-renders on an interval. Used to age session
 *  uptime labels/colors without each component spinning its own timer. */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}
