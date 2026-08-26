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
from erebus_agents.intents import IntentStore

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
            "UV_CACHE_DIR": "/private/tmp/erebus-uv-cache",
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


async def _write_call(
    session: ClientSession,
    intents: IntentStore,
    intent_key: str,
    tool: str,
    **kwargs: Any,
) -> dict[str, Any]:
    record = intents.prepare(intent_key, tool, kwargs)
    if record["state"] == "completed":
        return record["result"]
    arguments = {"operation_id": record["operation_id"], **record["arguments"]}
    try:
        result = await _call(session, tool, **arguments)
    except RuntimeError as original:
        # Classify before taking any recovery action. Pending and unknown operations are
        # deliberately left for an operator; minting a replacement id here could pay twice.
        findings = await _call(session, "reconcile")
        finding = next(
            (item for item in findings if item["operation_id"] == record["operation_id"]),
            None,
        )
        if finding is None or finding["next_action"] in {"wait", "operator_attention"}:
            raise original
        await _call(session, "resume_operation", operation_id=record["operation_id"])
        result = await _call(session, tool, **arguments)
    intents.complete(intent_key, result)
    return result


def _intent_path(params: StdioServerParameters, identity: str) -> Path:
    env = params.env or {}
    configured = env.get("EREBUS_CALLER_INTENT_PATH")
    if configured:
        return Path(configured)
    state_dir = env.get("EREBUS_STATE_DIR")
    if state_dir:
        return Path(state_dir) / "caller-intents.json"
    mock_store = env.get("EREBUS_MOCK_STORE_PATH")
    if mock_store:
        safe_identity = identity.replace("/", "_")
        return Path(mock_store).parent / f"caller-intents-{safe_identity}.json"
    raise ValueError("set EREBUS_CALLER_INTENT_PATH or EREBUS_STATE_DIR for durable writes")


def _decode_offer(raw: dict[str, Any], handle: str) -> Offer:
    terms = raw["terms"]
    return Offer(
        offer_id=raw["offer_id"],
        deal_id=int(raw.get("deal_id", 0)),
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
    return ChannelState(
        offers=[_decode_offer(o, handle) for o in raw["offers"]], settlements=[]
    )


async def run_negotiation_over_mcp(
    *,
    buyer_params: StdioServerParameters,
    seller_params: StdioServerParameters,
    buyer_policy: BuyerPolicy,
    seller_policy: SellerPolicy,
    buyer_address: str,
    seller_address: str,
    token: str,
    max_rounds: int = 3,
) -> dict[str, Any]:
    buyer_intents = IntentStore(_intent_path(buyer_params, buyer_address))
    seller_intents = IntentStore(_intent_path(seller_params, seller_address))
    async with stdio_client(seller_params) as (seller_read, seller_write):
        async with ClientSession(seller_read, seller_write) as seller:
            await seller.initialize()
            async with stdio_client(buyer_params) as (buyer_read, buyer_write):
                async with ClientSession(buyer_read, buyer_write) as buyer:
                    await buyer.initialize()

                    opened = await _write_call(
                        buyer, buyer_intents, "open-channel", "open_channel",
                        counterparty=seller_address,
                    )
                    buyer_handle = opened["channel_handle"]
                    opened = await _write_call(
                        seller, seller_intents, "open-channel", "open_channel",
                        counterparty=buyer_address,
                    )
                    seller_handle = opened["channel_handle"]
                    _log_event(
                        "channel_opened",
                        buyer_handle=buyer_handle,
                        seller_handle=seller_handle,
                    )

                    settled = False
                    for round_index in range(max_rounds + 1):
                        state = await _call(
                            buyer, "read_channel_state", channel_handle=buyer_handle
                        )
                        balance = await _call(buyer, "get_note_balance")
                        buyer_decision = buyer_policy.decide(
                            _decode_state(state, buyer_handle),
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
                            receipt = await _write_call(
                                buyer, buyer_intents, f"round-{round_index}-settle",
                                "accept_and_settle",
                                channel_handle=buyer_handle,
                                offer_id=buyer_decision.reply_to,
                            )
                            _log_event("settled", by="buyer", tx_hash=receipt["tx_hash"])
                            settled = True
                            break
                        if buyer_decision.action == NegotiationAction.PROPOSE:
                            offer = await _write_call(
                                buyer, buyer_intents, f"round-{round_index}-propose",
                                "propose_offer",
                                channel_handle=buyer_handle,
                                amount=str(buyer_decision.terms.amount),
                                token=token,
                                deadline=buyer_decision.terms.deadline,
                                memo_hash=buyer_decision.terms.memo_hash,
                            )
                            _log_event("proposed", by="buyer", offer_id=offer["offer_id"])
                        elif buyer_decision.action == NegotiationAction.COUNTER:
                            offer = await _write_call(
                                buyer, buyer_intents, f"round-{round_index}-counter",
                                "counter_offer",
                                channel_handle=buyer_handle,
                                reply_to=buyer_decision.reply_to,
                                amount=str(buyer_decision.terms.amount),
                                token=token,
                                deadline=buyer_decision.terms.deadline,
                                memo_hash=buyer_decision.terms.memo_hash,
                            )
                            _log_event("countered", by="buyer", offer_id=offer["offer_id"])

                        seller_state = await _call(
                            seller, "read_channel_state", channel_handle=seller_handle
                        )
                        seller_decision = seller_policy.decide(
                            _decode_state(seller_state, seller_handle), round_index
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
                            offer = await _write_call(
                                seller, seller_intents, f"round-{round_index}-counter",
                                "counter_offer",
                                channel_handle=seller_handle,
                                reply_to=seller_decision.reply_to,
                                amount=str(seller_decision.terms.amount),
                                token=token,
                                deadline=seller_decision.terms.deadline,
                                memo_hash=seller_decision.terms.memo_hash,
                            )
                            _log_event("countered", by="seller", offer_id=offer["offer_id"])

                    if not settled:
                        _log_event(
                            "negotiation_ended_without_settlement",
                            buyer_handle=buyer_handle,
                            seller_handle=seller_handle,
                        )

                    state = await _call(
                        buyer, "read_channel_state", channel_handle=buyer_handle
                    )

    _log_event(
        "final_state",
        buyer_handle=buyer_handle,
        seller_handle=seller_handle,
        offer_count=len(state["offers"]),
        settled=bool(state["settlements"]),
        disclosure="available_as_an_explicit_recipient_bound_operator_step",
    )
    return state
