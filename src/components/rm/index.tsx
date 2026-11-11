import { FC, ReactNode, useRef } from "react";
import { Focusable } from "@decky/ui";
import { ROMM_CSS } from "../../styles/rommTheme";

export { Icon } from "./Icon";
export type { IconName } from "./Icon";

/** Puts the RomM theme on the page once; render it at the top of every full-screen surface. */
export const RmStyles: FC = () => <style data-testid="rm-styles">{ROMM_CSS}</style>;

interface RmButtonProps {
  children?: ReactNode;
  /** Prototype modifiers: primary, play, warn, ghost, danger, sm, icon, block. */
  variant?: string;
  onActivate: () => void;
  className?: string;
  label?: string;
  testId?: string;
  disabled?: boolean;
}

/**
 * A prototype button that the gamepad can reach: a Focusable (A presses it, focus looks like the prototype's hover)
 * that also answers a tap or click. A press that arrives both ways within a moment counts once.
 */
export const RmButton: FC<RmButtonProps> = ({ children, variant = "", onActivate, className = "", label, testId, disabled }) => {
  const last = useRef(0);
  const fire = () => {
    const now = Date.now();
    if (disabled || now - last.current < 250) return;
    last.current = now;
    onActivate();
  };
  const mods = variant
    .split(" ")
    .filter(Boolean)
    .map((m) => `rm-${m}`)
    .join(" ");
  return (
    <Focusable noFocusRing
      className={`rm-btn ${mods} ${className}`.trim()}
      onActivate={fire}
      onClick={fire}
      {...(label ? { "aria-label": label } : {})}
      {...(testId ? { "data-testid": testId } : {})}
      {...(disabled ? { style: { opacity: 0.45 }, "aria-disabled": true } : {})}
    >
      {children}
    </Focusable>
  );
};

/** The round L1 / R1 hint shown at the ends of the tab bar. */
export const Glyph: FC<{ children: ReactNode; side?: "left" | "right" }> = ({ children, side }) => (
  <span className="rm-glyph" style={side === "left" ? { marginRight: 6 } : side === "right" ? { marginLeft: 6 } : undefined}>
    {children}
  </span>
);

export { RmCard, RmRow, RmValueRow, RmToggle, RmChoice, NavRow, AttRow, QamHeader, RmSegments, RmButtonGroup, ActionRow } from "./Controls";
