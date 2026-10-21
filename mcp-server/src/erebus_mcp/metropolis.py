"""One participant's native negotiation, payment, and optional access MCP tools."""

from __future__ import annotations

import asyncio
import json
import os
import stat
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from erebus import MetropolisError, MetropolisSeam, MetropolisWallet, WalletPolicy
from mcp.server import MCPServer

from erebus_mcp.config import ConfigError
from erebus_mcp.metropolis_errors import failure as classified
from erebus_mcp.metropolis_errors import refused
from erebus_mcp.metropolis_wallet_tools import DEFAULT_WAIT_SECONDS, MAX_WAIT_SECONDS, _register_wallet_tools, _settled_wallet
from erebus_mcp.operation_refs import OperationRefs


def _operator_file(value: str | None) -> tuple[str, dict[str, Any]]:
    if not value or not Path(value).is_absolute():
        raise ConfigError("configure absolute Metropolis operator files")
    path = Path(value)
    try:
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or not 1 <= info.st_size <= 32_768 or (os.name == "posix" and info.st_mode & 0o077):
            raise ConfigError("Metropolis configuration must be an owner-only regular file")
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        raise ConfigError("Metropolis configuration is unavailable or invalid") from None
    if not isinstance(data, dict):
        raise ConfigError("Metropolis configuration must be a JSON object")
    return str(path), data


@dataclass(frozen=True)
class MetropolisSettings:
    negotiation_config: str
    state_root: str
    role: str
    payment_config: str | None = None
    mode: str | None = None
    negotiation_binary: str | None = None
    payment_binary: str | None = None
    timeout: int = 600
    submission_mode: str = "direct"

    @classmethod
    def from_env(cls) -> MetropolisSettings:
        negotiation, data = _operator_file(os.environ.get("EREBUS_NEGOTIATION_CONFIG"))
        role, root = data.get("role"), data.get("state_root")
        if role not in {"buyer", "seller"} or not isinstance(root, str) or not Path(root).is_absolute():
            raise ConfigError("Metropolis needs a fixed buyer or seller role and absolute state root")
        payment, mode, submission_mode = None, None, "direct"
        if role == "buyer" and os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL") == "x402-exact":
            if os.environ.get("EREBUS_PAYMENT_CONFIG") or not os.environ.get("EREBUS_ACCESS_SERVICE_URL"):
                raise ConfigError("x402 requires an access service and no ordinary payment configuration")
            mode = "x402-exact"
        elif role == "buyer":
            payment, selected = _operator_file(os.environ.get("EREBUS_PAYMENT_CONFIG"))
            mode = selected.get("mode", "public-bound")
            if mode not in {"public-bound", "shielded"} or selected.get("state_root") != root:
                raise ConfigError("payment mode must be explicit and share this participant's state root")
            if selected.get("relay") is not None:
                if mode != "shielded" or not isinstance(selected["relay"], dict) or selected.get("transaction_key_file") is not None:
                    raise ConfigError("sponsorship requires shielded relay configuration and no buyer gas key")
                submission_mode = "sponsored"
        elif os.environ.get("EREBUS_PAYMENT_CONFIG"):
            raise ConfigError("a seller server cannot have payment configuration")
        timeout = os.environ.get("EREBUS_NATIVE_TIMEOUT_SECONDS", "600")
        if not timeout.isascii() or not timeout.isdecimal() or not 1 <= int(timeout) <= 900:
            raise ConfigError("native timeout must be between 1 and 900 seconds")
        return cls(negotiation, root, role, payment, mode, os.environ.get("EREBUS_NEGOTIATION_CLI"),
                   os.environ.get("EREBUS_PAYMENT_CLI"), int(timeout), submission_mode)


def build_metropolis_server(settings: MetropolisSettings | None = None) -> MCPServer:
    settings = settings or MetropolisSettings.from_env()
    if settings.mode == "x402-exact" and os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL") != "x402-exact":
        raise ConfigError("x402 access rail must be explicitly selected by the operator")
    try:
        seam = MetropolisSeam(**vars(settings))
        seam.version()
    except MetropolisError:
        raise ConfigError("Metropolis native binaries are unavailable or incompatible") from None
    server = MCPServer(name="erebus-metropolis", instructions=(
        "Private negotiation with operator-fixed service policy, participant keys, and settlement mode. "
        f"This participant is the {settings.role}. "
        "Reuse the same operation ID after interruption. Negotiation cannot submit a payment. "
        "Only the buyer can settle; recovery observes without submitting. "
        "Pending or failed access is not a reason to pay again. "
        "Public-bound settlement exposes the accepted terms and parties. "
        "Shielded proving uses local witnesses and authenticated public artifacts. "
        "Payment verification does not prove delivery. "
        + ("This buyer explicitly delegates submission to its operator-pinned sponsor. Remote relay status is not payment verification. No direct-submission fallback is allowed. "
           if settings.submission_mode == "sponsored" else "")
        + ("x402 exact mode exposes negotiation and access only. First retrieval signs one local permit; the seller submits payment. Retries reuse that permit; independent disclosure verifies payment."
           if settings.mode == "x402-exact" else "")
    ))

    refs = OperationRefs(settings.state_root)
    # One funding or settlement action at a time per participant: they share one signer.
    writes = asyncio.Lock()
    wallet: MetropolisWallet | None = None
    methods: list[str] = []

    def failure(error: MetropolisError) -> dict[str, Any]:
        return classified(error, "METROPOLIS_UNAVAILABLE")

    if os.environ.get("EREBUS_WALLET_CONFIG") or os.environ.get("EREBUS_WALLET_POLICY"):
        wallet_config, wallet_data = _operator_file(os.environ.get("EREBUS_WALLET_CONFIG"))
        _, wallet_policy = _operator_file(os.environ.get("EREBUS_WALLET_POLICY"))
        required_policy = {"version", "maximum_amount", "allowed_methods", "withdrawal_recipients"}
        if wallet_data.get("state_root") != settings.state_root or not required_policy <= set(wallet_policy) <= required_policy | {"replacement_fees"} or type(wallet_policy.get("version")) is not int or wallet_policy["version"] != 1:
            raise ConfigError("wallet must share this operator's state root and explicit versioned action policy")
        methods, recipients = wallet_policy.get("allowed_methods"), wallet_policy.get("withdrawal_recipients")
        if not isinstance(methods, list) or any(not isinstance(method, str) for method in methods) or len(methods) != len(set(methods)) or not isinstance(recipients, list) or any(not isinstance(recipient, str) for recipient in recipients):
            raise ConfigError("wallet policy requires explicit unique action and recipient lists")
        try:
            wallet = MetropolisWallet(config_file=wallet_config,
                policy=WalletPolicy(wallet_policy["maximum_amount"], frozenset(methods), frozenset(recipients),
                    tuple(wallet_policy["replacement_fees"]) if isinstance(wallet_policy.get("replacement_fees"), list) else None),
                binary=os.environ.get("EREBUS_WALLET_CLI"), timeout=settings.timeout)
            wallet.version()
        except (MetropolisError, TypeError):
            raise ConfigError("invalid wallet action policy or missing native wallet command") from None
        _register_wallet_tools(server, wallet, methods, refs, writes,
                               {"address": wallet_data.get("asset"), "signer": wallet_data.get("signer_address"),
                                "wrapper": wallet_data.get("wrapper")})

    seller = settings.role == "seller"

    async def negotiate(operation_ref: str | None, freeze_only: bool, wait_seconds: int,
                        peer: str | None) -> dict[str, Any]:
        if type(wait_seconds) is not int or not 0 <= wait_seconds <= MAX_WAIT_SECONDS:
            return refused("INVALID_REQUEST", "METROPOLIS_UNAVAILABLE", f"wait_seconds must be 0-{MAX_WAIT_SECONDS}")
        created, omitted = False, operation_ref is None
        if operation_ref is None:
            operation_ref, created = (refs.resolve_peer("negotiate", peer) if seller
                                      else refs.resolve("negotiate", None))
        deadline = time.monotonic() + wait_seconds
        while True:
            try:
                result = await asyncio.to_thread(seam.negotiate, operation_ref, freeze_only=freeze_only,
                                                 **({"peer": peer} if peer is not None else {}))
                break
            except MetropolisError as error:
                reply = failure(error)
                code = reply["error"]["code"]
                if code == "OPERATION_BOUND_TO_OTHER_PEER" and omitted and peer is not None and not created:
                    # A resumed unbound entry was bound natively to some other buyer after a failed
                    # call. Keep it for that buyer and resolve again for this one.
                    refs.update(operation_ref, peer_unknown=True)
                    operation_ref, created = refs.resolve_peer("negotiate", peer)
                    continue
                if code in {"PEER_UNAVAILABLE", "NO_PEER_CONNECTED"} and time.monotonic() < deadline:
                    await asyncio.sleep(2)
                    continue
                return {**reply, "operation_ref": operation_ref, "operation_ref_created": created}
        if seller:
            bound = {"peer": result["peer"], "peer_unknown": False} if "peer" in result else {}
            # A seller's deal ends at authorization; its next omitted reference is a new deal.
            refs.update(operation_ref, **bound, **({} if freeze_only else {"done": True}))
        if isinstance(result.get("evidence_file"), str):
            result = {**result, "evidence_name": Path(result["evidence_file"]).name}
        return {"ok": True, "result": result, "operation_ref": operation_ref, "operation_ref_created": created}

    if seller:
        @server.tool()
        async def negotiate_deal(operation_ref: str | None = None, peer: str | None = None, freeze_only: bool = False,
                                 wait_seconds: int = DEFAULT_WAIT_SECONDS) -> dict[str, Any]:
            """Negotiate and authorize one fixed-policy deal with one configured buyer; never submits payment.

            Deals run one at a time. The first configured buyer to connect binds the operation, and the result
            names it as peer with its deal_commitment; call again for the next buyer. Pass peer (from
            list_peers) to accept only that buyer. Omit operation_ref to resume this seller's unfinished deal for
            that peer, else an unbound one, else a new one. Keeps listening for up to wait_seconds (0-900).
            """
            return await negotiate(operation_ref, freeze_only, wait_seconds, peer)

        @server.tool()
        async def list_peers() -> dict[str, Any]:
            """List this seller's configured buyers: id, resource, price, and whether each can connect now."""
            try:
                peers = await asyncio.to_thread(seam.list_peers)
            except MetropolisError as error:
                return failure(error)
            return {"ok": True, "result": {"peers": peers}}
    else:
        @server.tool()
        async def negotiate_deal(operation_ref: str | None = None, freeze_only: bool = False,
                                 wait_seconds: int = DEFAULT_WAIT_SECONDS) -> dict[str, Any]:
            """Negotiate and authorize one fixed-policy deal; never submits payment.

            Omit operation_ref to have one created and saved; it is returned, and a retry without one
            resumes the same unfinished deal. While the counterparty is not connected yet, this keeps
            listening or reconnecting for up to wait_seconds (0-900) with the same operation_ref.
            """
            return await negotiate(operation_ref, freeze_only, wait_seconds, None)

    if settings.role == "buyer" and settings.mode != "x402-exact":
        async def payment(operation_ref: str, method: str) -> dict[str, Any]:
            if method in {"funding", "settle"} and wallet is not None:
                # A deposit or other wallet transaction that is not final yet still holds the
                # signer; settle only after the wallet has caught up and released it.
                held = await _settled_wallet(wallet, settings.timeout)
                if held is not None:
                    return {**held, "operation_ref": operation_ref}
            if method in {"settle", "observe"}:
                refs.update(operation_ref, done=True)
            try:
                response = await asyncio.to_thread(seam.payment, operation_ref, method=method)
            except MetropolisError as error:
                return {**failure(error), "operation_ref": operation_ref}
            reply = {"ok": response["status"] in {"ok", "ready"}, "result": response, "operation_ref": operation_ref}
            if response["status"] == "pending":
                reply["next_action"] = ("history or settlement is still pending; repeat this call with the same "
                                        "operation_ref, never a new payment")
            return reply

        @server.tool()
        async def check_settlement_funding(operation_ref: str) -> dict[str, Any]:
            """Check funding for an authorized deal without signing or submitting."""
            return await payment(operation_ref, "funding")

        if settings.submission_mode == "sponsored":
            @server.tool()
            async def sponsor_shielded_deal(operation_ref: str) -> dict[str, Any]:
                """Prove locally and delegate to the pinned sponsor; never directly broadcasts."""
                async with writes:
                    return await payment(operation_ref, "settle")
        else:
            @server.tool()
            async def settle_deal(operation_ref: str) -> dict[str, Any]:
                """Sync the wallet, then prepare, prove if shielded, and submit at most one automatic broadcast."""
                async with writes:
                    return await payment(operation_ref, "settle")

        @server.tool()
        async def recover_deal(operation_ref: str) -> dict[str, Any]:
            """Observe and reconcile this operation; cannot sign, prove, or submit a payment."""
            return await payment(operation_ref, "observe")

    if settings.role == "buyer":
        if os.environ.get("EREBUS_ACCESS_SERVICE_URL"):
            from erebus_mcp.access import AccessSettings, register_access_tools

            if settings.mode != "x402-exact" and os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL", "observe") != "observe":
                raise ConfigError("x402 authorization cannot be combined with ordinary settlement tools; use the dedicated access server")

            # Negotiation writes buyer evidence only under <state_root>/agent, so that is the
            # evidence directory; a separately configured one could only disagree with it.
            evidence = Path(settings.state_root) / "agent"
            configured = os.environ.get("EREBUS_ACCESS_EVIDENCE_DIR")
            if configured and Path(configured).resolve() != evidence.resolve():
                raise ConfigError("Metropolis access evidence is this participant's <state_root>/agent")
            try:
                evidence.mkdir(mode=0o700, parents=True, exist_ok=True)
            except OSError:
                raise ConfigError("cannot create the participant evidence directory") from None
            os.environ["EREBUS_ACCESS_EVIDENCE_DIR"] = str(evidence)
            register_access_tools(server, AccessSettings.from_env())
    return server
