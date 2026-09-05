"use client";

import { NEGOTIATION, SETTLEMENT } from "@/lib/content";
import { NoteLattice } from "./NoteLattice";
import { Secret } from "./Secret";
import { Eyebrow, Section } from "./Chrome";
import { useKey } from "./KeyContext";

export function Hero() {
  const { keyState } = useKey();

  return (
    <Section id="top" className="pt-10 pb-0 md:pt-16">
      <Eyebrow>ΕΛΕΥΣΙΣ · Fig. 01 — one settlement, mainnet, 2026-08-31</Eyebrow>

      <div className="mt-10 grid grid-cols-1 gap-10 lg:grid-cols-[1.35fr_0.65fr] lg:gap-14">
        <div>
          <h1 className="display m-0 text-[clamp(46px,8.4vw,124px)]">
            Negotiate in darkness,
            <br />
            <span className="italic text-ink-2">settle in silence.</span>
          </h1>

          <p className="mt-8 max-w-[58ch] font-[family-name:var(--font-display)] text-[clamp(19px,2vw,27px)] leading-[1.42] text-ink-2">
            Two agents open an encrypted channel carried in privacy-pool note salts, exchange
            structured offers over it, and settle atomically through the shielded pool. A third
            party can be handed one deal afterwards, and nothing else.
          </p>

          <div className="mt-9 flex flex-wrap items-center gap-x-8 gap-y-4">
            <a
              href="#run"
              className="border border-ink bg-ink px-5 py-3 text-[11px] uppercase tracking-[0.18em] text-bone transition-opacity hover:opacity-80"
            >
              Run a deal ↓
            </a>
            <a
              href="#leaks"
              className="mono-xs uppercase tracking-[0.16em] text-ink-2 underline decoration-rule-2 underline-offset-[6px] transition-colors hover:text-ink"
            >
              Read what still leaks
            </a>
            <a
              href="https://github.com/PoulavBhowmick03/Erebus"
              className="mono-xs uppercase tracking-[0.16em] text-ink-2 underline decoration-rule-2 underline-offset-[6px] transition-colors hover:text-ink"
            >
              Source ↗
            </a>
          </div>
        </div>

        {/* the anonymity set */}
        <figure className="m-0 flex min-h-[300px] flex-col lg:min-h-0">
          <div className="relative flex-1 border border-rule plate">
            <NoteLattice className="absolute inset-0" />
            <figcaption className="pointer-events-none absolute inset-x-0 bottom-0 flex items-end justify-between gap-3 p-3">
              <span className="mono-xs uppercase tracking-[0.14em] text-ink-3">
                Fig. 02 — the anonymity set
              </span>
              <span className="leak-tag">7 notes public</span>
            </figcaption>
          </div>
          <p className="mono-xs mt-3 leading-relaxed text-ink-3">
            Every shielded position in STRK20 is a note. A wire-v3 settlement always creates
            seven of them, and that count is public. Which value each one carries is not.
          </p>
        </figure>
      </div>

      {/* ── the record ─────────────────────────────────────────────────── */}

      <div className="mt-16 border-t border-rule pt-4 md:mt-24">
        <div className="flex flex-wrap items-baseline justify-between gap-3">
          <Eyebrow>Fig. 03 — the record of one mainnet deal</Eyebrow>
          <p className="mono-xs m-0 text-ink-3">
            {keyState === "held"
              ? "You hold a viewing key. This is the deal."
              : "You are a public chain reader. Hold a field to see what you actually get."}
          </p>
        </div>

        <dl className="mt-5 grid grid-cols-1 border-t border-rule sm:grid-cols-2 lg:grid-cols-5">
          {SETTLEMENT.map((f, i) => (
            <div
              key={f.label}
              className="border-b border-rule px-0 py-4 sm:px-4 lg:border-l lg:first:border-l-0 lg:[&:nth-child(5n+1)]:border-l-0 lg:[&:nth-child(5n+1)]:pl-0"
            >
              <dt className="mono-xs mb-2 uppercase tracking-[0.14em] text-ink-3">{f.label}</dt>
              <dd className="m-0 text-[13px] leading-snug">
                {f.leaks ? (
                  <span className="leak tnum break-all">{f.value}</span>
                ) : (
                  <Secret value={f.value} delay={i * 40} className="tnum" />
                )}
              </dd>
              <p className="mono-xs mt-2 m-0 leading-snug text-ink-3">
                {f.leaks ? <span className="leak-tag">public</span> : "hidden"}
                {f.note ? <span className="block pt-1 text-ink-3">{f.note}</span> : null}
              </p>
            </div>
          ))}
        </dl>

        <div className="mt-6 flex flex-wrap items-center gap-x-8 gap-y-3 pb-4">
          <span className="mono-xs uppercase tracking-[0.14em] text-ink-3">Path to agreement</span>
          {NEGOTIATION.map((n, i) => (
            <span key={n.step} className="flex items-baseline gap-2">
              <span className="mono-xs text-ink-3">{String(i + 1).padStart(2, "0")}</span>
              <span className="mono-sm text-ink-2">{n.step}</span>
              <Secret value={n.value} className="tnum text-[13px]" delay={400 + i * 60} />
            </span>
          ))}
        </div>
      </div>
    </Section>
  );
}
