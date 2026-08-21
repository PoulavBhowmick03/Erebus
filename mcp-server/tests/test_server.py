"""Tests for build_server()'s seam-backend startup checks: the version handshake and the
optional doctor pre-flight. No real erebus-cli involved — Seam itself is replaced."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from erebus_mcp.config import ConfigError

import erebus_mcp.server as server_module


def _install_fake_seam(monkeypatch: pytest.MonkeyPatch, *, protocol: int, ready: bool = True) -> dict:
    holder: dict[str, Any] = {}

    class _FakeSeam:
        def __init__(self, *, config: Any, binary: Any) -> None:
            self.doctor_called = False
            holder["instance"] = self

        def version(self) -> dict[str, Any]:
            return {"protocol": protocol}

        def doctor(self) -> dict[str, Any]:
            self.doctor_called = True
            return {"ready": ready, "checks": [], "repairs": []}

    monkeypatch.setattr("erebus.Seam", _FakeSeam)
    return holder


@pytest.fixture
def seam_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    pool_key = tmp_path / "pool.key"
    account_key = tmp_path / "account.key"
    cli = tmp_path / "erebus-cli"
    for f in (pool_key, account_key, cli):
        f.write_text("placeholder")

    env = {
        "AGENT_ADDRESS": "0xbuyer",
        "PROVING_SERVICE_URL": "http://unused.invalid",
        "EREBUS_SETTLEMENT_ROLE": "both",
        "EREBUS_BACKEND": "seam",
        "EREBUS_CLI": str(cli),
        "POOL_KEY_FILE": str(pool_key),
        "ACCOUNT_KEY_FILE": str(account_key),
        "STARKNET_RPC_URL": "http://unused.invalid",
        "POOL_ADDRESS": "0x1",
        "STARKNET_CHAIN_ID": "0x1",
        "EREBUS_STATE_DIR": str(tmp_path / "state"),
        "TOKEN_ADDRESS": "0x2",
    }
    for key, value in env.items():
        monkeypatch.setenv(key, value)


def test_matching_protocol_builds_and_runs_doctor(seam_env: None, monkeypatch: pytest.MonkeyPatch) -> None:
    holder = _install_fake_seam(monkeypatch, protocol=2)

    server_module.build_server()

    assert holder["instance"].doctor_called is True


def test_protocol_mismatch_is_a_clear_config_error(seam_env: None, monkeypatch: pytest.MonkeyPatch) -> None:
    _install_fake_seam(monkeypatch, protocol=999)

    with pytest.raises(ConfigError) as caught:
        server_module.build_server()

    message = str(caught.value)
    assert "999" in message
    assert "2" in message


def test_skip_startup_doctor_env_var_prevents_the_call(
    seam_env: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("EREBUS_SKIP_STARTUP_DOCTOR", "1")
    holder = _install_fake_seam(monkeypatch, protocol=2)

    server_module.build_server()

    assert holder["instance"].doctor_called is False


def test_a_not_ready_doctor_report_does_not_block_startup(
    seam_env: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_seam(monkeypatch, protocol=2, ready=False)

    # Must not raise: doctor is logged, not fatal.
    server_module.build_server()
