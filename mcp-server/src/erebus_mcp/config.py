"""Server configuration from environment variables.

``docs/ishita.md``: "It needs a prover URL and identity in its config, and it should fail
loudly rather than fall back to a shared endpoint." Erebus holds no keys, so this process
runs against the operator's prover (``docs/custody-design.md``). ``prover_url`` remains
required for the mock backend so every configuration names its endpoint explicitly.

``mock`` is the default and needs an address and prover URL. ``seam`` drives the Rust client
and needs the full protocol-4 configuration. Startup validation catches a missing
``POOL_KEY_FILE`` before a tool call starts proving.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path


class ConfigError(RuntimeError):
    """A required setting is missing at startup."""


class SettlementRole(str, Enum):
    """Which side of a payment this MCP identity is allowed to take.

    ``accept_and_settle`` spends the caller's notes. A configured role prevents a payee
    server from accepting. ``BOTH`` supports an identity that buys and sells.
    """

    PAYER = "payer"
    PAYEE = "payee"
    BOTH = "both"


@dataclass(frozen=True)
class SeamSettings:
    """Everything ``erebus-cli`` needs, plus where to find it.

    This process passes key-file paths to the Rust binary and never opens them.
    """

    binary: Path
    rpc_url: str
    pool_address: str
    chain_id: str
    pool_key_file: Path
    account_key_file: Path
    state_dir: Path
    token: str
    wire_version: str


@dataclass(frozen=True)
class TokenSpendingLimit:
    """Caps for one token. A missing field means unlimited on that axis."""

    per_deal: int | None = None
    daily: int | None = None


@dataclass(frozen=True)
class SpendingLimits:
    """Per-token spending caps enforced at the MCP layer (roadmap 9.1), not by agent
    policy. ``BuyerPolicy.budget`` stays as negotiation strategy; this is the safety
    boundary underneath it. Empty by default: a server enforces nothing until an operator
    opts in.
    """

    by_token: dict[str, TokenSpendingLimit] = field(default_factory=dict)

    def for_token(self, token: str) -> TokenSpendingLimit:
        return self.by_token.get(token.strip().lower(), TokenSpendingLimit())


@dataclass(frozen=True)
class ServerConfig:
    address: str
    prover_url: str
    backend: str
    settlement_role: SettlementRole
    mock_store_path: Path
    mock_latency_seconds: float
    mock_spendable_notes: tuple[int, ...]
    mock_pending_notes: tuple[int, ...]
    seam: SeamSettings | None = None
    #: Run the read-only `doctor` inspection when the server starts (seam backend only)
    #: and log every non-passing check with its repair. Costs a few RPC round-trips of
    #: startup latency; disable with EREBUS_SKIP_STARTUP_DOCTOR=1 when starting offline.
    startup_doctor: bool = True
    #: Per-token per-deal and daily-cumulative spending caps (9.1). Empty means no caps.
    spending_limits: SpendingLimits = field(default_factory=SpendingLimits)
    #: Where daily cumulative spend is persisted, so a restart does not reset it. Defaults
    #: to a path scoped to this identity; see `_spending_state_path`.
    spending_state_path: Path = field(default_factory=lambda: Path("/tmp/erebus-spending-state.json"))
    #: Base directory for durable caller-intent records (plan.md, Ishita task 1). Scoped
    #: per identity for the same reason spending state is; see `_intent_state_dir`.
    intent_state_dir: Path = field(default_factory=lambda: Path("/tmp/erebus-intent-state"))

    @classmethod
    def from_env(cls) -> ServerConfig:
        # `open_channel(operation_id, counterparty)` supplies the counterparty for each call (§4). Do not
        # bind the server to one counterparty in configuration.
        address = _require("AGENT_ADDRESS")
        prover_url = _require("PROVING_SERVICE_URL")
        backend = os.environ.get("EREBUS_BACKEND", "mock").strip().lower()
        if backend not in {"mock", "seam"}:
            raise ConfigError(f"EREBUS_BACKEND must be 'mock' or 'seam', got {backend!r}")

        role_raw = _require("EREBUS_SETTLEMENT_ROLE").strip().lower()
        try:
            settlement_role = SettlementRole(role_raw)
        except ValueError as exc:
            choices = ", ".join(role.value for role in SettlementRole)
            raise ConfigError(
                f"EREBUS_SETTLEMENT_ROLE must be one of {choices}, got {role_raw!r}"
            ) from exc

        store_path = os.environ.get("EREBUS_MOCK_STORE_PATH", "/tmp/erebus-mock-store.json")
        latency = os.environ.get("EREBUS_MOCK_LATENCY_SECONDS", "0.2")
        try:
            latency_seconds = float(latency)
        except ValueError as exc:
            raise ConfigError(f"EREBUS_MOCK_LATENCY_SECONDS is not a number: {latency!r}") from exc

        mock_spendable = _amounts("EREBUS_MOCK_SPENDABLE_NOTES", "1000000000000000000")
        mock_pending = _amounts("EREBUS_MOCK_PENDING_NOTES", "")

        seam = _seam_settings() if backend == "seam" else None

        return cls(
            address=address,
            prover_url=prover_url,
            backend=backend,
            settlement_role=settlement_role,
            mock_store_path=Path(store_path),
            mock_latency_seconds=latency_seconds,
            mock_spendable_notes=mock_spendable,
            mock_pending_notes=mock_pending,
            seam=seam,
            startup_doctor=not _flag("EREBUS_SKIP_STARTUP_DOCTOR"),
            spending_limits=_spending_limits(),
            spending_state_path=_spending_state_path(address, seam),
            intent_state_dir=_intent_state_dir(address, seam),
        )


def _seam_settings() -> SeamSettings:
    # EREBUS_CLI is optional: the erebus-cli wheel ships the binary, and binary_path()
    # finds it on PATH or inside the installed environment. That fallback matters for the
    # documented install — `uv tool install erebus-mcp-server` exposes only the server's
    # own executable, so the binary is present but not on PATH. An explicit EREBUS_CLI
    # still wins, which is what a developer running a locally built binary expects.
    configured = os.environ.get("EREBUS_CLI", "").strip()
    if configured:
        binary = Path(configured)
        if not binary.exists():
            raise ConfigError(
                f"EREBUS_CLI points at {binary}, which does not exist. Build it with "
                "`cargo build --release --bin erebus-cli` in sdk/rs."
            )
    else:
        from erebus_cli import binary_path

        found = binary_path()
        if found is None:
            raise ConfigError(
                "erebus-cli was not found on PATH or in this environment, and EREBUS_CLI "
                "is unset. Install the erebus-cli package, or build it with "
                "`cargo build --release --bin erebus-cli` in sdk/rs and set EREBUS_CLI to "
                "the resulting path."
            )
        binary = found

    pool_key = Path(_require("POOL_KEY_FILE"))
    account_key = Path(_require("ACCOUNT_KEY_FILE"))
    for label, path in (("POOL_KEY_FILE", pool_key), ("ACCOUNT_KEY_FILE", account_key)):
        if not path.exists():
            raise ConfigError(f"{label} points at {path}, which does not exist")

    wire_version = os.environ.get("EREBUS_WIRE_VERSION", "v3").strip().lower()
    if wire_version not in {"v2", "v3"}:
        raise ConfigError(
            f"EREBUS_WIRE_VERSION must be 'v2' or 'v3', got {wire_version!r}"
        )

    return SeamSettings(
        binary=binary,
        rpc_url=_require("STARKNET_RPC_URL"),
        pool_address=_require("POOL_ADDRESS"),
        chain_id=_require("STARKNET_CHAIN_ID"),
        pool_key_file=pool_key,
        account_key_file=account_key,
        state_dir=Path(_require("EREBUS_STATE_DIR")),
        token=_require("TOKEN_ADDRESS"),
        wire_version=wire_version,
    )


def _spending_limits() -> SpendingLimits:
    """Parses ``EREBUS_SPENDING_LIMITS``, a JSON object keyed by token address:

    ``{"0xtoken...": {"per_deal": "5000000000000000000", "daily": "20000000000000000000"}}``

    Amounts are decimal strings, not JSON numbers, for the same reason `propose_offer`
    takes `amount` as a string (F37): a base-unit amount routinely exceeds 2**53. Unset or
    empty means no caps. A token absent from the map, or a field absent within an entry,
    is unlimited on that axis, not zero.
    """
    raw = os.environ.get("EREBUS_SPENDING_LIMITS", "").strip()
    if not raw:
        return SpendingLimits()
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ConfigError(f"EREBUS_SPENDING_LIMITS is not valid JSON: {exc}") from exc
    if not isinstance(parsed, dict):
        raise ConfigError("EREBUS_SPENDING_LIMITS must be a JSON object keyed by token address")

    by_token: dict[str, TokenSpendingLimit] = {}
    for token, caps in parsed.items():
        if not isinstance(caps, dict):
            raise ConfigError(f"EREBUS_SPENDING_LIMITS[{token!r}] must be a JSON object")
        by_token[token.strip().lower()] = TokenSpendingLimit(
            per_deal=_cap_amount(token, "per_deal", caps.get("per_deal")),
            daily=_cap_amount(token, "daily", caps.get("daily")),
        )
    return SpendingLimits(by_token=by_token)


def _cap_amount(token: str, field_name: str, value: object) -> int | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise ConfigError(
            f"EREBUS_SPENDING_LIMITS[{token!r}][{field_name!r}] must be a decimal string, "
            f"got {value!r}"
        )
    try:
        parsed = int(value)
    except ValueError as exc:
        raise ConfigError(
            f"EREBUS_SPENDING_LIMITS[{token!r}][{field_name!r}] is not an integer: {value!r}"
        ) from exc
    if parsed <= 0:
        raise ConfigError(
            f"EREBUS_SPENDING_LIMITS[{token!r}][{field_name!r}] must be positive, got {parsed}"
        )
    return parsed


def _spending_state_path(address: str, seam: SeamSettings | None) -> Path:
    """Where daily cumulative spend is persisted.

    Must be scoped per identity: a shared default path would let two identities on one
    machine share, and silently corrupt, each other's daily counter. The seam backend
    already has a per-identity state directory; reuse it. The mock backend has none, so
    fall back to a path slugged from the identity address.
    """
    configured = os.environ.get("EREBUS_SPENDING_STATE_PATH", "").strip()
    if configured:
        return Path(configured)
    if seam is not None:
        return seam.state_dir / "spending.json"
    return Path(f"/tmp/erebus-spending-state-{_slug(address)}.json")


def _intent_state_dir(address: str, seam: SeamSettings | None) -> Path:
    """Base directory for `IntentStore`, which appends its own `pending_operations`
    subdirectory. Same identity-scoping rule as `_spending_state_path`: two identities
    sharing a default directory would let one identity's crash record collide with
    another's `IntentStore.begin()` scan.
    """
    configured = os.environ.get("EREBUS_INTENT_STATE_DIR", "").strip()
    if configured:
        return Path(configured)
    if seam is not None:
        return seam.state_dir
    return Path(f"/tmp/erebus-intent-state-{_slug(address)}")


def _slug(value: str) -> str:
    return "".join(c if c.isalnum() else "_" for c in value.strip())


def _flag(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in {"1", "true", "yes", "on"}


def _require(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ConfigError(f"{name} is required and not set")
    return value


def _amounts(name: str, default: str) -> tuple[int, ...]:
    raw = os.environ.get(name, default).strip()
    if not raw:
        return ()
    try:
        values = tuple(int(part.strip()) for part in raw.split(","))
    except ValueError as exc:
        raise ConfigError(f"{name} must be comma-separated integers, got {raw!r}") from exc
    if any(value <= 0 for value in values):
        raise ConfigError(f"{name} values must all be positive")
    return values
