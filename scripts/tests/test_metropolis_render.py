"""Hosted startup must preserve durable state and require explicit private configuration."""

import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("render_startup", ROOT / "packaging/metropolis/render/run.py")
startup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(startup)


def test_relay_gateway_authenticates_health_only(tmp_path):
    environment = {"EREBUS_HOSTED_SERVICE": "relay", "EREBUS_RELAY_TOKEN": "a" * 64,
                   "EREBUS_RELAY_INSECURE": "1"}
    binary, gateway = startup.configuration(environment, tmp_path)
    assert binary == "erebus-relay"
    assert "EREBUS_RELAY_INSECURE" not in environment
    assert environment["EREBUS_RELAY_ROOT"] == str(tmp_path / "relay")
    assert "a" * 64 not in gateway
    assert "handle /healthz" in gateway and "admin off" in gateway


@pytest.mark.parametrize("mode,port", [("unknown", "10000"), ("relay", "0"), ("relay", "8080\n"), ("relay", "65536")])
def test_invalid_configuration_is_rejected(tmp_path, mode, port):
    with pytest.raises(ValueError):
        startup.configuration({"EREBUS_HOSTED_SERVICE": mode, "PORT": port}, tmp_path)


def test_access_requires_provisioning_and_never_creates_or_resets_state(tmp_path):
    with pytest.raises(ValueError, match="provision"):
        startup.configuration({"EREBUS_HOSTED_SERVICE": "access"}, tmp_path)
    assert list(tmp_path.iterdir()) == []


def access_config(root, **backend):
    config = {"port": 8081, "state_root": str(root / "access-state"), "evidence_root": str(root / "evidence"),
              "payload_file": str(root / "payload"), "backend": backend}
    path = root / "access.json"
    path.write_text(json.dumps(config))
    path.chmod(0o600)


@pytest.mark.parametrize("field", ["transaction_key_file", "signer_journal_root"])
def test_x402_signer_keys_and_journals_cannot_escape_persistent_storage(tmp_path, field):
    backend = {"mode": "x402_exact", "transaction_key_file": str(tmp_path / "gas.key"),
               "signer_journal_root": str(tmp_path / "signer")}
    backend[field] = str(tmp_path.parent / "ephemeral")
    access_config(tmp_path, **backend)
    with pytest.raises(ValueError, match="persistent disk"):
        startup.configuration({"EREBUS_HOSTED_SERVICE": "access"}, tmp_path)


@pytest.mark.parametrize("mutation", ["public", "symlink", "length"])
def test_x402_gateway_rejects_invalid_transaction_key(tmp_path, mutation):
    key = tmp_path / "gas.key"
    key.write_bytes(bytes(31 if mutation == "length" else 32))
    key.chmod(0o644 if mutation == "public" else 0o600)
    if mutation == "symlink":
        target = tmp_path / "real.key"
        key.rename(target)
        key.symlink_to(target)
    access_config(tmp_path, mode="x402_exact", transaction_key_file=str(key), signer_journal_root=str(tmp_path / "signer"))
    with pytest.raises(ValueError, match="owner-only regular 32-byte"):
        startup.configuration({"EREBUS_HOSTED_SERVICE": "access"}, tmp_path)


def test_x402_gateway_keeps_the_existing_key_and_journal_unchanged(tmp_path):
    key = tmp_path / "gas.key"
    key.write_bytes(bytes(range(32)))
    key.chmod(0o600)
    journal = tmp_path / "signer"
    journal.mkdir()
    record = journal / "existing-payment.json"
    record.write_text("retained payment fence")
    access_config(tmp_path, mode="x402_exact", transaction_key_file=str(key), signer_journal_root=str(journal))
    environment = {"EREBUS_HOSTED_SERVICE": "access"}
    binary, gateway = startup.configuration(environment, tmp_path)
    assert binary == "erebus-access-service"
    assert environment["EREBUS_ACCESS_CONFIG"] == str(tmp_path / "access.json")
    assert key.read_bytes() == bytes(range(32)) and record.read_text() == "retained payment fence"
    assert str(key) not in gateway
