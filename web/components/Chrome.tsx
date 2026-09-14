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
  { href: "#proof", label: "Proof" },
  { href: "#how", label: "How it works" },
  { href: "#limits", label: "Limits" },
];

export function Header() {
  const { keyState, toggle } = useKey();
  const [stuck, setStuck] = useState(false);
  const [mobileOpen, setMobileOpen] = useState(false);

  useEffect(() => {
    const onScroll = () => setStuck(window.scrollY > 8);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    if (!mobileOpen) return;
    const onResize = () => {
      if (window.innerWidth >= 768) setMobileOpen(false);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [mobileOpen]);

  const held = keyState === "held";

  return (
    <header
      className={`sticky top-0 z-50 px-[var(--edge)] transition-colors duration-300 ${
        stuck || mobileOpen ? "bg-ground/95" : ""
      }`}
      style={{ borderBottom: `1px solid ${stuck || mobileOpen ? "var(--color-rule)" : "transparent"}` }}
    >
      <div className="mx-auto flex h-14 w-full max-w-[1560px] items-center justify-between gap-6">
        <a href="#top" className="flex items-center gap-3">
          <img src="/erebus-lockup.svg" alt="Erebus" className="h-[15px] w-auto sm:h-[17px]" />
          <span className="mono-xs hidden text-fore-3 lg:inline">private settlement for agents</span>
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

        <div className="flex shrink-0 items-center gap-4">
          <button
            type="button"
            onClick={() => setMobileOpen((o) => !o)}
            aria-expanded={mobileOpen}
            aria-controls="mobile-nav"
            className="mono-xs flex h-11 items-center uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore md:hidden"
          >
            {mobileOpen ? "Close" : "Menu"}
          </button>

          <button
            type="button"
            onClick={toggle}
            aria-pressed={held}
            className="group flex shrink-0 items-center gap-2.5 border px-3 py-2 transition-colors duration-300"
            style={{
              borderColor: held ? "var(--color-fore)" : "var(--color-cinnabar)",
              color: held ? "var(--color-fore)" : "var(--color-cinnabar)",
            }}
            title={
              held
                ? "Drop the key and read the page as a public chain reader"
                : "Take a viewing key and decrypt the record"
            }
          >
            <span className="mono-xs uppercase tracking-[0.16em]">
              <span className="hidden sm:inline" style={{ opacity: 0.7 }}>
                Viewing key /{" "}
              </span>
              <span className="tabular-nums">{held ? "held" : "dropped"}</span>
            </span>
          </button>
        </div>
      </div>

      {mobileOpen ? (
        <nav
          id="mobile-nav"
          aria-label="Primary mobile"
          className="flex flex-col border-t border-rule pb-2 md:hidden"
        >
          {NAV.map((n) => (
            <a
              key={n.href}
              href={n.href}
              onClick={() => setMobileOpen(false)}
              className="mono-xs flex min-h-[44px] items-center uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
            >
              {n.label}
            </a>
          ))}
          <a
            href="https://github.com/PoulavBhowmick03/Erebus"
            onClick={() => setMobileOpen(false)}
            className="mono-xs flex min-h-[44px] items-center uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
          >
            Source ↗
          </a>
        </nav>
      ) : null}
    </header>
  );
}
