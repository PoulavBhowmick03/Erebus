"""Tests for the subprocess seam.

**Read the assertions before adding one.** Nothing here asserts a computed value, and that
is the tripwire, not an accident: if a test in this package ever needs to check that some
number came out right, this package has started computing something and has become a third
implementation of the protocol (``erebus/__init__.py``, CLAUDE.md).

So these assert three things only — a call got through, a result came back with the shape
the contract promises, and a failure arrived as a structured error rather than a crash. The
*correctness* of anything inside those results is pinned on the Rust side, against Cairo
reference vectors, where there is exactly one implementation to be wrong.

Nothing here touches the chain either. ``version`` and ``generate_pool_key`` are local, and
the config-bearing calls are given inputs that fail argument parsing before any RPC happens.
A suite that needed a funded testnet account would stop being run.
"""

from __future__ import annotations

import dataclasses
import json
import subprocess
from pathlib import Path

import pytest

from erebus._seam import ErebusError, Seam, SeamConfig, SeamUnavailable

REPO_ROOT = Path(__file__).resolve().parents[3]
CLI = REPO_ROOT / "sdk" / "rs" / "target" / "debug" / "erebus-cli"

pytestmark = pytest.mark.skipif(
    not CLI.exists(),
    reason="erebus-cli not built; run `cargo build --bin erebus-cli` in sdk/rs",
)


@pytest.fixture
def key_files(tmp_path: Path) -> tuple[Path, Path]:
    """Two keys on disk, the way an operator supplies them.

    Note what the test does *not* do: hand either key to Python. It writes files and passes
    paths, which is the whole custody argument for this seam.
    """
    pool = tmp_path / "pool.key"
    pool.write_text("0x1234567890abcdef\n")
    pool.chmod(0o600)
    account = tmp_path / "account.key"
    account.write_text("0xfedcba0987654321\n")
    account.chmod(0o600)
    return pool, account


@pytest.fixture
def config(key_files: tuple[Path, Path], tmp_path: Path) -> SeamConfig:
    pool, account = key_files
    return SeamConfig(
        rpc_url="http://127.0.0.1:1",
        prover_url="http://127.0.0.1:1",
        pool_address="0x254a6b2",
        chain_id="0x534e5f5345504f4c4941",
        account_address="0xa11ce",
        pool_key_file=pool,
        account_key_file=account,
        state_dir=tmp_path / "state",
        token="0x7042",
    )


@pytest.fixture
def seam(config: SeamConfig) -> Seam:
    return Seam(config=config, binary=CLI)


# --- The call gets through ------------------------------------------------------


def test_version_round_trips(seam: Seam) -> None:
    result = seam.version()

    assert result["name"] == "erebus-sdk"
    assert result["protocol"] == 2


def test_generate_pool_key_returns_a_path_and_a_public_key(seam: Seam, tmp_path: Path) -> None:
    result = seam.generate_pool_key(tmp_path / "fresh.key")

    # Shape, not value. What the key *should* be is a cryptographic decision made in Rust;
    # asserting anything about it here would make this package a second opinion on entropy.
    assert result["pool_key_file"] == str(tmp_path / "fresh.key")
    assert result["public_key"].startswith("0x")


def test_generate_pool_key_never_returns_the_private_value(seam: Seam, tmp_path: Path) -> None:
    """The one property that matters about this call: the secret stays on disk."""
    path = tmp_path / "secret.key"
    result = seam.generate_pool_key(path)
    private = path.read_text().strip()

    assert private not in json.dumps(result)


# --- Failures arrive as structure, not as crashes -------------------------------


def test_a_missing_key_file_raises_a_structured_error(config: SeamConfig) -> None:
    broken = dataclasses.replace(config, pool_key_file="/definitely/not/here")

    with pytest.raises(ErebusError) as caught:
        Seam(config=broken, binary=CLI).open_channel("0xb0b")

    assert caught.value.code == "IDENTITY_UNAVAILABLE"
    assert caught.value.retryable is False


def test_a_malformed_argument_raises_rather_than_returning_garbage(seam: Seam) -> None:
    with pytest.raises(ErebusError) as caught:
        seam.open_channel("not-a-felt")

    assert caught.value.code == "INVALID_REQUEST"


def test_the_retryable_flag_survives_the_seam(seam: Seam) -> None:
    """The one field agent logic branches on. If it were dropped or defaulted somewhere in
    transit, every failure would look permanent and retry logic would silently never run."""
    with pytest.raises(ErebusError) as caught:
        seam.call("no_such_method")

    assert isinstance(caught.value.retryable, bool)
    assert caught.value.code


def test_an_unknown_method_does_not_hang_or_crash(seam: Seam) -> None:
    with pytest.raises(ErebusError):
        seam.call("definitely_not_a_method", {"anything": 1})


def test_a_config_free_seam_refuses_calls_that_need_one() -> None:
    """Constructing without config is legal, because keygen and version do not need one.
    Using it for a protocol call must fail as a broken setup, not as a protocol error."""
    bare = Seam(binary=CLI)
    bare.version()

    with pytest.raises(SeamUnavailable):
        bare.open_channel("0xb0b")


# --- Broken install is a different failure than a protocol error ----------------


def test_a_missing_binary_is_distinguishable_from_a_protocol_error() -> None:
    with pytest.raises(SeamUnavailable):
        Seam(binary="/definitely/not/erebus-cli").version()


def test_non_json_output_is_reported_as_a_broken_install(tmp_path: Path) -> None:
    """A binary that answers with something other than one JSON envelope is the wrong
    binary, not a failing call — so it must not surface as ErebusError, which agent code
    would treat as an ordinary protocol failure and possibly retry forever."""
    fake = tmp_path / "erebus-cli"
    fake.write_text("#!/bin/sh\necho 'this is not json'\n")
    fake.chmod(0o755)

    with pytest.raises(SeamUnavailable):
        Seam(binary=fake).version()


# --- Custody --------------------------------------------------------------------


def test_neither_key_is_passed_through_python(seam: Seam, key_files: tuple[Path, Path]) -> None:
    """The custody claim, asserted rather than asserted-in-prose.

    The request Python sends must contain each key file's *path* and never its contents.
    """
    pool, account = key_files
    sent: dict[str, object] = {}
    original = subprocess.run

    def capture(*args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return original(*args, **kwargs)  # type: ignore[arg-type]

    subprocess.run = capture  # type: ignore[assignment]
    try:
        with pytest.raises(ErebusError):
            seam.open_channel("not-a-felt")
    finally:
        subprocess.run = original  # type: ignore[assignment]

    rendered = json.dumps(sent)
    assert str(pool) in rendered, "the pool key path should be sent"
    assert str(account) in rendered, "the account key path should be sent"
    assert "1234567890abcdef" not in rendered, "pool key material reached the request body"
    assert "fedcba0987654321" not in rendered, "account key material reached the request body"
