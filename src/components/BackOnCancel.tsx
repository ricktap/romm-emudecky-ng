import { FC, ReactNode } from "react";
import { Focusable } from "@decky/ui";

interface BackOnCancelProps {
  onBack: () => void;
  children: ReactNode;
}

/**
 * Makes the controller's B button step back one level instead of closing the whole Quick Access Menu: the cancel
 * event is handled here and not passed on to the panel. Dropdowns, modals and menus inside the page still take
 * B first (they sit closer to the focus), so this only acts when nothing deeper wants it.
 */
export const BackOnCancel: FC<BackOnCancelProps> = ({ onBack, children }) => (
  <Focusable
    flow-children="down"
    noFocusRing
    onCancel={(e: CustomEvent) => {
      e.stopPropagation();
      onBack();
    }}
  >
    {children}
  </Focusable>
);
