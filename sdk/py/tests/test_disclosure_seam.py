"""Disclosure binding tests: paths cross the seam, private material does not."""

import json
import subprocess

import pytest

from erebus import DisclosureError, DisclosureSeam


def reply(monkeypatch, payload, *, returncode=0, stderr=""):
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, returncode, json.dumps(payload), stderr)

    monkeypatch.setattr(subprocess, "run", run)
    return calls


def test_undecodable_disclosure_output_is_redacted(monkeypatch):
    def run(*args, **kwargs):
        raise UnicodeDecodeError("utf-8", b"private-grant-marker\xff", 20, 21, "invalid byte")
    monkeypatch.setattr(subprocess, "run", run)
    with pytest.raises(DisclosureError) as error:
        DisclosureSeam(binary="local-rust").call("key_info", key_file="/operator/auditor.key")
    assert "private-grant-marker" not in str(error.value)
    assert "retain evidence" in str(error.value)


def test_only_paths_are_forwarded_and_no_key_is_read(monkeypatch):
    calls = reply(monkeypatch, {"status": "ok", "recipient_public_key": "ab" * 32})
    result = DisclosureSeam(binary="local-rust").call("key_info", key_file="/does/not/exist.key")
    assert result["recipient_public_key"] == "ab" * 32
    assert json.loads(calls[0][1]["input"]) == {"method": "key_info", "key_file": "/does/not/exist.key"}
    assert calls[0][0] == ["local-rust"]


@pytest.mark.parametrize("parameters", [{"key": "SECRET"}, {"key_file": "x", "method": "export"}])
def test_unknown_or_secret_request_fields_never_run(monkeypatch, parameters):
    calls = reply(monkeypatch, {"status": "ok"})
    with pytest.raises((DisclosureError, TypeError)):
        DisclosureSeam(binary="local-rust").call("key_info", **parameters)
    assert calls == []


@pytest.mark.parametrize("payload", [
    {"status": "ok", "private_key": "SECRET"},
    {"status": "ok", "issuer": {"private": "SECRET"}},
    {"status": "error", "error": "SECRET"},
    ["SECRET"],
    {"status": "ok", "methods": [{}]},
])
def test_private_or_malformed_response_is_not_exposed(monkeypatch, payload):
    reply(monkeypatch, payload)
    with pytest.raises(DisclosureError) as caught:
        DisclosureSeam(binary="local-rust").call("key_info", key_file="key.path")
    assert "SECRET" not in str(caught.value)


def test_offline_verification_cannot_claim_payment(monkeypatch):
    reply(monkeypatch, {"status": "ok", "agreement_verified": True,
                       "payment_verified": True, "delivery_verified": False})
    with pytest.raises(DisclosureError, match="claims"):
        DisclosureSeam(binary="local-rust").call(
            "verify_agreement", grant_file="grant.path", key_file="key.path", expected_issuer="public",
        )


def test_timeout_does_not_expose_request_or_raw_output(monkeypatch):
    def timeout(*args, **kwargs):
        raise subprocess.TimeoutExpired("SECRET", 1, output="SECRET")

    monkeypatch.setattr(subprocess, "run", timeout)
    with pytest.raises(DisclosureError) as caught:
        DisclosureSeam(binary="local-rust").call("key_info", key_file="key.path")
    assert "SECRET" not in str(caught.value)


def test_pending_payment_is_explicit_and_never_claims_payment(monkeypatch):
    reply(monkeypatch, {
        "status": "pending", "agreement_verified": True,
        "payment_verified": False, "delivery_verified": False,
        "ancestry_block": 42, "next_log_block": 10,
    }, returncode=2)
    response = DisclosureSeam(binary="local-rust").call(
        "verify_payment", grant_file="grant.path", key_file="key.path",
        expected_issuer="public", deployment={},
    )
    assert response["status"] == "pending"
    assert response["payment_verified"] is False


@pytest.mark.parametrize("mutation", [
    {"payment_verified": True}, {"delivery_verified": True},
    {"agreement_verified": False}, {"ancestry_block": True},
    {"ancestry_block": -1}, {"next_log_block": 2**64},
])
def test_pending_cannot_forge_verification_or_progress(monkeypatch, mutation):
    payload = {
        "status": "pending", "agreement_verified": True,
        "payment_verified": False, "delivery_verified": False, "ancestry_block": 42,
    }
    reply(monkeypatch, {**payload, **mutation}, returncode=2)
    with pytest.raises(DisclosureError):
        DisclosureSeam(binary="local-rust").call(
            "verify_payment", grant_file="grant.path", key_file="key.path",
            expected_issuer="public", deployment={},
        )


def test_pending_requires_its_exit_code_and_payment_method(monkeypatch):
    payload = {
        "status": "pending", "agreement_verified": True,
        "payment_verified": False, "delivery_verified": False, "ancestry_block": 42,
    }
    reply(monkeypatch, payload, returncode=0)
    with pytest.raises(DisclosureError):
        DisclosureSeam(binary="local-rust").call(
            "verify_payment", grant_file="grant.path", key_file="key.path",
            expected_issuer="public", deployment={},
        )
    reply(monkeypatch, payload, returncode=2)
    with pytest.raises(DisclosureError):
        DisclosureSeam(binary="local-rust").call(
            "verify_agreement", grant_file="grant.path", key_file="key.path", expected_issuer="public",
        )


SCOPED = {
    "status": "settled_before_checkpoint", "agreement_verified": True,
    "payment_verified": False, "delivery_verified": False, "checkpoint_block": 41,
    "next_action": "verify with a cache that starts earlier",
}


def verify_payment():
    return DisclosureSeam(binary="local-rust").call(
        "verify_payment", grant_file="grant.path", key_file="key.path",
        expected_issuer="public", deployment={},
    )


def test_settled_before_checkpoint_comes_back_without_a_payment_claim(monkeypatch):
    reply(monkeypatch, SCOPED, returncode=3)
    response = verify_payment()
    assert response["status"] == "settled_before_checkpoint"
    assert response["payment_verified"] is False


@pytest.mark.parametrize("mutation,returncode", [
    ({"payment_verified": True}, 3), ({"checkpoint_block": True}, 3),
    ({"checkpoint_block": -1}, 3), ({"next_action": 7}, 3), ({}, 0), ({}, 1),
    ({"ancestry_block": 1}, 3),
])
def test_settled_before_checkpoint_cannot_forge_a_claim_or_its_exit_code(monkeypatch, mutation, returncode):
    reply(monkeypatch, {**SCOPED, **mutation}, returncode=returncode)
    with pytest.raises(DisclosureError):
        verify_payment()


def test_checkpoint_scope_fields_only_accompany_their_status(monkeypatch):
    reply(monkeypatch, {"status": "ok", "agreement_verified": True, "payment_verified": True,
                        "delivery_verified": False, "checkpoint_block": 41}, returncode=0)
    with pytest.raises(DisclosureError, match="checkpoint scope"):
        verify_payment()


def test_checkpoint_capability_and_request_fields_cross_the_seam(monkeypatch):
    reply(monkeypatch, {"status": "ok", "protocol": 1, "methods": ["version", "verify_payment", "checkpoint"]})
    assert "checkpoint" in DisclosureSeam(binary="local-rust").call("version")["methods"]
    calls = reply(monkeypatch, {"status": "ok", "checkpoint": "created", "checkpoint_block": 41,
                                "history_from_block": 42, "payment_verified": False})
    result = DisclosureSeam(binary="local-rust").call("checkpoint", deployment={"cache_root": "/c"},
                                                      confirm_new_cache=True)
    assert result["checkpoint"] == "created"
    assert json.loads(calls[0][1]["input"]) == {"method": "checkpoint", "deployment": {"cache_root": "/c"},
                                                "confirm_new_cache": True}
