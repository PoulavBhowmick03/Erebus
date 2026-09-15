"use client";

import { useCallback, useRef, useState } from "react";
import { DEAL_SUMMARY, DISCLOSURE, REPLAY, TRANSCRIPT } from "@/lib/content";
import { Section } from "./Chrome";
import { Reveal } from "./Reveal";
import { Secret } from "./Secret";

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

type State = "ready" | "running" | "settled";

/**
 * The two halves of the argument in one block.
 *
 * The transcript is what a channel party sees. The left rail is what a public
 * chain reader sees at the same moment, annotated per stage. They advance
 * together on one click, which is the point: the deal is legible to the parties
 * and the traffic shape is legible to everyone.
 */
export function Replay() {
  const [stage, setStage] = useState(0);
  const [lines, setLines] = useState(0);
  const [state, setState] = useState<State>("ready");
  const running = useRef(false);

  const run = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    setLines(0);
    setStage(0);
    setState("running");

    let current = 0;
    for (let i = 0; i < TRANSCRIPT.length; i += 1) {
      const line = TRANSCRIPT[i];
      if (line.stage !== current) {
        current = line.stage;
        setStage(current);
        await wait(340);
      }
      setLines(i + 1);
      await wait(380);
    }
    setState("settled");
    running.current = false;
  }, []);

  const reset = useCallback(() => {
    running.current = false;
    setLines(0);
    setStage(0);
    setState("ready");
  }, []);

  return (
    <Section id="how" className="pt-24 md:pt-36">
      <Reveal className="section-head">
        <h2 className="display mb-0 max-w-[27ch] text-[clamp(25px,3.5vw,49px)]">
          Run the negotiation.
        </h2>
        <div className="flex flex-col justify-end">
          <p className="prose m-0 max-w-[54ch]">
            This browser simulation mirrors{" "}
            <code className="text-fore">agents/src/erebus_agents/demo.py</code>, the mock rehearsal
            the reference agents run. It does not submit a transaction or use a wallet.
          </p>
        </div>
      </Reveal>

      <div className="mt-12 grid grid-cols-1 border border-rule lg:grid-cols-[0.82fr_1.18fr]">
        {/* what a public chain reader sees, per stage */}
        <ol className="m-0 list-none border-b border-rule p-0 lg:border-b-0 lg:border-r">
          {REPLAY.map((s, i) => {
            const active = stage >= i + 1;
            return (
              <li
                key={s.id}
                className={`border-b border-rule border-l-2 p-6 transition-colors last:border-b-0 hover:bg-panel ${
                  active ? "border-l-fore" : "border-l-transparent"
                }`}
              >
                <p className="mono-xs m-0 mb-5 uppercase tracking-[0.16em]">
                  <span className="text-fore-3">{s.id}</span>{" "}
                  <span className={active ? "text-fore" : "text-fore-2"}>{s.title}</span>
                </p>
                <dl className="m-0 grid grid-cols-1 gap-4">
                  <div>
                    <dt className="mono-xs m-0 mb-1 uppercase tracking-[0.14em] text-fore-3">
                      hidden
                    </dt>
                    <dd className="m-0 text-[13px] text-fore-2">{s.hidden}</dd>
                  </div>
                  <div>
                    <dt className="mono-xs m-0 mb-1 uppercase tracking-[0.14em] text-fore-3">
                      public
                    </dt>
                    <dd className={`m-0 text-[13px] ${active ? "leak" : "text-fore-2"}`}>
                      {s.open}
                    </dd>
                  </div>
                </dl>
              </li>
            );
          })}
        </ol>

        {/* what a channel party sees */}
        <div className="flex flex-col">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-rule px-6 py-3">
            <span className="mono-xs uppercase tracking-[0.14em] text-fore-3">
              erebus / agent transcript
            </span>
            <span className="flex items-center gap-4">
              <span className="mono-xs uppercase tracking-[0.18em] text-fore-2">{state}</span>
              <button
                type="button"
                onClick={run}
                disabled={state === "running"}
                className="border border-fore bg-fore px-4 py-1.5 text-[10px] uppercase tracking-[0.18em] text-ground transition-opacity hover:opacity-80 disabled:cursor-wait disabled:opacity-45"
              >
                {state === "running" ? "running…" : state === "settled" ? "run again" : "run"}
              </button>
              <button
                type="button"
                onClick={reset}
                className="mono-xs uppercase tracking-[0.16em] text-fore-3 transition-colors hover:text-fore"
              >
                reset
              </button>
            </span>
          </div>

          <ol
            aria-live="polite"
            aria-label="Negotiation transcript"
            className="m-0 min-h-[300px] list-none p-6 lg:min-h-[340px]"
          >
            {lines === 0 ? (
              <li className="mono-sm text-fore-3">
                <span className="mr-5 inline-block w-6 text-fore-3">00</span>
                Run the deal. Nothing here submits a transaction.
              </li>
            ) : (
              TRANSCRIPT.slice(0, lines).map((l, i) => (
                <li
                  key={l.text}
                  className="mono-sm flex gap-5 border-b border-rule/60 py-3 text-fore-2 last:border-b-0"
                >
                  <span className="w-6 shrink-0 text-fore-3">
                    {String(i + 1).padStart(2, "0")}
                  </span>
                  <span>
                    {l.text}{" "}
                    {l.secret ? <Secret value={l.secret} className="text-fore" /> : null}
                    {l.tail ? <span className="text-fore">{l.tail}</span> : null}
                  </span>
                </li>
              ))
            )}
          </ol>

          {state === "settled" ? (
            <div className="grid grid-cols-2 border-t border-rule sm:grid-cols-4">
              {DEAL_SUMMARY.map((d) => (
                <div key={d.k} className="border-r border-rule px-6 py-4 last:border-r-0">
                  <p className="mono-xs m-0 mb-2 uppercase tracking-[0.14em] text-fore-3">
                    {d.k}
                  </p>
                  <p className="m-0 text-[13px]">
                    {d.hidden ? (
                      <Secret value={d.v} />
                    ) : (
                      <span className="text-fore-2">{d.v}</span>
                    )}
                  </p>
                </div>
              ))}
            </div>
          ) : null}
        </div>
      </div>

      <div className="section-head mt-12">
        <p className="prose m-0 max-w-[54ch]">
          Wire v3 encrypts offer terms under AES-256-GCM-SIV. It does not hide transaction timing,
          pool usage, or who you opened a channel with.
        </p>

        <table className="w-full border-collapse text-left">
          <thead>
            <tr className="border-y border-fore">
              <th className="label !text-fore-3 py-3 pr-6 font-normal">Observer</th>
              <th className="label !text-fore-3 py-3 pr-6 font-normal">Offer terms</th>
              <th className="label !text-fore-3 py-3 font-normal">Traffic shape</th>
            </tr>
          </thead>
          <tbody>
            {DISCLOSURE.map((r) => (
              <tr key={r.who} className="border-b border-rule transition-colors hover:bg-panel">
                <td className="py-4 pr-6 text-[13px]">{r.who}</td>
                <td
                  className={`py-4 pr-6 text-[13px] ${
                    r.terms === "Hidden" ? "text-fore-2" : "text-fore"
                  }`}
                >
                  {r.terms}
                </td>
                <td className="py-4 text-[13px] leak">Visible</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Section>
  );
}
