"""Python binding for Erebus.

Mirrors the frozen ``ErebusClient`` interface (ARCHITECTURE.md §4, typed in
``sdk/ts/src/interface.ts``). Types only for now.

This is a **binding, not a client.** It marshals arguments down to ``sdk/rs`` and marshals
results back. It does not hash, encode salts, convert felts, or decide anything. Every
failure mode in this protocol is silent — a wrong preimage derives a storage slot nobody
wrote to, and the note is simply "not found" with no error anywhere — so a second place
where protocol logic can live is a second place a wrong answer can hide.

The tripwire, because "no protocol logic" is a principle and principles drift:

    This package should never need a known-answer test.

If a test here has to assert a *computed value*, this package is computing something and has
become a third implementation. Its tests assert that a call got through and came back,
nothing more.

What this is waiting on
-----------------------
Not the protocol. P0.2 resolved to the salt lane — the negotiation payload rides in note
salts (ARCHITECTURE.md §7) — and the Sepolia proving endpoint exists and answers.

The open item is **P0.4, the seam mechanism**: subprocess (``erebus-cli``, JSON over stdio)
or PyO3/maturin. Costs are in ARCHITECTURE.md §3. The async decision narrowed it — PyO3
across an async boundary needs a runtime owned on one side, whereas a subprocess keeps the
runtime inside Rust and hands Python plain JSON — but it is not settled.

That undecided seam is the argument for this package existing at all. The boundary sits
here, so the mechanism underneath can change without the MCP server or the agents noticing.
"""

__all__ = ["__version__"]

__version__ = "0.0.0"
