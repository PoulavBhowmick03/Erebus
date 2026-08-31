#!/usr/bin/env python3
"""Run one bounded negotiation over two packaged Erebus MCP servers on mainnet."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import sys
from pathlib import Path

from erebus_agents.mcp_loop import run_negotiation_over_mcp
from erebus_agents.policy import BuyerPolicy, SellerPolicy
from mcp.client.stdio import StdioServerParameters


REPO_ROOT = Path(__file__).resolve().parents[1]
PROVER_URL = "https://api.starkscan.co/v1/SN_MAIN/prove"
BUYER_HANDLE = "ch_b7afee5fd1f75ddc8425e8ca8b7879b4780588f81163a581f258289c238d9af8"
SELLER_HANDLE = "ch_c94f5afb8ad73af9f78baf4be45099ca7f57f28c230fa3a1104bef70b0d04fb2"
ONE_STRK = 10**18


def _parse_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw_line in path.read_text().splitlines():
        line = raw_line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key] = value
    return values


def _params(path: Path, role: str, run_name: str) -> StdioServerParameters:
    values = _parse_env(path)
    required = ("AGENT_ADDRESS", "EREBUS_STATE_DIR", "TOKEN_ADDRESS")
    missing = [key for key in required if not values.get(key)]
    if missing:
        raise ValueError(f"{path} is missing {', '.join(missing)}")
    env = {
        **os.environ,
        "EREBUS_PROVER_URL_OVERRIDE": PROVER_URL,
        "EREBUS_CALLER_INTENT_PATH": str(
            Path(values["EREBUS_STATE_DIR"]) / f"{run_name}-caller-intents.json"
        ),
    }
    return StdioServerParameters(
        command=str(REPO_ROOT / "scripts" / "erebus-mcp.sh"),
        args=[str(path), role],
        cwd=str(REPO_ROOT),
        env=env,
    )


async def _run(account_a: Path, account_b: Path, run_name: str) -> dict:
    if not os.environ.get("STARKSCAN_API_KEY"):
        raise RuntimeError("STARKSCAN_API_KEY must be present in the process environment")
    a = _parse_env(account_a)
    b = _parse_env(account_b)
    if a.get("TOKEN_ADDRESS") != b.get("TOKEN_ADDRESS"):
        raise RuntimeError("the two identity envs use different tokens")
    return await run_negotiation_over_mcp(
        buyer_params=_params(account_a, "payer", run_name),
        seller_params=_params(account_b, "payee", run_name),
        buyer_policy=BuyerPolicy(
            identity=a["AGENT_ADDRESS"],
            budget=ONE_STRK,
            deadline_seconds=3600,
            max_rounds=1,
        ),
        seller_policy=SellerPolicy(
            identity=b["AGENT_ADDRESS"],
            reserve=8 * 10**17,
            deadline_seconds=3600,
            max_rounds=1,
        ),
        buyer_address=a["AGENT_ADDRESS"],
        seller_address=b["AGENT_ADDRESS"],
        token=a["TOKEN_ADDRESS"],
        max_rounds=1,
        buyer_handle=BUYER_HANDLE,
        seller_handle=SELLER_HANDLE,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("account_a_env", type=Path)
    parser.add_argument("account_b_env", type=Path)
    parser.add_argument("--run-name", default="mainnet-starkscan-2026-08-31")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s", stream=sys.stderr)
    result = asyncio.run(_run(args.account_a_env, args.account_b_env, args.run_name))
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
