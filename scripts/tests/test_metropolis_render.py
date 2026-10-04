"""Hosted startup must preserve durable state and require explicit private configuration."""

import importlib.util
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
