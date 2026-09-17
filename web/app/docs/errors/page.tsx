import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { Snippet } from "@/components/Snippet";
import { DocSection } from "@/components/DocSection";
import { ERROR_GROUPS, RESPONSE_ERR, RESPONSE_OK } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · Responses and errors",
  description: "The result envelope every tool returns, and what each error group means to do next.",
};

export default function Errors() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Responses and errors.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          One envelope shape, success or failure. Branch on the group, not the code.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="responses" n="01" title="The envelope">
          <p className="prose max-w-[62ch]">
            Every result comes back the same shape, success or failure, with <code>backend</code>{" "}
            (<code>mock</code> or <code>seam</code>) and <code>network</code> on both, so a
            transcript alone says whether a call touched a real chain and which one.
          </p>
          <div className="mt-6 space-y-3">
            <Snippet command={RESPONSE_OK} label="ok" />
            <Snippet command={RESPONSE_ERR} label="error" />
          </div>

          <p className="prose mt-8 max-w-[62ch]">
            Branch on the group below, not the individual code. <code>retryable</code> on the
            error is the thing to trust, not a guess from the name.
          </p>
          <div className="mt-6 border-t border-rule">
            {ERROR_GROUPS.map((e) => (
              <div
                key={e.group}
                className="grid grid-cols-1 gap-x-10 gap-y-2 border-b border-rule py-5 md:grid-cols-[12rem_1fr]"
              >
                <dt className="mono-sm text-fore">{e.group}</dt>
                <dd className="prose m-0 max-w-[62ch]">
                  <span className="flex flex-wrap gap-1.5">
                    {e.codes.map((c) => (
                      <span
                        key={c}
                        className="mono-xs border border-rule px-1.5 py-0.5 text-fore-3"
                      >
                        {c}
                      </span>
                    ))}
                  </span>
                  <span className="mt-3 block">{e.action}</span>
                </dd>
              </div>
            ))}
          </div>
          <p className="prose mt-6 max-w-[62ch]">
            A write takes one to four minutes. Do not create a new <code>operation_id</code> for
            one that appears stuck. Call <code>reconcile</code>, then{" "}
            <code>resume_operation</code> with the original ID once its result permits it.
          </p>
        </DocSection>
      </div>
    </>
  );
}
