"""Unit tests for env-var parsing that doesn't need a live client."""

from __future__ import annotations

from pathlib import Path

import pytest

from erebus_mcp.config import ConfigError, ServerConfig

REQUIRED_MOCK_ENV = {
    "AGENT_ADDRESS": "0xbuyer",
    "PROVING_SERVICE_URL": "http://unused.invalid",
    "EREBUS_SETTLEMENT_ROLE": "both",
}


@pytest.fixture(autouse=True)
def _base_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key, value in REQUIRED_MOCK_ENV.items():
        monkeypatch.setenv(key, value)


def test_spending_limits_default_to_empty() -> None:
    limits = ServerConfig.from_env().spending_limits
    cap = limits.for_token("0xtoken")
    assert cap.per_deal is None
    assert cap.daily is None


def test_spending_limits_parse_per_token_caps(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(
        "EREBUS_SPENDING_LIMITS",
        '{"0xToken": {"per_deal": "500", "daily": "2000"}}',
    )
    limits = ServerConfig.from_env().spending_limits

    # Lookup is case-insensitive; the map key is normalized at parse time.
    cap = limits.for_token("0xTOKEN")
    assert cap.per_deal == 500
    assert cap.daily == 2000

    # A token absent from the map is unlimited, not zero.
    other = limits.for_token("0xother")
    assert other.per_deal is None
    assert other.daily is None


def test_spending_limits_field_may_be_partial(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("EREBUS_SPENDING_LIMITS", '{"0xtoken": {"per_deal": "500"}}')
    cap = ServerConfig.from_env().spending_limits.for_token("0xtoken")
    assert cap.per_deal == 500
    assert cap.daily is None


@pytest.mark.parametrize(
    "raw",
    [
        "not json",
        "[]",
        '{"0xtoken": "not an object"}',
        '{"0xtoken": {"per_deal": 500}}',  # a JSON number, not a decimal string (F37 rule)
        '{"0xtoken": {"per_deal": "-500"}}',
        '{"0xtoken": {"per_deal": "0"}}',
        '{"0xtoken": {"per_deal": "not a number"}}',
    ],
)
def test_spending_limits_rejects_malformed_config(
    monkeypatch: pytest.MonkeyPatch, raw: str
) -> None:
    monkeypatch.setenv("EREBUS_SPENDING_LIMITS", raw)
    with pytest.raises(ConfigError):
        ServerConfig.from_env()


def test_spending_state_path_defaults_are_scoped_per_identity(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("AGENT_ADDRESS", "0xbuyer")
    buyer_path = ServerConfig.from_env().spending_state_path
    monkeypatch.setenv("AGENT_ADDRESS", "0xseller")
    seller_path = ServerConfig.from_env().spending_state_path

    assert buyer_path != seller_path


def test_spending_state_path_honors_explicit_override(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    override = tmp_path / "spend.json"
    monkeypatch.setenv("EREBUS_SPENDING_STATE_PATH", str(override))
    assert ServerConfig.from_env().spending_state_path == override


def test_intent_state_dir_defaults_are_scoped_per_identity(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("AGENT_ADDRESS", "0xbuyer")
    buyer_dir = ServerConfig.from_env().intent_state_dir
    monkeypatch.setenv("AGENT_ADDRESS", "0xseller")
    seller_dir = ServerConfig.from_env().intent_state_dir

    assert buyer_dir != seller_dir


def test_intent_state_dir_honors_explicit_override(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    override = tmp_path / "intents"
    monkeypatch.setenv("EREBUS_INTENT_STATE_DIR", str(override))
    assert ServerConfig.from_env().intent_state_dir == override


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


def _enable_seam(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, *, network: str
) -> None:
    for name in ("pool.key", "account.key", "erebus-cli"):
        (tmp_path / name).write_text("placeholder")
    values = {
        "EREBUS_BACKEND": "seam",
        "EREBUS_NETWORK": network,
        "EREBUS_CLI": str(tmp_path / "erebus-cli"),
        "STARKNET_RPC_URL": "http://unused.invalid",
        "POOL_KEY_FILE": str(tmp_path / "pool.key"),
        "ACCOUNT_KEY_FILE": str(tmp_path / "account.key"),
        "EREBUS_STATE_DIR": str(tmp_path / "state"),
        "TOKEN_ADDRESS": "0x2",
    }
    for key, value in values.items():
        monkeypatch.setenv(key, value)
    monkeypatch.delenv("STARKNET_CHAIN_ID", raising=False)
    monkeypatch.delenv("POOL_ADDRESS", raising=False)


@pytest.mark.parametrize(
    ("network", "chain_id", "pool_address"),
    [
        (
            "sepolia",
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
def test_named_network_supplies_canonical_chain_and_pool(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    network: str,
    chain_id: str,
    pool_address: str,
) -> None:
    _enable_seam(monkeypatch, tmp_path, network=network)

    seam = ServerConfig.from_env().seam

    assert seam is not None
    assert seam.network == network
    assert seam.chain_id == chain_id
    assert seam.pool_address == pool_address


@pytest.mark.parametrize(
    ("variable", "value"),
    [("STARKNET_CHAIN_ID", "0x1"), ("POOL_ADDRESS", "0x1")],
)
def test_named_network_rejects_conflicting_overrides(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    variable: str,
    value: str,
) -> None:
    _enable_seam(monkeypatch, tmp_path, network="mainnet")
    monkeypatch.setenv(variable, value)

    with pytest.raises(ConfigError, match=f"{variable} conflicts"):
        ServerConfig.from_env()


def test_named_network_rejects_unknown_name(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    _enable_seam(monkeypatch, tmp_path, network="testnet")

    with pytest.raises(ConfigError, match="must be one of sepolia, mainnet"):
        ServerConfig.from_env()


def test_legacy_canonical_pair_gets_friendly_network_label(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    _enable_seam(monkeypatch, tmp_path, network="sepolia")
    monkeypatch.delenv("EREBUS_NETWORK")
    monkeypatch.setenv("STARKNET_CHAIN_ID", "0x534e5f5345504f4c4941")
    monkeypatch.setenv(
        "POOL_ADDRESS",
        "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
    )

    seam = ServerConfig.from_env().seam

    assert seam is not None
    assert seam.network == "sepolia"
