"use client";

import { usePathname } from "next/navigation";
import { DOCS_PAGES } from "@/lib/content";

/** Prev/next between docs pages, in reading order. */
const norm = (p: string) => (p.length > 1 ? p.replace(/\/$/, "") : p);

export function DocsPager() {
  const pathname = norm(usePathname());
  const i = DOCS_PAGES.findIndex((p) => p.href === pathname);
  if (i === -1) return null;

  const prev = DOCS_PAGES[i - 1];
  const next = DOCS_PAGES[i + 1];
  if (!prev && !next) return null;

  return (
    <div className="mt-4 grid grid-cols-1 gap-4 border-t border-rule pt-8 sm:grid-cols-2">
      {prev ? (
        <a
          href={prev.href}
          className="pager-link group block border border-rule px-5 py-4 hover:-translate-x-1"
        >
          <span className="mono-xs block text-fore-3">← Previous</span>
          <span className="label mt-2 block !text-fore group-hover:text-ember">{prev.label}</span>
        </a>
      ) : (
        <span />
      )}
      {next ? (
        <a
          href={next.href}
          className="pager-link group block border border-rule px-5 py-4 text-right hover:translate-x-1"
        >
          <span className="mono-xs block text-fore-3">Next →</span>
          <span className="label mt-2 block !text-fore group-hover:text-ember">{next.label}</span>
        </a>
      ) : (
        <span />
      )}
    </div>
  );
}
