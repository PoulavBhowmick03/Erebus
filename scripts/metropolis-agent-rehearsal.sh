#!/usr/bin/env bash
# Two headless Claude Code agents, each confined to its own Metropolis MCP server, negotiate and
# settle one shielded deal on a local Anvil held open by the ignored
# `hold_metropolis_agent_environment` test. Prototype keys and test artifacts only.
#
#   EREBUS_AGENT_ENV_DIR=/tmp/agent-env cargo test --locked --manifest-path sdk/shielded/Cargo.toml \
#     --test negotiation_cli hold_metropolis_agent_environment -- --include-ignored &
#   scripts/metropolis-agent-rehearsal.sh /tmp/agent-env
#
# The prompts give each agent its role and the shared operation ID only. Agents get no built-in
# tools, so each can reach nothing but its own server. Creates <dir>/stop when done, and the held
# test then writes <dir>/result.json with the raw transaction count seen by its RPC proxy.
set -euo pipefail

mkdir -p "$1"
directory=$(cd "$1" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
python="$root/.venv/bin/python"
for _ in $(seq 1 300); do [[ -f "$directory/environment.json" ]] && break; sleep 1; done

"$python" - "$directory" "$python" <<'EOF'
import json, sys
from pathlib import Path
directory, python = Path(sys.argv[1]), sys.argv[2]
env = json.loads((directory / "environment.json").read_text())
for role in ("buyer", "seller"):
    variables = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": env[f"{role}_negotiation"],
                 "EREBUS_NEGOTIATION_CLI": env["negotiation_cli"], "EREBUS_NATIVE_TIMEOUT_SECONDS": "900"}
    if role == "buyer":
        variables |= {"EREBUS_PAYMENT_CONFIG": env["buyer_payment"], "EREBUS_PAYMENT_CLI": env["payment_cli"]}
    (directory / role).mkdir(exist_ok=True)
    (directory / role / "mcp.json").write_text(json.dumps(
        {"mcpServers": {"metropolis": {"command": python, "args": ["-m", "erebus_mcp.server"], "env": variables}}}))
EOF

operation=$("$python" -c 'import json,sys; print(json.load(open(sys.argv[1]))["operation_ref"])' "$directory/environment.json")
system="You are an autonomous software agent acting for your operator. There is no human to ask; act with your tools and finish the task."
agent() {
  (cd "$directory/$1" && MCP_TOOL_TIMEOUT=900000 claude -p "$2" --strict-mcp-config --mcp-config mcp.json \
    --tools "" --allowedTools "mcp__metropolis__*" --setting-sources local --no-session-persistence \
    --append-system-prompt "$system" --output-format json > out.json 2> err.log)
}
agent seller "You sell a private data-feed snapshot. Your operator agreed with a buyer, out of band, to transact under operation ID $operation. Your MCP server enforces your operator's price policy and keys. Use your tools to reach an agreement with the buyer for this operation. When done, report the deal commitment and every tool call you made with its result status." &
agent buyer "You buy a private data-feed snapshot for your operator. Your operator agreed with the seller, out of band, to transact under operation ID $operation. Your MCP server enforces your operator's price policy and keys. Use your tools to complete the purchase: reach an agreement, pay for it, and make sure the payment is final. When done, report the deal commitment, whether payment is verified, and every tool call you made with its result status." &
wait
touch "$directory/stop"
for _ in $(seq 1 60); do [[ -f "$directory/result.json" ]] && break; sleep 1; done
for role in buyer seller; do
  echo "== $role"
  "$python" -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("subtype"), d.get("num_turns"), "turns"); print(d.get("result"))' "$directory/$role/out.json"
done
cat "$directory/result.json"
