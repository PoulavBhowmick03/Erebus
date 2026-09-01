"""Subprocess binding tests.

These tests do not assert computed values. Such an assertion would mean that Python has
become another protocol implementation (``erebus/__init__.py``, CLAUDE.md).

The tests cover transport, response shape, and structured errors. Rust tests check computed
values against Cairo reference vectors.

These tests do not touch the chain. ``version`` and ``generate_pool_key`` are local. Other
calls fail argument parsing before an RPC call.
"""

from __future__ import annotations

import dataclasses
import json
import subprocess
from pathlib import Path

import pytest

from erebus import Network
from erebus._seam import ErebusError, Seam, SeamConfig, SeamUnavailable

REPO_ROOT = Path(__file__).resolve().parents[3]
CLI = REPO_ROOT / "sdk" / "rs" / "target" / "debug" / "erebus-cli"
OPERATION_ID = "op_" + "ab" * 32

pytestmark = pytest.mark.skipif(
    not CLI.exists(),
    reason="erebus-cli not built; run `cargo build --bin erebus-cli` in sdk/rs",
)


@pytest.fixture
def key_files(tmp_path: Path) -> tuple[Path, Path]:
    """Writes two operator keys to disk and returns their paths."""
    pool = tmp_path / "pool.key"
    pool.write_text("0x1234567890abcdef\n")
    pool.chmod(0o600)
    account = tmp_path / "account.key"
    account.write_text("0xfedcba0987654321\n")
    account.chmod(0o600)
    return pool, account


@pytest.fixture
def config(key_files: tuple[Path, Path], tmp_path: Path) -> SeamConfig:
    pool, account = key_files
    return SeamConfig(
        rpc_url="http://127.0.0.1:1",
        prover_url="http://127.0.0.1:1",
        pool_address="0x254a6b2",
        chain_id="0x534e5f5345504f4c4941",
        account_address="0xa11ce",
        pool_key_file=pool,
        account_key_file=account,
        state_dir=tmp_path / "state",
        token="0x7042",
    )


@pytest.fixture
def seam(config: SeamConfig) -> Seam:
    return Seam(config=config, binary=CLI)


@pytest.mark.parametrize(
    ("network", "chain_id", "pool_address"),
    [
        (
            Network.SEPOLIA,
            "0x534e5f5345504f4c4941",
            "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
        ),
        (
            "mainnet",
            "0x534e5f4d41494e",
            "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
        ),
    ],
)
def test_named_network_config_supplies_canonical_values(
    key_files: tuple[Path, Path],
    tmp_path: Path,
    network: Network | str,
    chain_id: str,
    pool_address: str,
) -> None:
    pool, account = key_files

    configured = SeamConfig.for_network(
        network,
        rpc_url="http://127.0.0.1:1",
        prover_url="http://127.0.0.1:1",
        account_address="0xa11ce",
        pool_key_file=pool,
        account_key_file=account,
        state_dir=tmp_path / "state",
        token="0x7042",
    )

    assert configured.chain_id == chain_id
    assert configured.pool_address == pool_address


# --- The call gets through ------------------------------------------------------


def test_version_round_trips(seam: Seam) -> None:
    result = seam.version()

    assert result["name"] == "erebus-sdk"
    assert result["protocol"] == 4
    assert result["default_wire_version"] == "v3"


def test_generate_pool_key_returns_a_path_and_a_public_key(seam: Seam, tmp_path: Path) -> None:
    result = seam.generate_pool_key(tmp_path / "fresh.key")

    # Check response shape only. Rust owns entropy generation and value checks.
    assert result["pool_key_file"] == str(tmp_path / "fresh.key")
    assert result["public_key"].startswith("0x")


def test_generate_pool_key_never_returns_the_private_value(seam: Seam, tmp_path: Path) -> None:
    """Checks that the secret stays on disk."""
    path = tmp_path / "secret.key"
    result = seam.generate_pool_key(path)
    private = path.read_text().strip()

    assert private not in json.dumps(result)


def test_balance_is_a_transport_only_configured_call(
    seam: Seam, monkeypatch: pytest.MonkeyPatch
) -> None:
    sent: dict[str, object] = {}

    def answer(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return subprocess.CompletedProcess(
            args=[str(CLI)],
            returncode=0,
            stdout='{"ok":true,"result":{"notes":["100"],"total":"100","pending":[]}}',
            stderr="",
        )

    monkeypatch.setattr(subprocess, "run", answer)
    result = seam.balance()

    assert sent["method"] == "balance"
    assert isinstance(sent["params"], dict)
    assert "config" in sent["params"]  # type: ignore[operator]
    assert set(result) == {"notes", "total", "pending"}


def test_doctor_is_a_transport_only_configured_call(
    seam: Seam, monkeypatch: pytest.MonkeyPatch
) -> None:
    sent: dict[str, object] = {}

    def answer(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return subprocess.CompletedProcess(
            args=[str(CLI)],
            returncode=0,
            stdout=(
                '{"ok":true,"result":{"ready":true,"checks":'
                '[{"name":"rpc_reachable","status":"pass","detail":"reached head 13095252"}],'
                '"repairs":[]}}'
            ),
            stderr="",
        )

    monkeypatch.setattr(subprocess, "run", answer)
    result = seam.doctor()

    assert sent["method"] == "doctor"
    assert isinstance(sent["params"], dict)
    assert "config" in sent["params"]  # type: ignore[operator]
    assert set(result) == {"ready", "checks", "repairs"}


def test_allowance_is_a_transport_only_configured_call(
    seam: Seam, monkeypatch: pytest.MonkeyPatch
) -> None:
    sent: dict[str, object] = {}

    def answer(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return subprocess.CompletedProcess(
            args=[str(CLI)],
            returncode=0,
            stdout='{"ok":true,"result":{"allowance":"2000000000000000000","fee_per_write":"0"}}',
            stderr="",
        )

    monkeypatch.setattr(subprocess, "run", answer)
    result = seam.allowance()

    assert sent["method"] == "allowance"
    assert isinstance(sent["params"], dict)
    assert "config" in sent["params"]  # type: ignore[operator]
    assert set(result) == {"allowance", "fee_per_write"}


def test_approve_sends_amount_and_returns_a_receipt(
    seam: Seam, monkeypatch: pytest.MonkeyPatch
) -> None:
    sent: dict[str, object] = {}

    def answer(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return subprocess.CompletedProcess(
            args=[str(CLI)],
            returncode=0,
            stdout='{"ok":true,"result":{"tx_hash":"0xabc","approved":"30000000000000000000"}}',
            stderr="",
        )

    monkeypatch.setattr(subprocess, "run", answer)
    result = seam.approve(OPERATION_ID, "5000000000000000000")

    assert sent["method"] == "approve"
    assert isinstance(sent["params"], dict)
    assert sent["params"]["operation_id"] == OPERATION_ID  # type: ignore[index]
    assert sent["params"]["amount"] == "5000000000000000000"  # type: ignore[index]
    assert set(result) == {"tx_hash", "approved"}


def test_deal_grant_carries_full_width_id_and_explicit_expiry(
    seam: Seam, monkeypatch: pytest.MonkeyPatch
) -> None:
    sent: dict[str, object] = {}
    capsule = {"version": 3, "ciphertext": [1, 2, 3]}

    def answer(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return subprocess.CompletedProcess(
            args=[str(CLI)],
            returncode=0,
            stdout=json.dumps(
                {
                    "ok": True,
                    "result": {
                        "channel_id": "ch_" + "ab" * 32,
                        "grantee": "0xa0d17",
                        "deal_id": "18446744073709551615",
                        "expires_at": 1_800_000_000,
                        "viewing_key": capsule,
                    },
                }
            ),
            stderr="",
        )

    monkeypatch.setattr(subprocess, "run", answer)
    result = seam.grant_viewing_key(
        "ch_" + "ab" * 32,
        "18446744073709551615",
        "0xa0d17",
        1_800_000_000,
    )

    params = sent["params"]
    assert isinstance(params, dict)
    assert params["deal_id"] == "18446744073709551615"
    assert params["expires_at"] == 1_800_000_000
    assert result["viewing_key"] == capsule


# --- Failures arrive as structure, not as crashes -------------------------------


def test_a_missing_key_file_raises_a_structured_error(config: SeamConfig) -> None:
    broken = dataclasses.replace(config, pool_key_file="/definitely/not/here")

    with pytest.raises(ErebusError) as caught:
        Seam(config=broken, binary=CLI).open_channel(OPERATION_ID, "0xb0b")

    assert caught.value.code == "IDENTITY_UNAVAILABLE"
    assert caught.value.retryable is False


def test_a_malformed_argument_raises_rather_than_returning_garbage(seam: Seam) -> None:
    with pytest.raises(ErebusError) as caught:
        seam.open_channel(OPERATION_ID, "not-a-felt")

    assert caught.value.code == "INVALID_REQUEST"


def test_the_retryable_flag_survives_the_seam(seam: Seam) -> None:
    """Checks that transport preserves the retry decision."""
    with pytest.raises(ErebusError) as caught:
        seam.call("no_such_method")

    assert isinstance(caught.value.retryable, bool)
    assert caught.value.code


def test_an_unknown_method_does_not_hang_or_crash(seam: Seam) -> None:
    with pytest.raises(ErebusError):
        seam.call("definitely_not_a_method", {"anything": 1})


def test_a_config_free_seam_refuses_calls_that_need_one() -> None:
    """A binding without configuration supports only local methods."""
    bare = Seam(binary=CLI)
    bare.version()

    with pytest.raises(SeamUnavailable):
        bare.open_channel(OPERATION_ID, "0xb0b")


# --- Broken install is a different failure than a protocol error ----------------


def test_a_missing_binary_is_distinguishable_from_a_protocol_error() -> None:
    with pytest.raises(SeamUnavailable):
        Seam(binary="/definitely/not/erebus-cli").version()


def test_non_json_output_is_reported_as_a_broken_install(tmp_path: Path) -> None:
    """Non-JSON output indicates a broken installation, not a protocol error."""
    fake = tmp_path / "erebus-cli"
    fake.write_text("#!/bin/sh\necho 'this is not json'\n")
    fake.chmod(0o755)

    with pytest.raises(SeamUnavailable):
        Seam(binary=fake).version()


# --- Custody --------------------------------------------------------------------


def test_neither_key_is_passed_through_python(seam: Seam, key_files: tuple[Path, Path]) -> None:
    """Requests contain key-file paths and never key contents."""
    pool, account = key_files
    sent: dict[str, object] = {}
    original = subprocess.run

    def capture(*args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return original(*args, **kwargs)  # type: ignore[arg-type]

    subprocess.run = capture  # type: ignore[assignment]
    try:
        with pytest.raises(ErebusError):
            seam.open_channel(OPERATION_ID, "not-a-felt")
    finally:
        subprocess.run = original  # type: ignore[assignment]

    rendered = json.dumps(sent)
    assert str(pool) in rendered, "the pool key path should be sent"
    assert str(account) in rendered, "the account key path should be sent"
    assert "1234567890abcdef" not in rendered, "pool key material reached the request body"
    assert "fedcba0987654321" not in rendered, "account key material reached the request body"
