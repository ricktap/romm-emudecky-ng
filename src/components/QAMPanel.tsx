import { useState, useRef, useEffect, FC, type ReactNode } from "react";
import { MainPage } from "./MainPage";
import { QamSettings } from "./QamSettings";
import { LibraryPage } from "./LibraryPage";
import { DownloadQueue } from "./DownloadQueue";
import { BackOnCancel } from "./BackOnCancel";
import { findOutermostScrollParent } from "../utils/scrollHelpers";

export type Page = "main" | "settings" | "platforms" | "downloads";

// Module-level page state survives QAM remounts (e.g. after modal close)
let currentPage: Page = "main";

export const QAMPanel: FC = () => {
  const [page, setPageState] = useState<Page>(currentPage); // NOSONAR(typescript:S6754) — setter intentionally renamed; setPage wraps it below to provide custom navigation behavior.
  const rootRef = useRef<HTMLDivElement>(null);
  const setPage = (p: Page) => {
    currentPage = p;
    setPageState(p);
  };

  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const container = findOutermostScrollParent(el);
    // rAF lets the new page mount/measure before we set scrollTop.
    const rafHandle = requestAnimationFrame(() => {
      if (container) container.scrollTop = 0;
    });
    // Steam's gamepad nav retains a focus pointer across page swaps and
    // resolves it on the next input — landing on a button at the old page's
    // position. Force focus to the first button so navigation starts at the
    // top. Same querySelector + .focus() + gpfocus pattern as CustomPlayButton.
    const focusTimer = setTimeout(() => {
      const btn = el.querySelector("button");
      if (btn) {
        btn.focus();
        btn.classList.add("gpfocus");
      }
    }, 50);
    return () => {
      cancelAnimationFrame(rafHandle);
      clearTimeout(focusTimer);
    };
  }, [page]);

  let content: ReactNode;
  switch (page) {
    case "settings":
      content = <QamSettings onBack={() => setPage("main")} />;
      break;
    case "platforms":
      content = <LibraryPage onBack={() => setPage("main")} />;
      break;
    case "downloads":
      content = <DownloadQueue compact onBack={() => setPage("main")} />;
      break;
    default:
      content = <MainPage onNavigate={(p) => setPage(p)} />;
  }

  // The main page is the QAM's root: B closes the panel as usual. Every other page is one level below it, so B
  // goes back to the main page first (like a history back) instead of closing the whole panel.
  return (
    <div ref={rootRef}>
      {page === "main" ? content : <BackOnCancel onBack={() => setPage("main")}>{content}</BackOnCancel>}
    </div>
  );
};
