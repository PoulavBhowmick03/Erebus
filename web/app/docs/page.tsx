import type { Metadata } from "next";
import { Header, Section } from "@/components/Chrome";
import { Footer } from "@/components/Footer";
import { Snippet } from "@/components/Snippet";
import { Reveal } from "@/components/Reveal";
import { CALL_PATH, ENV_VARS, INSTALL, MCP_CONFIG, TOOL_GROUPS, doc } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs",
  description:
    "Install the Erebus MCP server, configure an identity, and drive a shielded settlement from any agent framework.",
};

const NEXT = [
  ["runbook.md", doc("docs/runbook.md")],
  ["reference.md", doc("docs/reference.md")],
  ["ARCHITECTURE.md", doc("ARCHITECTURE.md")],
  ["status.md", doc("docs/status.md")],
] as const;

function Step({
  n,
  title,
  children,
}: {
  n: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="grid grid-cols-1 gap-6 border-t border-rule py-12 md:grid-cols-[4rem_1fr] md:gap-10">
      <span className="mono-xs pt-2 uppercase tracking-[0.16em] text-fore-3">{n}</span>
      <div>
        <h2 className="display m-0 mb-6 text-[clamp(21px,2.4vw,34px)] leading-[1.05]">{title}</h2>
        {children}
      </div>
    </section>
  );
}

export default function Docs() {
  return (
    <>
      <Header />
      <main>
        <Section className="pt-28 md:pt-36">
          <Reveal className="max-w-[68ch]">
            <h1 className="display mb-0 text-[clamp(36px,6vw,80px)]">Get started.</h1>
            <p className="lead mt-8 max-w-[56ch]">
              Erebus runs as an MCP server. Install it, give it an identity, and any client that can
              set environment can drive a negotiation and a shielded settlement.
            </p>
          </Reveal>

          <div className="mx-auto mt-16 max-w-[1080px] md:mt-20">
            <Step n="01" title="Install">
              <Snippet command={INSTALL} label="install" />
              <p className="prose mt-5 max-w-[62ch]">
                That installs the MCP server, the Python binding, and the Rust binary as a platform
                wheel. No Rust toolchain is needed. Linux x86-64 and macOS arm64.
              </p>
              <p className="prose mt-4 max-w-[62ch]">
                To run everything with no chain, no keys, and no gas, set{" "}
                <code>EREBUS_BACKEND=mock</code>.
              </p>
            </Step>

            <Step n="02" title="Configure an identity">
              <Snippet command={MCP_CONFIG} label="mcpServers" />
              <p className="prose mt-5 max-w-[62ch]">
                A negotiation has two sides. Register the counterparty as a second entry with its
                own identity, state directory, and{" "}
                <code>EREBUS_SETTLEMENT_ROLE=payee</code>.
              </p>

              <dl className="mt-8 border-t border-rule">
                {ENV_VARS.map((e) => (
                  <div
                    key={e.k}
                    className="grid grid-cols-1 gap-x-8 gap-y-1 border-b border-rule py-4 sm:grid-cols-[16rem_10rem_1fr]"
                  >
                    <dt className="mono-sm text-fore">{e.k}</dt>
                    <dd className="mono-sm tnum m-0 text-fore-2">{e.v}</dd>
                    <dd className="mono-xs m-0 text-fore-3">{e.note}</dd>
                  </div>
                ))}
              </dl>
            </Step>

            <Step n="03" title="Call the tools">
              <div className="border-t border-rule">
                {TOOL_GROUPS.map((g) => (
                  <div
                    key={g.label}
                    className="grid grid-cols-1 gap-x-10 gap-y-3 border-b border-rule py-5 md:grid-cols-[14rem_1fr]"
                  >
                    <p className="label m-0 !text-fore-2">{g.label}</p>
                    <ul className="m-0 flex list-none flex-wrap gap-x-5 gap-y-2 p-0">
                      {g.tools.map((t) => (
                        <li key={t} className="mono-sm text-fore-2">
                          {t}
                        </li>
                      ))}
                    </ul>
                  </div>
                ))}
              </div>
              <p className="prose mt-5 max-w-[62ch]">
                A full negotiation is <code>open_channel</code>, <code>propose_offer</code>,{" "}
                <code>wait_for_offers</code>, <code>counter_offer</code>,{" "}
                <code>accept_and_settle</code>, then <code>grant_viewing_key</code> and{" "}
                <code>reveal</code>.
              </p>
            </Step>

            <Step n="04" title="Know the boundary">
              <div className="flex flex-wrap items-center gap-x-4 gap-y-3">
                {CALL_PATH.map((p, i) => (
                  <span key={p} className="flex items-center gap-4">
                    <span
                      className={`border px-4 py-2.5 text-[12px] ${
                        i >= 2 && i <= 3 ? "border-fore text-fore" : "border-rule text-fore-2"
                      }`}
                    >
                      {p}
                    </span>
                    {i < CALL_PATH.length - 1 ? <span className="text-fore-3">→</span> : null}
                  </span>
                ))}
              </div>
              <p className="prose mt-6 max-w-[62ch]">
                Key material never crosses above the binding. The policy engine decides what to do
                and never touches keys.
              </p>
            </Step>

            <Step n="05" title="Read the source of truth">
              <ul className="m-0 list-none space-y-3 p-0">
                {NEXT.map(([label, href]) => (
                  <li key={label}>
                    <a href={href} className="link">
                      {label} ↗
                    </a>
                  </li>
                ))}
              </ul>
              <p className="prose mt-6 max-w-[62ch]">
                Unaudited, with no external security review. Do not use it for value you care about.
              </p>
            </Step>
          </div>
        </Section>
      </main>
      <Footer />
    </>
  );
}
