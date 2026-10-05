#!/usr/bin/env python3
"""Live Monad testnet rehearsal driven only by installed Metropolis product commands.

    metropolis-monad-rehearsal.py init      --plan plan.json --workdir DIR
    metropolis-monad-rehearsal.py preflight --workdir DIR          (read-only RPC; never signs)
    metropolis-monad-rehearsal.py run       --workdir DIR --authorize-live-transactions

`init` creates separate buyer, seller, and auditor directories with fresh keys (erebus-negotiate
prepare_operator / erebus-disclosure keygen), the shared service template (prepare_terms), and
every participant configuration. Gas keys are supplied by the operator and copied owner-only.
A plan may set `reuse_from` to an existing workdir: `init` then copies that workdir's owner-only
participant keys, public descriptors, auditor key, and gas key, so a replacement agreement can
reuse already-funded addresses without repeating onboarding. No operation state, journal, or
authorization is copied, and the source directory is never modified.
`agreement_lifetime_seconds` (or its older name `delivery_window_seconds`) must cover the
configured `verification_timeout_seconds` plus a settlement/delivery margin; the negotiation
command sets the payment expiry to now plus that lifetime, so a shorter value would expire the
agreement while the history scan is still running.
`preflight` checks both RPCs, the deployment runtime and origin, key isolation, the agreement
lifetime, and funding, and prints exactly what is missing. `run` refuses without
--authorize-live-transactions; it then negotiates in separate processes, pays once (public-bound
erebus-payment with paired finalized observation, or the x402 first paid request), recovers
access across a seller restart, and has the auditor verify payment from an encrypted grant
alone. Results go to DIR/run-record.json.

Token approval has no product command yet (M8 onboarding); preflight prints it as an explicit
operator step. Nothing here is an Anvil test: every RPC in the plan is the one used.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

PERMIT2 = "0x000000000022d473030f116ddee9f6b43ac78ba3"
EXACT_PROXY = "0x402085c248eea27d92e8b30b2c58ed07f9e20001"
OPERATION_BYTES = 32


# Keccak-256 (the pre-NIST padding Ethereum uses); hashlib only has SHA3-256.
def keccak256(data: bytes) -> bytes:
    rc = [0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000, 0x000000000000808B,
          0x0000000080000001, 0x8000000080008081, 0x8000000000008009, 0x000000000000008A, 0x0000000000000088,
          0x0000000080008009, 0x000000008000000A, 0x000000008000808B, 0x800000000000008B, 0x8000000000008089,
          0x8000000000008003, 0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
          0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008]
    rot = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
    mask = (1 << 64) - 1
    rate = 136
    padded = bytearray(data) + b"\x01" + b"\x00" * ((rate - (len(data) + 1) % rate) % rate)
    padded[-1] |= 0x80
    state = [[0] * 5 for _ in range(5)]
    for offset in range(0, len(padded), rate):
        for i in range(rate // 8):
            state[i % 5][i // 5] ^= int.from_bytes(padded[offset + 8 * i:offset + 8 * i + 8], "little")
        for round_constant in rc:
            c = [state[x][0] ^ state[x][1] ^ state[x][2] ^ state[x][3] ^ state[x][4] for x in range(5)]
            d = [c[(x - 1) % 5] ^ (((c[(x + 1) % 5] << 1) | (c[(x + 1) % 5] >> 63)) & mask) for x in range(5)]
            state = [[state[x][y] ^ d[x] for y in range(5)] for x in range(5)]
            b = [[0] * 5 for _ in range(5)]
            for x in range(5):
                for y in range(5):
                    r = rot[x][y]
                    b[y][(2 * x + 3 * y) % 5] = ((state[x][y] << r) | (state[x][y] >> (64 - r))) & mask if r else state[x][y]
            state = [[b[x][y] ^ ((~b[(x + 1) % 5][y]) & b[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
            state[0][0] ^= round_constant
    return b"".join(state[i % 5][i // 5].to_bytes(8, "little") for i in range(4))


class Rehearsal(RuntimeError):
    pass


class CommandTimeout(Rehearsal):
    """One bounded command invocation was killed; durable state is retained."""


# One product-command invocation may run a bounded slice of a long history scan. The slice
# itself issues up to `max_log_queries` concurrent `eth_getLogs` queries, so the cap must
# cover the whole slice wall-clock, not one query.
CALL_SECONDS = 1800
# A pending reply that does not advance this many times means the scan is stuck.
STALL_LIMIT = 2
# Read-only polls retry transient provider errors, but fail after this long with no
# successful reply. Public Monad RPCs intermittently disagree at the moving tip; a retry
# resumes from the durable checkpoint and never submits a payment.
STALL_SECONDS = 600
# A live Monad history scan from the deployment block is bounded by the public RPC's
# 100-block `eth_getLogs` cap, so the verification phase needs more than an hour.
VERIFICATION_SECONDS = 86_400
# Time reserved inside the agreement for negotiation, settlement, inclusion, delivery, and
# audit after the verification window ends. The negotiation command sets the payment expiry
# to `now + max_deal_lifetime_seconds`, so a lifetime shorter than the scan window would
# expire the agreement before the scan finishes.
AGREEMENT_MARGIN_SECONDS = 3600
# `erebus-negotiate` accepts max_deal_lifetime_seconds in 1..=86400.
MAX_AGREEMENT_LIFETIME_SECONDS = 86_400
# Planning bound per `eth_getLogs` query. Measured live Monad public-RPC average is about
# 1.1 s; two seconds is roughly twice that, pessimistic without rejecting a sound plan.
SCAN_QUERY_SECONDS = 2
# The seller's access request is valid for two minutes; one verification slice must fit so
# the first request is not wasted. A cold scan continues across retries with fresh requests.
ACCESS_REQUEST_SECONDS = 120
# Time reserved for settlement, inclusion, and finality after the buyer's scan.
SETTLEMENT_MARGIN_SECONDS = 900
# Time reserved for the auditor's export, cold scan, and verification after payment.
AUDIT_MARGIN_SECONDS = 600
# Defaults for a live Monad scan. Concurrency is bounded by the RPC's rate limits.
DEFAULT_LOG_QUERIES = 256
# Public Monad RPCs begin answering 429 above roughly eight concurrent `eth_getLogs`
# queries (measured: monadinfra 0 errors at 8, 37% at 16). The driver retries transient
# errors, but staying under the limit is faster than absorbing it.
DEFAULT_LOG_CONCURRENCY = 8
MAX_LOG_QUERIES = 1_024
MAX_LOG_CONCURRENCY = 32
# Parent links per invocation. A live chain advances while a catch-up walks; the walk must
# outrun the chain (about 2.5 blocks/s at 0.4 s blocks) or the backlog never closes. Concurrent
# block fetches plus a large per-invocation link budget keep the walk ahead of the tip.
DEFAULT_MAX_ANCESTRY = 8192
MAX_ANCESTRY_LINKS = 8192
DEFAULT_GRANT_LIFETIME_SECONDS = 3600


def rpc(url: str, method: str, params: list) -> object:
    request = urllib.request.Request(url, json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
                                     {"content-type": "application/json", "user-agent": "erebus-rehearsal"})
    with urllib.request.urlopen(request, timeout=20) as response:
        reply = json.load(response)
    if "error" in reply:
        raise Rehearsal(f"{method} failed: {reply['error'].get('message', 'error')}")
    return reply["result"]


def call(url: str, to: str, data: str, block: str = "latest") -> int:
    return int(rpc(url, "eth_call", [{"to": to, "data": data}, block]), 16)


def word(address: str) -> str:
    return address.lower().removeprefix("0x").rjust(64, "0")


def command(bin_dir: Path, name: str, request: dict, cwd: Path, timeout: float = 900) -> tuple[int, dict]:
    binary = bin_dir / name
    if not binary.is_file():
        raise Rehearsal(f"installed command missing: {binary}")
    try:
        done = subprocess.run([str(binary)], input=json.dumps(request), capture_output=True, text=True, cwd=cwd,
                              timeout=timeout, env={"PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin"})
    except subprocess.TimeoutExpired:
        raise CommandTimeout(f"{name} timed out; retain state and recover without a new payment") from None
    try:
        reply = json.loads(done.stdout)
        if not isinstance(reply, dict):
            raise ValueError("expected a JSON object")
        return done.returncode, reply
    except ValueError:
        raise Rehearsal(f"{name} returned no JSON (exit {done.returncode})") from None


def stop_process(process: subprocess.Popen) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None:
            stream.close()


def phase_timeout(plan: dict, name: str, maximum: int = 3600) -> int:
    value = plan.get(name, 600)
    if type(value) is not int or not 1 <= value <= maximum:
        raise Rehearsal(f"{name} must be between 1 and {maximum} seconds")
    return value


def agreement_lifetime(plan: dict, verification_timeout: int) -> int:
    """The new agreement's lifetime, which the negotiation command uses as payment expiry.

    `delivery_window_seconds` remains accepted as the older name for the same value. The
    lifetime must outlast the configured verification window plus settlement and delivery,
    or the payment would expire while the history scan is still running.
    """
    value = plan.get("agreement_lifetime_seconds", plan.get("delivery_window_seconds", 6 * 3600))
    if type(value) is not int or not 1 <= value <= MAX_AGREEMENT_LIFETIME_SECONDS:
        raise Rehearsal(
            f"agreement_lifetime_seconds must be between 1 and {MAX_AGREEMENT_LIFETIME_SECONDS} seconds"
        )
    required = verification_timeout + AGREEMENT_MARGIN_SECONDS
    if value < required:
        raise Rehearsal(
            "agreement_lifetime_seconds must cover the verification window plus settlement and "
            f"delivery: at least {required} seconds for a {verification_timeout}-second window"
        )
    return value


def validate_agreement_lifetimes(workdir: Path, plan: dict, verification_timeout: int) -> int:
    """Requires both participant configs to carry the validated new-agreement lifetime.

    A config with a shorter lifetime would let the payment expire mid-scan. This never edits
    a config and never extends an agreement: a mismatch means the operator must initialize a
    replacement with the current plan.
    """
    lifetime = agreement_lifetime(plan, verification_timeout)
    for role in ("buyer", "seller"):
        try:
            config = json.loads((workdir / role / "config.json").read_text())
        except (OSError, ValueError):
            raise Rehearsal(f"{role} negotiation configuration unavailable") from None
        if config.get("max_deal_lifetime_seconds") != lifetime:
            raise Rehearsal(
                f"{role} agreement lifetime does not match the plan; initialize a replacement agreement"
            )
    return lifetime


def observer_budget(plan: dict) -> dict:
    """The bounded observer configuration shared by buyer, seller, and auditor."""
    queries = plan.get("max_log_queries", DEFAULT_LOG_QUERIES)
    concurrency = plan.get("max_concurrent_queries", DEFAULT_LOG_CONCURRENCY)
    ancestry = plan.get("max_ancestry", DEFAULT_MAX_ANCESTRY)
    if type(queries) is not int or not 1 <= queries <= MAX_LOG_QUERIES:
        raise Rehearsal(f"max_log_queries must be between 1 and {MAX_LOG_QUERIES}")
    if type(concurrency) is not int or not 1 <= concurrency <= MAX_LOG_CONCURRENCY:
        raise Rehearsal(f"max_concurrent_queries must be between 1 and {MAX_LOG_CONCURRENCY}")
    if type(ancestry) is not int or not 1 <= ancestry <= MAX_ANCESTRY_LINKS:
        raise Rehearsal(f"max_ancestry must be between 1 and {MAX_ANCESTRY_LINKS}")
    return {"log_block_range": 100, "max_log_queries": queries,
            "max_concurrent_queries": concurrency, "max_ancestry": ancestry}


def scan_budget(blocks: int, block_range: int, concurrency: int) -> int:
    """A pessimistic wall-clock bound for scanning `blocks` at the configured concurrency."""
    if blocks <= 0:
        return 0
    queries = (blocks + block_range - 1) // block_range
    return (queries * SCAN_QUERY_SECONDS + concurrency - 1) // concurrency


def validate_observation_slice(plan: dict) -> int:
    """One observation slice must fit the access-request lifetime so the first request counts."""
    budget = observer_budget(plan)
    slice_seconds = scan_budget(budget["log_block_range"] * budget["max_log_queries"],
                                budget["log_block_range"], budget["max_concurrent_queries"])
    if slice_seconds > ACCESS_REQUEST_SECONDS:
        raise Rehearsal(
            f"one observation slice needs about {slice_seconds} seconds and cannot fit a "
            f"{ACCESS_REQUEST_SECONDS}-second access request; lower max_log_queries"
        )
    return slice_seconds


def grant_lifetime(plan: dict, estimated_scan: int) -> int:
    """The auditor grant must outlast its own cold scan and verification."""
    value = plan.get("grant_lifetime_seconds", DEFAULT_GRANT_LIFETIME_SECONDS)
    if type(value) is not int or not 1 <= value <= MAX_AGREEMENT_LIFETIME_SECONDS:
        raise Rehearsal(f"grant_lifetime_seconds must be between 1 and {MAX_AGREEMENT_LIFETIME_SECONDS} seconds")
    required = estimated_scan + AUDIT_MARGIN_SECONDS
    if value < required:
        raise Rehearsal(
            f"grant_lifetime_seconds must cover the auditor scan and verification: at least {required} seconds"
        )
    return value


def validate_live_budget(record: dict, chain_head: int) -> dict:
    """Rejects a plan whose scan cannot finish before its agreement and delivery deadlines."""
    plan = record["plan"]
    budget = observer_budget(plan)
    validate_observation_slice(plan)
    estimated = scan_budget(max(0, chain_head - int(plan["deployment_block"])),
                            budget["log_block_range"], budget["max_concurrent_queries"])
    verification = phase_timeout(plan, "verification_timeout_seconds", VERIFICATION_SECONDS)
    lifetime = agreement_lifetime(plan, verification)
    if lifetime < estimated + SETTLEMENT_MARGIN_SECONDS:
        raise Rehearsal(
            f"agreement_lifetime_seconds must cover the estimated {estimated}-second scan; initialize a replacement"
        )
    now = int(time.time())
    delivery = int(record["delivery_deadline"]) - now
    required = 2 * estimated + SETTLEMENT_MARGIN_SECONDS
    if delivery < required:
        raise Rehearsal(
            f"delivery deadline leaves {delivery} seconds; the buyer and seller scans need about "
            f"{required}; initialize a replacement agreement"
        )
    grant = grant_lifetime(plan, estimated)
    return {"estimated_scan_seconds": estimated, "delivery_remaining_seconds": delivery,
            "grant_lifetime_seconds": grant, "agreement_lifetime_seconds": lifetime}


def negotiate_participants(bin_dir: Path, workdir: Path, operation_ref: str, timeout: int) -> dict:
    deadline = time.monotonic() + timeout
    processes, replies = {}, {}
    try:
        for role in ("seller", "buyer"):
            processes[role] = subprocess.Popen([str(bin_dir / "erebus-negotiate")], stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=workdir / role,
                env={"PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin"})
        # Both requests must reach their peers before either reply is awaited.
        for role, process in processes.items():
            process.stdin.write(json.dumps({"method": "negotiate", "config_file": str(workdir / role / "config.json"),
                                            "operation_ref": operation_ref}))
            process.stdin.close()
            process.stdin = None
        for role, process in processes.items():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired("erebus-negotiate", timeout)
            output, _ = process.communicate(timeout=remaining)
            try:
                reply = json.loads(output)
            except ValueError:
                raise Rehearsal(f"{role} negotiation returned invalid JSON") from None
            if process.returncode or not isinstance(reply, dict) or reply.get("status") == "error":
                raise Rehearsal(f"{role} negotiation failed; retain participant state")
            replies[role] = reply
    except subprocess.TimeoutExpired:
        raise Rehearsal("negotiation timed out; retain participant state and the same operation reference") from None
    finally:
        for process in processes.values():
            stop_process(process)
    return replies


def settle_once(bin_dir: Path, request, buyer_dir: Path, timeout: float) -> dict:
    """Runs the one settlement call and never retries it.

    A killed call or an error reply may already have signed or broadcast, so this returns a
    pending observation marker instead of raising; the caller must recover by `observe` only.
    """
    try:
        code, reply = command(bin_dir, "erebus-payment", request("settle"), buyer_dir, timeout)
    except CommandTimeout:
        return {"status": "pending", "payment_verified": False}
    if code not in (0, 2):
        return {"status": "pending", "payment_verified": False}
    return reply


def settle_with_catch_up(bin_dir: Path, request, buyer_dir: Path, verification_timeout: int) -> dict:
    """Settles once, resuming bounded pre-submission history work when that is provably safe.

    A `history_pending` reply with a durable `broadcast_attempts` of zero is pre-submission
    work: the read-only scan and ancestry walk may continue from their checkpoints and settle
    may be called again. Any recorded attempt, an ambiguous submission, or a missing durable
    diagnostic switches to observe-only forever. No path creates another agreement or payment.
    """
    deadline = time.monotonic() + verification_timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise Rehearsal("pre-settlement catch-up timed out; retain state and recover without a new payment")
        settled = settle_once(bin_dir, request, buyer_dir, min(remaining, CALL_SECONDS))
        if settled.get("payment_verified"):
            return settled
        if settled.get("history_pending") and settled.get("broadcast_attempts") == 0:
            catch_up = max(1, int(deadline - time.monotonic()))
            settled = poll_reply(
                lambda timeout: command(bin_dir, "erebus-payment", request("observe"), buyer_dir, timeout),
                lambda reply: not reply.get("history_pending"),
                catch_up, "pre-settlement catch-up", retry_errors=True, complete_pending=True)
            if settled.get("payment_verified"):
                return settled
            continue
        return settled


def pending_marker(reply: dict):
    if not reply.get("history_pending"):
        return None
    return reply.get("next_log_block"), reply.get("ancestry_block")


def poll_reply(fetch, complete, timeout: int, phase: str, retry_errors: bool = False,
               complete_pending: bool = False) -> dict:
    """Poll one bounded phase until it completes, fails, stalls, or times out.

    Each fetch is one read-only or idempotent product-command invocation. A call killed at
    `CALL_SECONDS` leaves durable checkpoints, so it is retried rather than treated as failed.
    Pending history must advance; a repeated marker fails closed. With `retry_errors`, a
    transient provider error is retried until `STALL_SECONDS` passes with no successful reply;
    an error is never completion and never permission to submit a payment. With
    `complete_pending`, a `pending` reply that satisfies `complete` stops the poll: the caller
    re-verifies the deal itself, so this only ends read-only catch-up work.
    """
    deadline = time.monotonic() + timeout
    previous, repeats = None, 0
    progress_at = time.monotonic()

    def stalled() -> bool:
        return time.monotonic() - progress_at >= STALL_SECONDS

    def fail(reason: str) -> None:
        raise Rehearsal(f"{phase} {reason}; retain state and recover without a new payment")

    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail("timed out")
        try:
            code, reply = fetch(min(remaining, CALL_SECONDS))
        except CommandTimeout:
            if stalled():
                fail("made no progress")
            time.sleep(min(2, max(0, deadline - time.monotonic())))
            continue
        status = reply.get("status")
        if code not in (0, 2) or status == "error":
            if not retry_errors or stalled():
                fail("failed")
            time.sleep(min(2, max(0, deadline - time.monotonic())))
            continue
        if status in {"closed_unpaid", "funding_required"}:
            fail("failed")
        if (code == 0 or (complete_pending and code == 2)) and complete(reply):
            return reply
        marker = pending_marker(reply)
        if marker is not None and marker == previous:
            repeats += 1
            if repeats >= STALL_LIMIT:
                fail("made no progress")
        else:
            repeats = 0
            previous = marker
            progress_at = time.monotonic()
        time.sleep(min(2, max(0, deadline - time.monotonic())))


def private(path: Path, data: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)


def gas_seed(path: Path) -> bytes:
    if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o077:
        raise Rehearsal("gas key must be an owner-only regular file")
    raw = path.read_bytes()
    if len(raw) == 32:
        return raw
    try:
        seed = bytes.fromhex(raw.decode("ascii").strip().removeprefix("0x"))
    except (UnicodeError, ValueError):
        raise Rehearsal("gas key must hold 32 raw bytes or a 32-byte hex key") from None
    if len(seed) != 32:
        raise Rehearsal("gas key must hold 32 raw bytes or a 32-byte hex key")
    return seed


def load(workdir: Path) -> dict:
    return json.loads((workdir / "rehearsal.json").read_text())


def reuse_participants(source: Path, workdir: Path, rail: str, plan: dict, namespace: str) -> tuple[dict, str]:
    """Copies owner-only participant material from a retained workdir for a replacement.

    The replacement keeps the already-funded buyer and seller addresses, so onboarding is
    not repeated. Only keys and public descriptors are copied; no operation state, journal,
    or authorization is carried over, and the source directory is never modified.
    """
    source = source.resolve()
    record = load(source)
    if record.get("rail") != rail or record.get("namespace") != namespace:
        raise Rehearsal("reused participants belong to a different rail or deployment")
    gas_role = "buyer" if rail == "public-bound" else "seller"
    current = int(time.time())
    prepared = {}
    for role in ("buyer", "seller"):
        directory = workdir / role
        directory.mkdir(mode=0o700)
        for name in ("agreement.key", "transport.key"):
            origin = source / role / name
            if origin.is_symlink() or not origin.is_file() or origin.stat().st_mode & 0o077:
                raise Rehearsal(f"reused {role}/{name} must be an owner-only regular file")
            private(directory / name, origin.read_bytes())
        for name in (f"{role}.descriptor.json", f"{'seller' if role == 'buyer' else 'buyer'}.descriptor.json"):
            origin = source / role / name
            if origin.is_symlink() or not origin.is_file():
                raise Rehearsal(f"reused {role}/{name} must be a regular descriptor file")
            shutil.copyfile(origin, directory / name)
        local = json.loads((directory / f"{role}.descriptor.json").read_text())
        address = str(record.get(role, "")).lower().removeprefix("0x")
        if len(address) != 40 or local.get("seller_address") != address:
            raise Rehearsal(f"reused {role} descriptor does not match its recorded address")
        if type(local.get("expires")) is not int or local["expires"] <= current:
            raise Rehearsal(f"reused {role} descriptor has expired")
        prepared[role] = {"descriptor_file": str(directory / f"{role}.descriptor.json"),
                          "agreement_address": "0x" + address}
    private(workdir / gas_role / "gas.key", gas_seed(source / gas_role / "gas.key"))
    (workdir / "auditor").mkdir(mode=0o700)
    auditor_key = source / "auditor" / "auditor.key"
    if auditor_key.is_symlink() or not auditor_key.is_file() or auditor_key.stat().st_mode & 0o077:
        raise Rehearsal("reused auditor/auditor.key must be an owner-only regular file")
    private(workdir / "auditor/auditor.key", auditor_key.read_bytes())
    public_key = record.get("auditor_public_key")
    if not isinstance(public_key, str) or len(public_key) != 64:
        raise Rehearsal("reused auditor public key is missing")
    return prepared, public_key


def init(plan_path: Path, workdir: Path) -> dict:
    plan = json.loads(plan_path.read_text())
    rail = plan["rail"]
    if rail not in {"public-bound", "x402-exact"}:
        raise Rehearsal("rail must be public-bound or x402-exact")
    if plan["rpc_url"].rstrip("/") == plan["peer_rpc_url"].rstrip("/"):
        raise Rehearsal("paired observation needs two distinct RPC endpoints")
    phase_timeout(plan, "negotiation_timeout_seconds")
    verification = phase_timeout(plan, "verification_timeout_seconds", VERIFICATION_SECONDS)
    lifetime = agreement_lifetime(plan, verification)
    validate_observation_slice(plan)
    bin_dir = Path(plan["bin_dir"]).resolve()
    workdir.mkdir(mode=0o700)
    namespace = f"eip155:{plan['chain_id']}"
    asset = f"{namespace}/erc20:{plan['asset_contract'].lower()}"
    contract = plan["settlement_contract"].lower() if rail == "public-bound" else EXACT_PROXY
    endpoint = plan["negotiation_endpoint"]
    gas_role = "buyer" if rail == "public-bound" else "seller"
    if plan.get("reuse_from") is None:
        prepared = {}
        for role in ("buyer", "seller"):
            code, reply = command(bin_dir, "erebus-negotiate", {"method": "prepare_operator", "directory": str(workdir / role),
                                  "role": role, "endpoint": endpoint, "namespace": namespace, "assets": [asset],
                                  "descriptor_lifetime_seconds": 7 * 86400}, workdir)
            if code:
                raise Rehearsal(f"prepare_operator {role}: {reply.get('error')}")
            prepared[role] = reply
        (workdir / "auditor").mkdir(mode=0o700)
        code, auditor = command(bin_dir, "erebus-shielded-disclosure", {"method": "keygen", "key_file": str(workdir / "auditor/auditor.key")}, workdir)
        if code:
            raise Rehearsal(f"auditor keygen: {auditor.get('error')}")
        auditor_public_key = auditor["recipient_public_key"]
        private(workdir / gas_role / "gas.key", gas_seed(Path(plan["gas_key_file"])))
    else:
        prepared, auditor_public_key = reuse_participants(Path(plan["reuse_from"]), workdir, rail, plan, namespace)
    payload = Path(plan["payload_file"]).read_bytes()
    import hashlib
    digest = hashlib.sha256(payload).hexdigest()
    deadline = int(time.time()) + lifetime
    for role, peer, price in (("buyer", "seller", plan["buyer_start_price"]), ("seller", "buyer", plan["seller_price"])):
        directory = workdir / role
        peer_copy = directory / f"{peer}.descriptor.json"
        shutil.copyfile(prepared[peer]["descriptor_file"], peer_copy)
        code, reply = command(bin_dir, "erebus-negotiate", {"method": "prepare_terms", "output": str(directory / "service.terms"),
                              "role": role, "local_descriptor_file": prepared[role]["descriptor_file"],
                              "peer_descriptor_file": str(peer_copy), "settlement_contract": contract,
                              "verifier_version": int(plan.get("verifier_version", 1)), "asset": asset, "amount": str(price),
                              "resource": plan["resource"], "unit": "snapshot", "quantity": "1",
                              "fulfillment_method": "http-access-v1", "fulfillment_digest": digest,
                              "delivery_deadline": deadline}, workdir)
        if code:
            raise Rehearsal(f"prepare_terms {role}: {reply.get('error')}")
        config = {"version": 1, "role": role, "state_root": str(directory / "state"),
                  "transport_key_file": str(directory / "transport.key"), "agreement_key_file": str(directory / "agreement.key"),
                  "discovery_key_file": None, "local_descriptor_file": prepared[role]["descriptor_file"],
                  "peer_descriptor_file": str(peer_copy), "terms_template_file": str(directory / "service.terms"),
                  "endpoint": endpoint, "maximum_price": str(plan["buyer_maximum_price"]),
                  "minimum_price": str(plan["seller_price"]), "max_deal_lifetime_seconds": lifetime, "timeout_seconds": 300,
                  "seller_spend_secret_file": None, "seller_wallet_file": None, "seller_wallet_key_file": None}
        if role == "seller":
            (directory / "access-evidence").mkdir(mode=0o700)
            config["access_evidence_root"] = str(directory / "access-evidence")
            shutil.copyfile(plan["payload_file"], directory / "payload")
        private(directory / "config.json", json.dumps(config).encode())
    record = {"version": 1, "rail": rail, "plan": plan, "namespace": namespace, "asset": asset, "contract": contract,
              "bin_dir": str(bin_dir), "fulfillment_digest": digest, "delivery_deadline": deadline,
              "operation_ref": secrets.token_hex(OPERATION_BYTES), "service_id": secrets.token_hex(32),
              "buyer": prepared["buyer"]["agreement_address"], "seller": prepared["seller"]["agreement_address"],
              "gas_role": gas_role, "auditor_public_key": auditor_public_key}
    (workdir / "rehearsal.json").write_text(json.dumps(record, indent=2) + "\n")
    return {"status": "initialized", "workdir": str(workdir), "rail": rail, "buyer": record["buyer"], "seller": record["seller"],
            "next": "fund and approve as printed by preflight, then run with --authorize-live-transactions"}


def gas_address(workdir: Path, record: dict) -> str:
    code, reply = command(Path(record["bin_dir"]), "erebus-settle", {"method": "address", "key_file": str(workdir / record["gas_role"] / "gas.key")}, workdir)
    if code or "address" not in reply:
        raise Rehearsal("cannot derive the gas account address from its key file")
    return reply["address"].lower()


def runtime(record: dict) -> dict:
    plan = record["plan"]
    hashes = {}
    targets = [("settlement", record["contract"])] if record["rail"] == "public-bound" else [("permit2", PERMIT2), ("proxy", EXACT_PROXY)]
    for name, address in targets:
        codes = {url: rpc(url, "eth_getCode", [address, "finalized"]) for url in (plan["rpc_url"], plan["peer_rpc_url"])}
        if len(set(codes.values())) != 1 or next(iter(codes.values())) in ("0x", "0x0"):
            raise Rehearsal(f"{name} runtime is missing or differs between RPCs")
        hashes[name] = "0x" + keccak256(bytes.fromhex(next(iter(codes.values()))[2:])).hex()
    return hashes


def preflight(workdir: Path) -> dict:
    record = load(workdir)
    plan = record["plan"]
    verification = phase_timeout(plan, "verification_timeout_seconds", VERIFICATION_SECONDS)
    lifetime = validate_agreement_lifetimes(workdir, plan, verification)
    bin_dir = Path(record["bin_dir"])
    missing, checks = [], {}
    for label, url in (("rpc", plan["rpc_url"]), ("peer_rpc", plan["peer_rpc_url"])):
        code, reply = command(bin_dir, "erebus-network-check", {"chain_id": plan["chain_id"], "rpc_url": url, "timeout_ms": 15000}, workdir, 120)
        checks[label] = "ok" if code == 0 else reply.get("error", "failed")
        if code:
            missing.append(f"{label} failed erebus-network-check")
    checks["runtime_keccak256"] = runtime(record)
    if record["rail"] == "public-bound":
        origin = int(plan["deployment_block"])
        before = rpc(plan["rpc_url"], "eth_getCode", [record["contract"], hex(origin - 1)])
        at = rpc(plan["rpc_url"], "eth_getCode", [record["contract"], hex(origin)])
        if before not in ("0x", "0x0") or at in ("0x", "0x0"):
            missing.append("deployment_block is not the contract's first block")
        head = int(rpc(plan["rpc_url"], "eth_getBlockByNumber", ["finalized", False])["number"], 16)
        checks["chain_finalized"] = head
        checks["live_budget"] = validate_live_budget(record, head)
    for role in ("buyer", "seller"):
        for name in ("agreement.key", "transport.key", "config.json"):
            if (workdir / role / name).stat().st_mode & 0o077:
                missing.append(f"{role}/{name} is not owner-only")
    gas = gas_address(workdir, record)
    if gas in (record["buyer"], record["seller"]):
        missing.append("the gas account must differ from both agreement keys")
    price = int(plan["buyer_maximum_price"])
    spender = record["contract"] if record["rail"] == "public-bound" else PERMIT2
    balance = call(plan["rpc_url"], plan["asset_contract"], "0x70a08231" + word(record["buyer"]))
    allowance = call(plan["rpc_url"], plan["asset_contract"], "0xdd62ed3e" + word(record["buyer"]) + word(spender))
    native = {who: int(rpc(plan["rpc_url"], "eth_getBalance", [address, "latest"]), 16)
              for who, address in (("buyer", record["buyer"]), ("gas", gas))}
    checks.update({"buyer": record["buyer"], "seller": record["seller"], "gas_account": gas, "token_balance": balance,
                   "allowance": allowance, "allowance_spender": spender, "native_wei": native,
                   "verification_timeout_seconds": verification, "agreement_lifetime_seconds": lifetime})
    if balance < price:
        missing.append(f"mint or transfer at least {price} base units of {plan['asset_contract']} to buyer {record['buyer']}")
    if allowance < price:
        missing.append(f"approve {spender} for {price} base units from buyer {record['buyer']} using an operator wallet "
                       "(no product approval command yet; the agreement.key file holds raw bytes, not a cast --private-key argument)")
    if native["buyer"] == 0 and allowance < price:
        missing.append(f"buyer {record['buyer']} needs native gas only to send that approval")
    if native["gas"] < int(plan.get("minimum_gas_wei", 10**17)):
        missing.append(f"fund gas account {gas} with native MON for settlement")
    checks["approval_intent"] = {"chain_id": plan["chain_id"], "token": plan["asset_contract"], "owner": record["buyer"],
                                 "spender": spender, "amount_base_units": str(price),
                                 "calldata": "0x095ea7b3" + word(spender) + f"{price:064x}"}
    return {"status": "ready" if not missing else "missing_prerequisites", "rail": record["rail"], "checks": checks,
            "missing": missing, "live_transactions_sent": 0}


def run(workdir: Path) -> dict:
    record = load(workdir)
    ready = preflight(workdir)
    if ready["missing"]:
        raise Rehearsal("preflight is not ready: " + "; ".join(ready["missing"]))
    plan, bin_dir, rail = record["plan"], Path(record["bin_dir"]), record["rail"]
    gas = ready["checks"]["gas_account"]
    verification_timeout = phase_timeout(plan, "verification_timeout_seconds", VERIFICATION_SECONDS)
    validate_agreement_lifetimes(workdir, plan, verification_timeout)
    stages, result = {}, {"rail": rail, "operation_ref": record["operation_ref"]}
    started = time.monotonic()
    replies = negotiate_participants(bin_dir, workdir, record["operation_ref"],
                                      phase_timeout(plan, "negotiation_timeout_seconds"))
    stages["negotiation_ms"] = round((time.monotonic() - started) * 1000)
    buyer, seller = replies["buyer"], replies["seller"]
    if buyer["deal_commitment"] != seller["deal_commitment"]:
        raise Rehearsal("participants authorized different deals")
    result["deal_commitment"] = buyer["deal_commitment"]
    hashes = ready["checks"]["runtime_keccak256"]
    seller_dir, buyer_dir = workdir / "seller", workdir / "buyer"
    port = int(plan["access_port"])
    seller_key = bytes.fromhex(record["seller"][2:])
    service_config = {"service_id": list(bytes.fromhex(record["service_id"])), "seller_key": list(seller_key), "suite_id": 1,
                      "resource": plan["resource"], "payload_file": str(seller_dir / "payload"),
                      "evidence_root": str(seller_dir / "access-evidence"), "state_root": str(seller_dir / "access-state"), "port": port}
    observer = observer_budget(plan)
    if rail == "public-bound":
        service_config["backend"] = {"mode": "public_bound", "namespace": record["namespace"], "settlement_contract": record["contract"],
                                     "verifier_version": 1, "rpc_url": plan["peer_rpc_url"], "from_block": int(plan["deployment_block"]), **observer}
        first_hash = rpc(plan["rpc_url"], "eth_getBlockByNumber", [hex(int(plan["deployment_block"])), False])["hash"]
        payment = {"version": 1, "state_root": str(buyer_dir / "state"), "namespace": record["namespace"],
                   "settlement_contract": record["contract"], "verifier_version": 1, "runtime_keccak256": hashes["settlement"],
                   "first_block": int(plan["deployment_block"]), "first_hash": first_hash, "rpc_url": plan["rpc_url"],
                   "peer_rpc_url": plan["peer_rpc_url"], "buyer_address": record["buyer"], "asset": record["asset"],
                   "signer_address": gas, "signer_journal_root": str(buyer_dir / "signer"), "transaction_key_file": str(buyer_dir / "gas.key"),
                   "maximum_price": str(plan["buyer_maximum_price"]), "gas_limit": int(plan.get("gas_limit", 500000)),
                   "max_fee_per_gas": str(plan["max_fee_per_gas"]), "max_priority_fee_per_gas": str(plan["max_priority_fee_per_gas"]),
                   "timeout_seconds": 20, **observer}
        if (buyer_dir / "payment.json").exists():
            # A retained config with a smaller observation budget would silently make the
            # scan far slower than the plan validated. Never edit it here; ask the operator.
            existing = json.loads((buyer_dir / "payment.json").read_text())
            for field, value in observer.items():
                if existing.get(field) != value:
                    raise Rehearsal(
                        f"buyer payment configuration {field} does not match the plan; "
                        "update the observation budget before running"
                    )
        else:
            private(buyer_dir / "payment.json", json.dumps(payment).encode())
        request = lambda method: {"method": method, "config_file": str(buyer_dir / "payment.json"), "operation_ref": record["operation_ref"]}
        poll_reply(lambda timeout: command(bin_dir, "erebus-payment", request("funding"), buyer_dir, timeout),
                   lambda reply: reply.get("status") == "ready" or reply.get("payment_verified") is True,
                   verification_timeout, "funding and retained history", retry_errors=True)
        started = time.monotonic()
        settled = settle_with_catch_up(bin_dir, request, buyer_dir, verification_timeout)
        stages["settle_call_ms"] = round((time.monotonic() - started) * 1000)
        result["settle"] = {k: settled.get(k) for k in ("status", "stage", "transaction_hash", "submitted_this_call")}
        if not settled.get("payment_verified"):
            settled = poll_reply(lambda timeout: command(bin_dir, "erebus-payment", request("observe"), buyer_dir, timeout),
                                 lambda reply: reply.get("payment_verified") is True, verification_timeout,
                                 "finality observation", retry_errors=True)
        stages["submission_to_paired_finalized_ms"] = round((time.monotonic() - started) * 1000)
        transaction = settled.get("locally_signed_transaction_hash")
        result["payment_observed_by_buyer"] = {"stage": settled["stage"], "transaction": transaction}
    else:
        service_config["backend"] = {"mode": "x402_exact", "namespace": record["namespace"], "rpc_url": plan["rpc_url"],
                                     "peer_rpc_url": plan["peer_rpc_url"],
                                     "permit2_runtime_hash": list(bytes.fromhex(hashes["permit2"][2:])),
                                     "proxy_runtime_hash": list(bytes.fromhex(hashes["proxy"][2:])),
                                     "transaction_key_file": str(seller_dir / "gas.key"), "signer_journal_root": str(seller_dir / "signer"),
                                     "gas_limit": int(plan.get("gas_limit", 500000)), "max_fee_per_gas": str(plan["max_fee_per_gas"]),
                                     "max_priority_fee_per_gas": str(plan["max_priority_fee_per_gas"])}
    config_path = seller_dir / "access.json"
    if not config_path.exists():
        private(config_path, json.dumps(service_config).encode())

    def service() -> subprocess.Popen:
        process = subprocess.Popen([str(bin_dir / "erebus-access-service")], env={"EREBUS_ACCESS_CONFIG": str(config_path),
                                   "PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin"}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=1)
                return process
            except OSError:
                time.sleep(0.2)
        stop_process(process)
        raise Rehearsal("access service did not become healthy")

    retrieval = {"method": "retrieve", "evidence_file": buyer["evidence_file"], "buyer_key_file": str(buyer_dir / "agreement.key"),
                 "service_url": f"http://127.0.0.1:{port}/v1/access", "service_id": record["service_id"],
                 "cache_root": str(buyer_dir / "access-cache"), "allow_loopback_http": True}
    if rail == "x402-exact":
        retrieval["x402_exact"] = True
    process = service()
    started = time.monotonic()
    attempts, first_status = 0, None
    try:
        # The first x402 request authorizes the one payment; the seller is restarted after it.
        code, retrieved = command(bin_dir, "erebus-access", retrieval, buyer_dir)
        first_status = retrieved.get("status")
        stop_process(process)
        process = service()
        def retry_access(timeout):
            nonlocal attempts
            attempts += 1
            return command(bin_dir, "erebus-access", retrieval, buyer_dir, timeout)
        retrieved = poll_reply(retry_access, lambda reply: reply.get("status") == "retrieved",
                               verification_timeout, "access retrieval", retry_errors=True)
    finally:
        stop_process(process)
    stages["first_request_to_resource_ms"] = round((time.monotonic() - started) * 1000)
    receipt = retrieved["result"]
    result["access"] = {"first_status": first_status, "attempts_after_restart": attempts,
                        "resource_verified": receipt["resource_verified"], "resource_sha256": receipt["resource_sha256"],
                        "seller_reported_payment_finalized": receipt.get("seller_reported_payment_finalized"),
                        "payment_verified": receipt["payment_verified"]}
    if rail == "x402-exact":
        # The seller operator reads its own durable journal: the exact transaction it signed.
        payment = json.loads((seller_dir / "access-state" / "x402-payments" / f"{result['deal_commitment']}.json").read_text())
        if not payment.get("raw") or not payment.get("attempted"):
            raise Rehearsal("seller journal holds no submitted x402 transaction")
        transaction = "0x" + keccak256(bytes(payment["raw"])).hex()
    grant_seconds = ready["checks"]["live_budget"]["grant_lifetime_seconds"]
    code, exported = command(bin_dir, "erebus-shielded-disclosure", {"method": "export", "evidence_file": seller["evidence_file"],
                             "issuer_key_file": str(seller_dir / "agreement.key"), "recipient_public_key": record["auditor_public_key"],
                             "grant_file": str(workdir / "auditor/deal.grant"), "expires_at": int(time.time()) + grant_seconds}, seller_dir)
    if code:
        raise Rehearsal(f"grant export: {exported.get('error')}")
    if rail == "public-bound":
        deployment = {"namespace": record["namespace"], "settlement_contract": record["contract"], "verifier_version": 1,
                      "rpc_url": plan["peer_rpc_url"], "from_block": int(plan["deployment_block"]), **observer,
                      "cache_root": str(workdir / "auditor/public-history")}
    else:
        deployment = {"rail": "x402_exact", "namespace": record["namespace"], "rpc_url": plan["rpc_url"],
                      "peer_rpc_url": plan["peer_rpc_url"], "permit2_runtime_hash": hashes["permit2"],
                      "proxy_runtime_hash": hashes["proxy"], "transaction_hash": transaction}
    auditor_cli = "erebus-shielded-disclosure"
    verified = poll_reply(lambda timeout: command(bin_dir, auditor_cli,
                          {"method": "verify_payment", "grant_file": str(workdir / "auditor/deal.grant"),
                           "key_file": str(workdir / "auditor/auditor.key"), "expected_issuer": record["seller"],
                           "deployment": deployment}, workdir / "auditor", timeout),
                          lambda reply: reply.get("payment_verified") is True, verification_timeout,
                          "auditor verification", retry_errors=True)
    result["auditor"] = {k: verified.get(k) for k in ("agreement_verified", "payment_verified", "delivery_verified", "deal_commitment")}
    result["stages_ms"] = stages
    result["settlement_transaction"] = transaction
    (workdir / "run-record.json").write_text(json.dumps(result, indent=2) + "\n")
    if not (verified.get("payment_verified") and verified.get("deal_commitment") == result["deal_commitment"]):
        raise Rehearsal("the independent auditor did not verify this deal's payment")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("phase", choices=["init", "preflight", "run"])
    parser.add_argument("--workdir", type=Path, required=True)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--authorize-live-transactions", action="store_true")
    args = parser.parse_args()
    try:
        if args.phase == "init":
            if not args.plan:
                parser.error("init requires --plan")
            output = init(args.plan, args.workdir.resolve())
        elif args.phase == "preflight":
            output = preflight(args.workdir.resolve())
        else:
            if not args.authorize_live_transactions:
                parser.error("run broadcasts real Monad transactions; pass --authorize-live-transactions only with authorization")
            output = run(args.workdir.resolve())
    except Rehearsal as error:
        print(json.dumps({"status": "error", "error": str(error)}))
        raise SystemExit(1) from None
    print(json.dumps(output, indent=2))
    if output.get("status") == "missing_prerequisites":
        raise SystemExit(2)


if __name__ == "__main__":
    main()
