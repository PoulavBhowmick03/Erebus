"""Metropolis MCP: operator-fixed role and mode; only the buyer sees payment tools."""

import asyncio
import json
import sys
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus_mcp.config import ConfigError
from erebus_mcp.metropolis import MetropolisSettings

ROOT = Path(__file__).resolve().parents[2]
BINARIES = ROOT / "sdk/shielded/target/debug"


def operator_file(path: Path, data: dict, mode: int = 0o600) -> Path:
    path.write_text(json.dumps(data))
    path.chmod(mode)
    return path


def configure(monkeypatch, tmp_path, role="buyer", payment=None):
    state = tmp_path / "state"
    negotiation = operator_file(tmp_path / "negotiation.json", {"role": role, "state_root": str(state)})
    monkeypatch.setenv("EREBUS_NEGOTIATION_CONFIG", str(negotiation))
    if payment is not None:
        monkeypatch.setenv("EREBUS_PAYMENT_CONFIG", str(operator_file(tmp_path / "payment.json", {"state_root": str(state), **payment})))
    else:
        monkeypatch.delenv("EREBUS_PAYMENT_CONFIG", raising=False)
    return state


def test_buyer_mode_is_fixed_by_operator_configuration(monkeypatch, tmp_path):
    configure(monkeypatch, tmp_path, payment={"mode": "shielded"})
    settings = MetropolisSettings.from_env()
    assert (settings.role, settings.mode) == ("buyer", "shielded")


@pytest.mark.parametrize("payment", [{"mode": "free"}, {"mode": "shielded", "state_root": "/elsewhere"}])
def test_buyer_rejects_unknown_mode_or_foreign_state(monkeypatch, tmp_path, payment):
    configure(monkeypatch, tmp_path, payment=payment)
    with pytest.raises(ConfigError):
        MetropolisSettings.from_env()


def test_seller_cannot_be_given_payment_configuration(monkeypatch, tmp_path):
    configure(monkeypatch, tmp_path, role="seller", payment={"mode": "shielded"})
    with pytest.raises(ConfigError, match="seller"):
        MetropolisSettings.from_env()


def test_configuration_must_be_owner_only(monkeypatch, tmp_path):
    configure(monkeypatch, tmp_path, role="seller")
    (tmp_path / "negotiation.json").chmod(0o644)
    with pytest.raises(ConfigError, match="owner-only"):
        MetropolisSettings.from_env()


@pytest.mark.parametrize("value", ["0", "901", "1e2", "-5"])
def test_native_timeout_is_bounded(monkeypatch, tmp_path, value):
    configure(monkeypatch, tmp_path, role="seller")
    monkeypatch.setenv("EREBUS_NATIVE_TIMEOUT_SECONDS", value)
    with pytest.raises(ConfigError, match="timeout"):
        MetropolisSettings.from_env()


@pytest.mark.skipif(not (BINARIES / "erebus-payment").exists(), reason="build the shielded binaries")
@pytest.mark.parametrize("role,tools", [
    ("buyer", {"negotiate_deal", "check_settlement_funding", "settle_deal", "recover_deal"}),
    ("seller", {"negotiate_deal"}),
])
def test_stdio_tools_follow_the_configured_role(tmp_path, role, tools):
    state = tmp_path / "state"
    negotiation = operator_file(tmp_path / "negotiation.json", {"role": role, "state_root": str(state)})
    env = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": str(negotiation),
           "EREBUS_NEGOTIATION_CLI": str(BINARIES / "erebus-negotiate"), "EREBUS_PAYMENT_CLI": str(BINARIES / "erebus-payment")}
    if role == "buyer":
        env["EREBUS_PAYMENT_CONFIG"] = str(operator_file(tmp_path / "payment.json", {"mode": "shielded", "state_root": str(state)}))
    params = StdioServerParameters(command=sys.executable, args=["-m", "erebus_mcp.server"], cwd=str(ROOT), env=env)

    async def run():
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                assert {tool.name for tool in (await session.list_tools()).tools} == tools
                for name in tools:
                    response = await session.call_tool(name, {"operation_ref": "../outside"})
                    result = response.structured_content or json.loads(response.content[0].text)
                    assert result["ok"] is False
                    assert result["error"]["code"] == "METROPOLIS_UNAVAILABLE"
                    assert result["error"]["retry_without_new_payment"] is True
    asyncio.run(run())
