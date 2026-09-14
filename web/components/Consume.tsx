"use client";

import { useState } from "react";
import { FACTS, TOOL_GROUPS, doc } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

const INSTALL = `uv tool install \\
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \\
  erebus-mcp-server`;

const PATH = ["agents", "mcp-server", "sdk/py", "sdk/rs", "Starknet"];

function CopyInstall() {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(INSTALL);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      // clipboard unavailable — the command is still selectable text
    }
  };

  return (
    <button
      type="button"
      onClick={copy}
      className="mono-xs uppercase tracking-[0.16em] transition-colors"
      style={{ color: copied ? "var(--color-fore)" : "var(--color-cinnabar)" }}
    >
      {copied ? "Copied" : "Copy"}
    </button>
  );
}

export function Consume() {
  return (
    <Section id="consume" className="pt-28 md:pt-40">
      <Reveal className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Quickstart</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
            Infrastructure,
            <br />
            <span className="text-fore-3">not a platform.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-fore-2">
            There is no dashboard. Agents are the users, and they consume Erebus as MCP tools and
            SDK calls like anything else. Any framework, in any language, can drive the whole loop
            without touching Erebus internals.
          </p>
          <a
            href={doc("docs/reference.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] hover:text-fore"
          >
            reference.md ↗
          </a>
        </div>
      </Reveal>

      <div className="mt-12 grid grid-cols-1 border border-rule lg:grid-cols-2">
        <div
          className="border-b border-rule p-7 lg:border-b-0 lg:border-r"
          style={{ borderLeft: "2px solid var(--color-cinnabar)" }}
        >
          <div className="mb-5 flex items-center justify-between">
            <p className="label m-0" style={{ color: "var(--color-cinnabar)" }}>
              Install
            </p>
            <CopyInstall />
          </div>
          <pre className="m-0 overflow-x-auto text-[13px] leading-[1.9] text-fore md:text-[14px]">
            <code>{INSTALL}</code>
          </pre>
          <p className="mono-xs mt-6 leading-relaxed text-fore-3">
            Pulls three packages — the tool layer, the Python binding, and the Rust binary as a
            platform wheel. No Rust toolchain needed. Linux x86-64 and macOS arm64.
            <br />
            <br />
            Set <code className="text-fore-2">EREBUS_BACKEND=mock</code> to drive the whole surface
            with no chain, no keys, and no gas.
          </p>
        </div>

        <div className="p-7">
          <p className="label mb-5">Thirteen tools · Protocol 4</p>
          {TOOL_GROUPS.map((group, gi) => (
            <div key={group.label} className={gi > 0 ? "mt-8" : undefined}>
              <p className="mono-xs mb-3 border-b border-rule pb-2 font-semibold uppercase tracking-[0.14em] text-fore">
                {group.label}
              </p>
              <ul className="m-0 grid list-none grid-cols-1 gap-x-8 gap-y-2 p-0 sm:grid-cols-2">
                {group.tools.map((t) => (
                  <li key={t} className="mono-sm flex gap-4 text-fore-2">
                    <span>{t}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </div>

      {/* the call path */}
      <div className="mt-12 border-t border-rule pt-10 pb-2">
        <p className="label mb-8 text-center">
          The call path — Python above the binding, Rust below it
        </p>
        <div className="flex flex-wrap items-center justify-center gap-x-4 gap-y-3 py-4">
          {PATH.map((p, i) => (
            <span key={p} className="flex items-center gap-4">
              <span
                className="border px-4 py-2.5 text-[12px] text-fore-2 transition-shadow"
                style={
                  i === 3
                    ? {
                        borderColor: "var(--color-cinnabar)",
                        color: "var(--color-fore)",
                        boxShadow: "0 0 28px 4px rgba(255, 59, 31, 0.22)",
                      }
                    : i === 0
                      ? { borderColor: "var(--color-fore)", color: "var(--color-fore)", fontWeight: 600 }
                      : { borderColor: "var(--color-rule)" }
                }
              >
                {p}
              </span>
              {i < PATH.length - 1 ? (
                <span
                  className={i === 2 ? "text-[16px]" : "text-fore-3"}
                  style={i === 2 ? { color: "var(--color-cinnabar)" } : undefined}
                >
                  →
                </span>
              ) : null}
            </span>
          ))}
        </div>
        <p className="mono-xs mx-auto mt-5 max-w-[62ch] text-center leading-relaxed text-fore-3">
          You write <span className="text-fore">agents</span>; everything after it is Erebus
          infrastructure. Key material never crosses one arrow — an enforced boundary at{" "}
          <span style={{ color: "var(--color-cinnabar)" }}>sdk/rs</span>, not a convention.
        </p>
      </div>

      {/* facts */}
      <div className="mt-16 flex flex-wrap items-center gap-x-3 gap-y-3 border-t border-rule pt-8">
        {FACTS.map((f, i) => (
          <span key={f.k} className="flex items-center gap-3">
            <span className="mono-xs whitespace-nowrap">
              <span className="uppercase tracking-[0.14em] text-fore-3">{f.k}</span>{" "}
              <span className="text-fore">{f.v}</span>
            </span>
            {i < FACTS.length - 1 ? <span className="text-fore-3">·</span> : null}
          </span>
        ))}
      </div>
    </Section>
  );
}
