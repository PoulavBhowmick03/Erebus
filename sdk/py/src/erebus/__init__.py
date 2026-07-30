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

The seam
--------
**Subprocess, decided 2026-07-30** (ARCHITECTURE.md §3). ``erebus-cli`` takes one JSON
request on stdin and answers with one JSON envelope on stdout. :class:`erebus.Seam` is the
whole of it.

The deciding argument was custody. Requests carry a **path** to a key file, never a key, so
this process — the one that also runs agent frameworks and model-driven code — never holds a
pool private key at all. In-process via PyO3, CLAUDE.md constraint 6 would have degraded
from "structurally unreachable" to "same heap as whatever else got imported".

Entropy is generated inside the Rust binary for the same reason: if this package supplied a
salt, it would be making a cryptographic decision, and a second place that can produce a
weak salt is a second place a silent failure hides.
"""

from erebus._seam import ErebusError, Seam, SeamUnavailable

__all__ = ["ErebusError", "Seam", "SeamUnavailable", "__version__"]

__version__ = "0.0.0"
