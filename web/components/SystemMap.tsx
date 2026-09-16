"use client";

import { useEffect, useRef, useState } from "react";

const MIN_SCALE = 1;
const MAX_SCALE = 8;
const STEP = 0.75;

function MagnifierIcon({ minus }: { minus: boolean }) {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="7" cy="7" r="5.25" stroke="currentColor" strokeWidth="1.4" />
      <path d="M11.2 11.2L15 15" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      <path d="M4.5 7H9.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      {minus ? null : (
        <path d="M7 4.5V9.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      )}
    </svg>
  );
}

/**
 * A thumbnail that opens into a full-screen, pannable, zoomable view — no
 * new tab, no leaving the page. Scroll or drag to pan; the two magnifier
 * buttons at the bottom right zoom in and out around a fixed viewport width.
 */
export function SystemMap({ src, alt }: { src: string; alt: string }) {
  const [open, setOpen] = useState(false);
  const [scale, setScale] = useState(MIN_SCALE);
  const [dragging, setDragging] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const drag = useRef({ x: 0, y: 0, left: 0, top: 0 });

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    if (!el) return;
    // A left click/primary touch only — pinch and right-click stay native.
    if (e.button !== 0) return;
    drag.current = { x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
    setDragging(true);
    el.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    if (!el || !dragging) return;
    el.scrollLeft = drag.current.left - (e.clientX - drag.current.x);
    el.scrollTop = drag.current.top - (e.clientY - drag.current.y);
  };

  const endDrag = () => setDragging(false);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = prevOverflow;
    };
  }, [open]);

  return (
    <>
      <button
        type="button"
        onClick={() => {
          setScale(MIN_SCALE);
          setOpen(true);
        }}
        className="group relative block w-full overflow-hidden rounded-xl border border-rule text-left"
        aria-label="Open the system map full screen"
      >
        <img src={src} alt={alt} className="block h-auto w-full" />
        <span className="pointer-events-none absolute inset-0 flex items-center justify-center bg-ground/0 opacity-0 transition-opacity duration-200 group-hover:bg-ground/60 group-hover:opacity-100">
          <span className="label !text-fore">Click to view full screen</span>
        </span>
      </button>

      {open ? (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="System map, full screen"
          className="fixed inset-0 z-[100] bg-ground"
        >
          <button
            type="button"
            onClick={() => setOpen(false)}
            className="absolute right-6 top-6 z-10 mono-xs uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
          >
            Close ✕
          </button>

          <div
            ref={scrollRef}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={endDrag}
            onPointerLeave={endDrag}
            onPointerCancel={endDrag}
            className="h-full w-full select-none overflow-auto p-10"
            style={{ cursor: dragging ? "grabbing" : "grab", touchAction: "none" }}
          >
            <img
              src={src}
              alt={alt}
              draggable={false}
              style={{ width: `${scale * 92}vw`, height: "auto" }}
              className="mx-auto"
            />
          </div>

          <div className="absolute bottom-6 right-6 z-10 flex border border-rule bg-panel">
            <button
              type="button"
              onClick={() => setScale((s) => Math.max(MIN_SCALE, s - STEP))}
              disabled={scale <= MIN_SCALE}
              aria-label="Zoom out"
              className="flex h-11 w-11 items-center justify-center border-r border-rule text-fore-2 transition-colors hover:text-fore disabled:opacity-30"
            >
              <MagnifierIcon minus />
            </button>
            <button
              type="button"
              onClick={() => setScale((s) => Math.min(MAX_SCALE, s + STEP))}
              disabled={scale >= MAX_SCALE}
              aria-label="Zoom in"
              className="flex h-11 w-11 items-center justify-center text-fore-2 transition-colors hover:text-fore disabled:opacity-30"
            >
              <MagnifierIcon minus={false} />
            </button>
          </div>
        </div>
      ) : null}
    </>
  );
}
