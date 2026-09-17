import { SOURCE, X_HANDLE, doc } from "@/lib/content";
import { Section } from "./Chrome";
import { FooterMark } from "./FooterMark";

export function Footer() {
  return (
    <>
      <Section className="pt-24 pb-10 md:pt-36">
        <div className="grid grid-cols-1 gap-10 border-t border-fore pt-8 md:grid-cols-[1.2fr_0.9fr_0.9fr]">
          <div>
            <p className="label !text-fore mb-4 !tracking-[0.34em]">Erebus</p>
            <p className="prose m-0 max-w-[40ch]">
              Private coordination and shielded settlement for AI agents on Starknet, built on
              StarkWare&rsquo;s STRK20 pool. Apache-2.0.
            </p>
          </div>

          <div>
            <p className="label mb-4">Read</p>
            <ul className="m-0 list-none space-y-2 p-0">
              {[
                ["status.md", doc("docs/status.md")],
                ["privacy-model.md", doc("docs/privacy-model.md")],
                ["threat-model.md", doc("docs/metropolis-threat-model.md")],
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
              <li>Poulav Bhowmick, protocol and Cairo</li>
              <li>Ishita, agents and orchestration</li>
            </ul>
            <div className="mt-5 flex gap-5">
              <a
                href={SOURCE}
                className="mono-xs inline-block uppercase tracking-[0.16em] text-fore underline decoration-rule-2 underline-offset-[6px]"
              >
                github ↗
              </a>
              <a
                href={X_HANDLE}
                className="mono-xs inline-block uppercase tracking-[0.16em] text-fore underline decoration-rule-2 underline-offset-[6px]"
              >
                x ↗
              </a>
            </div>
          </div>
        </div>

        <div className="mono-xs mt-12 flex flex-wrap justify-between gap-4 border-t border-rule pt-5 text-fore-3">
          <span>Apache-2.0. Unaudited and experimental.</span>
          <span>Built on Starknet and STRK20</span>
        </div>
      </Section>

      <FooterMark />
    </>
  );
}
