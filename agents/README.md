# Reference agents

Owned by Ishita (CLAUDE.md, repo layout). Two agents — a buyer and a seller — running the
offer/counter/accept loop.

**Talks directly to a mock of `ErebusClient`, not through the MCP server.** `docs/ishita.md`
writes I1.2 as "against the mock" and I1.3 (the MCP server) separately; a literal reading
keeps this package's only dependency on the mock's *interface*
(`erebus_mcp.interface`/`erebus_mcp.mock_client`), not on MCP transport. The MCP server is
a second, independently-verified way to reach the same mock — see `mcp-server/README.md`.

```bash
uv sync --all-packages
uv run python agents/src/erebus_agents/demo.py
```

`--latency SECONDS` (default 0.2; real proof latency is ~29s per round) and `--rounds N`
are available — pass `--latency 29` to rehearse timing before recording the demo.

## What's here

- `policy.py` — `BuyerPolicy` / `SellerPolicy`. Pure, deterministic decision logic, no I/O.
  Threshold rules only, as scoped: one opening anchor, one counter, then accept or walk.
- `agent.py` — `run_negotiation()`: opens a channel, runs the negotiation loop bounded by
  `max_rounds`, settles or walks away, grants a viewing key, and reveals as a genuine
  third party (a fresh identity with no relationship to either agent) to prove the record
  reconstructs from shared state alone.
- `demo.py` — the CLI entry point above.

## Where it sits

```
agents → [mock ErebusClient today; sdk/py → sdk/rs → Starknet once I2.1 lands]
```

Built against the frozen `ErebusClient` interface (ARCHITECTURE §4), not the mock's
internals, so swapping the mock for the real seam should not require changes here.
