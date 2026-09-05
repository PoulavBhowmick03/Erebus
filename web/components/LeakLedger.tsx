import { LEAKS, doc } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";

export function LeakLedger() {
  return (
    <Section id="leaks" className="pt-24 md:pt-36">
      <div className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Fig. 04 — the privacy boundary</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[22ch] text-[clamp(34px,5.4vw,74px)]">
            Erebus hides the terms,
            <br />
            <span className="italic text-ink-2">not the relationship.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-ink-2">
            Wire v3 encrypts offer terms under AES-256-GCM-SIV and removes wire v2&rsquo;s fixed
            fifth-salt marker. It does not hide transaction timing, pool usage, the note frame, or
            who you opened a channel with. Every row below is reproduced from the privacy model,
            which is the only document in the repository allowed to make a privacy claim.
          </p>
          <a
            href={doc("docs/privacy-model.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-ink-2 underline decoration-rule-2 underline-offset-[6px] hover:text-ink"
          >
            privacy-model.md ↗
          </a>
        </div>
      </div>

      {/* the ledger */}
      <div className="mt-14 overflow-x-auto">
        <table className="w-full min-w-[720px] border-collapse text-left">
          <thead>
            <tr className="border-y border-ink">
              <th className="label !text-ink-3 w-[26%] py-3 pr-6 font-normal">Step</th>
              <th className="label !text-ink-3 w-[37%] py-3 pr-6 font-normal">Hidden</th>
              <th className="label w-[37%] py-3 font-normal" style={{ color: "var(--color-cinnabar)" }}>
                Public
              </th>
            </tr>
          </thead>
          <tbody>
            {LEAKS.map((row) => (
              <tr key={row.step} className="border-b border-rule align-top">
                <td className="py-5 pr-6">
                  <span className="mono-sm uppercase tracking-[0.1em] text-ink">{row.step}</span>
                </td>
                <td className="py-5 pr-6 text-[13px] leading-[1.6] text-ink-2">{row.hidden}</td>
                <td
                  className={`py-5 text-[13px] leading-[1.6] ${
                    row.open === "nothing" ? "text-ink-3" : "leak"
                  } ${row.severe ? "font-medium" : ""}`}
                >
                  {row.open}
                  {row.severe ? (
                    <span className="mono-xs mt-2 block uppercase tracking-[0.14em] opacity-70">
                      F38 — upstream of our encryption. no wire change fixes it.
                    </span>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <p className="mono-xs mt-5 max-w-[70ch] leading-relaxed text-ink-3">
        Steps 6 and 7 produce no chain activity at all. Disclosure is a local read against data
        that is already on chain, which is why a grant costs no gas and leaves no trace.
      </p>

      {/* who sees what. a wire-v3 grant is scoped to one deal, not to the channel. */}
      <div className="mt-16 overflow-x-auto">
        <p className="label mb-5">Fig. 04b — and who sees it</p>
        <table className="w-full min-w-[560px] border-collapse text-left">
          <thead>
            <tr className="border-y border-ink">
              <th className="label !text-ink-3 w-[40%] py-3 pr-6 font-normal">Observer</th>
              <th className="label !text-ink-3 w-[34%] py-3 pr-6 font-normal">Offer terms</th>
              <th className="label !text-ink-3 w-[26%] py-3 font-normal">Traffic shape</th>
            </tr>
          </thead>
          <tbody>
            {[
              ["Public chain reader", "Hidden", false],
              ["Channel party", "Readable", true],
              ["Viewing-grant holder", "Readable for one deal", true],
            ].map(([who, terms, readable]) => (
              <tr key={who as string} className="border-b border-rule">
                <td className="py-4 pr-6 text-[13px]">{who}</td>
                <td className={`py-4 pr-6 text-[13px] ${readable ? "text-ink" : "text-ink-2"}`}>
                  {terms}
                </td>
                <td className="py-4 text-[13px] leak">Visible</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Section>
  );
}
