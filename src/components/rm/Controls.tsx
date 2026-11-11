import { FC, ReactNode, useRef } from "react";
import { Focusable } from "@decky/ui";
import { Icon } from "./Icon";

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
  <Focusable noFocusRing className="rm-row" {...(testId ? { "data-testid": testId } : {})}>
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
    <Focusable noFocusRing
      className={`rm-sw${checked ? " rm-on" : ""}`}
      aria-label={label}
      data-testid={`toggle-${label}`}
      onActivate={flip}
      onClick={flip}
    >
      <span className="rm-knob" />
    </Focusable>
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
    <Focusable noFocusRing className="rm-corebtn" aria-label={label} data-testid={`choice-${label}`} onActivate={next} onClick={next}>
      {current}
    </Focusable>
  );
};

/** A drill-down row of the Quick Access Menu: label, optional count badge and a chevron (the prototype's `navrow`). */
export const NavRow: FC<{ label: string; badge?: number; onActivate: () => void }> = ({ label, badge, onActivate }) => {
  const go = useOnce(onActivate);
  return (
    <Focusable noFocusRing className="rm-navrow" data-testid={`nav-${label}`} onActivate={go} onClick={go}>
      <span className="rm-sp">{label}</span>
      {badge ? <span className="rm-badge">{badge}</span> : null}
      <Icon name="chev" />
    </Focusable>
  );
};

/** One thing that needs the user's attention: amber, with a title, a short line and a chevron (the prototype's `att`). */
export const AttRow: FC<{ title: string; sub?: string; onActivate: () => void }> = ({ title, sub, onActivate }) => {
  const go = useOnce(onActivate);
  return (
    <Focusable noFocusRing className="rm-att" data-testid="attention-row" onActivate={go} onClick={go}>
      <Icon name="warn" size={18} className="rm-amberc" />
      <span style={{ flex: 1 }}>
        <span className="rm-t">{title}</span>
        {sub ? <span className="rm-s">{sub}</span> : null}
      </span>
      <Icon name="chev" />
    </Focusable>
  );
};

/** The header of a Quick Access Menu sub page: a back button and the page title (the prototype's `qhead`). */
export const QamHeader: FC<{ title: string; onBack: () => void }> = ({ title, onBack }) => {
  const back = useOnce(onBack);
  return (
    <div className="rm-qhead">
      <Focusable noFocusRing className="rm-btn rm-sm rm-icon" aria-label="Back" data-testid="qam-back" onActivate={back} onClick={back}>
        <Icon name="back" size={16} />
      </Focusable>
      <h2>{title}</h2>
    </div>
  );
};

/** A row of small segment buttons (the prototype's tab-like chips) where exactly one is on. */
export const RmSegments: FC<{
  items: { id: string; label: string }[];
  value: string;
  onChange: (id: string) => void;
  testPrefix: string;
}> = ({ items, value, onChange, testPrefix }) => (
  <Focusable noFocusRing flow-children="right" className="rm-chips" style={{ marginBottom: 12 }}>
    {items.map((item) => (
      <Focusable noFocusRing
        key={item.id}
        data-testid={`${testPrefix}-${item.id}`}
        className={`rm-fchip${value === item.id ? " rm-on" : ""}`}
        onActivate={() => onChange(item.id)}
        onClick={() => onChange(item.id)}
      >
        {item.label}
      </Focusable>
    ))}
  </Focusable>
);
