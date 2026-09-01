"""First-run configuration tests; no network, keys, or funds."""

from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from erebus_mcp.onboarding import (
    OnboardingError,
    collect_interactive_config,
    configuration_schema,
    load_config_file,
    resolve_config_path,
    write_config_file,
)


def test_protected_config_round_trips_and_environment_wins(tmp_path: Path) -> None:
    path = write_config_file(
        tmp_path / "mcp.env",
        {
            "EREBUS_BACKEND": "seam",
            "EREBUS_NETWORK": "sepolia",
            "STARKNET_RPC_URL": "https://rpc.invalid/a path#fragment",
        },
    )
    environment = {"EREBUS_NETWORK": "mainnet"}

    loaded = load_config_file(path, environment)

    assert set(loaded) == {"EREBUS_BACKEND", "EREBUS_NETWORK", "STARKNET_RPC_URL"}
    assert environment["EREBUS_BACKEND"] == "seam"
    assert environment["EREBUS_NETWORK"] == "mainnet"
    assert environment["STARKNET_RPC_URL"] == "https://rpc.invalid/a path#fragment"
    if stat.S_IMODE(path.stat().st_mode) != 0:
        assert stat.S_IMODE(path.stat().st_mode) == 0o600


def test_config_writer_refuses_to_replace_existing_file(tmp_path: Path) -> None:
    path = write_config_file(tmp_path / "mcp.env", {"EREBUS_BACKEND": "mock"})

    with pytest.raises(OnboardingError, match="refusing to overwrite"):
        write_config_file(path, {"EREBUS_BACKEND": "seam"})


def test_loader_rejects_group_readable_config(tmp_path: Path) -> None:
    path = tmp_path / "mcp.env"
    path.write_text("EREBUS_BACKEND=mock\n")
    path.chmod(0o640)

    with pytest.raises(OnboardingError, match="chmod 600"):
        load_config_file(path, {})


def test_loader_rejects_shell_syntax_instead_of_evaluating_it(tmp_path: Path) -> None:
    path = tmp_path / "mcp.env"
    path.write_text("export EREBUS_BACKEND=mock\n")
    path.chmod(0o600)

    with pytest.raises(OnboardingError, match="expected NAME=value"):
        load_config_file(path, {})


def test_mock_first_run_needs_only_network_and_role() -> None:
    answers = iter(["mock", "payer"])

    values = collect_interactive_config(input_fn=lambda _: next(answers))

    assert values == {
        "EREBUS_BACKEND": "mock",
        "AGENT_ADDRESS": "0xmock",
        "PROVING_SERVICE_URL": "http://unused.invalid",
        "EREBUS_SETTLEMENT_ROLE": "payer",
    }


def test_sepolia_is_the_first_run_default() -> None:
    answers = iter(
        [
            "",  # network -> Sepolia
            "",  # role -> both
            "",  # default RPC
            "https://prover.invalid",
            "",  # default STRK
            "0xa11ce",
            "/protected/pool.key",
            "/protected/account.key",
            "/protected/state",
        ]
    )

    values = collect_interactive_config(input_fn=lambda _: next(answers))

    assert values["EREBUS_NETWORK"] == "sepolia"
    assert values["EREBUS_SETTLEMENT_ROLE"] == "both"
    assert "STARKSCAN_API_KEY" not in values


def test_mainnet_starkscan_key_is_collected_without_using_plain_input() -> None:
    answers = iter(
        [
            "mainnet",
            "payer",
            "https://rpc.invalid",
            "",  # default Starkscan prover
            "",  # default STRK
            "0xa11ce",
            "/protected/pool.key",
            "/protected/account.key",
            "/protected/state",
        ]
    )

    values = collect_interactive_config(
        input_fn=lambda _: next(answers), secret_fn=lambda _: "protected-api-key"
    )

    assert values["EREBUS_NETWORK"] == "mainnet"
    assert values["STARKSCAN_API_KEY"] == "protected-api-key"


def test_marketplace_schema_is_json_and_marks_secret_inputs() -> None:
    schema = configuration_schema()
    encoded = json.dumps(schema)
    fields = {field["name"]: field for field in schema["fields"]}  # type: ignore[index]

    assert json.loads(encoded)["version"] == 1
    assert fields["EREBUS_NETWORK"]["default"] == "sepolia"
    assert fields["STARKSCAN_API_KEY"]["secret"] is True
    assert fields["POOL_KEY_FILE"]["secret_path"] is True


def test_explicit_config_path_beats_environment(tmp_path: Path) -> None:
    explicit = tmp_path / "explicit.env"
    configured = tmp_path / "configured.env"

    assert resolve_config_path(explicit, {"EREBUS_CONFIG_FILE": str(configured)}) == explicit
