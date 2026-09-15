#!/usr/bin/env python3
"""Build and exercise uv tool onboarding outside the checkout, without live writes."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    uv = shutil.which("uv")
    if not uv:
        raise SystemExit("uv is required")
    binary = repo / "sdk/rs/target/debug/erebus-cli"
    if not binary.exists():
        raise SystemExit("build erebus-cli before running the install check")
    with tempfile.TemporaryDirectory(
        prefix="erebus-installed-onboarding-"
    ) as temporary:
        root = Path(temporary)
        wheels = root / "wheels"
        wheels.mkdir()
        for relative in ("packaging/erebus-cli", "sdk/py", "mcp-server"):
            source = repo / relative
            destination = root / source.name
            destination.mkdir()
            shutil.copy2(source / "pyproject.toml", destination / "pyproject.toml")
            shutil.copytree(
                source / "src",
                destination / "src",
                ignore=shutil.ignore_patterns("__pycache__"),
            )
            if relative == "packaging/erebus-cli":
                shutil.copy2(source / "hatch_build.py", destination / "hatch_build.py")
                (destination / "bin").mkdir()
                shutil.copy2(binary, destination / "bin/erebus-cli")
            subprocess.run(
                [
                    uv,
                    "build",
                    "--wheel",
                    "--no-sources",
                    "--out-dir",
                    str(wheels),
                    str(destination),
                ],
                check=True,
                cwd=root,
            )

        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("EREBUS_", "STARKNET_", "STARKSCAN_", "PROVING_"))
            and key
            not in {
                "AGENT_ADDRESS",
                "POOL_ADDRESS",
                "POOL_KEY_FILE",
                "ACCOUNT_KEY_FILE",
                "TOKEN_ADDRESS",
                "PYTHONPATH",
                "VIRTUAL_ENV",
            }
        }
        environment.update(
            {
                "UV_TOOL_DIR": str(root / "tools"),
                "UV_TOOL_BIN_DIR": str(root / "bin"),
                "XDG_CONFIG_HOME": str(root / "config"),
                "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            }
        )
        subprocess.run(
            [
                uv,
                "tool",
                "install",
                "--find-links",
                str(wheels),
                "erebus-mcp-server==0.3.0",
            ],
            check=True,
            cwd=root,
            env=environment,
        )

        def run(*args: str, expected: int = 0) -> dict:
            completed = subprocess.run(
                [str(root / "bin/erebus-init"), *args],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            if completed.returncode != expected:
                raise AssertionError(
                    f"initializer exited {completed.returncode}: {completed.stdout} {completed.stderr}"
                )
            return json.loads(completed.stdout)

        assert run("--network", "mock", "--json")["status"] == "ready"
        missing = run(
            "--new",
            "--network",
            "sepolia",
            "--config",
            str(root / "buyer.env"),
            "--json",
            expected=2,
        )
        assert missing["status"] == "configuration_required"
        assert set(missing["missing"]) == {"--rpc-url", "--prover-url"}
        assert not shutil.which("sncast", path=environment["PATH"])
        assert not shutil.which("erebus-cli", path=environment["PATH"])

        class Rpc(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                request = json.loads(
                    self.rfile.read(int(self.headers["Content-Length"]))
                )
                body = {"jsonrpc": "2.0", "id": request["id"]}
                if request["method"] == "starknet_chainId":
                    body["result"] = "0x534e5f5345504f4c4941"
                elif request["method"] == "starknet_getClassHashAt":
                    body["error"] = {"code": 20, "message": "Contract not found"}
                elif request["method"] == "starknet_call":
                    contract = request["params"]["request"]["contract_address"]
                    body["result"] = (
                        ["0x0", "0x0"]
                        if int(contract, 16)
                        == int(
                            "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d",
                            16,
                        )
                        else ["0x0"]
                    )
                else:
                    body["error"] = {"code": -32601, "message": "Unexpected method"}
                encoded = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)

        server = ThreadingHTTPServer(("127.0.0.1", 0), Rpc)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            funded_config = str(root / "funded.env")
            paused = run(
                "--new",
                "--network",
                "sepolia",
                "--config",
                funded_config,
                "--rpc-url",
                f"http://127.0.0.1:{server.server_port}",
                "--prover-url",
                "http://unused.invalid",
                "--json",
                expected=2,
            )
            assert paused["status"] == "funding_required", paused
            resumed = run("--resume", "--config", funded_config, "--json", expected=2)
            assert resumed["status"] == "funding_required", resumed
            assert resumed["address"] == paused["address"]
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

        installed_python = root / "tools/erebus-mcp-server/bin/python"
        code = """
import asyncio, os, sys
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client
async def check():
    params = StdioServerParameters(command=sys.argv[1], args=["--config", sys.argv[2]], env=dict(os.environ))
    async with stdio_client(params) as (reader, writer):
        async with ClientSession(reader, writer) as session:
            await session.initialize()
            tools = await session.list_tools()
            assert len(tools.tools) == 13, len(tools.tools)
asyncio.run(check())
"""
        subprocess.run(
            [
                str(installed_python),
                "-c",
                code,
                str(root / "bin/erebus-mcp-server"),
                str(root / "config/erebus/mcp.env"),
            ],
            cwd=root,
            env=environment,
            check=True,
            timeout=30,
        )
        print(
            "Installed onboarding passed: bundled binary resolution, account creation, funding pause/resume, and 13 MCP tools without a checkout or sncast."
        )


if __name__ == "__main__":
    main()
