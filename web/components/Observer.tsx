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
    <section id="observer" className="on-void relative mt-24 overflow-hidden md:mt-36">
      <div className="pointer-events-none absolute inset-0 opacity-[0.22]">
        <NoteLattice variant="void" className="h-full w-full" density={11} highlight={false} />
      </div>
      {/* a veil, so the measurement tables sit on solid ground rather than on noise */}
      <div
        className="pointer-events-none absolute inset-0"
        style={{
          background:
            "radial-gradient(120% 70% at 30% 45%, rgba(0,0,0,0.86) 0%, rgba(0,0,0,0.55) 45%, rgba(0,0,0,0) 100%)",
        }}
      />

      <div className="relative px-[var(--edge)] py-24 md:py-36">
        <div className="mx-auto w-full max-w-[1560px]">
          <p className="label m-0">Fig. 06 — the no-key recovery attack</p>

          <h2 className="display mt-6 mb-0 max-w-[18ch] text-[clamp(34px,5.4vw,74px)] text-[#efece4]">
            An observer with no key
            <span className="italic text-[#8d887e]"> recovers this.</span>
          </h2>

          {/* what calldata carries */}
          <div className="mt-12 max-w-[820px] border border-[#262626] p-6 md:p-8">
            <p className="mono-xs m-0 mb-5 uppercase tracking-[0.14em] text-[#78746c]">
              five salt values per message, as calldata carries them
            </p>
            <ul className="m-0 list-none space-y-2 p-0">
              {salts.map((s, i) => (
                <li key={i} className="mono-sm flex gap-5 tabular-nums text-[#9a958b]">
                  <span className="w-8 shrink-0 text-[#6d6961]">s{i}</span>
                  <span className="break-all">{s}</span>
                </li>
              ))}
            </ul>
            <p className="mono-xs mt-6 m-0 leading-relaxed text-[#807c74]">
              Illustrative — the shape of what a settlement writes, not a capture of one
              transaction. Against wire v3, <code className="text-[#9a958b]">scripts/observer.py</code>{" "}
              finds no plausible transcript in it: no message type, reply target, timestamp,
              amount, deadline, or memo hash.
            </p>
          </div>

          {/* the measured result */}
          <div className="mt-16 grid grid-cols-1 gap-10 lg:grid-cols-[0.9fr_1.1fr] lg:gap-16">
            <div>
              <p className="display m-0 text-[clamp(64px,9vw,132px)] leading-none text-[#efece4]">
                0.5000
              </p>
              <p className="mono-xs mt-4 m-0 uppercase tracking-[0.16em] text-[#78746c]">
                balanced accuracy — chance
              </p>
              <p className="mt-6 max-w-[46ch] text-[13px] leading-[1.75] text-[#9a958b]">
                Wire v2 filled 536 of 595 payload bits and left the rest zeroed, so the fifth salt
                of every message had bit 119 pinned. That predicate identified an Erebus message
                essentially every time. Wire v3 masks the spare bits with a separately derived
                HKDF keystream, and the same classifier now scores chance against the v3 fixture
                and 10,000 synthetic negatives.
              </p>
              <p className="mt-6 max-w-[46ch] border-t border-[#3a3a3a] pt-5 font-[family-name:var(--font-display)] text-[19px] leading-[1.45] text-[#efece4]">
                This is not a general anonymity claim. It is one classifier, defeated.
              </p>
            </div>

            <div>
              <p className="mono-xs m-0 mb-4 uppercase tracking-[0.14em] text-[#78746c]">
                measured, docs/threat-model.md §4
              </p>
              <table className="w-full border-collapse text-left">
                <tbody>
                  {METRICS.map((m) => (
                    <tr key={m.id} className="border-t border-[#262626] align-top">
                      <td className="w-10 py-4 pr-4">
                        <span className="mono-xs text-[#6d6961]">{m.id}</span>
                      </td>
                      <td className="py-4 pr-6">
                        <p className="m-0 text-[13px] leading-snug text-[#c3bfb5]">{m.question}</p>
                        <p className="mono-xs mt-2 m-0 leading-snug text-[#807c74]">{m.detail}</p>
                      </td>
                      <td className="w-24 py-4 text-right">
                        <span
                          className={`tnum text-[15px] ${m.bad ? "text-[#e2492f]" : "text-[#efece4]"}`}
                        >
                          {m.result}
                        </span>
                        <span className="mono-xs mt-1 block text-[#6d6961]">target {m.target}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>

              <div className="mt-8 flex flex-wrap gap-x-8 gap-y-3">
                <a
                  href={doc("scripts/observer.py")}
                  className="mono-xs uppercase tracking-[0.16em] text-[#9a958b] underline decoration-[#3a3a3a] underline-offset-[6px] hover:text-[#efece4]"
                >
                  observer.py ↗
                </a>
                <a
                  href={doc("docs/threat-model.md")}
                  className="mono-xs uppercase tracking-[0.16em] text-[#9a958b] underline decoration-[#3a3a3a] underline-offset-[6px] hover:text-[#efece4]"
                >
                  threat-model.md ↗
                </a>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
