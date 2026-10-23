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
                  offline=True, name=None)
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
    assert funding["buyer"]["address"] == "0x" + "aa" * 20 and funding["buyer"]["send_at_least_mon"] == quick.mon(quick.buyer_required_wei("wmon", 3000))
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
    assert quick.run(quick_args(world, "status"), ops())["auditor"] == {
        "history": "not started", "checkpoint_block": None, "history_from_block": None}
    started = quick.run(quick_args(world, "start"), ops())
    try:
        assert started["checkpoints"] == {"auditor": 41, "buyer": 42, "seller": 42}
        assert started["access_service"] == "started"
        # The auditor's checkpoint is taken first, so it precedes every deal the participants can make.
        order = [name for name, method in natives if method == "checkpoint"]
        assert order == ["erebus-shielded-disclosure", "erebus-wallet", "erebus-wallet"]
        assert (world.root / "qs/auditor/cache/history-checkpoint.json").is_file()
        access = json.loads((world.root / "qs/seller/access.json").read_text())
        assert (Path(access["state_root"]) / "history-checkpoint.json").is_file()
        again = quick.run(quick_args(world, "start"), ops())
        assert again["checkpoints"] == {"auditor": "existing", "buyer": "existing", "seller": "existing"}
        assert again["access_service"] == "running"
        status = quick.run(quick_args(world, "status"), ops())
        assert status["access_service"] == "running" and status["roles"]["buyer"]["checkpoint_block"] == 42
        assert status["auditor"]["history"] == "checkpoint" and status["auditor"]["checkpoint_block"] == 41
        assert status["auditor"]["history_from_block"] == 42
    finally:
        stopped = quick.run(quick_args(world, "down"), ops())
    assert stopped["access_service_was_running"] is True
    assert quick.run(quick_args(world, "status"), ops())["access_service"] == "stopped"
    assert (world.root / "qs/keys/buyer-gas.key").is_file()


def test_start_never_narrows_an_auditor_cache_that_began_full_history(world, natives):
    quick.run(quick_args(world, "up"), ops())
    (world.root / "qs/auditor/cache/primary").mkdir()
    started = quick.run(quick_args(world, "start"), ops())
    try:
        assert started["checkpoints"]["auditor"] == "full_history"
        assert not (world.root / "qs/auditor/cache/history-checkpoint.json").exists()
        assert ("erebus-shielded-disclosure", "checkpoint") not in natives
        status = quick.run(quick_args(world, "status"), ops())["auditor"]
        assert status["history"] == "full" and status["history_from_block"] == 7
    finally:
        quick.run(quick_args(world, "down"), ops())


def test_auditor_checkpoint_failure_stops_start_before_the_participants(world, natives, monkeypatch):
    quick.run(quick_args(world, "up"), ops())

    def refuse(bins, directory):
        raise ValueError("refused")
    monkeypatch.setattr(init, "auditor_checkpoint", refuse)
    with pytest.raises(ValueError, match="auditor's checkpoint failed"):
        quick.run(quick_args(world, "start"), ops())
    assert ("erebus-wallet", "checkpoint") not in natives


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


USDC_TOKEN = base.POOLS["usdc"][1]


def test_pool_defaults_are_in_each_pools_base_units(world, natives):
    quick.run(quick_args(world, "up", price=None, fund_amount=None), ops())
    wmon = json.loads((world.root / "qs" / quick.STATE).read_text())
    assert (wmon["price"], wmon["fund_amount"]) == (str(10**15), str(3 * 10**15))
    quick.run(quick_args(world, "up", directory=world.root / "u", pool="usdc", price=None, fund_amount=None), ops())
    usdc = json.loads((world.root / "u" / quick.STATE).read_text())
    assert (usdc["price"], usdc["fund_amount"]) == ("100000", "300000")
    assert quick.usdc(100000) == "0.100000" and quick.usdc(300000) == "0.300000"
    # Explicit amounts still win on either pool.
    quick.run(quick_args(world, "up", directory=world.root / "u2", pool="usdc", price="7", fund_amount="21"), ops())
    assert json.loads((world.root / "u2" / quick.STATE).read_text())["fund_amount"] == "21"


def test_usdc_funding_needs_its_sends_gas_limits_and_the_usdc_itself(world, natives):
    result = quick.run(quick_args(world, "up", pool="usdc", price=None, fund_amount=None, allow_agent_funding=True), ops())
    buyer = result["funding"]["buyer"]
    # Approve, deposit and a settlement at their own limits and the measured price, plus a settlement's
    # limit x max fee free for the next deal: no wrap, and no note value sent as MON.
    assert quick.BUYER_SENDS["usdc"] == ("approve", "deposit", "settle")
    assert quick.buyer_required_wei("usdc", 300000) == (65_000 + 2_100_000 + 3_010_000) * 102 * 10**9 + 3_010_000 * 300 * 10**9
    assert buyer["send_at_least_mon"] == "1.431" and buyer["why"].startswith("measured-price guidance")
    assert "wrapped" not in buyer["why"]
    assert (buyer["send_at_least_usdc_base_units"], buyer["send_at_least_usdc"], buyer["usdc_token"]) == (
        "300000", "0.300000", USDC_TOKEN)
    assert "300000 base units (0.300000 USDC)" in buyer["also"]
    state = json.loads((world.root / "qs" / quick.STATE).read_text())
    assert state["buyer_required_wei"] == str(quick.buyer_required_wei("usdc", 300000))
    assert (state["asset"], state["buyer_required_asset"]) == (USDC_TOKEN, "300000")
    policy = json.loads((world.root / "qs/buyer/wallet-policy.json").read_text())
    assert set(policy["allowed_methods"]) == {"notes", "sync", "status", "approve", "deposit"}
    assert policy["maximum_amount"] == "300000"
    prompt = (world.root / "qs/agents/buyer/prompt.md").read_text()
    assert 'fund_wallet(amount="300000")' in prompt and "0.300000 USDC" in prompt and "0.100000 USDC" in prompt
    assert "nothing wraps it" in prompt and "FUNDS_REQUIRED" in prompt
    assert "WMON" not in prompt and "USDC" not in (world.root / "qs/agents/seller/prompt.md").read_text()


def test_wmon_funding_and_prompt_are_unchanged(world, natives):
    result = quick.run(quick_args(world, "up", price=None, fund_amount=None), ops())
    buyer = result["funding"]["buyer"]
    fund = 3 * 10**15
    assert quick.BUYER_SENDS["wmon"] == ("wrap", "approve", "deposit", "settle")
    # WS6 target: below 1.6 MON (was 2.143); 5.24M gas at 102 gwei + 0.903 + 0.003 wrapped.
    assert quick.buyer_required_wei("wmon", fund) == 5_240_000 * 102 * 10**9 + 3_010_000 * 300 * 10**9 + fund
    assert buyer["send_at_least_mon"] == "1.440"
    assert set(buyer) == {"address", "send_at_least_mon", "why"} and buyer["why"].endswith("MON wrapped into the shielded note")
    state = json.loads((world.root / "qs" / quick.STATE).read_text())
    assert "asset" not in state and "buyer_required_asset" not in state
    prompt = (world.root / "qs/agents/buyer/prompt.md").read_text()
    assert "USDC" not in prompt and "{asset_note}" not in prompt
    assert ("ask the operator to fund the wallet instead.)\n2. Call negotiate_deal()" in prompt)
    # settle_deal checks funding itself; the prompt no longer requires check_settlement_funding.
    assert "check_settlement_funding" not in prompt and "3. Call settle_deal(operation_ref)" in prompt


def test_quickstart_gas_constants_match_the_operator_policy_and_native_limits():
    native = (ROOT / "sdk/shielded/src/gas_policy.rs").read_text()
    for name, kind in (("WRAP", "wrap"), ("APPROVE", "approve"), ("DEPOSIT", "deposit"), ("SETTLE", "settle")):
        assert f"pub const {name}_GAS_LIMIT: u64 = {quick.GAS_LIMITS[kind]:_};" in native, name
    text = (ROOT / "mcp-server/src/erebus_mcp/metropolis_setup/operator_init.py").read_text()
    assert f'"max_fee_per_gas": "{quick.MAX_FEE_PER_GAS}"' in text


def test_funding_waits_for_both_mon_gas_and_usdc(world, natives, monkeypatch):
    balances = {"mon": 0, "usdc": 0}
    reads = []
    fake_ops = SimpleNamespace(rpc_balance=lambda url, address: balances["mon"],
                               rpc_token_balance=lambda url, token, address: reads.append(token) or balances["usdc"])
    state = {"signers": {"buyer": "0x" + "aa" * 20}, "buyer_required_wei": str(183 * 10**16),
             "asset": USDC_TOKEN, "buyer_required_asset": "300000"}
    assert quick._funded(fake_ops, state, "https://x") is False and reads == []
    balances["mon"] = 183 * 10**16
    assert quick._funded(fake_ops, state, "https://x") is False and reads == [USDC_TOKEN]
    balances["usdc"] = 299999
    assert quick._funded(fake_ops, state, "https://x") is False
    balances["usdc"] = 300000
    assert quick._funded(fake_ops, state, "https://x") is True
    # WMON waits for MON only and never reads a token balance.
    wmon = {"signers": state["signers"], "buyer_required_wei": state["buyer_required_wei"]}
    fake_ops.rpc_token_balance = lambda *a: pytest.fail("WMON must not read a token balance")
    assert quick._funded(fake_ops, wmon, "https://x") is True
    monkeypatch.setattr(quick.time, "sleep", lambda seconds: None)
    balances["usdc"] = 0
    fake_ops.rpc_token_balance = lambda url, token, address: balances["usdc"]
    assert quick._wait_for_funding(fake_ops, state, "https://x", 0) is False


def test_status_reports_the_usdc_balance_and_what_is_left_to_fund(world, natives, monkeypatch):
    quick.run(quick_args(world, "up", pool="usdc", price=None, fund_amount=None), ops())
    monkeypatch.setattr(quick, "_head", lambda url: None)
    monkeypatch.setattr(init, "rpc_token_balance", lambda url, token, address, opener=None:
                        {"0x" + "aa" * 20: 300000}.get(address, 0) if token == USDC_TOKEN else pytest.fail(token))
    status = quick.run(quick_args(world, "status", offline=False), ops())
    assert status["roles"]["buyer"]["balance_usdc_base_units"] == "300000"
    assert status["roles"]["seller"]["balance_usdc_base_units"] == "0"
    assert status["next"] == ("fund the buyer with at least 1.431 MON and 300000 base units (0.300000 USDC) of "
                              "testnet USDC, then run quickstart start")
    quick.run(quick_args(world, "up", directory=world.root / "w"), ops())
    wmon = quick.run(quick_args(world, "status", directory=world.root / "w", offline=False), ops())
    assert "balance_usdc_base_units" not in wmon["roles"]["buyer"]
    assert wmon["next"].endswith(" MON, then run quickstart start")


def test_rpc_token_balance_reads_finalized_balance_of():
    sent = []

    class Reply:
        def __init__(self, body):
            self.body = body

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def read(self, limit):
            return self.body

    def opener(request, timeout):
        sent.append(json.loads(request.data))
        return Reply(json.dumps({"result": "0x" + format(300000, "064x")}).encode())
    holder = "0x" + "aa" * 20
    assert init.rpc_token_balance("https://x.example", USDC_TOKEN, holder, opener) == 300000
    (call, tag), = [request["params"] for request in sent]
    assert sent[0]["method"] == "eth_call" and tag == "finalized"
    assert call == {"to": USDC_TOKEN, "data": "0x70a08231" + "0" * 24 + "aa" * 20}
    with pytest.raises(ValueError):
        init.rpc_token_balance("https://x.example", "0x12", holder, opener)

    def short(request, timeout):
        return Reply(b'{"result": "0x"}')
    with pytest.raises(ValueError, match="one word"):
        init.rpc_token_balance("https://x.example", USDC_TOKEN, holder, short)


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


def test_status_reads_the_tip_of_segmented_and_legacy_history_caches(tmp_path):
    manifest = tmp_path / "v4.json"
    manifest.write_text(json.dumps({"version": 4, "tip": {"number": 72970452, "hash": "90" * 32}, "segments": []}))
    legacy = tmp_path / "v3.json"
    legacy.write_text(json.dumps({"version": 3, "blocks": [{"number": 7}, {"number": 8}]}))
    empty = tmp_path / "empty.json"
    empty.write_text(json.dumps({"version": 4, "tip": None, "segments": []}))
    assert quick._tip(manifest) == 72970452
    assert quick._tip(legacy) == 8
    assert quick._tip(empty) is None
    assert quick._tip(tmp_path / "missing.json") is None


def test_add_buyer_wires_a_second_buyer_without_touching_the_service(world, natives):
    quick.run(quick_args(world, "up"), ops())
    directory = world.root / "qs"
    seller = directory / "seller"
    access = (seller / "access.json").read_bytes()
    default = base.tree(seller / "peers" / "default")
    added = quick.run(quick_args(world, "add-buyer", name="b2", price="900", fund_amount="2000"), ops())
    assert added["status"] == "added" and added["peer"] == "b2" and added["price"] == "900"
    assert (seller / "access.json").read_bytes() == access
    assert base.tree(seller / "peers" / "default") == default
    buyer = directory / "buyers" / "b2"
    negotiate = json.loads((buyer / "negotiate.json").read_text())
    assert negotiate["negotiation_transport"] == 2 and negotiate["maximum_price"] == "900"
    assert negotiate["peer_descriptor_file"] == str(seller / "outbox" / "b2" / "seller.descriptor.json")
    assert json.loads((seller / "peers" / "b2" / "peer.json").read_text())["minimum_price"] == "900"
    state = json.loads((directory / quick.STATE).read_text())
    ports = set(state["ports"].values())
    assert state["buyers"]["b2"]["port"] not in ports and state["buyers"]["b2"]["price"] == "900"
    assert set(json.loads((buyer / "wallet-policy.json").read_text())["allowed_methods"]) == {"notes", "sync", "status"}
    files = directory / "agents" / "b2"
    assert list(json.loads((files / "claude-code.mcp.json").read_text())["mcpServers"]) == ["erebus-buyer"]
    assert "negotiate_deal()" in (files / "prompt.md").read_text()
    env = json.loads((files / "claude-code.mcp.json").read_text())["mcpServers"]["erebus-buyer"]["env"]
    assert env["EREBUS_NEGOTIATION_CONFIG"] == str(buyer / "negotiate.json")
    # The next name is allocated, and a name is never reused.
    third = quick.run(quick_args(world, "add-buyer"), ops())
    assert third["buyer"] == "b3"
    assert json.loads((directory / quick.STATE).read_text())["buyers"]["b3"]["port"] == state["buyers"]["b2"]["port"] + 1
    for name in ("b2", "default", "Bad"):
        with pytest.raises(ValueError, match="--name"):
            quick.run(quick_args(world, "add-buyer", name=name), ops())
    with pytest.raises(ValueError, match="cover"):
        quick.run(quick_args(world, "add-buyer", name="b9", price="5000", fund_amount="10"), ops())
    assert not (directory / "buyers" / "b9").exists()
    assert "seller sends nothing" not in json.dumps(added)
    assert not any(method in {"wrap", "deposit", "approve", "checkpoint"} for _, method in natives)


def test_start_checkpoints_funded_added_buyers_and_status_lists_every_buyer(world, natives, monkeypatch):
    quick.run(quick_args(world, "up"), ops())
    quick.run(quick_args(world, "add-buyer", name="b2", price="900"), ops())
    quick.run(quick_args(world, "add-buyer", name="b3", price="800"), ops())
    state = json.loads((world.root / "qs" / quick.STATE).read_text())
    unfunded = state["buyers"]["b3"]["signer"]
    monkeypatch.setattr(init, "rpc_balance",
                        lambda url, address, opener=None: 0 if address == unfunded and address != state["buyers"]["b2"]["signer"] else 3 * 10**18)
    started = quick.run(quick_args(world, "start"), ops())
    try:
        assert started["checkpoints"]["buyer"] == 42 and started["checkpoints"]["seller"] == 42
        assert set(started["doctor"]) == {"buyer", "seller", "b2", "b3"}
        status = quick.run(quick_args(world, "status"), ops())
        assert set(status["roles"]) == {"buyer", "seller", "b2", "b3"}
        assert (status["roles"]["buyer"]["peer"], status["roles"]["b2"]["peer"]) == ("default", "b2")
        assert status["roles"]["b2"]["price"] == "900"
    finally:
        quick.run(quick_args(world, "down"), ops())


def test_renew_gives_every_buyer_matching_fresh_terms(world, natives):
    quick.run(quick_args(world, "up"), ops())
    quick.run(quick_args(world, "add-buyer", name="b2", price="900"), ops())
    directory = world.root / "qs"
    seller = directory / "seller"
    old = {"default": json.loads((seller / "share" / "offer.json").read_text())["delivery_deadline"],
           "b2": json.loads((seller / "outbox" / "b2" / "offer.json").read_text())["delivery_deadline"]}
    import time
    time.sleep(1.1)
    result = quick.run(quick_args(world, "renew"), ops())
    assert set(result["delivery_deadlines"]) == {"default", "b2"}
    for peer, buyer, share in (("default", directory / "buyer", seller / "share"),
                               ("b2", directory / "buyers" / "b2", seller / "outbox" / "b2")):
        deadline = result["delivery_deadlines"][peer]
        assert deadline > old[peer]
        assert json.loads((share / "offer.json").read_text())["delivery_deadline"] == deadline
        # The fake terms record their request: seller and buyer terms carry the same new deadline.
        seller_terms = json.loads((seller / "peers" / peer / "service.terms").read_text())
        buyer_terms = json.loads((buyer / "keys" / "service.terms").read_text())
        assert seller_terms["delivery_deadline"] == buyer_terms["delivery_deadline"] == deadline
        assert (seller / "peers" / peer / "descriptor.json").read_bytes() == next(
            (buyer / "share").glob("*.descriptor.json")).read_bytes()
    assert json.loads((seller / "peers" / "b2" / "descriptor.json").read_text())["expires"] == 4_000_000_000
    assert (seller / "outbox" / "b2" / "seller.descriptor.json").read_bytes() == (
        seller / "share" / "seller.descriptor.json").read_bytes()
    assert not list(directory.rglob("*.renew"))
    # The renewed peer is still recognized by a repeated connect for the same buyer.
    repeated = init.connect(base.connect_args(world, "seller", directory=seller, peer_share=directory / "buyers/b2/share",
                                              peer_id="b2", amount="900", resource="sample", delivery_seconds=21600,
                                              payload_file=None, port=None))
    assert repeated["repeated"] is True


def test_add_buyer_on_a_usdc_pool_waits_for_gas_and_usdc_and_checkpoints_after_the_auditor(world, natives, monkeypatch):
    quick.run(quick_args(world, "up", pool="usdc", price=None, fund_amount=None), ops())
    added = quick.run(quick_args(world, "add-buyer", name="b2", price=None, fund_amount=None), ops())
    directory = world.root / "qs"
    state = json.loads((directory / quick.STATE).read_text())
    # Per-pool defaults and requirements: four sends of gas, no wrap, and the USDC itself.
    assert (added["price"], state["buyers"]["b2"]["fund_amount"]) == ("100000", "300000")
    funding = added["funding"]
    assert funding["send_at_least_mon"] == quick.mon(quick.buyer_required_wei("usdc", 300000))
    assert (funding["send_at_least_usdc_base_units"], funding["usdc_token"]) == ("300000", state["asset"])
    assert state["buyers"]["b2"]["buyer_required_asset"] == "300000"
    assert "USDC base units" in (directory / "agents" / "b2" / "prompt.md").read_text()
    signer = state["buyers"]["b2"]["signer"]
    usdc_held = {"b2": 0}
    monkeypatch.setattr(init, "rpc_token_balance",
                        lambda url, token, address, opener=None: usdc_held["b2"] if address == signer else 10**9)
    started = quick.run(quick_args(world, "start"), ops())
    try:
        assert next(iter(started["checkpoints"])) == "auditor"
        assert started["checkpoints"]["b2"] == "waiting for funding"
        usdc_held["b2"] = 300000
        again = quick.run(quick_args(world, "start"), ops())
        assert next(iter(again["checkpoints"])) == "auditor" and again["checkpoints"]["b2"] == 42
    finally:
        quick.run(quick_args(world, "down"), ops())
