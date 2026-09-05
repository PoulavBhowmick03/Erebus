import { SOURCE, doc } from "@/lib/content";
import { Section } from "./Chrome";

export function Footer() {
  return (
    <Section className="pt-24 pb-16 md:pt-36">
      <div className="grid grid-cols-1 gap-10 border-t border-ink pt-8 md:grid-cols-[1.2fr_0.9fr_0.9fr]">
        <div>
          <p className="label !text-ink mb-4 !tracking-[0.34em]">Erebus</p>
          <p className="m-0 max-w-[38ch] text-[13px] leading-[1.7] text-ink-2">
            Private coordination and shielded settlement for AI agents, composed from
            StarkWare&rsquo;s STRK20 privacy pool. Apache-2.0, matching the primitives it builds on.
          </p>
        </div>

        <div>
          <p className="label mb-4">Read</p>
          <ul className="m-0 list-none space-y-2 p-0">
            {[
              ["status.md — the tiebreaker", doc("docs/status.md")],
              ["privacy-model.md", doc("docs/privacy-model.md")],
              ["ARCHITECTURE.md", doc("ARCHITECTURE.md")],
              ["runbook.md", doc("docs/runbook.md")],
            ].map(([label, href]) => (
              <li key={label}>
                <a
                  href={href}
                  className="mono-sm text-ink-2 underline decoration-transparent underline-offset-[5px] transition hover:text-ink hover:decoration-rule-2"
                >
                  {label} ↗
                </a>
              </li>
            ))}
          </ul>
        </div>

        <div>
          <p className="label mb-4">Built by</p>
          <ul className="m-0 list-none space-y-2 p-0 text-[13px] text-ink-2">
            <li>Poulav Bhowmick — protocol, Cairo, Starknet</li>
            <li>Ishita — agents, orchestration, ML</li>
          </ul>
          <a
            href={SOURCE}
            className="mono-xs mt-5 inline-block uppercase tracking-[0.16em] text-ink underline decoration-rule-2 underline-offset-[6px]"
          >
            github ↗
          </a>
        </div>
      </div>

      <div className="mono-xs mt-12 flex flex-wrap justify-between gap-4 border-t border-rule pt-5 text-ink-3">
        <span>Erebus · Apache-2.0 · unaudited and experimental</span>
        <span>Built on Starknet and STRK20</span>
      </div>
    </Section>
  );
}
