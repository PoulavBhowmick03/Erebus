import type { Metadata } from "next";
import { Reveal } from "@/components/Reveal";
import { DocSection } from "@/components/DocSection";
import { CRYPTO_JOBS, WIRE_FIELDS } from "@/lib/content";

export const metadata: Metadata = {
  title: "Erebus docs · How it works",
  description:
    "Where a negotiation actually lives, what one deal does end to end, and what the settlement proof does and does not enforce.",
};

export default function HowItWorks() {
  return (
    <>
      <Reveal className="max-w-[68ch]">
        <h1 className="display mb-0 text-[clamp(28px,4.4vw,56px)]">How it works.</h1>
        <p className="lead mt-6 max-w-[56ch]">
          Two agents agree on a price without publishing it, then pay each other without
          publishing that either. This page explains the mechanism that makes both true at once.
        </p>
      </Reveal>

      <div className="mt-16 md:mt-20">
        <DocSection id="shape" n="01" title="The shape of the problem">
          <p className="prose max-w-[62ch]">
            A negotiation between two agents is a sequence of structured messages: an offer, a
            counter, an acceptance. Settlement is a payment. Doing either one privately is a
            solved problem on its own. Doing both, and binding them together so that the payment
            provably corresponds to the thing that was agreed, is the part that needs design.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Three cryptographic jobs are involved, and conflating them is the usual mistake. Only
            the third one requires a zero-knowledge proof.
          </p>
          <dl className="mt-6 border-t border-rule">
            {CRYPTO_JOBS.map((j) => (
              <div
                key={j.job}
                className="grid grid-cols-1 gap-x-8 gap-y-1 border-b border-rule py-4 md:grid-cols-[13rem_1fr_9rem]"
              >
                <dt className="mono-sm text-fore">{j.job}</dt>
                <dd className="mono-xs m-0 text-fore-2">{j.mechanism}</dd>
                <dd className="mono-xs m-0 text-fore-3">{j.proof}</dd>
              </div>
            ))}
          </dl>
          <p className="prose mt-6 max-w-[62ch]">
            Erebus owns the first two. The third belongs to the STRK20 privacy pool, which already
            exists and is already audited. That division is why there is no Erebus contract
            deployed anywhere: the negotiation rides inside a structure the pool already provides.
          </p>
        </DocSection>

        <DocSection id="salts" n="02" title="Where a negotiation lives">
          <p className="prose max-w-[62ch]">
            A pool note has no payload field. What it does have is a salt, a client-written value
            the contract accepts anywhere in the range <code>2 &le; salt &lt; 2^120</code> and
            stores in the high bits of the note. Nothing in the pool cares what that number means.
            Erebus puts a negotiation message there.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            One message is 400 bits of plaintext, laid out most-significant-first:
          </p>
          <dl className="mt-5 border-t border-rule">
            {WIRE_FIELDS.map((f) => (
              <div
                key={f.field}
                className="grid grid-cols-1 gap-x-8 gap-y-1 border-b border-rule py-3 sm:grid-cols-[10rem_5rem_1fr]"
              >
                <dt className="mono-sm text-fore">{f.field}</dt>
                <dd className="mono-xs tnum m-0 text-fore-2">{f.bits} bits</dd>
                <dd className="mono-xs m-0 text-fore-3">{f.note}</dd>
              </div>
            ))}
          </dl>
          <p className="prose mt-6 max-w-[62ch]">
            That plaintext is encrypted with AES-256-GCM-SIV before it goes anywhere. The current
            wire prepends a 64-bit deal id, which lets one channel carry more than one deal, and
            the resulting ciphertext and authentication tag are split across the salts of five
            notes. Each note carries 119 usable payload bits, so five notes give 595 bits to work
            with. A derived keystream masks the few bits left over, so the spare space does not
            form a recognizable pattern.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Salts are public. Every byte of that ciphertext is readable by anyone. The
            confidentiality comes entirely from the encryption, not from the storage location,
            which is the correct way around: an observer can see that five notes were written and
            cannot tell an offer of 1 STRK from an offer of 500.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            One constraint falls out of this and it is not optional. Structured salts are valid
            only on <strong>zero-amount notes</strong>. A note that carries value needs a random
            salt, because there the salt is the one-time-pad nonce for the encrypted amount.
            Reusing a mask across two different amounts would let an observer subtract the
            ciphertexts and recover the difference. Zero-amount notes carry no amount to leak, so
            they are safe to write structure into.
          </p>
        </DocSection>

        <DocSection id="deal" n="03" title="One deal, end to end">
          <ol className="m-0 mt-1 list-none space-y-5 p-0">
            {[
              [
                "Fund",
                "Shielding moves tokens into the pool as a real ERC-20 transfer, and registers the identity. This step is public in full: depositor, amount, token, and timing.",
              ],
              [
                "Open a channel",
                "Each side opens its own direction to the other. Both directions are required: a one-sided open leaves the counterparty unable to see the offer it is meant to answer.",
              ],
              [
                "Offer and counter",
                "Each message is encrypted, split across five zero-amount note salts, and written to the pool. The counterparty locates it by deriving the same note ids from the shared channel secret, so no scanning of public events is involved.",
              ],
              [
                "Accept and settle",
                "The payer builds a single action set that contains both the acceptance and the payment. Its notes are spent, the payee's note is created, and a change note is created if the inputs overshot the price.",
              ],
              [
                "Disclose, if you want to",
                "A grant hands one deal to one named recipient. It costs no gas and writes nothing to the chain, because it only unlocks data that is already there.",
              ],
            ].map(([title, body], i) => (
              <li key={title} className="grid grid-cols-[2.5rem_1fr] gap-x-5">
                <span className="mono-xs pt-1 text-fore-3">{String(i).padStart(2, "0")}</span>
                <div>
                  <p className="mono-sm m-0 text-fore">{title}</p>
                  <p className="prose mt-2 max-w-[58ch]">{body}</p>
                </div>
              </li>
            ))}
          </ol>
          <p className="prose mt-8 max-w-[62ch]">
            Every one of those writes follows the same path, and there is no faster one. The pool
            is an account contract, so the client simulates the action set locally, sends it for
            proving, and submits the resulting proof through <code>apply_actions</code>. A code
            path that skips proof generation is not an optimization, it is a bug.
          </p>
        </DocSection>

        <DocSection id="settle" n="04" title="What settlement enforces">
          <p className="prose max-w-[62ch]">
            Settlement is one action set, and the chain either applies all of it or none of it.
            The acceptance and the payment cannot come apart: there is no window in which one
            agent is committed and the other is not, because there is only one state transition.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            Be precise about what that atomicity buys, because it is narrower than it sounds. The
            pool&rsquo;s proof enforces the things the pool understands: that the spent notes
            exist, that the spender controls them, that inputs equal outputs plus fees, that no
            note is spent twice. Those are real guarantees and they are proof-backed.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            The check that the amount paid equals the amount accepted is{" "}
            <strong>a client-side check in the Rust SDK</strong>, not a predicate the STRK20
            circuit evaluates. The circuit does not know what a negotiation is. A counterparty
            running modified client code cannot steal your notes, since spending still requires
            your key, but the binding between &ldquo;this is the price we agreed&rdquo; and
            &ldquo;this is the amount that moved&rdquo; is enforced by software you are running,
            not by the chain. Pushing that check into the settlement verifier is known work and it
            is not done.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            A settlement creates six notes when the payer&rsquo;s inputs match the price exactly,
            and seven when they overshoot and a change note is minted.
          </p>
        </DocSection>

        <DocSection id="disclose" n="05" title="Disclosure afterwards">
          <p className="prose max-w-[62ch]">
            Confidentiality that cannot be selectively undone is not much use for anything
            involving an auditor, a counterparty dispute, or a regulator. A grant derives the read
            capability for exactly one deal in one direction, encrypts it to a recipient&rsquo;s
            registered pool key, and binds an expiry.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            The scope is the point. A recipient holding a grant for one deal cannot read the other
            deals in the same channel, and cannot spend anything, because spending requires the
            owner&rsquo;s pool private key and no grant contains one. This was demonstrated on
            mainnet: a third account reconstructed one deal from a scoped grant and could not read
            the two earlier deals in the same channel.
          </p>
          <p className="prose mt-4 max-w-[62ch]">
            What a disclosed record does and does not establish is worth reading in full before
            you rely on it. See <a href="/docs/privacy#record" className="link">the privacy model</a>.
          </p>
        </DocSection>
      </div>
    </>
  );
}
