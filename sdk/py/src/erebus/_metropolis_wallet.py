"""Policy-bound wallet commands. Rust owns keys, entropy, proofs and retained bytes."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from erebus._metropolis import MetropolisError, _digest, _native_error


READS = frozenset({"notes", "sync"})
OPTIONAL_READS = frozenset({"status"})
WRITES = frozenset({"approve", "wrap", "unwrap", "deposit", "withdraw", "resume", "replace"})


def _amount(value: str) -> int:
    if not isinstance(value, str) or re.fullmatch(r"[1-9][0-9]{0,38}", value, re.ASCII) is None or int(value) > 2**128 - 1:
        raise MetropolisError("wallet amount must be positive base units within operator policy")
    return int(value)


@dataclass(frozen=True)
class WalletPolicy:
    """Operator authorization, never supplied by a model invocation."""

    maximum_amount: str
    allowed_methods: frozenset[str] = READS
    withdrawal_recipients: frozenset[str] = frozenset()
    replacement_fees: tuple[str, str] | None = None

    def __post_init__(self) -> None:
        _amount(self.maximum_amount)
        if not isinstance(self.allowed_methods, frozenset) or not READS <= self.allowed_methods <= READS | OPTIONAL_READS | WRITES:
            raise MetropolisError("configure explicit supported wallet methods")
        if not isinstance(self.withdrawal_recipients, frozenset) or any(
            not isinstance(value, str) or re.fullmatch(r"0x[0-9a-f]{40}", value, re.ASCII) is None or value == "0x" + "0" * 40
            for value in self.withdrawal_recipients
        ):
            raise MetropolisError("configure canonical nonzero withdrawal recipients")
        if "replace" in self.allowed_methods:
            if not isinstance(self.replacement_fees, tuple) or len(self.replacement_fees) != 2:
                raise MetropolisError("replacement requires explicit operator fee caps")
            maximum, priority = map(_amount, self.replacement_fees)
            if priority > maximum:
                raise MetropolisError("replacement priority fee exceeds its cap")


class MetropolisWallet:
    """One fixed wallet config; no arbitrary target, calldata, key or path arguments."""

    def __init__(self, *, config_file: str, policy: WalletPolicy,
                 binary: str | None = None, timeout: int = 600) -> None:
        if not isinstance(config_file, str) or not Path(config_file).is_absolute() or len(config_file) > 4096:
            raise MetropolisError("configure an absolute wallet configuration path")
        if type(timeout) is not int or not 1 <= timeout <= 900 or not isinstance(policy, WalletPolicy):
            raise MetropolisError("configure bounded native wallet policy")
        self._binary = binary or shutil.which("erebus-wallet")
        if not self._binary:
            raise MetropolisError("install erebus-wallet")
        self._config, self._policy, self._timeout = config_file, policy, timeout

    def _invoke(self, request: dict[str, Any]) -> subprocess.CompletedProcess:
        try:
            return subprocess.run([self._binary], input=json.dumps(request), text=True,
                                  capture_output=True, timeout=self._timeout, check=False)
        except (OSError, UnicodeError, subprocess.TimeoutExpired):
            raise MetropolisError("wallet outcome unknown; retain the original operation and sync") from None

    def version(self) -> dict[str, Any]:
        """Reject older commands before enabling repeatable agent funding."""
        response = self._invoke({"method": "version"})
        try:
            envelope = json.loads(response.stdout)
            result = envelope["result"]
            if response.returncode != 0 or response.stderr or len(response.stdout) > 4096 or envelope.get("status") != "ok" or type(result.get("protocol")) is not int or result["protocol"] != 1:
                raise ValueError
            if not isinstance(result.get("methods"), list) or not self._policy.allowed_methods <= set(result["methods"]):
                raise ValueError
            if "deposit" in self._policy.allowed_methods and result.get("deposit_operation_references") is not True:
                raise ValueError
            if {"approve", "wrap", "unwrap"} & self._policy.allowed_methods and result.get("funding_operation_references") is not True:
                raise ValueError
            if "status" in self._policy.allowed_methods and result.get("paired_account_diagnostics") is not True:
                raise ValueError
        except (TypeError, ValueError, KeyError, AttributeError):
            raise MetropolisError("native wallet lacks the configured capabilities or durable deposit references") from None
        return {"protocol":1, "methods":sorted(self._policy.allowed_methods),
                "deposit_operation_references":result.get("deposit_operation_references") is True,
                "funding_operation_references":result.get("funding_operation_references") is True,
                "paired_account_diagnostics":result.get("paired_account_diagnostics") is True}

    def check(self, method: str, *, amount: str | None = None, operation_ref: str | None = None,
              commitment: str | None = None, recipient: str | None = None,
              identity: str | None = None) -> set[str]:
        """Operator-policy validation of one request, without invoking the native wallet."""
        if method not in self._policy.allowed_methods:
            raise MetropolisError("wallet action is not authorized by operator policy")
        args = {"amount": amount, "operation_ref": operation_ref, "commitment": commitment,
                "recipient": recipient, "identity": identity}
        fields = {"approve": {"amount", "operation_ref"}, "wrap": {"amount", "operation_ref"},
                  "unwrap": {"amount", "operation_ref"}, "deposit": {"amount", "operation_ref"},
                  "withdraw": {"commitment", "recipient"}, "resume": {"identity"}, "replace": {"identity"},
                  "notes": set(), "status": set(), "sync": set()}[method]
        if {name for name, value in args.items() if value is not None} != fields:
            raise MetropolisError("wallet request must contain exactly this action's public arguments")
        if amount is not None and _amount(amount) > int(self._policy.maximum_amount):
            raise MetropolisError("wallet amount exceeds operator policy")
        for name in ("operation_ref", "commitment", "identity"):
            if args[name] is not None and not _digest(args[name]):
                raise MetropolisError("wallet identifier must be nonzero lowercase hex")
        if recipient is not None and recipient not in self._policy.withdrawal_recipients:
            raise MetropolisError("withdrawal recipient is not authorized")
        return fields

    def execute(self, method: str, *, amount: str | None = None,
                operation_ref: str | None = None, commitment: str | None = None,
                recipient: str | None = None, identity: str | None = None) -> dict[str, Any]:
        """One explicit invocation, without automatic retry, replacement or cancellation."""
        fields = self.check(method, amount=amount, operation_ref=operation_ref, commitment=commitment,
                            recipient=recipient, identity=identity)
        args = {"amount": amount, "operation_ref": operation_ref, "commitment": commitment,
                "recipient": recipient, "identity": identity}
        if method == "withdraw":
            notes = self.execute("notes")["result"].get("notes")
            if not isinstance(notes, list):
                raise MetropolisError("wallet note evidence unavailable")
            matching = [note for note in notes if isinstance(note, dict) and note.get("commitment") == commitment]
            if len(matching) != 1 or _amount(matching[0].get("amount")) > int(self._policy.maximum_amount):
                raise MetropolisError("withdrawal note exceeds operator policy or is unavailable")
            if matching[0].get("included") is not True or matching[0].get("consumed") is not False or matching[0].get("reserved") is not False:
                raise MetropolisError("withdrawal needs an included, unspent, unreserved note; recover retained operations separately")
        request = {"method": method, "config_file": self._config,
                   **{name: args[name] for name in fields}}
        if method == "replace":
            request.update(zip(("max_fee_per_gas", "max_priority_fee_per_gas"), self._policy.replacement_fees, strict=True))
        response = self._invoke(request)
        if len(response.stdout) + len(response.stderr) > 131_072:
            raise MetropolisError("wallet response exceeds the bounded public diagnostic size")
        stream = response.stdout if response.returncode == 0 else response.stderr
        if (response.returncode not in {0, 2} or (response.stderr if response.returncode == 0 else response.stdout)):
            raise MetropolisError("wallet action failed; retain state and sync before any retry",
                                  native_error=_native_error(response.stderr, response.stdout))
        try:
            envelope = json.loads(stream)
        except (TypeError, ValueError):
            raise MetropolisError("invalid wallet response; retain original state") from None
        expected = "ok" if response.returncode == 0 else "pending"
        if not isinstance(envelope, dict) or envelope.get("status") != expected or not set(envelope) <= {"status", "result", "error"}:
            raise MetropolisError("invalid wallet response envelope")
        result = envelope.get("result", {})
        self._validate_public(result)
        return {"status": expected, "result": result, "retry_without_new_payment": True,
                "automatic_rebroadcast": False, "payment_verified": False,
                **({"message": "wallet history or operation pending; retain its ID and sync"} if expected == "pending" else {})}

    @classmethod
    def _validate_public(cls, value: Any, depth: int = 0) -> None:
        if depth > 8:
            raise MetropolisError("invalid wallet diagnostic nesting")
        allowed = {"notes", "operations", "commitment", "nullifier", "amount", "asset", "included", "consumed", "reserved",
                   "kind", "identity", "state", "retained", "target", "value", "attempts", "tx_hash", "outcome", "block",
                   "scanned_through", "completed", "reverted", "migration_required", "pending", "nonce_released",
                   "already_completed", "wrapper", "spender", "installation_ms", "proving_ms", "submission", "transaction_hash",
                   "retry_without_new_payment", "payment_verified", "recipient", "replaced", "resume_required",
                   "history_pending", "next_log_block", "through", "account", "native_balance", "asset_balance",
                   "allowance", "anchor_block", "anchor_hash", "anchor_timestamp", "checkpoints", "source",
                   "checkpoint_block", "checkpoint_hash", "blocks_behind", "caught_up",
                   "history_scope", "history_from_block"}
        if isinstance(value, dict):
            if not set(value) <= allowed:
                raise MetropolisError("wallet response contains non-public or unexpected fields")
            for name, item in value.items():
                if name == "payment_verified" and item is not False:
                    raise MetropolisError("wallet response must not claim deal verification")
                cls._validate_public(item, depth + 1)
        elif isinstance(value, list):
            if len(value) > 4096:
                raise MetropolisError("wallet diagnostic list exceeds its bound")
            for item in value:
                cls._validate_public(item, depth + 1)
        elif isinstance(value, str):
            if len(value) > 128 or re.fullmatch(r"[a-z0-9_.:-]+", value, re.ASCII) is None:
                raise MetropolisError("invalid public wallet diagnostic")
        elif value is not None and type(value) not in {int, bool}:
            raise MetropolisError("invalid public wallet diagnostic")
