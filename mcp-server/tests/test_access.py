"""Access MCP confines paths and never exposes a payment tool."""

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus import AccessError
from erebus_mcp.access import AccessSettings
from erebus_mcp.config import ConfigError

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "sdk/shielded/target/debug/erebus-access"


@pytest.mark.parametrize("name", ["../secret", "/secret", "x/y", ".", "..", "x" * 97])
def test_evidence_names_cannot_escape_the_operator_directory(tmp_path, name):
    settings = AccessSettings(tmp_path, tmp_path / "buyer.key", "https://example.com/v1/access", "ab" * 32, tmp_path / "cache")
    with pytest.raises(AccessError):
        settings.evidence(name)


def test_configuration_rejects_insecure_paths(monkeypatch, tmp_path):
    key = tmp_path / "buyer.key"
    key.write_bytes(b"x" * 32)
    key.chmod(0o600)
    monkeypatch.setenv("EREBUS_ACCESS_EVIDENCE_DIR", str(tmp_path))
    monkeypatch.setenv("EREBUS_ACCESS_BUYER_KEY_FILE", str(key))
    monkeypatch.setenv("EREBUS_ACCESS_SERVICE_URL", "https://example.com/v1/access")
    monkeypatch.setenv("EREBUS_ACCESS_SERVICE_ID", "ab" * 32)
    monkeypatch.setenv("EREBUS_ACCESS_CACHE", str(tmp_path / "cache"))
    tmp_path.chmod(0o755)
    with pytest.raises(ConfigError, match="owner-only"):
        AccessSettings.from_env()
    tmp_path.chmod(0o700)
    assert not AccessSettings.from_env().allow_loopback_http
    key.chmod(0o644)
    with pytest.raises(ConfigError, match="owner-only"):
        AccessSettings.from_env()
    key.chmod(0o600)
    monkeypatch.setenv("EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP", "yes")
    with pytest.raises(ConfigError, match="0 or 1"):
        AccessSettings.from_env()


@pytest.mark.skipif(not BINARY.exists(), reason="build the access binary")
def test_stdio_access_has_no_starknet_prover_or_payment_configuration(tmp_path):
    tmp_path.chmod(0o700)
    key = tmp_path / "buyer.key"
    key.write_bytes(b"x" * 32)
    key.chmod(0o600)
    params = StdioServerParameters(command=sys.executable, args=["-m", "erebus_mcp.server"], cwd=str(ROOT), env={
        "EREBUS_BACKEND": "access", "EREBUS_ACCESS_EVIDENCE_DIR": str(tmp_path),
        "EREBUS_ACCESS_BUYER_KEY_FILE": str(key), "EREBUS_ACCESS_SERVICE_URL": "https://example.com/v1/access",
        "EREBUS_ACCESS_SERVICE_ID": "ab" * 32, "EREBUS_ACCESS_CACHE": str(tmp_path / "cache"), "EREBUS_ACCESS_CLI": str(BINARY),
    })
    async def run():
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                assert {tool.name for tool in (await session.list_tools()).tools} == {"retrieve_service_access"}
                for name in ["../outside", "missing.evidence"]:
                    response = await session.call_tool("retrieve_service_access", {"evidence_name": name})
                    result = response.structured_content or json.loads(response.content[0].text)
                    assert result["ok"] is False
                    assert result["error"]["code"] == "ACCESS_UNAVAILABLE"
                    assert result["error"]["retry_without_payment"] is True
                    assert "payload_hex" not in result
    asyncio.run(run())


@pytest.mark.skipif(os.environ.get("EREBUS_M8_TEST_ACCESS_INSTALL") != "1", reason="requires built access binaries, Anvil, and EVM artifacts")
def test_installed_wheels_retrieve_funded_access_without_source_imports(tmp_path):
    import shutil
    import subprocess

    wheels = tmp_path / "wheels"
    wheels.mkdir()
    requirements = tmp_path / "requirements.txt"
    def command(*args):
        output = subprocess.run(["uv", *args], cwd=ROOT, capture_output=True, text=True, check=False)
        assert output.returncode == 0, output.stderr
    command("export", "--frozen", "--all-packages", "--no-dev", "--no-emit-project", "--no-emit-workspace", "--output-file", str(requirements))
    for package in ("erebus-sdk", "erebus-mcp-server"):
        command("build", "--wheel", "--package", package, "--out-dir", str(wheels))
    environment = tmp_path / "environment"
    command("venv", "--python", sys.executable, str(environment))
    python = environment / "bin/python"
    command("pip", "install", "--python", str(python), "-r", str(requirements))
    command("pip", "install", "--python", str(python), "--no-deps", *(str(path) for path in wheels.glob("*.whl")))
    installed_binary = environment / "bin/erebus-access"
    shutil.copy2(BINARY, installed_binary)
    imports = subprocess.run([str(python), "-c", "import erebus, erebus_mcp.access; print(erebus.__file__); print(erebus_mcp.access.__file__)"], cwd=tmp_path, capture_output=True, text=True, check=True)
    for path in imports.stdout.splitlines():
        assert Path(path).is_relative_to(environment)
        assert not Path(path).is_relative_to(ROOT)
    variables = {**os.environ, "EREBUS_TEST_MCP_PYTHON": str(python), "EREBUS_TEST_ACCESS_BIN": str(installed_binary)}
    output = subprocess.run(["cargo", "test", "--locked", "--test", "access_http", "--", "--include-ignored"], cwd=ROOT / "sdk/shielded", env=variables, capture_output=True, text=True, timeout=180, check=False)
    assert output.returncode == 0, output.stdout + output.stderr
    assert "1 passed" in output.stdout
