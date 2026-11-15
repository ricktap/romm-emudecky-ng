/**
 * Screenshot gallery on the game page's Overview tab: a row of thumbnails (each a DialogButton, the only element
 * Steam's gamepad engine focuses in this injected page), and a full-size viewer with Previous / Next.
 * Images come from RomM through the backend, one per call, and are cached on disk so they also show offline.
 */

import { FC, useEffect, useState } from "react";
import { DialogButton, Focusable, ModalRoot, showModal } from "@decky/ui";
import { getRomScreenshot, getRomScreenshots } from "../api/backend";
import { scrollFocusedToCenter } from "../utils/scrollHelpers";

type Shot = string | null;

async function loadShot(romId: number, index: number): Promise<Shot> {
  try {
    const result = await getRomScreenshot(romId, index);
    return result.base64 ? `data:${result.mime ?? "image/jpeg"};base64,${result.base64}` : null;
  } catch {
    return null;
  }
}

interface ViewerProps {
  romId: number;
  shots: Shot[];
  start: number;
  /** Injected by showModal; the modal stays on screen until it is called. */
  closeModal?: () => void;
}

export const ScreenshotViewer: FC<ViewerProps> = ({ romId, shots, start, closeModal }) => {
  const [index, setIndex] = useState(start);
  const [images, setImages] = useState<Shot[]>(shots);
  const count = images.length;
  const image = images[index] ?? null;

  // A screenshot whose thumbnail had not loaded yet is fetched when it is shown.
  useEffect(() => {
    if (image !== null) return;
    let live = true;
    void loadShot(romId, index).then((src) => {
      if (live && src) setImages((prev) => prev.map((s, i) => (i === index ? src : s)));
    });
    return () => {
      live = false;
    };
  }, [romId, index, image]);

  const step = (by: number) => setIndex((i) => (i + by + count) % count);

  return (
    <ModalRoot closeModal={closeModal}>
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "12px" }}>
        {image ? (
          <img
            src={image}
            alt={`Screenshot ${index + 1}`}
            style={{ maxWidth: "100%", maxHeight: "62vh", objectFit: "contain", borderRadius: "4px" }}
          />
        ) : (
          <div style={{ height: "40vh", display: "flex", alignItems: "center" }}>Loading...</div>
        )}
        <Focusable flow-children="right" style={{ display: "flex", gap: "8px", alignItems: "center" }}>
          <DialogButton style={{ minWidth: "120px" }} disabled={count < 2} onClick={() => step(-1)}>
            Previous
          </DialogButton>
          <div style={{ minWidth: "64px", textAlign: "center", opacity: 0.8 }}>
            {index + 1} / {count}
          </div>
          <DialogButton style={{ minWidth: "120px" }} disabled={count < 2} onClick={() => step(1)}>
            Next
          </DialogButton>
          <DialogButton style={{ minWidth: "120px" }} onClick={() => closeModal?.()}>
            Close
          </DialogButton>
        </Focusable>
      </div>
    </ModalRoot>
  );
};

interface GalleryProps {
  romId: number;
}

export const ScreenshotGallery: FC<GalleryProps> = ({ romId }) => {
  const [shots, setShots] = useState<Shot[]>([]);

  useEffect(() => {
    let live = true;
    setShots([]);
    void (async () => {
      let count = 0;
      try {
        const result = await getRomScreenshots(romId);
        count = result.success ? (result.count ?? 0) : 0;
      } catch {
        count = 0;
      }
      if (!live || count === 0) return;
      setShots(Array.from({ length: count }, () => null));
      // One at a time: the server is not asked for eight images at once.
      for (let i = 0; i < count && live; i++) {
        const src = await loadShot(romId, i);
        if (live && src) setShots((prev) => prev.map((s, at) => (at === i ? src : s)));
      }
    })();
    return () => {
      live = false;
    };
  }, [romId]);

  if (shots.length === 0) return null;

  return (
    <div className="romm-panel-section" style={{ padding: "12px 0" }} data-testid="screenshot-gallery">
      <div className="romm-panel-section-title">Screenshots</div>
      <Focusable style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
        {shots.map((src, i) => (
          <DialogButton
            key={i}
            aria-label={`Screenshot ${i + 1}`}
            onFocus={scrollFocusedToCenter}
            onClick={() => showModal(<ScreenshotViewer romId={romId} shots={shots} start={i} />)}
            style={{ width: "176px", minWidth: "176px", height: "99px", padding: 0, overflow: "hidden" }}
          >
            {src ? (
              <img src={src} alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
            ) : (
              <span style={{ fontSize: "12px", opacity: 0.6 }}>Loading...</span>
            )}
          </DialogButton>
        ))}
      </Focusable>
    </div>
  );
};
