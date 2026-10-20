# Metropolis Tester Results

Fill this in after following the [tester brief](metropolis-tester-brief.md). Leave a field blank
rather than guess. Do not paste keys, `wallet.json`, `payment.json`, `access.json`, grants or
keyed RPC URLs anywhere in this file.

## Environment

| | |
|---|---|
| Date and your time zone | |
| OS and version | |
| CPU architecture (`uname -m`) | |
| uv version (`uv --version`) | |
| Python used by uv | |
| Release installed | `0.4.0.dev2026101801` / other: |
| Buyer agent client and version | |
| Seller agent client and version | |
| Model used by each agent, if known | |
| RPCs | default pair / your own (host names only): |
| Network notes (VPN, proxy, slow link) | |

## Timeline

Wall-clock time at the start and end of each step. Note retries.

| Step | Start | End | Retries | Notes |
|---|---|---|---|---|
| Read the brief and quickstart | | | | |
| `uv tool install` | | | | |
| `quickstart up` until it printed funding | | | | |
| Faucet and funding sent | | | | |
| `up` finished (checkpoints, doctor, service) | | | | |
| Seller agent connected and waiting | | | | |
| Buyer `fund_wallet` | | | | |
| `negotiate_deal` agreed | | | | |
| `settle_deal` | | | | |
| `recover_deal` | | | | |
| `retrieve_service_access` | | | | |
| `quickstart status` | | | | |
| `quickstart down` | | | | |

Total time from install to retrieval:

## Errors seen

One row per error, including ones that cleared on retry. Copy `code` and `next_action` exactly.

| Time | Tool or command | `code` | `next_action` | `retry_same_operation_ref` | What you did | Did it work? |
|---|---|---|---|---|---|---|
| | | | | | | |

Any `pending` results (tool, how many repeats, how long):

## Buyer transactions

Transaction hashes are public. Copy them from the tool results or an explorer.

| Step (wrap, approve, deposit, settle, other) | Transaction hash |
|---|---|
| | |

| | |
|---|---|
| Buyer address | |
| MON received from the faucet | |
| Buyer MON balance before | |
| Buyer MON balance after | |
| MON spent | |
| Did the seller spend any MON? | |

## Outcome

- Did the buyer retrieve the resource? yes / no
- Did the agents finish without you typing anything beyond the prompts? yes / no. If no, what did
  you type?
- Paste the `quickstart status` output (check it for keyed URLs first):

```text

```

## Where the docs were unclear

For each: the file and section, what you expected, and what happened.

1.
2.
3.

## Help

- Did you need help from us? yes / no
- If yes: with what, at which step, and what answer unblocked you.
- Did you read anything other than the brief and quickstart (source code, other docs, web
  search)? Which, and why?

## Anything else

