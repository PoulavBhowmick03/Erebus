"""Scoped disclosure of one settled deal, through the Metropolis disclosure MCP servers.

An issuer (a participant) selects the deal from its durable state and exports a grant encrypted
to the auditor's public key. The auditor, holding only that grant and its own key, verifies the
agreement offline and the payment against chain data. Payment verification can stay pending for
a long time on a public RPC, which is not evidence of non-payment. Delivery is never verified,
and a result that claims it is rejected.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import sys
import time
from contextlib import AsyncExitStack
from pathlib import Path
from typing import Any, Awaitable, Callable

from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus_agents.metropolis_loop import HarnessError, ToolSession, _structured

FLAGS = ("agreement_verified", "payment_verified", "delivery_verified", "deal_commitment")
ISSUER_TOOLS = {"select_deal_disclosure", "export_deal_disclosure"}
AUDITOR_TOOLS = {"create_disclosure_key", "disclosure_key_info", "verify_disclosed_agreement"}


def _result(reply: dict[str, Any], failure: str) -> dict[str, Any]:
    if not reply.get("ok"):
        raise HarnessError(failure)
    return reply["result"]


async def drive_audit(auditor: ToolSession, grant_name: str, expected_issuer: str, *,
                      verify_payment: bool = False, expected_commitment: str | None = None,
                      poll_seconds: float = 5.0, max_polls: int = 120,
                      sleep: Callable[[float], Awaitable[None]] = asyncio.sleep) -> dict[str, Any]:
    arguments = {"grant_name": grant_name, "expected_issuer": expected_issuer}
    result = _result(_structured(await auditor.call_tool("verify_disclosed_agreement", arguments)),
                     "grant did not verify as an agreement")
    if result.get("agreement_verified") is not True:
        raise HarnessError("grant did not verify as an agreement")
    if verify_payment:
        for attempt in range(max_polls):
            reply = _structured(await auditor.call_tool("verify_disclosed_payment", arguments))
            if reply.get("ok") and reply["result"].get("payment_verified") is True:
                result = reply["result"]
                break
            if attempt + 1 < max_polls:
                await sleep(poll_seconds)
        else:
            raise HarnessError("payment not verified within the polling budget; pending is not evidence of non-payment")
    if result.get("delivery_verified"):
        raise HarnessError("disclosure never proves delivery")
    if expected_commitment is not None and result.get("deal_commitment") != expected_commitment:
        raise HarnessError("disclosed deal is not the negotiated deal")
    return {flag: result.get(flag) for flag in FLAGS}


async def auditor_public_key(auditor: ToolSession) -> str:
    """Recover the auditor's key from its file, creating it only when none exists."""
    reply = _structured(await auditor.call_tool("disclosure_key_info", {}))
    if not reply.get("ok"):
        reply = _structured(await auditor.call_tool("create_disclosure_key", {}))
    return _result(reply, "auditor key unavailable")["recipient_public_key"]


async def issue_grant(issuer: ToolSession, operation_ref: str, recipient_public_key: str, *,
                      evidence_name: str, grant_name: str, grant_seconds: int,
                      clock: Callable[[], float] = time.time) -> None:
    _result(_structured(await issuer.call_tool(
        "select_deal_disclosure", {"operation_ref": operation_ref, "evidence_name": evidence_name})),
        "issuer could not select this deal from its durable state")
    _result(_structured(await issuer.call_tool("export_deal_disclosure", {
        "evidence_name": evidence_name, "recipient_public_key": recipient_public_key,
        "grant_name": grant_name, "expires_at": int(clock()) + grant_seconds})),
        "issuer could not export the grant")


def disclosure_params(python: str, directory: str, *, cli: str | None = None, deployment: dict[str, Any] | None = None,
                      issuer: dict[str, str] | None = None, server_command: str | None = None) -> StdioServerParameters:
    """`issuer` holds state_root, store_root, namespace and issuer_key_file; the auditor has none."""
    env = {"EREBUS_BACKEND": "disclosure", "EREBUS_DISCLOSURE_DIR": directory}
    if cli:
        env["EREBUS_DISCLOSURE_CLI"] = cli
    if deployment is not None:
        env["EREBUS_DISCLOSURE_DEPLOYMENT"] = json.dumps(deployment)
    for name, value in (issuer or {}).items():
        env[f"EREBUS_DISCLOSURE_{name.upper()}"] = value
    if server_command:
        env["PATH"] = os.pathsep.join([str(Path(server_command).parent), "/usr/bin", "/bin"])
        return StdioServerParameters(command=server_command, args=[], env=env)
    return StdioServerParameters(command=python, args=["-m", "erebus_mcp.server"], env=env)


async def run_disclosure(issuer_params: StdioServerParameters, auditor_params: StdioServerParameters,
                         operation_ref: str, expected_issuer: str, *, issuer_dir: Path, auditor_dir: Path,
                         grant_name: str = "deal.grant", verify_payment: bool = False,
                         expected_commitment: str | None = None, grant_seconds: int = 3600,
                         **options: Any) -> dict[str, Any]:
    async with AsyncExitStack() as stack:
        sessions = []
        for params, allowed in ((issuer_params, ISSUER_TOOLS), (auditor_params, AUDITOR_TOOLS)):
            read, write = await stack.enter_async_context(stdio_client(params))
            session = await stack.enter_async_context(ClientSession(read, write))
            await session.initialize()
            tools = {tool.name for tool in (await session.list_tools()).tools}
            if not allowed <= tools:
                raise HarnessError(f"unexpected tool surface: {sorted(tools)}")
            sessions.append(session)
        issuer, auditor = sessions
        key = await auditor_public_key(auditor)
        await issue_grant(issuer, operation_ref, key, evidence_name=f"{operation_ref}.evidence",
                          grant_name=grant_name, grant_seconds=grant_seconds)
        # The grant is the only artifact that crosses from issuer to auditor.
        shutil.copyfile(issuer_dir / grant_name, auditor_dir / grant_name)
        os.chmod(auditor_dir / grant_name, 0o600)
        report = await drive_audit(auditor, grant_name, expected_issuer, verify_payment=verify_payment,
                                   expected_commitment=expected_commitment, **options)
    return {"operation_ref": operation_ref, "auditor": report}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--operation", required=True)
    parser.add_argument("--expected-issuer", required=True, help="the issuing participant's identity, obtained independently")
    parser.add_argument("--expected-commitment", help="deal commitment printed by the deal run")
    parser.add_argument("--issuer-dir", type=Path, required=True)
    parser.add_argument("--auditor-dir", type=Path, required=True)
    parser.add_argument("--state-root", required=True)
    parser.add_argument("--store-root", required=True)
    parser.add_argument("--namespace", required=True)
    parser.add_argument("--issuer-key-file", required=True)
    parser.add_argument("--disclosure-cli")
    parser.add_argument("--server-command", help="installed erebus-mcp-server; default: this Python's module")
    parser.add_argument("--deployment", type=Path, help="JSON file: the deployment the auditor verifies payment against")
    parser.add_argument("--grant-seconds", type=int, default=3600)
    parser.add_argument("--max-polls", type=int, default=120)
    args = parser.parse_args()
    deployment = json.loads(args.deployment.read_text()) if args.deployment else None
    issuer = {"state_root": args.state_root, "store_root": args.store_root,
              "namespace": args.namespace, "issuer_key_file": args.issuer_key_file}
    common = {"cli": args.disclosure_cli, "server_command": args.server_command}
    try:
        report = asyncio.run(run_disclosure(
            disclosure_params(sys.executable, str(args.issuer_dir), issuer=issuer, **common),
            disclosure_params(sys.executable, str(args.auditor_dir), deployment=deployment, **common),
            args.operation, args.expected_issuer, issuer_dir=args.issuer_dir, auditor_dir=args.auditor_dir,
            verify_payment=deployment is not None, expected_commitment=args.expected_commitment,
            grant_seconds=args.grant_seconds, max_polls=args.max_polls))
    except BaseException as error:
        from erebus_agents.metropolis_loop import _harness_error

        found = _harness_error(error)
        if found is None:
            raise
        print(json.dumps({"payment_verified": False, "error": str(found)}))
        raise SystemExit(1) from None
    print(json.dumps(report))


if __name__ == "__main__":
    main()
