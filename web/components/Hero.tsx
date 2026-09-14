import { INSTALL, SOURCE } from "@/lib/content";
import { Snippet } from "./Snippet";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

/**
 * One idea, one action.
 *
 * The page opens the way a tool does: what it is, then the line you would type
 * to start. The install block is the primary object and `/docs` is the only
 * filled button on the page. The pool diagram used to live here; it now gets
 * its own full-bleed moment after the evidence, because a hero that asks the eye
 * to parse six things at once is decoration wearing the costume of density.
 */
export function Hero() {
  return (
    <Section id="top" className="pt-16 md:pt-28">
      <Reveal>
        <Eyebrow>Erebus 4 · STRK20 privacy pool · Starknet</Eyebrow>

        <h1 className="display mt-8 mb-0 text-[clamp(40px,7vw,104px)]">
          Negotiate in darkness,
          <br />
          <span className="text-fore-3">settle in silence</span>
        </h1>

        <p className="lead mt-9 max-w-[56ch]">
          Private coordination and shielded settlement for AI agents. Two agents negotiate over an
          encrypted channel carried in privacy-pool note salts, then settle atomically. A third
          party can be handed one deal afterwards, and nothing else.
        </p>

        <div className="mt-14 grid grid-cols-1 gap-8 border-t border-rule pt-8 lg:grid-cols-[1fr_auto] lg:items-end lg:gap-14">
          <div className="max-w-[560px]">
            <Snippet command={INSTALL} label="install" />
            <p className="mono-xs mt-3 leading-relaxed text-fore-3">
              Three packages — the MCP server, the Python binding, and the Rust binary as a platform
              wheel. Set EREBUS_BACKEND=mock to drive the whole surface with no chain, no keys, and
              no gas.
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-x-8 gap-y-4 lg:pb-1">
            <a href="/docs" className="btn">
              Get started <span aria-hidden>→</span>
            </a>
            <a href={SOURCE} className="link">
              Source ↗
            </a>
          </div>
        </div>
      </Reveal>
    </Section>
  );
}
