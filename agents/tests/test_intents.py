from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from erebus_agents.intents import IntentConflict, IntentStore


def test_prepare_persists_one_stable_id_before_completion(tmp_path: Path) -> None:
    path = tmp_path / "caller" / "intents.json"
    store = IntentStore(path)

    first = store.prepare("round-0", "propose_offer", {"amount": "7", "token": "0x1"})
    second = IntentStore(path).prepare(
        "round-0", "propose_offer", {"token": "0x1", "amount": "7"}
    )

    assert first == second
    assert first["operation_id"].startswith("op_")
    assert len(first["operation_id"]) == 67
    assert first["state"] == "prepared"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600


def test_same_key_cannot_be_rebound_to_different_intent(tmp_path: Path) -> None:
    store = IntentStore(tmp_path / "intents.json")
    store.prepare("settle", "accept_and_settle", {"offer_id": "one"})

    with pytest.raises(IntentConflict):
        store.prepare("settle", "accept_and_settle", {"offer_id": "two"})


def test_completion_is_durable_and_does_not_expose_a_new_id(tmp_path: Path) -> None:
    path = tmp_path / "intents.json"
    store = IntentStore(path)
    prepared = store.prepare("open", "open_channel", {"counterparty": "0x2"})
    store.complete("open", {"channel_handle": "ch_1"})

    completed = IntentStore(path).prepare("open", "open_channel", {"counterparty": "0x2"})
    raw = json.loads(path.read_text())
    assert completed["operation_id"] == prepared["operation_id"]
    assert completed["state"] == "completed"
    assert completed["result"] == {"channel_handle": "ch_1"}
    assert raw["version"] == 1
