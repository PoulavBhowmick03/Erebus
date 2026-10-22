#!/usr/bin/env python3
"""Qualify the installed `erebus-init --metropolis` operator flow outside the checkout. Sends no transactions.

    check-metropolis-init.py (--registry DIR | --index URL) --version V --pins-version P [--work NEW_DIR] [--online]

In a new directory with a new uv cache it installs `erebus-mcp-server==V` (from a local registry served over
loopback, or a public index), then, using only installed commands:
1. requires plain `erebus-init --help` to remain the Starknet onboarding and `--metropolis --help` to list
   pins, init, connect, doctor and auditor;
2. runs `erebus-init --metropolis pins --version P` (public HTTPS pins of a published release);
3. for each authorized pool: init a buyer and a seller (fixture gas addresses, no key file), connect the
   seller with a payload and the buyer from the seller's share, and set up an auditor (the default full history;
   with `--online`, the opt-in paired checkpoint of the auditor's cache when the package advertises it);
4. validates every generated file with the installed commands: `erebus-wallet notes`, both MCP servers
   listing their tools, `erebus-access-service` answering `/healthz`, and the disclosure `check`;
5. runs `doctor` (offline unless `--online`, which also reads both RPC endpoints and signer balances).
Fixture gas addresses are random and unfunded; nothing is broadcast.
"""

from __future__ import annotations

import argparse
import functools
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

RPC = ("https://testnet-rpc.monad.xyz", "https://rpc.ankr.com/monad_testnet")
MCP_PROBE = """
import asyncio, json, sys
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client
async def main(path, name):
    server = json.load(open(path))["mcpServers"][name]
    params = StdioServerParameters(command=server["command"], args=[], env={**server["env"], "PATH": "/usr/bin:/bin", "HOME": sys.argv[3]})
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            print(json.dumps(sorted(t.name for t in (await session.list_tools()).tools)))
asyncio.run(main(sys.argv[1], sys.argv[2]))
"""


class Quiet(SimpleHTTPRequestHandler):
    def log_message(self, *args) -> None:
        pass


def install(work: Path, version: str, registry: Path | None, index: str | None) -> Path:
    uv = shutil.which("uv")
    if uv is None:
        raise ValueError("uv is required")
    environment = {k: v for k, v in os.environ.items() if not k.startswith(("EREBUS_", "UV_", "PIP_", "VIRTUAL_ENV", "PYTHON"))}
    environment.update({"UV_CACHE_DIR": str(work / "uv-cache"), "UV_HTTP_TIMEOUT": "300", "HOME": str(work / "home")})
    (work / "home").mkdir()
    subprocess.run([uv, "venv", "--python", "3.11", str(work / "env")], env=environment, cwd=work, check=True, capture_output=True)
    server = None
    if registry is not None:
        server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(registry)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        index = f"http://127.0.0.1:{server.server_port}/simple/"
    try:
        subprocess.run([uv, "pip", "install", "--python", str(work / "env/bin/python"), "--no-config", "--index", index,
                        "--default-index", "https://pypi.org/simple", "--index-strategy", "first-index",
                        f"erebus-mcp-server=={version}"], env=environment, cwd=work, check=True, capture_output=True)
    finally:
        if server is not None:
            server.shutdown()
            server.server_close()
    return work / "env/bin"


def run(bins: Path, work: Path, arguments: list[str], stdin: str | None = None, binary: str = "erebus-init") -> dict:
    result = subprocess.run([str(bins / binary), *arguments], input=stdin, capture_output=True, text=True, cwd=work,
                            timeout=600, env={"PATH": f"{bins}:/usr/bin:/bin", "HOME": str(work / "home")})
    try:
        reply = json.loads(result.stdout)
    except ValueError:
        reply = {"raw": result.stdout[-400:], "stderr": result.stderr[-400:]}
    reply["exit"] = result.returncode
    return reply


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


INTERRUPT = """
import os, sys
from pathlib import Path
from erebus_mcp.metropolis_setup import operator_init as init
real = os.replace
def failing(source, target):
    if Path(target).name == "payment.json":
        raise OSError("injected interruption")
    return real(source, target)
init.os.replace = failing
sys.exit(init.main(sys.argv[1:]))
"""


def policy_and_endpoint_rehearsal(bins: Path, work: Path, buyer: Path, pool: str) -> dict:
    """Installed wallet-policy refusals, endpoint change, interrupted-change recovery and damaged-config doctor."""
    refusals = {}
    for name, extra in (("withdraw_without_recipient", ["--allow", "withdraw"]),
                        ("replace_without_fee_caps", ["--allow", "replace"]),
                        ("wrap_outside_wmon", ["--allow", "wrap"]) if pool != "wmon" else ("zero_cap", ["--maximum-amount", "0"])):
        arguments = ["--metropolis", "wallet-policy", "--directory", str(buyer), "--maximum-amount", "1000", *extra]
        refusals[name] = run(bins, work, arguments).get("status") == "error"
    require(all(refusals.values()), f"{pool} wallet-policy refusals: {refusals}")
    wallet = json.loads((buyer / "wallet.json").read_text())
    original = (wallet["rpc_url"], wallet["peer_rpc_url"])
    swapped = run(bins, work, ["--metropolis", "endpoints", "--directory", str(buyer), "--rpc-url", original[1], "--peer-rpc-url", original[0]])
    require(swapped.get("status") == "changed" and swapped.get("wallet_opens") is True, f"{pool} endpoint change: {swapped}")
    payment = json.loads((buyer / "payment.json").read_text())
    require((payment["rpc_url"], payment["peer_rpc_url"]) == (original[1], original[0]) and "relay" not in payment,
            f"{pool} payment endpoints not updated consistently")
    interrupted = subprocess.run([str(bins / "python"), "-I", "-c", INTERRUPT, "endpoints", "--directory", str(buyer),
                                  "--rpc-url", original[0], "--peer-rpc-url", original[1]], capture_output=True, text=True, timeout=300,
                                 env={"PATH": f"{bins}:/usr/bin:/bin", "HOME": str(work / "home")})
    require((buyer / ".endpoint-change/journal.json").exists(), f"{pool} interruption left no journal: {interrupted.stdout[-300:]}")
    flagged = run(bins, work, ["--metropolis", "doctor", "--directory", str(buyer), "--offline"])
    require(any("incomplete" in problem for problem in flagged.get("problems", [])), f"{pool} doctor missed the interrupted change")
    resumed = run(bins, work, ["--metropolis", "endpoints", "--directory", str(buyer), "--resume"])
    require(resumed.get("status") == "resumed" and resumed.get("applied_now") == ["payment.json"], f"{pool} resume: {resumed}")
    restored = json.loads((buyer / "payment.json").read_text())
    require((restored["rpc_url"], restored["peer_rpc_url"]) == original and not (buyer / ".endpoint-change").exists(),
            f"{pool} endpoints after resume")
    damaged = buyer / "wallet.json"
    saved = damaged.read_bytes()
    damaged.write_text("{damaged")
    try:
        sick = run(bins, work, ["--metropolis", "doctor", "--directory", str(buyer), "--offline"])
    finally:
        damaged.write_bytes(saved)
    require(sick.get("status") == "attention" and "{damaged" not in json.dumps(sick), f"{pool} doctor on damaged wallet.json: {sick}")
    healthy = run(bins, work, ["--metropolis", "doctor", "--directory", str(buyer), "--offline"])
    require(healthy.get("status") == "healthy", f"{pool} doctor after recovery: {healthy}")
    return {"policy_refusals": refusals, "endpoint_change": "changed and swapped back via interrupted change + resume",
            "doctor_on_damaged_wallet": sick.get("status"), "doctor_after_recovery": healthy.get("status")}


def pool_flow(bins: Path, work: Path, pins: Path, pool: str, port_base: int, online: bool) -> dict:
    root = work / pool
    root.mkdir()
    deployment = str(pins / "monad-testnet-shielded-v2.json")
    common = ["--pool", pool, "--bin", str(bins), "--pins", str(pins), "--deployment", deployment,
              "--rpc-url", RPC[0], "--peer-rpc-url", RPC[1]]
    report = {}
    for role, port in (("buyer", port_base), ("seller", port_base + 1)):
        extra = ["--maximum-price", "900000"] if role == "buyer" else []
        reply = run(bins, work, ["--metropolis", "init", "--role", role, "--directory", str(root / role),
                                 "--endpoint", f"127.0.0.1:{port}", "--signer-address", "0x" + os.urandom(20).hex(), *common, *extra])
        require(reply.get("status") == "initialized", f"{pool} {role} init: {reply}")
    payload = root / "payload.json"
    payload.write_text('{"fixture": true}\n')
    access_port = port_base + 2
    reply = run(bins, work, ["--metropolis", "connect", "--directory", str(root / "seller"), "--peer-share", str(root / "buyer/share"),
                             "--resource", "dataset.snapshot.v1", "--amount", "700000", "--payload-file", str(payload), "--port", str(access_port)])
    require(reply.get("status") == "connected", f"{pool} seller connect: {reply}")
    reply = run(bins, work, ["--metropolis", "connect", "--directory", str(root / "buyer"), "--peer-share", str(root / "seller/share"),
                             "--resource", "dataset.snapshot.v1", "--amount", "600000"])
    require(reply.get("status") == "connected", f"{pool} buyer connect: {reply}")
    # The default auditor replays full history and reads no RPC at setup. With --online, packages that advertise
    # the auditor checkpoint also qualify the opt-in, which reads both RPC providers.
    disclosure = run(bins, work, [], json.dumps({"method": "version"}), "erebus-shielded-disclosure")
    history = (["--history", "checkpoint"]
               if online and "checkpoint" in (disclosure.get("methods") or []) else [])
    reply = run(bins, work, ["--metropolis", "auditor", "--pool", pool, "--directory", str(root / "auditor"), "--bin", str(bins), "--pins", str(pins),
                             "--deployment", deployment, "--rpc-url", RPC[0], "--peer-rpc-url", RPC[1], *history])
    require(reply.get("status") == "auditor-ready", f"{pool} auditor: {reply}")
    if history:
        require(reply.get("history") == history[1] and isinstance(reply.get("history_from_block"), int),
                f"{pool} auditor history: {reply}")
    for role in ("buyer", "seller"):
        notes = run(bins, work, [], json.dumps({"method": "notes", "config_file": str(root / role / "wallet.json")}), "erebus-wallet")
        require(notes.get("status") == "ok", f"{pool} {role} wallet does not open: {notes}")
        probe = subprocess.run([str(bins / "python"), "-I", "-c", MCP_PROBE, str(root / role / "mcp.json"), next(iter(json.load(open(root / role / "mcp.json"))["mcpServers"])), str(work / "home")],
                               capture_output=True, text=True, timeout=120)
        tools = json.loads(probe.stdout.strip().splitlines()[-1]) if probe.returncode == 0 else []
        require("negotiate_deal" in tools and (role == "seller" or "settle_deal" in tools), f"{pool} {role} MCP tools: {tools} {probe.stderr[-300:]}")
        doctor = run(bins, work, ["--metropolis", "doctor", "--directory", str(root / role), *([] if online else ["--offline"])])
        expected = {"healthy"} if not online else {"healthy", "attention"}
        require(doctor.get("status") in expected, f"{pool} {role} doctor: {doctor}")
        if online:
            require(doctor["problems"] == ["the gas signer has no testnet MON yet"], f"{pool} {role} doctor problems: {doctor['problems']}")
        policy = run(bins, work, ["--metropolis", "wallet-policy", "--directory", str(root / role), "--maximum-amount", "1000"])
        require(policy.get("status") == "wallet-policy-written" and policy.get("allowed_methods") == ["notes", "sync"],
                f"{pool} {role} default wallet policy: {policy}")
        capabilities = run(bins, work, [], json.dumps({"method": "version"}), "erebus-wallet").get("result") or {}
        funding = run(bins, work, ["--metropolis", "wallet-policy", "--directory", str(root / role), "--maximum-amount", "1000",
                                   "--allow", "deposit"])
        expected_funding = "wallet-policy-written" if capabilities.get("deposit_operation_references") is True else "error"
        require(funding.get("status") == expected_funding, f"{pool} {role} deposit opt-in vs installed capability: {funding}")
        if role == "buyer":
            require("relay" not in json.loads((root / role / "payment.json").read_text()), f"{pool} direct buyer carries a relay field")
        report[role] = {"wallet_opens": True, "mcp_tools": tools, "doctor": doctor.get("status"),
                        "wallet_policy_default": policy.get("allowed_methods"),
                        "deposit_opt_in": funding.get("status"), "wallet_deposit_references": capabilities.get("deposit_operation_references") is True,
                        "doctor_checks": {k: v for k, v in doctor.get("checks", {}).items() if k != "owner_only"}}
    report["buyer"].update(policy_and_endpoint_rehearsal(bins, work, root / "buyer", pool))
    service = subprocess.Popen([str(bins / "erebus-access-service")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               env={"PATH": f"{bins}:/usr/bin:/bin", "HOME": str(work / "home"),
                                    "EREBUS_ACCESS_CONFIG": str(root / "seller/access.json")})
    try:
        health = None
        for _ in range(30):
            time.sleep(1)
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{access_port}/healthz", timeout=5) as response:
                    health = json.loads(response.read())
                break
            except OSError:
                continue
        require(bool(health) and health.get("status") == "ok", f"{pool} access service health: {health}")
    finally:
        service.terminate()
        service.wait(timeout=30)
    check = subprocess.run([str(bins / "python"), "-I", "-m", "erebus_mcp.metropolis_setup.disclosure_config", "check",
                            "--config", str(root / "auditor/disclosure.json"), "--deployment", deployment, "--pool", pool,
                            "--release", str(work / "release.json")], capture_output=True, text=True, timeout=120)
    report["access_health"] = health
    report["disclosure_check"] = json.loads(check.stdout).get("status") if check.stdout.strip().startswith("{") else check.stderr[-200:]
    require(report["disclosure_check"] == "ok", f"{pool} disclosure check: {report['disclosure_check']}")
    # Operator configs must be owner-only; the hosting manifest copy and the Rust-written descriptor are public.
    configs = [root / role / name for role in ("buyer", "seller") for name in
               ("wallet.json", "payment.json", "negotiate.json", "access.json", "mcp.json", "operator.json", "wallet-policy.json")] + [
               root / "auditor/disclosure.json"] + [p for p in root.rglob("*.key")]
    loose = [str(p.relative_to(root)) for p in configs if p.exists() and p.stat().st_mode & 0o077]
    require(not loose, f"{pool} configs readable by others: {loose}")
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--registry", type=Path)
    source.add_argument("--index")
    parser.add_argument("--version", required=True)
    parser.add_argument("--pins-version", required=True)
    parser.add_argument("--work", type=Path)
    parser.add_argument("--online", action="store_true")
    args = parser.parse_args()
    work = (args.work or Path(tempfile.mkdtemp(prefix="erebus-init-check-"))).resolve()
    work.mkdir(mode=0o700, exist_ok=args.work is None)
    checkout = Path(__file__).resolve().parents[1]
    try:
        bins = install(work, args.version, args.registry.resolve() if args.registry else None, args.index)
        if args.registry:
            shutil.copyfile(args.registry / "release.json", work / "release.json")
        plain = subprocess.run([str(bins / "erebus-init"), "--help"], capture_output=True, text=True, timeout=60,
                               env={"PATH": f"{bins}:/usr/bin:/bin", "HOME": str(work / "home")})
        metropolis = subprocess.run([str(bins / "erebus-init"), "--metropolis", "--help"], capture_output=True, text=True, timeout=60,
                                    env={"PATH": f"{bins}:/usr/bin:/bin", "HOME": str(work / "home")})
        require(plain.returncode == 0 and "--network" in plain.stdout and "--account" in plain.stdout, "plain erebus-init is no longer Starknet onboarding")
        require(metropolis.returncode == 0 and all(word in metropolis.stdout for word in ("pins", "init", "connect", "doctor", "auditor")),
                "erebus-init --metropolis does not expose the operator commands")
        pins = work / "pins"
        reply = run(bins, work, ["--metropolis", "pins", "--version", args.pins_version, "--directory", str(pins)])
        require(reply.get("status") == "pinned", f"pins: {reply}")
        if not args.registry:
            # A published release manifest keys per-platform package records under `platforms`.
            tag = {("Darwin", "arm64"): "macosx_11_0_arm64", ("Linux", "x86_64"): "linux_x86_64"}[(os.uname().sysname, os.uname().machine)]
            published = json.loads((pins / "release-manifest.json").read_text())
            entry = next(v for v in published["platforms"].values() if v["platform_qualification"]["wheel_tag"] == tag)
            (work / "release.json").write_text(json.dumps(entry))
        pools = sorted(json.loads((pins / "testnet-release.json").read_text())["hosting_manifests"])
        report = {"status": "verified", "version": args.version, "pins_version": args.pins_version, "work": str(work),
                  "outside_checkout": not work.is_relative_to(checkout), "fresh_cache": True,
                  "starknet_init_preserved": True, "pins": reply["files"],
                  "pools": {pool: pool_flow(bins, work, pins, pool, 19100 + 10 * index, args.online) for index, pool in enumerate(pools)},
                  "transactions_sent": 0, "labels": ["test-only", "single-operator", "unaudited"]}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "failed", "error": str(error)[:2000], "work": str(work)}))
        return 1
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
