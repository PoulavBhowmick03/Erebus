"use client";

import { useEffect } from "react";
import Lenis from "lenis";
import { prefersReducedMotion, setScrollDelegate } from "@/lib/motion";

/**
 * Inertia scrolling. Lenis smooths the real scroll position rather than
 * hijacking it with a transform, so the conductor, the hairline draws and the
 * lattice all keep reading genuine `window.scrollY`.
 *
 * Reduced motion gets the browser's own scrolling, untouched.
 */
export function SmoothScroll() {
  useEffect(() => {
    if (prefersReducedMotion()) return;

    const lenis = new Lenis({ lerp: 0.085 });
    let raf = requestAnimationFrame(function loop(t) {
      lenis.raf(t);
      raf = requestAnimationFrame(loop);
    });

    // Capture phase: otherwise the browser's native hash jump lands first and
    // Lenis glides afterwards, which reads as a stutter.
    const onClick = (e: MouseEvent) => {
      const anchor = (e.target as Element | null)?.closest?.('a[href^="#"]');
      const hash = anchor?.getAttribute("href");
      if (!hash || hash === "#") return;
      const target = document.querySelector(hash);
      if (!target) return;
      e.preventDefault();
      lenis.scrollTo(target as HTMLElement);
    };
    document.addEventListener("click", onClick, { capture: true });
    setScrollDelegate((y) => lenis.scrollTo(y as never));

    return () => {
      setScrollDelegate(null);
      document.removeEventListener("click", onClick, { capture: true });
      cancelAnimationFrame(raf);
      lenis.destroy();
    };
  }, []);

  return null;
}
