import { FC } from "react";

/** The prototype's icon set (stroked, 24-unit viewBox). */
const PATHS = {
  check: { d: ["M5 12l5 5L20 7"], sw: 3.2 },
  cloud: { d: ["M7 18a4 4 0 0 1-.5-7.97A6 6 0 0 1 18 9a4.5 4.5 0 0 1-.5 9z"], sw: 2.4 },
  bang: { d: ["M12 7v6", "M12 17v.01"], sw: 3 },
  down: { d: ["M12 5v12", "M7 12l5 5 5-5"], sw: 3 },
  chev: { d: ["M9 6l6 6-6 6"], sw: 2.6 },
  chevd: { d: ["M6 9l6 6 6-6"], sw: 2.6 },
  back: { d: ["M15 6l-6 6 6 6"], sw: 2.6 },
  warn: { d: ["M12 3l10 18H2z", "M12 10v5", "M12 18v.01"], sw: 2.4 },
  okc: { d: ["M8 12l3 3 5-6"], circle: true, sw: 2.6 },
  neu: { d: ["M7 18a4 4 0 0 1-.5-7.97A6 6 0 0 1 18 9a4.5 4.5 0 0 1-.5 9z"], sw: 2.2 },
  busy: { d: ["M12 4v12", "M7 11l5 5 5-5", "M5 20h14"], sw: 2.6 },
  dl: { d: ["M12 4v12", "M7 11l5 5 5-5", "M5 20h14"], sw: 2.6 },
  pause: { d: ["M8 5v14M16 5v14"], sw: 3 },
  x: { d: ["M6 6l12 12M18 6L6 18"], sw: 2.4 },
  grid: { d: ["M3 3h7v7H3z", "M14 3h7v7h-7z", "M3 14h7v7H3z", "M14 14h7v7h-7z"], sw: 2.2 },
  sync: { d: ["M20 11a8 8 0 1 0-2.3 5.7", "M20 4v7h-7"], sw: 2.4 },
  search: { d: ["M20 20l-3.5-3.5"], circle11: true, sw: 2.4 },
} as const;

export type IconName = keyof typeof PATHS | "play" | "dots";

interface IconProps {
  name: IconName;
  size?: number;
  className?: string;
}

export const Icon: FC<IconProps> = ({ name, size = 14, className }) => {
  if (name === "play") {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
        <path d="M7 5l12 7-12 7z" />
      </svg>
    );
  }
  if (name === "dots") {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
        <circle cx="5" cy="12" r="2" />
        <circle cx="12" cy="12" r="2" />
        <circle cx="19" cy="12" r="2" />
      </svg>
    );
  }
  const spec = PATHS[name];
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={spec.sw}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      {"circle" in spec && <circle cx="12" cy="12" r="9" />}
      {"circle11" in spec && <circle cx="11" cy="11" r="7" />}
      {spec.d.map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  );
};
