# Erebus Technical Demo

Target: a public operating video of no more than three minutes, on **Monad testnet only**.
Record the terminal and your narration, not a source-code walkthrough or slides.

The flow is the [testnet quickstart](docs/metropolis-quickstart.md). It is on `main` and not yet in
a published release: the published `0.3.0.dev2026101307` has no `quickstart`, no fresh-wallet
checkpoint and no `fund_wallet`. Record it from a build of `main` (the quickstart's "until then"
install path), and say so on screen.

## What You Can Claim

- A fresh buyer and seller were set up with one command and transacted on live Monad testnet
  within minutes: no history replay, through two MCP servers, with two private payments, the second
  paid from the first payment's change note. (Verified on `main` on 2026-10-15 and 2026-10-16;
  see `docs/metropolis-evidence/ws1-checkpoint-design.md` §6a and `docs/metropolis-roadmap.md`.)
- A fresh wallet starts from a paired finalized history checkpoint read from two RPC providers.
- Payment amounts and the participants' payment keys stay hidden in the shielded pool; the gas
  sender, deposits and withdrawals are public.

Do not claim:
- that the published release includes this;
- an audit, a production setup, or an outside tester's run;
- that an autonomous LLM chose the deal terms, if a script drove the agent tools.

## Before Recording

- Fund the buyer address `quickstart up` prints (about 2.15 testnet MON from
  <https://faucet.monad.xyz>). The seller needs none.
- Use a large font, hide notifications, keep shell tracing off, and keep the machine awake: a
  sleeping laptop suspends the seller's access service and the agents.
- Never show `keys/`, `wallet.json`, `payment.json`, `access.json`, RPC URLs with keys, grants, or
  note openings. `quickstart` output prints addresses and RPC hosts only.
- Do a full dry run first, with its own directory, and keep it as a fallback clip labelled
  `RECORDED EARLIER RUN`.

```sh
erebus-init --metropolis quickstart up --directory ./demo --pins ./pins --allow-agent-funding
erebus-init --metropolis quickstart status --directory ./demo
```

## Three-Minute Recording

### 0:00-0:25: One command

Show `quickstart up` finishing, the printed funding block, then `quickstart status`.
Say: "One command set up a buyer, a seller and an auditor on Monad testnet, connected to each other.
Each fresh wallet starts from a verified checkpoint of the pool, read from two independent RPC
providers, so there is no history replay. It prints exactly what to fund."

### 0:25-2:10: Two agents

Open two terminals, one per agent, started with `demo/agents/<role>/claude-code.mcp.json`, and
paste each its `prompt.md`. Wait until `/mcp` shows the server connected before pasting (interactive
Claude Code can otherwise send the prompt first). Start the seller first.
Keep an on-screen label: `MONAD TESTNET - BUILD OF MAIN (UNRELEASED)`.
Edit out waiting; never replace a failed step with output from another run.

Say: "The buyer funds a private note with one tool call. The seller waits for the buyer to connect.
They negotiate over an encrypted channel. The buyer settles once with a local zero-knowledge proof,
verifies the payment by observation, and retrieves the resource by the deal's ID. The second deal
spends the change from the first."

Point to: `fund_wallet` completing, the shared `deal_commitment`, `payment_verified: true`, and the
retrieved resource hash.

### 2:10-2:40: What the chain shows

Open the settlement transaction on the explorer (the buyer's gas address is the sender).
Say: "The explorer shows a transaction to the pool from the buyer's gas address. It does not show
the amount, or which payment keys paid and received."

### 2:40-2:55: Scope and close

Say: "This is an experimental, single-operator, unaudited testnet system, built from main ahead of
its next release. Funding, withdrawals and the outer gas sender remain visible."

Stop before three minutes. Upload to a public video host and test the link while signed out.

## Recording Readiness

Measured on live Monad testnet (2026-10-16/17; see the roadmap's WS3 entry for the run record):

- `quickstart up` from the installed package to a ready setup: about 77 s, including both
  checkpoints, `doctor` and the access service start.
- `fund_wallet` (wrap, approve, deposit, spendable note): 211-500 s.
- A deal from negotiation to verified payment: about 8-12 min; delivery about 6 min after the
  payment is verified (334 s and 357 s in the 2026-10-17 run from `main`).
- Cost: about 0.31 MON per buyer transaction (Monad charges the gas limit); five per run.
