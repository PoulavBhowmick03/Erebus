"""Starknet mainnet quickstart against a fake chain and a fake seam. Nothing touches a network."""

from __future__ import annotations

import asyncio
import json
import os
import re
from pathlib import Path

import pytest
from erebus import PROTOCOL, SeamUnavailable

from erebus_mcp import config as config_module
from erebus_mcp import setup
from erebus_mcp.onboarding import load_config_file
from erebus_mcp.starknet_setup import quickstart as quick

WEI = 10**18
FEE = 6 * WEI  # fixture fee, not a live value
GAS = 3 * WEI  # fixture gas reserve, not a live value
MAINNET = "0x534e5f4d41494e"
ADDRESSES = {"buyer": "0xb0b0", "seller": "0x5e11"}
SECRET_RPC = "https://rpc.example.net/v0_8/SECRETRPCKEY"


class Identity:
    def __init__(self, address: str) -> None:
        self.address = address
        self.balance = 0
        self.allowance = 0
        self.registered = "0x0"
        self.deployed = False
        self.notes: list[int] = []
        self.journal: dict[str, dict] = {}


class Chain:
    """Blocks advance by `auto` on every head read, so waiting code really waits."""

    def __init__(self) -> None:
        self.block = 100
        self.auto = 1
        self.chain_id = MAINNET
        self.receipts: dict[str, int] = {}
        self.ids: dict[str, Identity] = {}
        self.writes: list[tuple] = []
        self.crash: set[tuple[str, str]] = set()
        self.urls: set[str] = set()
        self.count = 0

    def role(self, address: str) -> str:
        return next(role for role, value in ADDRESSES.items() if value == address)

    def tx(self) -> str:
        self.count += 1
        digest = f"0x{self.count:04x}"
        self.receipts[digest] = self.block
        return digest

    def rpc(self, url, method, params):
        self.urls.add(url)
        if method == "starknet_chainId":
            return self.chain_id
        if method == "starknet_blockNumber":
            self.block += self.auto
            return self.block
        if method == "starknet_getTransactionReceipt":
            return {"execution_status": "SUCCEEDED", "block_number": self.receipts[params["transaction_hash"]]}
        raise AssertionError(method)

    def kinds(self, role: str | None = None, kind: str | None = None) -> list[tuple]:
        return [w for w in self.writes if (role is None or w[1] == role) and (kind is None or w[0] == kind)]


class FakeSeam:
    def __init__(self, chain: Chain, config=None) -> None:
        self.chain, self.config = chain, config

    def version(self):
        return {"protocol": PROTOCOL}

    def generate_pool_key(self, path):
        Path(path).write_text("test-only")

    def call(self, method, params):
        assert method == "onboarding"
        action, args = params["action"], params["args"]
        if "rpc_url" in args:
            self.chain.urls.add(args["rpc_url"])
        if action == "discover":
            return []
        if action in {"create", "import"}:
            root = Path(args["directory"])
            role = root.parent.name
            address = ADDRESSES[role]
            if not (root / "account.json").exists():
                (root / "account.key").write_text("test-only")
                (root / "account.json").write_text(json.dumps({"address": address, "chain_id": args["chain_id"]}))
            self.chain.ids.setdefault(address, Identity(address))
            return {"address": address, "account_key_file": str(root / "account.key")}
        identity = self.chain.ids[args.get("address") or ADDRESSES[Path(args["directory"]).parent.name]]
        if action == "inspect":
            return {"public_balance": str(identity.balance), "allowance": str(identity.allowance),
                    "fee_per_write": str(FEE), "registered_public_key": identity.registered,
                    "gas_reserve_per_write": str(GAS), "proving_block_lag": 10}
        if action == "status":
            return {"address": identity.address, "deployed": identity.deployed}
        if action == "deploy":
            identity.deployed = True
            self.chain.writes.append(("deploy", self.chain.role(identity.address), self.chain.block))
            return {"transaction_hash": self.chain.tx()}
        if action == "verify_signer":
            return {"verified": True}
        raise AssertionError(action)

    @property
    def me(self) -> Identity:
        return self.chain.ids[self.config.account_address]

    def _write(self, kind, operation_id, amount, effect):
        role = self.chain.role(self.me.address)
        assert operation_id not in self.me.journal, f"{kind} sent twice under {operation_id}"
        effect()
        tx = self.chain.tx()
        self.me.journal[operation_id] = {"kind": kind, "tx": tx}
        self.chain.writes.append((kind, role, self.chain.block, int(amount), operation_id))
        if (kind, role) in self.chain.crash:
            self.chain.crash.discard((kind, role))
            raise SeamUnavailable("killed after submitting")
        return {"tx_hash": tx}

    def approve(self, operation_id, amount):
        return self._write("approve", operation_id, amount, lambda: setattr(self.me, "allowance", int(amount)))

    def shield(self, operation_id, amount):
        me = self.me

        def effect():
            assert me.allowance >= int(amount) + FEE
            me.allowance -= int(amount) + FEE
            me.balance -= int(amount) + FEE
            me.registered = "0x456"
            me.notes.append(int(amount))

        return self._write("shield", operation_id, amount, effect)

    def reconcile(self):
        return [{"operation_id": op, "operation": record["kind"], "outcome": "effect", "next_action": "none"}
                for op, record in self.me.journal.items()]

    def resume_operation(self, operation_id):
        self.chain.writes.append(("resume", self.chain.role(self.me.address), operation_id))
        return {"result": "already_complete", "transaction_hash": self.me.journal[operation_id]["tx"]}

    def allowance(self):
        return {"allowance": str(self.me.allowance), "fee_per_write": str(FEE)}

    def doctor(self):
        return {"ready": self.me.registered != "0x0", "checks": []}

    def balance(self):
        return {"notes": [str(n) for n in self.me.notes], "pending": []}


@pytest.fixture
def chain(monkeypatch, tmp_path):
    monkeypatch.setattr(Path, "home", lambda: tmp_path / "home")
    monkeypatch.delenv("STARKSCAN_API_KEY", raising=False)
    monkeypatch.delenv("EREBUS_CLI", raising=False)
    fake = Chain()
    monkeypatch.setattr(setup, "rpc", fake.rpc)
    monkeypatch.setattr(setup.time, "sleep", lambda seconds: None)
    return fake


@pytest.fixture
def files(tmp_path):
    bins = tmp_path / "bin"
    bins.mkdir()
    for name in ("erebus-cli", "erebus-mcp-server"):
        (bins / name).write_text("#!/bin/sh\n")
        (bins / name).chmod(0o755)
    key = tmp_path / "starkscan.key"
    key.write_text("test-only-starkscan\n")
    key.chmod(0o600)
    rpc = tmp_path / "rpc.url"
    rpc.write_text(SECRET_RPC + "\n")
    rpc.chmod(0o600)
    return {"cli": bins / "erebus-cli", "server": bins / "erebus-mcp-server", "key": key, "rpc": rpc}


def deps(chain, **changes):
    values = dict(seam_factory=lambda config=None: FakeSeam(chain, config), interactive=False,
                  sleep=lambda seconds: None, environ={}, log=lambda message: None)
    values.update(changes)
    return quick.Deps(**values)


def args(tmp_path, files, action, *extra):
    argv = ["quickstart", action, "--directory", str(tmp_path / "sn"), "--cli", str(files["cli"]),
            "--server-command", str(files["server"])]
    if action == "up":
        argv += ["--rpc-url-file", str(files["rpc"]), "--starkscan-key-file", str(files["key"]),
                 "--funding-timeout", "0", "--setup-wait", "120"]
    return quick.build_parser().parse_args(argv + list(extra))


def fund(chain, buyer=int(42.5 * WEI), seller=33_010_000_000_000_000_000):
    chain.ids[ADDRESSES["buyer"]].balance += buyer
    chain.ids[ADDRESSES["seller"]].balance += seller


def ready(tmp_path, files, chain):
    assert quick.run(args(tmp_path, files, "up", "--max-strk", "100"), deps(chain))["status"] == "funding_required"
    fund(chain)
    result = quick.run(args(tmp_path, files, "up", "--max-strk", "100", "--yes"), deps(chain))
    assert result["status"] == "ready", result
    return result


def env(tmp_path, role):
    values: dict[str, str] = {}
    load_config_file(tmp_path / "sn" / role / "mcp.env", values)
    return values


# --- planning --------------------------------------------------------------------------------


def test_plan_arithmetic_with_fixture_fees():
    buyer = setup.plan_budget(deposit=WEI // 2, writes=3, fee_per_write=FEE, gas_reserve_per_write=GAS,
                              deployed=False, current_allowance=0)
    seller = setup.plan_budget(deposit=WEI // 100, writes=2, fee_per_write=FEE, gas_reserve_per_write=GAS,
                               deployed=False, current_allowance=0)
    assert quick.strk(buyer["required"]) == "42.5" and quick.strk(buyer["allowance"]) == "24.5"
    assert quick.strk(seller["required"]) == "33.01" and quick.strk(seller["allowance"]) == "18.01"
    assert (buyer["charged_writes"], seller["charged_writes"]) == (4, 3)


def test_up_prints_both_addresses_and_exact_targets_and_sends_nothing(tmp_path, files, chain):
    result = quick.run(args(tmp_path, files, "up", "--max-strk", "100"), deps(chain))
    assert result["status"] == "funding_required"
    plan = result["plan"]
    assert {role: plan[role]["address"] for role in quick.ROLES} == ADDRESSES
    assert plan["buyer"]["send_until_balance_is_strk"] == "42.5"
    assert plan["seller"]["send_until_balance_is_strk"] == "33.01"
    assert plan["buyer"]["fee_per_write_strk"] == "6" and plan["buyer"]["gas_reserve_per_transaction_strk"] == "3"
    assert result["total_strk"] == "75.51"
    assert chain.writes == []
    state = (tmp_path / "sn" / quick.STATE).read_text()
    saved = json.loads(state)
    assert saved["backend"] == "starknet" and saved["network"] == "mainnet" and saved["chain_id"] == "SN_MAIN"
    assert saved["rpc_host"] == "rpc.example.net" and saved["prover_host"] == "api.starkscan.co"
    # Endpoints are recorded by host only; full URLs live in each role's mode-0600 mcp.env.
    assert not [key for key in saved if key.endswith("_url")] and "/prove" not in state
    assert "SECRET" not in state and "test-only-starkscan" not in state and "SECRET" not in json.dumps(result)
    assert (tmp_path / "sn" / quick.STATE).stat().st_mode & 0o777 == 0o600


def test_extra_rounds_and_deposit_size_each_role(tmp_path, files, chain):
    result = quick.run(args(tmp_path, files, "up", "--max-strk", "200", "--extra-rounds", "2"), deps(chain))
    # buyer: 0.5 + 6 x (5 + 1) = 36.5 allowance, 36.5 + 3 x (6 + 2) = 60.5 target
    assert result["plan"]["buyer"]["required_allowance_strk"] == "36.5"
    assert result["plan"]["buyer"]["send_until_balance_is_strk"] == "60.5"
    assert result["plan"]["seller"]["send_until_balance_is_strk"] == "51.01"
    assert env(tmp_path, "buyer")["EREBUS_MAX_WRITES"] == "5" and env(tmp_path, "seller")["EREBUS_MAX_WRITES"] == "4"


# --- refusals --------------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        (["--network", "custom"], "mainnet only"),
        (["--network", "sepolia"], "mainnet only"),
        (["--prover-url", "https://prover.example/v1/SN_MAIN/prove"], "unknown prover host"),
        (["--rpc-url", "http://rpc.example.net"], "https"),
        (["--buyer-account", "sncast:alpha-sepolia:alice"], "mainnet sncast account"),
        (["--buyer-account", "/home/me/.config/erebus/buyer.env"], "mainnet sncast account"),
    ],
)
def test_bad_network_host_or_account_is_refused_before_anything_exists(tmp_path, files, chain, extra, message):
    options = args(tmp_path, files, "up", "--max-strk", "100", *extra)
    if "--rpc-url" in extra:
        options.rpc_url_file = None
    with pytest.raises(ValueError, match=message):
        quick.run(options, deps(chain))
    assert not (tmp_path / "sn").exists() and chain.writes == []


def test_wrong_chain_is_refused(tmp_path, files, chain):
    chain.chain_id = "0x534e5f5345504f4c4941"
    with pytest.raises(ValueError, match="not Starknet mainnet"):
        quick.run(args(tmp_path, files, "up", "--max-strk", "100"), deps(chain))
    assert not (tmp_path / "sn").exists() and chain.ids == {}


def test_mainnet_requires_max_strk_and_refuses_a_plan_above_it(tmp_path, files, chain):
    missing = quick.run(args(tmp_path, files, "up"), deps(chain))
    assert missing["status"] == "configuration_required" and "--max-strk" in missing["missing"]
    above = quick.run(args(tmp_path, files, "up", "--max-strk", "75.5"), deps(chain))
    assert above["status"] == "plan_exceeds_max" and above["total_strk"] == "75.51"
    fund(chain)
    assert quick.run(args(tmp_path, files, "up", "--max-strk", "75.5", "--yes"), deps(chain))["status"] == \
        "plan_exceeds_max"
    assert chain.writes == []


def test_noninteractive_needs_yes_and_terminal_needs_the_exact_phrase(tmp_path, files, chain):
    quick.run(args(tmp_path, files, "up", "--max-strk", "100"), deps(chain))
    fund(chain)
    pending = quick.run(args(tmp_path, files, "up"), deps(chain))
    assert pending["status"] == "authorization_required" and "mainnet 75.51" in pending["next"]
    assert "Starkscan prover (api.starkscan.co)" in pending["notice"] and "rpc.example.net" in pending["notice"]
    assert "receive each identity's pool key" in pending["notice"]
    typed = []
    wrong = quick.run(args(tmp_path, files, "up"), deps(chain, interactive=True,
                                                     input=lambda prompt: typed.append(prompt) or "yes"))
    assert wrong["status"] == "authorization_required" and chain.writes == []
    assert '"mainnet 75.51"' in typed[0]
    right = quick.run(args(tmp_path, files, "up"), deps(chain, interactive=True, input=lambda prompt: "mainnet 75.51"))
    assert right["status"] == "ready"


# --- execution order and resume ----------------------------------------------------------------


def test_up_deploys_approves_once_waits_ten_blocks_then_shields_each_role(tmp_path, files, chain):
    result = ready(tmp_path, files, chain)
    for role, allowance, deposit in (("buyer", 24_500_000_000_000_000_000, WEI // 2),
                                     ("seller", 18_010_000_000_000_000_000, WEI // 100)):
        kinds = [w[0] for w in chain.kinds(role)]
        assert kinds == ["deploy", "approve", "shield"]
        (approve,), (shield,) = chain.kinds(role, "approve"), chain.kinds(role, "shield")
        assert approve[3] == allowance and shield[3] == deposit
        assert shield[2] >= approve[2] + 10
    # Buyer first, then seller.
    assert [w[1] for w in chain.writes] == ["buyer"] * 3 + ["seller"] * 3
    assert result["checks"]["buyer"]["counterparty_registered"] and result["checks"]["seller"]["registered"]
    assert set(result["register"]) == {"buyer", "seller"}
    # Rerunning a finished up never writes again.
    with pytest.raises(ValueError, match="already set up"):
        quick.run(args(tmp_path, files, "up", "--yes"), deps(chain))


def test_no_shield_before_the_approval_is_ten_blocks_deep(tmp_path, files, chain):
    quick.run(args(tmp_path, files, "up", "--max-strk", "100"), deps(chain))
    fund(chain)
    chain.auto = 0  # the chain stops
    options = args(tmp_path, files, "up", "--yes", "--setup-wait", "0")
    first = quick.run(options, deps(chain))
    assert first["status"] in {"deployment_pending", "approval_pending"}
    for _ in range(3):
        again = quick.run(options, deps(chain))
    assert again["status"] == "approval_pending" and again["role"] == "buyer"
    assert [w[0] for w in chain.writes] == ["deploy", "approve"]
    chain.block += 9
    assert quick.run(options, deps(chain))["status"] == "approval_pending"
    assert not chain.kinds(kind="shield")
    chain.block += 1
    assert quick.run(options, deps(chain))["status"] == "shield_pending"
    assert len(chain.kinds("buyer", "shield")) == 1 and len(chain.kinds("buyer", "approve")) == 1


@pytest.mark.parametrize("kind", ["approve", "shield"])
@pytest.mark.parametrize("role", ["buyer", "seller"])
def test_crash_after_submitting_resumes_without_repeating_the_write(tmp_path, files, chain, kind, role):
    quick.run(args(tmp_path, files, "up", "--max-strk", "100"), deps(chain))
    fund(chain)
    chain.crash = {(kind, role)}
    crashed = quick.run(args(tmp_path, files, "up", "--yes"), deps(chain))
    assert crashed["status"] == "setup_error" and crashed["role"] == role
    result = quick.run(args(tmp_path, files, "up", "--yes"), deps(chain))
    assert result["status"] == "ready", result
    for each in quick.ROLES:
        assert [w[0] for w in chain.kinds(each) if w[0] != "resume"] == ["deploy", "approve", "shield"]
    assert len(chain.kinds(role, "resume")) == 1


def test_live_fee_rise_after_confirmation_needs_a_new_confirmation(tmp_path, files, chain, monkeypatch):
    quick.run(args(tmp_path, files, "up", "--max-strk", "200"), deps(chain))
    fund(chain, buyer=100 * WEI, seller=100 * WEI)
    chain.auto = 0
    # Confirm, but stop before any approval reaches depth.
    quick.run(args(tmp_path, files, "up", "--yes", "--setup-wait", "0"), deps(chain))
    state = json.loads((tmp_path / "sn" / quick.STATE).read_text())
    state["confirmed"]["allowance"]["seller"] = str(10 * WEI)
    setup.save(tmp_path / "sn" / quick.STATE, state)
    chain.auto = 1
    result = quick.run(args(tmp_path, files, "up"), deps(chain))
    assert result["status"] == "authorization_required"
    assert not chain.kinds("seller", "approve")


# --- generated files ---------------------------------------------------------------------------


def test_agent_files_are_complete_and_use_starknet_server_names(tmp_path, files, chain):
    ready(tmp_path, files, chain)
    root = tmp_path / "sn"
    for role, other in (("buyer", "seller"), ("seller", "buyer")):
        name = quick.SERVER_NAMES[role]
        out = root / "agents" / role
        assert sorted(p.name for p in out.iterdir()) == ["claude-code.mcp.json", "codex.config.toml",
                                                         "cursor.mcp.json", "prompt.md"]
        for client in ("claude-code.mcp.json", "cursor.mcp.json"):
            entry = json.loads((out / client).read_text())["mcpServers"]
            assert list(entry) == [name]
            assert entry[name] == {"command": str(files["server"]),
                                   "args": ["--config", str(root / role / "mcp.env")]}
        assert f"[mcp_servers.{name}]" in (out / "codex.config.toml").read_text()
        assert all((out / f).stat().st_mode & 0o077 == 0 for f in os.listdir(out))
        values = env(tmp_path, role)
        assert values["EREBUS_SETTLEMENT_ROLE"] == quick.SETTLEMENT[role]
        assert values["EREBUS_NETWORK"] == "mainnet" and values["EREBUS_BACKEND"] == "seam"
        assert values["EREBUS_REQUIRE_CANONICAL_NETWORK"] == "1"
        assert values["EREBUS_CLI"] == str(files["cli"])
        assert json.loads(values["EREBUS_SPENDING_LIMITS"]) == {
            setup.STRK: {"per_deal": str(WEI // 5), "daily": str(2 * WEI // 5)}}
        for key in ("EREBUS_STATE_DIR", "EREBUS_INTENT_STATE_DIR", "EREBUS_SPENDING_STATE_PATH",
                    "POOL_KEY_FILE", "ACCOUNT_KEY_FILE"):
            assert Path(values[key]).is_relative_to(root), key
        prompt = (out / "prompt.md").read_text()
        assert ADDRESSES[other] in prompt and name in prompt
    buyer, seller = ((root / "agents" / r / "prompt.md").read_text() for r in ("buyer", "seller"))
    order = ["open_channel", "Wait for the counterparty's channel", "propose_offer", "wait_for_offers",
             "accept_and_settle"]
    assert [buyer.index(step) for step in order] == sorted(buyer.index(step) for step in order)
    order = ["open_channel", "wait_for_offers", "counter_offer", "read_channel_state"]
    assert [seller.index(step) for step in order] == sorted(seller.index(step) for step in order)
    assert "Never call accept_and_settle" in seller and "Counter at most once per buyer offer, 1 in total" in seller
    assert "Do not counter" in buyer


def test_start_status_renew_and_down(tmp_path, files, chain):
    ready(tmp_path, files, chain)
    started = quick.run(args(tmp_path, files, "start"), deps(chain))
    assert started["status"] == "ready"
    assert started["register"]["buyer"]["claude_code"].startswith("claude mcp add erebus-starknet-buyer -- ")
    assert "erebus-starknet-seller" in started["prompts"]["seller"]
    status = quick.run(args(tmp_path, files, "status"), deps(chain))["roles"]
    assert status["buyer"]["registered"] and status["buyer"]["deployed"]
    assert status["buyer"]["allowance_strk"] == "18" and status["buyer"]["allowance_covers_writes"] == 3
    assert status["seller"]["allowance_covers_writes"] == 2 and status["buyer"]["notes_strk"] == ["0.5"]
    assert (status["buyer"]["writes_used"], status["buyer"]["writes_cap"]) == (0, 3)
    assert status["buyer"]["unresolved_operations"] == [] and status["buyer"]["pending_intents"] == []
    offline = quick.run(args(tmp_path, files, "status", "--offline"), deps(chain))
    assert offline["offline"] and "public_strk" not in offline["roles"]["buyer"]
    assert offline["roles"]["buyer"]["setup_steps_done"] == ["approve", "shield"]

    before = len(chain.writes)
    unconfirmed = quick.run(args(tmp_path, files, "renew", "--deals", "2", "--max-strk", "100"), deps(chain))
    assert unconfirmed["status"] == "authorization_required" and len(chain.writes) == before
    renewed = quick.run(args(tmp_path, files, "renew", "--deals", "2", "--max-strk", "100", "--yes"), deps(chain))
    assert renewed["status"] == "renewed"
    assert renewed["approved_strk"] == {"buyer": "24", "seller": "12"}
    # An ERC-20 approve replaces: the allowance is exactly 2N fees for the buyer and N for the seller.
    assert chain.ids[ADDRESSES["buyer"]].allowance == 24 * WEI
    assert chain.ids[ADDRESSES["seller"]].allowance == 12 * WEI
    assert (env(tmp_path, "buyer")["EREBUS_MAX_WRITES"], env(tmp_path, "seller")["EREBUS_MAX_WRITES"]) == ("4", "2")
    assert len(chain.kinds("buyer", "approve")) == 2 and len(chain.kinds("seller", "approve")) == 2
    down = quick.run(args(tmp_path, files, "down", "--offline"), deps(chain))
    assert down["roles"]["buyer"]["remove"]["claude_code"] == "claude mcp remove erebus-starknet-buyer"
    assert "unshield" in down["roles"]["buyer"]["sweep"]["shielded_notes"]
    assert (tmp_path / "sn" / "buyer" / "mcp.env.identity" / "pool.key").read_text() == "test-only"


def test_renew_resumes_an_interrupted_approval_without_sending_it_again(tmp_path, files, chain):
    ready(tmp_path, files, chain)
    chain.crash = {("approve", "seller")}
    with pytest.raises(SeamUnavailable):
        quick.run(args(tmp_path, files, "renew", "--deals", "1", "--max-strk", "100", "--yes"), deps(chain))
    with pytest.raises(ValueError, match="same --deals"):
        quick.run(args(tmp_path, files, "renew", "--deals", "3"), deps(chain))
    resumed = quick.run(args(tmp_path, files, "renew", "--deals", "1"), deps(chain))
    assert resumed["status"] == "renewed"
    assert len(chain.kinds("seller", "approve")) == 2 and len(chain.kinds("buyer", "approve")) == 2
    assert len(chain.kinds("seller", "resume")) == 1


def test_starknet_and_monad_markers_refuse_each_other(tmp_path, files, chain):
    monad = tmp_path / "monad"
    monad.mkdir()
    (monad / quick.MONAD_STATE).write_text("{}")
    options = args(tmp_path, files, "up", "--max-strk", "100")
    options.directory = monad
    with pytest.raises(ValueError, match="Monad quickstart"):
        quick.run(options, deps(chain))
    options = args(tmp_path, files, "status")
    options.directory = monad
    with pytest.raises(ValueError, match="Monad quickstart"):
        quick.run(options, deps(chain))
    from erebus_mcp.metropolis_setup import quickstart as monad_quick

    ready(tmp_path, files, chain)
    from types import SimpleNamespace

    with pytest.raises(ValueError, match="Starknet quickstart"):
        monad_quick.run(SimpleNamespace(action="status", directory=tmp_path / "sn", offline=True), None)


# --- server guardrails -------------------------------------------------------------------------


def _result(response) -> dict:
    return json.loads(response.content[0].text)


def _server(monkeypatch, path: Path):
    values: dict[str, str] = {}
    load_config_file(path, values)
    for key in [k for k in os.environ if k.startswith("EREBUS_")]:
        monkeypatch.delenv(key)
    for key, value in values.items():
        monkeypatch.setenv(key, value)
    from erebus_mcp.server import build_server

    return build_server()


def _op() -> str:
    return "op_" + os.urandom(32).hex()


def test_max_writes_refuses_the_write_after_the_cap(tmp_path, monkeypatch):
    for key, value in {"EREBUS_BACKEND": "mock", "AGENT_ADDRESS": "0xcap", "PROVING_SERVICE_URL": "http://x.invalid",
                       "EREBUS_SETTLEMENT_ROLE": "payer", "EREBUS_MOCK_STORE_PATH": str(tmp_path / "store.json"),
                       "EREBUS_MOCK_LATENCY_SECONDS": "0", "EREBUS_INTENT_STATE_DIR": str(tmp_path / "intent"),
                       "EREBUS_SPENDING_STATE_PATH": str(tmp_path / "spend.json"), "EREBUS_MAX_WRITES": "1"}.items():
        monkeypatch.setenv(key, value)
    from erebus_mcp.server import build_server

    server = build_server()
    first = _op()

    async def scenario():
        opened = _result(await server.call_tool("open_channel", {"operation_id": first, "counterparty": "0xother"}))
        assert opened["ok"], opened
        again = _result(await server.call_tool("open_channel", {"operation_id": first, "counterparty": "0xother"}))
        assert again["ok"], "retrying the same operation never counts twice"
        refused = _result(await server.call_tool("open_channel", {"operation_id": _op(), "counterparty": "0xthird"}))
        assert not refused["ok"] and refused["error"]["code"] == "SPENDING_LIMIT_EXCEEDED"
        assert "EREBUS_MAX_WRITES" in refused["error"]["message"]

    asyncio.run(scenario())
    assert not list((tmp_path / "intent" / "pending_operations").glob("*.json"))
    assert len(json.loads((tmp_path / "intent" / "write-budget.json").read_text())["operations"]) == 1


def test_canonical_network_flag_refuses_custom_networks(monkeypatch):
    monkeypatch.delenv("EREBUS_NETWORK", raising=False)
    monkeypatch.setenv("STARKNET_CHAIN_ID", "0x1234")
    monkeypatch.setenv("POOL_ADDRESS", "0x5678")
    assert config_module._network_settings()[0] == "custom"
    monkeypatch.setenv("EREBUS_REQUIRE_CANONICAL_NETWORK", "1")
    with pytest.raises(config_module.ConfigError, match="canonical"):
        config_module._network_settings()
    monkeypatch.setenv("EREBUS_NETWORK", "mainnet")
    with pytest.raises(config_module.ConfigError, match="conflicts"):
        config_module._network_settings()
    monkeypatch.delenv("STARKNET_CHAIN_ID")
    monkeypatch.delenv("POOL_ADDRESS")
    assert config_module._network_settings()[0] == "mainnet"


def test_invalid_max_writes_is_a_startup_error(monkeypatch):
    monkeypatch.setenv("EREBUS_MAX_WRITES", "0")
    with pytest.raises(config_module.ConfigError, match="EREBUS_MAX_WRITES"):
        config_module._max_writes()


# --- mock end to end ---------------------------------------------------------------------------


def test_mock_flow_follows_the_prompts_order(tmp_path, files, monkeypatch):
    monkeypatch.delenv("STARKSCAN_API_KEY", raising=False)
    options = quick.build_parser().parse_args(["quickstart", "up", "--mock", "--directory", str(tmp_path / "sn"),
                                               "--cli", str(files["cli"]), "--server-command", str(files["server"])])
    created = quick.run(options, quick.Deps())
    assert created["status"] == "ready" and created["backend"] == "mock"
    root = tmp_path / "sn"
    assert json.loads((root / quick.STATE).read_text())["network"] == "mock"
    values = env(tmp_path, "buyer")
    assert values["EREBUS_MOCK_STORE_PATH"] == str(root / "mock-store.json")
    assert Path(values["EREBUS_INTENT_STATE_DIR"]).is_relative_to(root)
    buyer_prompt = (root / "agents/buyer/prompt.md").read_text()
    seller_prompt = (root / "agents/seller/prompt.md").read_text()
    amounts = re.findall(r'amount="(\d+)"', buyer_prompt + seller_prompt)
    opening, ask = amounts[0], amounts[1]
    token = setup.STRK

    buyer = _server(monkeypatch, root / "buyer" / "mcp.env")
    seller = _server(monkeypatch, root / "seller" / "mcp.env")
    buyer_address, seller_address = quick.MOCK_ADDRESSES["buyer"], quick.MOCK_ADDRESSES["seller"]
    deadline = 4_000_000_000

    async def call(server, name, **arguments):
        return _result(await server.call_tool(name, arguments))

    async def scenario():
        assert (await call(buyer, "doctor"))["ok"]
        assert (await call(buyer, "get_note_balance"))["result"]["total"] == str(WEI // 2)
        buyer_handle = (await call(buyer, "open_channel", operation_id=_op(), counterparty=seller_address))[
            "result"]["channel_handle"]
        assert (await call(seller, "doctor"))["ok"]
        seller_handle = (await call(seller, "open_channel", operation_id=_op(), counterparty=buyer_address))[
            "result"]["channel_handle"]
        offered = await call(buyer, "propose_offer", operation_id=_op(), channel_handle=buyer_handle,
                             amount=opening, token=token, deadline=deadline, memo_hash="0x1")
        assert offered["ok"], offered
        seen = await call(seller, "wait_for_offers", channel_handle=seller_handle, expected_count=1,
                          timeout_seconds=5)
        assert not seen["result"]["timed_out"]
        countered = await call(seller, "counter_offer", operation_id=_op(), channel_handle=seller_handle,
                               reply_to=seen["result"]["offers"][0]["offer_id"], amount=ask, token=token,
                               deadline=deadline, memo_hash="0x1")
        assert countered["ok"], countered
        refused = await call(seller, "accept_and_settle", operation_id=_op(), channel_handle=seller_handle,
                             offer_id=countered["result"]["offer_id"])
        assert not refused["ok"] and "payee" in refused["error"]["message"]
        replies = await call(buyer, "wait_for_offers", channel_handle=buyer_handle, expected_count=2,
                             timeout_seconds=5)
        latest = replies["result"]["offers"][-1]
        assert latest["proposer"] == seller_address and int(latest["terms"]["amount"]) <= WEI // 5
        settled = await call(buyer, "accept_and_settle", operation_id=_op(), channel_handle=buyer_handle,
                             offer_id=latest["offer_id"])
        assert settled["ok"], settled
        final = await call(seller, "read_channel_state", channel_handle=seller_handle)
        assert final["result"]["settlements"]
        # The buyer's three-write budget is spent; the next write is refused before it is proved.
        over = await call(buyer, "propose_offer", operation_id=_op(), channel_handle=buyer_handle,
                          amount=opening, token=token, deadline=deadline, memo_hash="0x2")
        assert over["error"]["code"] == "SPENDING_LIMIT_EXCEEDED"

    asyncio.run(scenario())
    status = quick.run(quick.build_parser().parse_args(["quickstart", "status", "--directory", str(root)]),
                       quick.Deps())
    assert status["roles"]["buyer"]["writes_used"] == 3 and status["roles"]["seller"]["writes_used"] == 2
    renewed = quick.run(quick.build_parser().parse_args(["quickstart", "renew", "--directory", str(root),
                                                         "--deals", "1"]), quick.Deps())
    assert renewed["max_writes"] == {"buyer": 5, "seller": 3}
