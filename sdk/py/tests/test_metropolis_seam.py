"""Path-only negotiation and payment; agents supply operation IDs, operators fix everything else."""

import json
import subprocess

import pytest

from erebus import MetropolisError, MetropolisSeam

OPERATION = "ab" * 32


def seam(role="buyer", mode="shielded"):
    return MetropolisSeam(negotiation_config="/operator/negotiation.json", state_root="/operator/state", role=role,
                          payment_config="/operator/payment.json" if role == "buyer" else None,
                          mode=mode if role == "buyer" else None,
                          negotiation_binary="negotiate", payment_binary="payment", timeout=30)


def test_undecodable_payment_output_requires_observation_without_retry(monkeypatch):
    calls = []
    def run(*args, **kwargs):
        calls.append(args)
        raise UnicodeDecodeError("utf-8", b"private-payment-marker\xff", 22, 23, "invalid byte")
    monkeypatch.setattr(subprocess, "run", run)
    with pytest.raises(MetropolisError) as error:
        seam().payment(OPERATION, method="settle")
    assert len(calls) == 1
    assert "private-payment-marker" not in str(error.value)
    assert "recover by observation" in str(error.value)


def authorized():
    return {"protocol_version": 1, "status": "authorized", "operation_ref": OPERATION, "deal_id": "12" * 16,
            "deal_commitment": "cd" * 32, "deal_nullifier": "ef" * 32, "agreement_verified": True,
            "evidence_file": f"/operator/state/agent/{OPERATION}.0.tx", "payment_verified": False, "delivery_verified": False,
            "measurements_ms": {"negotiation": 850}}


def settled():
    return {"status": "ok", "mode": "shielded", "agreement_verified": True, "payment_verified": True,
            "delivery_verified": False, "operation_ref": OPERATION, "deal_commitment": "cd" * 32,
            "deal_nullifier": "ef" * 32, "submitted_this_call": True, "retry_without_new_payment": True,
            "stage": "Finalized", "winning_commitment": "cd" * 32, "broadcast_attempts": 1,
            "transaction_hash": "0x" + "aa" * 32, "measurements_ms": {"proof_preparation": 1200, "submission": 30}}


def fake(monkeypatch, *values, code=0, stderr=""):
    seen, queue = [], list(values)
    def run(args, **kwargs):
        seen.append((args, kwargs))
        return subprocess.CompletedProcess(args, code, json.dumps(queue.pop(0)), stderr)
    monkeypatch.setattr(subprocess, "run", run)
    return seen


def test_negotiation_sends_only_paths_and_the_operation(monkeypatch):
    seen = fake(monkeypatch, authorized())
    assert seam().negotiate(OPERATION) == authorized()
    args, params = seen[0]
    assert args == ["negotiate"]
    assert json.loads(params["input"]) == {"method": "negotiate", "config_file": "/operator/negotiation.json",
                                           "operation_ref": OPERATION, "freeze_only": False}


def test_settlement_uses_the_fixed_payment_configuration(monkeypatch):
    seen = fake(monkeypatch, settled())
    assert seam().payment(OPERATION, method="settle")["payment_verified"] is True
    assert json.loads(seen[0][1]["input"]) == {"method": "settle", "config_file": "/operator/payment.json", "operation_ref": OPERATION}


def test_version_requires_both_native_protocols(monkeypatch):
    negotiate = {"status": "ok", "protocol_version": 1, "service": "erebus-negotiate", "settlement_submission": False}
    payment = {"status": "ok", "protocol_version": 1, "service": "erebus-payment",
               "modes": ["public-bound", "shielded"], "automatic_rebroadcast": False}
    fake(monkeypatch, negotiate, payment)
    assert seam().version()["mode"] == "shielded"
    fake(monkeypatch, negotiate, {**payment, "modes": ["public-bound"]})
    with pytest.raises(MetropolisError):
        seam().version()
    fake(monkeypatch, {**negotiate, "settlement_submission": True})
    with pytest.raises(MetropolisError):
        seam(role="seller").version()


def test_seller_cannot_pay(monkeypatch):
    seen = fake(monkeypatch, settled())
    with pytest.raises(MetropolisError):
        seam(role="seller").payment(OPERATION, method="settle")
    with pytest.raises(MetropolisError):
        MetropolisSeam(negotiation_config="/n.json", state_root="/s", role="seller", payment_config="/p.json",
                       negotiation_binary="negotiate")
    assert not seen


def test_x402_buyer_negotiates_but_cannot_use_the_ordinary_payment_rail(monkeypatch):
    seen = fake(monkeypatch, authorized())
    client = MetropolisSeam(negotiation_config="/operator/negotiation.json", state_root="/operator/state",
                            role="buyer", mode="x402-exact", negotiation_binary="negotiate")
    assert client.negotiate(OPERATION) == authorized()
    for method in ("funding", "settle", "observe"):
        with pytest.raises(MetropolisError):
            client.payment(OPERATION, method=method)
    assert len(seen) == 1
    with pytest.raises(MetropolisError):
        MetropolisSeam(negotiation_config="/n.json", state_root="/s", role="buyer", mode="x402-exact",
                       payment_config="/p.json", negotiation_binary="negotiate")


@pytest.mark.parametrize("operation", ["", "0" * 64, "AB" * 32, "ab" * 31, "../" + "a" * 61])
def test_bad_operation_never_starts_a_process(monkeypatch, operation):
    seen = fake(monkeypatch, authorized())
    with pytest.raises(MetropolisError):
        seam().negotiate(operation)
    with pytest.raises(MetropolisError):
        seam().payment(operation, method="observe")
    assert not seen


def test_unknown_payment_method_never_starts_a_process(monkeypatch):
    seen = fake(monkeypatch, settled())
    with pytest.raises(MetropolisError):
        seam().payment(OPERATION, method="rebroadcast")
    assert not seen


@pytest.mark.parametrize("field,value", [("payment_verified", True), ("delivery_verified", True), ("agreement_verified", False),
                                         ("evidence_file", "/elsewhere/agent.tx"), ("deal_commitment", "0" * 64), ("private_key", "x"),
                                         ("measurements_ms", {"negotiation": -1}), ("measurements_ms", {"proof_preparation": 1})])
def test_negotiation_overclaims_are_rejected(monkeypatch, field, value):
    fake(monkeypatch, {**authorized(), field: value})
    with pytest.raises(MetropolisError):
        seam().negotiate(OPERATION)


@pytest.mark.parametrize("field,value", [("mode", "public-bound"), ("stage", "Submitted"), ("winning_commitment", "11" * 32),
                                         ("delivery_verified", True), ("retry_without_new_payment", False),
                                         ("transaction_hash", "0xnot"), ("locally_signed_transaction_hash", "0xnot"),
                                         ("measurements_ms", {"negotiation_guess": 1}), ("calldata", "0x00")])
def test_payment_overclaims_and_mode_downgrades_are_rejected(monkeypatch, field, value):
    fake(monkeypatch, {**settled(), field: value})
    with pytest.raises(MetropolisError):
        seam().payment(OPERATION, method="settle")


def test_observation_cannot_claim_a_submission(monkeypatch):
    fake(monkeypatch, settled())
    with pytest.raises(MetropolisError):
        seam().payment(OPERATION, method="observe")


def test_pending_observation_is_returned_with_exit_two(monkeypatch):
    pending = {**settled(), "status": "pending", "payment_verified": False, "submitted_this_call": False,
               "stage": "Submitted", "history_pending": True, "locally_signed_transaction_hash": "0x" + "bb" * 32}
    del pending["winning_commitment"]
    fake(monkeypatch, pending, code=2)
    assert seam().payment(OPERATION, method="observe")["status"] == "pending"


@pytest.mark.parametrize("code,stderr", [(1, ""), (0, "private-provider-error")])
def test_process_errors_are_redacted(monkeypatch, code, stderr):
    fake(monkeypatch, {"status": "error", "error": "private-provider-error"}, code=code, stderr=stderr)
    with pytest.raises(MetropolisError) as error:
        seam().payment(OPERATION, method="observe")
    assert "private-provider-error" not in str(error.value)


def test_chain_times_pass_through_with_verified_payment(monkeypatch):
    times = {"finalized_anchor_block": 20, "finalized_anchor_unix": 1_700_000_020, "inclusion_block": 18, "inclusion_unix": 1_700_000_018}
    fake(monkeypatch, {**settled(), "chain_times": times})
    assert seam().payment(OPERATION, method="settle")["chain_times"] == times


@pytest.mark.parametrize("times", [{"inclusion_block": 1, "inclusion_unix": 1}, {"finalized_anchor_block": 1, "finalized_anchor_unix": -1},
                                   {"finalized_anchor_block": 1, "finalized_anchor_unix": 1, "rpc_url": 1}, []])
def test_malformed_chain_times_are_rejected(monkeypatch, times):
    fake(monkeypatch, {**settled(), "chain_times": times})
    with pytest.raises(MetropolisError):
        seam().payment(OPERATION, method="settle")


def test_chain_times_require_verified_payment(monkeypatch):
    pending = {**settled(), "status": "pending", "payment_verified": False, "stage": "Submitted",
               "chain_times": {"finalized_anchor_block": 1, "finalized_anchor_unix": 1}}
    del pending["winning_commitment"]
    fake(monkeypatch, pending, code=2)
    with pytest.raises(MetropolisError):
        seam().payment(OPERATION, method="settle")


def test_relay_status_is_diagnostic_not_payment_verification(monkeypatch):
    reply = {**settled(), "status": "pending", "payment_verified": False,
             "submitted_this_call": False, "stage": "Prepared", "broadcast_attempts": 0,
             "submission_delegated": True, "relay_status": "finalized",
             "relay_operation_ref": "ab" * 32, "relay_transaction_hash": "0x" + "cd" * 32,
             "measurements_ms": {"relay_submission": 10}}
    reply.pop("winning_commitment", None)
    fake(monkeypatch, reply, code=2)
    assert seam().payment(OPERATION, method="settle")["payment_verified"] is False
    fake(monkeypatch, {**reply, "submission_delegated": False}, code=2)
    with pytest.raises(MetropolisError, match="delegation"):
        seam().payment(OPERATION, method="settle")


def test_sponsored_sdk_rejects_direct_broadcast_claim(monkeypatch):
    client = MetropolisSeam(negotiation_config="/n.json", state_root="/s", role="buyer",
                           payment_config="/p.json", mode="shielded", submission_mode="sponsored",
                           negotiation_binary="negotiate", payment_binary="payment")
    fake(monkeypatch, settled())
    with pytest.raises(MetropolisError, match="direct buyer"):
        client.payment(OPERATION, method="settle")


def test_a_seller_names_one_peer_and_its_reply_names_the_bound_peer(monkeypatch):
    seen = fake(monkeypatch, {**authorized(), "peer": "b2"}, {**authorized(), "peer": "b1"})
    assert seam("seller").negotiate(OPERATION, peer="b2")["peer"] == "b2"
    assert json.loads(seen[0][1]["input"])["peer"] == "b2"
    # A seller that named no peer accepts whichever configured peer bound the operation.
    assert seam("seller").negotiate(OPERATION)["peer"] == "b1"
    assert "peer" not in json.loads(seen[1][1]["input"])


@pytest.mark.parametrize("reply", [{"peer": "b1"}, {"peer": "B1"}, {"peer": 7}])
def test_a_peer_reply_that_differs_or_is_malformed_is_refused(monkeypatch, reply):
    fake(monkeypatch, {**authorized(), **reply})
    with pytest.raises(MetropolisError):
        seam("seller").negotiate(OPERATION, peer="b2")


def test_a_buyer_neither_names_nor_reports_a_peer(monkeypatch):
    calls = fake(monkeypatch, {**authorized(), "peer": "b1"})
    with pytest.raises(MetropolisError):
        seam().negotiate(OPERATION, peer="b1")
    assert calls == []
    with pytest.raises(MetropolisError):
        seam().negotiate(OPERATION)
    with pytest.raises(MetropolisError):
        seam("seller").negotiate(OPERATION, peer="../b1")


def listing():
    return {"protocol_version": 1, "status": "ok", "config_version": 2, "peers": [
        {"peer": "b1", "enabled": True, "expired": False, "live": True, "resource": "data.snapshot.v1",
         "price": "70", "minimum_price": "65", "expires_at": 1, "legacy_transport": False}]}


def test_a_seller_lists_peers_through_the_native_command(monkeypatch):
    seen = fake(monkeypatch, listing())
    assert seam("seller").list_peers() == listing()["peers"]
    assert json.loads(seen[0][1]["input"]) == {"method": "list_peers", "config_file": "/operator/negotiation.json"}
    with pytest.raises(MetropolisError):
        seam().list_peers()


@pytest.mark.parametrize("change", [{"key": "00"}, {"price": "07"}, {"live": 1}, {"peer": "B"}])
def test_an_unexpected_peer_listing_is_refused(monkeypatch, change):
    bad = listing()
    bad["peers"][0].update(change)
    fake(monkeypatch, bad)
    with pytest.raises(MetropolisError):
        seam("seller").list_peers()
