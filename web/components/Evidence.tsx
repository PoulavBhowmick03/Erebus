import { MANIFEST, MANIFEST_TOTALS, doc, starkscan } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

const short = (h: string) => `${h.slice(0, 10)}…${h.slice(-6)}`;

export function Evidence() {
  return (
    <Section id="evidence" className="pt-24 md:pt-36">
      <Reveal className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Fig. 07 — evidence manifest</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
            Follow the transactions,
            <br />
            <span className="text-fore-3">not the claim.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-fore-2">
            Two screened 1 STRK canaries settled through MCP on Starknet mainnet on 2026-08-31,
            exercising 0.8/0.2 and 0.6/0.4 payment/change splits. The six transactions below are
            the second one, end to end. Every fee is the actual receipt amount.
          </p>
          <a
            href={doc("docs/runs/2026-08-31-mainnet-060-040-canary.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] hover:text-fore"
          >
            the full run record ↗
          </a>
        </div>
      </Reveal>

      <div className="mt-14 overflow-x-auto">
        <table className="w-full min-w-[780px] border-collapse text-left">
          <thead>
            <tr className="border-y border-fore">
              <th className="label !text-fore-3 py-3 pr-6 font-normal">Action</th>
              <th className="label !text-fore-3 py-3 pr-6 font-normal">Transaction</th>
              <th className="label !text-fore-3 py-3 pr-6 text-right font-normal">Block</th>
              <th className="label !text-fore-3 py-3 pr-6 text-right font-normal">UTC</th>
              <th className="label !text-fore-3 py-3 text-right font-normal">Fee, STRK</th>
            </tr>
          </thead>
          <tbody>
            {MANIFEST.map((r) => (
              <tr key={r.hash} className="group border-b border-rule">
                <td className="py-4 pr-6 text-[13px]">{r.action}</td>
                <td className="py-4 pr-6">
                  <a
                    href={starkscan(r.hash)}
                    className="mono-sm tnum text-fore-2 underline decoration-rule-2 underline-offset-[5px] transition-colors hover:text-fore"
                  >
                    {short(r.hash)} ↗
                  </a>
                </td>
                <td className="mono-sm tnum py-4 pr-6 text-right leak">{r.block}</td>
                <td className="mono-sm tnum py-4 pr-6 text-right leak">{r.utc}</td>
                <td className="mono-sm tnum py-4 text-right text-fore-2">{r.fee}</td>
              </tr>
            ))}
          </tbody>
          <tfoot>
            <tr>
              <td colSpan={2} className="py-4 pr-6 text-[13px] text-fore-3">
                Four of the six are <code className="text-fore-2">apply_actions</code> writes, each
                paying {MANIFEST_TOTALS.poolFee} on top of the network fee.
              </td>
              <td colSpan={2} className="mono-xs py-4 pr-6 text-right uppercase tracking-[0.14em] text-fore-3">
                network / pool
              </td>
              <td className="mono-sm tnum py-4 text-right">
                {MANIFEST_TOTALS.network} <span className="text-fore-3">/</span>{" "}
                {MANIFEST_TOTALS.pool}
              </td>
            </tr>
          </tfoot>
        </table>
      </div>

      <div className="mt-12 grid grid-cols-1 gap-px border border-rule bg-rule sm:grid-cols-3">
        {[
          {
            k: "Three-minute evidence video",
            v: "Public three-minute walkthrough of the complete mainnet workflow. It links both screened canaries, recovery, observer limits, and scoped disclosure.",
            href: "https://erebus-private-agents.vercel.app/erebus-private-sprint.mp4",
            cta: "Watch ↗",
          },
          {
            k: "Reproduce it yourself",
            v: "A clean-machine operator guide: install, identity, hosted proving, shielding, negotiation, settlement, recovery, observer inspection, disclosure, shutdown.",
            href: doc("docs/runbook.md"),
            cta: "runbook.md ↗",
          },
          {
            k: "Where the stack fought us",
            v: "Forty-two entries. What we tried, what the stack did instead, whether we worked around it, and what would have made it easier. Kept honest on purpose.",
            href: doc("docs/friction.md"),
            cta: "friction.md ↗",
          },
        ].map((c, i) => (
          <Reveal key={c.k} delay={i * 110} className="flex flex-col justify-between bg-ground p-7">
            <div>
              <p className="label mb-4">{c.k}</p>
              <p className="m-0 text-[13px] leading-[1.7] text-fore-2">{c.v}</p>
            </div>
            <a
              href={c.href}
              className="mono-xs mt-8 w-fit uppercase tracking-[0.16em] text-fore underline decoration-rule-2 underline-offset-[6px] hover:decoration-fore"
            >
              {c.cta}
            </a>
          </Reveal>
        ))}
      </div>
    </Section>
  );
}
