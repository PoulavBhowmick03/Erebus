import { NoteLattice } from "./NoteLattice";

/**
 * The pool, given room.
 *
 * A full-bleed moment between the evidence and the walkthrough: the anonymity
 * set, drifting, with one deal drawn over it. It is a diagram, not atmosphere —
 * only what actually leaks is cinnabar — and it sits on its own so the eye can
 * take it in without competing with a headline or a code block.
 */
export function PoolBand() {
  return (
    <section className="mt-24 md:mt-36" aria-label="The anonymity set and one deal">
      <div className="relative h-[44vh] min-h-[280px] w-full border-y border-rule">
        <NoteLattice className="absolute inset-0" story />
        <p className="pointer-events-none absolute inset-x-0 bottom-0 m-0 flex flex-wrap items-end justify-between gap-x-4 gap-y-1 px-[var(--edge)] pb-3">
          <span className="mono-xs uppercase tracking-[0.14em] text-fore-3">
            the pool, one deal
          </span>
          <span className="leak-tag">7 notes public</span>
        </p>
      </div>

      <div className="mx-auto w-full max-w-[1560px] px-[var(--edge)]">
        <p className="mono-xs mt-4 max-w-[74ch] leading-relaxed text-fore-3">
          Every shielded position in STRK20 is a note; the drifting field is that set. The deal
          draws <em className="not-italic text-fore-2">only what leaks</em> — the pair, the
          crossings, seven settlement notes, one scoped grant. No amount ever appears, and nothing
          here changes when you drop the key.
        </p>
      </div>
    </section>
  );
}
