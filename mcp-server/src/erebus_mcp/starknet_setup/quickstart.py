"""Starknet mainnet quickstart: a buyer and a seller identity, set up together on one machine.

    erebus-init --starknet quickstart up     --directory NEW --rpc-url URL|--rpc-url-file F --max-strk B
                                             [--price-cap 0.2] [--extra-rounds K] [--buyer-deposit 0.5]
                                             [--buyer-account ID] [--seller-account ID] [--yes] [--mock]
    erebus-init --starknet quickstart start  --directory DIR
    erebus-init --starknet quickstart status --directory DIR [--offline]
    erebus-init --starknet quickstart renew  --directory DIR --deals N [--max-strk B] [--yes]
    erebus-init --starknet quickstart down   --directory DIR [--offline]

`up` runs the ordinary `erebus-init` setup (``erebus_mcp.setup.run``) once per role, buyer (payer)
first, then seller (payee), each with its own protected config under DIR/<role>/mcp.env and its
identity directory beside it. Rust creates or imports each OpenZeppelin account, generates its
pool key, reads the live pool fee and gas reserve, deploys, approves the exact allowance, waits for
proving depth, and shields once (which registers the pool identity). Python handles paths, plans
and operation IDs only, as the setup flow already does; it never reads key material.

There is one confirmation for both roles: a terminal user types ``mainnet <total STRK>``; anything
non-interactive gets ``authorization_required`` unless it passed ``--yes``. ``--max-strk`` is
required on mainnet and a plan above it is refused. Setup's resume rules apply unchanged: an
interrupted write is finished under its original operation ID and never sent again, so rerunning
the same `up` continues where it stopped.

This quickstart never bridges, swaps or touches another chain. It shares nothing with the Monad
quickstart (``erebus-init --metropolis quickstart``): a different directory marker, different MCP
server names (``erebus-starknet-*`` against ``erebus-monad-*``) and separate env files, so both can
run side by side from one install.

`--mock` writes the same layout and prompts against the mock backend, with its shared store inside
DIR: no network, no keys, no funds.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import shlex
import shutil
import stat
import sys
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path
from typing import Any

from erebus import PROTOCOL, ErebusError, SeamConfig, network_preset

from erebus_mcp import setup
from erebus_mcp.intent import IntentStore, new_operation_id
from erebus_mcp.onboarding import OnboardingError, load_config_file, write_config_file
from erebus_mcp.write_budget import WriteBudget, budget_path

STATE = "starknet-quickstart.json"
MONAD_STATE = "quickstart.json"  # erebus_mcp.metropolis_setup.quickstart.STATE
NETWORK = "mainnet"
CHAIN_ID_NAME = "SN_MAIN"
STRK = setup.STRK
WEI = 10**18
DEFAULT_PROVER_URL = "https://api.starkscan.co/v1/SN_MAIN/prove"
# The prover receives the plaintext pool key with every proof. Only the canonical mainnet prover is
# accepted; anything else is an unknown host and refused.
KNOWN_PROVER_HOSTS = frozenset({"api.starkscan.co"})
ROLES = ("buyer", "seller")
SETTLEMENT = {"buyer": "payer", "seller": "payee"}
SERVER_NAMES = {"buyer": "erebus-starknet-buyer", "seller": "erebus-starknet-seller"}
# Buyer: open, propose, settle. Seller: open, counter. Each --extra-rounds adds one counter per side.
BASE_WRITES = {"buyer": 3, "seller": 2}
# A later deal on the open channels: the buyer proposes and settles, the seller counters.
WRITES_PER_RENEWED_DEAL = {"buyer": 2, "seller": 1}
DEFAULT_DEPOSITS = {"buyer": "0.5", "seller": "0.01"}
DEFAULT_PRICE_CAP = "0.2"
MOCK_ADDRESSES = {"buyer": "0x0b0b0b0b", "seller": "0x05e5e5e5"}
CLEAN_FINDINGS = {"none", "safe_to_retry"}
WAITING = {"funding_required"}
OK_STATUSES = {"ready", "ok", "renewed", "down"}


@dataclass
class Deps:
    """Everything `run` reaches outside this process. Tests replace the seam and the clock."""

    seam_factory: Callable[[SeamConfig | None], Any] | None = None
    interactive: bool = False
    input: Callable[[str], str] = input
    getpass: Callable[[str], str] = getpass.getpass
    sleep: Callable[[float], None] = time.sleep
    monotonic: Callable[[], float] = time.monotonic
    environ: Mapping[str, str] = field(default_factory=lambda: os.environ)
    log: Callable[[str], None] = field(default_factory=lambda: (lambda message: print(message, file=sys.stderr)))


def strk(value: int | str) -> str:
    text = format(Decimal(int(value)) / Decimal(WEI), "f")
    return text.rstrip("0").rstrip(".") if "." in text else text


def _host(url: str) -> str:
    """The endpoint's host only: keyed URLs carry their key in the path or query."""
    return url.split("://", 1)[-1].split("/", 1)[0].split("?", 1)[0].split("@")[-1].lower()


def _owner_only_text(path: Path, label: str) -> str:
    details = path.stat()
    if not stat.S_ISREG(details.st_mode) or (os.name != "nt" and details.st_mode & 0o077):
        raise ValueError(f"{label} must be an owner-only regular file (chmod 600)")
    return path.read_text().strip()


def _write(path: Path, text: str) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write(text)


def _config_path(directory: Path, role: str) -> Path:
    return directory / role / "mcp.env"


def _manifest_path(directory: Path, role: str) -> Path:
    config = _config_path(directory, role)
    return config.with_suffix(config.suffix + ".setup.json")


def _identity_dir(directory: Path, role: str) -> Path:
    """Where `erebus_mcp.setup` keeps this identity's account and pool key files."""
    config = _config_path(directory, role)
    return config.parent / (config.name + ".identity")


def _state_dir(directory: Path, role: str, mock: bool) -> Path:
    return directory / role / "state" if mock else _identity_dir(directory, role) / "state"


def _load(directory: Path) -> dict:
    if (directory / MONAD_STATE).exists():
        raise ValueError(
            f"{directory} holds a Monad quickstart (quickstart.json); use erebus-init --metropolis quickstart there"
        )
    path = directory / STATE
    if not path.is_file():
        raise ValueError(f"{directory} holds no Starknet quickstart; run quickstart up first")
    return json.loads(path.read_text())


def _save(directory: Path, state: dict) -> None:
    setup.save(directory / STATE, state)


def _values(directory: Path, role: str) -> dict[str, str]:
    """The role's protected config, or its setup manifest's values before the config exists."""
    config = _config_path(directory, role)
    values: dict[str, str] = {}
    if config.exists():
        load_config_file(config, values)
        return values
    manifest = _manifest_path(directory, role)
    if manifest.exists():
        return dict(json.loads(manifest.read_text())["values"])
    return values


def resolve_cli(explicit: Path | None) -> str | None:
    """The `erebus-cli` of this same install: the packaged binary beside this Python."""
    if explicit is not None:
        path = explicit.expanduser().absolute()
        if not path.is_file():
            raise ValueError(f"--cli {path} does not exist")
        return str(path)
    try:
        from erebus_cli import binary_path

        found = binary_path()
    except ImportError:
        found = None
    if found is None:
        located = shutil.which("erebus-cli")
        found = Path(located) if located else None
    return str(Path(found).absolute()) if found else None


def resolve_server(explicit: Path | None) -> str:
    if explicit is not None:
        path = explicit.expanduser().absolute()
        if not path.is_file():
            raise ValueError(f"--server-command {path} does not exist")
        return str(path)
    beside = Path(sys.executable).parent / "erebus-mcp-server"
    if beside.is_file():
        return str(beside.absolute())
    located = shutil.which("erebus-mcp-server")
    if located is None:
        raise ValueError("installed erebus-mcp-server not found; pass --server-command")
    return str(Path(located).absolute())


def _guard_env(directory: Path, role: str, state: dict) -> dict[str, str]:
    """The server-side guardrails and state paths every generated config carries explicitly, so an
    MCP client's inherited environment can supply none of them."""
    mock = state["backend"] == "mock"
    cap = int(state["price_cap"])
    root = _state_dir(directory, role, mock)
    env = {
        "EREBUS_SPENDING_LIMITS": json.dumps(
            {STRK: {"per_deal": str(cap), "daily": str(2 * cap)}}, separators=(",", ":")
        ),
        "EREBUS_MAX_WRITES": str(state["writes"][role]),
        "EREBUS_SPENDING_STATE_PATH": str(root / "spending.json"),
        "EREBUS_INTENT_STATE_DIR": str(root),
        "EREBUS_SKIP_STARTUP_DOCTOR": "0",
        "EREBUS_REQUIRE_CANONICAL_NETWORK": "0" if mock else "1",
    }
    if state.get("cli"):
        env["EREBUS_CLI"] = state["cli"]
    return env


def _rewrite_config(path: Path, updates: Mapping[str, str]) -> None:
    values: dict[str, str] = {}
    load_config_file(path, values)
    values.update(updates)
    fresh = path.with_name(f".{path.name}.rewrite")
    fresh.unlink(missing_ok=True)
    write_config_file(fresh, values)
    os.replace(fresh, path)


# --- Prompts --------------------------------------------------------------------------------

BUYER_PROMPT = """You are the buyer agent in an Erebus private negotiation on {label}. Use only the {server} MCP tools.

Seller (counterparty) address: {seller}
Token: STRK {token}. Every amount is a decimal string in base units (10^18 = 1 STRK).
Price cap: {cap} base units ({cap_strk} STRK). Never offer, counter or settle above it; the server
refuses a settlement above it in any case.
Write budget: at most {writes} charged writes (open, propose, {buyer_counters_text}settle). {cost}
The server refuses the write after the budget.

Give every write a new operation_id: `op_` followed by 64 random lowercase hex digits. If a write
fails or times out, call reconcile and then resume_operation with that same operation_id; never
retry it under a new one.

Follow this order exactly:
1. Call doctor and get_note_balance. If doctor is not ready, stop and report its repairs.
2. Call open_channel(operation_id, counterparty="{seller}"). Keep the channel_handle.
3. Wait for the counterparty's channel: do not propose until the operator confirms that the seller
   agent's open_channel has returned its channel_handle.
4. Call propose_offer(operation_id, channel_handle, amount="{opening}", token="{token}",
   deadline=<unix time now + 3600>, memo_hash="0x1").
5. Call wait_for_offers(channel_handle, expected_count=2, timeout_seconds=600), again while it times
   out. The seller answers with a counter-offer.
{buyer_counter_step}
6. If the seller's latest offer is at or below {cap} base units and its deadline has not passed,
   call accept_and_settle(operation_id, channel_handle, offer_id) once, with that offer's offer_id.
   Otherwise stop and report; do not settle.
7. Report the tx_hash and the settled amount.
"""

SELLER_PROMPT = """You are the seller agent in an Erebus private negotiation on {label}. Use only the {server} MCP tools.

Buyer (counterparty) address: {buyer}
Token: STRK {token}. Every amount is a decimal string in base units (10^18 = 1 STRK).
Your ask: {ask} base units ({ask_strk} STRK).
You are the payee. Never call accept_and_settle: the buyer settles, and this server refuses it.
Write budget: at most {writes} charged writes (open, then {seller_counters} counter-offer(s)). {cost}
The server refuses the write after the budget.

Give every write a new operation_id: `op_` followed by 64 random lowercase hex digits. If a write
fails or times out, call reconcile and then resume_operation with that same operation_id; never
retry it under a new one.

Follow this order exactly:
1. Call doctor. If it is not ready, stop and report its repairs.
2. Call open_channel(operation_id, counterparty="{buyer}"). Keep the channel_handle, and tell the
   operator it returned so the buyer can propose.
3. Call wait_for_offers(channel_handle, expected_count=1, timeout_seconds=600), again while it times
   out, until the buyer's offer arrives.
4. Counter at most once per buyer offer, {seller_counters} in total: counter_offer(operation_id,
   channel_handle, reply_to=<the buyer's latest offer_id>, amount="{ask}" (or the buyer's amount if
   that is higher), token="{token}", deadline=<unix time now + 3600>, memo_hash="0x1").
{seller_round_step}
5. Call read_channel_state(channel_handle) at most once a minute, for up to 20 minutes, until it
   shows a settlement, then report it. Reads are free; do not write while you wait.
"""


def _prompt_values(state: dict) -> dict[str, str]:
    cap = int(state["price_cap"])
    extra = int(state["extra_rounds"])
    mock = state["backend"] == "mock"
    fee = state.get("fee_per_write")
    if mock:
        cost = "On the mock backend writes are free, but follow the same rules."
    else:
        cost = (
            "Each is a real Starknet mainnet apply_actions transaction that pays the live pool fee"
            + (f" (about {strk(fee)} STRK when this was written)" if fee else "")
            + " from this identity's public STRK."
        )
    return {
        "label": "the Erebus mock backend (no chain, no funds)" if mock else "Starknet mainnet",
        "token": STRK,
        "cap": str(cap),
        "cap_strk": strk(cap),
        "opening": str(cap // 2),
        "ask": str(cap * 3 // 4),
        "ask_strk": strk(cap * 3 // 4),
        "cost": cost,
        "buyer": state["roles"]["buyer"]["address"],
        "seller": state["roles"]["seller"]["address"],
        "buyer_counters_text": f"up to {extra} counter-offer(s), " if extra else "",
        "seller_counters": str(1 + extra),
        "buyer_counter_step": (
            f"   You may answer with at most {extra} counter_offer(s) of your own (reply_to the seller's\n"
            "   latest offer_id, amount at or below the cap), each followed by wait_for_offers for the reply."
            if extra
            else "   Do not counter: settle the seller's counter-offer or stop."
        ),
        "seller_round_step": (
            "   After each counter, wait_for_offers for the buyer's reply before deciding the next one."
            if extra
            else "   That one counter is your final offer."
        ),
    }


def _agent_files(directory: Path, state: dict) -> list[str]:
    """One MCP config per client plus a prompt; each agent sees only its own role's server."""
    values = _prompt_values(state)
    written = []
    for role in ROLES:
        out = directory / "agents" / role
        out.mkdir(mode=0o700, parents=True, exist_ok=True)
        name = SERVER_NAMES[role]
        args = ["--config", str(_config_path(directory, role))]
        settings = {"mcpServers": {name: {"command": state["server_command"], "args": args}}}
        _write(out / "claude-code.mcp.json", json.dumps(settings, indent=2) + "\n")
        _write(out / "cursor.mcp.json", json.dumps(settings, indent=2) + "\n")
        toml = [
            f"[mcp_servers.{name}]",
            f"command = {json.dumps(state['server_command'])}",
            f"args = [{', '.join(json.dumps(arg) for arg in args)}]",
        ]
        _write(out / "codex.config.toml", "\n".join(toml) + "\n")
        prompt = BUYER_PROMPT if role == "buyer" else SELLER_PROMPT
        _write(out / "prompt.md", prompt.format(server=name, writes=state["writes"][role], **values))
        written += [str(path.relative_to(directory)) for path in sorted(out.iterdir())]
    return written


def _registration(directory: Path, state: dict) -> dict:
    commands = {}
    for role in ROLES:
        name, files = SERVER_NAMES[role], directory / "agents" / role
        config = _config_path(directory, role)
        commands[role] = {
            "server_name": name,
            "claude_code": f"claude mcp add {name} -- {shlex.quote(state['server_command'])} "
            f"--config {shlex.quote(str(config))}",
            "claude_code_session": f"claude --mcp-config {shlex.quote(str(files / 'claude-code.mcp.json'))}",
            "codex": f"merge {files / 'codex.config.toml'} into ~/.codex/config.toml (one role per Codex profile)",
            "cursor": f"copy {files / 'cursor.mcp.json'} to the project's .cursor/mcp.json",
            "prompt": str(files / "prompt.md"),
        }
    return commands


# --- Live helpers ---------------------------------------------------------------------------


def _factory(state: dict, deps: Deps) -> Callable[[SeamConfig | None], Any]:
    if deps.seam_factory is not None:
        return deps.seam_factory
    from erebus import Seam

    return lambda config=None: Seam(config, binary=state["cli"])


def _configured(factory, values: Mapping[str, str]):
    if values.get("STARKSCAN_API_KEY"):
        os.environ["STARKSCAN_API_KEY"] = values["STARKSCAN_API_KEY"]
    return factory(
        SeamConfig.for_network(
            values["EREBUS_NETWORK"],
            rpc_url=values["STARKNET_RPC_URL"],
            prover_url=values["PROVING_SERVICE_URL"],
            account_address=values["AGENT_ADDRESS"],
            pool_key_file=values["POOL_KEY_FILE"],
            account_key_file=values["ACCOUNT_KEY_FILE"],
            state_dir=values["EREBUS_STATE_DIR"],
            token=values["TOKEN_ADDRESS"],
        )
    )


def _account(factory, directory: Path, role: str, values: Mapping[str, str], action: str, **args):
    preset = network_preset(NETWORK)
    return factory(None).call(
        "onboarding",
        {
            "action": action,
            "args": {
                "directory": str(_identity_dir(directory, role)),
                "chain_id": preset.chain_id,
                "rpc_url": values["STARKNET_RPC_URL"],
                **args,
            },
        },
    )


def _inspect(factory, directory: Path, role: str, values: Mapping[str, str]) -> dict:
    return _account(
        factory, directory, role, values, "inspect",
        address=values["AGENT_ADDRESS"], pool_address=network_preset(NETWORK).pool_address, token=STRK,
    )


def _deployed(factory, directory: Path, role: str, values: Mapping[str, str]) -> bool:
    if not (_identity_dir(directory, role) / "account.json").exists():
        return True
    return bool(_account(factory, directory, role, values, "status")["deployed"])


def _check_endpoints(rpc_url: str, prover_url: str) -> None:
    for label, url in (("--rpc-url", rpc_url), ("--prover-url", prover_url)):
        if not url.startswith("https://"):
            raise ValueError(f"{label} must be an https URL")
    if _host(prover_url) not in KNOWN_PROVER_HOSTS:
        raise ValueError(
            f"unknown prover host {_host(prover_url)}: the prover receives each identity's pool key, and "
            f"this quickstart accepts only {', '.join(sorted(KNOWN_PROVER_HOSTS))}"
        )


def _check_chain(rpc_url: str) -> None:
    served = setup.rpc(rpc_url, "starknet_chainId", [])
    try:
        same = int(str(served), 16) == int(network_preset(NETWORK).chain_id, 16)
    except ValueError:
        same = False
    if not same:
        raise ValueError(
            f"the RPC at {_host(rpc_url)} serves chain {served}, not Starknet mainnet ({CHAIN_ID_NAME}); refusing"
        )


def _approvals(directory: Path, role: str, state: dict) -> list[str]:
    """Every approval this quickstart made for the role, oldest first."""
    hashes = []
    manifest = _manifest_path(directory, role)
    if manifest.exists():
        operations = json.loads(manifest.read_text())["operations"]
        for step, record in operations.items():
            if step.split(":")[0] == "approve" and (record.get("result") or {}).get("tx_hash"):
                hashes.append(record["result"]["tx_hash"])
    for renewal in state.get("renewals", []) + ([state["renewal"]] if state.get("renewal") else []):
        record = renewal["roles"].get(role) or {}
        if (record.get("result") or {}).get("tx_hash"):
            hashes.append(record["result"]["tx_hash"])
    return hashes


def _budget(values: Mapping[str, str]) -> WriteBudget:
    cap = values.get("EREBUS_MAX_WRITES")
    return WriteBudget(budget_path(Path(values["EREBUS_INTENT_STATE_DIR"])), int(cap) if cap else None)


# --- up -------------------------------------------------------------------------------------


def _setup_options(directory: Path, role: str, state: dict, *, yes: bool, wait: int,
                   rpc_url: str | None = None, prover_url: str | None = None,
                   starkscan_key: str | None = None, max_allowance: int | None = None) -> argparse.Namespace:
    config = _config_path(directory, role)
    argv = ["--config", str(config), "--network", NETWORK, "--json", "--non-interactive", "--wait", str(wait)]
    new = not _manifest_path(directory, role).exists()
    if new:
        if rpc_url is None or prover_url is None:
            raise ValueError("pass --rpc-url or --rpc-url-file to continue setting up the " + role)
        selected = state["roles"][role].get("import")
        argv += ["--account", selected] if selected else ["--new"]
        argv += ["--role", SETTLEMENT[role], "--rpc-url", rpc_url, "--prover-url", prover_url,
                 "--deposit", state["deposits_strk"][role], "--writes", str(state["writes"][role])]
        if state.get("accounts_file"):
            argv += ["--accounts-file", state["accounts_file"]]
    else:
        argv.append("--resume")
    if yes:
        argv.append("--yes")
    options = setup.parser().parse_args(argv)
    if new:
        extra = _guard_env(directory, role, state)
        if starkscan_key:
            extra["STARKSCAN_API_KEY"] = starkscan_key
        options.extra_values = extra
    options.max_allowance = max_allowance
    return options


def _starkscan_key(args, deps: Deps) -> str | None:
    if args.starkscan_key_file is not None:
        return _owner_only_text(args.starkscan_key_file.expanduser(), "--starkscan-key-file")
    if deps.environ.get("STARKSCAN_API_KEY"):
        return deps.environ["STARKSCAN_API_KEY"]
    if deps.interactive:
        return deps.getpass("Starkscan API key: ").strip() or None
    return None


def _validate_import(value: str | None, flag: str) -> str | None:
    if value is None:
        return None
    parts = value.split(":", 2)
    if len(parts) != 3 or parts[0] != "sncast" or parts[1] not in {"alpha-mainnet", "SN_MAIN", "mainnet"}:
        raise ValueError(
            f"{flag} takes a mainnet sncast account id from `erebus-init --list-accounts` "
            "(sncast:alpha-mainnet:<name>); an existing Erebus config is used directly, not imported, "
            "so its keys and state stay in one place"
        )
    return value


def up(args, deps: Deps) -> dict:
    directory = args.directory.expanduser().absolute()
    if (directory / MONAD_STATE).exists():
        raise ValueError(f"{directory} holds a Monad quickstart (quickstart.json); use a new directory")
    if directory.exists() and not (directory / STATE).exists():
        raise ValueError("use a new quickstart directory")
    return _up_mock(args, deps, directory) if args.mock else _up_live(args, deps, directory)


def _new_state(args, backend: str) -> dict:
    cap = setup.amount(args.price_cap)
    deposits = {"buyer": args.buyer_deposit, "seller": args.seller_deposit}
    for role, value in deposits.items():
        if setup.amount(value) <= 0:
            raise ValueError(f"the {role} deposit must be positive: a new address registers by shielding")
    if cap <= 0 or cap > setup.amount(deposits["buyer"]):
        raise ValueError("--price-cap must be positive and no more than --buyer-deposit, which pays it")
    if not 0 <= args.extra_rounds <= 10:
        raise ValueError("--extra-rounds must be between 0 and 10")
    return {
        "version": 1,
        "backend": backend,
        "network": NETWORK if backend == "starknet" else "mock",
        "chain_id": CHAIN_ID_NAME if backend == "starknet" else None,
        "pool": network_preset(NETWORK).pool_address if backend == "starknet" else None,
        "token": STRK,
        "price_cap": str(cap),
        "extra_rounds": args.extra_rounds,
        "deposits_strk": deposits,
        "writes": {role: BASE_WRITES[role] + args.extra_rounds for role in ROLES},
        "roles": {role: {"settlement_role": SETTLEMENT[role], "server": SERVER_NAMES[role]} for role in ROLES},
        "created": int(time.time()),
    }


def _up_mock(args, deps: Deps, directory: Path) -> dict:
    if directory.exists():
        state = _load(directory)
        if state["backend"] != "mock":
            raise ValueError("this directory holds a live Starknet quickstart; --mock needs a new directory")
        return {**start(args, deps), "status": "ready"}
    state = _new_state(args, "mock")
    state.update(server_command=resolve_server(args.server_command), cli=resolve_cli(args.cli),
                 rpc_host=None, prover_host=None)
    for role in ROLES:
        state["roles"][role]["address"] = MOCK_ADDRESSES[role]
    directory.mkdir(mode=0o700, parents=True)
    store = directory / "mock-store.json"
    for role in ROLES:
        values = {
            "EREBUS_BACKEND": "mock",
            "AGENT_ADDRESS": MOCK_ADDRESSES[role],
            "PROVING_SERVICE_URL": "http://unused.invalid",
            "EREBUS_SETTLEMENT_ROLE": SETTLEMENT[role],
            "EREBUS_MOCK_STORE_PATH": str(store),
            "EREBUS_MOCK_LATENCY_SECONDS": "0",
            "EREBUS_MOCK_SPENDABLE_NOTES": str(setup.amount(state["deposits_strk"][role])),
            "EREBUS_MOCK_PENDING_NOTES": "",
            **_guard_env(directory, role, state),
        }
        write_config_file(_config_path(directory, role), values)
        _state_dir(directory, role, True).mkdir(mode=0o700, parents=True, exist_ok=True)
    _save(directory, state)
    files = _agent_files(directory, state)
    return {"status": "ready", "backend": "mock", "directory": str(directory), "agent_files": files,
            **{key: value for key, value in start(args, deps).items() if key != "status"}}


def _plan_row(directory: Path, role: str, details: dict, inspection: dict | None) -> dict:
    row = {
        "address": details["address"],
        "settlement_role": SETTLEMENT[role],
        "deposit_strk": strk(details["deposit"]),
        "future_writes": details["future_writes"],
        "fee_per_write_strk": strk(details["fee_per_write"]),
        "required_allowance": details["required_allowance"],
        "required_allowance_strk": strk(details["required_allowance"]),
        "public_balance_strk": strk(details["public_balance"]),
        "target_public_balance": details["target_public_balance"],
        "send_until_balance_is_strk": strk(details["target_public_balance"]),
        "gas_budget_is_estimate": True,
    }
    if inspection is not None:
        row["gas_reserve_per_transaction_strk"] = strk(inspection["gas_reserve_per_write"])
    if "shortfall" in details:
        row["shortfall_strk"] = strk(details["shortfall"])
    return row


def _up_live(args, deps: Deps, directory: Path) -> dict:
    if args.network != NETWORK:
        raise ValueError(
            f"this quickstart runs on Starknet mainnet only, not {args.network!r}; custom networks are refused "
            "(use erebus-init directly for Sepolia)"
        )
    resuming = directory.exists()
    saved_maximum = _load(directory).get("max_strk") if resuming else None
    if args.max_strk is None and saved_maximum is None:
        return {"status": "configuration_required", "missing": ["--max-strk"],
                "message": "mainnet needs --max-strk: the most STRK both identities may be funded with"}
    maximum = setup.amount(args.max_strk) if args.max_strk is not None else int(saved_maximum)
    rpc_url = args.rpc_url
    if args.rpc_url_file is not None:
        rpc_url = _owner_only_text(args.rpc_url_file.expanduser(), "--rpc-url-file")
    if resuming:
        state = _load(directory)
        if state["backend"] != "starknet":
            raise ValueError("this directory holds a mock quickstart; a live one needs a new directory")
        if state.get("completed"):
            raise ValueError("this quickstart is already set up; use quickstart start, status or renew")
        rpc_url = rpc_url or _values(directory, "buyer").get("STARKNET_RPC_URL") or \
            _values(directory, "seller").get("STARKNET_RPC_URL")
        if rpc_url and _host(rpc_url) != state["rpc_host"]:
            raise ValueError(f"this quickstart uses the RPC host {state['rpc_host']}; keep that endpoint")
        # Only the prover host is saved; the URL comes from a role's protected config.
        prover_url = args.prover_url or _values(directory, "buyer").get("PROVING_SERVICE_URL") or \
            _values(directory, "seller").get("PROVING_SERVICE_URL") or DEFAULT_PROVER_URL
        if _host(prover_url) != state["prover_host"]:
            raise ValueError(f"this quickstart uses the prover host {state['prover_host']}; keep that endpoint")
        state["max_strk"] = str(maximum)
    else:
        if not rpc_url:
            return {"status": "configuration_required", "missing": ["--rpc-url or --rpc-url-file"]}
        prover_url = args.prover_url or DEFAULT_PROVER_URL
        _check_endpoints(rpc_url, prover_url)
        imports = {"buyer": _validate_import(args.buyer_account, "--buyer-account"),
                   "seller": _validate_import(args.seller_account, "--seller-account")}
        state = _new_state(args, "starknet")
        state.update(server_command=resolve_server(args.server_command), cli=resolve_cli(args.cli),
                     rpc_host=_host(rpc_url), prover_host=_host(prover_url),
                     max_strk=str(maximum),
                     accounts_file=str(args.accounts_file.expanduser().absolute()) if args.accounts_file else None)
        if not state["cli"]:
            raise ValueError("erebus-cli is not installed beside erebus-mcp-server; reinstall, or pass --cli")
        for role, selected in imports.items():
            if selected:
                state["roles"][role]["import"] = selected
    if rpc_url:
        _check_endpoints(rpc_url, prover_url)
        _check_chain(rpc_url)
    factory = _factory(state, deps)
    spoken = factory(None).version().get("protocol")
    if spoken != PROTOCOL:
        raise ValueError(f"{state['cli']} speaks seam protocol {spoken}, this install expects {PROTOCOL}; "
                         "reinstall erebus-mcp-server and erebus-cli from one release")
    starkscan_key = None
    if any(not _manifest_path(directory, role).exists() for role in ROLES):
        starkscan_key = _starkscan_key(args, deps)
        if not starkscan_key:
            return {"status": "configuration_required",
                    "missing": ["STARKSCAN_API_KEY environment variable or --starkscan-key-file"]}
    if not resuming:
        directory.mkdir(mode=0o700, parents=True)
        _save(directory, state)

    def plan_pass() -> dict[str, tuple[str, dict]]:
        results = {}
        for role in ROLES:
            options = _setup_options(directory, role, state, yes=False, wait=0, rpc_url=rpc_url,
                                     prover_url=prover_url, starkscan_key=starkscan_key)
            results[role] = setup.run(options, factory)
        return results

    results = plan_pass()
    for role, (status, details) in results.items():
        if status not in {"funding_required", "authorization_required"}:
            return {"status": status, "role": role, **details,
                    "next": f"resolve the {role}'s {status}, then rerun quickstart up --directory {directory}"}
    for role, (_, details) in results.items():
        state["roles"][role]["address"] = details["address"]
    state["fee_per_write"] = results["buyer"][1]["fee_per_write"]
    _save(directory, state)
    files = _agent_files(directory, state)

    def summarize(results) -> tuple[dict, int]:
        rows = {}
        for role, (_, details) in results.items():
            values = _values(directory, role)
            inspection = _inspect(factory, directory, role, values) if values.get("AGENT_ADDRESS") else None
            rows[role] = _plan_row(directory, role, details, inspection)
        total = sum(int(details["target_public_balance"]) for _, details in results.values())
        return rows, total

    rows, total = summarize(results)
    base = {"directory": str(directory), "network": NETWORK, "chain_id": CHAIN_ID_NAME,
            "rpc_host": state["rpc_host"], "prover_host": state["prover_host"], "agent_files": files}
    if total > maximum:
        return {"status": "plan_exceeds_max", **base, "plan": rows, "total_strk": strk(total),
                "max_strk": strk(maximum),
                "next": "raise --max-strk, or lower --buyer-deposit/--extra-rounds in a new directory"}
    if any(status in WAITING for status, _ in results.values()):
        deps.log(json.dumps({"event": "funding_required", "total_strk": strk(total), "plan": rows}))
        deadline = deps.monotonic() + args.funding_timeout
        while any(status in WAITING for status, _ in results.values()):
            if deps.monotonic() >= deadline:
                return {"status": "funding_required", **base, "plan": rows, "total_strk": strk(total),
                        "next": "send each address STRK until its public balance reaches send_until_balance_is_strk "
                                f"on Starknet mainnet, then rerun quickstart up --directory {directory}"}
            deps.sleep(10)
            results = plan_pass()
            for role, (status, details) in results.items():
                if status not in {"funding_required", "authorization_required"}:
                    return {"status": status, "role": role, **details}
        rows, total = summarize(results)
        if total > maximum:
            return {"status": "plan_exceeds_max", **base, "plan": rows, "total_strk": strk(total),
                    "max_strk": strk(maximum)}

    allowances = {role: int(details["required_allowance"]) for role, (_, details) in results.items()}
    confirmed = state.get("confirmed")
    already = confirmed is not None and total <= int(confirmed["total"]) and all(
        allowances[role] <= int(confirmed["allowance"][role]) for role in ROLES)
    if not already:
        phrase = f"mainnet {strk(total)}"
        notice = (
            f"Starknet MAINNET plan for two identities, total {strk(total)} STRK (cap {strk(maximum)}):\n"
            + "".join(f"  {role} {row['address']}: shield {row['deposit_strk']}, approve "
                      f"{row['required_allowance_strk']}, fund to {row['send_until_balance_is_strk']} STRK\n"
                      for role, row in rows.items())
            + f"The Starkscan prover ({state['prover_host']}) and your RPC ({state['rpc_host']}) receive each "
            "identity's pool key during every proof. Registration also encrypts the pool key to the pool "
            "auditor. Gas is an estimate."
        )
        if args.yes:
            pass
        elif deps.interactive:
            deps.log(notice)
            if deps.input(f'Type "{phrase}" to deploy, approve and shield both identities: ').strip() != phrase:
                return {"status": "authorization_required", **base, "plan": rows, "total_strk": strk(total),
                        "next": "nothing was sent; rerun up to confirm"}
        else:
            return {"status": "authorization_required", **base, "plan": rows, "total_strk": strk(total),
                    "notice": notice, "next": f"review the plan, then rerun with --yes (confirms {phrase})"}
        state["confirmed"] = {"total": str(total), "allowance": {role: str(v) for role, v in allowances.items()},
                              "at": int(time.time())}
        _save(directory, state)

    for role in ROLES:
        options = _setup_options(directory, role, state, yes=True, wait=args.setup_wait,
                                 max_allowance=int(state["confirmed"]["allowance"][role]))
        status, details = setup.run(options, factory)
        if status != "ready":
            return {"status": status, "role": role, **base, **details,
                    "next": f"rerun quickstart up --directory {directory}: finished steps never repeat"}
    state["completed"] = True
    _save(directory, state)
    readiness = start(argparse.Namespace(directory=directory), deps)
    return {**readiness, "status": "ready" if readiness["status"] == "ready" else readiness["status"], **base}


# --- start / status / renew / down ------------------------------------------------------------


def start(args, deps: Deps) -> dict:
    """Read-only readiness, then the MCP registration commands and prompts."""
    directory = args.directory.expanduser().absolute()
    state = _load(directory)
    checks: dict[str, dict] = {}
    if state["backend"] == "mock":
        for role in ROLES:
            values = _values(directory, role)
            checks[role] = {"config": bool(values.get("AGENT_ADDRESS")), "backend": values.get("EREBUS_BACKEND")}
        ready = all(entry["config"] and entry["backend"] == "mock" for entry in checks.values())
    else:
        if not state.get("completed"):
            return {"status": "not_ready", "next": f"finish quickstart up --directory {directory} first"}
        factory = _factory(state, deps)
        registered = {}
        for role in ROLES:
            values = _values(directory, role)
            inspection = _inspect(factory, directory, role, values)
            configured = _configured(factory, values)
            budget = _budget(values)
            remaining = max(0, (budget.maximum or 0) - budget.used())
            fee = int(inspection["fee_per_write"])
            lag = int(inspection["proving_block_lag"])
            findings = configured.reconcile()
            unresolved = [item["operation_id"] for item in findings if item.get("next_action") not in CLEAN_FINDINGS]
            pending = [record.operation_id for record in IntentStore(Path(values["EREBUS_INTENT_STATE_DIR"])).pending()]
            approvals = _approvals(directory, role, state)
            registered[role] = bool(int(inspection["registered_public_key"], 16))
            checks[role] = {
                "address": values["AGENT_ADDRESS"],
                "registered": registered[role],
                "allowance_covers_remaining_writes": int(inspection["allowance"]) >= remaining * fee,
                "public_balance_covers_remaining_fees": int(inspection["public_balance"]) >= remaining * fee,
                "approvals_at_proving_depth": all(setup.mature(values["STARKNET_RPC_URL"], tx, lag) for tx in approvals),
                "reconcile_clean": not unresolved and not pending,
                "doctor_ready": bool(configured.doctor().get("ready")),
                "remaining_writes": remaining,
                "fee_per_write_strk": strk(fee),
                "unresolved_operations": unresolved + pending,
            }
        for role in ROLES:
            other = "seller" if role == "buyer" else "buyer"
            checks[role]["counterparty_registered"] = registered[other]
        flags = ("registered", "counterparty_registered", "allowance_covers_remaining_writes",
                 "public_balance_covers_remaining_fees", "approvals_at_proving_depth", "reconcile_clean",
                 "doctor_ready")
        ready = all(checks[role][flag] for role in ROLES for flag in flags)
    result = {"status": "ready" if ready else "not_ready", "backend": state["backend"], "checks": checks}
    if ready:
        result.update(
            register=_registration(directory, state),
            prompts={role: (directory / "agents" / role / "prompt.md").read_text() for role in ROLES},
            next="register each role's server with its own agent, start the seller first, and paste each prompt.md",
        )
    else:
        result["next"] = "resolve the failing checks (quickstart status shows detail), then run quickstart start"
    return result


def status(args, deps: Deps) -> dict:
    directory = args.directory.expanduser().absolute()
    state = _load(directory)
    live = state["backend"] == "starknet" and not args.offline
    factory = _factory(state, deps) if live else None
    roles = {}
    for role in ROLES:
        values = _values(directory, role)
        entry: dict[str, Any] = {"server": SERVER_NAMES[role], "settlement_role": SETTLEMENT[role],
                                 "address": values.get("AGENT_ADDRESS"), "config": str(_config_path(directory, role))}
        if values.get("EREBUS_INTENT_STATE_DIR"):
            budget = _budget(values)
            entry |= {"writes_used": budget.used(), "writes_cap": budget.maximum,
                      "pending_intents": [record.operation_id for record in
                                          IntentStore(Path(values["EREBUS_INTENT_STATE_DIR"])).pending()]}
        manifest = _manifest_path(directory, role)
        if manifest.exists():
            saved = json.loads(manifest.read_text())
            entry["setup_steps_done"] = saved["done"]
            entry["setup_steps_unfinished"] = [step for step in saved["operations"] if step not in saved["done"]]
        if live and values.get("AGENT_ADDRESS"):
            inspection = _inspect(factory, directory, role, values)
            fee = int(inspection["fee_per_write"])
            entry |= {
                "deployed": _deployed(factory, directory, role, values),
                "registered": bool(int(inspection["registered_public_key"], 16)),
                "public_strk": strk(inspection["public_balance"]),
                "allowance_strk": strk(inspection["allowance"]),
                "allowance_covers_writes": int(inspection["allowance"]) // fee if fee else None,
                "fee_per_write_strk": strk(fee),
            }
            if (Path(values["POOL_KEY_FILE"])).exists():
                configured = _configured(factory, values)
                notes = configured.balance()
                entry["notes_strk"] = [strk(note) for note in notes.get("notes", [])]
                entry["pending_notes_strk"] = [strk(note) for note in notes.get("pending", [])]
                entry["unresolved_operations"] = [
                    item["operation_id"] for item in configured.reconcile()
                    if item.get("next_action") not in CLEAN_FINDINGS]
        roles[role] = entry
    return {"status": "ok", "backend": state["backend"], "network": state["network"], "chain_id": state["chain_id"],
            "rpc_host": state.get("rpc_host"), "prover_host": state.get("prover_host"),
            "setup_complete": state["backend"] == "mock" or bool(state.get("completed")), "roles": roles,
            "offline": not live}


def renew(args, deps: Deps) -> dict:
    """Approve a fresh allowance for N more deals (an ERC-20 approve replaces the old one) and raise
    each identity's write cap to match. The agents' MCP servers read the new cap when restarted."""
    directory = args.directory.expanduser().absolute()
    state = _load(directory)
    deals = args.deals
    if deals is None or deals < 1:
        raise ValueError("renew needs --deals N (N >= 1)")
    caps = {}
    if state["backend"] == "mock":
        for role in ROLES:
            values = _values(directory, role)
            caps[role] = _budget(values).used() + WRITES_PER_RENEWED_DEAL[role] * deals
            _rewrite_config(_config_path(directory, role), {"EREBUS_MAX_WRITES": str(caps[role])})
        return {"status": "renewed", "backend": "mock", "deals": deals, "max_writes": caps,
                "next": "restart both MCP servers so they read the new write caps"}
    if not state.get("completed"):
        raise ValueError("finish quickstart up before renewing")
    factory = _factory(state, deps)
    renewal = state.get("renewal")
    if renewal is not None and renewal["deals"] != deals:
        raise ValueError(f"a renew for --deals {renewal['deals']} is unfinished; rerun it with the same --deals")
    if renewal is None:
        if args.max_strk is None:
            return {"status": "configuration_required", "missing": ["--max-strk"]}
        maximum = setup.amount(args.max_strk)
        plan, total, short = {}, 0, {}
        for role in ROLES:
            values = _values(directory, role)
            inspection = _inspect(factory, directory, role, values)
            amount = WRITES_PER_RENEWED_DEAL[role] * deals * int(inspection["fee_per_write"])
            gas = int(inspection["gas_reserve_per_write"])
            need = amount + gas
            plan[role] = {"address": values["AGENT_ADDRESS"], "approve": str(amount), "approve_strk": strk(amount),
                          "public_balance_strk": strk(inspection["public_balance"]),
                          "fee_per_write_strk": strk(inspection["fee_per_write"])}
            total += need
            if int(inspection["public_balance"]) < need:
                short[role] = strk(need - int(inspection["public_balance"]))
        if total > maximum:
            return {"status": "plan_exceeds_max", "plan": plan, "total_strk": strk(total), "max_strk": strk(maximum)}
        if short:
            return {"status": "funding_required", "plan": plan, "shortfall_strk": short,
                    "next": "send the shortfall to each address, then rerun renew"}
        phrase = f"mainnet {strk(total)}"
        if not args.yes:
            if not deps.interactive:
                return {"status": "authorization_required", "plan": plan, "total_strk": strk(total),
                        "next": f"rerun with --yes (confirms {phrase})"}
            deps.log(json.dumps(plan, indent=2))
            if deps.input(f'Type "{phrase}" to approve both allowances: ').strip() != phrase:
                return {"status": "authorization_required", "plan": plan, "next": "nothing was sent"}
        renewal = {"deals": deals, "roles": {role: {"id": new_operation_id(), "amount": plan[role]["approve"]}
                                             for role in ROLES}}
        state["renewal"] = renewal
        _save(directory, state)
    for role in ROLES:
        values = _values(directory, role)
        record = renewal["roles"][role]
        configured = _configured(factory, values)
        if not record.get("result"):
            known = {item["operation_id"]: item for item in configured.reconcile()}
            if record["id"] in known:
                outcome, completed = setup.recover_operation(configured, record, known[record["id"]])
                if outcome != "done":
                    return {"status": "operation_pending", "role": role, "operation_id": record["id"],
                            "detail": completed, "next": "rerun renew with the same --deals; it never approves twice"}
            else:
                completed = configured.approve(record["id"], record["amount"])
            record["result"] = completed
            _save(directory, state)
        lag = int(_inspect(factory, directory, role, values)["proving_block_lag"])
        deadline = deps.monotonic() + args.setup_wait
        while not setup.mature(values["STARKNET_RPC_URL"], record["result"]["tx_hash"], lag):
            if deps.monotonic() >= deadline:
                return {"status": "approval_pending", "role": role, "transaction_hash": record["result"]["tx_hash"],
                        "next": "rerun renew with the same --deals once the approval is 10 blocks deep"}
            deps.sleep(5)
        if not record.get("max_writes"):
            record["max_writes"] = _budget(values).used() + WRITES_PER_RENEWED_DEAL[role] * deals
            _rewrite_config(_config_path(directory, role), {"EREBUS_MAX_WRITES": str(record["max_writes"])})
            _save(directory, state)
        caps[role] = record["max_writes"]
    state.setdefault("renewals", []).append(renewal)
    del state["renewal"]
    _save(directory, state)
    return {"status": "renewed", "deals": deals, "max_writes": caps,
            "approved_strk": {role: strk(renewal["roles"][role]["amount"]) for role in ROLES},
            "next": "restart both MCP servers so they read the new write caps; the buyer's shielded note must "
                    "still cover each price"}


def down(args, deps: Deps) -> dict:
    """Removal commands, balances and sweep instructions. Nothing is deleted or sent."""
    directory = args.directory.expanduser().absolute()
    state = _load(directory)
    snapshot = status(args, deps)
    roles = {}
    for role in ROLES:
        name = SERVER_NAMES[role]
        values = _values(directory, role)
        entry = {
            "remove": {
                "claude_code": f"claude mcp remove {name}",
                "codex": f"delete the [mcp_servers.{name}] table from ~/.codex/config.toml",
                "cursor": f"delete the {name} entry from the project's .cursor/mcp.json",
            },
            "balances": {key: snapshot["roles"][role].get(key) for key in ("public_strk", "notes_strk", "allowance_strk")},
        }
        if state["backend"] == "starknet":
            entry["sweep"] = {
                "public_strk": f"send it from {values.get('AGENT_ADDRESS')} with a Starknet wallet or sncast that "
                               f"holds the account key in {values.get('ACCOUNT_KEY_FILE')}; this command sends nothing",
                "shielded_notes": "this release has no unshield: the notes stay spendable by this identity in later "
                                  f"settlements, so keep {directory} (its pool key cannot be replaced)",
            }
        roles[role] = entry
    return {"status": "down", "backend": state["backend"], "roles": roles,
            "kept": "keys, configs, state, and funds are untouched; nothing was deleted or sent"}


def run(args, deps: Deps) -> dict:
    return {"up": up, "start": start, "status": status, "renew": renew, "down": down}[args.action](args, deps)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="erebus-init --starknet")
    commands = parser.add_subparsers(dest="command", required=True)
    quick = commands.add_parser("quickstart", help="Starknet mainnet buyer and seller on this machine")
    quick.add_argument("action", choices=("up", "start", "status", "renew", "down"))
    quick.add_argument("--directory", type=Path, required=True)
    quick.add_argument("--mock", action="store_true", help="up: mock backend, no network or funds")
    quick.add_argument("--network", default=NETWORK, help="up: only mainnet is accepted")
    quick.add_argument("--rpc-url", help="up: Starknet mainnet RPC (https); only its host is recorded")
    quick.add_argument("--rpc-url-file", type=Path, help="up: owner-only file holding a keyed RPC URL")
    quick.add_argument("--prover-url", help=f"up: default {DEFAULT_PROVER_URL}")
    quick.add_argument("--starkscan-key-file", type=Path, help="up: owner-only file holding the Starkscan API key "
                       "(default: STARKSCAN_API_KEY, or a hidden prompt)")
    quick.add_argument("--max-strk", help="up/renew: required on mainnet; refuse any plan above this many STRK")
    quick.add_argument("--price-cap", default=DEFAULT_PRICE_CAP, help="up: highest price in STRK (default 0.2)")
    quick.add_argument("--extra-rounds", type=int, default=0, help="up: extra counter rounds per side (default 0)")
    quick.add_argument("--buyer-deposit", default=DEFAULT_DEPOSITS["buyer"], help="up: buyer shield, STRK (0.5)")
    quick.add_argument("--seller-deposit", default=DEFAULT_DEPOSITS["seller"], help="up: seller shield, STRK (0.01)")
    quick.add_argument("--buyer-account", help="up: import this sncast mainnet account id instead of creating one")
    quick.add_argument("--seller-account", help="up: import this sncast mainnet account id instead of creating one")
    quick.add_argument("--accounts-file", type=Path, help="up: sncast accounts file for --*-account")
    quick.add_argument("--funding-timeout", type=int, default=1800, help="up: seconds to wait for funding")
    quick.add_argument("--setup-wait", type=int, default=1200, help="up/renew: seconds to wait for confirmations")
    quick.add_argument("--yes", action="store_true", help="up/renew: authorize the printed plan")
    quick.add_argument("--non-interactive", action="store_true", help="never prompt")
    quick.add_argument("--deals", type=int, help="renew: number of further deals to approve")
    quick.add_argument("--offline", action="store_true", help="status/down: skip RPC reads")
    quick.add_argument("--cli", type=Path, help="erebus-cli to use (default: the one in this install)")
    quick.add_argument("--server-command", type=Path, help="erebus-mcp-server for the MCP configs (default: this install)")
    return parser


def main(argv: list[str] | None = None, deps: Deps | None = None) -> int:
    args = build_parser().parse_args(argv)
    if deps is None:
        deps = Deps(interactive=sys.stdin.isatty() and not args.non_interactive)
    try:
        result = run(args, deps)
    except (ValueError, OnboardingError, ErebusError, OSError, RuntimeError) as error:
        result = {"status": "error", "error": str(error)}
    print(json.dumps(result, indent=2))
    return 0 if result["status"] in OK_STATUSES else 2
