"""Real stdio MCP calls over the built Rust disclosure binary, without a wallet or prover."""

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus_mcp.config import ConfigError
from erebus_mcp.disclosure import DisclosureSettings

ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get("EREBUS_TEST_DISCLOSURE_BIN", ROOT / "sdk/evm/target/debug/erebus-disclosure"))


def test_settings_reject_insecure_directories_and_partial_issuer_configuration(monkeypatch, tmp_path):
    monkeypatch.setenv("EREBUS_DISCLOSURE_DIR", str(tmp_path))
    tmp_path.chmod(0o755)
    with pytest.raises(ConfigError, match="owner-only"):
        DisclosureSettings.from_env()
    tmp_path.chmod(0o700)
    monkeypatch.setenv("EREBUS_DISCLOSURE_STATE_ROOT", str(tmp_path / "state"))
    with pytest.raises(ConfigError, match="four"):
        DisclosureSettings.from_env()


@pytest.mark.parametrize("name", ["../key", "/key", "nested/key", ".", "..", "x" * 97])
def test_artifact_names_cannot_escape_the_operator_directory(tmp_path, name):
    settings = DisclosureSettings(artifacts=tmp_path, key_file=tmp_path / "auditor.key")
    from erebus import DisclosureError
    with pytest.raises(DisclosureError):
        settings.artifact(name)


def test_symlink_directory_is_rejected(monkeypatch, tmp_path):
    actual = tmp_path / "actual"
    actual.mkdir(mode=0o700)
    link = tmp_path / "alias"
    link.symlink_to(actual, target_is_directory=True)
    monkeypatch.setenv("EREBUS_DISCLOSURE_DIR", str(link))
    with pytest.raises(ConfigError, match="real owner-only"):
        DisclosureSettings.from_env()


@pytest.mark.skipif(not BINARY.exists(), reason="build erebus-disclosure to run real MCP tests")
def test_auditor_stdio_server_uses_rust_keys_without_starknet_or_prover_settings(tmp_path):
    tmp_path.chmod(0o700)
    parameters = StdioServerParameters(
        command=sys.executable, args=["-m", "erebus_mcp.server"], cwd=str(ROOT),
        env={"EREBUS_BACKEND": "disclosure", "EREBUS_DISCLOSURE_DIR": str(tmp_path),
             "EREBUS_DISCLOSURE_CLI": str(BINARY)},
    )

    async def run():
        async with stdio_client(parameters) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                tools = {tool.name for tool in (await session.list_tools()).tools}
                assert tools == {"create_disclosure_key", "disclosure_key_info", "verify_disclosed_agreement"}
                def result(response):
                    return response.structured_content or json.loads(response.content[0].text)
                created = result(await session.call_tool("create_disclosure_key", {}))
                assert created["ok"] is True
                key = created["result"]["recipient_public_key"]
                restored = result(await session.call_tool("disclosure_key_info", {}))
                assert restored["result"]["recipient_public_key"] == key
                duplicate = result(await session.call_tool("create_disclosure_key", {}))
                assert duplicate["ok"] is False
                missing = result(await session.call_tool("verify_disclosed_agreement", {
                    "grant_name": "missing.grant", "expected_issuer": "0x" + "11" * 20,
                }))
                assert missing["ok"] is False
                escape = result(await session.call_tool("verify_disclosed_agreement", {
                    "grant_name": "../outside.grant", "expected_issuer": "0x" + "11" * 20,
                }))
                assert escape["ok"] is False
                assert escape["error"]["code"] == "INVALID_ARTIFACT"
                assert "private_key" not in json.dumps(created)
    asyncio.run(run())
