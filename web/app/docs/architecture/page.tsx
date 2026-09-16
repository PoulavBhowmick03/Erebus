import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { Snippet } from "@/components/Snippet";
import { DocSection } from "@/components/DocSection";
import { SystemMap } from "@/components/SystemMap";
import {
  BUILD_CLONE,
  BUILD_PYTHON,
  BUILD_RUST,
  CALL_PATH,
  CLI_METHODS,
  CLI_REQUEST,
  CLI_RESPONSE,
  SYSTEM_MAP_ALT,
  doc,
} from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs — Architecture",
  description: "The call path key material never crosses, and where to read the rest.",
};

const NEXT = [
  ["runbook.md", doc("docs/runbook.md")],
  ["reference.md", doc("docs/reference.md")],
  ["ARCHITECTURE.md", doc("ARCHITECTURE.md")],
  ["status.md", doc("docs/status.md")],
] as const;

export default function Architecture() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Architecture.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          Where key material stops, and where to read the rest.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="map" n="01" title="System map">
          <p className="prose max-w-[62ch]">
            One page, the whole system: the stack from agent to pool, the three layers, a deal
            start to finish, how an offer becomes five notes, who sees what, and what the system
            does and does not claim.
          </p>
          <div className="mt-5">
            <SystemMap src="/erebus-overview.svg" alt={SYSTEM_MAP_ALT} />
          </div>
        </DocSection>

        <DocSection id="boundary" n="02" title="Know the boundary">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-3">
            {CALL_PATH.map((p, i) => {
              const isFirst = i === 0;
              const isBoundary = p === "sdk/rs";
              const arrowCrossesBoundary = CALL_PATH[i + 1] === "sdk/rs";
              return (
                <span key={p} className="flex items-center gap-4">
                  <span
                    className={`border px-4 py-2.5 text-[12px] ${
                      isBoundary
                        ? "boundary-mark font-semibold"
                        : isFirst
                          ? "border-fore font-semibold text-fore"
                          : "border-rule text-fore-2"
                    }`}
                  >
                    {p}
                  </span>
                  {i < CALL_PATH.length - 1 ? (
                    <span className={arrowCrossesBoundary ? "text-ember" : "text-fore-3"}>→</span>
                  ) : null}
                </span>
              );
            })}
          </div>
          <p className="prose mt-6 max-w-[62ch]">
            You write <code>agents</code>; everything after it is Erebus infrastructure. Key
            material never crosses one arrow — an enforced boundary at{" "}
            <code style={{ color: "var(--color-ember)" }}>sdk/rs</code>, not a convention. The
            policy engine decides what to do and never touches keys.
          </p>
        </DocSection>

        <DocSection id="cli" n="03" title="The CLI protocol">
          <p className="prose max-w-[62ch]">
            <code>erebus-cli</code> reads one JSON request on stdin and writes one envelope on
            stdout. Key <em>paths</em> cross the boundary; key values never do.
          </p>
          <div className="mt-5 space-y-3">
            <Snippet command={CLI_REQUEST} label="request" />
            <Snippet command={CLI_RESPONSE} label="response" />
          </div>
          <p className="prose mt-6 max-w-[62ch]">
            <code>protocol</code> is the contract version. A consumer should refuse a mismatch by
            name rather than failing on a changed shape later — <code>erebus-sdk</code> does this
            on every call, and the MCP server handshakes at startup.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            <span className="mono-xs block text-fore-3">{CLI_METHODS}</span>
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            All except <code>version</code> and <code>generate_pool_key</code> take a{" "}
            <code>config</code> object. Every chain write also takes <code>operation_id</code>.
          </p>
        </DocSection>

        <DocSection id="build" n="04" title="Build from source">
          <Snippet command={BUILD_CLONE} label="clone" accent />
          <div className="mt-3 space-y-3">
            <Snippet command={BUILD_RUST} label="rust" />
            <Snippet command={BUILD_PYTHON} label="python" />
          </div>
          <p className="prose mt-6 max-w-[62ch]">
            <code>uv sync</code> without <code>--all-packages</code> skips the workspace
            members&rsquo; editable installs, and the <code>erebus-*</code> packages will not be
            importable.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            The TypeScript SDK is a differential-test oracle and ships nothing; it needs a sibling
            checkout of <code>starkware-libs/starknet-privacy</code>. Toolchain: scarb 2.17.0 /
            starknet-foundry 0.59.0, Node 20+, Rust stable.
          </p>
        </DocSection>

        <DocSection id="source" n="05" title="Read the source of truth">
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
        </DocSection>
      </div>
    </>
  );
}
