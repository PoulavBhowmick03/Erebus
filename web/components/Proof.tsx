import { MANIFEST, MANIFEST_TOTALS, starkscan } from "@/lib/content";
import { Section } from "./Chrome";
import { Reveal } from "./Reveal";

const short = (h: string) => `${h.slice(0, 10)}…${h.slice(-6)}`;

export function Proof() {
  return (
    <Section id="proof" className="pt-24 md:pt-32">
      <Reveal className="section-head">
        <h2 className="display mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
          The mainnet run.
        </h2>
        <div className="flex flex-col justify-end">
          <p className="prose m-0 max-w-[54ch]">
            Two screened 1 STRK canaries settled through MCP on Starknet mainnet on 2026-08-31, at
            0.8/0.2 and 0.6/0.4 payment/change splits. Public walkthrough of the complete mainnet
            workflow. Every fee below is the receipt amount.
          </p>
        </div>
      </Reveal>

      <Reveal className="mt-12" delay={80}>
        <video
          className="aspect-video w-full border border-rule bg-panel"
          controls
          preload="none"
          playsInline
          poster="/erebus-final-cut-poster.jpg"
          aria-label="Walkthrough of the Erebus mainnet workflow"
        >
          <source src="/erebus-final-cut.mp4" type="video/mp4" />
        </video>
      </Reveal>

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
