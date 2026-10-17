# Judging Erebus on Monad

Erebus on Monad is a CLI, a Python SDK and an MCP server, not a hosted web app. No login is
needed. Everything below runs on your own machine.

This page is kept current. If a step here disagrees with a linked guide, this page wins, and
please tell us.

## 1. Install check (about 2 minutes)

Needs macOS 11+ on Apple silicon or Linux x86_64, Python 3.11 or newer, and
[uv](https://docs.astral.sh/uv/).

```sh
uv venv --python 3.11 erebus-metropolis
uv pip install --python erebus-metropolis/bin/python \
  --no-config \
  --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
  --default-index https://pypi.org/simple \
  --index-strategy first-index \
  "erebus-mcp-server==0.3.0.dev2026101307"
erebus-metropolis/bin/erebus-payment --version
```

The last command prints a JSON line with `"status":"ok"` and both settlement modes
(`public-bound`, `shielded`). If downloads time out, set `UV_HTTP_TIMEOUT=300`. The release and
its signed manifest are on
[GitHub](https://github.com/PoulavBhowmick03/erebus-metropolis/releases/tag/v0.3.0.dev2026101307).

## 2. The full two-agent flow on Monad testnet

On `main` (after the judging deadline, so not part of the submission): the
[testnet quickstart](docs/metropolis-quickstart.md) runs the whole loop on Monad testnet with one
setup command. `erebus-init --metropolis quickstart up` creates a connected buyer, seller and
auditor, prints the one address to fund (about 2.15 testnet MON), takes fresh-wallet history
checkpoints and starts the seller's access service; two agents then fund, negotiate, settle,
verify and retrieve from the generated MCP configs. It needs a build of `main`; the published
`0.3.0.dev2026101307` does not include it.

## 3. Live on Monad testnet with the published release

Follow the [install guide](docs/metropolis-install.md) and the
[operations guide](docs/metropolis-operations.md). This needs dedicated testnet wallets, faucet
funds and two independent RPC providers.

Known limitation of the published release: a brand new wallet cannot make its first private
payment until its pool history has caught up. `main` fixes this with a paired finalized history
checkpoint (verified live on Monad testnet, two agents and two private payments); it ships in the
next release.

| | |
| --- | --- |
| Chain ID | `10143` (Monad testnet) |
| WMON pool | `0x8868f35140f3602c80e2c76132740b6f0c0b409d` |
| USDC pool | `0x863f540471c0ad33c68d1218f31e8c9fe9cd50fb` |

## Recorded testnet payments

All of these were run by our team; none is third-party acceptance.

- Live x402 payment, made with the earlier `0.3.0.dev4` release:
  [`0x8a0272e6…`](https://testnet.monadexplorer.com/tx/0x8a0272e69188ac3be52783ddfc4d4436496a58f2d5c63d808cd3320a1fa5df43)
- Live public-bound payment:
  [`0x1ba9ddb3…`](https://testnet.monadexplorer.com/tx/0x1ba9ddb363a0c916f8e41ca8eca7d4c524d1ba4722bf4ea37ed423547700ff73)
- Shielded WMON payment, made with source-built payment commands:
  [`0xe01666d8…`](https://testnet.monadexplorer.com/tx/0xe01666d8c4e16efe8195da8047674c71a325ee20ff250f02f4fdf570a04934fa)

## Videos

- Product: https://youtu.be/fIsfHgDBhxo
- Pitch: https://youtu.be/6Dwl0PfzdJ8
- Technical demo: https://youtu.be/ktBDsFbihTM

## What is and is not claimed

This is a test-only, single-operator, unaudited deployment. The Groth16 setup is a test setup, and
the live relayer loop is still being completed. Public-bound and x402 payments expose the
accepted terms and the parties; the shielded rail hides amounts and participant keys, while the
gas sender, funding and withdrawals stay public.

More context: [use cases](https://erebusagents.live/use-cases/) and the
[Monad docs](https://docs.erebusagents.live/monad).
