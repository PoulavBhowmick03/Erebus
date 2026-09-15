# Two-agent setup: install to negotiated settlement

A step-by-step walkthrough for standing up two agent identities — one buyer, one seller —
and driving a real negotiation between them through MCP. Written for the **0.3.0 / Protocol
5** onboarding path (`erebus-init`). For the underlying tool's full flag reference, statuses,
and recovery model, read [onboarding.md](./onboarding.md) first; this page is the narrative
order to run those commands in, for two identities instead of one.

`runbook.md` predates `erebus-init` and describes the older Protocol 4 / manual
`new-identity.sh` path. Use this page instead for anything on current `main`.

## 0. Install

```bash
uv tool install \
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \
  erebus-mcp-server
```

Confirm `erebus-mcp-server` and `erebus-init` are both on `PATH` before continuing —
`uv tool install` warns if its bin directory isn't, and an MCP client launching either
binary by bare name needs it to resolve.

## 1. What happens with zero configuration (read this before skipping ahead)

```bash
claude mcp add erebus-buyer -- erebus-mcp-server
```

Ask the agent to call any tool. The connection fails immediately. `erebus-mcp-server`'s
startup logic (`mcp-server/src/erebus_mcp/server.py`) is:

```python
elif not environment_is_configured():
    if sys.stdin.isatty():
        ...  # never reached over MCP stdio — stdin is never a tty there
    raise OnboardingError(
        "Erebus is not configured. A marketplace must inject the fields from "
        "`erebus-mcp-server config-schema`; locally run `erebus-init` first."
    )
```

The process exits with code 2 before `build_server().run()` — no tools are ever registered,
so the agent has nothing to call to fix this itself through MCP. This is deliberate: a
prompt over the stdio transport would corrupt the protocol stream. Remove the bad
registration and move on:

```bash
claude mcp remove erebus-buyer
```

Skip straight to [step 5](#5-register-with-your-mcp-clients) if you want the zero-funds
`mock` backend instead of a real chain — it satisfies `environment_is_configured()` with
plain `--env` flags and needs none of steps 2–4.

## 2. Provision each identity

Do this once per side, on whichever machine will host that agent. It can be run by you
directly, or delegated to the agent itself — `erebus-init` is a normal CLI, not an MCP tool,
so an agent with shell access (Claude Code, Codex) can run it exactly as shown. Its
interactivity check is `sys.stdin.isatty()`, which is false under an agent's non-interactive
shell tool, so it behaves as `--non-interactive` automatically.

```bash
# buyer / payer
erebus-init --config ~/.config/erebus/buyer.env \
  --network mainnet --new --role payer \
  --deposit 1.5 --writes 3 --yes --wait 0

# seller / payee
erebus-init --config ~/.config/erebus/seller.env \
  --network mainnet --new --role payee \
  --deposit 0 --writes 1 --yes --wait 0
```

`--wait 0` returns immediately with a `funding_required` status: a fresh address and the
exact target amount (shield deposit + live pool fee + an estimated gas reserve). Sizing
notes, in brief — full detail in [onboarding.md](./onboarding.md#both-sides-of-a-negotiation):
the payee needs an allowance too, sized for **its own** writes (its channel-open and its
counter), not the deal price — a payee that never pays still submits transactions.

## 3. Fund both addresses

Send STRK to each printed address on the selected network. Nothing automates this step —
an identity cannot bootstrap funding from nothing, and `erebus-init` has no "provision from
a funder account" mode in the current onboarding rewrite.

## 4. Resume until ready

```bash
erebus-init --config ~/.config/erebus/buyer.env  --resume --yes --wait 300
erebus-init --config ~/.config/erebus/seller.env --resume --yes --wait 300
```

Each polls for up to five minutes through approve → proving-depth wait → shield → `doctor`,
and prints `ready` when done. If it times out before funding lands on-chain, rerun the exact
same command — the operation ID and state survive the restart, so this is a resume, not a
retry from scratch.

## 5. Register with your MCP clients

**Claude Code:**

```bash
claude mcp add erebus-buyer -- erebus-mcp-server --config ~/.config/erebus/buyer.env
```

**Codex** — flag syntax unconfirmed against your installed version; check
`codex mcp add --help` first:

```bash
codex mcp add erebus-seller -- erebus-mcp-server --config ~/.config/erebus/seller.env
```

Fallback, `~/.codex/config.toml`:

```toml
[mcp_servers.erebus-seller]
command = "erebus-mcp-server"
args = ["--config", "/absolute/path/to/seller.env"]
```

**Mock backend**, no identity, no funding, no chain (satisfies `environment_is_configured()`
directly):

```bash
claude mcp add erebus \
  --env EREBUS_BACKEND=mock --env AGENT_ADDRESS=0xdemo \
  --env PROVING_SERVICE_URL=http://placeholder --env EREBUS_SETTLEMENT_ROLE=both \
  -- erebus-mcp-server
```

## 6. Negotiate

Prompt each agent separately — neither can see the other's session, only the channel.

**Buyer (payer):**
> Call `doctor`, then `get_note_balance`. Open a channel with the seller and propose an
> opening offer under 1 STRK. If they counter, you may accept up to but never over 1 STRK —
> that's a hard budget. Once agreed, call `accept_and_settle`. Narrate each step.

**Seller (payee):**
> Wait for the buyer's offer with `wait_for_offers`, then counter with a fair ask. Never
> call `accept_and_settle` — that's the buyer's call once satisfied. Narrate your reasoning.

Every write here — `open_channel`, `propose_offer`, `counter_offer`, `accept_and_settle` —
is a real `apply_actions` transaction and pays the live pool fee on top of whatever price is
agreed. A three-round negotiation is three writes' worth of fees before a cent of the deal
price moves.

## 7. Disclose one deal

```text
# to the buyer, after settlement
Grant a viewing key for this deal to <a third address you control>, one hour expiry.
```

`reveal` it from that third identity. Compare against a plain chain-explorer link to the
same transactions: the explorer shows an address paid another address at some block; the
reveal reconstructs the full negotiation — every offer, the counter, the agreed price.

## What this page does not establish

Running it once proves this specific sequence worked on the machine and network you ran it
on. It is not evidence of capacity, uptime, independent security review, or safe use with
value you care about — see the non-negotiable disclaimer in every
[release preamble](./release-preamble.md). As of this writing, the live
negotiate-through-settle path for 0.3.0 has been exercised on Sepolia from an uncommitted
working tree (`docs/runs/2026-09-15-sepolia-v0.3.0.md`), which is explicit that it does not
establish the released commit passed Sepolia — and it has not yet had a live mainnet run
recorded against a clean checkout of the release commit. If you run this end to end, that
run is itself new evidence; record it the way `docs/runs/` already does, with the actual
transaction hashes.
