"""Locates the packaged ``erebus-cli`` binary.

This package exists to ship the binary, not to wrap it. The Rust client owns every
derivation; anything computed here would be a second implementation of something that has
to agree byte for byte (CLAUDE.md).

The binary installs to the environment's script directory, so ``shutil.which`` finds it and
callers normally need nothing from this module. :func:`binary_path` is for the case where
the environment's scripts are not on ``PATH``, which is common when a launcher runs the
server as a subprocess.
"""

from __future__ import annotations

import shutil
import sysconfig
from pathlib import Path

__all__ = ["__version__", "binary_path"]

__version__ = "0.0.1"


def binary_path() -> Path | None:
    """Returns the packaged binary's path, or ``None`` when it cannot be found.

    Checks ``PATH`` first so a locally built binary wins over a packaged one, which is what
    a developer working in ``sdk/rs`` expects.
    """
    found = shutil.which("erebus-cli")
    if found:
        return Path(found)

    for key in ("scripts", "purelib"):
        base = sysconfig.get_path(key)
        if not base:
            continue
        candidate = Path(base).parent / "bin" / "erebus-cli" if key == "purelib" else Path(base) / "erebus-cli"
        if candidate.is_file():
            return candidate
    return None
