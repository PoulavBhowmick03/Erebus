"use client";

import { useCallback, useRef, useState } from "react";

// The neutral half of the headline. Split so each word can be measured on its
// own; the dark twin below renders the same string and is revealed through a
// clip rectangle spanning the hovered range.
const WORDS = "Negotiate in darkness,".split(" ");

type Inset = { top: number; right: number; bottom: number; left: number };

/**
 * The headline's one interactive move, and it behaves like a text selection.
 * The first word the cursor lands on becomes the anchor; moving to another word
 * fills everything from the anchor to the word under the cursor, so the block
 * grows English-style left to right ("Negotiate" → "Negotiate in" → "Negotiate
 * in darkness,") and just as readily the other way, hovering "darkness" first
 * and then "in" to fill "in darkness". Leaving the line drops the anchor.
 *
 * Two identical sets of words are stacked: the base in the normal foreground,
 * and a `fore`-filled twin in ground that is clipped to the range's bounding
 * box. Only the rectangle is animated, so the fill slides and stretches from
 * one range to the next rather than blinking off and on. The twin is
 * presentational and `aria-hidden`; the base carries the text.
 */
export function HeadlineHighlight() {
  const darkRef = useRef<HTMLSpanElement>(null);
  const wordRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const anchor = useRef<number | null>(null);
  const [inset, setInset] = useState<Inset | null>(null);
  const [on, setOn] = useState(false);

  const draw = useCallback((from: number, to: number) => {
    const dark = darkRef.current;
    if (!dark) return;
    const lo = Math.min(from, to);
    const hi = Math.max(from, to);
    let top = Infinity;
    let left = Infinity;
    let bottom = -Infinity;
    let right = -Infinity;
    for (let i = lo; i <= hi; i++) {
      const el = wordRefs.current[i];
      if (!el) continue;
      const r = el.getBoundingClientRect();
      top = Math.min(top, r.top);
      left = Math.min(left, r.left);
      bottom = Math.max(bottom, r.bottom);
      right = Math.max(right, r.right);
    }
    if (!Number.isFinite(top)) return;
    const d = dark.getBoundingClientRect();
    setInset({
      top: top - d.top,
      left: left - d.left,
      right: d.right - right,
      bottom: d.bottom - bottom,
    });
    setOn(true);
  }, []);

  const enter = useCallback(
    (i: number) => {
      if (anchor.current === null) anchor.current = i;
      draw(anchor.current, i);
    },
    [draw],
  );

  const leave = useCallback(() => {
    anchor.current = null;
    // Only fade opacity here — leave `inset` as its last-drawn shape so the
    // clip-path has nothing to snap to. Clearing it used to reset clip-path
    // to `none` (fully unclipped) the instant the mouse left, so the full
    // white box flashed behind the 160ms opacity fade instead of the fill
    // just dimming out in place.
    setOn(false);
  }, []);

  return (
    <span className="hl-line" onMouseLeave={leave}>
      <span className="hl-base">
        {WORDS.map((word, i) => (
          <span key={word}>
            <span
              ref={(el) => {
                wordRefs.current[i] = el;
              }}
              className="hl-word"
              onMouseEnter={() => enter(i)}
            >
              {word}
            </span>
            {i < WORDS.length - 1 ? " " : ""}
          </span>
        ))}
      </span>

      <span
        ref={darkRef}
        aria-hidden
        className="hl-dark"
        data-on={on ? "true" : undefined}
        style={
          inset
            ? {
                clipPath: `inset(${inset.top}px ${inset.right}px ${inset.bottom}px ${inset.left}px)`,
              }
            : undefined
        }
      >
        {WORDS.join(" ")}
      </span>
    </span>
  );
}
