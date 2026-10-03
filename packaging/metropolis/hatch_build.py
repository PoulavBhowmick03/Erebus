"""Tag the native bundle for this build host, not as platform-independent Python."""

from __future__ import annotations

import json
import sysconfig
from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


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
        tag = sysconfig.get_platform().replace("-", "_").replace(".", "_")
        build_data["pure_python"] = False
        build_data["infer_tag"] = False
        build_data["tag"] = f"py3-none-{tag}"
