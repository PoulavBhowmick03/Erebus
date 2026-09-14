import { SOURCE, TOOL_GROUPS, doc } from "@/lib/content";
import { Section } from "./Chrome";
import { FooterMark } from "./FooterMark";

const TOOL_LINE = TOOL_GROUPS.flatMap((g) => g.tools).join(" · ");

export function Footer() {
  return (
    <>
      <Section className="pt-24 pb-10 md:pt-36">
        <div className="grid grid-cols-1 gap-10 border-t border-fore pt-8 md:grid-cols-[1.2fr_0.9fr_0.9fr]">
          <div>
            <p className="label !text-fore mb-4 !tracking-[0.34em]">Erebus</p>
            <p className="prose m-0 max-w-[40ch]">
              Private coordination and shielded settlement for AI agents, composed from
              StarkWare&rsquo;s STRK20 privacy pool. Apache-2.0, matching the primitives it builds
              on.
            </p>
            <p className="mono-xs mt-6 max-w-[48ch] leading-relaxed text-fore-3">
              Thirteen MCP tools · Protocol 4
              <span className="mt-2 block text-fore-3">{TOOL_LINE}</span>
            </p>
          </div>

          <div>
            <p className="label mb-4">Read</p>
            <ul className="m-0 list-none space-y-2 p-0">
              {[
                ["status.md — the tiebreaker", doc("docs/status.md")],
                ["privacy-model.md", doc("docs/privacy-model.md")],
                ["threat-model.md", doc("docs/threat-model.md")],
                ["friction.md", doc("docs/friction.md")],
                ["runbook.md", doc("docs/runbook.md")],
              ].map(([label, href]) => (
                <li key={label}>
                  <a
                    href={href}
                    className="mono-sm text-fore-2 underline decoration-transparent underline-offset-[5px] transition hover:text-fore hover:decoration-rule-2"
                  >
                    {label} ↗
                  </a>
                </li>
              ))}
            </ul>
          </div>

          <div>
            <p className="label mb-4">Built by</p>
            <ul className="m-0 list-none space-y-2 p-0 text-[13px] text-fore-2">
              <li>Poulav Bhowmick — protocol, Cairo, Starknet</li>
              <li>Ishita — agents, orchestration, ML</li>
            </ul>
            <a
              href={SOURCE}
              className="mono-xs mt-5 inline-block uppercase tracking-[0.16em] text-fore underline decoration-rule-2 underline-offset-[6px]"
            >
              github ↗
            </a>
          </div>
        </div>

        <div className="mono-xs mt-12 flex flex-wrap justify-between gap-4 border-t border-rule pt-5 text-fore-3">
          <span>Erebus · Apache-2.0 · unaudited and experimental</span>
          <span>Built on Starknet and STRK20</span>
        </div>
      </Section>

      <FooterMark />
    </>
  );
}
