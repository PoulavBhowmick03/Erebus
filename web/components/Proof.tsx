import { MANIFEST, MANIFEST_TOTALS, doc, starkscan } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

const short = (h: string) => `${h.slice(0, 10)}…${h.slice(-6)}`;

export function Proof() {
  return (
    <Section id="proof" className="pt-24 md:pt-32">
      <Reveal className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Fig. 01 — the run</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
            Follow the transactions,
            <br />
            <span className="text-fore-3">not the claim.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-fore-2">
            Two screened 1 STRK canaries settled through MCP on Starknet mainnet on 2026-08-31,
            exercising 0.8/0.2 and 0.6/0.4 payment/change splits. No edits, no cuts. Public
            walkthrough of the complete mainnet workflow. Every fee below is the actual receipt
            amount.
          </p>
          <a
            href={doc("docs/runs/2026-08-31-mainnet-060-040-canary.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] hover:text-fore"
          >
            the full run record ↗
          </a>
        </div>
      </Reveal>

      <Reveal className="mt-12" delay={80}>
        <video
          className="w-full border border-rule"
          controls
          preload="metadata"
          playsInline
          aria-label="Walkthrough of the Erebus mainnet workflow"
        >
          <source src="/erebus-final-cut.mp4" type="video/mp4" />
        </video>
      </Reveal>

      {/* the six writes, in order. block and time are public, so they are cinnabar. */}
      <div className="mt-10 overflow-x-auto">
        <ol className="m-0 flex min-w-[820px] list-none gap-px border-t border-rule p-0">
          {MANIFEST.map((r, i) => (
            <li key={r.hash} className="flex-1 border-b border-rule py-5 pr-6">
              <p className="mono-xs m-0 mb-3 flex items-baseline gap-2 uppercase tracking-[0.14em] text-fore-3">
                <span>{String(i + 1).padStart(2, "0")}</span>
                <span>{r.action}</span>
              </p>
              <a
                href={starkscan(r.hash)}
                className="mono-sm tnum text-fore-2 underline decoration-rule-2 underline-offset-[5px] transition-colors hover:text-fore"
              >
                {short(r.hash)} ↗
              </a>
              <p className="mono-xs m-0 mt-3">
                <span className="leak tnum">block {r.block}</span>
                <span className="leak tnum block">{r.utc}</span>
              </p>
            </li>
          ))}
        </ol>
      </div>

      <p className="mono-xs mt-4 max-w-[74ch] leading-relaxed text-fore-3">
        Four of the six are <code className="text-fore-2">apply_actions</code> writes, each paying{" "}
        {MANIFEST_TOTALS.poolFee} on top of the network fee. Network{" "}
        <span className="text-fore-2">{MANIFEST_TOTALS.network}</span>, pool{" "}
        <span className="text-fore-2">{MANIFEST_TOTALS.pool}</span>.
      </p>
    </Section>
  );
}
