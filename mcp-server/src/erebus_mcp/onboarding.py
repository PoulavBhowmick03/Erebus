"""Protected first-run configuration for the installed Erebus MCP server.

The MCP transport owns stdin and stdout while serving, so setup is a separate command. A
marketplace can read ``configuration_schema()`` and inject the same values as environment
variables; local users can run the interactive initializer once.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import re
import shlex
import stat
import sys
import tempfile
from collections.abc import Callable, Mapping, MutableMapping, Sequence
from pathlib import Path

CONFIG_FILE_ENV = "EREBUS_CONFIG_FILE"
_KEY = re.compile(r"^[A-Z][A-Z0-9_]*$")
_STRK = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d"


class OnboardingError(RuntimeError):
    """The protected setup could not be loaded or created safely."""


def default_config_path(environ: Mapping[str, str] | None = None) -> Path:
    """Return the per-user default; callers use ``--config`` for more identities."""

    values = os.environ if environ is None else environ
    configured_home = values.get("XDG_CONFIG_HOME", "").strip()
    base = Path(configured_home).expanduser() if configured_home else Path.home() / ".config"
    return base / "erebus" / "mcp.env"


def environment_is_configured(environ: Mapping[str, str] | None = None) -> bool:
    """Whether enough launch configuration exists to let ``ServerConfig`` take over."""

    values = os.environ if environ is None else environ
    return all(
        values.get(name, "").strip()
        for name in ("AGENT_ADDRESS", "PROVING_SERVICE_URL", "EREBUS_SETTLEMENT_ROLE")
    )


def resolve_config_path(
    explicit: str | Path | None, environ: Mapping[str, str] | None = None
) -> Path | None:
    """Resolve explicit, environment, then default config without inventing a file."""

    values = os.environ if environ is None else environ
    if explicit is not None:
        return Path(explicit).expanduser()
    configured = values.get(CONFIG_FILE_ENV, "").strip()
    if configured:
        return Path(configured).expanduser()
    default = default_config_path(values)
    return default if default.exists() else None


def load_config_file(
    path: str | Path, environ: MutableMapping[str, str] | None = None
) -> tuple[str, ...]:
    """Load one protected env file without shell evaluation; existing env values win."""

    target = Path(path).expanduser()
    try:
        details = target.stat()
    except OSError as exc:
        raise OnboardingError(f"cannot read Erebus config {target}: {exc}") from exc
    if not stat.S_ISREG(details.st_mode):
        raise OnboardingError(f"Erebus config is not a regular file: {target}")
    if os.name != "nt" and details.st_mode & 0o077:
        raise OnboardingError(
            f"Erebus config {target} is readable by group or others; run chmod 600 {target}"
        )

    destination = os.environ if environ is None else environ
    loaded: list[str] = []
    try:
        lines = target.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise OnboardingError(f"cannot read Erebus config {target}: {exc}") from exc
    for number, raw in enumerate(lines, start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        try:
            words = shlex.split(line, comments=True, posix=True)
        except ValueError as exc:
            raise OnboardingError(f"{target}:{number}: invalid quoting: {exc}") from exc
        if len(words) != 1 or "=" not in words[0]:
            raise OnboardingError(f"{target}:{number}: expected NAME=value")
        key, value = words[0].split("=", 1)
        if not _KEY.fullmatch(key):
            raise OnboardingError(f"{target}:{number}: invalid environment name {key!r}")
        destination.setdefault(key, value)
        loaded.append(key)
    return tuple(loaded)


def write_config_file(path: str | Path, values: Mapping[str, str]) -> Path:
    """Atomically create one mode-0600 config and refuse to replace an existing file."""

    target = Path(path).expanduser()
    if target.exists():
        raise OnboardingError(f"refusing to overwrite existing Erebus config: {target}")
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if os.name != "nt":
        target.parent.chmod(0o700)

    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{target.name}.", dir=target.parent, text=True
    )
    temporary = Path(temporary_name)
    try:
        if os.name != "nt":
            os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            for key, value in values.items():
                if not _KEY.fullmatch(key):
                    raise OnboardingError(f"invalid environment name {key!r}")
                if "\n" in value or "\r" in value or "\0" in value:
                    raise OnboardingError(f"{key} contains a forbidden control character")
                output.write(f"{key}={shlex.quote(value)}\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, target)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return target


def collect_interactive_config(
    *,
    input_fn: Callable[[str], str] = input,
    secret_fn: Callable[[str], str] = getpass.getpass,
) -> dict[str, str]:
    """Collect paths and service configuration; never ask for private-key contents."""

    network = _choice(input_fn, "Network", ("sepolia", "mainnet", "mock"), "sepolia")
    role = _choice(input_fn, "Settlement role", ("payer", "payee", "both"), "both")
    if network == "mock":
        return {
            "EREBUS_BACKEND": "mock",
            "AGENT_ADDRESS": "0xmock",
            "PROVING_SERVICE_URL": "http://unused.invalid",
            "EREBUS_SETTLEMENT_ROLE": role,
        }

    identity_root = Path.home() / ".erebus" / network
    rpc_default = (
        "https://starknet-sepolia-rpc.publicnode.com" if network == "sepolia" else ""
    )
    prover_default = (
        "https://api.starkscan.co/v1/SN_MAIN/prove" if network == "mainnet" else ""
    )
    values = {
        "EREBUS_BACKEND": "seam",
        "EREBUS_NETWORK": network,
        "EREBUS_SETTLEMENT_ROLE": role,
        "STARKNET_RPC_URL": _required(input_fn, "Starknet RPC URL", rpc_default),
        "PROVING_SERVICE_URL": _required(input_fn, "Proving service URL", prover_default),
        "TOKEN_ADDRESS": _required(input_fn, "Token address", _STRK),
        "AGENT_ADDRESS": _required(input_fn, "Agent account address"),
        "POOL_KEY_FILE": _required(
            input_fn, "Pool-key file path", str(identity_root / "pool.key")
        ),
        "ACCOUNT_KEY_FILE": _required(
            input_fn, "Account-key file path", str(identity_root / "account.key")
        ),
        "EREBUS_STATE_DIR": _required(
            input_fn, "State directory", str(identity_root / "state")
        ),
        "EREBUS_WIRE_VERSION": "v3",
    }
    if values["PROVING_SERVICE_URL"].rstrip("/").endswith("/v1/SN_MAIN/prove"):
        values["STARKSCAN_API_KEY"] = _required_secret(secret_fn, "Starkscan API key")
    return values


def interactive_init(
    path: str | Path,
    *,
    input_fn: Callable[[str], str] = input,
    secret_fn: Callable[[str], str] = getpass.getpass,
) -> Path:
    """Run the one-time local setup and persist only the protected configuration."""

    values = collect_interactive_config(input_fn=input_fn, secret_fn=secret_fn)
    return write_config_file(path, values)


def configuration_schema() -> dict[str, object]:
    """Describe marketplace install fields without embedding one platform's manifest."""

    return {
        "version": 1,
        "command": "erebus-mcp-server",
        "fields": [
            {"name": "EREBUS_BACKEND", "choices": ["mock", "seam"], "default": "seam"},
            {
                "name": "EREBUS_NETWORK",
                "choices": ["sepolia", "mainnet"],
                "default": "sepolia",
                "required_when": {"EREBUS_BACKEND": "seam"},
            },
            {
                "name": "EREBUS_SETTLEMENT_ROLE",
                "choices": ["payer", "payee", "both"],
                "default": "both",
            },
            {"name": "STARKNET_RPC_URL", "secret": True, "required_when": {"EREBUS_BACKEND": "seam"}},
            {"name": "PROVING_SERVICE_URL", "secret": True, "required": True},
            {
                "name": "STARKSCAN_API_KEY",
                "secret": True,
                "required_when": {
                    "PROVING_SERVICE_URL": "https://api.starkscan.co/v1/SN_MAIN/prove"
                },
            },
            {"name": "TOKEN_ADDRESS", "required_when": {"EREBUS_BACKEND": "seam"}},
            {"name": "AGENT_ADDRESS", "required": True},
            {"name": "POOL_KEY_FILE", "secret_path": True, "required_when": {"EREBUS_BACKEND": "seam"}},
            {"name": "ACCOUNT_KEY_FILE", "secret_path": True, "required_when": {"EREBUS_BACKEND": "seam"}},
            {"name": "EREBUS_STATE_DIR", "required_when": {"EREBUS_BACKEND": "seam"}},
        ],
    }


def init_main(argv: Sequence[str] | None = None) -> int:
    """Console entry point for ``erebus-init`` and ``erebus-mcp-server init``."""

    parser = argparse.ArgumentParser(prog="erebus-init")
    parser.add_argument("--config", type=Path, default=None)
    args = parser.parse_args(argv)
    target = args.config or default_config_path()
    created = interactive_init(target)
    print(f"Erebus configuration written to {created}")
    print(f"Start with: erebus-mcp-server --config {shlex.quote(str(created))}")
    return 0


def schema_main() -> int:
    """Print the platform-neutral marketplace configuration contract."""

    print(json.dumps(configuration_schema(), indent=2))
    return 0


def _choice(
    input_fn: Callable[[str], str], label: str, choices: tuple[str, ...], default: str
) -> str:
    rendered = "/".join(choices)
    while True:
        value = input_fn(f"{label} [{rendered}] ({default}): ").strip().lower() or default
        if value in choices:
            return value
        print(f"Choose one of: {rendered}", file=sys.stderr)


def _required(
    input_fn: Callable[[str], str], label: str, default: str = ""
) -> str:
    prompt = f"{label}{f' ({default})' if default else ''}: "
    while True:
        value = input_fn(prompt).strip() or default
        if value:
            return value
        print(f"{label} is required", file=sys.stderr)


def _required_secret(secret_fn: Callable[[str], str], label: str) -> str:
    while True:
        value = secret_fn(f"{label}: ").strip()
        if value:
            return value
        print(f"{label} is required", file=sys.stderr)


if __name__ == "__main__":
    raise SystemExit(init_main())
