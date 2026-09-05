"use client";

import { useCallback, useRef, useState } from "react";
import { Eyebrow, Section } from "./Chrome";
import { Secret } from "./Secret";
import { doc } from "@/lib/content";

type Line = { n: number; text: string; secret?: string; tail?: string };

const fmt = (v: number) => v.toLocaleString("en-US");
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function Simulation() {
  const [budget, setBudget] = useState(1000);
  const [reserve, setReserve] = useState(800);
  const [lines, setLines] = useState<Line[]>([]);
  const [state, setState] = useState<"ready" | "running" | "settled" | "no deal">("ready");
  const [note, setNote] = useState("No keys or funds are used in this simulation.");
  const [record, setRecord] = useState<{ agreed: number; paid: number } | null>(null);
  const running = useRef(false);

  const run = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    setRecord(null);
    setLines([]);
    setState("running");
    setNote("Applying the same policy decisions as the Python reference agents.");

    const push = async (l: Line) => {
      setLines((prev) => [...prev, l]);
      await wait(320);
    };

    await push({ n: 1, text: "opened encrypted channel", tail: "ch_b7afee5f…8d9af8" });
    await push({ n: 2, text: "buyer proposed", secret: `${fmt(budget)} units` });

    if (budget < reserve) {
      await push({ n: 3, text: "seller rejected — reserve not met", secret: `${fmt(reserve)} units` });
      await push({ n: 4, text: "negotiation ended without settlement" });
      setState("no deal");
      setNote("Raise the buyer budget or lower the seller reserve to reach a settlement.");
    } else {
      await push({ n: 3, text: "seller countered", secret: `${fmt(budget)} units` });
      await push({ n: 4, text: "buyer accepted the counteroffer" });
      await push({ n: 5, text: "accepted offer and shielded payment committed atomically" });
      await push({ n: 6, text: "deal-scoped viewing grant created for", tail: "0xauditor" });
      await push({ n: 7, text: "auditor reconstructed two offers and the settlement record" });
      setRecord({ agreed: budget, paid: budget });
      setState("settled");
      setNote("Simulation complete. The evidence manifest links the runs that did this on chain.");
    }
    running.current = false;
  }, [budget, reserve]);

  return (
    <Section id="run" className="pt-24 md:pt-36">
      <div className="grid grid-cols-1 gap-8 border-t border-rule pt-6 lg:grid-cols-[1.1fr_0.9fr] lg:gap-16">
        <div>
          <Eyebrow>Fig. 05 — reference agent flow</Eyebrow>
          <h2 className="display mt-5 mb-0 max-w-[21ch] text-[clamp(34px,5.4vw,74px)]">
            Watch one deal move
            <br />
            <span className="italic text-ink-2">through Erebus.</span>
          </h2>
        </div>
        <div className="flex flex-col justify-end">
          <p className="m-0 max-w-[52ch] text-[13px] leading-[1.75] text-ink-2">
            This browser simulation mirrors{" "}
            <code className="text-ink">agents/src/erebus_agents/demo.py</code>, the deterministic
            mock rehearsal the reference agents run. It applies the same accept/reject threshold.
            It does not submit a transaction or use a wallet.
          </p>
          <a
            href={doc("agents/src/erebus_agents/demo.py")}
            className="mono-xs mt-5 w-fit uppercase tracking-[0.16em] text-ink-2 underline decoration-rule-2 underline-offset-[6px] hover:text-ink"
          >
            demo.py ↗
          </a>
        </div>
      </div>

      <div className="mt-12 grid grid-cols-1 border border-rule lg:grid-cols-[0.62fr_1.38fr]">
        {/* policy */}
        <div className="border-b border-rule p-7 lg:border-b-0 lg:border-r">
          <p className="label mb-7">Policy</p>

          <div className="mb-8">
            <label htmlFor="budget" className="mb-3 flex items-baseline justify-between">
              <span className="mono-sm uppercase tracking-[0.12em] text-ink-2">Buyer budget</span>
              <output htmlFor="budget" className="tnum text-[15px]">
                {fmt(budget)}
              </output>
            </label>
            <input
              id="budget"
              type="range"
              min={800}
              max={1400}
              step={50}
              value={budget}
              onChange={(e) => setBudget(Number(e.target.value))}
            />
          </div>

          <div className="mb-9">
            <label htmlFor="reserve" className="mb-3 flex items-baseline justify-between">
              <span className="mono-sm uppercase tracking-[0.12em] text-ink-2">Seller reserve</span>
              <output htmlFor="reserve" className="tnum text-[15px]">
                {fmt(reserve)}
              </output>
            </label>
            <input
              id="reserve"
              type="range"
              min={500}
              max={1200}
              step={50}
              value={reserve}
              onChange={(e) => setReserve(Number(e.target.value))}
            />
          </div>

          <button
            id="run-demo"
            type="button"
            onClick={run}
            disabled={state === "running"}
            className="w-full border border-ink bg-ink px-5 py-3 text-[11px] uppercase tracking-[0.18em] text-bone transition-opacity hover:opacity-80 disabled:cursor-wait disabled:opacity-45"
          >
            {state === "running" ? "Negotiating…" : "Run negotiation"}
          </button>

          <p className="mono-xs mt-4 m-0 leading-relaxed text-ink-3">{note}</p>
        </div>

        {/* transcript */}
        <div className="flex flex-col">
          <div className="flex items-center justify-between border-b border-rule px-6 py-3">
            <span className="mono-xs uppercase tracking-[0.14em] text-ink-3">
              erebus / agent transcript
            </span>
            <span className="mono-xs uppercase tracking-[0.18em]">{state}</span>
          </div>

          <ol
            aria-live="polite"
            aria-label="Negotiation transcript"
            className="m-0 min-h-[300px] list-none p-6 lg:min-h-[360px]"
          >
            {lines.length === 0 ? (
              <li className="mono-sm text-ink-3">
                <span className="mr-5 inline-block w-5 text-ink-3">00</span>
                Set the policies and run the negotiation.
              </li>
            ) : (
              lines.map((l) => (
                <li
                  key={l.n}
                  className="mono-sm flex gap-5 border-b border-rule/60 py-3 text-ink-2 last:border-b-0"
                >
                  <span className="w-5 shrink-0 text-ink-3">{String(l.n).padStart(2, "0")}</span>
                  <span className="text-ink-2">
                    {l.text}{" "}
                    {l.secret ? <Secret value={l.secret} className="text-ink" /> : null}
                    {l.tail ? <span className="text-ink">{l.tail}</span> : null}
                  </span>
                </li>
              ))
            )}
          </ol>

          {record ? (
            <div className="grid grid-cols-2 border-t border-rule sm:grid-cols-4">
              {[
                ["channel", "ch_b7afee5f…"],
                ["participants", "buyer ↔ seller"],
                ["agreed", `${fmt(record.agreed)} units`],
                ["paid", `${fmt(record.paid)} units`],
              ].map(([k, v], i) => (
                <div key={k} className="border-r border-rule px-6 py-4 last:border-r-0">
                  <p className="mono-xs m-0 mb-2 uppercase tracking-[0.14em] text-ink-3">{k}</p>
                  <p className="m-0 text-[13px]">
                    {i < 2 ? <span className="text-ink-2">{v}</span> : <Secret value={v} />}
                  </p>
                </div>
              ))}
            </div>
          ) : null}
        </div>
      </div>
    </Section>
  );
}
