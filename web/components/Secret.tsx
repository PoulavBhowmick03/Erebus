"use client";

import { useEffect, useRef, useState } from "react";
import { useKey } from "./KeyContext";

const GLYPHS = "0123456789abcdef";

/** Deterministic per-value ciphertext, so nothing jumps between renders. */
function cipherFor(value: string): string {
  let h = 2166136261;
  const out: string[] = [];
  for (let i = 0; i < value.length; i += 1) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
    const c = value[i];
    // preserve the shape of the field: separators stay, characters become hex
    out.push(/[\s.:·—/,]/.test(c) ? c : GLYPHS[(h >>> (i % 24)) % GLYPHS.length]);
  }
  return out.join("");
}

/**
 * A value that is private on-chain.
 *
 * Three states, and the honesty of the page depends on them being distinct:
 *   • barred      — an ink bar. you know a value is here, nothing more.
 *   • peeked      — lift the bar and you get what a chain reader gets: ciphertext.
 *   • decrypted   — with a viewing key the ciphertext resolves to the record.
 *
 * The plaintext is always in the DOM. This is a demonstration of the protocol's
 * disclosure model, not a security boundary, and it must not pretend otherwise.
 */
export function Secret({
  value,
  className = "",
  delay = 0,
}: {
  value: string;
  className?: string;
  delay?: number;
}) {
  const { keyState, reduced } = useKey();
  const [text, setText] = useState(value);
  const [busy, setBusy] = useState(false);
  const [held, setHeld] = useState(false);
  const frame = useRef<number>(0);
  const mounted = useRef(false);

  useEffect(() => {
    const cipher = cipherFor(value);
    const target = keyState === "held" ? value : cipher;
    const from = keyState === "held" ? cipher : value;

    if (!mounted.current) {
      mounted.current = true;
      // first client pass: adopt the state without animating into it
      setText(target);
      return;
    }

    if (reduced) {
      setText(target);
      return;
    }

    // resolve left to right: each character settles, the rest churn
    const started = performance.now();
    const total = 260 + value.length * 26;
    setBusy(true);

    const tick = (now: number) => {
      const t = Math.min(1, Math.max(0, (now - started - delay) / total));
      const settled = Math.floor(t * target.length);
      let next = "";
      for (let i = 0; i < target.length; i += 1) {
        if (i < settled) next += target[i];
        else if (/[\s.:·—/,]/.test(target[i])) next += target[i];
        else next += GLYPHS[Math.floor(Math.random() * GLYPHS.length)];
      }
      setText(next);
      if (t < 1) {
        frame.current = requestAnimationFrame(tick);
      } else {
        setText(target);
        setBusy(false);
      }
    };

    frame.current = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame.current);
    // `from` participates only as documentation of the direction of travel
    void from;
  }, [keyState, value, reduced, delay]);

  return (
    <span
      className={`redact ${className}`}
      data-held={held ? "true" : undefined}
      tabIndex={keyState === "dropped" ? 0 : -1}
      role={keyState === "dropped" ? "button" : undefined}
      aria-label={
        keyState === "dropped" ? `redacted field — hold to see what a chain reader sees` : undefined
      }
      onPointerDown={() => setHeld(true)}
      onPointerUp={() => setHeld(false)}
      onPointerLeave={() => setHeld(false)}
    >
      <span className={`scramble ${busy ? "scrambling" : ""}`}>{text}</span>
    </span>
  );
}
