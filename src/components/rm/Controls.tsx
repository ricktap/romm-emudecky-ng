import { FC, ReactNode, useRef } from "react";
import { Focusable } from "@decky/ui";

/** A titled group of settings rows (the prototype's `scard`). */
export const RmCard: FC<{ title: string; children: ReactNode }> = ({ title, children }) => (
  <div className="rm-scard">
    <h3>{title}</h3>
    {children}
  </div>
);

/** One label / control row; `sub` is the small grey line under the label. */
export const RmRow: FC<{ label: ReactNode; sub?: ReactNode; children?: ReactNode; testId?: string }> = ({
  label,
  sub,
  children,
  testId,
}) => (
  <div className="rm-row" {...(testId ? { "data-testid": testId } : {})}>
    <span>
      {label}
      {sub ? <div className="rm-muted rm-small">{sub}</div> : null}
    </span>
    {children}
  </div>
);

/** A row that only reports a value (grey, right-aligned). The row itself can take the focus so the cursor can stop on it. */
export const RmValueRow: FC<{ label: ReactNode; sub?: ReactNode; value?: ReactNode; testId?: string }> = ({
  label,
  sub,
  value,
  testId,
}) => (
  <Focusable className="rm-row" {...(testId ? { "data-testid": testId } : {})}>
    <span>
      {label}
      {sub ? <div className="rm-muted rm-small">{sub}</div> : null}
    </span>
    <span className="rm-v">{value}</span>
  </Focusable>
);

/** Press-once helper shared by the controls: a tap that also arrives as an A press counts once. */
function useOnce(fn: () => void): () => void {
  const last = useRef(0);
  return () => {
    const now = Date.now();
    if (now - last.current < 250) return;
    last.current = now;
    fn();
  };
}

/** The prototype's switch. */
export const RmToggle: FC<{ checked: boolean; onChange: (value: boolean) => void; label: string; disabled?: boolean }> = ({
  checked,
  onChange,
  label,
  disabled,
}) => {
  const flip = useOnce(() => !disabled && onChange(!checked));
  return (
    <Focusable
      className={`rm-sw${checked ? " rm-on" : ""}`}
      aria-label={label}
      data-testid={`toggle-${label}`}
      onActivate={flip}
      onClick={flip}
    />
  );
};

/** A small bordered button showing the current choice; each press moves to the next option (the prototype's core button). */
export const RmChoice: FC<{
  value: string;
  options: { data: string; label: string }[];
  onChange: (value: string) => void;
  label: string;
}> = ({ value, options, onChange, label }) => {
  const next = useOnce(() => {
    if (options.length === 0) return;
    const at = options.findIndex((o) => o.data === value);
    onChange(options[(at + 1) % options.length]!.data);
  });
  const current = options.find((o) => o.data === value)?.label ?? value;
  return (
    <Focusable className="rm-corebtn" aria-label={label} data-testid={`choice-${label}`} onActivate={next} onClick={next}>
      {current}
    </Focusable>
  );
};
