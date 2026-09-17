import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { Snippet } from "@/components/Snippet";
import { DocSection } from "@/components/DocSection";
import { ENV_VARS, IDENTITY_BOOTSTRAP, IDENTITY_KEYS, INSTALL, MCP_CONFIG } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · Quickstart",
  description:
    "Install the Erebus MCP server, configure an identity, and drive a shielded settlement from any agent framework.",
};

export default function Docs() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(36px,6vw,80px)]">Get started.</h1>
        <p className="lead mt-8 max-w-[56ch]">
          Erebus runs as an MCP server. Install it, give it an identity, and any client that can
          set environment can drive a negotiation and a shielded settlement.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="install" n="01" title="Install">
          <Snippet command={INSTALL} label="install" highlight />
          <p className="prose mt-5 max-w-[62ch]">
            That installs the MCP server, the Python binding, and the Rust binary as a platform
            wheel. No Rust toolchain is needed. Linux x86-64 and macOS arm64.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            <code>--python 3.12</code> is required, not decoration. Without it, <code>uv</code>{" "}
            uses whatever interpreter it finds and won&rsquo;t download one. On a machine whose
            only Python is the system 3.9, the install fails with a dependency error that never
            mentions Python. With the flag, <code>uv</code> fetches a managed 3.12 itself.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            To run everything with no chain, no keys, and no gas, set{" "}
            <code>EREBUS_BACKEND=mock</code>.
          </p>
        </DocSection>

        <DocSection id="identity" n="02" title="Set up an identity">
          <p className="prose max-w-[62ch]">
            An identity is a Starknet account plus two key files, registered with the pool and
            holding shielded notes. Six on-chain steps get you there, and one script does all of
            them:
          </p>
          <div className="mt-5">
            <Snippet command={IDENTITY_BOOTSTRAP} label="bootstrap" accent />
          </div>
          <p className="prose mt-5 max-w-[62ch]">
            That creates the account, funds it, deploys it, generates the pool key and extracts
            the account key, approves the pool for the live per-write fee, then shields 1 STRK,
            which also registers the identity, and runs <code>doctor</code>. It exits non-zero if{" "}
            <code>doctor</code> is not ready. Without a funded account to pay from, use the faucet
            flow instead: <code>create</code>, fund the printed address by hand, then{" "}
            <code>activate</code>. Both are documented in the script&rsquo;s header.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            <strong>Registration is irreversible</strong> and writes the identity&rsquo;s pool
            private key, encrypted, to the pool&rsquo;s auditor on-chain. From that moment the
            auditor can decrypt everything that identity ever does. Use a dedicated low-value
            identity for any mainnet canary.
          </p>

          <p className="prose mt-8 max-w-[62ch]">
            Three keys come out of this, and conflating them is the usual mistake:
          </p>
          <dl className="mt-5 border-t border-rule">
            {IDENTITY_KEYS.map((k) => (
              <div
                key={k.key}
                className="grid grid-cols-1 gap-x-8 gap-y-1 border-b border-rule py-4 sm:grid-cols-[13rem_16rem_1fr]"
              >
                <dt className="mono-sm text-fore">{k.key}</dt>
                <dd className="mono-sm m-0 text-fore-2">{k.purpose}</dd>
                <dd className="mono-xs m-0 text-fore-3">{k.seenBy}</dd>
              </div>
            ))}
          </dl>
          <p className="prose mt-5 max-w-[62ch]">
            Python never sees key material, only file paths.
          </p>
        </DocSection>

        <DocSection id="configure" n="03" title="Configure an identity">
          <Snippet command={MCP_CONFIG} label="mcpServers" />
          <p className="prose mt-5 max-w-[62ch]">
            A negotiation has two sides. Register the counterparty as a second entry with its own
            identity, state directory, and <code>EREBUS_SETTLEMENT_ROLE=payee</code>.
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
          <p className="prose mt-5 max-w-[62ch]">
            <code>doctor</code> checks all of this before a write ever reaches the chain: key
            files and their modes, the state directory, RPC, prover, chain id, registration,
            allowance, and balance, each failing check naming one direct repair. Run it first when
            anything above is in doubt.
          </p>
        </DocSection>
      </div>
    </>
  );
}
