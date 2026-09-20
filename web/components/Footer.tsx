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
            <div className="mt-5 flex items-center gap-4">
              <a href={SOURCE} aria-label="GitHub" className="text-fore-2 transition hover:text-fore">
                <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" aria-hidden>
                  <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.57.1.78-.25.78-.55 0-.27-.01-1.16-.02-2.1-3.2.7-3.87-1.36-3.87-1.36-.53-1.33-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.75.4-1.25.73-1.54-2.55-.29-5.24-1.28-5.24-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.05 0 0 .97-.31 3.18 1.18a11.02 11.02 0 0 1 5.79 0c2.2-1.49 3.17-1.18 3.17-1.18.63 1.59.24 2.76.12 3.05.74.8 1.18 1.83 1.18 3.08 0 4.41-2.69 5.38-5.25 5.67.41.36.78 1.06.78 2.14 0 1.54-.01 2.79-.01 3.17 0 .3.2.66.79.55A10.52 10.52 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z" />
                </svg>
              </a>
              <a href={X_HANDLE} aria-label="X" className="text-fore-2 transition hover:text-fore">
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden>
                  <path d="M18.24 2.5h3.05l-6.67 7.62 7.84 10.38h-6.15l-4.81-6.3-5.5 6.3H2.15l7.14-8.16L1.8 2.5h6.3l4.35 5.76 5.79-5.76Zm-1.07 16.2h1.69L7.12 4.2H5.3l11.87 14.5Z" />
                </svg>
              </a>
              <a href="mailto:erebus.privacy@gmail.com" aria-label="Email" className="text-fore-2 transition hover:text-fore">
                <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden>
                  <rect x="2.5" y="4.5" width="19" height="15" rx="1.5" />
                  <path d="m3.5 6 8.5 7 8.5-7" />
                </svg>
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
