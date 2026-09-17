import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { Snippet } from "@/components/Snippet";
import { DocSection } from "@/components/DocSection";
import { SETTLE_RESULT, WALK_COUNTER, WALK_OPEN, WALK_SETTLE, WALK_SHIELD, doc } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · Walkthrough",
  description:
    "One deal from funding to disclosure, with the commands, the two-sided open, and the two costs people forget to budget for.",
};

export default function Walkthrough() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">Walkthrough.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          One deal, start to finish, between two identities you control. Run it against{" "}
          <code>mock</code> first, where nothing costs anything and no key is needed.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="before" n="01" title="Before you start">
          <p className="prose max-w-[62ch]">
            You need two identities, because a negotiation has two sides and each one settles from
            its own notes. Follow <a href="/docs" className="link">the quickstart</a> twice, once
            per identity, into two separate state directories. Then shield funds on the payer
            side, which also registers the identity and makes it available as a counterparty.
          </p>
          <div className="mt-5">
            <Snippet command={WALK_SHIELD} label="shield" accent />
          </div>
          <p className="prose mt-5 max-w-[62ch]">
            If the approval is too fresh the shield can fail, because the proof is built against a
            historical block. Wait for the approval to reach proving depth before retrying.
          </p>

          <p className="prose mt-8 max-w-[62ch]">
            Two costs are easy to miss when budgeting a first run, and both have caught operators
            out:
          </p>
          <ul className="mt-4 max-w-[62ch] list-none space-y-3 p-0">
            <li className="prose m-0 border-l border-rule pl-5">
              The payee needs an allowance for <strong>two</strong> charged writes, its own open
              and its counter, even though it never pays the price. Opening a direction is an{" "}
              <code>apply_actions</code> transaction, not a read.
            </li>
            <li className="prose m-0 border-l border-rule pl-5">
              The pool fee is pulled from the identity&rsquo;s <strong>public</strong> STRK
              balance. A healthy shielded balance does not cover it, and neither does a large
              allowance. <code>doctor</code> reports this case as a <code>gas_balance</code>{" "}
              warning.
            </li>
          </ul>
        </DocSection>

        <DocSection id="negotiate" n="02" title="Negotiate">
          <p className="prose max-w-[62ch]">
            <strong>Both sides open their own direction.</strong> This is not optional and it is
            the single most common first-run mistake. A one-sided open leaves the counterparty
            with <code>unknown channel handle</code>, and a payee that never opens cannot see the
            offer it is meant to answer.
          </p>
          <div className="mt-6 space-y-3">
            <Snippet command={WALK_OPEN} label="payer" />
            <Snippet command={WALK_COUNTER} label="payee" />
          </div>
          <p className="prose mt-6 max-w-[62ch]">
            The payer calls <code>get_note_balance</code> before naming a price, because it is the
            side that has to cover it. Amounts here are base units, so the values above are 0.6
            and 1.0 STRK. The exact prices and deadlines belong in your agent policy. What matters
            for the shape of the run is the lifecycle, not the numbers.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Every write takes an <code>operation_id</code>. Persist it, and the intent behind it,
            before the call rather than after.
          </p>
        </DocSection>

        <DocSection id="settle" n="03" title="Settle">
          <p className="prose max-w-[62ch]">
            Only the payer settles, because <code>accept_and_settle</code> spends the
            caller&rsquo;s notes. A payee leaves its final offer for the payer to accept.
          </p>
          <div className="mt-5">
            <Snippet command={WALK_SETTLE} label="settle" accent />
          </div>
          <p className="prose mt-5 max-w-[62ch]">
            Settlement consumes the payer&rsquo;s notes, creates the payee&rsquo;s note, returns
            change if the inputs overshot, and leaves the channel pair open for later deals. The
            result carries:
          </p>
          <div className="mt-4 flex flex-wrap gap-2">
            {SETTLE_RESULT.map((f) => (
              <span key={f} className="mono-xs border border-rule px-2 py-1 text-fore-2">
                {f}
              </span>
            ))}
          </div>
          <p className="prose mt-6 max-w-[62ch]">
            A write takes one to four minutes. Most of that is proving.
          </p>
        </DocSection>

        <DocSection id="disclose" n="04" title="Disclose one deal">
          <p className="prose max-w-[62ch]">
            <code>grant_viewing_key</code> writes a grant file scoped to one deal and one
            registered recipient, with an explicit expiry. <code>reveal</code> reads it back and
            reconstructs that deal. Neither touches the chain, so neither costs gas.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Four properties are worth stating to whoever receives the grant: it is scoped to one
            deal, it has an expiry, it carries no spending authority, and the recipient&rsquo;s
            pool key must match the one the grant names. Opening a disclosure does not erase what
            was already disclosed.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            To see the negative control, run the observer script against the same public chain
            record and compare. It should recover nothing from the current wire, while still
            showing that the relationship and the timing remain public.
          </p>
        </DocSection>

        <DocSection id="recover" n="05" title="When a write looks stuck">
          <p className="prose max-w-[62ch]">
            Recovery is explicit and operator-driven. The rule that matters: do not mint a new{" "}
            <code>operation_id</code> for a write that looks stuck. That is how you pay twice.
          </p>
          <ol className="mt-5 max-w-[62ch] list-none space-y-3 p-0">
            <li className="prose m-0 border-l border-rule pl-5">
              Call <code>reconcile</code> first. It is read-only and never submits anything.
            </li>
            <li className="prose m-0 border-l border-rule pl-5">
              Call <code>resume_operation</code> only when the classification says the operation
              is resumable, keeping the original id.
            </li>
            <li className="prose m-0 border-l border-rule pl-5">
              If the result says the proof expired, rebuild under the same id, and only when the
              journal permits it.
            </li>
          </ol>
          <p className="prose mt-6 max-w-[62ch]">
            The full clean-machine guide, including the faucet path and what to record from a run,
            is{" "}
            <a href={doc("docs/runbook.md")} className="link">
              runbook.md ↗
            </a>
            .
          </p>
        </DocSection>
      </div>
    </>
  );
}
