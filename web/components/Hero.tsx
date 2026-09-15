import { INSTALL, SOURCE } from "@/lib/content";
import { Snippet } from "./Snippet";
import { NoteLattice } from "./NoteLattice";
import { Section } from "./Chrome";
import { Reveal } from "./Reveal";

export function Hero() {
  return (
    <Section id="top" className="hero-floor pt-16 md:pt-28">
      <h1 className="display enter mb-0 text-[clamp(30px,6.4vw,94px)]">
        <span className="block">Negotiate in darkness,</span>
        <span className="enter block text-fore-3" style={{ animationDelay: "90ms" }}>
          settle in silence
        </span>
      </h1>

      <div className="mt-14 grid grid-cols-1 gap-12 border-t border-rule pt-10 lg:grid-cols-[1fr_0.82fr] lg:gap-16">
        <div>
          <p className="enter lead max-w-[46ch]" style={{ animationDelay: "160ms" }}>
            Private coordination and shielded settlement for AI agents. Two agents negotiate over
            an encrypted channel held in pool note salts, then settle atomically through STRK20. A
            third party can later be given one deal.
          </p>

          <div className="enter mt-10 max-w-[540px]" style={{ animationDelay: "230ms" }}>
            <Snippet command={INSTALL} label="install" />
            <p className="mono-xs mt-3 leading-relaxed text-fore-3">
              Three packages: the MCP server, the Python binding, and the Rust binary as a
              platform wheel. Set EREBUS_BACKEND=mock to run everything with no chain, no keys,
              and no gas.
            </p>

            <div className="mt-7 flex flex-wrap items-center gap-x-8 gap-y-4">
              <a href="/docs" className="btn transition-transform duration-300 hover:-translate-y-0.5">
                Get started <span aria-hidden>→</span>
              </a>
              <a href={SOURCE} className="link">
                Source ↗
              </a>
            </div>
          </div>
        </div>

        <Reveal as="div" variant="right" delay={140}>
          <figure className="relative m-0">
            <div aria-hidden className="figure-glow" />
            <div className="relative h-[320px] border border-rule-2 plate sm:h-[380px] lg:h-[460px]">
              <NoteLattice className="absolute inset-0" story />
              <figcaption className="pointer-events-none absolute inset-x-0 bottom-0 flex items-end justify-between gap-4 p-3">
                <span className="mono-xs uppercase tracking-[0.14em] text-fore-3">
                  the pool, one deal
                </span>
                <span className="leak-tag">7 notes public</span>
              </figcaption>
            </div>
            <p className="mono-xs mt-3 max-w-[52ch] leading-relaxed text-fore-3">
              Every shielded position in STRK20 is a note. This field is that set. One deal is drawn
              over it: the two parties, the offers, seven settlement notes, one viewing grant.
              Cinnabar marks what a chain reader can see.
            </p>
          </figure>
        </Reveal>
      </div>
    </Section>
  );
}
