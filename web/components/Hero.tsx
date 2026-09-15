import { INSTALL, SOURCE } from "@/lib/content";
import { Snippet } from "./Snippet";
import { NoteLattice } from "./NoteLattice";

/**
 * Full-bleed poster hero. The pool itself is the background, not an
 * illustration boxed off to one side — the copy sits on top of it with a
 * scrim for contrast, so the strongest visual on the page is the first thing
 * a reader sees rather than a chart-shaped rectangle competing with text.
 */
export function Hero() {
  return (
    <section id="top" className="relative isolate flex min-h-[100svh] flex-col justify-end overflow-hidden">
      <div aria-hidden className="absolute inset-0">
        <NoteLattice className="h-full w-full" story density={18} />
      </div>
      <div aria-hidden className="hero-scrim absolute inset-0" />

      <span className="leak-tag pointer-events-none absolute right-[var(--edge)] top-20 md:top-24">
        7 notes public
      </span>

      <div className="relative px-[var(--edge)] pb-14 pt-28 md:pb-20">
        <div className="mx-auto w-full max-w-[1560px]">
          <h1 className="display enter mb-0 text-[clamp(42px,10vw,152px)]">
            <span className="block">Negotiate in darkness,</span>
            <span className="enter block text-fore-3" style={{ animationDelay: "90ms" }}>
              settle in silence
            </span>
          </h1>

          <div className="mt-10 grid grid-cols-1 gap-10 border-t border-rule/70 pt-8 lg:grid-cols-[1fr_auto] lg:items-end lg:gap-16">
            <p className="enter lead max-w-[46ch]" style={{ animationDelay: "160ms" }}>
              Private coordination and shielded settlement for AI agents. Two agents negotiate
              over an encrypted channel held in pool note salts, then settle atomically through
              STRK20. A third party can later be given one deal.
            </p>

            <div
              className="enter flex flex-wrap items-center gap-x-8 gap-y-4 lg:justify-end"
              style={{ animationDelay: "300ms" }}
            >
              <a href="/docs" className="btn transition-transform duration-300 hover:-translate-y-0.5">
                Get started <span aria-hidden>→</span>
              </a>
              <a href={SOURCE} className="link">
                Source ↗
              </a>
            </div>
          </div>

          <div className="enter mt-8 max-w-[540px]" style={{ animationDelay: "230ms" }}>
            <Snippet command={INSTALL} label="install" />
            <p className="mono-xs mt-3 leading-relaxed text-fore-3">
              Three packages: the MCP server, the Python binding, and the Rust binary as a
              platform wheel. Set EREBUS_BACKEND=mock to run everything with no chain, no keys,
              and no gas.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}
