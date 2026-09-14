import { NON_CLAIMS, OBSERVER, doc } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

/**
 * The honesty block. Every value a public chain reader still recovers lives
 * here, marked in cinnabar, because the page is only credible if the boundary
 * is as legible as the promise.
 */
export function Boundary() {
  return (
    <Section id="limits" className="pt-24 md:pt-36">
      <Reveal className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Fig. 03 — the boundary</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
            What this
            <br />
            <span className="text-fore-3">does not do.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-fore-2">
            An observer with no key still sees the submitting account, the timing, the pool usage,
            and the counterparty at channel-open. Wire v3 removed wire v2&rsquo;s fixed salt
            classifier; that defeats one classifier. It is not anonymisation, and this page will
            not imply otherwise.
          </p>
          <a
            href={doc("docs/threat-model.md")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-fore-2 underline decoration-rule-2 underline-offset-[6px] hover:text-fore"
          >
            threat-model.md ↗
          </a>
        </div>
      </Reveal>

      <div className="mt-14 grid grid-cols-1 gap-px border-t border-rule sm:grid-cols-3">
        {OBSERVER.map((m) => (
          <div key={m.k} className="border-b border-rule py-6 pr-6">
            <p className="mono-xs m-0 mb-3 uppercase tracking-[0.14em] text-fore-3">{m.k}</p>
            <p className={`tnum m-0 text-[clamp(28px,3vw,40px)] leading-none ${m.bad ? "leak" : ""}`}>
              {m.v}
            </p>
            <p className="mono-xs mt-3 m-0 max-w-[38ch] leading-relaxed text-fore-3">{m.note}</p>
          </div>
        ))}
      </div>

      <ol className="m-0 mt-16 list-none p-0">
        {NON_CLAIMS.map((c, i) => (
          <Reveal
            as="li"
            key={c.title}
            delay={i * 90}
            className="grid grid-cols-1 gap-4 border-t border-rule py-8 md:grid-cols-[3rem_1.1fr_1fr] md:gap-10"
          >
            <span className="mono-xs pt-3 uppercase tracking-[0.16em] text-fore-3">
              {String(i + 1).padStart(2, "0")}
            </span>
            <h3 className="display m-0 text-[clamp(19px,2.2vw,31px)] leading-[1.06]">{c.title}</h3>
            <p className="m-0 max-w-[46ch] self-center text-[13px] leading-[1.75] text-fore-2">
              {c.body}
            </p>
          </Reveal>
        ))}
      </ol>

      <p className="mono-xs mt-6 max-w-[74ch] border-t border-rule pt-6 leading-relaxed text-fore-3">
        Unaudited and experimental. Erebus has had no external security review. Do not put value
        you care about through it.
      </p>
    </Section>
  );
}
