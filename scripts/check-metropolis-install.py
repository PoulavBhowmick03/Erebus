#!/usr/bin/env python3
"""Install only from a local Metropolis index; verify binaries and MCP outside the repo."""

from __future__ import annotations

import argparse
import functools
import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class RegistryHandler(SimpleHTTPRequestHandler):
    def log_message(self, *args) -> None:
        pass


def check(registry: Path, rehearse_x402: bool = False) -> dict:
    manifest = json.loads((registry / "release.json").read_text())
    if manifest["channel"] != "metropolis-testnet" or manifest["published"] is not False:
        raise ValueError("expected an unpublished Metropolis registry")
    for name, record in manifest["wheels"].items():
        if Path(name).name != name:
            raise ValueError("invalid registry inventory")
        with (registry / "wheels" / name).open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != record["sha256"]:
                raise ValueError("registry wheel hash mismatch")
    uv = shutil.which("uv")
    if not uv:
        raise ValueError("uv is required")
    with tempfile.TemporaryDirectory(prefix="metropolis-installed-") as temporary:
        # macOS temp dirs sit behind the /var -> /private/var symlink; compare resolved paths.
        root = Path(temporary).resolve()
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("EREBUS_", "UV_", "PIP_", "STARKNET_", "PROVING_"))
                       and key not in {"PYTHONPATH", "PYTHONHOME", "VIRTUAL_ENV"}}
        environment.update({"UV_CACHE_DIR": str(root / "uv-cache"), "XDG_CONFIG_HOME": str(root / "config")})
        subprocess.run([uv, "venv", "--python", sys.executable, str(root / "environment")],
                       env=environment, cwd=root, check=True)
        python = root / "environment/bin/python"
        server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(RegistryHandler, directory=str(registry)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            subprocess.run([uv, "pip", "install", "--python", str(python),
                            "--extra-index-url", f"http://127.0.0.1:{server.server_port}/simple/",
                            "--index-url", "https://pypi.org/simple", "--index-strategy", "first-index",
                            f"erebus-mcp-server=={manifest['version']}"], env=environment, cwd=root, check=True)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
        bins = root / "environment/bin"
        environment["PATH"] = str(bins) + os.pathsep + "/usr/bin:/bin:/usr/sbin:/sbin"
        imports = subprocess.check_output([str(python), "-I", "-c",
            "import erebus, erebus_mcp, importlib.metadata as m; "
            "print(erebus.__file__); print(erebus_mcp.__file__); "
            "print(m.version('erebus-sdk')); print(m.version('erebus-cli')); print(m.version('erebus-mcp-server'))"],
            env=environment, cwd=root, text=True).splitlines()
        if any(not Path(path).resolve().is_relative_to(root / "environment") for path in imports[:2]) or imports[2:] != [manifest["version"]] * 3:
            raise ValueError("imports or package versions escaped the isolated environment")
        for name, record in (manifest["binaries"] | manifest["launchers"]).items():
            if Path(name).name != name:
                raise ValueError("invalid binary inventory")
            binary = bins / name
            with binary.open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != record["sha256"]:
                    raise ValueError("installed native binary hash mismatch")
        subprocess.run([str(bins / "erebus-selfhost"), "init", str(root / "operator files")],
                       env=environment, cwd=root, timeout=20, check=True)
        operator = root / "operator files"
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        relay_config = operator / "relay.env"
        relay_config.write_text(relay_config.read_text().replace("EREBUS_RELAY_PORT=8080", f"EREBUS_RELAY_PORT={port}"))
        try:
            for command in ("up", "check"):
                subprocess.run([str(bins / "erebus-selfhost"), command, str(operator)],
                               env=environment, cwd=root, timeout=30, check=True)
        finally:
            subprocess.run([str(bins / "erebus-selfhost"), "down", str(operator)],
                           env=environment, cwd=root, timeout=20, check=True)
        for name in ("erebus-negotiate", "erebus-payment"):
            result = subprocess.run([str(bins / name)], input='{"method":"version"}',
                                    text=True, capture_output=True, cwd=root, env=environment, timeout=20, check=True)
            response = json.loads(result.stdout)
            if response.get("protocol_version") != 1 or response.get("status") != "ok" or result.stderr:
                raise ValueError("installed native protocol mismatch")
            if name == "erebus-payment" and response.get("modes") != ["public-bound", "shielded"]:
                raise ValueError("installed payment modes are incomplete")
        private = root / "operator"
        private.mkdir(mode=0o700)
        key = private / "buyer.key"
        key.write_bytes(bytes(32))
        key.chmod(0o600)
        environment.update({
            "EREBUS_BACKEND": "access", "EREBUS_ACCESS_EVIDENCE_DIR": str(private),
            "EREBUS_ACCESS_BUYER_KEY_FILE": str(key), "EREBUS_ACCESS_SERVICE_URL": "https://unused.invalid/v1/access",
            "EREBUS_ACCESS_SERVICE_ID": "ab" * 32, "EREBUS_ACCESS_CACHE": str(private / "cache"),
        })
        code = """
import asyncio, json, os, sys
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client
async def check():
    params = StdioServerParameters(command=sys.argv[1], env=dict(os.environ))
    async with stdio_client(params) as (reader, writer):
        async with ClientSession(reader, writer) as session:
            await session.initialize()
            names = {tool.name for tool in (await session.list_tools()).tools}
            tool, arguments, retry = sys.argv[2], json.loads(sys.argv[3]), sys.argv[4]
            assert names == {tool}, names
            result = await session.call_tool(tool, arguments)
            reply = result.structured_content or json.loads(result.content[0].text)
            assert reply['ok'] is False
            assert reply['error'][retry] is True
asyncio.run(check())
"""
        subprocess.run([str(python), "-I", "-c", code, str(bins / "erebus-mcp-server"), "retrieve_service_access",
                        json.dumps({"evidence_name": "../outside"}), "retry_without_payment"],
                       env=environment, cwd=root, timeout=45, check=True)
        # The combined mode must resolve the installed native binaries from PATH, and a seller
        # server must expose negotiation only.
        negotiation = private / "negotiation.json"
        negotiation.write_text(json.dumps({"role": "seller", "state_root": str(private / "state")}))
        negotiation.chmod(0o600)
        for name in [key for key in environment if key.startswith("EREBUS_")]:
            del environment[name]
        environment.update({"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": str(negotiation)})
        subprocess.run([str(python), "-I", "-c", code, str(bins / "erebus-mcp-server"), "negotiate_deal",
                        json.dumps({"operation_ref": "../outside"}), "retry_without_new_payment"],
                       env=environment, cwd=root, timeout=45, check=True)
        if rehearse_x402:
            checkout = Path(__file__).resolve().parents[1]
            cargo = shutil.which("cargo")
            if not cargo or not shutil.which("anvil"):
                raise ValueError("the local chain fixture requires cargo and Anvil")
            fixture_environment = dict(environment)
            fixture_environment.update({
                "PATH": str(bins) + os.pathsep + os.environ.get("PATH", ""),
                "EREBUS_TEST_INSTALLED_BIN_DIR": str(bins),
                "EREBUS_TEST_MCP_PYTHON": str(python),
            })
            subprocess.run([cargo, "test", "--locked", "--offline", "--manifest-path",
                            str(checkout / "sdk/shielded/Cargo.toml"), "--test", "negotiation_cli",
                            "negotiated_x402_settles_once_recovers_restart_and_audits_from_the_grant",
                            "--", "--ignored", "--nocapture"],
                           env=fixture_environment, cwd=root, timeout=600, check=True)
            # Negotiation and the initial paid request through separate installed MCP servers.
            fixture_environment["EREBUS_TEST_MCP_SERVER"] = str(bins / "erebus-mcp-server")
            subprocess.run([cargo, "test", "--locked", "--offline", "--manifest-path",
                            str(checkout / "sdk/shielded/Cargo.toml"), "--test", "negotiation_cli",
                            "two_mcp_agents_pay_over_x402_through_a_dropped_response_and_restarts",
                            "--", "--ignored", "--nocapture"],
                           env=fixture_environment, cwd=root, timeout=900, check=True)
    return {"status": "verified", "version": manifest["version"], "binaries": len(manifest["binaries"]),
            "source_imports": False, "live_payment": False, "published": False,
            "installed_x402_rehearsal": rehearse_x402}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--rehearse-x402", action="store_true", help="run the Anvil fixture with installed participant, access, MCP, and auditor binaries")
    args = parser.parse_args()
    print(json.dumps(check(args.registry.resolve(), args.rehearse_x402)))


if __name__ == "__main__":
    main()
