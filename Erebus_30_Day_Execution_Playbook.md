# Erebus — 30-Day Execution Playbook

> Rebuilt from `Erebus_30_Day_Execution_Playbook.docx`.
> **Execution window:** 13 September – 12 October 2026. Move the dates if work starts later;
> preserve the sequence.
> **Status:** nothing here has been posted, scheduled, or sent.

## The one rule

**Astra and Grok never receive files.** They are remote and have web access. Every prompt below
is self-contained: it names the public URLs to fetch and inlines the facts that must not be
wrong. This playbook, its appendices, the `~/Desktop/asset-brief` bundle, and every repo file
under `artifacts/` are **operator-only** and must never be attached to an Astra or Grok request.

If a needed thing is not public, the prompt says so and gives a labelled fallback. The rule is:
fetch, don't upload; if it cannot be fetched, reconstruct and label it, or produce a different
asset; never fabricate a receipt, a terminal capture, a user session, or a number.

---

## Public sources — the only inputs Astra and Grok may use

Repository: `https://github.com/PoulavBhowmick03/Erebus` · branch `main` · raw base
`https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/`

| What | Public path |
|---|---|
| Overview / install / claims | `README.md` |
| Current truth (tiebreaker) | `docs/status.md` |
| Privacy boundary (canonical) | `docs/privacy-model.md` |
| Cross-framework run | `docs/runs/2026-09-07-mainnet-2strk-agents.md` |
| 0.6/0.4 canary | `docs/runs/2026-08-31-mainnet-060-040-canary.md` |
| First full mainnet canary | `docs/runs/2026-08-31-mainnet-starkscan-workflow.md` |
| Roadmap / gaps | `docs/roadmap.md`, `docs/production-gaps.md` |
| Use cases | `docs/usecases.md` |
| Brand tokens | `web/app/globals.css` |
| Mark / icon | `web/app/icon.svg` |
| Page copy | `web/lib/content.ts` |
| Thumbnail / overview | `docs/assets/demo-thumbnail.jpg`, `docs/assets/erebus-overview.excalidraw.svg` |
| Sprint video | `demo/erebus-private-sprint.mp4` |
| Live site | `https://erebusagents.live` |
| X (brand) | the account linked from the site and README — find it, do not assume a handle |
| X (founder) | `@impoulav` |

`README.md` and `docs/status.md` carry the current claim. If a prompt and a fetched page
disagree, the fetched page wins.

---

## Source drift — read before posting

The public `main` branch is **behind** the local working branch. Astra and Grok will publish
what `main` says, so either post the public numbers or push the newer docs first.

| Claim | Public `main` (what Astra/Grok fetch) | Local working branch |
|---|---|---|
| Bounded mainnet runs | **Three**: 0.8/0.2, 0.6/0.4, 2.0/0.5 | Four: adds 0.8/0.1 (2026-09-11) |
| 0.8/0.1 run record | Not present (404) | `docs/runs/2026-09-11-mainnet-subagent-canary.md` |
| Independent operator check | **Unverified / open** | Complete |

Do not hardcode the run count in any post. Instruction to Astra and Grok: **quote the count and
splits exactly as `docs/status.md` reads when fetched.** As of this writing that is three runs;
if the newer run is pushed, the same instruction picks up four with no edit.

**Correction to an earlier draft.** A previous edit of this file "corrected" a draft from three
runs to four and the operator check from open to complete. That was based on the unpushed local
branch and is wrong against the public repo. The drafts below treat the fetched `status.md` as
authoritative.

---

## What Astra and Grok cannot fetch

These are not on GitHub or X. Do not ask for uploads; use the fallback.

| Post | Missing input | Fallback |
|---|---|---|
| P03 | A fresh clean-install recording | Labelled source-based walkthrough from the public README quickstart. Never a fake terminal capture. |
| P05 | A controlled uncertain-write recording | Labelled explainer of the reconciliation mechanism from the run records. Never stage a real failure. |
| P09 | A consented outside-installation session | Recruitment card asking for builders, no session claim. |
| P10 | A shipped operator-controls UI | Label the graphic PROPOSED; never present it as shipped. |
| P11 | Real study results | Publish the frozen method and label it design, not results. |
| P16 | The operator's private numbers | Post only observed facts; render a counter as 0 rather than invent it. |

---

## How to use this file

- Each day is one section. Days with a post carry: seed copy, an **Astra** prompt, a **Grok**
  prompt, and a **Do not claim** line. Days without a post carry the brief only.
- Send Astra and Grok **the prompt text only**. Nothing else.
- `[ ]` boxes are gates; do not publish with a gate unchecked.
- Times are test windows (14:00 / 17:00 UTC alternating), not an algorithm promise.

Legend: `[ ] Verified` · `[ ] Recorded` · `[ ] Labelled` (simulated vs chain, proposed vs
shipped, operator-controlled) · `[ ] No invented numbers`.

---

## Operating rules

| Choice | Working default |
|---|---|
| Cadence | Four substantial brand posts each week; one optional founder perspective. |
| Time | Alternate 14:00 and 17:00 UTC when someone can answer. |
| Effort | 12–15 GTM hours/week, engineering additional. If stretched, three originals. |
| Owners | Poulav validates technical claims and integrations. Ishita or another teammate owns the experiment and calendar. Proposed. |
| Conversion | One main call to action per post. Count outside mock runs, workflow conversations, and paid commitments separately. |
| Publication | Drafts are editable. Check the stated requirement before publishing. |

Current-release honest summary: the release supports mock mode and has a public cross-framework
mainnet demonstration. Erebus is experimental and unaudited. The launch had about 10.2K public
post views at observation — not 10.2K video completions and not 10.2K prospective buyers.

---

## Calendar at a glance

| Day · date | Post | Astra format | Owner | Time (UTC) | UTM |
|---|---|---|---|---|---|
| 1 · 13 Sep | P01 · Two frameworks, one settled deal | Video 35–45 s | Poulav | 14:00 | `p01` |
| 2 · 14 Sep | — · Discovery and setup | — | Team | — | — |
| 3 · 15 Sep | P02 · Privacy boundaries | Still | Poulav | 17:00 | `p02` |
| 4 · 16 Sep | P03 · The lowest-friction first run | Video 60–90 s | Poulav | 14:00 | `p03` |
| 5 · 17 Sep | — · Buyer discovery | — | Team | — | — |
| 6 · 18 Sep | P04 · One deal, one disclosure | Video 30–45 s | Poulav | 17:00 | `p04` |
| 7 · 19 Sep | — · Weekly review | — | Team | — | — |
| 8 · 20 Sep | P05 · An uncertain write | Video 60–90 s | Poulav | 14:00 | `p05` |
| 9 · 21 Sep | — · Onboarding and offer | — | Team | — | — |
| 10 · 22 Sep | P06 · Article one | Still + carousel | Team | 17:00 | `p06` |
| 11 · 23 Sep | P07 · Experiment announcement | Still | Experiment owner | 14:00 | `p07` |
| 12 · 24 Sep | — · Run and observe | — | Experiment owner | — | — |
| 13 · 25 Sep | P08 · The economics question | Still | Poulav | 17:00 | `p08` |
| 14 · 26 Sep | — · Weekly review | — | Team | — | — |
| 15 · 27 Sep | P09 · Outside installation | Video or still | Team | 14:00 | `p09` |
| 16 · 28 Sep | — · Scope paid pilots | — | Poulav | — | — |
| 17 · 29 Sep | P10 · Operator controls | Still (PROPOSED) | Team | 17:00 | `p10` |
| 18 · 30 Sep | P11 · The privacy study | Still | Experiment owner | 14:00 | `p11` |
| 19 · 1 Oct | — · Workflow work | — | Poulav | — | — |
| 20 · 2 Oct | P12 · A concrete customer problem | Still | Team | 17:00 | `p12` |
| 21 · 3 Oct | — · Weekly review | — | Team | — | — |
| 22 · 4 Oct | P13 · Workflow before feature list | Still | Team | 14:00 | `p13` |
| 23 · 5 Oct | — · Pilot follow-through | — | Team | — | — |
| 24 · 6 Oct | P14 · Visible product progress | Before/after | Poulav | 17:00 | `p14` |
| 25 · 7 Oct | P15 · A commercial invitation | Offer card | Team | 14:00 | `p15` |
| 26 · 8 Oct | — · Commercial calls | — | Team | — | — |
| 27 · 9 Oct | P16 · Month-one learning | Still | Team | 17:00 | `p16` |
| 28 · 10 Oct | — · Weekly review | — | Team | — | — |
| 29 · 11 Oct | — · Retention conversations | — | Team | — | — |
| 30 · 12 Oct | — · Product decision | — | Team | — | — |

UTM: `utm_source=x · utm_medium=organic · utm_campaign=erebus_month1 · utm_content=p0N`.

---

## Launch baseline (for Grok)

Observed 12 September 2026. These count overlapping exposures, not unique people.

| Post | Replies | Reposts | Likes | Bookmarks |
|---|---:|---:|---:|---:|
| Launch video post | 35 | 24 | 80 | 18 |
| Follow-up 1 | 22 | 6 | 39 | — |
| Follow-up 2 | 4 | 6 | 20 | — |
| Follow-up 3 | — | 5 | 17 | — |

View sequence: **16,202 → 1,823 → 504 → 330**. The final post had about 2.5% of the opening
post's views.

**Register (as read by Grok).** Short declaratives, one claim per line. The video post did **not**
state the limit; tweet 2 said "all the terms stay hidden"; tweet 4 finally stated the boundary
and carried two links — that is where the thread died. Design against it: boundary, proof, and
next action in the main post.

**Drift to avoid.** Post-launch replies decayed into emoji, "gib moarrr reach," "Have you checked
it out, anon?," and unqualified "Private settlement." Do not use the 13 Sep quote-tweet as a
template. Drop "1000+ website viewers" and "couple of users" — not in any run record. "15k views"
is fine only as a post-view count.

**Founder voice.** First person on `@impoulav`. Specific splits, the protocol number, what broke.
Match the 5 Sep v0.2.0 post; not the GTA line.

---

## Global Astra brief

Set once in Astra's persistent instructions, then send only the per-post request.

```text
You are the media producer for Erebus: private coordination and shielded settlement
infrastructure for AI agents on Starknet. You have web access. Never ask for file uploads —
fetch public sources yourself, and if something is not public, reconstruct it and label it, or
produce the labelled fallback the prompt gives. Never invent data.

FETCH FIRST (public)
- Repository: https://github.com/PoulavBhowmick03/Erebus, branch main.
  Raw base: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/
- Read README.md and docs/status.md for the current claim and the run count/splits.
- Read docs/privacy-model.md before drawing any boundary.
- Brand tokens: web/app/globals.css. Mark: web/app/icon.svg. Page copy: web/lib/content.ts.
- Site: https://erebusagents.live. X: the account linked from the site and README.
- Quote the run count exactly as docs/status.md reads when fetched. Never hardcode a count.

BRAND SYSTEM (fallback if a fetch fails; source of truth is web/app/globals.css)
- Ground #000000 = the problem and the close. Light ground #fafaf9 = the product acts.
- Fore #E9E9EC / #8B8B93 / #55555C. Rules #1E1E21 / #303035.
- Cinnabar #FF3B1F on dark, #D92D14 on light, means "a public observer can see this" and is used
  NOWHERE ELSE. Not a decorative accent. Teal #1FD6C0 means shielded, launch cut only. A masked
  value is a grey bar #26262B with edge #35353C, never cinnabar.
- Type, never mixed: IBM Plex Mono 400/500 = machine output (ledgers, addresses, terminals);
  Archivo 600 = narration (titles, captions).
- Canvas 16:9, 1920x1080, 30 fps unless asked otherwise. Stills PNG; motion GIF plus MP4.
- Motion: fade-in 0.35 s, transitions 0.30 s, no bounce, no gradients, no glow, no stock-photo
  people, no rounded-corner SaaS cards. GIFs may loop with a clear reset, but never imply one
  recorded payment is repeated activity.
- Logo: three bars, two ink one cinnabar = redacted lines of text. Never a padlock, shield,
  hexagon, cube, orb, eye, monogram, glow, bevel or drop shadow.

HARD RULES
- Label simulated vs chain: the browser demo is a simulation; the linked receipts are the chain
  evidence. Label proposed vs shipped. Label operator-controlled demonstrations.
- Say "authorized recipient", never "auditor". Never show Anthropic or OpenAI logos or imply a
  vendor partnership.
- Never fabricate a terminal capture, transaction receipt, product UI, or user session. If the
  real footage is not public, animate from the published run record and caption it
  "Reconstruction from published run record".
- Keep source URLs and commit/date references in the delivery notes, not on the artwork.
- If an export tool is unavailable, say so and deliver the supported format; never claim a file
  you did not produce.
```

---

## Global Grok brief

Set once, then send only the per-post request.

```text
You are the copy lead for Erebus on X. You have web access. Never ask for file uploads.

FETCH FIRST
- Find the Erebus brand account through the links on https://erebusagents.live and
  https://github.com/PoulavBhowmick03/Erebus. Read the 11 September 2026 launch thread and every
  post since. Record the register and where it drifted.
- Read @impoulav's posts. For founder-account variants, match his voice.
- Read the facts yourself: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/README.md,
  docs/status.md, docs/privacy-model.md, and the run records under docs/runs/.
- Quote the run count and splits exactly as docs/status.md reads when fetched. Never hardcode.

REGISTER (observed)
- Short declaratives, one claim per line, a limit stated plainly, one link, uncertainty admitted.
  Never "fully private".
- The launch video post omitted the limit; the boundary only arrived in tweet 4 (views fell
  16,202 -> 1,823 -> 504 -> 330). Put the boundary, the proof, and the next action in the main post.
- No emoji replies, no "gib reach", no "check it out, anon", no unqualified "Private settlement",
  no follow-for-follow, no copied replies. Do not template the 13 Sep quote-tweet.

RULES FOR EVERY DRAFT
- Main post <= 280 chars: hook, the plain-language boundary, one CTA.
- One call to action. Numbers only from the fetched run records. Launch figures are post views,
  not video completions and not buyers.
- Output: (a) three main-post variants — direct / question-led / technical; (b) one first-reply
  with method, scope, caveats and the evidence link; (c) one @impoulav founder-voice alternative.
  Give character counts. Keep the boundary in the main post, not only the reply.
```

---

## Handoff format

Send only **post number · still/video/GIF · intended content · duration**. Example:
`P02 · GIF · what a public observer sees versus an authorized recipient · 8 seconds`.
Astra fetches the rest. Do not paste this file, the appendices, or any local path.

---

# Week one — turn launch attention into first use

## Day 1 · Sun 13 Sep 2026 — P01 · Two frameworks, one settled deal

**Gate:** `[ ] Verified` result · `[ ] Recorded` clip or labelled reconstruction · `[ ] Labelled` operator-controlled · `[ ] No invented numbers`
**Owner:** Poulav. Publish the cross-framework result; add a visible mock-start path.
**Evidence:** log baseline; identify 10 relevant teams. **Time:** 14:00 UTC · **UTM:** `p01`

**Seed copy:**

> Claude Code negotiated with Codex. They settled 2 STRK on Starknet, then disclosed one deal to a third party.
>
> Erebus protects terms and amounts. The channel relationship stays public.
>
> Run the mock: https://github.com/PoulavBhowmick03/Erebus

**Astra — video · 35–45 s**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/runs/2026-09-07-mainnet-2strk-agents.md
and https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/privacy-model.md.
Facts (do not exceed): payer Claude Code, payee Codex; offer 1.6 STRK; counter 2.0; settled 2.0
with 0.5 change; a third recipient reconstructed this one deal while two earlier deals stayed
unreadable; operator-controlled demonstration.
Create: a 35-45 s proof video. The masked/decrypted record view is NOT published — build it from
the run record and caption it "Reconstruction from published run record". Do not imitate a real
terminal capture.
Shots: 0:00 result "Claude Code -> Codex · 2 STRK settled"; 0:04 "offer 1.6 · counter 2.0";
0:12 "accepted 2.0 · paid 2.0 · change 0.5"; 0:21 "one deal granted to a third recipient";
0:30 boundary card "Terms and amounts are hidden. The relationship and timing stay public.";
0:37 "Try a mock deal. No funds." + repo URL + "Experimental, unaudited".
Constraints: on-screen "Operator-controlled demonstration · edited for length"; no vendor logos.
Deliver: P01_two_frameworks_one_deal.mp4.
```

**Grok**

```text
Fetch: the Erebus launch thread (find the account via erebusagents.live and the README);
@impoulav; https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/runs/2026-09-07-mainnet-2strk-agents.md;
docs/privacy-model.md.
Facts (do not exceed): Claude Code payer, Codex payee; offer 1.6; counter and settle 2.0; change
0.5; third recipient reconstructed one deal, earlier deals unreadable; operator-controlled; quote
the run count from docs/status.md.
Task: three variants (direct / question / technical), a first-reply with method, run-record link,
the operator-controlled caveat and edited-wait label, and a founder-voice alternative for
@impoulav. Boundary and "run the mock" in the main post.
```

**Do not claim:** a customer transaction, a new run, throughput, or endorsement by Anthropic or
OpenAI. Do not present the 2.5 STRK shield as the price of a deal.

### Grok output — P01 (12 Sep 2026), reconciled

> As returned. The run count now defers to the fetched `status.md` (public main reads three).

**Direct**

> Agents pay each other onchain. Offers and amounts do not have to sit in the clear.
>
> Erebus hides the terms, not the relationship. Amounts stay hidden. The channel pair and timing stay public.
>
> Bounded mainnet runs: 0.8/0.2, 0.6/0.4, 2.0/0.5 (quote docs/status.md).
> https://erebusagents.live

**Question-led**

> Do your agents have to publish every offer and amount?
>
> Erebus hides terms and settlement amounts. The channel pair and timing stay public. Not an anonymity set.
>
> https://erebusagents.live

**Technical**

> Wire v3 on STRK20. Offers go into pool notes. Amount, token, deadline stay encrypted. Settle and change in one action set.
>
> Pair and timing stay public. Counterparty is in channel-open calldata.
>
> https://erebusagents.live

**First reply**

> Method: two-party channel in STRK20 note salts. MCP tools. Atomic settle with change. Wire v3 deal IDs. Scoped viewing grant per deal, not per channel.
>
> Scope: a public-chain reader cannot recover amount, token, deadline, memo hash, or settlement recipient. They can see the pair, submitting account, timing, and note count. Channel-open writes the counterparty in the clear.
>
> Caveats: unaudited. Experimental. No escrow. Bounded canaries, not production capacity. Browser demo is a simulation. Independent operator check of the runbook is open (as docs/status.md reads).
>
> https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/privacy-model.md

**Founder-voice, @impoulav**

> i put the limit in tweet 4 of the launch thread. most people only saw the video.
>
> erebus hides terms and amounts. it does not hide who the channel is with, or when it settled. bounded mainnet runs: 0.8/0.2, 0.6/0.4, 2.0/0.5.
>
> unaudited. https://erebusagents.live

---

## Day 2 · Mon 14 Sep 2026 — No post · Discovery and setup

**Owner:** Team. **Work:** invite existing responders to workflow interviews; replay the quickstart
on a clean machine. **Evidence:** two interviews booked; one install-friction list.

---

## Day 3 · Tue 15 Sep 2026 — P02 · Privacy boundaries

**Gate:** `[ ] Verified` claims · `[ ] Labelled` comparison graphic
**Owner:** Poulav. Publish one deal through the public, party and reviewer views, checked against
the privacy model. **Evidence:** questions from builders and operators.
**Time:** 17:00 UTC · **UTM:** `p02`

**Seed copy:**

> Private payment terms do not mean invisible counterparties.
>
> Erebus hides deal contents from public observers. Channel relationships and timing remain visible.
>
> Here is what each participant can see: https://github.com/PoulavBhowmick03/Erebus/blob/c7795cd297e26bff71c7b524257b2c0723402d23/docs/privacy-model.md

**Astra — still · 1920×1080 + 1080×1350**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/privacy-model.md.
Facts (from that page): a public observer sees the channel was opened, the submitting account,
timing, action shape and note count, and cannot recover wire-v3 offer terms; parties see the terms
they negotiated; an authorized recipient sees one deal-scoped grant with an expiry and no spending
authority; the configured pool auditor and the write prover/RPC have their own trust roles.
Create: one graphic, four columns — Public observer / Party / Authorized recipient / Configured
roles — each listing what that view can and cannot see.
On-screen: cinnabar only on the boundary line "Terms and amounts are hidden. The relationship and
timing stay public." Masked values are grey #26262B bars.
Constraints: caption it "Simplified view, not the full threat model." Do not draw an "only two
people know" diagram.
Deliver: P02_privacy_boundaries_16x9.png, P02_privacy_boundaries_4x5.png.
```

**Grok**

```text
Fetch: docs/privacy-model.md (raw URL above); the Erebus launch thread; @impoulav.
Facts: Erebus hides the terms, not the relationship. Private payment terms do not mean invisible
counterparties; channel relationships and timing remain public; the pool auditor and prover/RPC
have separate roles.
Task: three variants, a first-reply linking docs/privacy-model.md and naming the auditor and
prover/RPC roles, and a founder-voice alternative. Boundary in the main post. No "fully private".
```

**Do not claim:** complete anonymity, hidden counterparties, or that the pool auditor has no access.

---

## Day 4 · Wed 16 Sep 2026 — P03 · The lowest-friction first run

**Gate:** `[ ] Verified` mock mode · `[ ] Recorded` or labelled reconstruct
**Owner:** Poulav. Show the shortest verified setup. **Evidence:** first outside mock completions.
**Time:** 14:00 UTC · **UTM:** `p03`

**Seed copy:**

> You can try an Erebus agent deal without a wallet, keys, or gas.
>
> Mock mode lets you test the negotiation workflow before touching a chain.
>
> Start here: https://github.com/PoulavBhowmick03/Erebus

**Astra — video · 60–90 s**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/README.md (quickstart) and
docs/status.md.
Facts: mock mode lets a developer test the negotiation workflow without a wallet, keys, gas, or a
chain transaction; the browser demo is a simulation.
Create: a 60-90 s walkthrough in the Instrument Black style. No public footage of a clean install
exists, so animate the quickstart from the README and caption it "Walkthrough built from the
public quickstart" — do not fabricate a terminal session.
On-screen (persistent): "MOCK MODE — no wallet, no keys, no gas, no chain transaction."
Shots: environment; install command; first command; mock negotiation; settled mock result; success
evidence. Keep full commands in a linked guide, not on screen throughout.
Constraints: never call the browser simulation a mainnet transaction. Do not invent a setup time.
Deliver: P03_mock_walkthrough.mp4.
```

**Grok**

```text
Fetch: the README quickstart; docs/status.md; the Erebus account.
Facts: mock mode needs no wallet, keys, gas, or chain transaction.
Task: three variants, a first-reply with the repo link and an invitation to report installation
blockers, and a founder-voice alternative. Never call the browser simulation a mainnet tx.
```

**Do not claim:** a mainnet transaction, or a setup time you did not measure.

---

## Day 5 · Thu 17 Sep 2026 — No post · Buyer discovery

**Owner:** Team. **Work:** finish interviews, identify the budget owner and existing counterparty,
prepare the pilot offer. **Evidence:** five interviews targeted by day seven. Interview for who
pays whom, how often, what must stay confidential, and who controls the budget. A community repost
is not a customer commitment.

---

## Day 6 · Fri 18 Sep 2026 — P04 · One deal, one disclosure

**Gate:** `[ ] Verified` recorded result · `[ ] Labelled` authorized-recipient (not auditor)
**Owner:** Poulav. Explain the authorized-recipient test and expiry limits; offer one guided mock.
**Evidence:** qualified walkthrough requests. **Time:** 17:00 UTC · **UTM:** `p04`

**Seed copy:**

> An auditor may need to inspect one deal, not your whole history.
>
> The Erebus mainnet demo gave a third recipient access to one settled deal, without spending authority.
>
> The run and boundaries: https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/runs/2026-09-07-mainnet-2strk-agents.md

**Astra — video · 30–45 s**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/runs/2026-09-07-mainnet-2strk-agents.md
and docs/privacy-model.md.
Facts: a third recipient got a deal-scoped grant with an expiry, reconstructed all three offers and
the settlement, and could not read two earlier deals in the same channel; the grant carries no
spending authority; expiry does not erase what was already read.
Create: 30-45 s, one deal through four views — public observer, payer, payee, authorized recipient.
For each view show one fact visible and one out of scope. The record view is not published; build
it from the run record and caption "Reconstruction from published run record."
Constraints: say "authorized recipient", never "auditor". Caption the companion note that the
configured pool auditor and the prover/RPC have separate trust roles. No vendor logos.
Deliver: P04_one_deal_four_views.mp4.
```

**Grok**

```text
Fetch: the run record above; docs/privacy-model.md; the Erebus account.
Facts: one deal-scoped grant with expiry; all three offers reconstructed; earlier deals unreadable;
no spending authority; expiry does not erase.
Task: three variants, a first-reply with the run-record link and the "authorized recipient, not an
independent auditor" caveat, and a founder-voice alternative.
```

**Do not claim:** auditor independence, data deletion, or that disclosure covers history.

---

## Day 7 · Sat 19 Sep 2026 — No post · Weekly review

**Owner:** Team. **Work:** inspect 24/72-hour outcomes and interview notes; optional founder post.
**Evidence:** continue if three interviews reveal a specific repeat problem.

---

# Week two — test activation and a paid offer

## Day 8 · Sun 20 Sep 2026 — P05 · An uncertain write

**Gate:** `[ ] Verified` mechanism · `[ ] Labelled` controlled mock failure
**Owner:** Poulav. Record a controlled uncertain-write example and the recovery flow.
**Evidence:** builder questions about reliability. **Time:** 14:00 UTC · **UTM:** `p05`

**Seed copy:**

> A payment request timed out. Did the payment fail?
>
> Not necessarily. Blindly submitting a new operation can make the problem worse.
>
> Erebus exposes operation IDs and reconciliation tools. Here is the recovery workflow: https://github.com/PoulavBhowmick03/Erebus

**Astra — video · 60–90 s**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/runs/2026-09-07-mainnet-2strk-agents.md,
docs/runs/2026-08-27-packaged-recovery-canary.md, and docs/status.md.
Facts: each write has a durable operation ID; a timeout is reconciled before any retry, so a blind
resubmit is the actual risk.
Create: 60-90 s. No public footage of a controlled failure exists, so animate the mechanism from
the run records and caption it "Reconstruction from the published recovery record."
Shots: uncertain state labelled "CONTROLLED MOCK FAILURE — uncertain status"; the state check; the
durable operation ID; resuming the same operation rather than resubmitting.
Constraints: never stage or imply a real payment failure; no fabricated receipt.
Deliver: P05_uncertain_write_recovery.mp4.
```

**Grok**

```text
Fetch: the run records above; docs/status.md; the Erebus account.
Facts: a durable operation ID per write; reconciliation before retry.
Task: three variants, a first-reply on how state is checked and the same operation resumed, and a
founder-voice alternative. No mainnet failure story.
```

**Do not claim:** a new run, or that a timeout is harmless.

---

## Day 9 · Mon 21 Sep 2026 — No post · Onboarding and offer

**Owner:** Team. **Work:** run outside mock sessions; discuss the $750 evaluation with qualified
budget owners. **Evidence:** objections, next meeting, decision owner, counterparty.

---

## Day 10 · Tue 22 Sep 2026 — P06 · Article one

**Gate:** `[ ] Verified` links · `[ ] Labelled` wording checked
**Owner:** Team. Publish "What Erebus keeps private" (Appendix B is the operator's draft). Pair
with one self-contained post. **Evidence:** article visits, interview requests.
**Time:** 17:00 UTC · **UTM:** `p06`

**Seed copy:**

> What does "private" actually hide?
>
> For Erebus, terms and amounts can be confidential while the channel relationship stays public.
>
> That distinction decides which problems the product can solve.
>
> A practical guide: https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/privacy-model.md

**Astra — still + 3-panel carousel**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/privacy-model.md and
https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/status.md.
Facts: terms and amounts can be confidential while the channel relationship stays public; the
disclosure is deal-scoped with expiry; Erebus is not escrow, not deferred delivery, not complete
anonymity.
Create: a 16:9 header ("What Erebus keeps private", Archivo 600, one cinnabar rule, the boundary
line) and three panels — public observer / parties / authorized recipient.
Constraints: keep the honest limits on the panels. No invented figures.
Deliver: P06_article_header_16x9.png, P06_carousel_1..3.png.
```

**Grok**

```text
Fetch: docs/privacy-model.md; docs/status.md; the Erebus account.
Facts: the article rejects "the entire transaction is private" in favour of "terms hidden,
relationship public".
Task: three variants pointing to the published article, a first-reply with the rejected hook and
the precise claim, and a founder-voice alternative. CTA: bring a workflow where rate
confidentiality matters.
```

**Do not claim:** production readiness or customer outcomes.

---

## Day 11 · Wed 23 Sep 2026 — P07 · Experiment announcement

**Gate:** `[ ] Verified` method and harness available before publishing
**Owner:** Experiment owner. Publish the preregistered method after the ten-trial harness check.
**Evidence:** reproducibility feedback; zero result claims. **Time:** 14:00 UTC · **UTM:** `p07`

**Seed copy:**

> Do agents really want privacy?
>
> We are testing tool choices across tasks, instructions, and privacy costs. Public settlement is correct in some cases.
>
> The protocol will be published before the results. What scenario should we include?

**Astra — still · study design**

```text
No fetch required. Render this frozen design: 3 model/version configurations x 4 contexts x 3 fees
x 2 instruction conditions x 2 orders x 5 repetitions = 720 episodes, with a ten-trial smoke test
labelled separately. Contexts: confidential rate; routine public expense; required public
reporting; sensitive amount with an acceptable public relationship. Costs: 0%, 1%, 5% simulated
fee — label them experimental treatments, not Erebus prices.
On-screen: "Protocol published before results. A null finding is publishable." No numeric result.
Deliver: P07_study_design.png.
```

**Grok**

```text
Fetch: the Erebus account (for tone). No result to quote.
Task: three variants asking readers which scenario to include, a first-reply with the method link
(or "study design" if the harness is unpublished), and a founder-voice alternative. Include
required-public-reporting cases. Announce no result.
```

**Do not claim:** any result. This is a method announcement.

---

## Day 12 · Thu 24 Sep 2026 — No post · Run and observe

**Owner:** Experiment owner. **Work:** run the frozen exploratory design within budget; watch
outside setup. **Evidence:** all trials, errors and installation blockers. Reduce or postpone the
study if it displaces customer discovery, and publish the revised design before running it.

---

## Day 13 · Fri 25 Sep 2026 — P08 · The economics question

**Gate:** `[ ] Verified` dated figures · `[ ] No invented numbers`
**Owner:** Poulav. Publish cost components with dates and scope; ask about acceptable payment size.
**Evidence:** payment ranges and latency requirements. **Time:** 17:00 UTC · **UTM:** `p08`

**Seed copy:**

> A private payment has more than one cost.
>
> Funding, negotiation rounds, settlement, proving, and support all matter.
>
> We are separating those costs before positioning Erebus for tiny payments. What size payment would your workflow need?

**Astra — still · dated cost table**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/runs/2026-09-07-mainnet-2strk-agents.md
and docs/runs/2026-08-31-mainnet-060-040-canary.md.
Facts (each with its date and source): the 7 Sep run used a 6 STRK pool fee per apply_actions and
about 35.8 STRK combined pool and network fees across six writes; the earlier 0.6 STRK canary is a
historical data point.
Create: 1920x1080, IBM Plex Mono, rows for funding/allowance, negotiation rounds, settlement
(apply_actions pool fee), proving, support.
On-screen: "Historical figures, not current marginal cost."
Constraints: separate pool fees, network fees and reusable setup. Do not imply Erebus is cheap or
expensive without the table.
Deliver: P08_cost_table.png.
```

**Grok**

```text
Fetch: the run records above; docs/status.md; the Erebus account.
Facts: the numbers above, dated. No current-price claim.
Task: three variants asking for the reader's payment size and latency need, a first-reply with the
dated table and the "historical, not present marginal cost" caveat, and a founder-voice
alternative.
```

**Do not claim:** current prices or a per-payment cost the record does not support.

---

## Day 14 · Sat 26 Sep 2026 — No post · Weekly review

**Owner:** Team. **Work:** review five outside mock runs and two replayed workflows; optional
founder reflection. **Evidence:** two budget owners in concrete pilot discussions targeted.

---

# Week three — connect research to operator needs

## Day 15 · Sun 27 Sep 2026 — P09 · Outside installation

**Gate:** `[ ] Verified` consented result (conditional)
**Owner:** Team. Share a permissioned installation lesson, or the recruitment fallback.
**Evidence:** unassisted completions and time to first mock deal. **Time:** 14:00 UTC · **UTM:** `p09`

**Seed copy (only after the session happens):**

> The most useful Erebus test this week: an outside developer trying the quickstart while we watched.
>
> We are documenting where they got stuck and what changed.
>
> If you build agents, we would like to watch your first mock run too.

**Recruitment fallback:**

> Can a new developer complete an Erebus mock deal without our help? We are looking for three builders to test the quickstart and tell us where it breaks.

**Astra — video or still**

```text
No source is public (this needs a real consented session). Produce the recruitment card instead:
1920x1080, black ground, Archivo 600 question "Can a new developer complete an Erebus mock deal
without our help?", one cinnabar rule, the repo URL, and "Looking for three builders."
If a session recording is provided later, use it: show the real unassisted install and the exact
stall point, keep the person anonymous unless they approved identification, and never invent a
stall or time.
Deliver: P09_recruitment.png (or P09_outside_installation.mp4 if footage exists).
```

**Grok**

```text
Fetch: the Erebus account. No session data is public.
Task: draft the recruitment version — three variants inviting three builders to test the quickstart
and report where it breaks — plus a first-reply and a founder-voice alternative. If a session
happened and was consented, use the observed stall and fix instead, and never invent one.
```

**Do not claim:** a specific person's identity without consent, or a completion that did not happen.

---

## Day 16 · Mon 28 Sep 2026 — No post · Scope paid pilots

**Owner:** Poulav. **Work:** agree one supported environment, one workflow, support cap,
deliverables. **Evidence:** written scope and payment intent, not vague interest.

---

## Day 17 · Tue 29 Sep 2026 — P10 · Operator controls

**Gate:** `[ ] Labelled` PROPOSED — enforcement has not shipped
**Owner:** Team. Show a proposed approval screen, clearly labelled; interview operators about
limits. **Evidence:** specific mandatory controls and their owner. **Time:** 17:00 UTC · **UTM:** `p10`

**Seed copy:**

> An agent choosing a payment route is only half the problem.
>
> Who sets its budget? Which counterparties are allowed? When must a human approve?
>
> For an Erebus operator workspace, which control would you need before the first payment?

**Astra — still · PROPOSED**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/production-gaps.md and
docs/roadmap.md (what is missing and what is planned). Copy the UI grammar from
https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/web/app/globals.css.
Create: a hypothetical approval screen — spending limit, counterparty allowlist, human-approval
threshold, a denied action.
On-screen (persistent, cinnabar): "PROPOSED DESIGN — NOT SHIPPED."
Constraints: no fake balances, no controls presented as available.
Deliver: P10_proposed_controls.png.
```

**Grok**

```text
Fetch: docs/production-gaps.md; docs/roadmap.md; the Erebus account.
Facts: these are proposals, not shipped behaviour.
Task: three variants asking which control an operator needs first, a first-reply stating the screen
is a proposed design, and a founder-voice alternative. Never imply enforcement shipped.
```

**Do not claim:** that any control has shipped, or that the workspace exists as a product.

---

## Day 18 · Wed 30 Sep 2026 — P11 · The privacy study

**Gate:** `[ ] Verified` results, or a method article if incomplete
**Owner:** Experiment owner. Publish the study if ready; else methods and limitations.
**Evidence:** reproductions and real scenarios. **Time:** 14:00 UTC · **UTM:** `p11`

**Seed copy:**

> "Agents prefer privacy" can hide several different findings.
>
> Did the model follow an instruction, favor the first tool, avoid a fee, or actually complete the confidential action?
>
> Our study separates those questions. The method and limits are below.

**Astra — still · results or method**

```text
Fetch: the Erebus account (context only). The study is not public.
If results exist: one table per model configuration, columns for attempted runs, public selections,
confidential selections, abstentions, successful actions, policy violations, plus cost trend and
instruction effect. Keep configurations separate; repeated runs are not additional models. Include
failures. Headline only from real data: "Under these tasks and settings, configuration A completed
the confidential action in N of M trials."
If results are incomplete: render the P07 frozen design table and label it
"STUDY DESIGN — no results yet".
Add: "A credible null finding is publishable."
Deliver: P11_study_results.png.
```

**Grok**

```text
Fetch: the Erebus account. No result is public.
Task: three variants (method-led / question-led; results-led only if numbers are verified), a
first-reply with denominator, model versions, scenario and failures, and a founder-voice
alternative. Never assert an inherent privacy preference.
```

**Do not claim:** that agents inherently prefer privacy, or any number whose denominator is unclear.

---

## Day 19 · Thu 1 Oct 2026 — No post · Workflow work

**Owner:** Poulav. **Work:** deliver an agreed pilot step or record why the customer has not
committed. **Evidence:** paid service work tracked separately from product use.

---

## Day 20 · Fri 2 Oct 2026 — P12 · A concrete customer problem

**Gate:** `[ ] Labelled` illustrative — scenario hypothetical
**Owner:** Team. Present the known-supplier scenario as illustrative and invite interviews.
**Evidence:** buyer conversations where the boundary is acceptable. **Time:** 17:00 UTC · **UTM:** `p12`

**Seed copy:**

> Your supplier relationship may already be public. Your negotiated rate may still be sensitive.
>
> That is one potential use for Erebus: confidential settlement between known counterparties.
>
> Does this occur in your actual payment workflow?

**Astra — still · illustrative**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/usecases.md and
docs/privacy-model.md.
Create: two fictional parties ("Service Provider" / "Buyer — illustrative") and a flow: offchain
delivery accepted, then confidential settlement of the negotiated rate. Draw the relationship edge
in visible grey labelled "public"; the rate is a grey #26262B bar labelled "confidential".
On-screen (persistent): "ILLUSTRATIVE — fictional parties."
Constraints: no logo, no invented saving, no real company.
Deliver: P12_illustrative_scenario.png.
```

**Grok**

```text
Fetch: docs/usecases.md; docs/privacy-model.md; the Erebus account.
Facts: fictional scenario; delivery accepted offchain before payment.
Task: three variants asking whether the reader's supplier relationships are public while the rate
is sensitive, a first-reply stating the scenario is fictional, and a founder-voice alternative.
CTA: show an anonymized example in a workflow interview.
```

**Do not claim:** an existing customer, a real logo, or a measured saving.

---

## Day 21 · Sat 3 Oct 2026 — No post · Weekly review

**Owner:** Team. **Work:** compare confidentiality vs reliability messages; aggregate buyer
objections. **Evidence:** keep one segment and one next product slice.

---

# Week four — test repeatability and commercial commitment

## Day 22 · Sun 4 Oct 2026 — P13 · Workflow before feature list

**Gate:** `[ ] Labelled` discovery — no customer claim
**Owner:** Team. Publish a discovery invitation or permissioned workflow case.
**Evidence:** repeated pain and one budget owner. **Time:** 14:00 UTC · **UTM:** `p13`

**Seed copy:**

> Before adding another Erebus feature, we want to understand one real payment workflow.
>
> Who approves it? What must stay confidential? How is it reconciled? What happens if it stalls?
>
> If your team already makes these payments, can we learn from your last one?

**Astra — still · discovery**

```text
No fetch required. Four questions in IBM Plex Mono, one per row with a thin rule: Who approves it?
What must stay confidential? How is it reconciled? What happens if it stalls? Header in Archivo
600: "One real payment workflow, before another feature." Footer: "Research invitation — no
customer implied." One cinnabar rule. No metrics.
Deliver: P13_workflow_questions.png.
```

**Grok**

```text
Fetch: the Erebus account (tone).
Task: three variants inviting the reader to describe their last payment of this kind, a first-reply
marking it a research invitation with no customer implied, and a founder-voice alternative. Ask
about approval, confidentiality, reconciliation and stalls.
```

**Do not claim:** an existing design partner unless one has consented.

---

## Day 23 · Mon 5 Oct 2026 — No post · Pilot follow-through

**Owner:** Team. **Work:** run scheduled evaluations; discuss the next use of the same workflow.
**Evidence:** a specific next payment or evaluation date.

---

## Day 24 · Tue 6 Oct 2026 — P14 · Visible product progress

**Gate:** `[ ] Verified` tested change (conditional) · `[ ] Recorded` before/after
**Owner:** Poulav. Show a tested improvement, or the labelled discovery fallback.
**Evidence:** outside task success before and after, if measured. **Time:** 17:00 UTC · **UTM:** `p14`

**Seed copy:**

> This week's Erebus update should make one thing easier: completing your first deal.
>
> We are using outside setup sessions to choose what to fix next.
>
> Which step is stopping you: install, funding, counterparties, or understanding the privacy boundary?

**Astra — still · before/after or discovery**

```text
Fetch: https://raw.githubusercontent.com/PoulavBhowmick03/Erebus/main/docs/status.md (in-flight
work) and the latest release notes.
If a tested change shipped: a side-by-side before/after of the same task, same environment, with
the exact change named and the release linked. Label "BEFORE" and "AFTER"; cinnabar only on the
changed step.
If nothing shipped: a discovery card listing the four stall points (install, funding,
counterparties, privacy boundary) headed "Which step is stopping you?"
Constraint: never render a fake improvement.
Deliver: P14_progress_before_after.png (or P14_stall_points.png).
```

**Grok**

```text
Fetch: docs/status.md; the Erebus account.
Task: three variants (shipped change or discovery fallback), a first-reply with before/after on
the same task if shipped or an honest "nothing shipped this week" if not, and a founder-voice
alternative. Never promise an easier experience without evidence.
```

**Do not claim:** a shipped improvement that is not tested, or an unmeasured task-success rate.

---

## Day 25 · Wed 7 Oct 2026 — P15 · A commercial invitation

**Gate:** `[ ] Verified` scope and capacity confirmed
**Owner:** Team. Publish the scoped service offer only when delivery capacity is available.
**Evidence:** qualified inquiries and signed scopes or payments. **Time:** 14:00 UTC · **UTM:** `p15`

**Seed copy:**

> We are opening two Erebus evaluation slots for teams with a real confidential-payment workflow.
>
> Fixed scope: workflow review, guided integration, and a go/no-go report.
>
> Proposed fee: $750. Experimental software; no production-readiness promise. Interested?

**Astra — still · offer card**

```text
No fetch required. Content: "Erebus confidential-settlement evaluation"; buyer profile (a technical
team already paying known suppliers with a concrete need to protect terms or amounts); fixed scope
(one workflow interview, one supported-environment setup, guided mock integration, documented
privacy/cost/recovery assessment, go/no-go report); duration five business days after
prerequisites; support cap six founder hours; proposed fee $750; exclusions (no security audit, no
custody, no production SLA, no custom chain support, no service-delivery guarantee, no unlimited
feature work); "Experimental software; no production-readiness promise. No mainnet funds needed for
the mock evaluation."
Label the fee "proposed". Do not present an unbuilt dashboard as a subscription.
Deliver: P15_pilot_offer.png, P15_pilot_offer_4x5.png.
```

**Grok**

```text
Fetch: the Erebus account (tone).
Task: three variants opening two scoped evaluation slots at a proposed $750, a first-reply listing
scope, exclusions and support cap, and a founder-voice alternative. Mark it a proposed service
offer, not a feature. No paying-customer claim.
```

**Do not claim:** paying customers, production readiness, or capacity you do not have.

---

## Day 26 · Thu 8 Oct 2026 — No post · Commercial calls

**Owner:** Team. **Work:** resolve scope, price, setup and trust objections; do not promise features
to close every call. **Evidence:** customer-specific reasons for accepting or declining.

---

## Day 27 · Fri 9 Oct 2026 — P16 · Month-one learning

**Gate:** `[ ] Verified` observations · `[ ] No invented numbers`
**Owner:** Team. Publish measured outcomes and the next product decision.
**Evidence:** external scrutiny; clear next action. **Time:** 17:00 UTC · **UTM:** `p16`

**Seed copy:**

> A month after launching Erebus, these are the questions that matter most to us:
>
> Can an outside team run it? Does the job repeat? Does someone value it enough to pay?
>
> We are sharing what we learned, including the assumptions that did not survive.

**Astra — still · learning report**

```text
No fetch required (these numbers are private to the operator; never invent them). Three questions
as the spine, each with a real, dated observation beneath it:
- Can an outside team run it? (outside mock completions)
- Does the job repeat? (same workflow reused)
- Does someone value it enough to pay? (pilot fees vs recurring revenue)
Four separate counters, never merged: visitors, outside mock runs, pilot fees, recurring revenue;
team demos listed separately or omitted. If the honest value is zero, render 0.
On-screen: "Month one: what we learned" / "Including the assumptions that did not survive. Next:
<narrow decision>."
Deliver: P16_month_one_learning.png.
```

**Grok**

```text
Fetch: the Erebus account (tone). The numbers come from the operator.
Task: three variants asking whether an outside team can run it, whether the job repeats, and
whether anyone will pay; a first-reply separating visitors, outside mock runs, pilot fees,
recurring revenue and team demos, stating zero honestly if zero; and a founder-voice alternative.
End on the next narrow product decision.
```

**Do not claim:** success metrics that are not measured, or that the experiment validated demand.

---

## Day 28 · Sat 10 Oct 2026 — No post · Weekly review

**Owner:** Team. **Work:** consolidate the month's post and company-level data; optional founder
reflection. **Evidence:** separate content reach, activation, service fees, MRR.

---

## Day 29 · Sun 11 Oct 2026 — No post · Retention conversations

**Owner:** Team. **Work:** ask evaluated companies to repeat the same job; propose $299/month only
if recurring value exists. **Evidence:** repeat-use commitment and support burden.

---

## Day 30 · Mon 12 Oct 2026 — No post · Product decision

**Owner:** Team. **Work:** apply the strategy report's gates; write a one-page continue, narrow or
stop decision. **Evidence:** target — two paid pilots, or one paid plus a documented second
purchase process.

After twenty qualified conversations, no concrete commitment is a reason to revisit the segment or
offer. Do not default to another dashboard feature. A specialist integration service may be the
viable outcome; a recurring product remains a separate hypothesis.

---

# Appendices — OPERATOR ONLY

> The appendices are for the human running the calendar. **Never send them to Astra or Grok.** They
> fetch their own sources. These are here so the operator can QA the output and hold the canonical
> wording.

## Appendix A — Week review sheets

### Post log

| Field | What to enter |
|---|---|
| Identity | Post ID, live URL, publication time, author, topic, format, asset, CTA, preparation hours. |
| At 24 and 72 hours | Views, replies, reposts, quotes, bookmarks, qualified interactions, attributed visits, completed next steps. |
| Distribution context | Community reshares, founder activity, concurrent events, changed claims, external links. |
| Outcome | Outside mock runs, interviews, evaluation requests, attributable with reasonable confidence; unknown recorded as unknown. |
| Decision | Repeat, revise, retire, or investigate; state the observation. |

### Company pipeline

| Stage | Required evidence |
|---|---|
| Relevant | A real team with a specific payment job; public relationship visibility acceptable. |
| Interviewed | Last workflow described, frequency, pain, current substitute. |
| Activated | Someone outside the founders completes the agreed mock or integration task. |
| Qualified to buy | Budget owner, counterparty, network, asset, timing, success criteria known. |
| Paid evaluation | Agreed scope and actual payment; tracked as service revenue. |
| Repeat use | Same company, same core workflow, no fresh bespoke project. |
| Subscription | Recurring agreement, billed period, payment, renewal, support cost. |

### Weekly decision sheet

Five questions: Which content produced a qualified next step? Where did outside builders stall?
Which buyer problem repeated? What did someone commit to? Which assumption should change? Use the
answers to pick next week's three or four originals.

### Handling a breakout or a quiet post

| Situation | Action |
|---|---|
| Unusual reach | Answer substantive questions, make the next action easy, capture the audience, publish a distinct follow-up within one or two days if there is new evidence. |
| Many views, few visits | Check CTA visibility, audience relevance, measurement; try a direct start link. |
| Visits, few mock completions | Watch an outside installation; fix the first observed blocker. |
| Mock use, no buyer interest | Separate developer curiosity from the payment job; interview operators. |
| Buyer interest, no payment | Ask about approval, budget, trust, fees, asset support, timing, competing tools; record the reason. |
| Low reach, strong conversations | Continue the subject; a small relevant audience can be commercially valuable. |
| No demand after twenty conversations | Narrow or stop the workspace hypothesis; consider integration services. |

## Appendix B — Article one draft: "What Erebus keeps private"

> Operator reference. Grok drafts the post copy from fetched facts; this is the long-form draft to
> publish and to QA wording against.

An agent pays a supplier. The relationship may already be known. The negotiated price may still be
commercially sensitive. That is a more precise starting point for payment privacy than asking
whether the entire transaction is "private."

Erebus combines confidential negotiation with shielded settlement for agent workflows on Starknet.
It also supports disclosure scoped to an individual deal. The important boundary is that Erebus
protects deal contents while channel relationships and transaction metadata remain visible.

### Different observers see different things

A public observer can see that a channel was opened with a counterparty, the submitting account,
and transaction timing. Funding movements and action patterns can also reveal information. The
current design should not be described as hiding who is connected to whom.

The parties need access to the terms they are negotiating. An authorized recipient can receive a
deal-scoped disclosure rather than a general key to a participant's history. That does not mean
every other infrastructure participant has no access: the configured pool auditor and the write
prover and RPC have their own roles and trust boundaries. The privacy model describes these
explicitly.

### Why disclose one deal

An operator may need to review an agreement, reconcile a payment, or share a record with someone
authorized to inspect it. Broad access is often unnecessary for that job. Erebus's v3 disclosure is
bound to a recipient, scoped to a deal, and includes expiry. It does not grant spending authority.

Expiry is an access condition, not a mechanism for making someone forget data they already read. A
record also needs careful interpretation: the current disclosure machinery does not independently
authenticate every business claim or prove that a service was delivered.

### A demonstration with two agent frameworks

In the recorded 7 September mainnet run, Claude Code acted as payer and Codex as payee. An offer of
1.6 STRK was countered at 2 STRK. The accepted deal settled for 2 STRK with 0.5 STRK change. A
third recipient then inspected the selected deal while earlier deals remained outside that grant's
scope. This was an operator-controlled demonstration, not a customer deployment, throughput at
scale, or endorsement by the companies behind the agent frameworks.

### Where this could be useful

One plausible job is paying a known service supplier after the work has been accepted through an
existing process. Both parties already know each other, and the relationship may be public. The
agreed rate is the sensitive information. That remains a product hypothesis. Private chat plus an
ordinary wallet may already solve enough of the problem for some teams; for others, setup, cost,
trust assumptions or network requirements may be unacceptable.

### Where it does not fit today

Erebus is not a general promise of anonymous counterparties. Its acceptance-and-payment action set
is not proof that a supplier delivered a file, completed compute, or will honor future access. It
should not be marketed as a general escrow or two-asset atomic exchange. The project is
experimental and unaudited; mainnet records establish bounded demonstrations, not production
readiness.

### Try the boundary before buying the story

A developer can start with mock mode without keys, a wallet, or gas. The browser simulation is
another way to understand the flow, but it is not a live chain transaction. If your team already
makes payments where negotiated rates should remain confidential, we would like to understand the
last real example.

## Appendix C — The agent experiment (operator reference)

Use the question "Do agents really want privacy?" as a question, not a conclusion. Ten trials can
test the harness; ten model decisions do not establish an inherent preference or a paying market.

**Frozen method**

| Item | Specification |
|---|---|
| Smoke test | Ten trials to validate the harness, labelled separately. |
| Exploratory design | 3 model/version configurations × 4 contexts × 3 fees × 2 instruction conditions × 2 orders × 5 repetitions = 720 episodes. |
| Contexts | Confidential rate; routine public expense; required public reporting; sensitive amount with an acceptable public relationship. |
| Cost treatments | 0%, 1%, 5% simulated fee. Experimental treatments, not Erebus prices. |
| Controls | Balanced tool descriptions, neutral names, randomized order, fresh runs, fixed settings, no brand persuasion. |
| Record | Tool selected, action completed, policy satisfied, abstention, error, cost, latency, model/version, prompt. |
| Product bridge | Separately demonstrate a chosen confidential action through Erebus in mock mode. Do not represent the simulator study as onchain use. |

Reduce the design before collecting if the budget is too high, and publish the change. Never run
the study with real customer data or uncontrolled agent access to funds.

## Appendix D — Paid evaluation offer (operator reference)

| Term | Proposed offer |
|---|---|
| Name | Erebus confidential-settlement evaluation |
| Buyer | A technical team already paying known suppliers, needing to protect terms or amounts. |
| Price | Test $750 once. $500–$1500 only when scope changes explicitly. |
| Duration | Five business days after prerequisites, by agreement. |
| Included | One workflow interview; one supported-environment setup; guided mock integration; documented privacy/cost/recovery assessment; go/no-go report. |
| Success | The buyer reproduces the agreed task and understands what blocks or enables deployment. |
| Support cap | Up to six founder hours; extra work is separately scoped. |
| Excluded | Security audit, custody, production SLA, custom chain support, service-delivery guarantee, unlimited feature development. |
| Transaction costs | No mainnet funds for the mock; live validation is separately scoped. |
| Next offer | $299 per team per month only after repeated value and readiness. |

**Discovery message**

> Hi, we are working on Erebus, confidential negotiation and settlement for agent workflows on
> Starknet. I saw your question about agent payments. Could you walk us through the last service
> payment your team made? We are trying to understand what must stay confidential and where the
> current process breaks. A 20-minute call would help; a short written example also works.

**Pilot proposal message**

> Based on the workflow you described, we can offer a fixed-scope Erebus evaluation for $750:
> review that workflow, set up one supported environment, reproduce it in mock mode, and deliver a
> go/no-go assessment covering privacy boundaries, costs, and recovery. Erebus is experimental and
> unaudited, so this is an integration evaluation. Would that deliverable be valuable enough for
> your team to fund?

## Appendix E — Video scripts (operator reference)

**Video one — a settled deal (P01), 35–45 s.** Result first (payer, payee, 2 STRK); offer 1.6 and
counter 2.0 from the record; acceptance and settlement; a separate recipient inspecting one
granted deal; boundary card; mock-start destination with the experimental label. Use actual
records, large type, captions; label edits; remove local secrets; no partner logos.

**Video two — one deal through four views (P02, P04), 30–45 s.** Public observer, payer, payee,
authorized recipient; one fact visible and one out of scope per view; companion note on the
configured auditor and prover/RPC roles. A four-view device is not a complete threat model.

**Video three — first use or recovery (P03, P05), 60–90 s.** Label the environment mock, testnet or
mainnet; show install, first command, result, success evidence. For recovery, show uncertain state
then reconciliation. Keep full commands in a linked guide; never invent a setup time.

## Appendix F — Profile and landing copy (operator reference)

**Proposed X bio:** Confidential negotiation + shielded settlement for agents on Starknet.
Deal-scoped disclosure. Experimental. Try the mock ↓

**Proposed hero:** Headline "Confidential deals between agents." Body: negotiate terms, settle on
Starknet, disclose a selected deal to an authorized recipient; channel relationships and
transaction metadata remain public. Primary action: try the simulation. Secondary: run a developer
mock. Label: experimental, unaudited; the simulation does not transact onchain.

**Commercial concept page:** headline "A settlement workspace for confidential supplier deals."
Body: an operator workflow for approved payments, status, reconciliation and scoped disclosure,
with MCP access. Action: discuss a scoped evaluation. Show what exists now, what the pilot
includes, and which interface features are proposed.

## Appendix G — Source map

**Public (Astra and Grok fetch these; verified 200 on `main`):**

`README.md` · `docs/status.md` · `docs/privacy-model.md` · `docs/roadmap.md` ·
`docs/production-gaps.md` · `docs/usecases.md` · `docs/reference.md` · `docs/runbook.md` ·
`docs/runs/` (all except the 2026-09-11 record) · `docs/assets/demo-thumbnail.jpg` ·
`docs/assets/erebus-overview.excalidraw.svg` · `demo/erebus-private-sprint.mp4` ·
`web/app/globals.css` · `web/app/icon.svg` · `web/lib/content.ts` · `https://erebusagents.live` ·
`@impoulav`.

**Local-only (never sent to Astra or Grok):** `artifacts/erebus-demo/` (Instrument Black frames,
deck, fonts, motion spec), `vendor/` assets, `~/Desktop/asset-brief/`, `~/Desktop/erebus-assets/`,
and `docs/runs/2026-09-11-mainnet-subagent-canary.md` (unpublished).

Earlier posts cite the frozen commit `c7795cd297e26bff71c7b524257b2c0723402d23`; resolve those
links against the public repo.

Source links identify exact pages or frozen repository files and are project documentation, not an
independent security audit. Public X counters are a point-in-time observation.
