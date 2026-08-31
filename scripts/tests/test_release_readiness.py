from __future__ import annotations

import importlib.util
import json
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "check-release-readiness.py"
SPEC = importlib.util.spec_from_file_location("release_readiness", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def _repo(tmp_path: Path, *, version: str = "0.2.0") -> Path:
    for relative, kind in MODULE.MANIFESTS:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if kind == "json":
            path.write_text(json.dumps({"version": version}))
        elif kind == "cargo":
            path.write_text(f'[package]\nversion = "{version}"\n')
        else:
            path.write_text(f'[project]\nversion = "{version}"\n')
    for relative, names in MODULE.LOCK_PACKAGES.items():
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "".join(
                f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
                for name in sorted(names)
            )
        )
    preamble = tmp_path / MODULE.PREAMBLE_PATH
    preamble.parent.mkdir(parents=True, exist_ok=True)
    preamble.write_text("unaudited\nnot the relationship\nLinux x86-64 and macOS arm64\n")
    return tmp_path


def _evidence(root: Path) -> dict:
    body = {
        "release": "0.2.0",
        "package_version": "0.2.0",
        "status": "passed",
        "network": "SN_MAIN",
        "source_commit": "a" * 40,
        "prover_version": "0.19.0-rc.2",
        "rpc_spec_version": "0.10.3-rc.2",
        "completed_at": "2026-08-31T12:00:00Z",
        "transactions": {
            name: {"transaction_hash": f"0x{index:x}"}
            for index, name in enumerate(MODULE.TRANSACTIONS, 1)
        },
        "checks": {name: True for name in MODULE.CHECKS},
    }
    path = root / MODULE.EVIDENCE_PATH
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(body))
    return body


def test_matching_versions_and_baseline_preamble_pass(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    assert MODULE.check(root, expected_version="0.2.0", tag="v0.2.0") == []


def test_manifest_version_drift_fails(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    (root / "sdk/ts/package.json").write_text('{"version":"0.1.0"}')
    errors = MODULE.check(root, expected_version="0.2.0")
    assert any("sdk/ts/package.json" in error and "0.1.0" in error for error in errors)


def test_lock_version_drift_fails(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    cargo_lock = root / "sdk/rs/Cargo.lock"
    cargo_lock.write_text(cargo_lock.read_text().replace('version = "0.2.0"', 'version = "0.1.0"'))
    errors = MODULE.check(root, expected_version="0.2.0")
    assert any("Cargo.lock" in error and "0.1.0" in error for error in errors)


def test_tag_must_match_version(tmp_path: Path) -> None:
    errors = MODULE.check(_repo(tmp_path), expected_version="0.2.0", tag="v0.2.1")
    assert any("tag 'v0.2.1'" in error for error in errors)


def test_mainnet_evidence_is_required(tmp_path: Path) -> None:
    errors = MODULE.check(_repo(tmp_path), expected_version="0.2.0", require_mainnet=True)
    assert any(str(MODULE.EVIDENCE_PATH) in error for error in errors)


def test_incomplete_or_secret_bearing_evidence_fails(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    body = _evidence(root)
    body["checks"]["recovery_verified"] = False
    body["rpc_url"] = "https://example.invalid/key"
    (root / MODULE.EVIDENCE_PATH).write_text(json.dumps(body))
    errors = MODULE.check(root, expected_version="0.2.0", require_mainnet=True)
    assert any("recovery_verified" in error for error in errors)
    assert any("rpc_url" in error for error in errors)


def test_placeholder_commit_and_transaction_hashes_fail(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    body = _evidence(root)
    body["source_commit"] = "0" * 40
    body["transactions"]["shield"]["transaction_hash"] = "0x0"
    (root / MODULE.EVIDENCE_PATH).write_text(json.dumps(body))
    errors = MODULE.check(root, expected_version="0.2.0", require_mainnet=True)
    assert any("source_commit" in error for error in errors)
    assert any("transactions.shield" in error for error in errors)


def test_complete_mainnet_evidence_and_updated_preamble_pass(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    _evidence(root)
    (root / MODULE.PREAMBLE_PATH).write_text(
        "unaudited\nnot the relationship\nLinux x86-64 and macOS arm64\n"
        f"Evidence: {MODULE.EVIDENCE_PATH}\n"
    )
    assert MODULE.check(root, expected_version="0.2.0", tag="v0.2.0", require_mainnet=True) == []


def test_repository_manifests_and_locks_are_aligned() -> None:
    root = Path(__file__).parents[2]
    assert MODULE.check(root) == []
