"use client";

import { useRef } from "react";
import { progressOf, useConductor } from "@/lib/motion";

/**
 * The closing signature: the brand lockup at full bleed, ghosted, enlarging as
 * the footer scrolls into view.
 *
 * Written imperatively — the subscriber writes `style` directly instead of
 * setting React state, because this runs every frame while scrolling and a
 * re-render per frame is exactly the layout thrash `lib/motion` exists to
 * avoid. Under `prefers-reduced-motion` the hook never subscribes, so the
 * static inline frame is the whole effect.
 */
export function FooterMark() {
  const ref = useRef<HTMLDivElement>(null);

  useConductor((scrollY, viewport) => {
    const host = ref.current;
    const mark = host?.firstElementChild;
    if (!host || !(mark instanceof HTMLElement)) return;
    const p = progressOf(host, scrollY, viewport);
    mark.style.transform = `scale(${(0.9 + p * 0.14).toFixed(3)})`;
    mark.style.opacity = (0.05 + p * 0.07).toFixed(3);
  });

  return (
    <div ref={ref} className="overflow-hidden py-4" aria-hidden>
      <img
        src="/erebus-lockup.svg"
        alt=""
        className="mx-auto block w-[112%] max-w-none"
        style={{ transform: "scale(0.96)", opacity: "0.08", transformOrigin: "center center" }}
      />
    </div>
  );
}
