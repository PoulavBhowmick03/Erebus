"use client";

import { useEffect, useState } from "react";
import { prefersReducedMotion, useInView } from "@/lib/motion";

type Variant = "rise" | "left" | "right";

const HIDDEN: Record<Variant, string> = {
  rise: "opacity-0 translate-y-5",
  left: "opacity-0 -translate-x-5",
  right: "opacity-0 translate-x-5",
};

/**
 * Scroll-triggered reveal.
 *
 * Server-renders fully visible, then arms the hidden state after mount on
 * motion-capable clients. Same rule as `Secret`: the finished page is what
 * ships in the markup, and the animation is layered on top of it. A crawler,
 * a no-JS reader and a reduced-motion reader all get the final state.
 */
export function Reveal({
  children,
  className = "",
  variant = "rise",
  delay = 0,
  as: Tag = "div",
}: {
  children: React.ReactNode;
  className?: string;
  variant?: Variant;
  delay?: number;
  as?: "div" | "section" | "li" | "tr";
}) {
  const { ref, inView } = useInView<HTMLDivElement>();
  const [armed, setArmed] = useState(false);

  useEffect(() => {
    if (!prefersReducedMotion()) setArmed(true);
  }, []);

  const hidden = armed && !inView;

  return (
    <Tag
      ref={ref as never}
      className={`${className} ${
        hidden ? HIDDEN[variant] : "translate-x-0 translate-y-0 opacity-100"
      } transition-[opacity,transform] duration-[850ms] ease-[cubic-bezier(0.22,1,0.36,1)]`}
      style={delay ? { transitionDelay: `${delay}ms` } : undefined}
    >
      {children}
    </Tag>
  );
}
