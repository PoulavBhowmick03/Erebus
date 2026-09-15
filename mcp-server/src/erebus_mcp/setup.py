"""Installed account discovery and resumable onboarding orchestration.

Rust owns account files, signatures, and all chain calculations. This module keeps the
user's choices and operation IDs, and reports the next action to humans or agents.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import sys
import tempfile
import time
import urllib.request
from decimal import Decimal, InvalidOperation, localcontext
from pathlib import Path
from typing import Any

from erebus import ErebusError, Seam, SeamConfig, identify_network, network_preset

from erebus_mcp.intent import _FileLock, new_operation_id
from erebus_mcp.onboarding import (
    OnboardingError,
    default_config_path,
    load_config_file,
    write_config_file,
)

STRK = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d"


def config_network(values: dict[str, str]) -> str:
    if values.get("EREBUS_NETWORK"):
        return values["EREBUS_NETWORK"]
    network = identify_network(
        values.get("STARKNET_CHAIN_ID", ""), values.get("POOL_ADDRESS", "")
    )
    return network.value if network else "unknown"


def amount(value: str) -> int:
    try:
        with localcontext() as context:
            context.prec = max(80, len(value) + 20)
            scaled = Decimal(value) * 10**18
            if (
                not scaled.is_finite()
                or scaled < 0
                or scaled >= 2**128
                or scaled != scaled.to_integral_value()
            ):
                raise ValueError
            return int(scaled)
    except (ValueError, InvalidOperation, OverflowError) as exc:
        raise argparse.ArgumentTypeError(
            "use a nonnegative STRK amount with at most 18 decimal places"
        ) from exc


def save(path: Path, data: dict[str, Any]) -> None:
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    try:
        with os.fdopen(descriptor, "w") as stream:
            json.dump(data, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        Path(temporary).unlink(missing_ok=True)


def discover(
    seam: Seam, accounts_file: Path, config_path: Path
) -> list[dict[str, Any]]:
    """Inspect known configuration locations; never read account secrets in Python."""
    paths = {config_path, default_config_path()}
    paths.update(default_config_path().parent.glob("*.env"))
    paths.update((Path.home() / ".erebus").glob("**/mcp.env"))
    paths.update(Path.home().glob(".erebus*/env"))
    rows: list[dict[str, Any]] = []
    for path in sorted(paths):
        if not path.is_file():
            continue
        try:
            values: dict[str, str] = {}
            load_config_file(path, values)
            if values.get("EREBUS_BACKEND") == "mock" or not values.get(
                "AGENT_ADDRESS"
            ):
                continue
            rows.append(
                {
                    "id": str(path),
                    "source": "erebus",
                    "config": str(path),
                    "address": values["AGENT_ADDRESS"],
                    "network": config_network(values),
                    "has_signer": Path(
                        values.get("ACCOUNT_KEY_FILE", "/nonexistent")
                    ).is_file(),
                }
            )
        except OnboardingError:
            # A bad candidate does not conceal the rest, and is never silently imported.
            rows.append(
                {
                    "id": str(path),
                    "source": "erebus",
                    "config": str(path),
                    "error": "config is unreadable or has unsafe permissions",
                }
            )
    accounts = seam.call(
        "onboarding",
        {"action": "discover", "args": {"accounts_file": str(accounts_file)}},
    )
    for account in accounts:
        source_network = account["network"]
        network = {
            "alpha-sepolia": "sepolia",
            "alpha-mainnet": "mainnet",
            "SN_SEPOLIA": "sepolia",
            "SN_MAIN": "mainnet",
        }.get(source_network, source_network)
        rows.append(
            {
                **account,
                "id": f"sncast:{source_network}:{account['name']}",
                "source": "sncast",
                "network": network,
                "source_network": source_network,
            }
        )
    return rows


def rpc(url: str, method: str, params: dict[str, Any] | list[Any]) -> Any:
    # Public endpoints such as `starknet-sepolia-rpc.publicnode.com` answer 403 to the
    # default `Python-urllib/x.y` agent. Without an explicit User-Agent the very first
    # readiness read fails and onboarding reports only "RPC request failed", which names
    # nothing. `curl` succeeds against the same URL, which is what makes this opaque.
    request = urllib.request.Request(
        url,
        json.dumps(
            {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        ).encode(),
        {"Content-Type": "application/json", "User-Agent": "erebus-mcp-server"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            body = json.load(response)
    except Exception as exc:
        raise OnboardingError(
            "RPC request failed; rerun onboarding when the endpoint is available"
        ) from exc
    if "error" in body:
        raise OnboardingError(
            f"RPC {method} returned error code {body['error'].get('code')}"
        )
    return body["result"]


def mature(url: str, tx: str, lag: int) -> bool:
    receipt = rpc(url, "starknet_getTransactionReceipt", {"transaction_hash": tx})
    if receipt.get("execution_status") == "REVERTED":
        raise OnboardingError(
            "onboarding transaction reverted; inspect its receipt before retrying"
        )
    block = receipt.get("block_number")
    return (
        receipt.get("execution_status") == "SUCCEEDED"
        and block is not None
        and rpc(url, "starknet_blockNumber", []) >= block + lag
    )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="erebus-init",
        description="Discover an account or create one, fund it, and configure Erebus.",
    )
    result.add_argument("--config", type=Path)
    result.add_argument("--network", choices=("sepolia", "mainnet", "mock"))
    selection = result.add_mutually_exclusive_group()
    selection.add_argument("--account", help="ID printed by --list-accounts")
    selection.add_argument(
        "--new", action="store_true", help="create a dedicated local account"
    )
    selection.add_argument(
        "--resume", action="store_true", help="continue the saved setup"
    )
    result.add_argument("--list-accounts", action="store_true")
    result.add_argument(
        "--accounts-file",
        type=Path,
        default=Path.home() / ".starknet_accounts/starknet_open_zeppelin_accounts.json",
    )
    result.add_argument("--role", choices=("payer", "payee", "both"), default="both")
    result.add_argument("--rpc-url")
    result.add_argument("--prover-url")
    result.add_argument(
        "--deposit",
        type=amount,
        default=None,
        help="STRK to shield once during this setup (default: 1 for new identities, 0 for existing configs)",
    )
    result.add_argument(
        "--writes",
        type=int,
        default=3,
        help="future charged writes to fund (default: 3)",
    )
    result.add_argument(
        "--yes",
        action="store_true",
        help="authorize the displayed deployment, allowance and shield plan",
    )
    result.add_argument(
        "--json", action="store_true", help="machine-readable output; never prompt"
    )
    result.add_argument(
        "--non-interactive",
        action="store_true",
        help="never prompt; report missing inputs",
    )
    result.add_argument(
        "--wait",
        type=int,
        default=None,
        metavar="SECONDS",
        help="wait for funding and confirmation (default: 300 in a terminal, 0 for agents)",
    )
    return result


def main(argv: list[str] | None = None) -> int:
    options = parser().parse_args(argv)
    interactive = sys.stdin.isatty() and not (options.json or options.non_interactive)
    wait_seconds = (
        options.wait if options.wait is not None else (300 if interactive else 0)
    )
    target = (options.config or default_config_path()).expanduser().absolute()
    resume_command = f"erebus-init --config {shlex.quote(str(target))} --resume"

    def report(status: str, **details: Any) -> int:
        body = {"status": status, **details}
        if status != "ready":
            body["resume"] = resume_command
        if options.json:
            print(json.dumps(body))
        else:
            print(status.replace("_", " ").capitalize())
            for key, value in body.items():
                if key != "status":
                    print(
                        f"  {key}: {json.dumps(value) if isinstance(value, (dict, list)) else value}"
                    )
        return 0 if status in {"ready", "accounts"} else 2

    try:
        if not 1 <= options.writes < 2**32:
            raise OnboardingError("--writes must be between 1 and 4294967295")
        if wait_seconds < 0:
            raise OnboardingError("--wait must be nonnegative")
        seam = Seam(binary=os.environ.get("EREBUS_CLI"))
        if options.list_accounts:
            return report(
                "accounts", accounts=discover(seam, options.accounts_file, target)
            )
        target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        manifest_path = target.with_suffix(target.suffix + ".setup.json")
        with _FileLock(target.with_suffix(target.suffix + ".setup.lock")):
            if manifest_path.exists():
                if options.new or options.account:
                    raise OnboardingError(
                        "setup already exists; use --resume or select a different --config for another identity"
                    )
                if manifest_path.stat().st_mode & 0o077:
                    raise OnboardingError("setup state must have mode 0600")
                state = json.loads(manifest_path.read_text())
                if options.network and options.network != state["network"]:
                    raise OnboardingError("network differs from saved setup")
                if target.exists():
                    current: dict[str, str] = {}
                    load_config_file(target, current)
                    for name in (
                        "AGENT_ADDRESS",
                        "POOL_KEY_FILE",
                        "ACCOUNT_KEY_FILE",
                        "EREBUS_STATE_DIR",
                        "EREBUS_NETWORK",
                        "TOKEN_ADDRESS",
                    ):
                        if current.get(name) != state["values"].get(name):
                            raise OnboardingError(
                                f"{name} changed since setup; restore the selected identity before resuming"
                            )
                    # Operators can repair endpoint credentials in their protected config.
                    for name in (
                        "STARKNET_RPC_URL",
                        "PROVING_SERVICE_URL",
                        "STARKSCAN_API_KEY",
                    ):
                        if current.get(name):
                            state["values"][name] = current[name]
                for name, supplied in (
                    ("STARKNET_RPC_URL", options.rpc_url),
                    ("PROVING_SERVICE_URL", options.prover_url),
                ):
                    if supplied and supplied != state["values"].get(name):
                        raise OnboardingError(
                            f"update {name} in the protected config before resuming"
                        )
                if os.environ.get("STARKSCAN_API_KEY"):
                    state["values"]["STARKSCAN_API_KEY"] = os.environ[
                        "STARKSCAN_API_KEY"
                    ]
                save(manifest_path, state)
                if (
                    options.deposit is not None
                    and str(options.deposit) != state["deposit"]
                ):
                    if state["operations"]:
                        raise OnboardingError(
                            "cannot change deposit after setup writes have started"
                        )
                    state["deposit"] = str(options.deposit)
                    save(manifest_path, state)
            else:
                if options.resume and not target.exists():
                    raise OnboardingError("no saved setup found at this config path")
                candidates = (
                    []
                    if options.new or options.network == "mock"
                    else discover(seam, options.accounts_file, target)
                )
                selected = None
                if options.account:
                    selected = next(
                        (row for row in candidates if row["id"] == options.account),
                        None,
                    )
                    if selected is None:
                        raise OnboardingError("account not found; run --list-accounts")
                elif target.exists() and options.resume:
                    selected = {"source": "erebus", "config": str(target)}
                elif not options.new and options.network != "mock":
                    if not interactive:
                        return report(
                            "selection_required",
                            accounts=candidates,
                            choices="--account <id> or --new",
                        )
                    for index, row in enumerate(candidates, 1):
                        print(
                            f"{index}. {row['id']} {row.get('network', '')} {row.get('address', '')} {row.get('error', '')}",
                            file=sys.stderr,
                        )
                    answer = input("Use account number, or type new: ").strip()
                    if answer != "new":
                        try:
                            index = int(answer)
                            if index < 1:
                                raise ValueError
                            selected = candidates[index - 1]
                        except (ValueError, IndexError) as exc:
                            raise OnboardingError(
                                "choose a listed account number or new"
                            ) from exc
                network = options.network or (selected or {}).get("network")
                if network not in {"sepolia", "mainnet", "mock"}:
                    if not interactive:
                        return report(
                            "configuration_required",
                            missing=["--network sepolia|mainnet|mock"],
                        )
                    network = (
                        input("Network [sepolia/mainnet/mock] (sepolia): ").strip()
                        or "sepolia"
                    )
                    if network not in {"sepolia", "mainnet", "mock"}:
                        raise OnboardingError("invalid network")
                values: dict[str, str] = {}
                if selected and selected["source"] == "erebus":
                    load_config_file(selected["config"], values)
                    if config_network(values) != network:
                        raise OnboardingError(
                            "selected config network differs; preserve that identity and select the correct network"
                        )
                    missing = [
                        name
                        for name in (
                            "ACCOUNT_KEY_FILE",
                            "POOL_KEY_FILE",
                            "EREBUS_STATE_DIR",
                            "EREBUS_SETTLEMENT_ROLE",
                            "TOKEN_ADDRESS",
                        )
                        if not values.get(name)
                    ]
                    if missing:
                        return report(
                            "configuration_required",
                            missing=missing,
                            message="Add these fields to the selected protected config, then select it again.",
                        )
                    if Path(selected["config"]) == target and any(
                        value and value != values.get(key)
                        for key, value in (
                            ("STARKNET_RPC_URL", options.rpc_url),
                            ("PROVING_SERVICE_URL", options.prover_url),
                        )
                    ):
                        raise OnboardingError(
                            "endpoint overrides require a new --config path so the original identity config is preserved"
                        )
                elif target.exists():
                    raise OnboardingError(
                        "config already exists; select it or use a different --config for a new identity"
                    )
                if network == "mock":
                    values = {
                        "EREBUS_BACKEND": "mock",
                        "AGENT_ADDRESS": "0xmock",
                        "PROVING_SERVICE_URL": "http://unused.invalid",
                        "EREBUS_SETTLEMENT_ROLE": options.role,
                    }
                    write_config_file(target, values)
                    return report(
                        "ready",
                        config=str(target),
                        command=f"erebus-mcp-server --config {shlex.quote(str(target))}",
                    )
                rpc_url = (
                    options.rpc_url
                    or values.get("STARKNET_RPC_URL")
                    or os.environ.get("STARKNET_RPC_URL")
                )
                prover = (
                    options.prover_url
                    or values.get("PROVING_SERVICE_URL")
                    or os.environ.get("PROVING_SERVICE_URL")
                )
                if network == "mainnet" and not prover:
                    prover = "https://api.starkscan.co/v1/SN_MAIN/prove"
                for name, current in (("RPC URL", rpc_url), ("Prover URL", prover)):
                    if not current and interactive:
                        supplied = input(f"{name}: ").strip()
                        if name == "RPC URL":
                            rpc_url = supplied
                        else:
                            prover = supplied
                if not rpc_url or not prover:
                    return report(
                        "configuration_required",
                        missing=[
                            name
                            for name, v in (
                                ("--rpc-url", rpc_url),
                                ("--prover-url", prover),
                            )
                            if not v
                        ],
                    )
                values.update(
                    {"STARKNET_RPC_URL": rpc_url, "PROVING_SERVICE_URL": prover}
                )
                if prover.rstrip("/").endswith("/v1/SN_MAIN/prove"):
                    key = values.get("STARKSCAN_API_KEY") or os.environ.get(
                        "STARKSCAN_API_KEY"
                    )
                    if not key and interactive:
                        import getpass

                        key = getpass.getpass("Starkscan API key: ")
                    if not key:
                        return report(
                            "configuration_required",
                            missing=["STARKSCAN_API_KEY environment variable"],
                        )
                    values["STARKSCAN_API_KEY"] = key
                directory = target.parent / (target.name + ".identity")
                directory.mkdir(mode=0o700, exist_ok=True)
                directory.chmod(0o700)
                state = {
                    "network": network,
                    "selected": selected,
                    "directory": str(directory),
                    "values": values,
                    "role": options.role,
                    "accounts_file": str(options.accounts_file.expanduser().absolute()),
                    "deposit": str(
                        options.deposit
                        if options.deposit is not None
                        else (0 if values.get("AGENT_ADDRESS") else 10**18)
                    ),
                    "writes": options.writes,
                    "operations": {},
                    "done": [],
                }
                save(manifest_path, state)
            deadline = time.monotonic() + wait_seconds
            while True:
                status, details = advance(
                    state,
                    manifest_path,
                    target,
                    seam,
                    options,
                    interactive,
                    lambda status, **details: (status, details),
                )
                if (
                    status
                    not in {
                        "funding_required",
                        "deployment_pending",
                        "approval_pending",
                        "shield_pending",
                        "operation_pending",
                    }
                    or time.monotonic() >= deadline
                ):
                    return report(status, **details)
                print(f"{status}: {json.dumps(details)}", file=sys.stderr)
                time.sleep(min(5, max(0, deadline - time.monotonic())))
    except (OnboardingError, ErebusError, OSError, ValueError, RuntimeError) as exc:
        return report("setup_error", message=str(exc))


def advance(state, manifest_path, target, seam, options, interactive, report):
    network, values = state["network"], state["values"]
    preset = network_preset(network)
    directory = Path(state["directory"])

    def account(action, **args):
        return seam.call(
            "onboarding",
            {
                "action": action,
                "args": {
                    "directory": str(directory),
                    "chain_id": preset.chain_id,
                    "rpc_url": values["STARKNET_RPC_URL"],
                    **args,
                },
            },
        )

    if not values.get("AGENT_ADDRESS"):
        selected = state["selected"]
        if selected:
            if selected.get("network") != network:
                raise OnboardingError("selected account belongs to another network")
            if not selected.get("has_signer"):
                raise OnboardingError(
                    "selected address has no local signer; connect a supported signing account"
                )
            metadata = account(
                "import",
                accounts_file=state["accounts_file"],
                source_network=selected["source_network"],
                name=selected["name"],
            )
        else:
            metadata = account("create")
        values.update(
            {
                "AGENT_ADDRESS": metadata["address"],
                "ACCOUNT_KEY_FILE": metadata["account_key_file"],
                "POOL_KEY_FILE": str(directory / "pool.key"),
                "EREBUS_STATE_DIR": str(directory / "state"),
                "EREBUS_BACKEND": "seam",
                "EREBUS_NETWORK": network,
                "EREBUS_SETTLEMENT_ROLE": state["role"],
                "TOKEN_ADDRESS": STRK,
                "EREBUS_WIRE_VERSION": "v3",
            }
        )
        save(manifest_path, state)
    if values.get("TOKEN_ADDRESS") != STRK:
        raise OnboardingError(
            "automatic funding currently supports STRK; preserve this config and fund other tokens through the SDK"
        )
    inspection = account(
        "inspect",
        address=values["AGENT_ADDRESS"],
        pool_address=preset.pool_address,
        token=STRK,
    )
    deployment = (
        account("status")
        if (directory / "account.json").exists()
        else {"deployed": True}
    )
    pool_key = Path(values["POOL_KEY_FILE"])
    if not pool_key.exists():
        if int(inspection["registered_public_key"], 16):
            return report(
                "pool_key_required",
                address=values["AGENT_ADDRESS"],
                message="This address already has a pool identity. Restore its original pool key; it cannot be replaced.",
                pool_key_file=str(pool_key),
            )
        pool_key.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        seam.generate_pool_key(pool_key)
    Path(values["EREBUS_STATE_DIR"]).mkdir(parents=True, exist_ok=True, mode=0o700)
    if not target.exists():
        write_config_file(target, values)
    for key in ("STARKSCAN_API_KEY",):
        if values.get(key):
            os.environ[key] = values[key]
    configured = Seam(
        SeamConfig.for_network(
            network,
            rpc_url=values["STARKNET_RPC_URL"],
            prover_url=values["PROVING_SERVICE_URL"],
            account_address=values["AGENT_ADDRESS"],
            pool_key_file=pool_key,
            account_key_file=values["ACCOUNT_KEY_FILE"],
            state_dir=values["EREBUS_STATE_DIR"],
            token=STRK,
        ),
        binary=os.environ.get("EREBUS_CLI"),
    )
    # Read outcomes before budgeting: a crash after shield must not request its deposit again.
    findings = configured.reconcile() if state["operations"] else []
    known = {finding["operation_id"]: finding for finding in findings}
    shield_id = state["operations"].get("shield", {}).get("id")
    shield_effect = known.get(shield_id, {}).get("outcome") == "effect"
    deposit = 0 if "shield" in state["done"] or shield_effect else int(state["deposit"])
    if not int(inspection["registered_public_key"], 16) and not deposit:
        return report(
            "deposit_required",
            message="An unregistered identity needs a first shield. Start a setup with a positive --deposit.",
        )
    count = state["writes"] + bool(deposit)
    allowance = deposit + int(inspection["fee_per_write"]) * count
    if allowance >= 2**128:
        raise OnboardingError(
            "planned allowance exceeds the supported amount; reduce --deposit or --writes"
        )
    extra_transactions = int(not deployment["deployed"]) + int(
        int(inspection["allowance"]) < allowance
    )
    required = allowance + int(inspection["gas_reserve_per_write"]) * (
        count + extra_transactions
    )
    plan = {
        "network": network,
        "address": values["AGENT_ADDRESS"],
        "deposit": str(deposit),
        "fee_per_write": inspection["fee_per_write"],
        "future_writes": state["writes"],
        "required_allowance": str(allowance),
        "public_balance": inspection["public_balance"],
        "target_public_balance": str(required),
        "gas_budget_is_estimate": True,
        "units": "STRK base units (10^18 = 1 STRK)",
    }
    plan["target_public_balance_strk"] = str(Decimal(required) / 10**18)
    if int(inspection["public_balance"]) < required:
        return report(
            "funding_required",
            **plan,
            shortfall=str(required - int(inspection["public_balance"])),
        )
    if not options.yes:
        if not interactive:
            return report(
                "authorization_required",
                **plan,
                next="rerun with --resume --yes to execute this setup",
            )
        print(json.dumps(plan, indent=2), file=sys.stderr)
        print(
            "Setup registers the pool identity with its auditor. The configured RPC and prover receive the pool key during proving.",
            file=sys.stderr,
        )
        if input("Execute this setup? [y/N]: ").strip().lower() != "y":
            return report("authorization_required", **plan)
        options.yes = True
    if not deployment["deployed"]:
        submitted = account("deploy")
        return report("deployment_pending", **submitted)
    account(
        "verify_signer",
        address=values["AGENT_ADDRESS"],
        account_key_file=values["ACCOUNT_KEY_FILE"],
    )
    preflight = configured.doctor()
    blockers = [
        check
        for check in preflight["checks"]
        if check["status"] not in {"pass", "warn"}
        and check["name"] not in {"allowance", "gas", "registration"}
    ]
    registration = next(
        (check for check in preflight["checks"] if check["name"] == "registration"),
        None,
    )
    if (
        registration
        and int(inspection["registered_public_key"], 16)
        and registration["status"] != "pass"
    ):
        blockers.append(registration)
    if blockers:
        return report("repair_required", checks=blockers)
    # Recover an interrupted write under its original ID before starting another one.
    findings = configured.reconcile()
    known = {finding["operation_id"]: finding for finding in findings}
    for step, record in state["operations"].items():
        if step in state["done"]:
            continue
        finding = known.get(record["id"])
        if finding:
            if finding["outcome"] == "unknown":
                return report(
                    "operation_pending", operation_id=record["id"], detail=finding
                )
            result = configured.resume_operation(record["id"])
            kind = result.get("result")
            if kind in {"rebuilt", "recovered_proof"}:
                completed = result["operation_result"]
            elif kind == "already_complete" and result.get("transaction_hash"):
                completed = {"tx_hash": result["transaction_hash"]}
            else:
                return report(
                    "operation_pending", operation_id=record["id"], detail=result
                )
            record["result"] = completed
            state["done"].append(step)
            save(manifest_path, state)
    other = [
        f
        for f in findings
        if f.get("next_action") not in {"none", "safe_to_retry"}
        and f["operation_id"] not in {r["id"] for r in state["operations"].values()}
    ]
    if other:
        return report("recovery_required", operations=other)

    def write(step, quantity):
        record = state["operations"].get(step)
        if record is None:
            record = {"id": new_operation_id(), "amount": str(quantity)}
            state["operations"][step] = record
            save(manifest_path, state)
        result = getattr(configured, step.split(":")[0])(record["id"], record["amount"])
        record["result"] = result
        state["done"].append(step)
        save(manifest_path, state)
        return result

    approvals = [
        step for step in state["operations"] if step.split(":")[0] == "approve"
    ]
    approval = (
        state["operations"].get(approvals[-1], {}).get("result") if approvals else None
    )
    if approval and not mature(
        values["STARKNET_RPC_URL"], approval["tx_hash"], inspection["proving_block_lag"]
    ):
        return report(
            "approval_pending",
            transaction_hash=approval["tx_hash"],
            message="Waiting for approval to reach proving depth; rerun --resume.",
        )
    if int(configured.allowance()["allowance"]) < allowance:
        step = "approve" if not approvals else f"approve:{len(approvals)}"
        approval = write(step, allowance)
        if not mature(
            values["STARKNET_RPC_URL"],
            approval["tx_hash"],
            inspection["proving_block_lag"],
        ):
            return report(
                "approval_pending",
                transaction_hash=approval["tx_hash"],
                message="Waiting for approval to reach proving depth; rerun --resume.",
            )
    if deposit and "shield" not in state["done"]:
        write("shield", deposit)
    shield = state["operations"].get("shield", {}).get("result")
    if shield and not mature(
        values["STARKNET_RPC_URL"], shield["tx_hash"], inspection["proving_block_lag"]
    ):
        return report("shield_pending", transaction_hash=shield["tx_hash"])
    doctor = configured.doctor()
    if not doctor["ready"]:
        return report("repair_required", checks=doctor["checks"])
    return report(
        "ready",
        config=str(target),
        address=values["AGENT_ADDRESS"],
        command=f"erebus-mcp-server --config {shlex.quote(str(target))}",
        claude_command=f"claude mcp add erebus -- erebus-mcp-server --config {shlex.quote(str(target))}",
    )
