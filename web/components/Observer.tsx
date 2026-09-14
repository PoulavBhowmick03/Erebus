"use client";

import { useEffect, useState } from "react";
import { METRICS, doc } from "@/lib/content";
import { NoteLattice } from "./NoteLattice";

const hex = (n: number) =>
  "0x" + Array.from({ length: n }, () => "0123456789abcdef"[Math.floor(Math.random() * 16)]).join("");

export function Observer() {
  const [salts, setSalts] = useState<string[]>(() =>
    ["…", "…", "…", "…", "…"].map(() => "0x" + "0".repeat(30)),
  );

  useEffect(() => {
    const roll = () => setSalts(Array.from({ length: 5 }, () => hex(30)));
    roll();
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduced) return;
    const id = window.setInterval(roll, 2600);
    return () => window.clearInterval(id);
  }, []);

  return (
    <section id="observer" className="on-flare relative mt-28 overflow-hidden md:mt-40">
      <div className="pointer-events-none absolute inset-0 opacity-[0.22]">
        <NoteLattice variant="light" className="h-full w-full" density={11} highlight={false} />
      </div>
      {/* a veil, so the measurement tables sit on solid ground rather than on noise */}
      <div
        className="pointer-events-none absolute inset-0"
        style={{
          background:
            "radial-gradient(120% 70% at 30% 45%, rgba(243,244,246,0.92) 0%, rgba(243,244,246,0.6) 45%, rgba(243,244,246,0) 100%)",
        }}
      />

      <div className="relative px-[var(--edge)] py-24 md:py-36">
        <div className="mx-auto w-full max-w-[1560px]">
          <p className="label m-0">The no-key recovery attack</p>

          <h2 className="display mt-6 mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)] text-[#0b0b0d]">
            An observer with no key
            <br />
            <span className="text-[#5c5c63]">recovers this</span>
          </h2>

          {/* the measured result */}
          <div className="mt-12 grid grid-cols-1 gap-10 lg:grid-cols-[0.9fr_1.1fr] lg:gap-16">
            <div>
              <p className="display m-0 text-[clamp(64px,9vw,132px)] leading-none text-[#0b0b0d]">
                0.5000
              </p>
              <p className="mono-xs mt-4 m-0 uppercase tracking-[0.16em] text-[#6b6b73]">
                balanced accuracy — chance
              </p>
              <p className="mt-6 max-w-[46ch] text-[13px] leading-[1.75] text-[#3a3a40]">
                The strongest classifier we have finds no plausible transcript against wire v3 —
                against 10,000 synthetic negatives, it scores chance.
              </p>
              <p className="mt-6 max-w-[46ch] border-t border-[#c9ccd1] pt-5 font-[family-name:var(--font-display)] text-[19px] leading-[1.45] text-[#0b0b0d]">
                This is not a general anonymity claim. It is one classifier, defeated.
              </p>
            </div>

            <div>
              <p className="mono-xs m-0 mb-4 uppercase tracking-[0.14em] text-[#6b6b73]">
                all four measured metrics — docs/threat-model.md §4
              </p>
              <table className="w-full border-collapse text-left">
                <tbody>
                  {METRICS.map((m) => (
                    <tr key={m.id} className="border-t border-[#d2d4d8] align-top">
                      <td className="w-10 py-4 pr-4">
                        <span className="mono-xs text-[#5a5a61]">{m.id}</span>
                      </td>
                      <td className="py-4 pr-6">
                        <p className="m-0 text-[13px] leading-snug text-[#2c2c32]">{m.question}</p>
                        <p className="mono-xs mt-2 m-0 leading-snug text-[#6b6b73]">{m.detail}</p>
                      </td>
                      <td className="w-24 py-4 text-right">
                        <span
                          className={`tnum text-[15px] ${m.bad ? "text-[#cc2a10]" : "text-[#0b0b0d]"}`}
                        >
                          {m.result}
                        </span>
                        <span className="mono-xs mt-1 block text-[#5a5a61]">target {m.target}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>

              <div className="mt-8 flex flex-wrap gap-x-8 gap-y-3">
                <a
                  href={doc("scripts/observer.py")}
                  className="mono-xs uppercase tracking-[0.16em] text-[#3a3a40] underline decoration-[#c9ccd1] underline-offset-[6px] hover:text-[#0b0b0d]"
                >
                  observer.py ↗
                </a>
                <a
                  href={doc("docs/threat-model.md")}
                  className="mono-xs uppercase tracking-[0.16em] text-[#3a3a40] underline decoration-[#c9ccd1] underline-offset-[6px] hover:text-[#0b0b0d]"
                >
                  threat-model.md ↗
                </a>
              </div>
            </div>
          </div>

          {/* the deep dive — full width so it never lopsides either column */}
          <details className="group mt-14 border-t border-[#d2d4d8] pt-8">
            <summary className="mono-xs m-0 flex w-fit cursor-pointer list-none items-center gap-2 uppercase tracking-[0.14em] text-[#6b6b73] [&::-webkit-details-marker]:hidden">
              <span aria-hidden className="inline-block transition-transform group-open:rotate-90">
                →
              </span>
              how wire v3 defeats the classifier
            </summary>

            <div className="mt-8 grid grid-cols-1 gap-10 lg:grid-cols-[0.9fr_1.1fr] lg:gap-16">
              <div className="border border-[#d2d4d8] p-6 md:p-7">
                <p className="mono-xs m-0 mb-5 uppercase tracking-[0.14em] text-[#6b6b73]">
                  five salt values per message, as calldata carries them
                </p>
                <ul className="m-0 list-none space-y-2 p-0">
                  {salts.map((s, i) => (
                    <li key={i} className="mono-sm flex gap-5 tabular-nums text-[#3a3a40]">
                      <span className="w-8 shrink-0 text-[#5a5a61]">s{i}</span>
                      <span className="break-all">{s}</span>
                    </li>
                  ))}
                </ul>
                <p className="mono-xs mt-6 m-0 leading-relaxed text-[#6b6b73]">
                  Illustrative — the shape of what a settlement writes, not a captured
                  transaction.
                </p>
              </div>

              <p className="m-0 max-w-[60ch] text-[13px] leading-[1.75] text-[#3a3a40]">
                Wire v2 filled 536 of 595 payload bits and left the rest zeroed, pinning bit 119
                in every message&rsquo;s fifth salt — a predicate that caught an Erebus message
                almost every time. Wire v3 masks the spare bits with a separately derived HKDF
                keystream, and{" "}
                <code className="text-[#3a3a40]">scripts/observer.py</code> re-scores chance
                against the v3 fixture.
              </p>
            </div>
          </details>
        </div>
      </div>
    </section>
  );
}
