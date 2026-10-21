#!/usr/bin/env python3
"""Installed two-agent workflow gate on a loopback Anvil chain 10143 (rehearsal pins unless prepublication).

    rehearse-metropolis-workflow.py --registry DIR --version V --authorized-pins DIR --work NEW [--pools wmon,usdc]
                                    [--deployment-journal DIR] [--prepublication-authorization FILE]

It installs `erebus-mcp-server==V` from the local candidate registry into a new environment outside the checkout,
then re-runs itself under that environment's Python (`-I`, so nothing is imported from the checkout) and:

1. starts Anvil on 127.0.0.1 with chain id 10143 and replays the six signed production deployment transactions
   from the deployment journal, after placing local mock token code at the two asset addresses. Pools, verifiers
   and Poseidon land at their production addresses with their production runtime hashes, so the publisher-signed
   hosting manifests apply unchanged. The asset code is NOT canonical WMON or Circle USDC: it is the repository's
   MockWMON / MockERC20, so token behaviour (decimals, fees, blocklists, upgrades) is the mock's only;
2. writes a local deployment record (production addresses and runtime pins; local deployment blocks and hashes);
3. creates rehearsal pins with the installed `erebus-init --metropolis pins --rehearsal-release` (exact candidate
   release.json and the published authorization's signed pool manifests);
4. per pool, generates seller, buyer and auditor directories with installed `erebus-init` commands only (no
   hand-edited JSON), funds signer gas with `anvil_setBalance` and, for the ERC-20 pool, mints mock tokens;
5. drives separate installed MCP servers: policy-bound funding (approve/wrap/deposit), denials, concurrent
   negotiation, one settlement through a dropped broadcast response, buyer restart and recovery by observation,
   seller access-service restart and retrieval, a seller-exported scoped grant and auditor `verify_payment`;
6. on the first pool also: a subprocess timeout during settlement with a retained reservation, backup and
   restoration of the buyer directory, an interrupted endpoint change resumed, and an expired-offer/late-delivery
   pair. Payment and delivery are asserted separately; every eth_sendRawTransaction through the operators'
   RPC endpoints is counted.

Fixture keys are random and stay under the work directory; every endpoint is loopback; no public RPC is used.
The sponsored-relayer gate is not covered here: it needs the installed relayer fixture (not an in-process service).
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import importlib.util
import json
import os
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import threading
import time
import urllib.request
from contextlib import AsyncExitStack
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

CHECKOUT = Path(__file__).resolve().parents[1]
# The signed production deployment (public, already broadcast). Not committed: native CI needs it supplied.
JOURNAL = CHECKOUT / ".local/metropolis/candidate-20261010/live-deployment-journal"
DEPLOYMENT = CHECKOUT / "contracts/evm/deployments/monad-testnet-shielded-v2.json"
EVM_OUT = CHECKOUT / "contracts/evm/out"
LABELS = ["test-only", "single-operator", "unaudited", "rehearsal-pins", "not-publication-evidence", "local-anvil"]
MOCKS = {"wmon": ("MockWMON.sol", "MockWMON"), "usdc": ("MockERC20.sol", "MockERC20")}
AMOUNTS = {"wmon": {"price": 10**15, "deposit": 4 * 10**15, "cap": 5 * 10**15},
           "usdc": {"price": 700, "deposit": 3000, "cap": 5000}}
PAYLOAD = b"erebus metropolis rehearsal resource\n" * 64


class Failure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def private(path: Path, data: bytes) -> Path:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(data)
    return path


# ---- chain -------------------------------------------------------------------------------------------------------

def rpc(url: str, method: str, params: list, timeout: float = 30) -> object:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    request = urllib.request.Request(url, body, {"content-type": "application/json"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        reply = json.loads(response.read())
    if "error" in reply:
        raise Failure(f"{method}: {reply['error']}")
    return reply["result"]


def receipt(url: str, transaction: str, seconds: float = 60) -> dict:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        found = rpc(url, "eth_getTransactionReceipt", [transaction])
        if found:
            return found
        time.sleep(0.2)
    raise Failure(f"no receipt for {transaction}")


class Proxy:
    """A loopback JSON-RPC endpoint in front of Anvil that counts broadcasts and injects one fault at a time."""

    def __init__(self, upstream: str, name: str) -> None:
        self.upstream, self.name = upstream, name
        self.sends: list[str] = []
        self.requests = 0
        self.drop_next_send_response = False
        self.stall_next_send_seconds = 0.0
        self.lock = threading.Lock()
        proxy = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers.get("content-length", "0")))
                calls = json.loads(body)
                methods = [call.get("method") for call in (calls if isinstance(calls, list) else [calls])]
                drop, stall = False, 0.0
                with proxy.lock:
                    proxy.requests += 1
                    if "eth_sendRawTransaction" in methods:
                        drop, proxy.drop_next_send_response = proxy.drop_next_send_response, False
                        stall, proxy.stall_next_send_seconds = proxy.stall_next_send_seconds, 0.0
                if stall:
                    time.sleep(stall)
                request = urllib.request.Request(proxy.upstream, body, {"content-type": "application/json"})
                with urllib.request.urlopen(request, timeout=60) as response:
                    data = response.read()
                if "eth_sendRawTransaction" in methods:
                    replies = json.loads(data)
                    for reply in (replies if isinstance(replies, list) else [replies]):
                        if isinstance(reply.get("result"), str):
                            with proxy.lock:
                                proxy.sends.append(reply["result"])
                if drop:
                    self.close_connection = True
                    self.connection.shutdown(socket.SHUT_RDWR)
                    return
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


class Chain:
    def __init__(self, work: Path) -> None:
        self.port = free_port()
        self.url = f"http://127.0.0.1:{self.port}"
        self.log = open(work / "anvil.log", "wb")
        self.accounts = work / "anvil-accounts.json"
        # A random mnemonic: fixture keys exist only in this owner-only work directory and are never printed.
        self.process = subprocess.Popen(["anvil", "--host", "127.0.0.1", "--port", str(self.port), "--chain-id", "10143",
                                         "--block-time", "1", "--slots-in-an-epoch", "1", "--accounts", "16",
                                         "--mnemonic-random", "--config-out", str(self.accounts)],
                                        stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                if int(str(rpc(self.url, "eth_chainId", [])), 16) == 10143:
                    break
            except OSError:
                time.sleep(0.1)
        else:
            raise Failure("anvil did not start")
        self.proxies = {name: Proxy(self.url, name) for name in ("primary", "peer", "replacement")}

    def fixture_keys(self) -> list[tuple[str, str]]:
        """Anvil accounts 1..15 (account 0 funds mock mints); private keys stay in the work directory."""
        config = json.loads(self.accounts.read_text())
        self.accounts.chmod(0o600)
        pairs = list(zip(config["available_accounts"], config["private_keys"]))[1:]
        return [(address.lower(), key) for address, key in pairs]

    def sends(self) -> int:
        return sum(len(proxy.sends) for proxy in self.proxies.values())

    def close(self) -> None:
        for proxy in self.proxies.values():
            proxy.close()
        self.process.terminate()
        self.process.wait(timeout=30)
        self.log.close()

    def deploy(self, work: Path, journal: Path) -> tuple[Path, dict]:
        """Replay the signed production deployment; returns a local deployment record and the replay evidence."""
        record = json.loads(DEPLOYMENT.read_text())
        mocks = {}
        for pool in record["pools"]:
            source, contract = MOCKS[pool["name"]]
            runtime = json.loads((EVM_OUT / source / f"{contract}.json").read_text())["deployedBytecode"]["object"]
            rpc(self.url, "anvil_setCode", [pool["asset"], runtime])
            mocks[pool["name"]] = {"address": pool["asset"].lower(), "code": f"{source}:{contract}",
                                   "canonical_asset_runtime_keccak256": pool["asset_runtime_keccak256"],
                                   "note": "local mock code, not the canonical testnet asset"}
        replayed = []
        for index in range(6):
            signed = json.loads((journal / f"{index}-signed.json").read_text())
            if index == 0:
                sender = rpc(self.url, "eth_getTransactionByHash", [signed["hash"]])
                require(sender is None, "the chain already holds the deployment")
            replayed.append(signed)
        deployer = "0x5b6459b961baec43a40c305099365d767793e8ad"
        rpc(self.url, "anvil_setBalance", [deployer, hex(10**21)])
        receipts = []
        for signed in replayed:
            require(rpc(self.url, "eth_sendRawTransaction", [signed["raw"]]) == signed["hash"], "replayed hash differs")
            receipts.append(receipt(self.url, signed["hash"]))
        by_address = {}
        for creation, found in zip(record["creations"], receipts):
            require(found["status"] == "0x1" and found["contractAddress"].lower() == creation["address"].lower(),
                    f"{creation['name']} did not land at its production address")
            code = rpc(self.url, "eth_getCode", [creation["address"], "latest"])
            keccak = subprocess.run(["cast", "keccak", code], capture_output=True, text=True, check=True).stdout.strip()
            require(keccak == creation["runtime_keccak256"], f"{creation['name']} runtime differs from production")
            creation.update(block=int(found["blockNumber"], 16), block_hash=found["blockHash"])
            by_address[creation["address"].lower()] = creation
        for pool in record["pools"]:
            created = by_address[pool["pool"].lower()]
            pool["deployment_block"], pool["deployment_hash"] = created["block"], created["block_hash"]
        first = min(by_address.values(), key=lambda entry: entry["block"])
        record["anchor"] = {"block": first["block"], "hash": first["block_hash"]}
        record["local_rehearsal"] = {"chain": "anvil 127.0.0.1 chain-id 10143", "replayed_journal": str(journal),
                                     "asset_code": mocks, "labels": LABELS}
        path = work / "local-deployment.json"
        private(path, json.dumps(record, indent=2).encode())
        return path, {"replayed_transactions": [signed["hash"] for signed in replayed],
                      "runtime_hashes_match_production": True, "mock_assets": mocks}

    def token_balance(self, token: str, holder: str) -> int:
        data = "0x70a08231" + holder.lower().removeprefix("0x").rjust(64, "0")
        return int(str(rpc(self.url, "eth_call", [{"to": token, "data": data}, "latest"])), 16)

    def mint(self, token: str, holder: str, amount: int) -> str:
        """Fixture setup through Anvil directly (not an operator endpoint): mint mock ERC-20 to a buyer."""
        funder = rpc(self.url, "eth_accounts", [])[0]
        data = "0x40c10f19" + holder.lower().removeprefix("0x").rjust(64, "0") + hex(amount)[2:].rjust(64, "0")
        sent = rpc(self.url, "eth_sendTransaction", [{"from": funder, "to": token, "data": data}])
        require(receipt(self.url, str(sent))["status"] == "0x1", "mock mint failed")
        return str(sent)


# ---- installed commands ------------------------------------------------------------------------------------------

class Installed:
    def __init__(self, bins: Path, work: Path) -> None:
        self.bins, self.work = bins, work
        self.log = (work / "commands.log").open("a")

    def environment(self, extra: dict | None = None) -> dict:
        return {"PATH": f"{self.bins}:/usr/bin:/bin", "HOME": str(self.work / "home"), **(extra or {})}

    def init(self, *arguments: str, expect_ok: bool = True) -> dict:
        command = [str(self.bins / "erebus-init"), "--metropolis", *arguments]
        result = subprocess.run(command, capture_output=True, text=True, cwd=self.work, timeout=600,
                                env=self.environment())
        self.log.write(json.dumps({"command": command[1:], "exit": result.returncode}) + "\n")
        self.log.flush()
        try:
            reply = json.loads(result.stdout)
        except ValueError:
            reply = {"status": "unparsed", "stdout": result.stdout[-400:], "stderr": result.stderr[-400:]}
        if expect_ok:
            require(result.returncode == 0 and reply.get("status") != "error", f"erebus-init {arguments[0]}: {reply}")
        return reply

    def native(self, name: str, request: dict, cwd: Path) -> dict:
        result = subprocess.run([str(self.bins / name)], input=json.dumps(request), capture_output=True, text=True,
                                cwd=cwd, timeout=600, env=self.environment())
        self.log.write(json.dumps({"native": name, "method": request.get("method"), "exit": result.returncode}) + "\n")
        self.log.flush()
        try:
            return json.loads(result.stdout)
        except ValueError:
            return {"status": "unparsed", "stderr": result.stderr[-400:]}


class AccessService:
    def __init__(self, installed: Installed, config: Path, port: int) -> None:
        self.installed, self.config, self.port, self.process = installed, config, port, None

    def start(self) -> dict:
        self.process = subprocess.Popen([str(self.installed.bins / "erebus-access-service")],
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                        env=self.installed.environment({"EREBUS_ACCESS_CONFIG": str(self.config)}))
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/healthz", timeout=2) as response:
                    return json.loads(response.read())
            except OSError:
                require(self.process.poll() is None, "access service exited")
                time.sleep(0.2)
        raise Failure("access service did not become healthy")

    def stop(self) -> None:
        if self.process and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=15)


class Agent:
    """One installed MCP server, started from the operator's generated mcp.json; restart() is a new process.

    Each server's session lives in its own task (anyio scopes must be exited by the task that entered them)."""

    def __init__(self, directory: Path, overrides: dict | None = None) -> None:
        settings = json.loads((directory / "mcp.json").read_text())
        (self.name, entry), = settings["mcpServers"].items()
        self.command, self.env = entry["command"], dict(entry["env"])
        self.home = directory.parent.parent.parent / "home"
        self.errors = directory / "mcp-server.stderr.log"
        self.overrides = overrides or {}
        self.queue: asyncio.Queue | None = None
        self.task: asyncio.Task | None = None
        self.tools: set[str] = set()

    async def _serve(self, ready: asyncio.Future) -> None:
        from mcp import ClientSession
        from mcp.client.stdio import StdioServerParameters, stdio_client

        bins = str(Path(self.command).parent)
        params = StdioServerParameters(command=self.command, args=[], env={
            **self.env, **self.overrides, "PATH": f"{bins}:/usr/bin:/bin", "HOME": str(self.home)})
        try:
            with self.errors.open("a") as errors:
                async with stdio_client(params, errlog=errors) as (read, write):
                    async with ClientSession(read, write, read_timeout_seconds=900.0) as session:
                        await session.initialize()
                        ready.set_result({tool.name for tool in (await session.list_tools()).tools})
                        while (item := await self.queue.get()) is not None:
                            tool, arguments, answer = item
                            try:
                                answer.set_result(await session.call_tool(tool, arguments))
                            except Exception as error:  # delivered to the caller
                                answer.set_exception(error)
        except Exception as error:
            if not ready.done():
                ready.set_exception(error)

    async def start(self) -> set[str]:
        self.queue = asyncio.Queue()
        ready = asyncio.get_running_loop().create_future()
        self.task = asyncio.create_task(self._serve(ready))
        self.tools = await ready
        return self.tools

    async def stop(self) -> None:
        if self.task is not None:
            await self.queue.put(None)
            await self.task
            self.task = None

    async def restart(self, overrides: dict | None = None) -> set[str]:
        await self.stop()
        self.overrides = overrides or {}
        return await self.start()

    async def call(self, tool: str, **arguments) -> dict:
        require(self.task is not None and not self.task.done(), f"{self.name} server is not running")
        answer = asyncio.get_running_loop().create_future()
        await self.queue.put((tool, arguments, answer))
        result = await answer
        if result.structured_content is not None:
            return result.structured_content
        text = result.content[0].text if result.content else "{}"
        try:
            return json.loads(text)
        except ValueError:
            return {"ok": False, "error": {"raw": text[:300]}}


# ---- workflow ----------------------------------------------------------------------------------------------------

def reference() -> str:
    return secrets.token_hex(32)


def check_operator_mode(replies: dict, summaries: list[dict], prepublication: bool) -> None:
    for name in ("seller_init", "buyer_init", "auditor"):
        if name not in replies:
            continue
        if prepublication:
            require(not replies[name].get("rehearsal"), f"{name} unexpectedly used rehearsal pins")
        else:
            require(bool(replies[name].get("rehearsal")), f"{name} output is not marked as a rehearsal")
    for summary in summaries:
        if prepublication:
            require(summary.get("prepublication") is True and not summary.get("rehearsal")
                    and "rehearsal-pins" not in summary.get("labels", []),
                    "operator.json is not labelled as prepublication")
        else:
            require("rehearsal-pins" in summary.get("labels", [])
                    and (summary.get("rehearsal") or {}).get("publication_evidence") is False
                    and not summary.get("prepublication"), "operator.json is not labelled as a rehearsal")


class Pool:
    def __init__(self, name: str, installed: Installed, chain: Chain, deployment: Path, pins: Path, work: Path,
                 dev_keys: list[tuple[str, str]], prepublication: bool = False) -> None:
        self.name, self.installed, self.chain, self.deployment, self.pins = name, installed, chain, deployment, pins
        self.root = work / name
        self.root.mkdir(mode=0o700)
        self.record = next(p for p in json.loads(deployment.read_text())["pools"] if p["name"] == name)
        self.asset, self.contract = self.record["asset"].lower(), self.record["pool"].lower()
        self.amounts = AMOUNTS[name]
        self.dev_keys = dev_keys
        self.prepublication = prepublication
        self.report: dict = {"pool": name, "asset_code": "local MockWMON" if name == "wmon" else
                             "local MockERC20 at the testnet USDC address (not Circle USDC)"}

    def signer(self, label: str) -> tuple[str, Path]:
        address, key = self.dev_keys.pop()
        keys = self.root / "fixture-keys"
        keys.mkdir(mode=0o700, exist_ok=True)
        path = private(keys / f"{label}.key", bytes.fromhex(key.removeprefix("0x")))
        rpc(self.chain.url, "anvil_setBalance", [address, hex(100 * 10**18)])
        return address, path

    def operators(self, pair: str, delivery_seconds: int | None = None) -> dict:
        """Seller, buyer (and on the main pair an auditor), generated by installed erebus-init only."""
        primary, peer = self.chain.proxies["primary"].url, self.chain.proxies["peer"].url
        base = self.root / pair
        base.mkdir(mode=0o700)
        seller_address, seller_key = self.signer(f"{pair}-seller")
        buyer_address, buyer_key = self.signer(f"{pair}-buyer")
        seller_port, buyer_port, access_port = free_port(), free_port(), free_port()
        common = ["--pool", self.name, "--bin", str(self.installed.bins), "--pins", str(self.pins),
                  "--deployment", str(self.deployment), "--rpc-url", primary, "--peer-rpc-url", peer]
        seller = base / "seller"
        buyer = base / "buyer"
        replies = {"seller_init": self.installed.init("init", "--role", "seller", "--directory", str(seller), *common,
                                                      "--endpoint", f"127.0.0.1:{seller_port}",
                                                      "--signer-address", seller_address,
                                                      "--transaction-key-file", str(seller_key)),
                   "buyer_init": self.installed.init("init", "--role", "buyer", "--directory", str(buyer), *common,
                                                     "--endpoint", f"127.0.0.1:{buyer_port}",
                                                     "--signer-address", buyer_address,
                                                     "--transaction-key-file", str(buyer_key),
                                                     "--maximum-price", str(self.amounts["price"]))}
        payload = private(base / "payload.bin", PAYLOAD)
        delivery = [] if delivery_seconds is None else ["--delivery-seconds", str(delivery_seconds)]
        replies["seller_connect"] = self.installed.init(
            "connect", "--directory", str(seller), "--peer-share", str(buyer / "share"), "--resource", "rehearsal-dataset",
            "--amount", str(self.amounts["price"]), "--payload-file", str(payload), "--port", str(access_port), *delivery)
        replies["buyer_connect"] = self.installed.init(
            "connect", "--directory", str(buyer), "--peer-share", str(seller / "share"), "--resource", "rehearsal-dataset",
            "--amount", str(self.amounts["price"]), "--service-url", f"http://127.0.0.1:{access_port}/v1/access",
            "--allow-loopback-access-http", *delivery)
        funding = ["approve", "deposit", "status"] + (["wrap"] if self.name == "wmon" else [])
        allow = [argument for method in funding for argument in ("--allow", method)]
        replies["buyer_policy"] = self.installed.init("wallet-policy", "--directory", str(buyer),
                                                      "--maximum-amount", str(self.amounts["cap"]), *allow)
        operators = {"seller": seller, "buyer": buyer, "buyer_address": buyer_address, "access_port": access_port,
                     "access": AccessService(self.installed, seller / "access.json", access_port)}
        if pair == "main":
            auditor = base / "auditor"
            replies["auditor"] = self.installed.init("auditor", "--pool", self.name, "--directory", str(auditor),
                                                     "--bin", str(self.installed.bins), "--pins", str(self.pins),
                                                     "--deployment", str(self.deployment), "--rpc-url", primary,
                                                     "--peer-rpc-url", peer, "--history", "checkpoint")
            operators["auditor"] = auditor
            # WS5: this auditor is set up before any deal here, so it opts in to a paired finalized checkpoint.
            require(replies["auditor"].get("history") == "checkpoint"
                    and isinstance(replies["auditor"].get("checkpoint_block"), int),
                    f"auditor history checkpoint: {replies['auditor']}")
            operators["auditor_checkpoint_block"] = replies["auditor"]["checkpoint_block"]
        check_operator_mode(replies, [json.loads((operators[role] / "operator.json").read_text())
                                     for role in ("seller", "buyer")], self.prepublication)
        self.report.setdefault("generated", {})[pair] = {
            name: reply.get("status") for name, reply in replies.items()}
        return operators

    async def fund(self, buyer: Agent, address: str) -> dict:
        """Policy-bound wallet funding through the buyer's MCP tools; each action under a retained reference."""
        amounts, before = self.amounts, self.chain.sends()
        results = {}
        if self.name == "usdc":
            self.chain.mint(self.asset, address, amounts["deposit"])
        else:
            results["wrap"] = await self.wallet(buyer, "wrap", amount=str(amounts["deposit"]), operation_ref=reference())
        balance_before = self.chain.token_balance(self.asset, address)
        pool_before = self.chain.token_balance(self.asset, self.contract)
        results["approve"] = await self.wallet(buyer, "approve", amount=str(amounts["deposit"]), operation_ref=reference())
        deposit_ref = reference()
        results["deposit"] = await self.wallet(buyer, "deposit", amount=str(amounts["deposit"]), operation_ref=deposit_ref)
        # Retrying the same reference must not send again.
        sends = self.chain.sends()
        repeat = await buyer.call("deposit_wallet", amount=str(amounts["deposit"]), operation_ref=deposit_ref)
        require(self.chain.sends() == sends, "a repeated deposit reference sent another transaction")
        require(self.chain.token_balance(self.asset, self.contract) - pool_before == amounts["deposit"],
                "the pool did not receive exactly the deposit")
        require(balance_before - self.chain.token_balance(self.asset, address) == amounts["deposit"],
                "the buyer's token balance did not fall by exactly the deposit")
        spendable = await self.synced(buyer)
        return {"funding_broadcasts": self.chain.sends() - before, "repeat_deposit_reference_sent": False,
                "repeat_deposit_reply_ok": repeat.get("ok"), "pool_received": amounts["deposit"],
                "notes_after_sync": spendable}

    async def operations(self, agent: Agent) -> list[dict]:
        notes = await agent.call("wallet_notes")
        return ((notes.get("result") or {}).get("result") or {}).get("operations") or []

    async def wallet(self, agent: Agent, method: str, **arguments) -> dict:
        """One funding action through the MCP tool, the way the native wallet tests drive it: wait for a free signer
        lane, act once under a durable reference, then sync until that operation is completed. A `pending` reply is
        retried only with the same reference. On failure the same request is replayed to the installed wallet only to
        read its public error; with the same reference it cannot create another action."""
        known = {operation["identity"] for operation in await self.operations(agent)}
        for _ in range(120):
            reply = await agent.call(f"{method}_wallet", **arguments)
            if reply.get("ok") is True or (reply.get("result") or {}).get("status") != "pending":
                break
            await agent.call("sync_wallet")
            done = [operation for operation in await self.operations(agent)
                    if operation["identity"] not in known and operation["kind"] == method]
            if done:
                break
            await asyncio.sleep(1)
        for _ in range(120):
            await agent.call("sync_wallet")
            done = [operation for operation in await self.operations(agent)
                    if operation["identity"] not in known and operation["kind"] == method]
            if done and done[0]["state"] == "completed":
                return {"ok": True, "operation": done[0]["identity"], "state": "completed",
                        "attempts": len(done[0].get("attempts") or [])}
            if method == "deposit" and reply.get("ok") is True:
                return reply
            await asyncio.sleep(1)
        directory = Path(agent.env["EREBUS_WALLET_CONFIG"]).parent
        native = self.installed.native("erebus-wallet", {"method": method, "config_file": agent.env["EREBUS_WALLET_CONFIG"],
                                                         **arguments}, directory)
        raise Failure(f"{method}: {reply}; native: {json.dumps(native)[:600]}")

    async def synced(self, agent: Agent, attempts: int = 60) -> int:
        for _ in range(attempts):
            reply = await agent.call("sync_wallet")
            notes = await agent.call("wallet_notes")
            listed = ((notes.get("result") or {}).get("result") or {}).get("notes") or []
            spendable = [note for note in listed if note.get("included") is True and note.get("consumed") is False
                         and not note.get("reserved")]
            if reply.get("ok") and spendable:
                return len(spendable)
            await asyncio.sleep(1)
        raise Failure("the wallet did not sync a spendable note")

    async def note_states(self, agent: Agent) -> list[dict]:
        notes = await agent.call("wallet_notes")
        return [{"commitment": note.get("commitment"), "reserved": bool(note.get("reserved")),
                 "consumed": note.get("consumed"), "included": note.get("included")}
                for note in ((notes.get("result") or {}).get("result") or {}).get("notes") or []]

    async def denials(self, buyer: Agent) -> dict:
        before = self.chain.sends()
        over = await buyer.call("deposit_wallet", amount=str(self.amounts["cap"] + 1), operation_ref=reference())
        result = {"deposit_over_cap_refused": over.get("ok") is False,
                  "withdraw_tool_absent": "withdraw_wallet" not in buyer.tools,
                  "replace_tool_absent": "replace_wallet" not in buyer.tools,
                  "unwrap_tool_absent": "unwrap_wallet" not in buyer.tools}
        if self.name != "wmon":
            result["wrap_tool_absent"] = "wrap_wallet" not in buyer.tools
        result["broadcasts"] = self.chain.sends() - before
        require(all(value is True for key, value in result.items() if key != "broadcasts") and result["broadcasts"] == 0,
                f"policy denials: {result}")
        return result

    async def negotiate(self, buyer: Agent, seller: Agent, operation: str) -> tuple[dict, dict]:
        buyer_deal, seller_deal = await asyncio.gather(buyer.call("negotiate_deal", operation_ref=operation),
                                                       seller.call("negotiate_deal", operation_ref=operation))
        require(buyer_deal.get("ok") and seller_deal.get("ok"), f"negotiation: {buyer_deal} / {seller_deal}")
        require(buyer_deal["result"]["deal_commitment"] == seller_deal["result"]["deal_commitment"], "different deals")
        return buyer_deal["result"], seller_deal["result"]

    async def recover(self, buyer: Agent, operation: str, polls: int = 180) -> dict:
        for _ in range(polls):
            observed = await buyer.call("recover_deal", operation_ref=operation)
            outcome = observed.get("result") or {}
            if outcome.get("payment_verified"):
                return outcome
            require(outcome.get("status") != "closed_unpaid", f"deal closed unpaid: {outcome}")
            await asyncio.sleep(1)
        raise Failure("payment was not verified by observation")

    async def retrieve(self, buyer: Agent, evidence_file: str, polls: int = 60) -> dict:
        name = Path(evidence_file).name
        last = None
        for _ in range(polls):
            last = await buyer.call("retrieve_service_access", evidence_name=name)
            if last.get("ok"):
                return last["result"]["result"]
            if (last.get("result") or {}).get("retry_without_payment") is False:
                break
            await asyncio.sleep(1)
        return {"delivered": False, "last": last}

    def sender_of(self, transaction: str) -> str:
        return str((rpc(self.chain.url, "eth_getTransactionByHash", [transaction]) or {}).get("from", "")).lower()

    async def prior_history(self, blocks: int) -> dict:
        """WS1: pool history created before the fresh main pair joins (real proofs, separate keys)."""
        history = self.operators("history")
        agent = Agent(history["buyer"])
        try:
            await agent.start()
            funding = await self.fund(agent, history["buyer_address"])
        finally:
            await agent.stop()
        rpc(self.chain.url, "anvil_mine", [hex(blocks)])
        head = int(str(rpc(self.chain.url, "eth_blockNumber", [])), 16)
        deadline = time.monotonic() + 180
        while int(str(rpc(self.chain.url, "eth_getBlockByNumber", ["finalized", False])["number"]), 16) <= head:
            require(time.monotonic() < deadline, "finality did not pass the prior history")
            time.sleep(0.5)
        return {"funding_broadcasts": funding["funding_broadcasts"], "empty_blocks": blocks, "head_before_join": head}

    def checkpoint_participants(self, operators: dict) -> dict:
        """WS1: explicit fresh-participant checkpoints, then the record placed for the seller access state."""
        result = {}
        for role in ("buyer", "seller"):
            directory = operators[role]
            reply = self.installed.native("erebus-wallet", {"method": "checkpoint", "confirm_fresh_wallet": True,
                                                            "config_file": str(directory / "wallet.json")}, directory)
            require(reply.get("status") == "ok", f"{role} checkpoint: {reply}")
            result[role] = {key: reply["result"][key] for key in ("checkpoint", "checkpoint_block", "leaf_count",
                                                                 "history_from_block")}
        # Generated seller configs keep the access service in its own state root. Placing the same
        # deployment-bound record there is an operator step until packaging creates it (integration gate).
        access = json.loads((operators["seller"] / "access.json").read_text())
        wallet = json.loads((operators["seller"] / "wallet.json").read_text())
        target = Path(access["state_root"]) / "history-checkpoint.json"
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(Path(wallet["state_root"]) / "history-checkpoint.json", target)
        target.chmod(0o600)
        result["seller_access_record_placed_by_harness"] = True
        return result

    def checkpoint_coverage(self, operators: dict, blocks: dict) -> dict:
        """WS1: every cache the main pair wrote is checkpointed at its own participant's block."""
        caches = {}
        buyer_state = Path(json.loads((operators["buyer"] / "wallet.json").read_text())["state_root"])
        seller_state = Path(json.loads((operators["seller"] / "wallet.json").read_text())["state_root"])
        seller_access = Path(json.loads((operators["seller"] / "access.json").read_text())["state_root"])
        candidates = [*((path, blocks["buyer"]) for path in sorted(buyer_state.glob("wallet-index/*.json"))),
                      *((path, blocks["buyer"]) for path in sorted(buyer_state.glob("payment-observation/*.json"))),
                      *((path, blocks["seller"]) for path in sorted(seller_state.glob("wallet-index/*.json"))),
                      *((path, blocks["seller"]) for path in sorted(seller_access.glob("pool-*history.json")))]
        require(candidates, "no participant pool caches were written")
        for path, checkpoint_block in candidates:
            stored = json.loads(path.read_text())
            first = (stored.get("blocks") or [{}])[0].get("number")
            require(stored.get("version") == 3 and stored.get("checkpoint", {}).get("block_number") == checkpoint_block
                    and (first is None or first == checkpoint_block + 1), f"{path.name} is not checkpointed")
            caches[str(path.relative_to(path.parents[2]))] = {"version": 3, "checkpoint_block": checkpoint_block,
                                                              "first_scanned_block": first}
        return caches

    async def main_pair(self, extended: bool, checkpoint_history: int | None = None) -> dict:
        operators = self.operators("main")
        checkpoint = None
        if checkpoint_history is not None:
            checkpoint = {"prior_history": await self.prior_history(checkpoint_history),
                          "participants": self.checkpoint_participants(operators)}
        health = operators["access"].start()
        buyer, seller = Agent(operators["buyer"]), Agent(operators["seller"])
        try:
            report = {"access_health": health.get("status"), "buyer_tools": sorted(await buyer.start()),
                      "seller_tools": sorted(await seller.start())}
            require("retrieve_service_access" in buyer.tools and "settle_deal" in buyer.tools, f"buyer tools {buyer.tools}")
            require(seller.tools == {"negotiate_deal"}, f"seller tools {seller.tools}")
            report["funding"] = await self.fund(buyer, operators["buyer_address"])
            report["denials"] = await self.denials(buyer)
            report["deal"] = await self.deal(buyer, seller, operators)
            if checkpoint is not None:
                blocks = {role: checkpoint["participants"][role]["checkpoint_block"] for role in ("buyer", "seller")}
                checkpoint["coverage"] = self.checkpoint_coverage(operators, blocks)
                report["checkpoint"] = checkpoint
            if extended:
                report["timeout_and_reservation"] = await self.timeout_deal(buyer, seller, operators)
                report["backup_restore"] = await self.backup_restore(buyer, operators)
                report["endpoint_change"] = await self.endpoint_change(buyer, operators)
            return report
        finally:
            await buyer.stop()
            await seller.stop()
            operators["access"].stop()

    async def deal(self, buyer: Agent, seller: Agent, operators: dict) -> dict:
        operation = reference()
        buyer_deal, seller_deal = await self.negotiate(buyer, seller, operation)
        funding = await buyer.call("check_settlement_funding", operation_ref=operation)
        require(funding.get("ok") is True, f"funding: {funding}")
        pool_before = self.chain.token_balance(self.asset, self.contract)
        before = self.chain.sends()
        self.chain.proxies["primary"].drop_next_send_response = True
        self.chain.proxies["peer"].drop_next_send_response = True
        settled = await buyer.call("settle_deal", operation_ref=operation)
        require(self.chain.sends() - before == 1, f"settle_deal broadcast {self.chain.sends() - before} times: {settled}")
        # The buyer process is restarted before recovery: a new server, the same files.
        await buyer.restart()
        outcome = await self.recover(buyer, operation)
        sends = self.chain.proxies["primary"].sends + self.chain.proxies["peer"].sends
        require(self.chain.sends() - before == 1, f"settlement broadcasts: {self.chain.sends() - before}")
        transaction = sends[-1]
        mined = receipt(self.chain.url, transaction)
        require(mined["status"] == "0x1" and mined["to"].lower() == self.contract, "settlement receipt")
        require(self.sender_of(transaction) == operators["buyer_address"].lower(), "settlement sender is not the buyer signer")
        require(self.chain.token_balance(self.asset, self.contract) == pool_before, "a private payment moved pool tokens")
        require(outcome.get("winning_commitment") == buyer_deal["deal_commitment"], "finalized a different deal")
        # Seller access restart, then retrieval; delivery is asserted separately from payment.
        operators["access"].stop()
        restarted = operators["access"].start()
        delivery = await self.retrieve(buyer, buyer_deal["evidence_file"])
        require(delivery.get("resource_verified") is True and delivery.get("resource_sha256") == sha256(PAYLOAD),
                f"delivery: {delivery}")
        audit = self.audit(operators, seller_deal)
        require(self.chain.sends() - before == 1, "recovery, retrieval or disclosure sent a transaction")
        return {"operation_ref": operation, "deal_commitment": buyer_deal["deal_commitment"],
                "settle_reply": {key: (settled.get("result") or {}).get(key) for key in ("status", "stage", "payment_verified")},
                "settle_ok": settled.get("ok"), "dropped_broadcast_response": True,
                "buyer_restarted_before_recovery": True, "payment_verified_by_recovery": True,
                "settlement_broadcasts": 1, "transaction": transaction, "sender": "buyer signer (direct mode)",
                "pool_token_balance_unchanged": True, "seller_access_restarted": restarted.get("status"),
                "delivery": {"resource_verified": True, "resource_sha256": delivery["resource_sha256"],
                             "payment_verified_claimed_by_access": delivery.get("payment_verified")},
                "auditor": audit}

    def audit(self, operators: dict, seller_deal: dict) -> dict:
        auditor = operators["auditor"]
        info = self.installed.native("erebus-shielded-disclosure", {"method": "key_info", "key_file": str(auditor / "auditor.key")}, auditor)
        require(info.get("status") == "ok", f"auditor key: {info}")
        seller_summary = json.loads((operators["seller"] / "operator.json").read_text())
        agreement_key = Path(seller_summary["operator"]["agreement_key_file"])
        grant = auditor / "deal.grant"
        exported = self.installed.native("erebus-shielded-disclosure", {
            "method": "export", "evidence_file": seller_deal["evidence_file"], "issuer_key_file": str(agreement_key),
            "recipient_public_key": info["recipient_public_key"], "grant_file": str(grant),
            "expires_at": int(time.time()) + 600}, operators["seller"])
        require(exported.get("status") == "ok", f"export: {exported}")
        issuer = "0x" + agreement_key.with_name("agreement.public").read_bytes().hex()
        deployment = json.loads((auditor / "disclosure.json").read_text())
        # A full-history auditor cache advances in bounded scans; "pending" is progress, never a verdict.
        for _ in range(60):
            verified = self.installed.native("erebus-shielded-disclosure", {
                "method": "verify_payment", "grant_file": str(grant), "key_file": str(auditor / "auditor.key"),
                "expected_issuer": issuer, "deployment": deployment}, auditor)
            if verified.get("status") != "pending" or verified.get("payment_verified") is True:
                break
        require(verified.get("agreement_verified") is True and verified.get("payment_verified") is True
                and verified.get("delivery_verified") is False, f"auditor: {verified}")
        wrong = self.installed.native("erebus-shielded-disclosure", {
            "method": "verify_payment", "grant_file": str(grant), "key_file": str(auditor / "auditor.key"),
            "expected_issuer": "0x" + "11" * 32, "deployment": deployment}, auditor)
        require(wrong.get("payment_verified") is not True, "a wrong expected issuer verified")
        # The verified payment came from caches that start after the auditor's own checkpoint.
        checkpoint_block = operators["auditor_checkpoint_block"]
        cache = Path(deployment["cache_root"])
        for name in ("primary", "peer"):
            stored = json.loads((cache / name / "index.json").read_text())
            first = (stored.get("blocks") or [{}])[0].get("number")
            require(stored.get("version") == 3 and stored.get("checkpoint", {}).get("block_number") == checkpoint_block
                    and (first is None or first == checkpoint_block + 1), f"auditor {name} cache is not checkpointed")
        return {"agreement_verified": True, "payment_verified": True, "delivery_verified": False,
                "wrong_issuer_refused": True, "scope": "one seller-exported grant for one deal",
                "history_from_block": checkpoint_block + 1}

    async def timeout_deal(self, buyer: Agent, seller: Agent, operators: dict) -> dict:
        """A settlement whose subprocess times out while its broadcast is held; the reservation must survive."""
        operation = reference()
        buyer_deal, _ = await self.negotiate(buyer, seller, operation)
        before = self.chain.sends()
        await buyer.restart({"EREBUS_NATIVE_TIMEOUT_SECONDS": "20"})
        for proxy in ("primary", "peer"):
            self.chain.proxies[proxy].stall_next_send_seconds = 35.0
        timed_out = await buyer.call("settle_deal", operation_ref=operation)
        held = await self.note_states(buyer)
        await asyncio.sleep(20)
        await buyer.restart()
        outcome = await self.recover(buyer, operation)
        sends = self.chain.sends() - before
        await self.synced(buyer)
        after = await self.note_states(buyer)
        # A consumed note keeps its reservation marker; the held one is reserved and not yet consumed.
        reserved = [note for note in held if note["reserved"] and note["consumed"] is False]
        require(len(reserved) == 1, f"expected one reserved unspent note after the timeout: {held}")
        spent = next((note for note in after if note["commitment"] == reserved[0]["commitment"]), None)
        require(spent is not None and spent["consumed"] is True, f"the reserved note was not consumed by the recovered payment: {after}")
        require(timed_out.get("ok") is False and (timed_out.get("error") or {}).get("retry_without_new_payment") is True,
                f"timeout reply: {timed_out}")
        require(sends == 1, f"timeout path broadcasts: {sends}")
        require(outcome.get("winning_commitment") == buyer_deal["deal_commitment"], "timeout path finalized another deal")
        return {"settle_timed_out": True, "retry_without_new_payment": True,
                "reserved_notes_after_timeout": len(reserved), "reserved_note_consumed_after_recovery": True,
                "recovered_by_observation": True, "broadcasts": sends}

    async def backup_restore(self, buyer: Agent, operators: dict) -> dict:
        directory = operators["buyer"]
        await buyer.stop()
        archive = directory.parent / "buyer-backup.tar"
        with tarfile.open(archive, "w") as bundle:
            bundle.add(directory, arcname="buyer")
        archive.chmod(0o600)
        damaged = directory.parent / "buyer-damaged"
        directory.rename(damaged)
        with tarfile.open(archive) as bundle:
            bundle.extractall(directory.parent, filter="tar")
        shutil.rmtree(damaged)
        doctor = self.installed.init("doctor", "--directory", str(directory), "--offline", expect_ok=False)
        sends = self.chain.sends()
        await buyer.start()
        notes = await buyer.call("wallet_notes")
        listed = ((notes.get("result") or {}).get("result") or {})
        synced = await buyer.call("sync_wallet")
        require(doctor.get("status") == "healthy", f"doctor after restore: {doctor}")
        require(self.chain.sends() == sends, "restoration sent a transaction")
        return {"restored_from": "tar archive with permissions", "doctor": doctor.get("status"),
                "notes": len(listed.get("notes") or []), "operations": len(listed.get("operations") or []),
                "sync_ok": synced.get("ok"), "broadcasts": 0}

    async def endpoint_change(self, buyer: Agent, operators: dict) -> dict:
        directory = operators["buyer"]
        await buyer.stop()
        replacement = self.chain.proxies["replacement"]
        interrupt = (
            "import os, sys\nfrom pathlib import Path\nfrom erebus_mcp.metropolis_setup import operator_init as init\n"
            "real = os.replace\n"
            "def failing(source, target):\n"
            "    if Path(target).name == 'payment.json':\n        raise OSError('injected interruption')\n"
            "    return real(source, target)\n"
            "init.os.replace = failing\nsys.exit(init.main(sys.argv[1:]))\n")
        result = subprocess.run([str(self.installed.bins / "python"), "-I", "-c", interrupt, "endpoints", "--directory",
                                 str(directory), "--rpc-url", replacement.url, "--peer-rpc-url",
                                 self.chain.proxies["peer"].url], capture_output=True, text=True,
                                timeout=300, env=self.installed.environment())
        interrupted = result.returncode != 0 and (directory / ".endpoint-change" / "journal.json").is_file()
        require(interrupted, f"the injected interruption did not leave a committed journal: {result.stdout[-300:]}")
        doctor = self.installed.init("doctor", "--directory", str(directory), "--offline", expect_ok=False)
        resumed = self.installed.init("endpoints", "--directory", str(directory), "--resume")
        healthy = self.installed.init("doctor", "--directory", str(directory), expect_ok=False)
        requests = replacement.requests
        await buyer.start()
        synced = await buyer.call("sync_wallet")
        require(interrupted and doctor.get("status") == "attention", "the interrupted endpoint change was not reported")
        require(healthy.get("status") == "healthy", f"doctor after resume: {healthy}")
        require(synced.get("ok") is True and replacement.requests > requests, "the wallet did not use the new endpoint")
        return {"interrupted": True, "doctor_while_incomplete": doctor.get("status"), "resumed": resumed.get("status"),
                "doctor_after_resume_online": healthy.get("status"), "wallet_used_new_endpoint": True}

    async def deadline_pair(self) -> dict:
        """Short delivery window: a deal paid in time, retrieval only after the deadline, and an expired offer."""
        operators = self.operators("deadline", delivery_seconds=90)
        operators["access"].start()
        buyer, seller = Agent(operators["buyer"]), Agent(operators["seller"])
        try:
            await buyer.start()
            await seller.start()
            await self.fund(buyer, operators["buyer_address"])
            operation = reference()
            buyer_deal, _ = await self.negotiate(buyer, seller, operation)
            before = self.chain.sends()
            await buyer.call("settle_deal", operation_ref=operation)
            await self.recover(buyer, operation)
            paid = self.chain.sends() - before
            offer = json.loads((operators["seller"] / "share/offer.json").read_text())
            await asyncio.sleep(max(0, offer["delivery_deadline"] - time.time() + 5))
            late = await self.retrieve(buyer, buyer_deal["evidence_file"], polls=5)
            expired = await buyer.call("negotiate_deal", operation_ref=reference())
            report = {"payment_verified": True, "settlement_broadcasts": paid,
                      "late_retrieval_delivered": late.get("resource_verified") is True,
                      "late_retrieval_state": late if late.get("resource_verified") is not True else "delivered",
                      "expired_offer_negotiation_ok": expired.get("ok"),
                      "broadcasts_after_deadline": self.chain.sends() - before - paid}
            require(paid == 1 and report["broadcasts_after_deadline"] == 0, f"deadline pair: {report}")
            require(expired.get("ok") is False, "an expired offer negotiated")
            return report
        finally:
            await buyer.stop()
            await seller.stop()
            operators["access"].stop()


async def workflow(arguments: argparse.Namespace) -> dict:
    import erebus_mcp

    work = arguments.work
    bins = Path(sys.executable).parent
    require(not Path(erebus_mcp.__file__).resolve().is_relative_to(CHECKOUT), "erebus_mcp imported from the checkout")
    installed = Installed(bins, work)
    release_path = arguments.registry / "release.json"
    release_sha = sha256(release_path.read_bytes())
    release = json.loads(release_path.read_text())
    for name, entry in release["binaries"].items():
        require(sha256((bins / name).read_bytes()) == entry["sha256"], f"installed {name} differs from the candidate")
    chain = Chain(work)
    prepublication = arguments.prepublication_authorization is not None
    native = (os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
              and release.get("dirty_source") is False)
    report: dict = {"labels": LABELS if not prepublication else LABELS[:3] + ["prepublication-pins", "local-anvil"],
                    "pins_mode": "prepublication" if prepublication else "rehearsal",
                    **({} if prepublication else {"rehearsal": True, "publication_evidence": False}),
                    "release_json_sha256": release_sha, "version": release["version"],
                    "source_commit": release.get("source_commit"),
                    "candidate_source_tree_sha256": (release.get("candidate") or {}).get("source_tree_sha256"),
                    "installed_binaries_sha256": {name: entry["sha256"] for name, entry in release["binaries"].items()},
                    "claims": {"native_platform_qualification": native, "live_payment": False,
                               "note": "loopback Anvil chain 10143 with mock assets; not live Monad evidence"},
                    "source_imports": False, "public_rpc_used": False}
    if arguments.demo_main_only:
        report.update(demo_only=True, gate_complete=False, publication_evidence=False,
                      coverage_gaps=["timeout_and_reservation", "backup_restore", "endpoint_change", "deadline"])
        report["claims"]["native_platform_qualification"] = False
    try:
        deployment, report["chain"] = chain.deploy(work, arguments.deployment_journal)
        pins = work / "pins"
        if prepublication:
            reply = installed.init("pins", "--directory", str(pins), "--prepublication-release", str(release_path),
                                   "--prepublication-release-sha256", release_sha, "--authorization",
                                   str(arguments.prepublication_authorization), "--authorized-pins", str(arguments.authorized_pins))
            require(reply.get("prepublication") is True, f"pins: {reply}")
        else:
            reply = installed.init("pins", "--directory", str(pins), "--rehearsal-release", str(release_path),
                                   "--rehearsal-release-sha256", release_sha, "--authorized-pins", str(arguments.authorized_pins))
            require(reply.get("rehearsal") is True and reply.get("publication_evidence") is False, f"pins: {reply}")
        refused = installed.init("pins", "--directory", str(work / "pins-wrong"), "--rehearsal-release", str(release_path),
                                 "--rehearsal-release-sha256", "0" * 64, "--authorized-pins",
                                 str(arguments.authorized_pins), expect_ok=False)
        require(refused.get("status") == "error" and not (work / "pins-wrong").exists(), "a wrong candidate digest was pinned")
        report["pins"] = {"files": reply["files"], "wrong_candidate_digest_refused": True}
        dev_keys = chain.fixture_keys()
        report["pools"] = {}
        for index, name in enumerate(arguments.pools):
            pool = Pool(name, installed, chain, deployment, pins, work, dev_keys, prepublication=prepublication)
            extended = index == 0 and not arguments.demo_main_only
            # WS1 rehearsal only when requested; the default call is unchanged.
            checkpoint_history = getattr(arguments, "checkpoint_fresh_participants", None)
            pool.report["main"] = await (pool.main_pair(extended=extended) if checkpoint_history is None
                                         else pool.main_pair(extended=extended, checkpoint_history=checkpoint_history))
            if index == 0 and not arguments.demo_main_only:
                pool.report["deadline"] = await pool.deadline_pair()
            report["pools"][name] = pool.report
        report["total_operator_broadcasts"] = chain.sends()
        report["status"] = "verified"
        return report
    except BaseException as error:
        if arguments.hold_on_failure:
            (work / "failure.txt").write_text(f"{type(error).__name__}: {error}")
            # Diagnostics only: keep the loopback chain up until <work>/release exists.
            (work / "chain.json").write_text(json.dumps({"anvil": chain.url, **{k: v.url for k, v in chain.proxies.items()}}))
            while not (work / "release").exists():
                await asyncio.sleep(1)
        raise
    finally:
        chain.close()


def bootstrap(arguments: argparse.Namespace) -> int:
    """Install the candidate outside the checkout, then re-run this script under the installed Python."""
    work = arguments.work.resolve()
    work.mkdir(mode=0o700)
    spec = importlib.util.spec_from_file_location("check_metropolis_init", CHECKOUT / "scripts/check-metropolis-init.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    bins = module.install(work, arguments.version, arguments.registry.resolve(), None)
    command = [str(bins / "python"), "-I", str(Path(__file__).resolve()), "--phase", "run", "--registry",
               str(arguments.registry.resolve()), "--version", arguments.version, "--authorized-pins",
               str(arguments.authorized_pins.resolve()), "--work", str(work), "--pools", ",".join(arguments.pools),
               "--deployment-journal", str(arguments.deployment_journal.resolve())]
    if arguments.prepublication_authorization is not None:
        command += ["--prepublication-authorization", str(arguments.prepublication_authorization.resolve())]
    if arguments.demo_main_only:
        command += ["--demo-main-only"]
    if getattr(arguments, "checkpoint_fresh_participants", None) is not None:
        command += ["--checkpoint-fresh-participants", str(arguments.checkpoint_fresh_participants)]
    environment = {key: value for key, value in os.environ.items() if not key.startswith(("EREBUS_", "VIRTUAL_ENV", "PYTHON"))}
    environment["PATH"] = f"{bins}:{os.environ.get('PATH', '/usr/bin:/bin')}"
    return subprocess.run(command, env=environment, check=False).returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--authorized-pins", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--pools", default="wmon,usdc", type=lambda value: value.split(","))
    parser.add_argument("--phase", choices=("bootstrap", "run"), default="bootstrap")
    parser.add_argument("--hold-on-failure", action="store_true", help="keep the loopback chain up for diagnosis")
    parser.add_argument("--demo-main-only", action="store_true",
                        help="record the core MCP workflow only; explicitly incomplete and never publication evidence")
    parser.add_argument("--checkpoint-fresh-participants", type=int, metavar="EMPTY_BLOCKS",
                        help="WS1 rehearsal: create prior history (one funded pair plus EMPTY_BLOCKS blocks), then "
                             "checkpoint the fresh main buyer and seller; requires --demo-main-only")
    parser.add_argument("--deployment-journal", type=Path, default=JOURNAL,
                        help="directory with the six signed production deployment transactions (<n>-signed.json)")
    parser.add_argument("--prepublication-authorization", type=Path,
                        help="native qualification: the committed owner authorization naming this version (else rehearsal pins)")
    arguments = parser.parse_args()
    if arguments.phase == "bootstrap":
        return bootstrap(arguments)
    try:
        report = asyncio.run(workflow(arguments))
    except BaseException as error:  # report the failure, then fail
        import traceback
        traceback.print_exc()
        report = {"status": "failed", "error": f"{type(error).__name__}: {error}", "labels": LABELS,
                  "rehearsal": True, "publication_evidence": False}
        print(json.dumps(report, indent=2))
        return 1
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
