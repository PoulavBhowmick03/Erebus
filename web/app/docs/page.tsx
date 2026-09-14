import type { Metadata } from "next";
import { Eyebrow, Header, Section } from "@/components/Chrome";
import { Footer } from "@/components/Footer";
import { Snippet } from "@/components/Snippet";
import { Reveal } from "@/components/Reveal";
import { CALL_PATH, ENV_VARS, INSTALL, MCP_CONFIG, TOOL_GROUPS, doc } from "@/lib/content";

export const metadata: Metadata = {
  title: "Get started — Erebus",
  description:
    "Install the Erebus MCP server, configure an identity, and drive a private negotiation and shielded settlement from any agent framework.",
};

const NEXT = [
  ["runbook.md — clean-machine operator guide", doc("docs/runbook.md")],
  ["reference.md — the tool and SDK surface", doc("docs/reference.md")],
  ["ARCHITECTURE.md — the protocol, end to end", doc("ARCHITECTURE.md")],
  ["status.md — what actually works today", doc("docs/status.md")],
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
        <Section className="pt-16 md:pt-24">
          <Reveal className="max-w-[68ch]">
            <Eyebrow>Get started</Eyebrow>
            <h1 className="display mt-6 mb-0 text-[clamp(36px,6vw,80px)]">
              Run Erebus
              <br />
              <span className="text-fore-3">in ten minutes.</span>
            </h1>
            <p className="lead mt-8">
              Erebus is consumed as MCP tools. Install the server, give it an identity, and any
              client that can set environment can drive a private negotiation and an atomic
              shielded settlement without touching Erebus internals.
            </p>
          </Reveal>

          <div className="mx-auto mt-16 max-w-[1080px] md:mt-20">
            <Step n="01" title="Install">
              <Snippet command={INSTALL} label="install" />
              <p className="prose mt-5 max-w-[62ch]">
                That pulls three packages: <code>erebus-mcp-server</code> (the tool layer),
                <code>erebus-sdk</code> (the Python binding), and the Rust binary as a platform
                wheel. No Rust toolchain is needed. Linux x86-64 and macOS arm64.
              </p>
              <p className="prose mt-4 max-w-[62ch]">
                To try the whole surface with no chain, no keys, and no gas, set{" "}
                <code>EREBUS_BACKEND=mock</code>.
              </p>
            </Step>

            <Step n="02" title="Configure an identity">
              <Snippet command={MCP_CONFIG} label="mcpServers" />
              <p className="prose mt-5 max-w-[62ch]">
                Negotiation has two sides, so register the counterparty as a second entry with its
                own identity, its own state directory, and{" "}
                <code>EREBUS_SETTLEMENT_ROLE=payee</code>. Nothing is shared between them — that is
                the point.
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
                Thirteen tools, Protocol 4. A full negotiation is: <code>open_channel</code>,{" "}
                <code>propose_offer</code>, <code>wait_for_offers</code>, <code>counter_offer</code>,{" "}
                <code>accept_and_settle</code>, then <code>grant_viewing_key</code> and{" "}
                <code>reveal</code> for scoped disclosure.
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
                Key material never crosses upward past the binding. The negotiation policy engine
                decides <em>what</em> to do and never handles keys, which makes that boundary
                enforced rather than conventional.
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
                Unaudited and experimental, with no external security review. Do not put value you
                care about through it.
              </p>
            </Step>
          </div>
        </Section>
      </main>
      <Footer />
    </>
  );
}
