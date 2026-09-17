"use client";

import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { RollingLink } from "./RollingLink";
import { VERSION_BADGE, DOCS_PAGES } from "@/lib/content";

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
  { href: "https://docs.erebusagents.live", label: "Docs" },
];

const SOURCE_URL = "https://github.com/PoulavBhowmick03/Erebus";

export function Header() {
  const pathname = usePathname();
  const [stuck, setStuck] = useState(false);
  const [mobileOpen, setMobileOpen] = useState(false);

  // In-page anchors only resolve on the landing page; from /docs they have to
  // walk back to the root first.
  const onHome = pathname === "/";
  const onDocs = pathname.startsWith("/docs");
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

  return (
    <header
      className={`fixed inset-x-0 top-0 z-50 px-[var(--edge)] transition-colors duration-300 ${
        stuck || mobileOpen ? "bg-ground/80 backdrop-blur-md" : ""
      }`}
      style={{ borderBottom: `1px solid ${stuck || mobileOpen ? "var(--color-rule)" : "transparent"}` }}
    >
      <div className="mx-auto flex h-14 w-full max-w-[1560px] items-center justify-between gap-6">
        <div className="flex items-center gap-3">
          <a
            href={onHome ? "#top" : onDocs ? "https://erebusagents.live" : "/"}
            className="nav-mark flex items-center"
            title={onDocs ? "Back to erebusagents.live" : undefined}
          >
            <img src="/erebus-lockup.svg" alt="Erebus" className="h-[16px] w-auto sm:h-[18px]" />
          </a>
          {onDocs ? (
            <>
              <span className="mono-xs text-fore-3" aria-hidden>
                /
              </span>
              <RollingLink href="/docs" className="mono-xs uppercase tracking-[0.14em]">
                Docs
              </RollingLink>
              <span
                className="mono-xs hidden px-2 py-1 text-fore-3 sm:inline-block"
                style={{ border: "1px solid rgba(251, 64, 32, 0.3)" }}
              >
                {VERSION_BADGE}
              </span>
            </>
          ) : null}
        </div>

        {onDocs ? null : (
          <nav aria-label="Primary" className="hidden items-center gap-7 md:flex">
            {NAV.map((n) => (
              <span key={n.href} className="nav-mark">
                <RollingLink
                  href={navHref(n.href)}
                  className="mono-xs uppercase tracking-[0.14em]"
                  external={n.href.startsWith("http")}
                >
                  {n.label}
                </RollingLink>
              </span>
            ))}
          </nav>
        )}

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
            className="mono-xs hidden font-medium uppercase tracking-[0.14em] text-fore transition-colors duration-200 hover:font-semibold hover:text-ember md:inline"
          >
            GitHub ↗
          </a>
        </div>
      </div>

      {mobileOpen ? (
        <nav
          id="mobile-nav"
          aria-label="Primary mobile"
          className="flex flex-col border-t border-rule pb-2 md:hidden"
        >
          {onDocs ? (
            <>
              <a
                href="/"
                onClick={() => setMobileOpen(false)}
                className="mono-xs flex min-h-[44px] items-center px-[var(--edge)] uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
              >
                ← Back to site
              </a>
              <p className="label m-0 px-[var(--edge)] pb-1 pt-4 !text-fore-3">Docs</p>
              {DOCS_PAGES.map((p) => (
                <a
                  key={p.href}
                  href={p.href}
                  onClick={() => setMobileOpen(false)}
                  className={`mono-xs flex min-h-[44px] items-center gap-3 px-[var(--edge)] uppercase tracking-[0.16em] transition-colors ${
                    pathname.replace(/\/$/, "") === p.href
                      ? "text-fore"
                      : "text-fore-2 hover:text-fore"
                  }`}
                >
                  <span className="text-fore-3">{p.n}</span>
                  {p.label}
                </a>
              ))}
            </>
          ) : (
            NAV.map((n) => (
              <a
                key={n.href}
                href={navHref(n.href)}
                onClick={() => setMobileOpen(false)}
                className="mono-xs flex min-h-[44px] items-center px-[var(--edge)] uppercase tracking-[0.16em] text-fore-2 transition-colors hover:text-fore"
              >
                {n.label}
              </a>
            ))
          )}
          <a
            href={SOURCE_URL}
            onClick={() => setMobileOpen(false)}
            className="mono-xs flex min-h-[44px] items-center px-[var(--edge)] font-medium uppercase tracking-[0.16em] text-fore transition-colors duration-200 hover:font-semibold hover:text-ember"
          >
            GitHub ↗
          </a>
        </nav>
      ) : null}
    </header>
  );
}
