# Erebus

**Negotiate in darkness, settle in silence.**

Erebus lets two AI agents agree on a deal over an encrypted channel and pay for it through a
shielded pool, so competitors watching the chain don't learn the price. Afterwards, one
participant can show that single deal to an auditor without revealing anything else.

It is infrastructure, not an app: agents use it through an MCP server, a CLI and a Python SDK.
It is experimental and unaudited. Don't put value you care about through it.

**Judging for Monad Metropolis? Start at [JUDGES.md](./JUDGES.md).**

## Two backends

| | Starknet | Monad |
| --- | --- | --- |
| Network | mainnet | testnet (chain 10143) |
| Pool | StarkWare's STRK20 privacy pool | Erebus's own shielded pools for WMON and USDC |
| Release | `v0.3.0` | `0.4.0.dev2026101801` (prerelease) |
| Guide | [docs/setup.md](./docs/setup.md) | [docs/metropolis-install.md](./docs/metropolis-install.md) |

The agent side is the same on both: negotiate, authorize one agreement, settle once, recover by
watching the chain instead of paying again, and disclose one deal.

## Install on Monad

Python 3.11+, [uv](https://docs.astral.sh/uv/), and macOS on Apple silicon or Linux x86_64. Two
commands give you a connected buyer, seller and auditor on Monad testnet:

```sh
uv tool install --python 3.11 --no-config \
  --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
  --default-index https://pypi.org/simple --index-strategy first-index \
  "erebus-mcp-server==0.4.0.dev2026101801"
erebus-init --metropolis quickstart up --directory ./erebus --allow-agent-funding
```

`quickstart up` prints the one address to fund (about 2.15 testnet MON from the faucet), waits for
it, starts each fresh wallet from a verified history checkpoint, and writes MCP configs and prompts
for Claude Code, Codex and Cursor. Two agents then fund, negotiate, settle and retrieve. See the
[quickstart](./docs/metropolis-quickstart.md); keep the index order (the Metropolis index first,
PyPI only for dependencies). To install into a virtual environment instead:

```sh
uv venv --python 3.11 erebus-metropolis
uv pip install --python erebus-metropolis/bin/python --no-config \
  --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
  --default-index https://pypi.org/simple --index-strategy first-index \
  "erebus-mcp-server==0.4.0.dev2026101801"
```

To run a buyer and a seller on separate machines, follow the
[install guide](./docs/metropolis-install.md) and the [operations guide](./docs/metropolis-operations.md).

## Install on Starknet

```sh
uv tool install --python 3.12 \
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \
  erebus-mcp-server

claude mcp add erebus \
  --env EREBUS_BACKEND=mock \
  --env AGENT_ADDRESS=0xdemo \
  --env PROVING_SERVICE_URL=http://placeholder \
  --env EREBUS_SETTLEMENT_ROLE=both \
  -- erebus-mcp-server
```

## Evidence

Starknet mainnet. Amount and recipient can't be recovered from any of these:

| Date | Paid | Transaction |
| --- | --- | --- |
| 2026-08-31 | 0.8 STRK | [`0x72adebfc…`](https://starkscan.co/tx/0x72adebfcffdfd45bba66d2152c7e11b50107aa0fe2b4c9f1ea851da16ab6c6d) |
| 2026-08-31 | 0.6 STRK | [`0x79167f21…`](https://starkscan.co/tx/0x79167f213952fb33a57eec6457963fa7dd7ba3a38160d5ef04540e91bd4f97a) |
| 2026-09-07 | 2.0 STRK | [`0x2582f34a…`](https://starkscan.co/tx/0x2582f34a10f6a3c9f1fbfdad4622c2f8a79398a1782c7d425eac13698e7f5a7) |
| 2026-09-12 | 0.8 STRK | [`0x43329b0f…`](https://starkscan.co/tx/0x43329b0ff5d7865e0be3d4180e608229a5e8fd0d90312f4189f7aa9b8acadcd) |

Monad testnet, all run by our team:

| Payment | Transaction |
| --- | --- |
| x402, with the earlier `0.3.0.dev4` release | [`0x8a0272e6…`](https://testnet.monadexplorer.com/tx/0x8a0272e69188ac3be52783ddfc4d4436496a58f2d5c63d808cd3320a1fa5df43) |
| Public-bound | [`0x1ba9ddb3…`](https://testnet.monadexplorer.com/tx/0x1ba9ddb363a0c916f8e41ca8eca7d4c524d1ba4722bf4ea37ed423547700ff73) |
| Shielded WMON, with source-built payment commands | [`0xe01666d8…`](https://testnet.monadexplorer.com/tx/0xe01666d8c4e16efe8195da8047674c71a325ee20ff250f02f4fdf570a04934fa) |

Videos: [Starknet demo](https://drive.google.com/file/d/1zOkEJt08DwRiHeLIu4IaXCl1s8VRSKuu/view?usp=sharing),
[Monad product](https://youtu.be/fIsfHgDBhxo), [pitch](https://youtu.be/6Dwl0PfzdJ8) and
[technical demo](https://youtu.be/ktBDsFbihTM).

## What it hides and what it doesn't

Erebus hides the terms, not the relationship. Negotiation content and shielded amounts stay
private. That two parties opened a channel is public, and so are the gas sender, timing,
deposits and withdrawals. Public-bound and x402 payments on Monad expose the accepted terms and
the parties. Disclosure proves what was agreed and paid, never that anything was delivered.
The full boundary is in [docs/privacy-model.md](./docs/privacy-model.md) (Starknet) and
[docs/metropolis-threat-model.md](./docs/metropolis-threat-model.md) (Monad).

## What you can build

Find a seller, buy a service, pay per API call, prepay for metered calls, and show an audited
receipt. Each one runs on the participants' own servers:
[agents/README.md](./agents/README.md) and [erebusagents.live/use-cases](https://erebusagents.live/use-cases/).

## Repository

```
sdk/rs           Starknet Rust client; holds all key material
sdk/py           Thin Python binding over the Rust commands, no protocol logic
sdk/core         Chain-neutral agreement encoding and spending policy
sdk/shielded     Monad shielded payments, wallet, access, relayer and native commands
sdk/evm          Monad public-bound settlement and disclosure
contracts/evm    Monad pool contracts;  circuits/  the Circom proving circuits
mcp-server       MCP server exposing Erebus tools to any agent framework
agents           Reference agents and use cases
web              erebusagents.live
docs             Guides, run records, privacy model and the friction log
```

Design: [ARCHITECTURE.md](./ARCHITECTURE.md) (Starknet) and
[docs/metropolis-architecture.md](./docs/metropolis-architecture.md) (Monad). Where the stack
fought us: [docs/friction.md](./docs/friction.md).

## How this was built

Claude, Codex and OpenCode helped with implementation, tests, documentation and review. Every
change still needs human review, and none of that makes it audited.

The Monad work builds on Circom, circomlib, circomlibjs and snarkjs for circuits and proving,
arkworks and ark-circom for Rust proving, and ethers and solc for EVM tooling. The Starknet
backend composes `starkware-libs/starknet-privacy`. Versions are pinned in each package's
manifest and lockfile.

## Team

- **Poulav Bhowmick**: protocol, Cairo, Starknet and Monad. StarkWare track winner at ETHIndia 2024, Starknet Foundation grantee, Ethereum Protocol Fellow.
- **Ishita**: agents, orchestration, ML. Multi-agent systems, x402 and ERC-8004 agent-payment infrastructure.

## License

Apache-2.0. See [LICENSE](./LICENSE).
