"""The same negotiation loop as agent.py, driven over real MCP connections.

Each identity is a genuine MCP client talking to its own `server.py` subprocess over
stdio, not a direct `MockErebusClient` call. Same `BuyerPolicy`/`SellerPolicy` decision
logic either way — this only changes the transport.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

from erebus_mcp.interface import ChannelState, Offer, OfferStatus, OfferTerms
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus_agents.policy import BuyerPolicy, NegotiationAction, SellerPolicy

logger = logging.getLogger("erebus_agents")

REPO_ROOT = Path(__file__).resolve().parents[3]
SERVER_PATH = REPO_ROOT / "mcp-server" / "src" / "server.py"


def _log_event(event: str, **fields: object) -> None:
    logger.info(json.dumps({"event": event, **fields}))


def server_params(
    store_path: Path,
    role: str,
    identity: str,
    spendable_notes: str = "",
    latency_seconds: str = "0",
) -> StdioServerParameters:
    return StdioServerParameters(
        command="uv",
        args=["run", "python", str(SERVER_PATH)],
        cwd=str(REPO_ROOT),
        env={
            "AGENT_ADDRESS": identity,
            "PROVING_SERVICE_URL": "http://unused.invalid",
            "EREBUS_MOCK_STORE_PATH": str(store_path),
            "EREBUS_MOCK_LATENCY_SECONDS": latency_seconds,
            "EREBUS_MOCK_SPENDABLE_NOTES": spendable_notes,
            "EREBUS_SETTLEMENT_ROLE": role,
        },
    )


def _structured(result: Any) -> dict[str, Any]:
    if result.structured_content is not None:
        return result.structured_content
    return json.loads(result.content[0].text)


async def _call(session: ClientSession, tool: str, **kwargs: Any) -> dict[str, Any]:
    outcome = _structured(await session.call_tool(tool, kwargs))
    if not outcome["ok"]:
        raise RuntimeError(f"{tool} failed: {outcome['error']}")
    return outcome["result"]


def _decode_offer(raw: dict[str, Any], handle: str) -> Offer:
    terms = raw["terms"]
    return Offer(
        offer_id=raw["offer_id"],
        channel_id=handle,
        proposer=raw["proposer"],
        terms=OfferTerms(
            amount=int(terms["amount"]),
            token=terms["token"],
            deadline=terms["deadline"],
            memo_hash=int(terms["memo_hash"], 16),
        ),
        status=OfferStatus(raw["status"]),
        created_at=raw["created_at"],
        reply_to=raw.get("reply_to"),
    )


def _decode_state(raw: dict[str, Any], handle: str) -> ChannelState:
    return ChannelState(offers=[_decode_offer(o, handle) for o in raw["offers"]])


async def run_negotiation_over_mcp(
    *,
    buyer_params: StdioServerParameters,
    seller_params: StdioServerParameters,
    auditor_params: StdioServerParameters,
    buyer_policy: BuyerPolicy,
    seller_policy: SellerPolicy,
    buyer_address: str,
    seller_address: str,
    auditor_address: str,
    grant_export_path: Path,
    token: str,
    max_rounds: int = 3,
) -> dict[str, Any]:
    async with stdio_client(seller_params) as (seller_read, seller_write):
        async with ClientSession(seller_read, seller_write) as seller:
            await seller.initialize()
            async with stdio_client(buyer_params) as (buyer_read, buyer_write):
                async with ClientSession(buyer_read, buyer_write) as buyer:
                    await buyer.initialize()

                    opened = await _call(buyer, "open_channel", counterparty=seller_address)
                    handle = opened["channel_handle"]
                    await _call(seller, "open_channel", counterparty=buyer_address)
                    _log_event("channel_opened", channel_handle=handle)

                    settled = False
                    for round_index in range(max_rounds + 1):
                        state = await _call(buyer, "read_channel_state", channel_handle=handle)
                        balance = await _call(buyer, "get_note_balance")
                        buyer_decision = buyer_policy.decide(
                            _decode_state(state, handle),
                            round_index,
                            token,
                            int(balance["total"]),
                        )
                        _log_event(
                            "buyer_decision", round=round_index, action=buyer_decision.action.value
                        )

                        if buyer_decision.action == NegotiationAction.WALK:
                            _log_event("buyer_walked", round=round_index)
                            break
                        if buyer_decision.action == NegotiationAction.ACCEPT:
                            assert buyer_decision.reply_to is not None
                            receipt = await _call(
                                buyer,
                                "accept_and_settle",
                                channel_handle=handle,
                                offer_id=buyer_decision.reply_to,
                            )
                            _log_event("settled", by="buyer", tx_hash=receipt["tx_hash"])
                            settled = True
                            break
                        if buyer_decision.action == NegotiationAction.PROPOSE:
                            offer = await _call(
                                buyer,
                                "propose_offer",
                                channel_handle=handle,
                                amount=str(buyer_decision.terms.amount),
                                token=token,
                                deadline=buyer_decision.terms.deadline,
                                memo_hash=buyer_decision.terms.memo_hash,
                            )
                            _log_event("proposed", by="buyer", offer_id=offer["offer_id"])
                        elif buyer_decision.action == NegotiationAction.COUNTER:
                            offer = await _call(
                                buyer,
                                "counter_offer",
                                channel_handle=handle,
                                reply_to=buyer_decision.reply_to,
                                amount=str(buyer_decision.terms.amount),
                                token=token,
                                deadline=buyer_decision.terms.deadline,
                                memo_hash=buyer_decision.terms.memo_hash,
                            )
                            _log_event("countered", by="buyer", offer_id=offer["offer_id"])

                        seller_state = await _call(seller, "read_channel_state", channel_handle=handle)
                        seller_decision = seller_policy.decide(
                            _decode_state(seller_state, handle), round_index
                        )
                        _log_event(
                            "seller_decision", round=round_index, action=seller_decision.action.value
                        )

                        if seller_decision.action == NegotiationAction.WALK:
                            _log_event("seller_walked", round=round_index)
                            break
                        if seller_decision.action == NegotiationAction.ACCEPT:
                            raise RuntimeError("seller policy cannot accept: the accepting identity pays")
                        if seller_decision.action == NegotiationAction.COUNTER:
                            offer = await _call(
                                seller,
                                "counter_offer",
                                channel_handle=handle,
                                reply_to=seller_decision.reply_to,
                                amount=str(seller_decision.terms.amount),
                                token=token,
                                deadline=seller_decision.terms.deadline,
                                memo_hash=seller_decision.terms.memo_hash,
                            )
                            _log_event("countered", by="seller", offer_id=offer["offer_id"])

                    if not settled:
                        _log_event("negotiation_ended_without_settlement", channel_handle=handle)

                    grant_export = await _call(
                        buyer,
                        "grant_viewing_key",
                        channel_handle=handle,
                        grantee=auditor_address,
                        export_path=str(grant_export_path),
                    )
                    _log_event(
                        "viewing_key_granted",
                        channel_handle=handle,
                        grantee=auditor_address,
                        exported_to=grant_export["exported_to"],
                    )
                    # Read the grant back off disk, out of band from the MCP transcript —
                    # the tool result never carries it (roadmap 9.2). In production this
                    # read happens on whatever machine the file was delivered to, not here;
                    # this demo plays both roles.
                    grant = json.loads(grant_export_path.read_text())

    async with stdio_client(auditor_params) as (auditor_read, auditor_write):
        async with ClientSession(auditor_read, auditor_write) as auditor:
            await auditor.initialize()
            record = await _call(
                auditor,
                "reveal",
                channel_id=grant["channel_id"],
                grantee=grant["grantee"],
                viewing_key=grant["viewing_key"],
            )

    _log_event(
        "revealed",
        channel_handle=record["channel_id"],
        participants=record["participants"],
        offer_count=len(record["offers"]),
        settled=record["settlement"] is not None,
    )
    return record
