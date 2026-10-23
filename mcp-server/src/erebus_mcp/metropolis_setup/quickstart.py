"""One-command Monad testnet quickstart: a connected buyer, seller and auditor on one machine.

    erebus-init --metropolis quickstart up       --directory NEW [--version V | --pins DIR] [--allow-agent-funding]
    erebus-init --metropolis quickstart prefetch --directory DIR
    erebus-init --metropolis quickstart start  --directory DIR
    erebus-init --metropolis quickstart status --directory DIR [--offline]
    erebus-init --metropolis quickstart renew  --directory DIR
    erebus-init --metropolis quickstart down   --directory DIR
    erebus-init --metropolis quickstart add-buyer --directory DIR [--name bN] [--price P] [--fund-amount F]
                                                  [--buyer-key-file K] [--allow-agent-funding]

`add-buyer` adds another buyer of the same resource, at its own price, to the seller: a new buyer operator
under buyers/<name>/, a seller peer peers/<name>/ and outbox/<name>/, and agents/<name>/ files. The seller
still negotiates one deal at a time; its agent loops over negotiate_deal.

`up` runs the ordinary `init`, `connect`, `wallet-policy` and `auditor` steps for both roles, with the two
roles' shares wired to each other, so there is nothing to copy between machines. Gas keys are created by
the installed `erebus-settle` (or passed in as owner-only files); Python only handles paths. It writes MCP
configs for Claude Code, Codex and Cursor and ready-to-paste agent prompts, prints exactly what to fund,
and waits for the buyer's balance (MON gas, plus the USDC itself on the USDC pool). `start` (run by `up`
once funded) takes the auditor's history checkpoint first, then the fresh-wallet checkpoints, runs doctor,
and starts the seller's access service in the background. The auditor's checkpoint precedes every deal these
participants can make; a deal settled at or before it verifies as `settled_before_checkpoint`, never as paid
or unpaid.

Nothing here signs or sends a transaction. Agents may fund the buyer's shielded wallet only if the operator
passed `--allow-agent-funding`, which grants the policy-capped approve and deposit tools, plus wrap on the
WMON pool. The USDC pool has no wrap: the buyer must already hold the USDC it deposits.
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from types import SimpleNamespace

# Verified live on Monad testnet (chain 10143) on 2026-10-16, including a paired checkpoint: both report
# the same finalized block. (drpc's finalized tag ran about two blocks ahead, so paired checkpoint
# acquisition never agreed.) Operators can pass their own endpoints, or files holding keyed URLs.
DEFAULT_RPC_URL = "https://testnet-rpc.monad.xyz"
DEFAULT_PEER_RPC_URL = "https://rpc.ankr.com/monad_testnet"
FAUCET_URL = "https://faucet.monad.xyz"
# Monad testnet makes about 3.3 blocks per second and a history sync needs about 1.1 reads per block per
# provider; 20 per second catches up from a gap (about 14 blocks per second) and stays under the official
# endpoint's 50 per IP.
REQUESTS_PER_SECOND = 20
ACCESS_CONCURRENT_READS = 16
# Monad charges the full gas limit, so each send costs its per-method limit at the gas price. These mirror the
# native operator policy (sdk/shielded/src/gas_policy.rs): each sits above the measured estimate plus 5%
# (wrap 51k, approve 53k, deposit 1.89M, settle 2.73M on Monad testnet, 2026-10-22).
MON = 10**18
GAS_LIMITS = {"wrap": 65_000, "approve": 65_000, "deposit": 2_100_000, "settle": 3_010_000}
# Measured effective gas price on Monad testnet (2026-10-22); guidance only, the chain decides the price.
MEASURED_GAS_PRICE = 102 * 10**9
# The generated max_fee_per_gas (operator_init.GAS): a send needs its limit x max_fee free before it is signed.
MAX_FEE_PER_GAS = 300 * 10**9
# The buyer's own funding sends and its first settlement. USDC: the buyer already holds the asset, no wrap.
BUYER_SENDS = {"wmon": ("wrap", "approve", "deposit", "settle"), "usdc": ("approve", "deposit", "settle")}
SETTLE_HEADROOM = GAS_LIMITS["settle"] * MAX_FEE_PER_GAS
# Defaults in each pool's base units: WMON has 18 decimals, the deployed testnet USDC 6, so the WMON defaults
# (0.001 / 0.003 WMON) would be 1,000,000,000 / 3,000,000,000 USDC. USDC: 0.1 USDC price, 0.3 USDC note.
POOL_DEFAULTS = {"wmon": {"price": str(10**15), "fund_amount": str(3 * 10**15)},
                 "usdc": {"price": str(10**5), "fund_amount": str(3 * 10**5)}}
USDC = 10**6
LIFETIME = 30 * 24 * 3600
BINDING_LIFETIME = LIFETIME - 3600
ROLES = ("buyer", "seller")
STATE = "quickstart.json"


def mon(value: int) -> str:
    return f"{value / MON:.3f}"


def usdc(value: int) -> str:
    return f"{value // USDC}.{value % USDC:06d}"


def buyer_required_wei(pool: str, fund: int) -> int:
    """MON the buyer's gas address needs, as measured-price guidance: the funding sends and the first settlement
    at their per-method limits and the measured gas price, plus a settlement's limit x max_fee still free for the
    next deal. Wrapping (WMON only) also sends the note's value as MON."""
    sends = sum(GAS_LIMITS[kind] for kind in BUYER_SENDS[pool]) * MEASURED_GAS_PRICE
    return sends + SETTLE_HEADROOM + (fund if pool == "wmon" else 0)


def funding_why(pool: str, fund: int) -> str:
    sends = ", ".join(f"{kind} {GAS_LIMITS[kind]:,}" for kind in BUYER_SENDS[pool])
    return (f"measured-price guidance: gas limits {sends} at about {MEASURED_GAS_PRICE // 10**9} gwei "
            f"(Monad charges the full gas limit), plus {mon(SETTLE_HEADROOM)} MON (settle limit x max fee) free for "
            "the next settlement" + (f", and {mon(fund)} MON wrapped into the shielded note" if pool == "wmon" else ""))


def _endpoint(value: str | None, file: Path | None, default: str) -> str:
    if file is not None:
        return file.read_text().strip()
    return value or default


def _host(url: str) -> str:
    """The endpoint's host only: keyed URLs carry their key in the path or query."""
    return url.split("://", 1)[-1].split("/", 1)[0].split("?", 1)[0].split("@")[-1]


def default_bin() -> Path:
    """The installed native commands: beside this Python (a `uv tool` environment), else on PATH."""
    here = Path(sys.executable).parent
    if (here / "erebus-negotiate").is_file():
        return here
    found = shutil.which("erebus-negotiate")
    if found is None:
        raise ValueError("installed erebus-negotiate not found; pass --bin")
    return Path(found).parent


def installed_version() -> str:
    from importlib.metadata import version
    return version("erebus-mcp-server")


def _gas_key(ops, bins: Path, role: str, given: Path | None, keys: Path) -> tuple[str, Path]:
    if given is not None:
        path = given.resolve()
        reply = ops.command(bins, "erebus-settle", {"method": "address", "key_file": str(path)}, keys)
    else:
        path = keys / f"{role}-gas.key"
        reply = ops.command(bins, "erebus-settle", {"method": "generate_testnet_key", "key_file": str(path)}, keys)
    return reply["address"].lower(), path


def _write(path: Path, text: str) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write(text)


def _toml(value: str) -> str:
    return json.dumps(value)


def _agent_files(directory: Path, role: str, operation_hint: str, operator: Path | None = None) -> list[str]:
    """One MCP config per client plus a prompt; each agent sees only its own role's server."""
    settings = json.loads(((operator or directory / role) / "mcp.json").read_text())
    (name, entry), = settings["mcpServers"].items()
    out = directory / "agents" / role
    out.mkdir(mode=0o700, parents=True, exist_ok=True)
    _write(out / "claude-code.mcp.json", json.dumps(settings, indent=2) + "\n")
    _write(out / "cursor.mcp.json", json.dumps(settings, indent=2) + "\n")
    lines = [f"[mcp_servers.{name}]", f"command = {_toml(entry['command'])}", "args = []", "",
             f"[mcp_servers.{name}.env]"] + [f"{key} = {_toml(value)}" for key, value in entry["env"].items()]
    _write(out / "codex.config.toml", "\n".join(lines) + "\n")
    _write(out / "prompt.md", operation_hint)
    return [str(path.relative_to(directory)) for path in sorted(out.iterdir())]


BUYER_PROMPT = """You are the buyer agent for an Erebus private deal on Monad testnet. Use only the {server} MCP tools.

1. If the wallet has no spendable note, call fund_wallet(amount="{fund}"). If it returns ok=false with a
   pending status, call fund_wallet again with the operation_ref it returned until it reports complete.
   (Without operator-granted funding tools, ask the operator to fund the wallet instead.)
{asset_note}2. Call negotiate_deal() with no operation_ref. Keep the operation_ref it returns for every later call.
3. Call settle_deal(operation_ref). It checks the note and gas itself first: if it returns funding_required or a
   retryable error, nothing was sent; follow its next_action and call settle_deal again with the same
   operation_ref. Once it has submitted (submitted_this_call or a pending stage), never settle that deal again.
4. Call recover_deal(operation_ref) until payment_verified is true.
5. Call retrieve_service_access(operation_ref=...) until the resource is verified, and report its hash.

On any error, follow the error's next_action. Reuse the same operation_ref whenever
retry_same_operation_ref is true; never start a new payment because something is pending.
"""

SELLER_PROMPT = """You are the seller agent for an Erebus private deal on Monad testnet. Use only the {server} MCP tools.

You may sell to several buyers, one deal at a time. Repeat this loop:
1. Call negotiate_deal(wait_seconds=600) with no operation_ref, so you wait for the next buyer to connect.
2. Report the peer, deal_commitment and operation_ref it returns.
3. If it fails with PEER_UNAVAILABLE or NO_PEER_CONNECTED, call it again; otherwise follow its next_action.
Call list_peers to see which buyers are configured. You never pay; the seller's access service delivers each
resource once that buyer's payment is final.
"""

USDC_NOTE = """   Amounts are USDC base units (6 decimals): {fund} is {fund_usdc} USDC and the price is {price_usdc} USDC.
   The buyer must already hold that USDC; nothing wraps it. If fund_wallet returns FUNDS_REQUIRED, ask the
   operator to send the buyer address the shortfall it names, then call fund_wallet with the same operation_ref.
"""


def up(args, ops) -> dict:
    directory = args.directory.resolve()
    if directory.exists():
        raise ValueError("use a new quickstart directory")
    bins = (args.bin or default_bin()).resolve()
    directory_pins = None
    if args.pins is None:
        # Pin the release this command belongs to, so install -> quickstart up needs no pins step.
        directory_pins = args.directory.resolve() / "pins"
    pins = (args.pins or directory_pins).resolve()
    deployment = (args.deployment or pins / "monad-testnet-shielded-v2.json").resolve() if args.pins else None
    primary = _endpoint(args.rpc_url, args.rpc_url_file, DEFAULT_RPC_URL)
    peer = _endpoint(args.peer_rpc_url, args.peer_rpc_url_file, DEFAULT_PEER_RPC_URL)
    if primary == peer:
        raise ValueError("the primary and peer RPC endpoints must differ")
    defaults = POOL_DEFAULTS[args.pool]
    fund = ops.require_unsigned(args.fund_amount if args.fund_amount is not None else defaults["fund_amount"],
                                "--fund-amount")
    price = ops.require_unsigned(args.price if args.price is not None else defaults["price"], "--price")
    if int(fund) < int(price):
        raise ValueError("--fund-amount must cover at least one --price")
    directory.mkdir(mode=0o700)
    if directory_pins is not None:
        ops.pins(SimpleNamespace(version=args.version or installed_version(), directory=directory_pins,
                                 rehearsal_release=None, rehearsal_release_sha256=None, authorized_pins=None,
                                 prepublication_release=None, prepublication_release_sha256=None, authorization=None))
        deployment = (args.deployment or pins / "monad-testnet-shielded-v2.json").resolve()
    keys = directory / "keys"
    keys.mkdir(mode=0o700)
    signers = {role: _gas_key(ops, bins, role, getattr(args, f"{role}_key_file"), keys) for role in ROLES}
    ports = {"seller": args.port, "buyer": args.port + 1, "access": args.port + 2}
    for role in ("seller", "buyer"):
        ops.init(SimpleNamespace(
            role=role, pool=args.pool, directory=directory / role, bin=bins, pins=pins, deployment=deployment,
            endpoint=f"127.0.0.1:{ports[role]}", rpc_url=primary, peer_rpc_url=peer,
            signer_address=signers[role][0], transaction_key_file=signers[role][1],
            maximum_price=price if role == "buyer" else None, relay_url=None, relay_identity=None,
            relay_sponsor=None, relay_auth_key_file=None, relay_timeout_seconds=60,
            descriptor_lifetime_seconds=LIFETIME, binding_lifetime_seconds=BINDING_LIFETIME))
    payload = args.payload_file
    if payload is None:
        payload = directory / "sample-resource.txt"
        _write(payload, "Erebus quickstart sample resource\n" + os.urandom(16).hex() + "\n")
    base = dict(resource=args.resource, amount=price, minimum_price=None, unit="snapshot", quantity="1",
                delivery_seconds=21600)
    ops.connect(SimpleNamespace(directory=directory / "seller", peer_share=directory / "buyer" / "share",
                                payload_file=payload, port=ports["access"], service_url=None,
                                allow_loopback_access_http=False, **base))
    ops.connect(SimpleNamespace(directory=directory / "buyer", peer_share=directory / "seller" / "share",
                                payload_file=None, port=None,
                                service_url=f"http://127.0.0.1:{ports['access']}/v1/access",
                                allow_loopback_access_http=True, **base))
    allow = ["status"] + (["wrap", "approve", "deposit"] if args.pool == "wmon" else ["approve", "deposit"]) \
        if args.allow_agent_funding else ["status"]
    ops.wallet_policy(SimpleNamespace(directory=directory / "buyer", maximum_amount=fund, allow=allow,
                                      withdrawal_recipient=[], replacement_fees=None))
    # Quickstart's auditor is checkpointed, but not here: `start` takes that checkpoint first, while the cache is
    # still empty, so it precedes both participants' checkpoints and every deal they can make.
    ops.auditor(SimpleNamespace(pool=args.pool, directory=directory / "auditor", bin=bins, pins=pins,
                                deployment=deployment, rpc_url=primary, peer_rpc_url=peer, history="full"))
    _set_rates(ops, directory)
    asset_note = "" if args.pool == "wmon" else USDC_NOTE.format(
        fund=fund, fund_usdc=usdc(int(fund)), price_usdc=usdc(int(price)))
    files = []
    for role, prompt in (("buyer", BUYER_PROMPT), ("seller", SELLER_PROMPT)):
        files += _agent_files(directory, role, prompt.format(server=f"erebus-{role}", fund=fund, asset_note=asset_note))
    required = buyer_required_wei(args.pool, int(fund))
    state = {"version": 1, "pool": args.pool, "bin": str(bins), "ports": ports, "price": price, "fund_amount": fund,
             "pins": str(pins), "deployment": str(deployment), "buyers": {},
             "resource": args.resource, "agent_funding": bool(args.allow_agent_funding),
             "signers": {role: signers[role][0] for role in ROLES}, "buyer_required_wei": str(required),
             "rpc_hosts": [_host(primary), _host(peer)], "created": int(time.time()),
             "descriptor_expires_at": int(time.time()) - 60 + LIFETIME}
    if args.pool != "wmon":
        # The buyer deposits the asset it already holds; `up` and `status` wait for it as well as for gas.
        state.update(asset=_config(directory, "buyer")["asset"], buyer_required_asset=fund)
    ops.private_json(directory / STATE, state)
    funding = {
        "faucet": FAUCET_URL,
        "buyer": {"address": signers["buyer"][0], "send_at_least_mon": mon(required),
                  "why": funding_why(args.pool, int(fund))},
        "seller": {"address": signers["seller"][0], "send_at_least_mon": "0",
                   "why": "the seller sends nothing in this flow; fund it later only to withdraw earnings"},
    }
    if args.pool == "usdc":
        funding["buyer"].update(
            usdc_token=state["asset"], send_at_least_usdc_base_units=fund, send_at_least_usdc=usdc(int(fund)),
            also=f"send at least {fund} base units ({usdc(int(fund))} USDC) of testnet USDC, token {state['asset']}, "
                 "to the buyer address; nothing here mints or wraps it")
    result = {"status": "created", "directory": str(directory), "rpc_hosts": state["rpc_hosts"],
              "funding": funding, "agent_files": files, "agent_funding": state["agent_funding"]}
    if args.no_wait:
        return {**result, "next": "fund the buyer address, then run quickstart start"}
    waited = _wait_for_funding(ops, state, primary, args.funding_timeout)
    if not waited:
        return {**result, "next": "the buyer is not funded yet; fund it, then run quickstart start"}
    return {**result, **start(SimpleNamespace(directory=directory), ops)}


def _set_rates(ops, directory: Path) -> None:
    for relative in ("buyer/wallet.json", "seller/wallet.json", "buyer/payment.json"):
        path = directory / relative
        data = json.loads(path.read_text())
        data.update(rpc_requests_per_second=REQUESTS_PER_SECOND, peer_rpc_requests_per_second=REQUESTS_PER_SECOND)
        ops.replace_private_json(path, data)
    path = directory / "seller" / "access.json"
    data = json.loads(path.read_text())
    data["backend"]["requests_per_second"] = REQUESTS_PER_SECOND
    data["backend"]["max_concurrent_queries"] = ACCESS_CONCURRENT_READS
    ops.replace_private_json(path, data)


def _funded(ops, state: dict, url: str) -> bool:
    """The buyer holds enough MON for gas and, on a non-wrapped pool, enough of the asset to deposit."""
    buyer = state["signers"]["buyer"]
    if ops.rpc_balance(url, buyer) < int(state["buyer_required_wei"]):
        return False
    needed = int(state.get("buyer_required_asset", "0"))
    return needed == 0 or ops.rpc_token_balance(url, state["asset"], buyer) >= needed


def _signer_funded(ops, state: dict, url: str, signer: str, required_wei: str, required_asset: str) -> bool:
    """One buyer's gas address holds enough MON and, on a non-wrapped pool, enough of the asset."""
    if ops.rpc_balance(url, signer) < int(required_wei):
        return False
    needed = int(required_asset)
    return needed == 0 or ops.rpc_token_balance(url, state["asset"], signer) >= needed


def _wait_for_funding(ops, state: dict, url: str, timeout: int) -> bool:
    deadline = time.monotonic() + timeout
    while True:
        try:
            if _funded(ops, state, url):
                return True
        except (OSError, ValueError, KeyError):
            pass
        if time.monotonic() >= deadline:
            return False
        time.sleep(10)


def _config(directory: Path, role: str, name: str = "wallet.json") -> dict:
    return json.loads((directory / role / name).read_text())


def _auditor_cache(directory: Path) -> Path:
    return Path(_config(directory, "auditor", "disclosure.json")["cache_root"])


def _auditor_checkpoint(ops, bins: Path, directory: Path) -> int | str:
    """The auditor's checkpoint, taken before either participant's so that it precedes every deal."""
    cache = _auditor_cache(directory)
    if not (cache / "history-checkpoint.json").is_file() and any((cache / name).exists() for name in ("primary", "peer")):
        return "full_history"  # Verification already began from the deployment block; never narrowed.
    try:
        reply = ops.auditor_checkpoint(bins, directory / "auditor")
    except ValueError:
        raise ValueError("the auditor's checkpoint failed: both RPC providers must answer and agree on the same "
                         "finalized block; retry `quickstart start`, or run `up` again with another --peer-rpc-url") from None
    return "existing" if reply["checkpoint"] == "existing" else reply["checkpoint_block"]


def _auditor_status(directory: Path) -> dict:
    cache = _auditor_cache(directory)
    record = cache / "history-checkpoint.json"
    if record.is_file():
        block = json.loads(record.read_text())["checkpoint"]["block_number"]
        return {"history": "checkpoint", "checkpoint_block": block, "history_from_block": block + 1,
                "scope": "deals settled at or before checkpoint_block verify as settled_before_checkpoint"}
    started = any((cache / name).exists() for name in ("primary", "peer"))
    first = _config(directory, "auditor", "disclosure.json")["first_block"]
    return {"history": "full" if started else "not started", "checkpoint_block": None,
            "history_from_block": first if started else None}


def start(args, ops) -> dict:
    """Checkpoint the auditor, then the fresh participants, run doctor, and start the seller's access service."""
    directory = args.directory.resolve()
    state = json.loads((directory / STATE).read_text())
    bins = Path(state["bin"])
    artifacts = prefetch(SimpleNamespace(directory=directory), ops)["artifacts"]
    # The auditor first, so its checkpoint precedes every participant's and every deal they can make.
    checkpoints = {"auditor": _auditor_checkpoint(ops, bins, directory)}
    url = _config(directory, "buyer")["rpc_url"]
    for role, path in _participants(directory, state):
        wallet = json.loads((path / "wallet.json").read_text())
        record = Path(wallet["state_root"]) / "history-checkpoint.json"
        if record.is_file():
            checkpoints[role] = "existing"
            continue
        added = state.get("buyers", {}).get(role)
        if added is not None:
            # An added buyer is checkpointed once it is funded; until then `start` leaves it for later.
            try:
                funded = _signer_funded(ops, state, url, added["signer"], added["buyer_required_wei"],
                                        added.get("buyer_required_asset", "0"))
            except (OSError, ValueError, KeyError):
                funded = False
            if not funded:
                checkpoints[role] = "waiting for funding"
                continue
        try:
            reply = ops.command(bins, "erebus-wallet", {"method": "checkpoint", "confirm_fresh_wallet": True,
                                                        "config_file": str(path / "wallet.json")}, path)
        except ValueError:
            raise ValueError(f"the {role}'s fresh-wallet checkpoint failed: both RPC providers must answer and agree "
                             "on the same finalized block; retry `quickstart start`, or run `up` again with another "
                             "--peer-rpc-url") from None
        checkpoints[role] = reply["result"]["checkpoint_block"]
    # Generated seller configs keep the access service in its own state root; it starts from the same record.
    access = _config(directory, "seller", "access.json")
    target = Path(access["state_root"]) / "history-checkpoint.json"
    if not target.exists():
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(Path(_config(directory, "seller")["state_root"]) / "history-checkpoint.json", target)
        target.chmod(0o600)
    doctors = {role: ops.doctor(SimpleNamespace(directory=path, offline=False)).get("status")
               for role, path in _participants(directory, state)}
    service = _start_service(directory, bins, state)
    return {"status": "started", "artifacts": {role: f"{entry['cached']}/{entry['of']}" for role, entry in artifacts.items()},
            "checkpoints": checkpoints, "doctor": doctors, "access_service": service,
            "next": "add agents/buyer and agents/seller MCP configs to two agents and paste their prompt.md"}


CIRCUITS = ("deposit", "transfer", "withdraw")


def _participants(directory: Path, state: dict) -> list[tuple[str, Path]]:
    """Every operator directory: the first buyer, the seller, then each added buyer."""
    return [(role, directory / role) for role in ROLES] + [
        (name, directory / "buyers" / name) for name in sorted(state.get("buyers", {}))]


def _artifacts(directory: Path, role: str, path: Path | None = None) -> tuple[int, int]:
    wallet = json.loads(((path or directory / role) / "wallet.json").read_text())
    circuits = json.loads(Path(wallet["artifact_manifest_file"]).read_text())["circuits"]
    wanted = [circuit[field]["sha256"] for circuit in circuits for field in ("wasm", "r1cs", "zkey")]
    cache = Path(wallet["artifact_cache_root"])
    return sum((cache / f"{digest}.artifact").exists() for digest in wanted), len(wanted)


def prefetch(args, ops, roles=("buyer",)) -> dict:
    """Download and hash-check the proving artifacts now, one circuit at a time.

    Each circuit is installed by the native `erebus-artifacts`, which checks every file against the
    signed manifest and skips cached ones; after a timeout, run it again to continue.
    """
    directory = args.directory.resolve()
    state = json.loads((directory / STATE).read_text())
    bins = Path(state["bin"])
    report = {}
    for role in roles:
        wallet = _config(directory, role)
        done = []
        for circuit in CIRCUITS:
            request = {"manifest_file": wallet["artifact_manifest_file"],
                       "manifest_sha256": wallet["artifact_manifest_sha256"],
                       "manifest_authorization": wallet["artifact_manifest_authorization"], "circuit": circuit,
                       "cache_root": wallet["artifact_cache_root"], "allow_test_artifacts": wallet["allow_test_artifacts"],
                       "allow_loopback_http": wallet["allow_loopback_artifact_http"]}
            started = time.monotonic()
            try:
                result = subprocess.run([str(bins / "erebus-artifacts")], input=json.dumps(request), capture_output=True,
                                        text=True, timeout=900, cwd=directory,
                                        env={"PATH": f"{bins}{os.pathsep}/usr/bin:/bin", "HOME": str(directory)})
                reply = json.loads(result.stdout)
            except (subprocess.TimeoutExpired, ValueError):
                reply = {"status": "error"}
            if reply.get("status") != "ok":
                cached, total = _artifacts(directory, role)
                raise ValueError(f"{role} {circuit} artifacts did not install ({cached} of {total} files cached); "
                                 "check the network and run `quickstart prefetch` again to continue")
            entry = {"circuit": circuit, "downloaded_bytes": reply.get("downloaded_bytes", 0),
                     "cache_hits": reply.get("cache_hits", 0), "seconds": round(time.monotonic() - started, 1)}
            print(json.dumps({"event": "artifact_installed", "role": role, **entry}), file=sys.stderr, flush=True)
            done.append(entry)
        cached, total = _artifacts(directory, role)
        report[role] = {"circuits": done, "cached": cached, "of": total}
    return {"status": "prefetched", "artifacts": report}


def _pid(directory: Path) -> int | None:
    try:
        pid = int((directory / "run" / "access-service.pid").read_text())
    except (OSError, ValueError):
        return None
    try:
        os.kill(pid, 0)
    except OSError:
        return None
    return pid


def _healthy(port: int) -> bool:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=2) as response:
            return json.loads(response.read(4096)).get("status") == "ok"
    except (OSError, ValueError):
        return False


def _start_service(directory: Path, bins: Path, state: dict) -> str:
    if _pid(directory) is not None:
        return "running"
    run = directory / "run"
    run.mkdir(mode=0o700, exist_ok=True)
    log = os.open(run / "access-service.log", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    process = subprocess.Popen([str(bins / "erebus-access-service")], stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                               start_new_session=True,
                               env={"PATH": f"{bins}{os.pathsep}/usr/bin:/bin", "HOME": str(directory),
                                    "EREBUS_ACCESS_CONFIG": str(directory / "seller" / "access.json")})
    os.close(log)
    _write(run / "access-service.pid", str(process.pid))
    for _ in range(100):
        if _healthy(state["ports"]["access"]):
            return "started"
        if process.poll() is not None:
            return "exited; see run/access-service.log"
        time.sleep(0.2)
    return "not healthy yet; see run/access-service.log"


def _tip(path: Path) -> int | None:
    """The newest block a history cache holds: a v4 manifest's `tip`, or a legacy file's last block."""
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict):
        return None
    tip = data.get("tip")
    if isinstance(tip, dict) and isinstance(tip.get("number"), int):
        return tip["number"]
    blocks = data.get("blocks") or []
    return blocks[-1]["number"] if blocks and isinstance(blocks[-1], dict) else None


def _head(url: str) -> int | None:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_getBlockByNumber", "params": ["finalized", False]}).encode()
    try:
        with urllib.request.urlopen(urllib.request.Request(url, body, {"content-type": "application/json",
                                                                      "user-agent": "erebus-quickstart"}), timeout=20) as reply:
            return int(json.loads(reply.read(1 << 16))["result"]["number"], 16)
    except (OSError, ValueError, KeyError, TypeError):
        return None


def status(args, ops) -> dict:
    directory = args.directory.resolve()
    state = json.loads((directory / STATE).read_text())
    url = _config(directory, "buyer")["rpc_url"]
    head = None if args.offline else _head(url)
    now = int(time.time())
    roles = {}
    for role, path in _participants(directory, state):
        wallet = json.loads((path / "wallet.json").read_text())
        root = Path(wallet["state_root"])
        record = root / "history-checkpoint.json"
        checkpoint = json.loads(record.read_text())["checkpoint"]["block_number"] if record.is_file() else None
        tip = _tip(root / "wallet-index" / "primary.json") or checkpoint
        address = state["signers"][role] if role in ROLES else state["buyers"][role]["signer"]
        entry = {"address": address, "checkpoint_block": checkpoint, "history_through": tip,
                 "blocks_behind": None if head is None or tip is None else max(0, head - tip)}
        if not args.offline:
            try:
                entry["balance_mon"] = mon(ops.rpc_balance(url, address))
            except (OSError, ValueError, KeyError):
                entry["balance_mon"] = None
            if "asset" in state:
                try:
                    entry["balance_usdc_base_units"] = str(ops.rpc_token_balance(url, state["asset"], address))
                except (OSError, ValueError, KeyError):
                    entry["balance_usdc_base_units"] = None
        if role in state.get("buyers", {}):
            entry |= {"peer": role, "price": state["buyers"][role]["price"]}
        elif role == "buyer":
            entry |= {"peer": "default", "price": state["price"]}
        cached, total = _artifacts(directory, role, path)
        entry["artifacts_cached"] = f"{cached}/{total}"
        descriptor = next((path / "keys").glob("*.descriptor.json"))
        expires = json.loads(descriptor.read_text()).get("expires")
        entry["descriptor_expires_in_hours"] = None if expires is None else round((expires - now) / 3600, 1)
        roles[role] = entry
    service = "running" if _pid(directory) is not None and _healthy(state["ports"]["access"]) else "stopped"
    access_tip = _tip(Path(_config(directory, "seller", "access.json")["state_root"]) / "pool-history.json")
    expiring = [role for role, entry in roles.items()
                if entry["descriptor_expires_in_hours"] is not None and entry["descriptor_expires_in_hours"] < 24]
    buyer_ready = roles["buyer"]["checkpoint_block"] is not None
    if expiring:
        step = "descriptors expire within a day; run quickstart renew"
    elif not buyer_ready:
        step = f"fund the buyer with at least {mon(int(state['buyer_required_wei']))} MON" + (
            f" and {state['buyer_required_asset']} base units ({usdc(int(state['buyer_required_asset']))} USDC) of "
            "testnet USDC" if "buyer_required_asset" in state else "") + ", then run quickstart start"
    elif roles["buyer"]["artifacts_cached"].split("/")[0] != roles["buyer"]["artifacts_cached"].split("/")[1]:
        step = "proving artifacts are incomplete; run quickstart prefetch"
    elif service != "running":
        step = "run quickstart start to restart the seller's access service"
    else:
        step = "ready: run the buyer and seller agents with their agents/<role> files"
    return {"status": "ok", "roles": roles, "auditor": _auditor_status(directory), "access_service": service,
            "access_history_through": access_tip, "chain_finalized": head, "rpc_hosts": state["rpc_hosts"], "next": step}


def down(args, ops) -> dict:
    directory = args.directory.resolve()
    pid = _pid(directory)
    if pid is not None:
        os.killpg(os.getpgid(pid), signal.SIGTERM)
        for _ in range(50):
            if _pid(directory) is None:
                break
            time.sleep(0.2)
    (directory / "run" / "access-service.pid").unlink(missing_ok=True)
    return {"status": "stopped", "access_service_was_running": pid is not None,
            "kept": "keys, wallets, history and funds are untouched; quickstart start resumes"}


DELIVERY_SECONDS = 21600


def _buyers(directory: Path, state: dict) -> list[tuple[str, Path, Path, str]]:
    """(peer id, buyer directory, the seller share it connected against, its price) for every buyer."""
    seller = directory / "seller"
    entries = [("default", directory / "buyer", seller / "share", state["price"])]
    for name, added in sorted(state.get("buyers", {}).items()):
        entries.append((name, directory / "buyers" / name, seller / "outbox" / name, added["price"]))
    return entries


def _prepare_terms(ops, bins: Path, operator_dir: Path, role: str, peer_share: Path, offer: dict, amount: str,
                   output: Path) -> None:
    operator = json.loads((operator_dir / "operator.json").read_text())
    files = operator["operator"]
    fresh = output.with_name(output.name + ".renew")
    fresh.unlink(missing_ok=True)
    namespace = "eip155:10143"
    ops.command(bins, "erebus-negotiate", {
        "method": "prepare_terms", "suite_id": 2, "output": str(fresh), "role": role,
        "local_descriptor_file": files["descriptor_file"],
        "peer_descriptor_file": str(next(peer_share.glob("*.descriptor.json"))),
        "local_binding_file": str(operator_dir / "keys" / "identity.binding"),
        "peer_binding_file": str(peer_share / "identity.binding"), "settlement_contract": operator["settlement_contract"],
        "verifier_version": 2, "asset": f"{namespace}/erc20:{operator['asset']}", "amount": amount,
        "resource": offer["resource"], "unit": offer["unit"], "quantity": offer["quantity"],
        "fulfillment_method": offer["fulfillment_method"], "fulfillment_digest": offer["fulfillment_digest"],
        "delivery_deadline": offer["delivery_deadline"]}, operator_dir)
    os.replace(fresh, output)


def renew(args, ops) -> dict:
    """Re-sign every descriptor with its own keys, re-issue bindings, and give every buyer fresh terms.

    Each buyer's offer gets a new delivery_deadline, written identically into the seller's peer terms and the
    buyer's own terms; the access service is restarted once at the end.
    """
    directory = args.directory.resolve()
    state = json.loads((directory / STATE).read_text())
    bins = Path(state["bin"])
    running = _pid(directory) is not None
    if running:
        down(args, ops)
    participants = _participants(directory, state)
    expiry = None
    for _, path in participants:
        operator = json.loads((path / "operator.json").read_text())["operator"]
        reply = ops.command(bins, "erebus-negotiate", {
            "method": "renew_descriptor", "descriptor_file": operator["descriptor_file"],
            "discovery_key_file": operator["discovery_key_file"], "descriptor_lifetime_seconds": LIFETIME}, directory)
        expiry = reply["descriptor_expires_at"] if expiry is None else min(expiry, reply["descriptor_expires_at"])
        shutil.copyfile(operator["descriptor_file"], path / "share" / Path(operator["descriptor_file"]).name)
    for _, path in participants:
        operator = json.loads((path / "operator.json").read_text())
        files, binding = operator["operator"], path / "keys" / "identity.binding"
        fresh = binding.with_name("identity.binding.renew")
        fresh.unlink(missing_ok=True)
        ops.command(bins, "erebus-negotiate", {
            "method": "prepare_binding", "output": str(fresh), "descriptor_file": files["descriptor_file"],
            "discovery_key_file": files["discovery_key_file"], "agreement_key_file": files["agreement_key_file"],
            "settlement_contract": operator["settlement_contract"], "verifier_version": 2,
            "expires_at": expiry - 3600}, directory)
        os.replace(fresh, binding)
        shutil.copyfile(binding, path / "share" / "identity.binding")
        (path / "share" / "identity.binding").chmod(0o600)
    seller = directory / "seller"
    routed = json.loads((seller / "negotiate.json").read_text()).get("version") == 2
    deadlines = {}
    for identifier, buyer, share, price in _buyers(directory, state):
        if share != seller / "share":
            # The outbox carries the seller's renewed public files to this buyer.
            for source in sorted((seller / "share").glob("*.descriptor.json")) + [seller / "share" / "identity.binding"]:
                shutil.copyfile(source, share / source.name)
                (share / source.name).chmod(0o600)
        peer = seller / "peers" / identifier
        record = peer / "connect.json"
        source = json.loads(record.read_text()) if record.is_file() else {}
        offer = json.loads((share / "offer.json").read_text())
        offer["delivery_deadline"] = int(time.time()) + int(source.get("delivery_seconds", DELIVERY_SECONDS))
        ops.replace_private_json(share / "offer.json", offer)
        deadlines[identifier] = offer["delivery_deadline"]
        if routed:
            # The seller's copy of this buyer's renewed public files, and terms for the new deadline.
            descriptor = next(buyer.joinpath("share").glob("*.descriptor.json"))
            shutil.copyfile(descriptor, peer / "descriptor.json")
            shutil.copyfile(buyer / "share" / "identity.binding", peer / "identity.binding")
            for name in ("descriptor.json", "identity.binding"):
                (peer / name).chmod(0o600)
            _prepare_terms(ops, bins, seller, "seller", buyer / "share", offer, price, peer / "service.terms")
            settings = json.loads((peer / "peer.json").read_text())
            settings["expires_at"] = ops.descriptor_expiry(peer / "descriptor.json")
            ops.replace_private_json(peer / "peer.json", settings)
            if source:
                source.update(descriptor_sha256=ops.sha256(descriptor),
                              binding_sha256=ops.sha256(buyer / "share" / "identity.binding"))
                ops.replace_private_json(record, source)
        else:
            _prepare_terms(ops, bins, seller, "seller", buyer / "share", offer, price, seller / "keys" / "service.terms")
        _prepare_terms(ops, bins, buyer, "buyer", share, offer, price, buyer / "keys" / "service.terms")
    state["descriptor_expires_at"] = expiry
    ops.replace_private_json(directory / STATE, state)
    restarted = _start_service(directory, bins, state) if running else "not running"
    return {"status": "renewed", "descriptor_expires_at": expiry, "delivery_deadlines": deadlines,
            "access_service": restarted}


def add_buyer(args, ops) -> dict:
    """Add one buyer of the seller's resource at its own price, wired like `up` wires the first one."""
    directory = args.directory.resolve()
    state = json.loads((directory / STATE).read_text())
    buyers = state.setdefault("buyers", {})
    name = args.name or f"b{len(buyers) + 2}"
    if not ops.PEER_ID.fullmatch(name) or name in {"default", "buyer", "seller", "auditor"} or name in buyers:
        raise ValueError("--name must be a new buyer name of a-z, 0-9 and - (at most 32), not default/buyer/seller")
    target = directory / "buyers" / name
    if target.exists():
        raise ValueError(f"buyers/{name} already exists; choose another --name")
    bins = Path(state["bin"])
    defaults = POOL_DEFAULTS[state["pool"]]
    fund = ops.require_unsigned(args.fund_amount if args.fund_amount is not None else defaults["fund_amount"],
                                "--fund-amount")
    price = ops.require_unsigned(args.price if args.price is not None else defaults["price"], "--price")
    if int(fund) < int(price):
        raise ValueError("--fund-amount must cover at least one --price")
    pins = Path(state.get("pins") or directory / "pins")
    deployment = Path(state.get("deployment") or pins / "monad-testnet-shielded-v2.json")
    wallet = _config(directory, "seller")
    used = set(state["ports"].values()) | {entry["port"] for entry in buyers.values()}
    port = max(used) + 1
    keys = directory / "keys"
    signer, key_file = _gas_key(ops, bins, name, args.buyer_key_file, keys)
    (directory / "buyers").mkdir(mode=0o700, exist_ok=True)
    ops.init(SimpleNamespace(
        role="buyer", pool=state["pool"], directory=target, bin=bins, pins=pins, deployment=deployment,
        endpoint=f"127.0.0.1:{port}", rpc_url=wallet["rpc_url"], peer_rpc_url=wallet["peer_rpc_url"],
        signer_address=signer, transaction_key_file=key_file, maximum_price=price, relay_url=None, relay_identity=None,
        relay_sponsor=None, relay_auth_key_file=None, relay_timeout_seconds=60,
        descriptor_lifetime_seconds=LIFETIME, binding_lifetime_seconds=BINDING_LIFETIME))
    base = dict(resource=state["resource"], amount=price, minimum_price=None, unit="snapshot", quantity="1",
                delivery_seconds=DELIVERY_SECONDS)
    ops.connect(SimpleNamespace(directory=directory / "seller", peer_share=target / "share", payload_file=None,
                                port=None, service_url=None, allow_loopback_access_http=False, peer_id=name, **base))
    ops.connect(SimpleNamespace(directory=target, peer_share=directory / "seller" / "outbox" / name, payload_file=None,
                                port=None, service_url=f"http://127.0.0.1:{state['ports']['access']}/v1/access",
                                allow_loopback_access_http=True, peer_id=None, **base))
    allow = ["status"] + (["wrap", "approve", "deposit"] if state["pool"] == "wmon" else ["approve", "deposit"]) \
        if args.allow_agent_funding else ["status"]
    ops.wallet_policy(SimpleNamespace(directory=target, maximum_amount=fund, allow=allow, withdrawal_recipient=[],
                                      replacement_fees=None))
    for relative in ("wallet.json", "payment.json"):
        path = target / relative
        data = json.loads(path.read_text())
        data.update(rpc_requests_per_second=REQUESTS_PER_SECOND, peer_rpc_requests_per_second=REQUESTS_PER_SECOND)
        ops.replace_private_json(path, data)
    pool = state["pool"]
    asset_note = "" if pool == "wmon" else USDC_NOTE.format(
        fund=fund, fund_usdc=usdc(int(fund)), price_usdc=usdc(int(price)))
    files = _agent_files(directory, name, BUYER_PROMPT.format(server="erebus-buyer", fund=fund, asset_note=asset_note),
                         operator=target)
    required = buyer_required_wei(pool, int(fund))
    buyers[name] = {"port": port, "price": price, "fund_amount": fund, "signer": signer,
                    "buyer_required_wei": str(required), "agent_funding": bool(args.allow_agent_funding),
                    "created": int(time.time())}
    if pool != "wmon":
        # This buyer deposits the asset it already holds; `start` waits for it as well as for gas.
        buyers[name]["buyer_required_asset"] = fund
    ops.replace_private_json(directory / STATE, state)
    funding = {"address": signer, "send_at_least_mon": mon(required), "faucet": FAUCET_URL,
               "why": funding_why(pool, int(fund))}
    if pool != "wmon":
        funding.update(usdc_token=state["asset"], send_at_least_usdc_base_units=fund, send_at_least_usdc=usdc(int(fund)),
                       also=f"send at least {fund} base units ({usdc(int(fund))} USDC) of testnet USDC, token "
                            f"{state['asset']}, to the buyer address; nothing here mints or wraps it")
    return {"status": "added", "buyer": name, "peer": name, "price": price, "directory": str(target),
            "funding": funding, "agent_files": files,
            "next": f"fund {signer}, run quickstart start, then give agents/{name} to the new buyer agent; the "
                    "seller agent's negotiate_deal loop serves it in turn"}


def run(args, ops) -> dict:
    return {"up": up, "start": start, "status": status, "down": down, "renew": renew,
            "prefetch": prefetch, "add-buyer": add_buyer}[args.action](args, ops)


def add_parser(commands) -> None:
    quick = commands.add_parser("quickstart", help="one-command Monad testnet buyer, seller and auditor on this machine")
    quick.add_argument("action", choices=("up", "start", "status", "prefetch", "renew", "down", "add-buyer"))
    quick.add_argument("--name", help="add-buyer: the new buyer's name and seller peer id (default bN)")
    quick.add_argument("--directory", type=Path, required=True)
    quick.add_argument("--pins", type=Path, help="up: an existing pins directory (default: pin --version)")
    quick.add_argument("--version", help="up: release to pin (default: the installed erebus-mcp-server version)")
    quick.add_argument("--bin", type=Path, help="up: installed release bin directory (default: where erebus-negotiate is)")
    quick.add_argument("--deployment", type=Path, help="up: deployment record (default: the one in --pins)")
    quick.add_argument("--pool", choices=("wmon", "usdc"), default="wmon")
    quick.add_argument("--rpc-url")
    quick.add_argument("--peer-rpc-url")
    quick.add_argument("--rpc-url-file", type=Path, help="owner-only file holding a keyed primary RPC URL")
    quick.add_argument("--peer-rpc-url-file", type=Path, help="owner-only file holding a keyed peer RPC URL")
    quick.add_argument("--buyer-key-file", type=Path, help="existing owner-only buyer gas key (default: create one)")
    quick.add_argument("--seller-key-file", type=Path, help="existing owner-only seller gas key (default: create one)")
    quick.add_argument("--price", help="deal price in the pool asset's base units (default: 10^15 = 0.001 WMON; "
                                       "100000 = 0.1 USDC)")
    quick.add_argument("--fund-amount", help="shielded note the buyer funds, base units (default: 3x10^15 = 0.003 WMON; "
                                             "300000 = 0.3 USDC)")
    quick.add_argument("--resource", default="quickstart-sample")
    quick.add_argument("--payload-file", type=Path, help="the resource the seller delivers (default: a sample file)")
    quick.add_argument("--port", type=int, default=9311, help="seller negotiation port; buyer +1, access service +2")
    quick.add_argument("--allow-agent-funding", action="store_true",
                       help="grant the buyer agent policy-capped approve/deposit, plus wrap on WMON (fund_wallet)")
    quick.add_argument("--no-wait", action="store_true", help="up: do not wait for the buyer's balance")
    quick.add_argument("--funding-timeout", type=int, default=1800, help="up: seconds to wait for funding")
    quick.add_argument("--offline", action="store_true", help="status: skip RPC reads")


if __name__ == "__main__":
    sys.exit("run through erebus-init --metropolis quickstart")
