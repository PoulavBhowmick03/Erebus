"""Tag the native bundle for this build host, not as platform-independent Python."""

from __future__ import annotations

import json
import platform
from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


# The builder pins MACOSX_DEPLOYMENT_TARGET to this, so the tag matches the binaries' minos.
MACOS_DEPLOYMENT_TARGET = "11.0"


def native_tag() -> str:
    """Tag what the bundled binaries are, not what this Python was built for.

    A universal2 Python reports `macosx-10.9-universal2`, but the bundle is thin arm64 with a
    macOS 11.0 minimum; that tag would let pip install it where it cannot run.
    """
    system, machine = platform.system(), platform.machine()
    if system == "Darwin" and machine == "arm64":
        return "macosx_" + MACOS_DEPLOYMENT_TARGET.replace(".", "_") + "_arm64"
    if system == "Linux" and machine == "x86_64":
        return "linux_x86_64"
    raise RuntimeError("Metropolis bundles are built on macOS arm64 or Linux x86_64 only")


class CustomBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        if version == "editable":
            raise RuntimeError("Metropolis bundles require a built platform wheel")
        root = Path(self.root)
        inventory = json.loads((root / "binaries.json").read_text())
        for name in inventory:
            binary = root / "bin" / name
            if not binary.is_file() or binary.is_symlink():
                raise RuntimeError("native bundle is incomplete")
        tag = native_tag()
        build_data["pure_python"] = False
        build_data["infer_tag"] = False
        build_data["tag"] = f"py3-none-{tag}"
