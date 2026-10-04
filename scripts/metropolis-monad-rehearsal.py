#!/usr/bin/env python3
"""Live Monad testnet rehearsal driven only by installed Metropolis product commands.

    metropolis-monad-rehearsal.py init      --plan plan.json --workdir DIR
    metropolis-monad-rehearsal.py preflight --workdir DIR          (read-only RPC; never signs)
    metropolis-monad-rehearsal.py run       --workdir DIR --authorize-live-transactions

`init` creates separate buyer, seller, and auditor directories with fresh keys (erebus-negotiate
prepare_operator / erebus-disclosure keygen), the shared service template (prepare_terms), and
every participant configuration. Gas keys are supplied by the operator and copied owner-only.
`preflight` checks both RPCs, the deployment runtime and origin, key isolation, and funding, and
prints exactly what is missing. `run` refuses without --authorize-live-transactions; it then
negotiates in separate processes, pays once (public-bound erebus-payment with paired finalized
observation, or the x402 first paid request), recovers access across a seller restart, and has
the auditor verify payment from an encrypted grant alone. Results go to DIR/run-record.json.

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


def command(bin_dir: Path, name: str, request: dict, cwd: Path, timeout: int = 900) -> tuple[int, dict]:
    binary = bin_dir / name
    if not binary.is_file():
        raise Rehearsal(f"installed command missing: {binary}")
    done = subprocess.run([str(binary)], input=json.dumps(request), capture_output=True, text=True, cwd=cwd,
                          timeout=timeout, env={"PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin"})
    try:
        return done.returncode, json.loads(done.stdout)
    except ValueError:
        raise Rehearsal(f"{name} returned no JSON (exit {done.returncode})") from None


def private(path: Path, data: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)


def load(workdir: Path) -> dict:
    return json.loads((workdir / "rehearsal.json").read_text())


def init(plan_path: Path, workdir: Path) -> dict:
    plan = json.loads(plan_path.read_text())
    rail = plan["rail"]
    if rail not in {"public-bound", "x402-exact"}:
        raise Rehearsal("rail must be public-bound or x402-exact")
    if plan["rpc_url"].rstrip("/") == plan["peer_rpc_url"].rstrip("/"):
        raise Rehearsal("paired observation needs two distinct RPC endpoints")
    bin_dir = Path(plan["bin_dir"]).resolve()
    workdir.mkdir(mode=0o700)
    namespace = f"eip155:{plan['chain_id']}"
    asset = f"{namespace}/erc20:{plan['asset_contract'].lower()}"
    contract = plan["settlement_contract"].lower() if rail == "public-bound" else EXACT_PROXY
    endpoint = plan["negotiation_endpoint"]
    prepared = {}
    for role in ("buyer", "seller"):
        code, reply = command(bin_dir, "erebus-negotiate", {"method": "prepare_operator", "directory": str(workdir / role),
                              "role": role, "endpoint": endpoint, "namespace": namespace, "assets": [asset],
                              "descriptor_lifetime_seconds": 7 * 86400}, workdir)
        if code:
            raise Rehearsal(f"prepare_operator {role}: {reply.get('error')}")
        prepared[role] = reply
    payload = Path(plan["payload_file"]).read_bytes()
    import hashlib
    digest = hashlib.sha256(payload).hexdigest()
    deadline = int(time.time()) + int(plan.get("delivery_window_seconds", 6 * 3600))
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
                  "minimum_price": str(plan["seller_price"]), "max_deal_lifetime_seconds": 3600, "timeout_seconds": 300,
                  "seller_spend_secret_file": None, "seller_wallet_file": None, "seller_wallet_key_file": None}
        if role == "seller":
            (directory / "access-evidence").mkdir(mode=0o700)
            config["access_evidence_root"] = str(directory / "access-evidence")
            shutil.copyfile(plan["payload_file"], directory / "payload")
        private(directory / "config.json", json.dumps(config).encode())
    (workdir / "auditor").mkdir(mode=0o700)
    code, auditor = command(bin_dir, "erebus-shielded-disclosure", {"method": "keygen", "key_file": str(workdir / "auditor/auditor.key")}, workdir)
    if code:
        raise Rehearsal(f"auditor keygen: {auditor.get('error')}")
    gas_role = "buyer" if rail == "public-bound" else "seller"
    gas_seed = Path(plan["gas_key_file"]).read_bytes()
    if len(gas_seed) != 32:
        raise Rehearsal("gas key file must hold 32 raw bytes")
    private(workdir / gas_role / "gas.key", gas_seed)
    record = {"version": 1, "rail": rail, "plan": plan, "namespace": namespace, "asset": asset, "contract": contract,
              "bin_dir": str(bin_dir), "fulfillment_digest": digest, "delivery_deadline": deadline,
              "operation_ref": secrets.token_hex(OPERATION_BYTES), "service_id": secrets.token_hex(32),
              "buyer": prepared["buyer"]["agreement_address"], "seller": prepared["seller"]["agreement_address"],
              "gas_role": gas_role, "auditor_public_key": auditor["recipient_public_key"]}
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
                   "allowance": allowance, "allowance_spender": spender, "native_wei": native})
    if balance < price:
        missing.append(f"mint or transfer at least {price} base units of {plan['asset_contract']} to buyer {record['buyer']}")
    if allowance < price:
        missing.append(f"approve {spender} for at least {price} from buyer {record['buyer']} (no product command yet), e.g. "
                       f"cast send {plan['asset_contract']} 'approve(address,uint256)' {spender} {price} --private-key <buyer workdir/buyer/agreement.key> --rpc-url {plan['rpc_url']}")
    if native["buyer"] == 0 and allowance < price:
        missing.append(f"buyer {record['buyer']} needs native gas only to send that approval")
    if native["gas"] < int(plan.get("minimum_gas_wei", 10**17)):
        missing.append(f"fund gas account {gas} with native MON for settlement")
    return {"status": "ready" if not missing else "missing_prerequisites", "rail": record["rail"], "checks": checks,
            "missing": missing, "live_transactions_sent": 0}


def run(workdir: Path) -> dict:
    record = load(workdir)
    ready = preflight(workdir)
    if ready["missing"]:
        raise Rehearsal("preflight is not ready: " + "; ".join(ready["missing"]))
    plan, bin_dir, rail = record["plan"], Path(record["bin_dir"]), record["rail"]
    gas = ready["checks"]["gas_account"]
    stages, result = {}, {"rail": rail, "operation_ref": record["operation_ref"]}
    negotiate = lambda role: subprocess.Popen([str(bin_dir / "erebus-negotiate")], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        text=True, cwd=workdir / role, env={"PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin"})
    started = time.monotonic()
    processes = {role: negotiate(role) for role in ("seller", "buyer")}
    # Both requests go out before either reply is awaited: each process needs its peer running.
    for role, process in processes.items():
        process.stdin.write(json.dumps({"method": "negotiate", "config_file": str(workdir / role / "config.json"),
                                        "operation_ref": record["operation_ref"]}))
        process.stdin.close()
    replies = {}
    for role, process in processes.items():
        replies[role] = json.loads(process.stdout.read())
        process.wait(timeout=600)
        if process.returncode:
            raise Rehearsal(f"{role} negotiation: {replies[role].get('error')}")
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
    observer = {"log_block_range": 100, "max_log_queries": 1024, "max_ancestry": 8192}
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
        if not (buyer_dir / "payment.json").exists():
            private(buyer_dir / "payment.json", json.dumps(payment).encode())
        request = lambda method: {"method": method, "config_file": str(buyer_dir / "payment.json"), "operation_ref": record["operation_ref"]}
        code, funding = command(bin_dir, "erebus-payment", request("funding"), buyer_dir)
        if code or funding.get("status") != "ready":
            raise Rehearsal(f"funding not ready: {funding.get('funding') or funding.get('error')}")
        started = time.monotonic()
        code, settled = command(bin_dir, "erebus-payment", request("settle"), buyer_dir)
        stages["settle_call_ms"] = round((time.monotonic() - started) * 1000)
        result["settle"] = {k: settled.get(k) for k in ("status", "stage", "transaction_hash", "submitted_this_call")}
        while not settled.get("payment_verified"):
            if settled.get("status") == "closed_unpaid":
                raise Rehearsal("deal closed unpaid")
            time.sleep(2)
            code, settled = command(bin_dir, "erebus-payment", request("observe"), buyer_dir)
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
        process.kill()
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
        process.kill()
        process.wait()
        process = service()
        while True:
            attempts += 1
            code, retrieved = command(bin_dir, "erebus-access", retrieval, buyer_dir)
            if code == 0 and retrieved.get("status") == "retrieved":
                break
            if attempts > 300:
                raise Rehearsal("resource not retrieved; retry retrieval later, never pay again")
            time.sleep(2)
    finally:
        process.kill()
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
    code, exported = command(bin_dir, "erebus-shielded-disclosure", {"method": "export", "evidence_file": seller["evidence_file"],
                             "issuer_key_file": str(seller_dir / "agreement.key"), "recipient_public_key": record["auditor_public_key"],
                             "grant_file": str(workdir / "auditor/deal.grant"), "expires_at": int(time.time()) + 3600}, seller_dir)
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
    while True:
        code, verified = command(bin_dir, auditor_cli, {"method": "verify_payment", "grant_file": str(workdir / "auditor/deal.grant"),
                                 "key_file": str(workdir / "auditor/auditor.key"), "expected_issuer": record["seller"],
                                 "deployment": deployment}, workdir / "auditor")
        if code != 2:
            break
        time.sleep(2)
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
