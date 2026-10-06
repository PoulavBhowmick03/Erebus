"""The auditor driver never turns missing or overreaching evidence into a pass."""

import asyncio

import pytest

from erebus_agents.metropolis_audit import drive_audit
from erebus_agents.metropolis_loop import HarnessError

COMMITMENT = "cd" * 32


def facts(**overrides):
    return {"ok": True, "result": {"agreement_verified": True, "payment_verified": False,
                                   "delivery_verified": False, "deal_commitment": COMMITMENT, **overrides}}


class Auditor:
    def __init__(self, **replies):
        self.replies = replies
        self.calls = []

    async def call_tool(self, name, arguments):
        assert arguments == {"grant_name": "grant.json", "expected_issuer": "buyer"}
        self.calls.append(name)
        return self.replies[name]


def run(auditor, **options):
    return asyncio.run(drive_audit(auditor, "grant.json", "buyer", **options))


def test_agreement_only_audit_skips_payment_verification():
    auditor = Auditor(verify_disclosed_agreement=facts())
    assert run(auditor)["agreement_verified"] is True
    assert auditor.calls == ["verify_disclosed_agreement"]


def test_payment_audit_reports_the_payment_result():
    auditor = Auditor(verify_disclosed_agreement=facts(), verify_disclosed_payment=facts(payment_verified=True))
    report = run(auditor, verify_payment=True, expected_commitment=COMMITMENT)
    assert report["payment_verified"] is True and report["delivery_verified"] is False


@pytest.mark.parametrize("agreement", [facts(agreement_verified=False), {"ok": False, "error": {"code": "DISCLOSURE_UNAVAILABLE"}}])
def test_unverified_grant_is_rejected(agreement):
    with pytest.raises(HarnessError, match="agreement"):
        run(Auditor(verify_disclosed_agreement=agreement))


def test_unavailable_payment_check_is_not_a_pass():
    auditor = Auditor(verify_disclosed_agreement=facts(),
                      verify_disclosed_payment={"ok": False, "error": {"code": "DISCLOSURE_UNAVAILABLE"}})
    with pytest.raises(HarnessError, match="non-payment"):
        run(auditor, verify_payment=True)


def test_a_delivery_claim_is_rejected():
    with pytest.raises(HarnessError, match="delivery"):
        run(Auditor(verify_disclosed_agreement=facts(delivery_verified=True)))


def test_a_grant_for_another_deal_is_rejected():
    with pytest.raises(HarnessError, match="negotiated deal"):
        run(Auditor(verify_disclosed_agreement=facts(deal_commitment="11" * 32)), expected_commitment=COMMITMENT)
