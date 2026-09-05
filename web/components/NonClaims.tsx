import { NON_CLAIMS, doc } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";

export function NonClaims() {
  return (
    <Section id="limits" className="pt-24 md:pt-36">
      <div className="border-t border-rule pt-6">
        <Eyebrow>Fig. 08 — the non-claims</Eyebrow>
        <h2 className="display mt-5 mb-0 max-w-[16ch] text-[clamp(34px,5.4vw,74px)]">
          What this
          <br />
          <span className="italic text-ink-2">does not do.</span>
        </h2>
      </div>

      <ol className="m-0 mt-12 list-none p-0">
        {NON_CLAIMS.map((c, i) => (
          <li key={c.title} className="grid grid-cols-1 gap-4 border-t border-rule py-8 md:grid-cols-[3rem_1.1fr_1fr] md:gap-10">
            <span className="mono-xs pt-3 uppercase tracking-[0.16em] text-ink-3">
              {String(i + 1).padStart(2, "0")}
            </span>
            <h3 className="display m-0 text-[clamp(24px,3.1vw,42px)] leading-[1.06]">
              {c.title}
            </h3>
            <p className="m-0 max-w-[46ch] self-center text-[13px] leading-[1.75] text-ink-2">
              {c.body}
              {c.ref ? (
                <a
                  href={doc("docs/friction.md")}
                  className="ml-2 leak underline decoration-transparent underline-offset-[5px] hover:decoration-current"
                >
                  {c.ref} ↗
                </a>
              ) : null}
            </p>
          </li>
        ))}
      </ol>

      <p className="mono-xs mt-6 max-w-[74ch] border-t border-rule pt-6 leading-relaxed text-ink-3">
        Unaudited and experimental. It has had no external security review. Do not put value you
        care about through it.
      </p>
    </Section>
  );
}
