"use client";

import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { RollingLink } from "./RollingLink";
import { useKey } from "./KeyContext";
import { InstallPill } from "./InstallPill";

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
  { href: "/docs", label: "Docs" },
];

const SOURCE_URL = "https://github.com/PoulavBhowmick03/Erebus";

export function Header() {
  const { keyState, toggle } = useKey();
  const pathname = usePathname();
  const [stuck, setStuck] = useState(false);
  const [mobileOpen, setMobileOpen] = useState(false);

  // In-page anchors only resolve on the landing page; from /docs they have to
  // walk back to the root first.
  const onHome = pathname === "/";
  const navHref = (href: string) => (onHome || !href.startsWith("#") ? href : `/${href}`);

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
        stuck || mobileOpen ? "bg-ground/80 backdrop-blur-md" : ""
      }`}
      style={{ borderBottom: `1px solid ${stuck || mobileOpen ? "var(--color-rule)" : "transparent"}` }}
    >
      <div className="mx-auto flex h-14 w-full max-w-[1560px] items-center justify-between gap-6">
        <a href={onHome ? "#top" : "/"} className="flex items-center">
          <img src="/erebus-lockup.svg" alt="Erebus" className="h-[16px] w-auto sm:h-[18px]" />
        </a>

        <nav aria-label="Primary" className="hidden items-center gap-7 md:flex">
          {NAV.map((n) => (
            <RollingLink
              key={n.href}
              href={navHref(n.href)}
              className="mono-xs uppercase tracking-[0.14em] text-fore-2 transition-colors hover:text-fore"
            >
              {n.label}
            </RollingLink>
          ))}
        </nav>

        <InstallPill className="hidden max-w-[300px] lg:flex" />

        <div className="flex shrink-0 items-center gap-6">
          <button
            type="button"
            onClick={() => setMobileOpen((o) => !o)}
            aria-expanded={mobileOpen}
            aria-controls="mobile-nav"
            className="mono-xs flex h-11 items-center uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore md:hidden"
          >
            {mobileOpen ? "Close" : "Menu"}
          </button>

          <a
            href={SOURCE_URL}
            className="mono-xs hidden uppercase tracking-[0.14em] text-fore-2 transition-colors hover:text-fore md:inline"
          >
            GitHub ↗
          </a>

          {/* A status LED, not a button in a box. Cinnabar while the key is
              dropped, because that is the state in which the page shows what a
              chain reader sees. Hidden on /docs, where there is no record. */}
          {onHome ? (
            <button
              type="button"
              onClick={toggle}
              aria-pressed={held}
              className="group hidden shrink-0 items-center gap-2.5 transition-colors duration-300 md:flex"
              style={{ color: held ? "var(--color-fore-2)" : "var(--color-cinnabar)" }}
              title={
                held
                  ? "Drop the key and read the page as a public chain reader"
                  : "Take a viewing key and decrypt the record"
              }
            >
              <span
                aria-hidden
                className="inline-block h-1.5 w-1.5"
                style={{ background: "currentColor" }}
              />
              <span className="mono-xs uppercase tracking-[0.18em] transition-colors group-hover:text-fore">
                key {held ? "held" : "dropped"}
              </span>
            </button>
          ) : null}
        </div>
      </div>

      {mobileOpen ? (
        <nav
          id="mobile-nav"
          aria-label="Primary mobile"
          className="flex flex-col border-t border-rule pb-2 md:hidden"
        >
          <div className="py-3">
            <InstallPill className="w-full" />
          </div>
          {NAV.map((n) => (
            <a
              key={n.href}
              href={navHref(n.href)}
              onClick={() => setMobileOpen(false)}
              className="mono-xs flex min-h-[44px] items-center uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
            >
              {n.label}
            </a>
          ))}
          <a
            href={SOURCE_URL}
            onClick={() => setMobileOpen(false)}
            className="mono-xs flex min-h-[44px] items-center uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
          >
            GitHub ↗
          </a>
          {onHome ? (
            <button
              type="button"
              onClick={() => {
                toggle();
                setMobileOpen(false);
              }}
              className="mono-xs flex min-h-[44px] items-center gap-2.5 uppercase tracking-[0.16em]"
              style={{ color: held ? "var(--color-fore-2)" : "var(--color-cinnabar)" }}
            >
              <span
                aria-hidden
                className="inline-block h-1.5 w-1.5"
                style={{ background: "currentColor" }}
              />
              key {held ? "held" : "dropped"}
            </button>
          ) : null}
        </nav>
      ) : null}
    </header>
  );
}
