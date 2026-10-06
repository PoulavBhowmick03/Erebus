"""The auditor driver never turns missing, pending, or overreaching evidence into a pass."""

import asyncio

import pytest

from erebus_agents.metropolis_audit import (
    auditor_public_key, disclosure_params, drive_audit, issue_grant)
from erebus_agents.metropolis_loop import HarnessError

COMMITMENT = "cd" * 32
UNAVAILABLE = {"ok": False, "error": {"code": "DISCLOSURE_UNAVAILABLE"}}


def facts(**overrides):
    return {"ok": True, "result": {"status": "ok", "agreement_verified": True, "payment_verified": False,
                                   "delivery_verified": False, "deal_commitment": COMMITMENT, **overrides}}


class Session:
    def __init__(self, **replies):
        self.replies = {name: list(values) for name, values in replies.items()}
        self.calls = []

    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        return self.replies[name].pop(0)

    def names(self):
        return [name for name, _ in self.calls]


async def no_sleep(_):
    pass


def run(auditor, **options):
    return asyncio.run(drive_audit(auditor, "grant.json", "seller", sleep=no_sleep, **options))


def test_agreement_only_audit_skips_payment_verification():
    auditor = Session(verify_disclosed_agreement=[facts()])
    assert run(auditor)["agreement_verified"] is True
    assert auditor.names() == ["verify_disclosed_agreement"]
    assert auditor.calls[0][1] == {"grant_name": "grant.json", "expected_issuer": "seller"}


def test_payment_audit_polls_through_pending_and_errors():
    pending = facts(status="pending", ancestry_block=7)
    auditor = Session(verify_disclosed_agreement=[facts()], verify_disclosed_payment=[
        pending, UNAVAILABLE, facts(payment_verified=True)])
    report = run(auditor, verify_payment=True, expected_commitment=COMMITMENT)
    assert report["payment_verified"] is True and report["delivery_verified"] is False
    assert auditor.names().count("verify_disclosed_payment") == 3


def test_payment_that_stays_pending_exhausts_the_budget_without_a_pass():
    auditor = Session(verify_disclosed_agreement=[facts()], verify_disclosed_payment=[facts(status="pending")] * 2)
    with pytest.raises(HarnessError, match="non-payment"):
        run(auditor, verify_payment=True, max_polls=2)


@pytest.mark.parametrize("agreement", [facts(agreement_verified=False), UNAVAILABLE])
def test_unverified_grant_is_rejected(agreement):
    with pytest.raises(HarnessError, match="agreement"):
        run(Session(verify_disclosed_agreement=[agreement]))


def test_a_delivery_claim_is_rejected():
    with pytest.raises(HarnessError, match="delivery"):
        run(Session(verify_disclosed_agreement=[facts(delivery_verified=True)]))


def test_a_grant_for_another_deal_is_rejected():
    with pytest.raises(HarnessError, match="negotiated deal"):
        run(Session(verify_disclosed_agreement=[facts(deal_commitment="11" * 32)]), expected_commitment=COMMITMENT)


def test_existing_auditor_key_is_recovered_not_regenerated():
    auditor = Session(disclosure_key_info=[{"ok": True, "result": {"recipient_public_key": "pk"}}])
    assert asyncio.run(auditor_public_key(auditor)) == "pk"
    assert auditor.names() == ["disclosure_key_info"]


def test_missing_auditor_key_is_created_once():
    auditor = Session(disclosure_key_info=[UNAVAILABLE],
                      create_disclosure_key=[{"ok": True, "result": {"recipient_public_key": "pk"}}])
    assert asyncio.run(auditor_public_key(auditor)) == "pk"
    assert auditor.names() == ["disclosure_key_info", "create_disclosure_key"]


def test_grant_is_selected_then_exported_to_the_auditor_key():
    issuer = Session(select_deal_disclosure=[facts()], export_deal_disclosure=[facts()])
    asyncio.run(issue_grant(issuer, "op", "pk", evidence_name="op.evidence", grant_name="deal.grant",
                            grant_seconds=60, clock=lambda: 1000.0))
    assert issuer.calls == [
        ("select_deal_disclosure", {"operation_ref": "op", "evidence_name": "op.evidence"}),
        ("export_deal_disclosure", {"evidence_name": "op.evidence", "recipient_public_key": "pk",
                                    "grant_name": "deal.grant", "expires_at": 1060})]


def test_an_issuer_that_cannot_select_the_deal_exports_nothing():
    issuer = Session(select_deal_disclosure=[UNAVAILABLE], export_deal_disclosure=[facts()])
    with pytest.raises(HarnessError, match="select"):
        asyncio.run(issue_grant(issuer, "op", "pk", evidence_name="e", grant_name="g", grant_seconds=60))
    assert issuer.names() == ["select_deal_disclosure"]


def test_only_the_issuer_server_gets_issuer_settings_and_only_the_auditor_gets_a_deployment():
    issuer = disclosure_params("python", "/issuer", issuer={"state_root": "/s", "store_root": "/t",
                                                           "namespace": "ns", "issuer_key_file": "/k"})
    auditor = disclosure_params("python", "/auditor", deployment={"rail": "x402_exact"}, cli="/bin/disclosure")
    assert issuer.env["EREBUS_DISCLOSURE_ISSUER_KEY_FILE"] == "/k" and "EREBUS_DISCLOSURE_DEPLOYMENT" not in issuer.env
    assert "EREBUS_DISCLOSURE_ISSUER_KEY_FILE" not in auditor.env
    assert auditor.env["EREBUS_DISCLOSURE_CLI"] == "/bin/disclosure"
    assert "x402_exact" in auditor.env["EREBUS_DISCLOSURE_DEPLOYMENT"]
