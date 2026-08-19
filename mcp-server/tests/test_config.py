"""Unit tests for env-var parsing that doesn't need a live client."""

from __future__ import annotations

import pytest

from erebus_mcp.config import ServerConfig

REQUIRED_MOCK_ENV = {
    "AGENT_ADDRESS": "0xbuyer",
    "PROVING_SERVICE_URL": "http://unused.invalid",
    "EREBUS_SETTLEMENT_ROLE": "both",
}


@pytest.fixture(autouse=True)
def _base_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key, value in REQUIRED_MOCK_ENV.items():
        monkeypatch.setenv(key, value)


def test_startup_doctor_defaults_on() -> None:
    assert ServerConfig.from_env().startup_doctor is True


@pytest.mark.parametrize("value", ["1", "true", "True", "yes", "YES", "on"])
def test_startup_doctor_skip_accepts_common_truthy_values(
    monkeypatch: pytest.MonkeyPatch, value: str
) -> None:
    monkeypatch.setenv("EREBUS_SKIP_STARTUP_DOCTOR", value)
    assert ServerConfig.from_env().startup_doctor is False


@pytest.mark.parametrize("value", ["0", "false", "no", "", "banana"])
def test_startup_doctor_skip_ignores_other_values(
    monkeypatch: pytest.MonkeyPatch, value: str
) -> None:
    monkeypatch.setenv("EREBUS_SKIP_STARTUP_DOCTOR", value)
    assert ServerConfig.from_env().startup_doctor is True
