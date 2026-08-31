# OpenAI Agents v0.2.0 release-candidate run — Sepolia, 2026-08-31

This run tested locally built `erebus-cli`, `erebus-sdk`, and `erebus-mcp-server` `0.2.0`
wheels from the release-candidate working tree based on commit
`306c2f2f6fbe04d04c7b750bb4f4e0e6002292f2`. The environment was macOS arm64 with Python
3.11 and `openai-agents==0.22.0`.

The packaged headless check passed before the live run:

- Protocol 4 and package version `0.2.0`.
- Exactly thirteen MCP tools.
- Operation IDs removed from model-visible schemas.
- Canonical caller intent persisted before the MCP write.
- Same-intent replay returned the same result.
- A local scripted model completed one OpenAI Agents SDK MCP tool loop.

## Live Sepolia result

Run scope: `sepolia-openai-rc-20260831-1`. The payer budget was bounded to
`100000000000000000` base units and the seller reserve to `80000000000000000` base units.
Both identities passed `doctor`; reconciliation required no action before the run.

Two channel writes completed and reconciled to chain effects:

| Side | Transaction | Reconciliation |
|---|---|---|
| Payer | `0x25e5732efcdb6bb91e19e7a5ccf610f7e836c95fbc507c8fd0a6a5aaccddd54` | `effect`, `next_action=none`, accepted at `1788160582` |
| Payee | `0x6af0b93d5c5deb8f7d520729426fa104c1cf302d17caa693e4e73e56a835169` | `effect`, `next_action=none`, accepted at `1788160605` |

The first model request then returned OpenAI API error `credit_balance_exhausted`. No offer
or settlement was attempted. Reusing the same run scope after credits are available will
reuse the persisted channel intents instead of creating replacement operations.

This is channel evidence, not a complete private settlement. Channel creation publicly
reveals the relationship. No negotiation terms or settlement amount were written in this
partial run.
