#!/usr/bin/env python3
"""Build one erebus-cli request from an identity env file."""

import json
import os
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) not in (3, 4):
        raise SystemExit("usage: erebus-request.py <env-file> <method> [params-json]")

    values = {}
    for raw_line in Path(sys.argv[1]).expanduser().read_text().splitlines():
        line = raw_line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key] = value

    required = {
        "rpc_url": "STARKNET_RPC_URL",
        "prover_url": "PROVING_SERVICE_URL",
        "pool_address": "POOL_ADDRESS",
        "chain_id": "STARKNET_CHAIN_ID",
        "account_address": "AGENT_ADDRESS",
        "pool_key_file": "POOL_KEY_FILE",
        "account_key_file": "ACCOUNT_KEY_FILE",
        "state_dir": "EREBUS_STATE_DIR",
        "token": "TOKEN_ADDRESS",
    }
    missing = [source for source in required.values() if not values.get(source)]
    if missing:
        raise SystemExit("missing env values: " + ", ".join(missing))

    params = json.loads(sys.argv[3]) if len(sys.argv) == 4 else {}
    params["config"] = {target: values[source] for target, source in required.items()}
    if os.environ.get("EREBUS_RPC_URL_OVERRIDE"):
        params["config"]["rpc_url"] = os.environ["EREBUS_RPC_URL_OVERRIDE"]
    if os.environ.get("EREBUS_PROVER_URL_OVERRIDE"):
        params["config"]["prover_url"] = os.environ["EREBUS_PROVER_URL_OVERRIDE"]
    params["config"]["wire_version"] = values.get("EREBUS_WIRE_VERSION", "v3")
    print(json.dumps({"method": sys.argv[2], "params": params}))


if __name__ == "__main__":
    main()
