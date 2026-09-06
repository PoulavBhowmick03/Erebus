"use client";

import { useEffect, useRef, useState } from "react";

export const prefersReducedMotion = () =>
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/**
 * One rAF loop for every scroll-linked effect on the page.
 *
 * Each subscriber gets the same frame and the same scroll value. The point is
 * that reads happen together and writes happen together: if each effect ran its
 * own listener it would measure, write, and force the next one to measure again,
 * which is a layout thrash per effect per frame.
 */
type Subscriber = (scrollY: number, viewport: number) => void;

const subscribers = new Set<Subscriber>();
let frame = 0;

function pump() {
  frame = requestAnimationFrame(pump);
  const y = window.scrollY;
  const h = window.innerHeight;
  for (const fn of subscribers) fn(y, h);
}

export function useConductor(fn: Subscriber, enabled = true) {
  const held = useRef(fn);
  held.current = fn;

  useEffect(() => {
    if (!enabled || prefersReducedMotion()) return;
    const sub: Subscriber = (y, h) => held.current(y, h);
    subscribers.add(sub);
    if (subscribers.size === 1) frame = requestAnimationFrame(pump);
    return () => {
      subscribers.delete(sub);
      if (subscribers.size === 0) cancelAnimationFrame(frame);
    };
  }, [enabled]);
}

export const clamp = (v: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));

/** Progress of an element through the viewport, 0 as it enters, 1 as it leaves. */
export function progressOf(el: HTMLElement, scrollY: number, viewport: number) {
  const rect = el.getBoundingClientRect();
  const top = rect.top + scrollY;
  return clamp((scrollY + viewport - top) / (viewport + rect.height));
}

/**
 * Fires once, when the element first crosses into view.
 *
 * IntersectionObserver alone is not sufficient here. Under inertia scrolling a
 * fast travel can land past an element without the observer ever reporting it
 * intersecting, and a `Reveal` that never fires is content that silently stays
 * invisible — a worse failure than having no animation at all. So the observer
 * is backed by a geometry check on scroll: anything whose top edge has passed
 * the bottom of the viewport is revealed regardless of what the observer saw.
 */
export function useInView<T extends HTMLElement>(rootMargin = "-10% 0px") {
  const ref = useRef<T>(null);
  const [inView, setInView] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (prefersReducedMotion()) {
      setInView(true);
      return;
    }

    let done = false;
    const reveal = () => {
      if (done) return;
      done = true;
      setInView(true);
      io.disconnect();
      window.removeEventListener("scroll", check);
      window.removeEventListener("resize", check);
    };

    const check = () => {
      const r = el.getBoundingClientRect();
      if (r.top < window.innerHeight * 0.92) reveal();
    };

    const io = new IntersectionObserver(
      (entries) => entries[0]?.isIntersecting && reveal(),
      { rootMargin },
    );
    io.observe(el);
    window.addEventListener("scroll", check, { passive: true });
    window.addEventListener("resize", check, { passive: true });
    check();

    return () => {
      io.disconnect();
      window.removeEventListener("scroll", check);
      window.removeEventListener("resize", check);
    };
  }, [rootMargin]);

  return { ref, inView };
}

/** Lenis owns the means of travel, so anchor jumps glide like everything else. */
let scrollTo: ((y: number | HTMLElement) => void) | null = null;
export const setScrollDelegate = (fn: typeof scrollTo) => (scrollTo = fn);
export const glideTo = (target: number | HTMLElement) => {
  if (scrollTo) scrollTo(target);
  else if (target instanceof HTMLElement) target.scrollIntoView({ behavior: "smooth" });
};
