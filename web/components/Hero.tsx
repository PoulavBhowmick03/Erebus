import { INSTALL, SOURCE } from "@/lib/content";
import { Snippet } from "./Snippet";
import { NoteLattice } from "./NoteLattice";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

/**
 * A statement, then the two halves of the argument under it.
 *
 * The headline gets the full measure so it can hold at display size on one
 * breath; below it, the left column says what Erebus is and how to start it,
 * and the right column is the pool, drawn, with one deal moving through it.
 * `/docs` is the only filled button on the page.
 */
export function Hero() {
  return (
    <Section id="top" className="hero-floor pt-14 md:pt-24">
      <Reveal>
        <Eyebrow>Erebus 4 · STRK20 privacy pool · Starknet</Eyebrow>

        <h1 className="display mt-8 mb-0 text-[clamp(30px,6.4vw,94px)]">
          Negotiate in darkness,
          <br />
          <span className="text-fore-3">settle in silence</span>
        </h1>

        <div className="mt-14 grid grid-cols-1 gap-12 border-t border-rule pt-10 lg:grid-cols-[1fr_0.82fr] lg:gap-16">
          <div>
            <p className="lead max-w-[48ch]">
              Private coordination and shielded settlement for AI agents. Two agents negotiate over
              an encrypted channel carried in privacy-pool note salts, then settle atomically. A
              third party can be handed one deal afterwards, and nothing else.
            </p>

            <div className="mt-10 max-w-[540px]">
              <Snippet command={INSTALL} label="install" />
              <p className="mono-xs mt-3 leading-relaxed text-fore-3">
                Three packages — the MCP server, the Python binding, and the Rust binary as a
                platform wheel. EREBUS_BACKEND=mock drives the whole surface with no chain, no keys,
                and no gas.
              </p>

              <div className="mt-7 flex flex-wrap items-center gap-x-8 gap-y-4">
                <a href="/docs" className="btn">
                  Get started <span aria-hidden>→</span>
                </a>
                <a href={SOURCE} className="link">
                  Source ↗
                </a>
              </div>
            </div>
          </div>

          <figure className="m-0">
            <div className="relative h-[300px] border border-rule plate sm:h-[360px] lg:h-[420px]">
              <NoteLattice className="absolute inset-0" story />
              <figcaption className="pointer-events-none absolute inset-x-0 bottom-0 flex items-end justify-between gap-4 p-3">
                <span className="mono-xs uppercase tracking-[0.14em] text-fore-3">
                  the pool, one deal
                </span>
                <span className="leak-tag">7 notes public</span>
              </figcaption>
            </div>
            <p className="mono-xs mt-3 max-w-[52ch] leading-relaxed text-fore-3">
              Every shielded position in STRK20 is a note; the drifting field is that set. The deal
              draws <em className="not-italic text-fore-2">only what leaks</em> — the pair, the
              crossings, seven settlement notes, one scoped grant.
            </p>
          </figure>
        </div>
      </Reveal>
    </Section>
  );
}
