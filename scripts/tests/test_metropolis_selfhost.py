"""Installed service configuration is literal data, never shell code."""

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / "scripts/metropolis-selfhost.sh"


def test_relayer_environment_preserves_spaces_and_metacharacters(tmp_path):
    root = tmp_path / "operator files"
    subprocess.run(["bash", str(LAUNCHER), "init", str(root)], check=True)
    (root / "relay.env").unlink()
    expected = str(root / "state with spaces;$not_expanded")
    (root / "relayer.env").write_text(f"EREBUS_STATE_ROOT={expected}\n")
    binary = tmp_path / "erebus-tx-relayer"
    binary.write_text('#!/usr/bin/env bash\n[[ "$EREBUS_STATE_ROOT" = "$EXPECTED" ]] || exit 9\nprintf \'{"status":"ok"}\\n\'\n')
    binary.chmod(0o755)
    environment = dict(os.environ, PATH=f"{tmp_path}:{os.environ['PATH']}", EXPECTED=expected)
    result = subprocess.run(["bash", str(LAUNCHER), "check", str(root)], env=environment, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert "relayer: configured" in result.stdout


def test_invalid_environment_line_fails_closed(tmp_path):
    subprocess.run(["bash", str(LAUNCHER), "init", str(tmp_path)], check=True)
    (tmp_path / "relay.env").unlink()
    (tmp_path / "relayer.env").write_text("export EREBUS_STATE_ROOT=/tmp\n")
    result = subprocess.run(["bash", str(LAUNCHER), "check", str(tmp_path)], capture_output=True, text=True)
    assert result.returncode != 0
    assert "invalid environment entry" in result.stderr


def test_relative_root_is_rejected(tmp_path):
    result = subprocess.run(["bash", str(LAUNCHER), "init", "relative"], cwd=tmp_path, capture_output=True, text=True)
    assert result.returncode == 2
    assert not (tmp_path / "relative").exists()
