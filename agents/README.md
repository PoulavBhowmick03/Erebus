# Reference agents

Owned by Ishita (CLAUDE.md, repo layout). Two agents, a buyer and a seller, running the
offer/counter/accept loop. Two ways to run them:

**`demo.py`** talks directly to a mock of `ErebusClient` — fast, deterministic rehearsal,
no MCP transport involved (`erebus_mcp.interface`/`erebus_mcp.mock_client`).

**`demo_mcp.py`** runs the same policies as real MCP clients against three live
`server.py` subprocesses (buyer, seller, auditor), over stdio. This is the "any framework
can drive it" claim exercised for real, not simulated.

```bash
uv sync --all-packages
uv run python agents/src/erebus_agents/demo.py       # mock rehearsal
uv run python agents/src/erebus_agents/demo_mcp.py    # real MCP servers
```

`demo.py` takes `--latency SECONDS` (default 0.2; real proof latency is ~29s per round,
pass `--latency 29` to rehearse timing). Both take `--rounds`, `--budget`, `--reserve`.

## What's here

- `policy.py`. `BuyerPolicy` / `SellerPolicy`. Pure, deterministic decision logic, no I/O.
Threshold rules only. The buyer only names amounts at or below its spendable total (settlement
covers the price and returns change); the seller never accepts because the accepting identity
pays, and confirms agreement by countering at the buyer's amount so the buyer can accept a
seller-authored offer.
- `agent.py`. `run_negotiation()`: same loop against `MockErebusClient` directly, plus
  the auditor reveal.
- `mcp_loop.py`. `run_negotiation_over_mcp()`: the same loop, driven by real MCP tool
  calls over three subprocess servers instead of direct mock calls.
- `demo.py` / `demo_mcp.py`, the two CLI entry points above.

## Where it sits

```
reference policy rehearsal → mock ErebusClient
reference policy rehearsal (real transport) → MCP → mock/seam ErebusClient
external autonomous agents → MCP → sdk/py → sdk/rs → Starknet
```

Built against the frozen `ErebusClient` interface (ARCHITECTURE §4), not the mock's
internals, so swapping the mock for the real seam should not require changes here.
