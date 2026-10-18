"""Quickstart: one directory holds a connected buyer, seller and auditor, and nothing is ever sent."""

import importlib.util
import json
import socket
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("quickstart_operator_tests", HERE / "test_metropolis_operator_init.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
init = base.init
quick = init.load("quickstart")
world = base.world

FAKE_SERVICE = r'''#!PYTHON
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
port = json.load(open(os.environ["EREBUS_ACCESS_CONFIG"]))["port"]
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b'{"status":"ok"}')
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", port), H).serve_forever()
'''
FAKE_ARTIFACTS = r'''#!PYTHON
import json, os, sys
from pathlib import Path
request = json.loads(sys.stdin.read())
if (Path(request["cache_root"]).parent / f"fail-{request['circuit']}").exists():
    print(json.dumps({"status": "error", "error": "artifact installation failed"})); sys.exit(1)
circuits = json.loads(Path(request["manifest_file"]).read_text())["circuits"]
cache = Path(request["cache_root"]); cache.mkdir(parents=True, exist_ok=True)
circuit = next(c for c in circuits if c["kind"] == request["circuit"])
fresh = 0
for field in ("wasm", "r1cs", "zkey"):
    path = cache / (circuit[field]["sha256"] + ".artifact")
    fresh += not path.exists(); path.write_bytes(b"x")
print(json.dumps({"status": "ok", "downloaded_bytes": fresh, "cache_hits": 3 - fresh}))
'''


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@pytest.fixture
def natives(world, monkeypatch):
    calls = []
    real = init.command

    def fake(bins, name, request, cwd):
        calls.append((name, request["method"]))
        method = request["method"]
        if name == "erebus-settle" and method == "generate_testnet_key":
            path = Path(request["key_file"])
            assert path.parent.stat().st_mode & 0o077 == 0
            path.write_bytes(b"\x01" * 32)
            path.chmod(0o600)
            return {"status": "ok", "address": "0x" + ("aa" if "buyer" in path.name else "bb") * 20}
        if name == "erebus-settle" and method == "address":
            return {"status": "ok", "address": "0x" + "cc" * 20}
        if name == "erebus-wallet" and method == "version":
            return {"status": "ok", "result": {"protocol": 1, "methods": ["notes", "sync", "status", *init.WALLET_WRITES],
                    "deposit_operation_references": True, "funding_operation_references": True,
                    "paired_account_diagnostics": True}}
        if name == "erebus-wallet" and method == "checkpoint":
            root = Path(json.loads(Path(request["config_file"]).read_text())["state_root"])
            (root / "history-checkpoint.json").write_text(json.dumps({"checkpoint": {"block_number": 42}}))
            return {"status": "ok", "result": {"checkpoint": "created", "checkpoint_block": 42}}
        if name == "erebus-negotiate" and method == "renew_descriptor":
            path = Path(request["descriptor_file"])
            data = json.loads(path.read_text())
            data["expires"] = 4_000_000_000
            path.write_text(json.dumps(data))
            return {"status": "ok", "descriptor_expires_at": 4_000_000_000}
        return real(bins, name, request, cwd)
    monkeypatch.setattr(init, "command", fake)
    monkeypatch.setattr(init, "doctor", lambda args: {"status": "ok"})
    monkeypatch.setattr(init, "rpc_balance", lambda url, address, opener=None: 3 * 10**18)
    release = json.loads((world.pins / "release-manifest.json").read_text())
    platform, = release["platforms"].values()
    for name, body in (("erebus-access-service", FAKE_SERVICE), ("erebus-artifacts", FAKE_ARTIFACTS)):
        path = world.bins / name
        path.write_text(body.replace("#!PYTHON", f"#!{sys.executable}"))
        path.chmod(0o755)
        platform["binaries"][name]["sha256"] = init.sha256(path)
    (world.pins / "release-manifest.json").write_text(json.dumps(release))
    (world.bins / "erebus-settle").write_text("#!/bin/sh\n")
    (world.bins / "erebus-settle").chmod(0o755)
    return calls


def quick_args(world, action, **changes):
    values = dict(action=action, directory=world.root / "qs", pins=world.pins, version=None, bin=world.bins,
                  deployment=world.deployment,
                  pool="wmon", rpc_url=None, peer_rpc_url=None, rpc_url_file=None, peer_rpc_url_file=None,
                  buyer_key_file=None, seller_key_file=None, price="1000", fund_amount="3000", resource="sample",
                  payload_file=None, port=free_port(), allow_agent_funding=False, no_wait=True, funding_timeout=0,
                  offline=True)
    values.update(changes)
    return SimpleNamespace(**values)


def ops():
    return SimpleNamespace(**vars(init))


def test_up_connects_both_roles_writes_agent_files_and_never_sends(world, natives):
    result = quick.run(quick_args(world, "up"), ops())
    directory = world.root / "qs"
    assert result["status"] == "created" and result["agent_funding"] is False
    assert result["rpc_hosts"] == ["testnet-rpc.monad.xyz", "rpc.ankr.com"]
    funding = result["funding"]
    assert funding["buyer"]["address"] == "0x" + "aa" * 20 and funding["buyer"]["send_at_least_mon"] == quick.mon(4 * quick.MEASURED_COST_PER_TRANSACTION + quick.SEND_HEADROOM + 3000)
    assert funding["seller"]["send_at_least_mon"] == "0" and funding["faucet"].startswith("https://")
    # The roles are wired to each other's share directories; nothing was copied by hand.
    buyer = json.loads((directory / "buyer/negotiate.json").read_text())
    assert buyer["peer_descriptor_file"] == str(directory / "seller/share/seller.descriptor.json")
    for role in ("buyer", "seller"):
        files = directory / "agents" / role
        claude = json.loads((files / "claude-code.mcp.json").read_text())
        assert list(claude["mcpServers"]) == [f"erebus-{role}"]
        assert f"[mcp_servers.erebus-{role}]" in (files / "codex.config.toml").read_text()
        assert base.owner_only(files / "prompt.md") and base.owner_only(directory / "keys" / f"{role}-gas.key")
    assert "fund_wallet" in (directory / "agents/buyer/prompt.md").read_text()
    assert "retrieve_service_access(operation_ref" in (directory / "agents/buyer/prompt.md").read_text()
    # Without --allow-agent-funding the buyer agent cannot spend: reads only.
    policy = json.loads((directory / "buyer/wallet-policy.json").read_text())
    assert set(policy["allowed_methods"]) == {"notes", "sync", "status"}
    for relative in ("buyer/wallet.json", "seller/wallet.json", "buyer/payment.json"):
        config = json.loads((directory / relative).read_text())
        assert config["rpc_requests_per_second"] == config["peer_rpc_requests_per_second"] == quick.REQUESTS_PER_SECOND
    backend = json.loads((directory / "seller/access.json").read_text())["backend"]
    assert backend["requests_per_second"] == quick.REQUESTS_PER_SECOND
    assert backend["max_concurrent_queries"] == quick.ACCESS_CONCURRENT_READS
    assert (directory / "auditor/disclosure.json").is_file()
    assert not any(method in {"checkpoint", "wrap", "deposit", "approve"} for _, method in natives)


def test_agent_funding_is_explicit_and_policy_capped(world, natives):
    quick.run(quick_args(world, "up", allow_agent_funding=True), ops())
    policy = json.loads((world.root / "qs/buyer/wallet-policy.json").read_text())
    assert set(policy["allowed_methods"]) == {"notes", "sync", "status", "wrap", "approve", "deposit"}
    assert policy["maximum_amount"] == "3000"


def test_keyed_endpoints_come_from_files_and_are_never_reported(world, natives, tmp_path):
    primary, peer = tmp_path / "primary.url", tmp_path / "peer.url"
    primary.write_text("https://a.example.net/v2/SECRETKEY\n")
    peer.write_text("https://b.example.net/?apikey=OTHERSECRET\n")
    result = quick.run(quick_args(world, "up", rpc_url_file=primary, peer_rpc_url_file=peer), ops())
    assert "SECRET" not in json.dumps(result) and result["rpc_hosts"] == ["a.example.net", "b.example.net"]
    state = (world.root / "qs" / quick.STATE).read_text()
    assert "SECRET" not in state


def test_start_checkpoints_then_status_and_down_manage_the_service(world, natives):
    quick.run(quick_args(world, "up"), ops())
    started = quick.run(quick_args(world, "start"), ops())
    try:
        assert started["checkpoints"] == {"buyer": 42, "seller": 42} and started["access_service"] == "started"
        access = json.loads((world.root / "qs/seller/access.json").read_text())
        assert (Path(access["state_root"]) / "history-checkpoint.json").is_file()
        again = quick.run(quick_args(world, "start"), ops())
        assert again["checkpoints"] == {"buyer": "existing", "seller": "existing"} and again["access_service"] == "running"
        status = quick.run(quick_args(world, "status"), ops())
        assert status["access_service"] == "running" and status["roles"]["buyer"]["checkpoint_block"] == 42
    finally:
        stopped = quick.run(quick_args(world, "down"), ops())
    assert stopped["access_service_was_running"] is True
    assert quick.run(quick_args(world, "status"), ops())["access_service"] == "stopped"
    assert (world.root / "qs/keys/buyer-gas.key").is_file()


def test_renew_reissues_descriptors_bindings_and_terms_in_place(world, natives):
    quick.run(quick_args(world, "up"), ops())
    directory = world.root / "qs"
    before = (directory / "buyer/keys/service.terms").read_bytes()
    result = quick.run(quick_args(world, "renew"), ops())
    assert result["status"] == "renewed" and result["descriptor_expires_at"] == 4_000_000_000
    assert [method for name, method in natives if name == "erebus-negotiate"][-6:] == [
        "renew_descriptor", "renew_descriptor", "prepare_binding", "prepare_binding", "prepare_terms", "prepare_terms"]
    shared = json.loads((directory / "seller/share/seller.descriptor.json").read_text())
    assert shared["expires"] == 4_000_000_000
    assert (directory / "buyer/keys/service.terms").read_bytes() != before
    assert not list(directory.rglob("*.renew"))
    status = quick.run(quick_args(world, "status"), ops())
    assert status["roles"]["seller"]["descriptor_expires_in_hours"] > 24


def test_up_refuses_reuse_and_unsafe_amounts(world, natives):
    quick.run(quick_args(world, "up"), ops())
    with pytest.raises(ValueError, match="new quickstart directory"):
        quick.run(quick_args(world, "up"), ops())
    with pytest.raises(ValueError, match="cover"):
        quick.run(quick_args(world, "up", directory=world.root / "qs2", fund_amount="10"), ops())
    with pytest.raises(ValueError, match="differ"):
        quick.run(quick_args(world, "up", directory=world.root / "qs3", rpc_url="https://x.example", peer_rpc_url="https://x.example"), ops())


def test_up_without_pins_pins_the_installed_release_itself(world, natives, monkeypatch):
    pinned = []

    def pins(args, opener=None):
        pinned.append(args.version)
        import shutil
        shutil.copytree(world.pins, args.directory)
        (args.directory / "monad-testnet-shielded-v2.json").write_text(world.deployment.read_text())
        return {"status": "pinned"}
    monkeypatch.setattr(init, "pins", pins)
    monkeypatch.setattr(quick, "installed_version", lambda: "0.4.0.dev2026101801")
    result = quick.run(quick_args(world, "up", pins=None, deployment=None), ops())
    assert result["status"] == "created" and pinned == ["0.4.0.dev2026101801"]
    assert (world.root / "qs/pins/release-manifest.json").is_file()
    quick.run(quick_args(world, "up", directory=world.root / "qs2", pins=None, deployment=None,
                         version="0.4.0.dev2026101899"), ops())
    assert pinned[-1] == "0.4.0.dev2026101899"


def test_prefetch_installs_each_circuit_and_resumes_after_a_failure(world, natives, monkeypatch):
    quick.run(quick_args(world, "up"), ops())
    cache = Path(json.loads((world.root / "qs/buyer/wallet.json").read_text())["artifact_cache_root"])
    marker = cache.parent / "fail-withdraw"
    marker.write_text("")
    with pytest.raises(ValueError, match="6 of 9 files cached.*again to continue"):
        quick.run(quick_args(world, "prefetch"), ops())
    marker.unlink()
    result = quick.run(quick_args(world, "prefetch"), ops())
    buyer = result["artifacts"]["buyer"]
    assert (buyer["cached"], buyer["of"]) == (9, 9)
    # Resumed: the first two circuits were cache hits, only the last downloaded.
    assert [entry["downloaded_bytes"] for entry in buyer["circuits"]] == [0, 0, 3]
    assert quick.run(quick_args(world, "status"), ops())["roles"]["buyer"]["artifacts_cached"] == "9/9"


def test_default_bin_prefers_the_tool_environment(tmp_path, monkeypatch):
    tool = tmp_path / "tool" / "bin"
    tool.mkdir(parents=True)
    (tool / "erebus-negotiate").write_text("")
    monkeypatch.setattr(quick.sys, "executable", str(tool / "python"))
    assert quick.default_bin() == tool
    monkeypatch.setattr(quick.sys, "executable", str(tmp_path / "elsewhere" / "python"))
    monkeypatch.setattr(quick.shutil, "which", lambda name: None)
    with pytest.raises(ValueError, match="pass --bin"):
        quick.default_bin()
