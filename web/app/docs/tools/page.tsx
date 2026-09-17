import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { DocSection } from "@/components/DocSection";
import { TOOL_DETAILS, TOOL_GROUPS, VERSION_NOTE } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · Call the tools",
  description: "The thirteen MCP tools: signatures, notes, and the full negotiation sequence.",
};

export default function Tools() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Call the tools.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          Thirteen tools, Protocol 4. Amounts are decimal strings, <code>memo_hash</code> is hex.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="tools" n="01" title="The tool surface">
          <div className="border-t border-rule">
            {TOOL_GROUPS.map((g) => (
              <div key={g.label} className="border-b border-rule py-6">
                <p className="label m-0 mb-4 !text-fore-2">{g.label}</p>
                <div className="space-y-4">
                  {g.tools.map((t) => (
                    <div
                      key={t}
                      className="grid grid-cols-1 gap-x-8 gap-y-1 xl:grid-cols-[11rem_19rem_1fr]"
                    >
                      <span className="mono-sm text-fore">{t}</span>
                      <span className="mono-xs tnum text-fore-3">{TOOL_DETAILS[t]?.signature}</span>
                      <span className="mono-xs text-fore-3">{TOOL_DETAILS[t]?.note}</span>
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>
          <p className="prose mt-5 max-w-[62ch]">
            A full negotiation is <code>open_channel</code>, <code>propose_offer</code>,{" "}
            <code>wait_for_offers</code>, <code>counter_offer</code>,{" "}
            <code>accept_and_settle</code>, then <code>grant_viewing_key</code> and{" "}
            <code>reveal</code>.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Every write takes <code>operation_id</code>. Persist it and the intent before the
            call, and reuse both after a restart. See <code>operation_id</code> under core
            concepts.
          </p>
          <p className="prose mt-4 max-w-[62ch]">{VERSION_NOTE}</p>
        </DocSection>
      </div>
    </>
  );
}
