"""Unit tests for IntentStore: durable caller intent (plan.md, Ishita task 1)."""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from erebus_mcp.intent import IntentConflict, IntentStore, new_operation_id

_ID_PATTERN = re.compile(r"^op_[0-9a-f]{64}$")


def test_operation_id_matches_the_plan_md_format() -> None:
    assert _ID_PATTERN.match(new_operation_id())


def test_operation_ids_are_unique() -> None:
    assert new_operation_id() != new_operation_id()


def test_begin_persists_a_mode_0600_file(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    operation_id = store.begin("open_channel", {"counterparty": "0xseller"})

    path = tmp_path / "pending_operations" / f"{operation_id}.json"
    assert path.exists()
    assert oct(path.stat().st_mode)[-3:] == "600"

    raw = json.loads(path.read_text())
    assert raw == {
        "operation_id": operation_id,
        "tool": "open_channel",
        "params": {"counterparty": "0xseller"},
    }


def test_begin_with_identical_params_reuses_the_same_id(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    first = store.begin("open_channel", {"counterparty": "0xseller"})
    second = store.begin("open_channel", {"counterparty": "0xseller"})
    assert first == second


def test_begin_preserves_a_valid_caller_supplied_id(tmp_path: Path) -> None:
    operation_id = "op_" + "ab" * 32
    store = IntentStore(tmp_path)

    assert (
        store.begin("open_channel", {"counterparty": "0xseller"}, operation_id)
        == operation_id
    )


def test_caller_id_cannot_be_rebound_or_used_as_a_path(tmp_path: Path) -> None:
    operation_id = "op_" + "ab" * 32
    store = IntentStore(tmp_path)
    store.begin("open_channel", {"counterparty": "0xseller"}, operation_id)

    with pytest.raises(IntentConflict):
        store.begin("open_channel", {"counterparty": "0xother"}, operation_id)
    with pytest.raises(IntentConflict):
        store.begin("open_channel", {"counterparty": "0xseller"}, "../../escape")


def test_begin_with_different_params_mints_a_new_id(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    first = store.begin("open_channel", {"counterparty": "0xseller"})
    second = store.begin("open_channel", {"counterparty": "0xother"})
    assert first != second


def test_begin_with_a_different_tool_mints_a_new_id_even_with_the_same_params(
    tmp_path: Path,
) -> None:
    store = IntentStore(tmp_path)
    first = store.begin("propose_offer", {"channel_handle": "h1"})
    second = store.begin("counter_offer", {"channel_handle": "h1"})
    assert first != second


def test_resolve_removes_the_record(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    operation_id = store.begin("open_channel", {"counterparty": "0xseller"})
    store.resolve(operation_id)

    path = tmp_path / "pending_operations" / f"{operation_id}.json"
    assert not path.exists()
    assert store.pending() == []


def test_resolving_an_unknown_id_is_not_an_error(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    store.resolve("op_" + "0" * 64)  # never persisted; must not raise


def test_resolve_twice_is_not_an_error(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    operation_id = store.begin("open_channel", {"counterparty": "0xseller"})
    store.resolve(operation_id)
    store.resolve(operation_id)  # already gone; must not raise


def test_pending_lists_unresolved_records(tmp_path: Path) -> None:
    store = IntentStore(tmp_path)
    resolved = store.begin("open_channel", {"counterparty": "0xa"})
    unresolved = store.begin("open_channel", {"counterparty": "0xb"})
    store.resolve(resolved)

    pending = store.pending()
    assert [r.operation_id for r in pending] == [unresolved]


def test_pending_is_empty_before_any_intent_is_begun(tmp_path: Path) -> None:
    assert IntentStore(tmp_path).pending() == []


def test_a_record_left_after_a_simulated_crash_is_reused_by_a_fresh_store(
    tmp_path: Path,
) -> None:
    """The crash-and-restart case this module exists for: a first process persists an
    intent and never resolves it (simulating a kill mid-call). A second `IntentStore`
    built over the same directory, standing in for the restarted process, must see the
    same pending record and reuse its id for the identical request rather than minting a
    new one."""
    first = IntentStore(tmp_path)
    started = first.begin("accept_and_settle", {"channel_handle": "h1", "offer_id": "o1"})
    # No resolve() call: the process "crashes" here.

    second = IntentStore(tmp_path)
    assert [r.operation_id for r in second.pending()] == [started]
    resumed = second.begin("accept_and_settle", {"channel_handle": "h1", "offer_id": "o1"})
    assert resumed == started
