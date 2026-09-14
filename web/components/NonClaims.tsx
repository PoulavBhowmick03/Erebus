import { NON_CLAIMS } from "@/lib/content";
import { Eyebrow, Section } from "./Chrome";
import { Reveal } from "./Reveal";

export function NonClaims() {
  return (
    <Section id="limits" className="bg-[#0a0a0c] mt-28 pb-14 md:mt-40 md:pb-20">
      <Reveal className="border-t border-rule pt-14 md:pt-20">
        <Eyebrow>The non-claims</Eyebrow>
        <h2 className="display mt-5 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
          What this
          <br />
          <span className="text-fore-3">does not do.</span>
        </h2>
      </Reveal>

      <ol className="m-0 mt-12 list-none p-0">
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
            <h3 className="display m-0 text-[clamp(19px,2.2vw,31px)] leading-[1.06]">
              {c.title}
            </h3>
            <p className="m-0 max-w-[46ch] self-center text-[13px] leading-[1.75] text-fore-2">
              {c.body}
            </p>
          </Reveal>
        ))}
      </ol>
    </Section>
  );
}
