"""First-install and interruption scenarios. No live accounts or funds are used."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from erebus_mcp import setup
from erebus_mcp.onboarding import write_config_file


@pytest.fixture
def fake(monkeypatch, tmp_path):
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    state = SimpleNamespace(
        accounts=[],
        writes=[],
        registered="0x0",
        balance=100 * 10**18,
        allowance=0,
        findings=[],
        resume_result={},
        mature=True,
    )

    class Seam:
        def __init__(self, *args, **kwargs):
            pass

        def call(self, method, params):
            action, args = params["action"], params["args"]
            if action == "discover":
                return state.accounts
            if action in {"create", "import"}:
                root = Path(args["directory"])
                (root / "account.key").write_text("test-only")
                return {
                    "address": "0x123",
                    "account_key_file": str(root / "account.key"),
                }
            if action == "verify_signer":
                return {"verified": True}
            if action == "inspect":
                return {
                    "public_balance": str(state.balance),
                    "allowance": str(state.allowance),
                    "fee_per_write": str(10**18),
                    "registered_public_key": state.registered,
                    "gas_reserve_per_write": str(3 * 10**18),
                    "proving_block_lag": 10,
                }
            raise AssertionError(action)

        def generate_pool_key(self, path):
            Path(path).write_text("test-only")

        def reconcile(self):
            return state.findings

        def allowance(self):
            return {"allowance": str(state.allowance)}

        def resume_operation(self, operation_id):
            state.writes.append(("resume", operation_id))
            return state.resume_result

        def approve(self, operation_id, amount):
            state.writes.append(("approve", operation_id, amount))
            state.allowance = int(amount)
            return {"tx_hash": "0xa"}

        def shield(self, operation_id, amount):
            state.writes.append(("shield", operation_id, amount))
            state.registered = "0x456"
            return {"tx_hash": "0xb"}

        def doctor(self):
            return {"ready": True, "checks": []}

    monkeypatch.setattr(setup, "Seam", Seam)
    monkeypatch.setattr(setup, "mature", lambda *args: state.mature)
    return state


def arguments(tmp_path):
    return [
        "--config",
        str(tmp_path / "buyer.env"),
        "--network",
        "sepolia",
        "--new",
        "--rpc-url",
        "https://rpc.invalid",
        "--prover-url",
        "https://prover.invalid",
        "--json",
    ]


def result(capsys):
    return json.loads(capsys.readouterr().out)


def test_unattended_never_selects_existing_account_implicitly(fake, tmp_path, capsys):
    fake.accounts = [
        {
            "name": "alice",
            "network": "alpha-sepolia",
            "address": "0x123",
            "has_signer": True,
        }
    ]
    assert setup.main(["--json"]) == 2
    body = result(capsys)
    assert body["status"] == "selection_required"
    assert body["accounts"][0]["id"] == "sncast:alpha-sepolia:alice"
    assert not fake.writes


def test_funding_pause_preserves_identity_and_never_submits(fake, tmp_path, capsys):
    fake.balance = 0
    assert setup.main(arguments(tmp_path)) == 2
    body = result(capsys)
    assert body["status"] == "funding_required"
    assert body["address"] == "0x123"
    assert body["target_public_balance_strk"] == "20"
    config = tmp_path / "buyer.env"
    before = config.read_bytes()
    assert setup.main(["--config", str(config), "--resume", "--json"]) == 2
    assert result(capsys)["address"] == "0x123"
    assert config.read_bytes() == before
    assert not fake.writes
    assert (tmp_path / "buyer.env.setup.json").stat().st_mode & 0o777 == 0o600


def test_ready_retries_do_not_repeat_approval_or_shield(fake, tmp_path, capsys):
    assert setup.main(arguments(tmp_path) + ["--yes"]) == 0
    assert result(capsys)["status"] == "ready"
    writes = list(fake.writes)
    assert [r[0] for r in writes] == ["approve", "shield"]
    assert (
        setup.main(
            ["--config", str(tmp_path / "buyer.env"), "--resume", "--yes", "--json"]
        )
        == 0
    )
    assert result(capsys)["status"] == "ready"
    assert fake.writes == writes


def test_registered_address_without_pool_key_stops(fake, tmp_path, capsys):
    fake.registered = "0x456"
    assert setup.main(arguments(tmp_path) + ["--yes"]) == 2
    assert result(capsys)["status"] == "pool_key_required"
    assert not (tmp_path / "buyer.env.identity/pool.key").exists()
    assert not fake.writes


def test_no_shield_before_approval_maturity(fake, tmp_path, capsys):
    fake.mature = False
    assert setup.main(arguments(tmp_path) + ["--yes"]) == 2
    assert result(capsys)["status"] == "approval_pending"
    assert [r[0] for r in fake.writes] == ["approve"]
    fake.mature = True
    assert (
        setup.main(
            ["--config", str(tmp_path / "buyer.env"), "--resume", "--yes", "--json"]
        )
        == 0
    )
    assert result(capsys)["status"] == "ready"
    assert [r[0] for r in fake.writes] == ["approve", "shield"]


def test_existing_config_is_reused_without_overwriting_keys(fake, tmp_path, capsys):
    fake.registered = "0x456"
    pool, key = tmp_path / "pool.key", tmp_path / "account.key"
    pool.write_text("existing-pool")
    key.write_text("existing-account")
    config = tmp_path / "buyer.env"
    write_config_file(
        config,
        {
            "EREBUS_BACKEND": "seam",
            "EREBUS_NETWORK": "sepolia",
            "AGENT_ADDRESS": "0x123",
            "ACCOUNT_KEY_FILE": str(key),
            "POOL_KEY_FILE": str(pool),
            "EREBUS_STATE_DIR": str(tmp_path / "state"),
            "STARKNET_RPC_URL": "https://rpc.invalid",
            "PROVING_SERVICE_URL": "https://prover.invalid",
            "TOKEN_ADDRESS": setup.STRK,
            "EREBUS_SETTLEMENT_ROLE": "payee",
        },
    )
    before = config.read_bytes()
    assert (
        setup.main(
            [
                "--config",
                str(config),
                "--account",
                str(config),
                "--network",
                "sepolia",
                "--yes",
                "--json",
            ]
        )
        == 0
    )
    assert result(capsys)["status"] == "ready"
    assert config.read_bytes() == before
    assert pool.read_text() == "existing-pool"
    assert key.read_text() == "existing-account"
    assert all(r[0] != "shield" for r in fake.writes)


def test_mock_needs_no_wallet_or_endpoint(fake, tmp_path, capsys):
    assert (
        setup.main(
            ["--config", str(tmp_path / "demo.env"), "--network", "mock", "--json"]
        )
        == 0
    )
    assert result(capsys)["status"] == "ready"
    assert not fake.writes


@pytest.mark.parametrize(
    "value",
    [
        "-1",
        "NaN",
        "Infinity",
        "0.0000000000000000001",
        "1e100",
        "1e100000",
        "1.00000000000000000000000000000001",
    ],
)
def test_invalid_deposits_fail_before_any_account_creation(value):
    with pytest.raises(argparse.ArgumentTypeError):
        setup.amount(value)


def test_full_precision_deposit_does_not_round():
    assert (
        setup.amount("123456789012345678.123456789012345678")
        == 123456789012345678123456789012345678
    )


def test_lost_shield_response_recovers_original_operation(fake, tmp_path, capsys):
    assert setup.main(arguments(tmp_path) + ["--yes"]) == 0
    result(capsys)
    manifest = tmp_path / "buyer.env.setup.json"
    state = json.loads(manifest.read_text())
    state["done"].remove("shield")
    del state["operations"]["shield"]["result"]
    setup.save(manifest, state)
    original_id = state["operations"]["shield"]["id"]
    fake.findings = [
        {
            "operation_id": original_id,
            "outcome": "effect",
            "next_action": "commit_journal",
        }
    ]
    fake.resume_result = {"result": "already_complete", "transaction_hash": "0xb"}
    assert (
        setup.main(
            ["--config", str(tmp_path / "buyer.env"), "--resume", "--yes", "--json"]
        )
        == 0
    )
    assert result(capsys)["status"] == "ready"
    assert [r[0] for r in fake.writes].count("shield") == 1
    assert ("resume", original_id) in fake.writes


def test_unknown_operation_never_starts_another_write(fake, tmp_path, capsys):
    fake.mature = False
    assert setup.main(arguments(tmp_path) + ["--yes"]) == 2
    result(capsys)
    manifest = tmp_path / "buyer.env.setup.json"
    state = json.loads(manifest.read_text())
    state["done"].remove("approve")
    del state["operations"]["approve"]["result"]
    setup.save(manifest, state)
    original_id = state["operations"]["approve"]["id"]
    fake.findings = [
        {
            "operation_id": original_id,
            "outcome": "unknown",
            "next_action": "operator_attention",
        }
    ]
    assert (
        setup.main(
            ["--config", str(tmp_path / "buyer.env"), "--resume", "--yes", "--json"]
        )
        == 2
    )
    assert result(capsys)["status"] == "operation_pending"
    assert [r[0] for r in fake.writes] == ["approve"]


def test_rpc_identifies_itself(monkeypatch):
    """F43: `publicnode` answers 403 to urllib's default `Python-urllib/x.y` agent."""

    captured: dict[str, str] = {}

    class Response:
        def __enter__(self) -> "Response":
            return self

        def __exit__(self, *exc: object) -> bool:
            return False

        def read(self) -> bytes:
            return b'{"jsonrpc":"2.0","id":1,"result":"0x1"}'

    def fake_urlopen(request, timeout):
        captured.update({k.lower(): v for k, v in request.header_items()})
        return Response()

    monkeypatch.setattr(setup.urllib.request, "urlopen", fake_urlopen)

    assert setup.rpc("https://rpc.invalid", "starknet_blockNumber", []) == "0x1"
    assert captured.get("user-agent")
