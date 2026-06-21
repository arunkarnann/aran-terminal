import type { ReactNode } from "react";

interface FocusRingProps {
  /** 0..1 fraction filled. */
  progress: number;
  size: number;
  stroke: number;
  color: string;
  /** Center content (clock, label). */
  children?: ReactNode;
  className?: string;
}

/** A circular progress ring. Pure SVG, no animation lib — the parent re-renders it. */
export function FocusRing({
  progress,
  size,
  stroke,
  color,
  children,
  className,
}: FocusRingProps) {
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const clamped = Math.max(0, Math.min(1, progress));
  const offset = c * (1 - clamped);

  return (
    <div
      className={`focus-ring ${className ?? ""}`}
      style={{ width: size, height: size }}
    >
      <svg width={size} height={size} className="focus-ring-svg">
        <circle
          className="focus-ring-track"
          cx={size / 2}
          cy={size / 2}
          r={r}
          strokeWidth={stroke}
          fill="none"
        />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          strokeWidth={stroke}
          fill="none"
          stroke={color}
          strokeLinecap="butt"
          strokeDasharray={c}
          strokeDashoffset={offset}
          transform={`rotate(-90 ${size / 2} ${size / 2})`}
          style={{ transition: "stroke-dashoffset 0.5s linear" }}
        />
      </svg>
      <div className="focus-ring-center">{children}</div>
    </div>
  );
}
