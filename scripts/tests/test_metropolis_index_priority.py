"""Verify the documented uv commands against two conflicting local indexes."""

import hashlib
import os
import shlex
import shutil
import subprocess
import sys
import threading
import zipfile
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
INDEX = "https://poulavbhowmick03.github.io/erebus-metropolis/simple/"


def documented_install(path):
    lines = path.read_text().replace("\\\n", "").splitlines()
    line = next(line for line in lines if line.startswith("uv pip install") and "0.3.0.dev4" in line)
    return shlex.split(line)


@pytest.mark.parametrize("path", ["README.md", "docs/metropolis-install.md"])
def test_public_install_prioritizes_metropolis_over_pypi(path):
    command = documented_install(ROOT / path)
    assert command[command.index("--index") + 1] == INDEX
    assert command[command.index("--default-index") + 1] == "https://pypi.org/simple"
    assert command[command.index("--index-strategy") + 1] == "first-index"
    assert "--no-config" in command
    assert "--extra-index-url" not in command and "--index-url" not in command


def wheel_index(root, source):
    page = root / "simple/erebus-sdk"
    page.mkdir(parents=True)
    name = "erebus_sdk-0.3.0.dev4-py3-none-any.whl"
    wheel = page / name
    info = "erebus_sdk-0.3.0.dev4.dist-info"
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("registry_priority_probe.py", f"source = {source!r}\n")
        archive.writestr(f"{info}/METADATA", "Metadata-Version: 2.1\nName: erebus-sdk\nVersion: 0.3.0.dev4\n")
        archive.writestr(f"{info}/WHEEL", "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n")
        archive.writestr(f"{info}/RECORD", "")
    digest = hashlib.sha256(wheel.read_bytes()).hexdigest()
    (page / "index.html").write_text(f'<a href="{name}#sha256={digest}">{name}</a>')


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def test_conflicting_package_on_default_index_cannot_shadow_metropolis(tmp_path):
    uv = shutil.which("uv")
    if not uv:
        pytest.skip("uv is required for the installed index-priority regression")
    servers, threads = [], []
    for source in ("metropolis", "untrusted-default"):
        root = tmp_path / source
        wheel_index(root, source)
        server = ThreadingHTTPServer(("127.0.0.1", 0), partial(QuietHandler, directory=str(root)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        servers.append(server)
        threads.append(thread)
        thread.start()
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("UV_", "PIP_")) and key not in {"PYTHONPATH", "PYTHONHOME", "VIRTUAL_ENV"}}
    try:
        venv = tmp_path / "environment"
        subprocess.run([uv, "venv", "--no-config", "--python", sys.executable, str(venv)],
                       env=environment, cwd=tmp_path, check=True, capture_output=True, timeout=30)
        python = venv / "bin/python"
        # Use the flags from the README, not a second hand-maintained example.
        command = documented_install(ROOT / "README.md")
        command[0] = uv
        command[command.index("--python") + 1] = str(python)
        command[command.index("--index") + 1] = f"http://127.0.0.1:{servers[0].server_port}/simple/"
        command[command.index("--default-index") + 1] = f"http://127.0.0.1:{servers[1].server_port}/simple/"
        command[-1] = "erebus-sdk==0.3.0.dev4"
        subprocess.run(command + ["--no-cache", "--no-deps"], env=environment, cwd=tmp_path,
                       check=True, capture_output=True, timeout=30)
        source = subprocess.check_output([str(python), "-I", "-c", "import registry_priority_probe; print(registry_priority_probe.source)"],
                                         env=environment, cwd=tmp_path, text=True, timeout=10)
        assert source.strip() == "metropolis"
    finally:
        for server, thread in zip(servers, threads):
            server.shutdown()
            server.server_close()
            thread.join()
