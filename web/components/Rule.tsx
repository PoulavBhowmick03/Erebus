"use client";

import { useEffect, useRef, useState } from "react";

/** A hairline that draws itself once, when it first enters the viewport. */
export function Rule({ className = "" }: { className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [drawn, setDrawn] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) {
          setDrawn(true);
          io.disconnect();
        }
      },
      { rootMargin: "-8% 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);

  return (
    <div
      ref={ref}
      data-drawn={drawn}
      className={`draw h-px w-full bg-rule ${className}`}
      aria-hidden
    />
  );
}
