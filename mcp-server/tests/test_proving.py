"""Local-prover MCP has no chain, model, or hosted prover dependency."""

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus import LocalProvingError
from erebus_mcp.config import ConfigError
from erebus_mcp.proving import ProvingSettings

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "sdk/shielded/target/debug/erebus-local-prove"


@pytest.mark.parametrize("name", ["../secret", "/secret", "x/y", ".", "..", "x" * 97])
def test_witness_names_cannot_escape_the_operator_directory(tmp_path, name):
    settings = ProvingSettings(tmp_path, tmp_path / "manifest.json", "ab" * 32, tmp_path / "cache")
    with pytest.raises(LocalProvingError):
        settings.witness(name)


def test_configuration_rejects_insecure_directory(monkeypatch, tmp_path):
    monkeypatch.setenv("EREBUS_WITNESS_DIR", str(tmp_path))
    monkeypatch.setenv("EREBUS_ARTIFACT_MANIFEST", str(tmp_path / "manifest.json"))
    monkeypatch.setenv("EREBUS_ARTIFACT_MANIFEST_SHA256", "ab" * 32)
    monkeypatch.setenv("EREBUS_ARTIFACT_CACHE", str(tmp_path / "cache"))
    tmp_path.chmod(0o755)
    with pytest.raises(ConfigError, match="owner-only"):
        ProvingSettings.from_env()
    tmp_path.chmod(0o700)
    assert not ProvingSettings.from_env().allow_test_artifacts
    monkeypatch.setenv("EREBUS_ALLOW_TEST_ARTIFACTS", "yes")
    with pytest.raises(ConfigError, match="0 or 1"):
        ProvingSettings.from_env()


@pytest.mark.skipif(not BINARY.exists(), reason="build the local proving binary")
def test_stdio_proving_server_needs_no_starknet_configuration(tmp_path):
    tmp_path.chmod(0o700)
    params = StdioServerParameters(command=sys.executable, args=["-m", "erebus_mcp.server"], cwd=str(ROOT), env={
        "EREBUS_BACKEND": "local-prover", "EREBUS_WITNESS_DIR": str(tmp_path),
        "EREBUS_ARTIFACT_MANIFEST": str(tmp_path / "manifest.json"), "EREBUS_ARTIFACT_MANIFEST_SHA256": "ab" * 32,
        "EREBUS_ARTIFACT_CACHE": str(tmp_path / "cache"), "EREBUS_LOCAL_PROVER_CLI": str(BINARY),
    })
    async def run():
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                assert {tool.name for tool in (await session.list_tools()).tools} == {"prove_local_transition"}
                for name in ["../outside", "missing.json"]:
                    response = await session.call_tool("prove_local_transition", {"witness_name": name, "circuit": "deposit", "expected_public": ["1"] * 6})
                    result = response.structured_content or json.loads(response.content[0].text)
                    assert result["ok"] is False
                    assert result["error"]["code"] == "LOCAL_PROVING_UNAVAILABLE"
                    assert "calldata" not in result
    asyncio.run(run())


@pytest.mark.skipif(os.environ.get("EREBUS_M8_TEST_LOCAL_PROOF") != "1", reason="requires generated M5 test artifacts and a built local prover")
def test_real_stdio_mcp_downloads_artifacts_and_proves_without_a_checkout(tmp_path):
    import hashlib
    import shutil
    import subprocess
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    build = ROOT / "circuits/m5/build"
    wheels = tmp_path / "wheels"
    wheels.mkdir()
    requirements = tmp_path / "requirements.txt"
    def command(*args):
        result = subprocess.run(["uv", *args], cwd=ROOT, capture_output=True, text=True, check=False)
        assert result.returncode == 0, result.stderr
    command("export", "--frozen", "--all-packages", "--no-dev", "--no-emit-project", "--no-emit-workspace", "--output-file", str(requirements))
    for package in ("erebus-sdk", "erebus-mcp-server"):
        command("build", "--wheel", "--package", package, "--out-dir", str(wheels))
    environment = tmp_path / "environment"
    command("venv", "--python", sys.executable, str(environment))
    python = environment / "bin/python"
    command("pip", "install", "--python", str(python), "-r", str(requirements))
    command("pip", "install", "--python", str(python), "--no-deps", *(str(path) for path in wheels.glob("*.whl")))
    installed_binary = environment / "bin/erebus-local-prove"
    shutil.copy2(BINARY, installed_binary)
    # These wheel installs must not resolve production Python code through an editable checkout.
    installed = subprocess.run([str(python), "-c", "import erebus, erebus_mcp; print(erebus.__file__); print(erebus_mcp.__file__)"], cwd=tmp_path, capture_output=True, text=True, check=True)
    assert str(environment) in installed.stdout and str(ROOT) not in installed.stdout
    bodies = {}
    for circuit in ("deposit", "transfer", "withdraw"):
        for suffix in ("wasm", "r1cs", "zkey"):
            path = build / (f"{circuit}_js/{circuit}.wasm" if suffix == "wasm" else f"{circuit}.{suffix}")
            bodies[f"/{circuit}.{suffix}"] = path.read_bytes()
    calls = []

    class Artifacts(BaseHTTPRequestHandler):
        def do_GET(self):
            calls.append(self.path)
            body = bodies.get(self.path)
            if body is None:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    http = ThreadingHTTPServer(("127.0.0.1", 0), Artifacts)
    thread = threading.Thread(target=http.serve_forever, daemon=True)
    thread.start()
    try:
        witness = json.loads((build / "deposit-input.json").read_text())
        manifest = {
            "version": 1, "chain_id": int(witness["chainId"]),
            "settlement_contract": f'0x{int(witness["contractAddress"]):040x}',
            "verifier_version": int(witness["verifierVersion"]), "test_only": True,
            "circuits": [{"kind": kind, **{suffix: {
                "url": f"http://127.0.0.1:{http.server_port}/{kind}.{suffix}",
                "bytes": len(bodies[f"/{kind}.{suffix}"]),
                "sha256": hashlib.sha256(bodies[f"/{kind}.{suffix}"]).hexdigest(),
            } for suffix in ("wasm", "r1cs", "zkey")}} for kind in ("deposit", "transfer", "withdraw")],
        }
        tmp_path.chmod(0o700)
        manifest_bytes = json.dumps(manifest).encode()
        manifest_path = tmp_path / "manifest.json"
        manifest_path.write_bytes(manifest_bytes)
        witness_path = tmp_path / "deposit.json"
        witness_path.write_text(json.dumps(witness))
        witness_path.chmod(0o600)
        expected = [witness[name] for name in ("chainId", "contractAddress", "verifierVersion", "asset", "amount", "noteCommitment")]
        params = StdioServerParameters(command=str(python), args=["-m", "erebus_mcp.server"], cwd=str(tmp_path), env={
            "HOME": str(tmp_path),
            "EREBUS_BACKEND": "local-prover", "EREBUS_WITNESS_DIR": str(tmp_path),
            "EREBUS_ARTIFACT_MANIFEST": str(manifest_path), "EREBUS_ARTIFACT_MANIFEST_SHA256": hashlib.sha256(manifest_bytes).hexdigest(),
            "EREBUS_ARTIFACT_CACHE": str(tmp_path / "cache"), "EREBUS_LOCAL_PROVER_CLI": str(installed_binary),
            "EREBUS_ALLOW_TEST_ARTIFACTS": "1", "EREBUS_ALLOW_LOOPBACK_ARTIFACT_HTTP": "1",
        })

        async def run():
            async with stdio_client(params) as (read, write):
                async with ClientSession(read, write) as session:
                    await session.initialize()
                    for hits in (0, 3):
                        reply = await session.call_tool("prove_local_transition", {"witness_name": "deposit.json", "circuit": "deposit", "expected_public": expected})
                        result = reply.structured_content or json.loads(reply.content[0].text)
                        assert result["ok"] is True, result
                        assert result["result"]["calldata"][3] == expected
                        assert result["result"]["cache_hits"] == hits
                        assert "witness" not in result["result"]
        asyncio.run(run())
        assert sorted(calls) == ["/deposit.r1cs", "/deposit.wasm", "/deposit.zkey"]
    finally:
        http.shutdown()
        http.server_close()
        thread.join(timeout=5)
