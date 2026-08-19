"""Python binding for the frozen ``ErebusClient`` interface (ARCHITECTURE.md §4).

This package sends arguments to ``sdk/rs`` and returns its results. It does not hash, encode
salts, convert felts, or make protocol decisions. A wrong preimage silently derives an
unused storage slot, so duplicate protocol logic can hide a wrong answer.

This package must not need a known-answer test. Its tests cover transport and response shape.
A computed-value assertion means that Python has become another protocol implementation.

The seam
--------
The project chose a subprocess on 2026-07-30 (ARCHITECTURE.md §3). ``erebus-cli`` reads one
JSON request from stdin and writes one JSON envelope to stdout. :class:`erebus.Seam` wraps
this exchange.

Requests contain key-file paths, not keys. The Python process runs agent frameworks and
model-driven code but never holds a pool private key. An in-process PyO3 binding would put
the key in the same heap, contrary to CLAUDE.md constraint 6.

The Rust binary also generates entropy. Python does not produce keys or salts.
"""

from erebus._seam import PROTOCOL, ErebusError, Seam, SeamConfig, SeamUnavailable

__all__ = ["PROTOCOL", "ErebusError", "Seam", "SeamConfig", "SeamUnavailable", "__version__"]

__version__ = "0.0.0"
