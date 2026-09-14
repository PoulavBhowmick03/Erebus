import { INSTALL, SOURCE } from "@/lib/content";
import { NoteLattice } from "./NoteLattice";
import { Snippet } from "./Snippet";
import { Eyebrow, Section } from "./Chrome";

/**
 * The page opens the way a tool does: the thing you would type first, then the
 * one deal that shows what it buys you. The lattice is the same diagram as
 * before — it draws only what leaks — kept as a bounded figure rather than
 * atmosphere, because a background that implies more privacy than exists would
 * be the page lying.
 */
export function Hero() {
  return (
    <Section id="top" className="pt-8 md:pt-14">
      <Eyebrow>Erebus 4 · STRK20 privacy pool · Starknet</Eyebrow>

      <div className="mt-8 grid grid-cols-1 gap-12 lg:grid-cols-[1.45fr_0.55fr] lg:gap-16">
        <div>
          <h1 className="display m-0 text-[clamp(38px,6.4vw,96px)]">
            Negotiate in darkness,
            <br />
            <span className="text-fore-3">settle in silence</span>
          </h1>

          <p className="mt-7 max-w-[54ch] font-[family-name:var(--font-display)] text-[clamp(15px,1.3vw,19px)] font-normal leading-[1.5] text-fore-2">
            Private coordination and shielded settlement for AI agents. Two agents negotiate over
            an encrypted channel carried in privacy-pool note salts, then settle atomically. A
            third party can be handed one deal afterwards, and nothing else.
          </p>

          <div className="mt-8 max-w-[620px]">
            <Snippet command={INSTALL} label="install" />
            <p className="mono-xs mt-3 leading-relaxed text-fore-3">
              Three packages — the MCP server, the Python binding, and the Rust binary as a platform
              wheel. Set EREBUS_BACKEND=mock to drive the whole surface with no chain, no keys, and
              no gas.
            </p>
          </div>

          <div className="mt-9 flex flex-wrap items-center gap-x-8 gap-y-4">
            <a
              href="#proof"
              className="border border-fore bg-fore px-5 py-3 text-[11px] uppercase tracking-[0.18em] text-ground transition-opacity hover:opacity-80"
            >
              Watch it run ↓
            </a>
            <a
              href="#how"
              className="mono-xs uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] transition-colors hover:text-fore"
            >
              How it works ↓
            </a>
            <a
              href={SOURCE}
              className="mono-xs uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] transition-colors hover:text-fore"
            >
              Source ↗
            </a>
          </div>
        </div>

        <figure className="m-0 flex min-h-[300px] flex-col lg:min-h-0">
          <div className="relative flex-1 border border-rule plate">
            <NoteLattice className="absolute inset-0" story />
            <figcaption className="pointer-events-none absolute inset-x-0 bottom-0 flex flex-wrap items-end justify-between gap-x-4 gap-y-1 p-3">
              <span className="mono-xs uppercase tracking-[0.14em] text-fore-3">
                the pool, one deal
              </span>
              <span className="leak-tag">7 notes public</span>
            </figcaption>
          </div>
          <p className="mono-xs mt-3 leading-relaxed text-fore-3">
            Every shielded position in STRK20 is a note; the drifting field is that set. The deal
            draws <em className="not-italic text-fore-2">only what leaks</em> — the pair, the
            crossings, seven settlement notes, one scoped grant. No amount ever appears, and
            nothing here changes when you drop the key.
          </p>
        </figure>
      </div>
    </Section>
  );
}
