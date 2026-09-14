import { LEAKS, doc } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

export function LeakLedger() {
  return (
    <Section id="leaks" className="bg-[#0a0a0c] mt-28 pb-14 md:mt-40 md:pb-20">
      <Reveal className="grid grid-cols-1 gap-8 border-t border-rule pt-14 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16 md:pt-20">
        <div>
          <Eyebrow>The privacy boundary</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
            Erebus hides the terms,
            <br />
            <span className="text-fore-3">not the relationship.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-fore-2">
            Wire v3 encrypts offer terms and hides the negotiation. It does not hide who you
            opened a channel with, or that you opened one at all. Every row below is sourced from
            the privacy model — the only document here allowed to make a privacy claim.
          </p>
          <a
            href={doc("docs/privacy-model.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] hover:text-fore"
          >
            privacy-model.md ↗
          </a>
        </div>
      </Reveal>

      {/* the ledger */}
      <div className="mt-14 overflow-x-auto">
        <table className="w-full min-w-[720px] border-collapse text-left">
          <thead>
            <tr className="border-y border-fore">
              <th className="label !text-fore-3 w-[26%] py-3 pr-6 font-normal">Step</th>
              <th className="label !text-fore-3 w-[37%] py-3 pr-6 font-normal">Hidden</th>
              <th className="label w-[37%] py-3 font-normal" style={{ color: "var(--color-cinnabar)" }}>
                Public
              </th>
            </tr>
          </thead>
          <tbody>
            {LEAKS.map((row) => (
              <tr key={row.step} className="border-b border-rule align-top">
                <td className="py-5 pr-6">
                  <span className="mono-sm uppercase tracking-[0.1em] text-fore">{row.step}</span>
                </td>
                <td className="py-5 pr-6 text-[13px] leading-[1.6] text-fore-2">{row.hidden}</td>
                <td
                  className={`py-5 text-[13px] leading-[1.6] ${
                    row.open === "nothing" ? "text-fore-3" : "leak"
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

      {/* who sees what. a wire-v3 grant is scoped to one deal, not to the channel. */}
      <details className="group mt-16">
        <summary className="label flex w-fit cursor-pointer list-none items-center gap-2 [&::-webkit-details-marker]:hidden">
          <span aria-hidden className="inline-block transition-transform group-open:rotate-90">
            →
          </span>
          And who sees it
        </summary>
        <div className="mt-5 max-w-[70ch]">
          <p className="m-0 text-[13px] leading-[1.8] text-fore-2">
            Traffic shape — <span className="leak">that a deal happened</span> — is visible to
            every observer, key or no key. Offer terms are the only thing a key changes:{" "}
            <span className="text-fore-3">hidden</span> to a public chain reader,{" "}
            <span className="text-fore">readable</span> to the channel party, and{" "}
            <span className="text-fore">readable for one deal</span> to a viewing-grant holder.
          </p>
        </div>
      </details>
    </Section>
  );
}
