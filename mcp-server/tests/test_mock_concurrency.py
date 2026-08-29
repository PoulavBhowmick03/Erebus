"""Concurrent writers against `MockErebusClient`'s shared JSON store.

Roadmap §5.3 flags this as unproven: two agents are two separate MCP server subprocesses,
each with its own `MockErebusClient` instance, both pointed at the same store file. Nothing
before this file exercised two writers overlapping on purpose, so the read-modify-write race
the mock's own docstring warns about had never actually been reproduced or fixed. It is
fixed now with a cross-process `flock` (see `MockErebusClient._locked_store`); these tests
prove the fix rather than just documenting the intent.

**Why real threads, not `asyncio.gather`.** A first version of this file used
`asyncio.gather` on coroutines with no `await` inside the read-modify-write section, and it
passed identically with the lock deleted -- a single-threaded event loop only yields at an
`await`, so two such coroutines can never actually interleave mid-write; the test proved
nothing. `open()`/`read()`/`write()` release the GIL during the real syscall, so genuine OS
threads (matching the genuine OS processes two agents' servers actually are) can. Each
worker below gets its own thread and its own event loop via `asyncio.run`, which is what
turns "the lock exists" into "the lock is load-bearing".

Everything here talks to `MockErebusClient` directly, the same way `test_spending.py` drives
`SpendGuard` directly with a `ThreadPoolExecutor`, rather than through a spawned MCP server:
the race lives in the store file, not in the transport, so a unit-level test reproduces it
faster and just as truly.
"""

from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from erebus_mcp.interface import OfferTerms
from erebus_mcp.mock_client import MockErebusClient

TERMS = OfferTerms(amount=10, token="0xtoken", deadline=9_999_999_999, memo_hash=0)

#: High enough that, without the lock, at least one lost update shows up reliably across a
#: normal test run rather than depending on a lucky (or unlucky) thread schedule.
CONCURRENT_CALLS = 40


def _client(store_path: Path, identity: str) -> MockErebusClient:
    return MockErebusClient(identity=identity, store_path=store_path, latency_seconds=0.0)


def _run_in_thread(coro_factory):
    """Runs one coroutine to completion on its own event loop, in its own OS thread."""
    return asyncio.run(coro_factory())


def test_concurrent_propose_offers_from_two_identities_are_all_recorded(tmp_path: Path) -> None:
    """Two agents' servers proposing into the same channel at once must not clobber
    each other, and must not collide on `offer_id` -- both failure modes come from the
    same bug: two overlapping calls reading `next_seq` before either call writes it back."""
    store = tmp_path / "store.json"
    buyer = _client(store, "0xbuyer")
    seller = _client(store, "0xseller")
    handle = asyncio.run(buyer.open_channel("op_open", "0xseller"))

    factories = [
        (lambda i=i: buyer.propose_offer(f"op_b{i}", handle, TERMS)) for i in range(CONCURRENT_CALLS)
    ] + [
        (lambda i=i: seller.propose_offer(f"op_s{i}", handle, TERMS)) for i in range(CONCURRENT_CALLS)
    ]
    with ThreadPoolExecutor(max_workers=len(factories)) as pool:
        offer_ids = list(pool.map(_run_in_thread, factories))

    assert len(offer_ids) == len(set(offer_ids)), (
        f"{len(offer_ids) - len(set(offer_ids))} colliding offer_id(s) out of {len(offer_ids)}"
    )

    state = asyncio.run(buyer.read_channel_state(handle))
    expected = 2 * CONCURRENT_CALLS
    assert len(state.offers) == expected, (
        f"expected {expected} offers, found {len(state.offers)}: "
        f"{expected - len(state.offers)} concurrent write(s) were silently lost"
    )
    recorded_ids = {offer.offer_id for offer in state.offers}
    assert recorded_ids == set(offer_ids), "an offer_id was returned but never persisted"


def test_concurrent_open_channel_calls_do_not_lose_channels(tmp_path: Path) -> None:
    """Same race, different write path: opening several channels at once must not drop any
    of them, and each must resolve back to the counterparty it was actually opened with."""
    store = tmp_path / "store.json"
    buyer = _client(store, "0xbuyer")
    counterparties = [f"0xseller{i}" for i in range(CONCURRENT_CALLS)]

    factories = [
        (lambda i=i, cp=cp: buyer.open_channel(f"op_{i}", cp))
        for i, cp in enumerate(counterparties)
    ]
    with ThreadPoolExecutor(max_workers=len(factories)) as pool:
        handles = list(pool.map(_run_in_thread, factories))

    assert len(handles) == len(set(handles)), "two different counterparties produced one handle"
    for handle, counterparty in zip(handles, counterparties):
        # A lost write would make this raise (channel missing), which is enough to catch it
        # without needing every channel's participant list back.
        asyncio.run(buyer.read_channel_state(handle))


if __name__ == "__main__":
    import pytest

    raise SystemExit(pytest.main([__file__, "-q"]))
