"use client";

import { useEffect, useRef, useState } from "react";
import { usePathname, useRouter } from "next/navigation";
import { DOCS_PAGES, PAGE_SECTIONS, SEARCH_INDEX } from "@/lib/content";

/**
 * The left rail across every /docs page: search on top, the page list below
 * it, current page marked with the same ember tick each page opens with.
 */
const norm = (p: string) => (p.length > 1 ? p.replace(/\/$/, "") : p);

export function DocsToc() {
  const pathname = norm(usePathname());
  const router = useRouter();
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const sections = PAGE_SECTIONS[pathname] ?? [];
  const [activeSection, setActiveSection] = useState(sections[0]?.id);
  const [hoveredHref, setHoveredHref] = useState<string | null>(null);

  useEffect(() => {
    setActiveSection(sections[0]?.id);
    if (sections.length === 0) return;
    const targets = sections
      .map((s) => document.getElementById(s.id))
      .filter((el): el is HTMLElement => el !== null);

    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setActiveSection(visible[0].target.id);
      },
      { rootMargin: "-112px 0px -70% 0px", threshold: 0 },
    );

    targets.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pathname]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        inputRef.current?.focus();
      }
      if (e.key === "Escape") {
        setOpen(false);
        inputRef.current?.blur();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, []);

  const q = query.trim().toLowerCase();
  const results = q
    ? SEARCH_INDEX.filter(
        (r) => r.title.toLowerCase().includes(q) || r.snippet.toLowerCase().includes(q),
      ).slice(0, 8)
    : [];

  const go = (href: string) => {
    setOpen(false);
    setQuery("");
    router.push(href);
  };

  // Plain <a href> tags do a hard browser navigation whenever the path
  // doesn't match exactly (e.g. trailingSlash makes "/docs" and "/docs/"
  // different documents), which is what made clicking a sub-section jump to
  // the top of a freshly reloaded page instead of scrolling to it. Route
  // every internal click through the router instead: same page scrolls in
  // place, a different page gets a client-side transition, never a reload.
  const navigate = (e: React.MouseEvent, targetPage: string, id?: string) => {
    e.preventDefault();
    const dest = id ? `${targetPage}#${id}` : targetPage;
    if (norm(targetPage) === pathname && id) {
      document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
      window.history.replaceState(null, "", dest);
      return;
    }
    router.push(dest);
  };

  return (
    <nav aria-label="Docs" className="hidden lg:block">
      <div className="sticky top-28">
        <div ref={boxRef} className="relative">
          <div className="flex items-center gap-2 border border-rule px-3 py-2">
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setOpen(true);
              }}
              onFocus={() => setOpen(true)}
              type="text"
              placeholder="Search docs"
              aria-label="Search docs"
              className="mono-xs w-full bg-transparent text-fore outline-none placeholder:text-fore-3"
            />
            <span className="mono-xs shrink-0 text-fore-3">⌘K</span>
          </div>

          {open && results.length > 0 ? (
            <ul className="absolute left-0 right-0 top-[calc(100%+6px)] z-10 m-0 list-none border border-rule bg-panel p-0">
              {results.map((r) => (
                <li key={r.title} className="border-b border-rule last:border-b-0">
                  <button
                    type="button"
                    onClick={() => go(r.href)}
                    className="group block w-full px-3 py-2.5 text-left transition-colors hover:bg-panel-2"
                  >
                    <span className="mono-xs block text-fore group-hover:text-ember">{r.title}</span>
                    <span className="mono-xs mt-1 block text-fore-3">{r.snippet}</span>
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
        </div>

        <p className="label m-0 mb-4 mt-8">Pages</p>
        <ul className="m-0 list-none space-y-3 border-l border-rule p-0">
          {DOCS_PAGES.map((p) => {
            const isActive = pathname === p.href;
            const pageSections = PAGE_SECTIONS[p.href] ?? [];
            const expanded = pageSections.length > 1 && hoveredHref === p.href;
            return (
              <li
                key={p.href}
                className={isActive ? "-ml-px border-l-2 border-ember" : undefined}
                onMouseEnter={() => setHoveredHref(p.href)}
                onMouseLeave={() => setHoveredHref(null)}
                onFocus={() => setHoveredHref(p.href)}
                onBlur={(e) => {
                  if (!e.currentTarget.contains(e.relatedTarget as Node)) setHoveredHref(null);
                }}
              >
                <a
                  href={p.href}
                  onClick={(e) => navigate(e, p.href)}
                  aria-current={isActive ? "page" : undefined}
                  aria-expanded={pageSections.length > 1 ? expanded : undefined}
                  className={`mono-xs flex gap-3 py-0.5 pl-4 uppercase tracking-[0.1em] transition-all duration-200 hover:translate-x-1 ${
                    isActive ? "text-fore" : "text-fore-2 hover:text-fore"
                  }`}
                >
                  <span className={isActive ? "text-ember" : "text-fore-3"}>{p.n}</span>
                  {p.label}
                </a>
                {pageSections.length > 1 ? (
                  <div
                    className={`grid transition-all duration-200 ease-out ${
                      expanded ? "mt-2 grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0"
                    }`}
                  >
                    <ul className="m-0 list-none space-y-1.5 overflow-hidden border-l border-rule p-0">
                      {pageSections.map((s) => {
                        const isCurrent = isActive && s.id === activeSection;
                        return (
                          <li
                            key={s.id}
                            className={isCurrent ? "-ml-px border-l-2 border-ember" : undefined}
                          >
                            <a
                              href={`${p.href}#${s.id}`}
                              onClick={(e) => navigate(e, p.href, s.id)}
                              className={`mono-xs block py-0.5 pl-7 normal-case tracking-normal transition-all duration-200 hover:translate-x-1 ${
                                isCurrent ? "text-fore" : "text-fore-3 hover:text-fore-2"
                              }`}
                            >
                              {s.label}
                            </a>
                          </li>
                        );
                      })}
                    </ul>
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      </div>
    </nav>
  );
}
