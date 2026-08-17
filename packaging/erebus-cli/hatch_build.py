"""Forces a platform-specific wheel tag.

The wheel carries a compiled binary, so it is valid on exactly one platform. Hatchling
infers a platform tag when it sees an extension module and cannot infer one from a plain
data file, so without this hook the wheel would be tagged `py3-none-any` and pip would
happily install a macOS binary on Linux.

`EREBUS_WHEEL_PLATFORM` overrides the detected tag so a cross-build can name its target.
Locally it is unset and the tag comes from the running interpreter.
"""

from __future__ import annotations

import os
import sysconfig
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


def _platform_tag() -> str:
    override = os.environ.get("EREBUS_WHEEL_PLATFORM")
    if override:
        return override
    # e.g. "macosx-15.0-arm64" -> "macosx_15_0_arm64"
    return sysconfig.get_platform().replace("-", "_").replace(".", "_")


class CustomBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        # An editable install is the development checkout, where the binary comes from
        # `cargo build` and is found on PATH. Requiring it here would break
        # `uv sync --all-packages` for anyone who has not run a release build.
        if version == "editable":
            return

        binary = os.path.join(self.root, "bin", "erebus-cli")
        if not os.path.exists(binary):
            raise RuntimeError(
                f"{binary} is missing. Build it first:\n"
                "  cargo build --release --bin erebus-cli --manifest-path sdk/rs/Cargo.toml\n"
                "then copy it to packaging/erebus-cli/bin/erebus-cli"
            )
        build_data["pure_python"] = False
        build_data["infer_tag"] = False
        build_data["tag"] = f"py3-none-{_platform_tag()}"
