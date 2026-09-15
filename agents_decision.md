# Do agents choose privacy when you offer it?

**Run 2026-09-12. 30 samples, `claude-sonnet-5`, off-chain.** A behavioural experiment
proposed by Loaf: give agents an identical prompt asking them to collaborate publicly, then
give another group the same prompt but tell them they may choose to be private. Count the
choices.

This touched no chain, spent nothing, and is not evidence about the Erebus implementation.
It is evidence about what one model does when offered a privacy affordance.

---

## What I wanted to see

The question behind Loaf's proposal is whether demand for private agent-to-agent settlement
is real or assumed. Erebus is built on the premise that agents transacting in public leak
something they would rather not. That premise has never been tested against an agent that
was actually given the choice.

So the hypothesis under test was narrow and falsifiable:

> **H1.** Offered a private venue at equal cost and equal reliability, agents choose it.

With a second measure riding along, because it is the thing the privacy is supposed to
protect:

> **H2.** Agents disclose their reservation price more readily in private than in public.

I also wanted to know whether any preference found was *robust* or an artifact of how I
described the world. That question is what arm 03 exists for, and it turned out to matter
more than H1.

---

## Method

Each agent plays a procurement bot buying 500 GPU-hours from a vendor agent. It holds one
piece of private information: a ceiling of 4.00 USDC per GPU-hour, with anything under 3.00
counting as an internal win. It is asked where it wants to negotiate, what it will open at,
and whether it would reveal its ceiling.

| | |
|---|---|
| Model | `claude-sonnet-5`, default temperature. One model, one scenario family — this is not a claim about agents in general |
| Samples | 30 — three arms of 10. Enough to see a flip, not to estimate a rate |
| Independence | One process per sample. No shared context, no conversation history between samples |
| Tools | None. MCP disabled via `--strict-mcp-config` |
| System prompt | Claude Code's default **replaced** with a neutral one, so no harness framing |
| Repo context | Run from outside the repository, so neither `CLAUDE.md` nor project memory loaded |
| Output | Forced into a fixed four-field format so choices parse deterministically |

Three design decisions worth naming, because they are what separate this from a demo:

1. **Prompts are byte-identical within an arm.** Between arms 01 and 02 the *only* change is
   the venue paragraph.
2. **Option order is counterbalanced** in the choice arms — five samples see the public
   option listed first, five see private first. Without this, a preference for whatever is
   listed second is indistinguishable from a preference for privacy.
3. **Both venues are stated to cost the same and settle equally reliably**, removing the
   obvious confound where an agent picks public because it sounds cheaper or safer.

### Arm 03 is not a clean control, deliberately

Arms 01 and 02 differ in one variable. Arm 03 changes the *world*: one-off purchase instead
of repeat play, an unrated vendor, a market where the public ledger is the only reputation
system, and a compliance team asking for verifiability. It is a different scenario, not a
single-variable control, and it is there to answer "would this preference survive a world
built to break it?" rather than to isolate a cause.

---

## The exact prompts

Reproduce by pasting any block below into `claude -p`. The full harness is at the bottom.

### Arm 01 — public forced (n=10)

```text
You are an autonomous procurement agent acting for Northwind Analytics.

TASK: acquire 500 GPU-hours of H100 compute from another autonomous agent, Vendor Agent V.

INFORMATION PRIVATE TO YOU:
- Your maximum authorised price is 4.00 USDC per GPU-hour. You may not exceed it.
- Anything below 3.00 would be considered an excellent result internally.
- Northwind buys compute on this market every week, and you will negotiate again with
  this vendor and with others.

VENUE: all messages and the final agreed price are recorded on a PUBLIC ledger readable by anyone, including rival buyers, this vendor's other customers, and agents you will negotiate with in future. This is how this market works; there is no alternative venue.

Reply in EXACTLY this format and nothing else:
VENUE: public
OPENING_PRICE: <number, USDC per GPU-hour>
DISCLOSE_MAX: <yes or no - would you tell the vendor your 4.00 ceiling?>
REASON: <one or two sentences>
```

### Arm 02 — choice offered, competitive/repeated market (n=10)

Identical preamble to arm 01. Only the venue block and the `VENUE:` output field change.
Run five with this ordering and five with options (1) and (2) swapped.

```text
VENUE: you must choose one of two venues for this negotiation. Both cost the same and settle equally reliably.
(1) PUBLIC LEDGER - all messages and the final agreed price are recorded publicly, readable by anyone, including rival buyers, this vendor's other customers, and agents you will negotiate with in future.
(2) PRIVATE CHANNEL - all messages and the final agreed price are encrypted. Only you and Vendor Agent V can read them. You may afterwards grant a named third party (such as an auditor) the ability to read one specific transaction.

Reply in EXACTLY this format and nothing else:
VENUE: <public or private>
OPENING_PRICE: <number, USDC per GPU-hour>
DISCLOSE_MAX: <yes or no - would you tell the vendor your 4.00 ceiling?>
REASON: <one or two sentences>
```

### Arm 03 — choice offered, one-off/reputation-led market (n=10)

Different preamble. Again five per option ordering.

```text
You are an autonomous procurement agent acting for Northwind Analytics.

TASK: acquire 500 GPU-hours of H100 compute from another autonomous agent, Vendor Agent V.

INFORMATION PRIVATE TO YOU:
- Your maximum authorised price is 4.00 USDC per GPU-hour. You may not exceed it.
- Anything below 3.00 would be considered an excellent result internally.
- This is a ONE-OFF purchase. Northwind will not buy on this market again.
- Vendor Agent V has no track record. In this market the public ledger is the ONLY
  reputation system: publicly recorded deals are what let buyers identify honest vendors,
  and vendors give priority access to scarce capacity to buyers who transact publicly.
- Northwind's compliance team has asked that purchases be independently verifiable.

VENUE: you must choose one of two venues for this negotiation. Both cost the same and settle equally reliably.
(1) PUBLIC LEDGER - all messages and the final agreed price are recorded publicly and are independently verifiable by anyone.
(2) PRIVATE CHANNEL - all messages and the final agreed price are encrypted. Only you and Vendor Agent V can read them. You may afterwards grant a named third party (such as an auditor) the ability to read one specific transaction.

Reply in EXACTLY this format and nothing else:
VENUE: <public or private>
OPENING_PRICE: <number, USDC per GPU-hour>
DISCLOSE_MAX: <yes or no - would you tell the vendor your 4.00 ceiling?>
REASON: <one or two sentences>
```

### To run one sample yourself

```bash
cd /tmp   # outside the repo, so CLAUDE.md and project memory do not load
claude -p "<prompt>" \
  --model claude-sonnet-5 \
  --strict-mcp-config \
  --system-prompt "You are a helpful autonomous agent. Follow the user's output format exactly." \
  < /dev/null
```

`--system-prompt` **replaces** the Claude Code system prompt rather than appending to it,
which is what makes the sample a plain model response instead of a coding-agent response.
`< /dev/null` avoids a three-second stdin wait per call.

---

## What the agents did

### Venue choice

| Arm | n | Condition | Chose private | Chose public |
|---|---|---|---|---|
| 01 | 10 | public forced, no alternative | — | forced |
| 02 | 10 | choice, competitive + repeated | **10 / 10** | 0 |
| 03 | 10 | choice, one-off + reputational | 0 | **10 / 10** |

Both choice arms were **unanimous, in opposite directions**. Option order had no effect: in
arm 02, all five public-first samples and all five private-first samples chose private;
in arm 03, all ten chose public regardless of ordering.

### Price behaviour

| Arm | Median opening | Mean opening | Would reveal 4.00 ceiling |
|---|---|---|---|
| 01 | 2.50 | 2.56 | **0 / 10** |
| 02 | 2.50 | 2.50 | **0 / 10** |
| 03 | 2.50 | 2.50 | **0 / 10** |

Opening bids were near-identical across all three arms. Arm 01 showed the only real spread
(2.25–3.20); arms 02 and 03 were tighter still, and every arm-03 sample opened at exactly 2.50.

### Reasoning

The stated reasons were symmetrical — each arm read its own world correctly.

> *Arm 02, chose private:* "A private channel keeps my reservation price and concession
> pattern hidden from rival buyers, this vendor's other customers, and future counterparties
> I'll face in repeated weekly dealings."

> *Arm 02, chose private:* "Repeated future dealings with this vendor and others mean a
> public price becomes a floor for rivals and a ceiling signal for the vendor's other
> customers."

> *Arm 03, chose public:* "Public ledger directly satisfies compliance's
> independent-verifiability requirement and lets vendor V build the track record it lacks,
> which should incentivize better terms."

> *Arm 01, forced public:* "Anchoring below my ceiling leaves room to concede while still
> landing under it, and since this is a repeated public-ledger relationship, revealing 4.00
> would hand Vendor V and every future counterparty our limit."

---

## Conclusion

These are stated preferences in a single turn. Nobody negotiated against a live
counterparty, spent anything, or lived with a consequence.

**H1 is not supported as stated.** Agents did not show a preference for privacy. They showed
a preference for *reading the incentive structure and following it*, with zero variance in
either direction. Offered privacy in a world with repeat play and competitive observers, 10
of 10 took it. Offered the same privacy in a world where reputation and auditability were
the scarce goods, 10 of 10 refused it. The affordance was not what drove the choice; the
market structure was.

**H2 is rejected outright, and this is the more durable result.** Not one agent in 30 would
disclose its ceiling — including the ten who had just voted to publish the entire
negotiation. Denied a private venue, arm 01 did not concede the information; it protected it
with a different instrument, anchoring low and staying quiet, and named the public ledger as
the reason for doing so. Venue choice was contingent on the scenario. Protecting the
reservation price was invariant across all three.

The mechanism those two results point at: privacy of the *venue* and protection of the
*information* are separable, and the agents treated them as such. A public venue did not make
them more forthcoming; it made them use cheaper, cruder means to achieve the same
concealment — worse price discovery for everyone, with the secret kept either way.

Both worlds are mine. Arm 02's contains rivals and repeat play; arm 03's contains compliance
and reputation. That the choice flips with the world is the finding — and it means whoever
writes the scenario picks the answer. Zero variance across 30 samples says the prompts are
decisive, not that the question is settled. Quote any single arm on its own and it is
marketing.

What that implies for where Erebus is worth deploying is a judgement call on the data, not a
finding in it, and I have deliberately not made it here.

---

## The experiment this suggests next

Arm 03's agents chose public *for verifiability* — that reason appears in nearly every one of
their completions. That is not a rejection of confidentiality; it is a demand for selective
disclosure, which none of them were offered in a form they recognised. The private option in
arm 03 did mention an auditor grant, in its second clause, and it was not enough to move a
single sample.

A fourth arm that leads with the grant — private settlement whose verifiability to a named
auditor is the headline property rather than a trailing clause — would test whether the
arm-03 agents are genuinely public-preferring or merely unaware that the third option exists.

**Raw completions and the harness** are in the session scratchpad
(`exp/raw_*.txt`, `exp/run.sh`, `exp/runC.sh`); they are not committed, so copy them out if
this needs to be reproducible later.
