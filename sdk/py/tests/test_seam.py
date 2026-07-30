"""Tests for the subprocess seam.

**Read the assertions before adding one.** Nothing here asserts a computed value, and that
is the tripwire, not an accident: if a test in this package ever needs to check that some
number came out right, this package has started computing something and has become a third
implementation of the protocol (``erebus/__init__.py``, CLAUDE.md).

So these assert three things only — a call got through, a result came back with the shape
the contract promises, and a failure arrived as a structured error rather than a crash. The
*correctness* of anything inside those results is pinned on the Rust side, against Cairo
reference vectors, where there is exactly one implementation to be wrong.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from erebus._seam import ErebusError, Seam, SeamUnavailable

REPO_ROOT = Path(__file__).resolve().parents[3]
CLI = REPO_ROOT / "sdk" / "rs" / "target" / "debug" / "erebus-cli"

pytestmark = pytest.mark.skipif(
    not CLI.exists(),
    reason="erebus-cli not built; run `cargo build --bin erebus-cli` in sdk/rs",
)


@pytest.fixture
def seam() -> Seam:
    return Seam(binary=CLI)


@pytest.fixture
def key_file(tmp_path: Path) -> Path:
    """A pool key on disk, the way an operator supplies one.

    Note what the test does *not* do: hand the key to Python. It writes a file and passes
    the path, which is the whole custody argument for this seam.
    """
    path = tmp_path / "pool.key"
    path.write_text("0x1234567890abcdef")
    path.chmod(0o600)
    return path


# --- The call gets through ------------------------------------------------------


def test_version_round_trips(seam: Seam) -> None:
    result = seam.version()

    assert result["name"] == "erebus-sdk"
    assert result["protocol"] == 1


def test_open_channel_returns_a_handle(seam: Seam, key_file: Path) -> None:
    result = seam.open_channel(
        address="0xa11ce",
        key_file=key_file,
        counterparty_address="0xb0b",
        counterparty_public_key="0x9bcdef",
        token="0x7042",
        register=True,
    )

    # Shape, not value. What the handle *should* be is pinned in Rust against the library;
    # asserting it here would make this package a second opinion on a derivation.
    assert result["channel_handle"].startswith("0x")
    assert result["counterparty"] == "0xb0b"
    assert result["registered"] is True


def test_the_handle_is_stable_across_calls(seam: Seam, key_file: Path) -> None:
    """Not a correctness claim about the derivation — a claim that the seam is not
    injecting nondeterminism of its own, e.g. by reordering or re-encoding arguments."""
    kwargs = dict(
        address="0xa11ce",
        key_file=key_file,
        counterparty_address="0xb0b",
        counterparty_public_key="0x9bcdef",
        token="0x7042",
    )
    first = seam.open_channel(**kwargs)
    second = seam.open_channel(**kwargs)

    assert first["channel_handle"] == second["channel_handle"]


# --- Failures arrive as structure, not as crashes -------------------------------


def test_a_missing_key_file_raises_a_structured_error(seam: Seam) -> None:
    with pytest.raises(ErebusError) as caught:
        seam.open_channel(
            address="0xa11ce",
            key_file="/definitely/not/here",
            counterparty_address="0xb0b",
            counterparty_public_key="0x9bcdef",
            token="0x7042",
        )

    assert caught.value.code == "IDENTITY_UNAVAILABLE"
    assert caught.value.retryable is False


def test_a_malformed_argument_raises_rather_than_returning_garbage(
    seam: Seam, key_file: Path
) -> None:
    with pytest.raises(ErebusError) as caught:
        seam.open_channel(
            address="not-a-felt",
            key_file=key_file,
            counterparty_address="0xb0b",
            counterparty_public_key="0x9bcdef",
            token="0x7042",
        )

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


def test_the_key_is_never_passed_through_python(seam: Seam, key_file: Path) -> None:
    """The custody claim, asserted rather than asserted-in-prose.

    The request Python sends must contain the key file's *path* and never its contents.
    """
    sent: dict[str, object] = {}
    original = subprocess.run

    def capture(*args: object, **kwargs: object):  # type: ignore[no-untyped-def]
        sent.update(json.loads(kwargs["input"]))  # type: ignore[arg-type]
        return original(*args, **kwargs)  # type: ignore[arg-type]

    subprocess.run = capture  # type: ignore[assignment]
    try:
        seam.open_channel(
            address="0xa11ce",
            key_file=key_file,
            counterparty_address="0xb0b",
            counterparty_public_key="0x9bcdef",
            token="0x7042",
        )
    finally:
        subprocess.run = original  # type: ignore[assignment]

    rendered = json.dumps(sent)
    assert str(key_file) in rendered, "the path should be sent"
    assert "1234567890abcdef" not in rendered, "key material reached the request body"
