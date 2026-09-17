import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { DocSection } from "@/components/DocSection";
import { InlineCode } from "@/components/InlineCode";
import {
  DISCLOSURE_ASSERTS,
  DISCLOSURE_PROVES,
  INFRA_VISIBILITY,
  KNOWN_LEAKS,
  LEAK_STEPS,
  PRIVACY_CLAIM,
  PRIVACY_NONCLAIM,
  PRIVACY_ONE_LINE,
  doc,
} from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · Privacy model",
  description:
    "What Erebus hides, what it does not, and the known leaks in descending severity. The canonical statement.",
};

export default function Privacy() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Privacy model.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          What is actually hidden, what is not, and where the edges are. Read this before you
          decide whether Erebus fits your threat model.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="claim" n="01" title="The claim and the non-claim">
          <p className="label m-0 mb-3 !text-fore-2">The claim</p>
          <p className="prose max-w-[62ch]">{PRIVACY_CLAIM}</p>

          <p className="label m-0 mb-3 mt-8 !text-fore-2">The non-claim</p>
          <p className="prose max-w-[62ch]">{PRIVACY_NONCLAIM}</p>

          <p
            className="mt-8 max-w-[52ch] border-l-2 py-1 pl-5 text-[clamp(17px,1.9vw,23px)] leading-[1.35] text-fore"
            style={{ borderColor: "var(--color-ember)" }}
          >
            {PRIVACY_ONE_LINE}
          </p>
          <p className="prose mt-6 max-w-[62ch]">
            Never describe this as private in an absolute sense. If a decision depends on an
            observer not knowing that two parties transacted, Erebus does not give you that today.
          </p>
        </DocSection>

        <DocSection id="steps" n="02" title="What leaks at each step">
          <dl className="border-t border-rule">
            {LEAK_STEPS.map((s) => (
              <div
                key={s.step}
                className="grid grid-cols-1 gap-x-8 gap-y-2 border-b border-rule py-5 lg:grid-cols-[13rem_1fr_1fr]"
              >
                <dt className="mono-sm text-fore">{s.step}</dt>
                <dd className="mono-xs m-0 text-fore-2">
                  <span className="label m-0 mb-1 block !text-fore-3">Hidden</span>
                  {s.hidden}
                </dd>
                <dd className="mono-xs m-0 text-fore-3">
                  <span className="label m-0 mb-1 block !text-fore-3">Public</span>
                  {s.open}
                </dd>
              </div>
            ))}
          </dl>
          <p className="prose mt-6 max-w-[62ch]">
            Granting and revealing produce no chain activity at all. Disclosure is a local read
            against data that is already on chain, which is why a grant costs no gas and leaves no
            trace.
          </p>
        </DocSection>

        <DocSection id="leaks" n="03" title="The known leaks">
          <p className="prose max-w-[62ch]">
            In descending severity. These are not hypothetical: each one is either measured in
            this repository or visible in the upstream contract source.
          </p>
          <div className="mt-8 space-y-10">
            {KNOWN_LEAKS.map((l) => (
              <div key={l.n} className="grid grid-cols-[2.5rem_1fr] gap-x-5">
                <span className="mono-xs pt-1 text-ember">{l.n}</span>
                <div>
                  <p className="mono-sm m-0 text-fore">{l.title}</p>
                  <p className="prose mt-3 max-w-[58ch]">
                    <InlineCode text={l.body} />
                  </p>
                  <p className="mono-xs mt-3 max-w-[58ch] text-fore-3">{l.fix}</p>
                </div>
              </div>
            ))}
          </div>
        </DocSection>

        <DocSection id="infra" n="04" title="Infrastructure sees more than the chain">
          <p className="prose max-w-[62ch]">
            The chain is not the only observer, and this is the part most readers miss. The pool
            is an account contract simulated locally, so the compiled action set embeds the pool
            private key in its calldata. Two endpoints receive that calldata.
          </p>
          <dl className="mt-6 border-t border-rule">
            {INFRA_VISIBILITY.map((e) => (
              <div
                key={e.endpoint}
                className="grid grid-cols-1 gap-x-8 gap-y-1 border-b border-rule py-4 md:grid-cols-[13rem_1fr_9rem]"
              >
                <dt className="mono-sm text-fore">{e.endpoint}</dt>
                <dd className="mono-xs m-0 text-fore-2">{e.how}</dd>
                <dd className="mono-xs m-0 text-fore-3">{e.key}</dd>
              </div>
            ))}
          </dl>
          <p className="prose mt-6 max-w-[62ch]">
            Both can therefore reconstruct that identity&rsquo;s full history. The exposure
            aggregates across users on a shared prover, and it is permanent: there is no rotation
            and no revocation. Choosing a hosted prover means choosing to put that provider inside
            your confidentiality boundary. Self-hosting removes the provider and adds node,
            storage, screening, and uptime work.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            The submitted transaction does not carry the key. That part of the design holds.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Separately, registration writes your pool private key, encrypted, to a single
            pool-wide auditor key. It is set once, it covers everything that identity ever does,
            and it is not something you grant. It happens the moment you register, and it cannot
            be undone. Use a dedicated low-value identity for anything on mainnet.
          </p>
        </DocSection>

        <DocSection id="record" n="05" title="What a disclosed record proves">
          <p className="label m-0 mb-3 !text-fore-2">It proves</p>
          <p className="prose max-w-[62ch]">
            <InlineCode text={DISCLOSURE_PROVES} />
          </p>
          <p className="label m-0 mb-3 mt-8 !text-fore-2">It only asserts</p>
          <p className="prose max-w-[62ch]">
            <InlineCode text={DISCLOSURE_ASSERTS} />
          </p>
          <p className="prose mt-8 max-w-[62ch]">
            Atomicity is narrower than semantic proof. The acceptance and the payment share one
            action set, but the amount-equality check is client-side validation, not a statement
            that the STRK20 circuit understands the negotiation.
          </p>
          <p className="prose mt-6 max-w-[62ch]">
            The canonical version of this page, with the contract line references behind each
            claim, is{" "}
            <a href={doc("docs/privacy-model.md")} className="link">
              privacy-model.md ↗
            </a>
            . Where the two disagree, that file is right and this page is stale.
          </p>
        </DocSection>
      </div>
    </>
  );
}
