import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { DocSection } from "@/components/DocSection";
import { NOT_DOES, PROD_GAPS, doc } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · Limits",
  description:
    "What Erebus does not do, what is unfinished before production, and which workloads it actually fits.",
};

export default function Limits() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Limits.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          Erebus v0.2.0 is a published, mainnet-verified technical preview. It is not ready for
          material real value, and this page is specific about why.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="not" n="01" title="What Erebus does not do">
          <p className="prose max-w-[62ch]">
            These are design boundaries rather than open bugs. Four of the five cannot be fixed
            client-side, so no amount of care in your agent code works around them.
          </p>
          <div className="mt-8 space-y-8">
            {NOT_DOES.map((n) => (
              <div key={n.title}>
                <p className="mono-sm m-0 text-fore">{n.title}</p>
                <p className="prose mt-2 max-w-[58ch]">{n.body}</p>
              </div>
            ))}
          </div>
          <p className="prose mt-8 max-w-[62ch]">
            The escrow limit is the one that most often changes a design. Because settlement is a
            single atomic action set, the pool offers no timelock and no conditional release,
            there is no point at which funds sit committed but undelivered. That makes Erebus a
            good fit for work that is verifiable at the moment of payment, and a poor fit for
            anything needing delivery-versus-payment.
          </p>
        </DocSection>

        <DocSection id="production" n="02" title="What is unfinished">
          <dl className="border-t border-rule">
            {PROD_GAPS.map((g) => (
              <div
                key={g.area}
                className="grid grid-cols-1 gap-x-10 gap-y-2 border-b border-rule py-5 md:grid-cols-[13rem_1fr]"
              >
                <dt className="mono-sm text-fore">{g.area}</dt>
                <dd className="prose m-0 max-w-[58ch]">{g.body}</dd>
              </div>
            ))}
          </dl>
          <p className="prose mt-6 max-w-[62ch]">
            The security line is worth restating on its own. <strong>No independent
            cryptographic or security review</strong> covers the wire, the settlement binding, the
            disclosure design, the hosted-prover transport, or the recovery journal. Four bounded
            mainnet runs demonstrate that the workflow completes. They do not establish capacity,
            uptime, or safety under adversarial conditions.
          </p>
        </DocSection>

        <DocSection id="use" n="03" title="Where that leaves you">
          <p className="prose max-w-[62ch]">
            Reasonable uses today are bounded and low-frequency: evaluating the protocol,
            developing against <code>mock</code>, running a demo, or a small canary on testnet
            with value you are willing to lose. Every mainnet run so far has been deliberately
            bounded in exactly this way.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Use a dedicated low-value identity for anything touching mainnet. Registration is
            irreversible and permanently places that identity inside the auditor&rsquo;s view, and
            your choice of prover permanently places it inside that provider&rsquo;s view.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            If your use case depends on hiding the relationship rather than the terms, on escrow,
            or on proving delivery, the honest answer is that Erebus does not do those things and
            adding them is not a configuration change.
          </p>
          <p className="prose mt-8 max-w-[62ch]">
            The current gap list is{" "}
            <a href={doc("docs/production-gaps.md")} className="link">
              production-gaps.md ↗
            </a>
            , and the tiebreaker for current state is{" "}
            <a href={doc("docs/status.md")} className="link">
              status.md ↗
            </a>
            . Where any page here disagrees with those, they are right and this is stale.
          </p>
        </DocSection>
      </div>
    </>
  );
}
