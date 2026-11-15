/**
 * Gallery on the game page's Overview tab: SteamGridDB artwork (heroes and wide grids) first, then RomM's
 * screenshots. A row of thumbnails (each a DialogButton, the only element Steam's gamepad engine focuses in this
 * injected page), and a full-size viewer with Previous / Next. Images come through the backend one per call and are
 * cached on disk, so they also show offline.
 */

import { FC, useEffect, useState } from "react";
import { DialogButton, Focusable, ModalRoot, showModal } from "@decky/ui";
import { getRomScreenshot, getRomScreenshots, getSgdbGallery, getSgdbGalleryImage } from "../api/backend";
import { scrollFocusedToCenter } from "../utils/scrollHelpers";

type Shot = string | null;
type Source = "sgdb" | "romm";
export interface GalleryItem {
  source: Source;
  index: number;
}

const COUNT: Record<Source, (romId: number) => Promise<{ success: boolean; count?: number }>> = {
  sgdb: getSgdbGallery,
  romm: getRomScreenshots,
};
const IMAGE: Record<Source, (romId: number, index: number) => Promise<{ base64: string | null; mime?: string }>> = {
  sgdb: getSgdbGalleryImage,
  romm: getRomScreenshot,
};

async function countOf(source: Source, romId: number): Promise<number> {
  try {
    const result = await COUNT[source](romId);
    return result.success ? (result.count ?? 0) : 0;
  } catch {
    return 0;
  }
}

async function loadImage(romId: number, item: GalleryItem): Promise<Shot> {
  try {
    const result = await IMAGE[item.source](romId, item.index);
    return result.base64 ? `data:${result.mime ?? "image/jpeg"};base64,${result.base64}` : null;
  } catch {
    return null;
  }
}

interface ViewerProps {
  romId: number;
  items: GalleryItem[];
  shots: Shot[];
  start: number;
  /** Injected by showModal; the modal stays on screen until it is called. */
  closeModal?: () => void;
}

export const GalleryViewer: FC<ViewerProps> = ({ romId, items, shots, start, closeModal }) => {
  const [index, setIndex] = useState(start);
  const [images, setImages] = useState<Shot[]>(shots);
  const count = items.length;
  const image = images[index] ?? null;
  const item = items[index];

  // An image whose thumbnail had not loaded yet is fetched when it is shown.
  useEffect(() => {
    if (image !== null || !item) return;
    let live = true;
    void loadImage(romId, item).then((src) => {
      if (live && src) setImages((prev) => prev.map((s, i) => (i === index ? src : s)));
    });
    return () => {
      live = false;
    };
  }, [romId, index, image, item]);

  const step = (by: number) => setIndex((i) => (i + by + count) % count);

  return (
    <ModalRoot closeModal={closeModal}>
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "12px" }}>
        {image ? (
          <img
            src={image}
            alt={`Image ${index + 1}`}
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

export const GameGallery: FC<GalleryProps> = ({ romId }) => {
  const [items, setItems] = useState<GalleryItem[]>([]);
  const [shots, setShots] = useState<Shot[]>([]);

  useEffect(() => {
    let live = true;
    setItems([]);
    setShots([]);
    void (async () => {
      const [art, screenshots] = await Promise.all([countOf("sgdb", romId), countOf("romm", romId)]);
      const all: GalleryItem[] = [
        ...Array.from({ length: art }, (_, index) => ({ source: "sgdb" as const, index })),
        ...Array.from({ length: screenshots }, (_, index) => ({ source: "romm" as const, index })),
      ];
      if (!live || all.length === 0) return;
      setItems(all);
      setShots(all.map(() => null));
      // One at a time: no server is asked for many images at once.
      for (let i = 0; i < all.length && live; i++) {
        const src = await loadImage(romId, all[i]!);
        if (live && src) setShots((prev) => prev.map((s, at) => (at === i ? src : s)));
      }
    })();
    return () => {
      live = false;
    };
  }, [romId]);

  if (items.length === 0) return null;

  return (
    <div className="romm-panel-section" style={{ padding: "12px 0" }} data-testid="game-gallery">
      <div className="romm-panel-section-title">Gallery</div>
      <Focusable style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
        {items.map((item, i) => (
          <DialogButton
            key={`${item.source}-${item.index}`}
            aria-label={`Image ${i + 1}`}
            onFocus={scrollFocusedToCenter}
            onClick={() => showModal(<GalleryViewer romId={romId} items={items} shots={shots} start={i} />)}
            style={{ width: "176px", minWidth: "176px", height: "99px", padding: 0, overflow: "hidden" }}
          >
            {shots[i] ? (
              <img src={shots[i]!} alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
            ) : (
              <span style={{ fontSize: "12px", opacity: 0.6 }}>Loading...</span>
            )}
          </DialogButton>
        ))}
      </Focusable>
    </div>
  );
};
