"""Path-only retrieval and strict separation of resource and payment evidence."""

import json
import subprocess

import pytest

from erebus import AccessError, AccessSeam


def request():
    return dict(evidence_file="deal.evidence", buyer_key_file="buyer.key", service_url="https://example.com/v1/access",
                service_id="ab" * 32, cache_root="cache")


def response():
    return {"status": "retrieved", "result": {"deal_commitment": "ab" * 32, "issuance_id": "cd" * 32,
        "resource_sha256": "ef" * 32, "resource_bytes": 123, "resource_file": "/private/cache/resource",
        "cached": False, "seller_reported_payment_finalized": True, "payment_verified": False,
        "resource_verified": True, "delivery_verified": False}}


def fake(monkeypatch, value, code=0, stderr=""):
    seen = []
    def run(args, **kwargs):
        seen.append((args, kwargs))
        return subprocess.CompletedProcess(args, code, json.dumps(value), stderr)
    monkeypatch.setattr(subprocess, "run", run)
    return seen


def test_paths_only_and_no_second_payment_request(monkeypatch):
    seen = fake(monkeypatch, response())
    result = AccessSeam(binary="access").retrieve(**request())
    assert result == response()
    args, params = seen[0]
    assert args == ["access"]
    assert json.loads(params["input"]) == {**request(), "method": "retrieve", "allow_loopback_http": False}
    assert result["result"]["resource_verified"] and not result["result"]["payment_verified"]


def test_pending_preserves_uncertainty(monkeypatch):
    pending = {"status": "pending", "retry_without_payment": True, "payment_verified": False,
               "resource_verified": False, "delivery_verified": False}
    fake(monkeypatch, pending, 2)
    assert AccessSeam(binary="access").retrieve(**request()) == pending
    pending["payment_verified"] = True
    fake(monkeypatch, pending, 2)
    with pytest.raises(AccessError):
        AccessSeam(binary="access").retrieve(**request())


def test_paid_but_undelivered_is_a_seller_claim_not_chain_evidence(monkeypatch):
    report = {"status":"paid_but_undelivered", "seller_reported_payment_finalized":True,
              "retry_without_payment":True, "payment_verified":False, "resource_verified":False, "delivery_verified":False}
    fake(monkeypatch, report, 2)
    assert AccessSeam(binary="access").retrieve(**request()) == report
    report["payment_verified"] = True
    fake(monkeypatch, report, 2)
    with pytest.raises(AccessError):
        AccessSeam(binary="access").retrieve(**request())


def test_protocol_without_keys_or_service(monkeypatch):
    seen = fake(monkeypatch, {"status": "ok", "protocol": 1, "methods": ["retrieve"]})
    assert AccessSeam(binary="access").version()["protocol"] == 1
    assert seen[0][0] == ["access", "--version"]
    assert seen[0][1]["input"] is None


@pytest.mark.parametrize("field,value", [("evidence_file", ""), ("buyer_key_file", {}), ("service_id", "0" * 64), ("service_id", "AB" * 32), ("allow_loopback_http", 1)])
def test_bad_input_never_starts_a_process(monkeypatch, field, value):
    seen = fake(monkeypatch, response())
    params = request()
    params[field] = value
    with pytest.raises(AccessError):
        AccessSeam(binary="access").retrieve(**params)
    assert not seen


@pytest.mark.parametrize("field,value", [("resource_bytes", True), ("resource_bytes", 1024 * 1024 + 1), ("resource_file", ""), ("resource_sha256", "not-a-hash"), ("cached", 1), ("payment_verified", True), ("delivery_verified", True), ("resource_verified", False), ("seller_reported_payment_finalized", False), ("payload_hex", "private-dataset")])
def test_malformed_or_private_metadata_is_rejected(monkeypatch, field, value):
    output = response()
    output["result"][field] = value
    fake(monkeypatch, output)
    with pytest.raises(AccessError):
        AccessSeam(binary="access").retrieve(**request())


@pytest.mark.parametrize("code,stderr", [(1, ""), (0, "private-provider-error")])
def test_process_errors_are_redacted(monkeypatch, code, stderr):
    fake(monkeypatch, {"status": "error", "error": "private-provider-error"}, code, stderr)
    with pytest.raises(AccessError) as error:
        AccessSeam(binary="access").retrieve(**request())
    assert "private-provider-error" not in str(error.value)
