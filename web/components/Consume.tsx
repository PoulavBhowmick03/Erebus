import { FACTS, TOOLS, doc } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";

const INSTALL = `uv tool install \\
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \\
  erebus-mcp-server`;

const PATH = ["agents", "mcp-server", "sdk/py", "sdk/rs", "Starknet"];

export function Consume() {
  return (
    <Section id="consume" className="pt-24 md:pt-36">
      <div className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Fig. 09 — the tool surface</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[18ch] text-[clamp(34px,5.4vw,74px)]">
            Infrastructure,
            <br />
            <span className="italic text-ink-2">not a platform.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-ink-2">
            There is no dashboard. Agents are the users, and they consume Erebus as MCP tools and
            SDK calls the same way they consume anything else. Any framework in any language can
            drive the whole loop without touching Erebus internals.
          </p>
          <a
            href={doc("docs/reference.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-ink-2 underline decoration-rule-2 underline-offset-[6px] hover:text-ink"
          >
            reference.md ↗
          </a>
        </div>
      </div>

      <div className="mt-12 grid grid-cols-1 border border-rule lg:grid-cols-2">
        <div className="border-b border-rule p-7 lg:border-b-0 lg:border-r">
          <p className="label mb-5">Install</p>
          <pre className="m-0 overflow-x-auto text-[12px] leading-[1.8] text-ink">
            <code>{INSTALL}</code>
          </pre>
          <p className="mono-xs mt-6 leading-relaxed text-ink-3">
            Pulls three packages — the tool layer, the Python binding, and the Rust binary as a
            platform wheel. No Rust toolchain needed. Linux x86-64 and macOS arm64.
            <br />
            <br />
            Set <code className="text-ink-2">EREBUS_BACKEND=mock</code> to drive the whole surface
            with no chain, no keys, and no gas.
          </p>
        </div>

        <div className="p-7">
          <p className="label mb-5">Thirteen tools · Protocol 4</p>
          <ul className="m-0 grid list-none grid-cols-1 gap-x-8 gap-y-2 p-0 sm:grid-cols-2">
            {TOOLS.map((t, i) => (
              <li key={t} className="mono-sm flex gap-4 text-ink-2">
                <span className="w-5 shrink-0 text-ink-3">{String(i + 1).padStart(2, "0")}</span>
                <span>{t}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>

      {/* the call path */}
      <div className="mt-12 border-t border-rule pt-6">
        <p className="label mb-6">The call path — Python above the binding, Rust below it</p>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-3">
          {PATH.map((p, i) => (
            <span key={p} className="flex items-center gap-4">
              <span
                className={`border px-4 py-2.5 text-[12px] ${
                  i >= 2 && i <= 3 ? "border-ink text-ink" : "border-rule text-ink-2"
                }`}
              >
                {p}
              </span>
              {i < PATH.length - 1 ? <span className="text-ink-3">→</span> : null}
            </span>
          ))}
        </div>
        <p className="mono-xs mt-5 max-w-[74ch] leading-relaxed text-ink-3">
          Key material never crosses upward past the binding, which makes that boundary an
          enforced one rather than a convention.
        </p>
      </div>

      {/* facts */}
      <dl className="mt-16 grid grid-cols-2 border-t border-rule sm:grid-cols-3 lg:grid-cols-6">
        {FACTS.map((f) => (
          <div key={f.k} className="border-b border-rule py-5 pr-6 lg:border-l lg:pl-5 lg:first:border-l-0 lg:first:pl-0">
            <dt className="mono-xs mb-2 uppercase tracking-[0.14em] text-ink-3">{f.k}</dt>
            <dd className="m-0 text-[13px] text-ink">{f.v}</dd>
          </div>
        ))}
      </dl>
    </Section>
  );
}
