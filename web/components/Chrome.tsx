"use client";

import { useEffect, useState } from "react";
import { RollingLink } from "./RollingLink";
import { useKey } from "./KeyContext";

export function Eyebrow({ children }: { children: React.ReactNode }) {
  return <p className="label m-0">{children}</p>;
}

export function Section({
  id,
  children,
  className = "",
}: {
  id?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section id={id} className={`px-[var(--edge)] ${className}`}>
      <div className="mx-auto w-full max-w-[1560px]">{children}</div>
    </section>
  );
}

const NAV = [
  { href: "#leaks", label: "What leaks" },
  { href: "#run", label: "Run a deal" },
  { href: "#observer", label: "Observer" },
  { href: "#evidence", label: "Evidence" },
];

export function Header() {
  const { keyState, toggle } = useKey();
  const [stuck, setStuck] = useState(false);

  useEffect(() => {
    const onScroll = () => setStuck(window.scrollY > 8);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  const held = keyState === "held";

  return (
    <header
      className={`sticky top-0 z-50 px-[var(--edge)] transition-colors duration-300 ${
        stuck ? "bg-ground/95" : ""
      }`}
      style={{ borderBottom: `1px solid ${stuck ? "var(--color-rule)" : "transparent"}` }}
    >
      <div className="mx-auto flex h-14 w-full max-w-[1560px] items-center justify-between gap-6">
        <a href="#top" className="flex items-baseline gap-3">
          <span className="label !text-fore !tracking-[0.34em] text-[11px]">Erebus</span>
          <span className="mono-xs hidden text-fore-3 sm:inline">private settlement for agents</span>
        </a>

        <nav aria-label="Primary" className="hidden items-center gap-7 md:flex">
          {NAV.map((n) => (
            <RollingLink
              key={n.href}
              href={n.href}
              className="mono-xs uppercase tracking-[0.14em] text-fore-2 transition-colors hover:text-fore"
            >
              {n.label}
            </RollingLink>
          ))}
          <RollingLink
            href="https://github.com/PoulavBhowmick03/Erebus"
            external
            className="mono-xs uppercase tracking-[0.14em] text-fore-2 transition-colors hover:text-fore"
          >
            Source
          </RollingLink>
        </nav>

        <button
          type="button"
          onClick={toggle}
          aria-pressed={held}
          className="group flex shrink-0 items-center gap-2.5 border border-rule px-3 py-2 transition-colors hover:border-fore"
          title={
            held
              ? "Drop the key and read the page as a public chain reader"
              : "Take a viewing key and decrypt the record"
          }
        >
          <span
            aria-hidden
            className="inline-block h-2 w-2 transition-colors"
            style={{ background: held ? "var(--color-ink)" : "transparent", border: "1px solid var(--color-ink)" }}
          />
          <span className="mono-xs uppercase tracking-[0.16em]">
            Viewing key <span className="text-fore-3">/</span>{" "}
            <span className="tabular-nums">{held ? "held" : "dropped"}</span>
          </span>
        </button>
      </div>
    </header>
  );
}
