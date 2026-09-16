import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { DocSection } from "@/components/DocSection";
import { InlineCode } from "@/components/InlineCode";
import { CONCEPTS } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs — Core concepts",
  description: "Channel, offer, deal, note, operation_id, viewing grant — the vocabulary the rest of the docs assume.",
};

export default function Concepts() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Core concepts.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          Six terms the rest of these pages assume. Read this once before the tool reference.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="concepts" n="01" title="Vocabulary">
          <dl className="border-t border-rule">
            {CONCEPTS.map((c) => (
              <div
                key={c.term}
                className="grid grid-cols-1 gap-x-10 gap-y-2 border-b border-rule py-5 md:grid-cols-[11rem_1fr]"
              >
                <dt className="mono-sm text-fore">{c.term}</dt>
                <dd className="prose m-0 max-w-[58ch]">
                  <InlineCode text={c.def} />
                </dd>
              </div>
            ))}
          </dl>
        </DocSection>
      </div>
    </>
  );
}
