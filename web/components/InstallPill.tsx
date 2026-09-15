"use client";

import { useState } from "react";
import { INSTALL } from "@/lib/content";

const SHORT = "uv tool install erebus-mcp-server";

/**
 * The install command, one click away from anywhere on the site.
 *
 * This is a dev tool — the install command is the single most load-bearing
 * line on the page, and burying it in the hero means anyone who scrolls past
 * it once has to hunt for it again. The header shows a trimmed, readable
 * form; a click copies the real command (with the package index flag) so the
 * shortened label is never what lands in a terminal.
 */
export function InstallPill({ className = "" }: { className?: string }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(INSTALL);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };

  return (
    <button
      type="button"
      onClick={copy}
      aria-live="polite"
      title="Copy the install command"
      className={`group flex items-center gap-3 border border-rule-2 py-1.5 pl-3 pr-2.5 transition-colors hover:border-ember ${className}`}
    >
      <span className="mono-xs truncate text-fore-2 transition-colors group-hover:text-fore">
        <span className="text-fore-3">$</span> {SHORT}
      </span>
      <span
        className="mono-xs shrink-0 uppercase tracking-[0.14em] transition-colors"
        style={{ color: copied ? "var(--color-ember)" : "var(--color-fore-3)" }}
      >
        {copied ? "copied" : "copy"}
      </span>
    </button>
  );
}
