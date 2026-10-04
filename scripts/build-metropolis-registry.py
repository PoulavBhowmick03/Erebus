#!/usr/bin/env python3
"""Build an unpublished Metropolis prerelease index without editing stable packages."""

from __future__ import annotations

import argparse
from email.parser import Parser
import hashlib
import html
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sysconfig
import tempfile
import tomllib
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BINARY_GROUPS = {
    "sdk/rs": ["erebus-cli"],
    "sdk/evm": ["erebus-settle", "erebus-tx-relayer", "erebus-disclosure", "erebus-network-check"],
    "sdk/transport": ["erebus-relay"],
    "sdk/shielded": [
        "erebus-negotiate", "erebus-payment", "erebus-local-prove", "erebus-artifacts",
        "erebus-access", "erebus-access-service", "erebus-shielded-disclosure",
        "erebus_pool_indexer",
    ],
}
PACKAGE_PATHS = ["packaging/erebus-cli", "sdk/py", "mcp-server"]
PROJECT_NAMES = {"erebus-cli", "erebus-sdk", "erebus-mcp-server"}
DEV_VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\.dev[0-9]+", re.ASCII)


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def validate_binary(path: Path) -> None:
    """Reject scripts and wrong host architectures; this is not a portability audit."""
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or path.is_symlink() or not info.st_mode & 0o111:
        raise ValueError("native bundle requires executable regular files")
    with path.open("rb") as stream:
        header = stream.read(64)
    if platform.system() == "Darwin" and platform.machine() == "arm64":
        valid = header[:4] == b"\xcf\xfa\xed\xfe" and int.from_bytes(header[4:8], "little") == 0x0100000C
    elif platform.system() == "Linux" and platform.machine() == "x86_64":
        valid = header[:6] == b"\x7fELF\x02\x01" and int.from_bytes(header[18:20], "little") == 62
    else:
        raise ValueError("supported build hosts are macOS arm64 and Linux x86_64")
    if len(header) != 64 or not valid:
        raise ValueError("native file does not match the build host architecture")


def native_wheel_tag(wheels: Path) -> str:
    """Read the isolated build's tag; its Python need not match this process's Python."""
    native = list(wheels.glob("erebus_cli-*.whl"))
    if len(native) != 1:
        raise ValueError("expected one native wheel for this platform")
    with zipfile.ZipFile(native[0]) as archive:
        records = [name for name in archive.namelist() if name.endswith(".dist-info/WHEEL")]
        if len(records) != 1:
            raise ValueError("native wheel requires one WHEEL metadata record")
        metadata = Parser().parsestr(archive.read(records[0]).decode("utf-8"))
    tags = metadata.get_all("Tag", [])
    if len(tags) != 1 or not tags[0].startswith("py3-none-"):
        raise ValueError("native wheel requires one py3-none platform tag")
    tag = tags[0].removeprefix("py3-none-")
    if not native[0].name.endswith(f"-py3-none-{tag}.whl") or metadata.get("Root-Is-Purelib") != "false":
        raise ValueError("native wheel filename and metadata disagree")
    if tag not in {"linux_x86_64", "macosx_11_0_arm64"}:
        raise ValueError("native wheel tag must name a supported build host")
    return tag


def package_metadata(source: Path, version: str, binaries: dict[str, Path] | None = None) -> str:
    """Project only the known package fields into a separate staging directory."""
    parsed = tomllib.loads((source / "pyproject.toml").read_text())
    project = parsed["project"]
    if project["name"] not in PROJECT_NAMES or not DEV_VERSION.fullmatch(version):
        raise ValueError("Metropolis requires a dev prerelease version and known project")
    dependencies = []
    for dependency in project.get("dependencies", []):
        workspace = dependency.split("==", 1)[0]
        dependencies.append(f"{workspace}=={version}" if workspace in PROJECT_NAMES else dependency)
    q = json.dumps
    lines = ["[project]", f"name = {q(project['name'])}", f"version = {q(version)}",
             f"description = {q('Experimental Metropolis testnet package: ' + project['description'])}",
             f"requires-python = {q(project['requires-python'])}", f"dependencies = {q(dependencies)}",
             'license = "Apache-2.0"',
             f"license-files = {q(['LICENSE', THIRD_PARTY_NOTICES] if binaries is not None else ['LICENSE'])}"]
    if project.get("scripts"):
        lines.extend(["", "[project.scripts]"])
        lines.extend(f"{q(name)} = {q(target)}" for name, target in project["scripts"].items())
    build = parsed["build-system"]
    packages = parsed["tool"]["hatch"]["build"]["targets"]["wheel"]["packages"]
    lines.extend(["", "[build-system]", f"requires = {q(build['requires'])}",
                  f"build-backend = {q(build['build-backend'])}", "", "[tool.hatch.build.targets.wheel]",
                  f"packages = {q(packages)}"])
    if binaries is not None:
        lines.extend(["", "[tool.hatch.build.targets.wheel.shared-scripts]"])
        lines.extend(f"{q('bin/' + name)} = {q(name)}" for name in sorted(binaries))
        lines.extend(["", "[tool.hatch.build.targets.wheel.hooks.custom]", 'path = "hatch_build.py"'])
    return "\n".join(lines) + "\n"


THIRD_PARTY_NOTICES = "THIRD_PARTY_NOTICES"
LICENSE_TEXT = re.compile(r"(licen[cs]e|copying|notice)", re.IGNORECASE)


def third_party_notices() -> tuple[str, dict]:
    """License expressions and texts for every crate linked into the bundled binaries.

    Only normal dependencies reachable from the four bundled crates are included; dev and build
    dependencies do not ship. Workspace crates are covered by the project LICENSE.
    """
    packages: dict[str, dict] = {}
    for crate in BINARY_GROUPS:
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1", "--manifest-path", str(ROOT / crate / "Cargo.toml")],
            cwd=ROOT, text=True))
        by_id = {package["id"]: package for package in metadata["packages"]}
        nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
        pending = [metadata["resolve"]["root"]]
        seen = set()
        while pending:
            current = pending.pop()
            if current in seen:
                continue
            seen.add(current)
            for dependency in nodes[current]["deps"]:
                if any(kind["kind"] is None for kind in dependency["dep_kinds"]):
                    pending.append(dependency["pkg"])
        for package_id in seen:
            package = by_id[package_id]
            if package["source"] is None:
                continue
            packages[f"{package['name']} {package['version']}"] = package
    sections, missing = [], []
    for key in sorted(packages):
        package = packages[key]
        directory = Path(package["manifest_path"]).parent
        texts = sorted(path for path in directory.iterdir() if path.is_file() and LICENSE_TEXT.match(path.name))
        body = "\n\n".join(f"--- {path.name} ---\n{path.read_text(errors='replace').strip()}" for path in texts)
        if not texts:
            # The published crate carries no license file; its terms are the canonical texts in
            # the appendix, with this crate's own copyright holders.
            missing.append(key)
            holders = ", ".join(package.get("authors") or []) or f"the {package['name']} authors"
            body = (f"No license file is published inside this crate. Copyright (c) {holders}"
                    f" ({package.get('repository') or 'repository not declared'}). Licensed under"
                    f" {package.get('license') or 'an undeclared license'}; the canonical texts are in the appendix.")
        sections.append(f"=== {key} ({package.get('license') or 'no SPDX expression'}) ===\n{body}")
    header = ("Third-party software linked into the Erebus Metropolis native binaries.\n"
              "Generated from `cargo metadata --locked` for sdk/rs, sdk/evm, sdk/transport, and sdk/shielded.\n\n")
    appendix = "\n\n".join(f"=== Appendix: {name} ===\n{text.strip()}" for name, text in canonical_texts().items())
    return (header + "\n\n".join(sections) + "\n\n" + appendix + "\n",
            {"crates": len(packages), "crates_relying_on_appendix_texts": missing})


def canonical_texts() -> dict[str, str]:
    """Standard license texts that crates without their own license file refer to."""
    return {
        "Apache-2.0": (ROOT / "LICENSE").read_text(),
        "MIT": """Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the
following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO
EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
USE OR OTHER DEALINGS IN THE SOFTWARE.""",
        "BSD-3-Clause": """Redistribution and use in source and binary forms, with or without modification, are permitted
provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the
   following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and
   the following disclaimer in the documentation and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or
   promote products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED
WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.""",
        "0BSD": """Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is
hereby granted.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE
INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE,
DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.""",
        "LLVM-exception": """As an exception, if, as a result of your compiling your source code, portions of this Software are
embedded into an Object form of such source code, you may redistribute such embedded portions in such
Object form without complying with the conditions of Sections 4(a), 4(b) and 4(d) of the License.

In addition, if you combine or link compiled forms of this Software with software that is licensed under
the GPLv2 ("Combined Software") and if a court of competent jurisdiction determines that the patent
provision (Section 3), the indemnity provision (Section 9) or other Section of the License conflicts with
the conditions of the GPLv2, you may retroactively and prospectively choose to deem waived or otherwise
exclude such Section(s) of the License, but only in their entirety and only with respect to the Combined
Software.""",
        "CC0-1.0": "CC0 1.0 Universal public-domain dedication: https://creativecommons.org/publicdomain/zero/1.0/legalcode",
    }


def require_licenses(wheels: Path) -> None:
    for wheel in wheels.glob("*.whl"):
        with zipfile.ZipFile(wheel) as archive:
            names = {name.rsplit("/", 1)[-1] for name in archive.namelist() if ".dist-info/licenses/" in name}
        required = {"LICENSE", THIRD_PARTY_NOTICES} if wheel.name.startswith("erebus_cli-") else {"LICENSE"}
        if not required <= names:
            raise ValueError(f"{wheel.name} is missing license files: {sorted(required - names)}")


def wheel_link(wheel: Path, base_url: str | None) -> str:
    name = html.escape(wheel.name, quote=True)
    digest = sha256(wheel)
    if base_url:
        return f"{base_url.rstrip('/')}/{name}#sha256={digest}"
    return f"../../wheels/{name}#sha256={digest}"


def build_index(wheels: Path, destination: Path, base_url: str | None = None) -> dict[str, list[Path]]:
    projects: dict[str, list[Path]] = {}
    for wheel in sorted(wheels.glob("*.whl")):
        name = re.sub(r"[-_.]+", "-", wheel.name.split("-")[0]).lower()
        if name not in PROJECT_NAMES:
            raise ValueError("unexpected project in the Metropolis registry")
        projects.setdefault(name, []).append(wheel)
    if set(projects) != PROJECT_NAMES:
        raise ValueError("Metropolis registry requires all three packages")
    destination.mkdir(parents=True)
    (destination / "index.html").write_text("<!doctype html>\n" + "\n".join(
        f'<a href="{name}/">{name}</a><br>' for name in sorted(projects)
    ))
    for name, files in projects.items():
        page = destination / name
        page.mkdir()
        (page / "index.html").write_text("<!doctype html>\n" + "\n".join(
            f'<a href="{wheel_link(wheel, base_url)}">{html.escape(wheel.name)}</a><br>'
            for wheel in files
        ))
    return projects


def build(version: str, output: Path, profile: str, build_native: bool, base_url: str | None = None) -> dict:
    if not DEV_VERSION.fullmatch(version):
        raise ValueError("use a dev prerelease version, for example 0.3.0.dev20261003")
    if output.exists() or profile not in {"debug", "release"}:
        raise ValueError("use a new output directory and a debug or release profile")
    uv = shutil.which("uv")
    if not uv:
        raise ValueError("uv is required")
    binaries: dict[str, Path] = {}
    for crate, names in BINARY_GROUPS.items():
        if build_native:
            command = ["cargo", "build", "--locked", "--manifest-path", str(ROOT / crate / "Cargo.toml")]
            if profile == "release":
                command.append("--release")
            for name in names:
                command.extend(["--bin", name])
            environment = dict(os.environ)
            if platform.system() == "Darwin":
                # Must match MACOS_DEPLOYMENT_TARGET in packaging/metropolis/hatch_build.py.
                environment["MACOSX_DEPLOYMENT_TARGET"] = "11.0"
            subprocess.run(command, cwd=ROOT, env=environment, check=True)
        for name in names:
            binary = ROOT / crate / "target" / profile / name
            validate_binary(binary)
            binaries[name] = binary
    notices, notice_summary = third_party_notices()
    source_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="metropolis-package-") as staging:
        stage = Path(staging)
        registry = stage / "registry"
        wheels = registry / "wheels"
        wheels.mkdir(parents=True)
        for relative in PACKAGE_PATHS:
            source = ROOT / relative
            destination = stage / source.name
            destination.mkdir()
            shutil.copytree(source / "src", destination / "src", ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            native = dict(binaries) if relative == "packaging/erebus-cli" else None
            if native is not None:
                native["erebus-selfhost"] = ROOT / "scripts/metropolis-selfhost.sh"
            (destination / "pyproject.toml").write_text(package_metadata(source, version, native))
            shutil.copy2(ROOT / "LICENSE", destination / "LICENSE")
            if native is not None:
                (destination / THIRD_PARTY_NOTICES).write_text(notices)
            if native is not None:
                (destination / "bin").mkdir()
                for name, path in native.items():
                    shutil.copy2(path, destination / "bin" / name)
                shutil.copy2(ROOT / "packaging/metropolis/hatch_build.py", destination / "hatch_build.py")
                (destination / "binaries.json").write_text(json.dumps(sorted(native)))
            environment = {key: value for key, value in os.environ.items() if key not in {"EREBUS_WHEEL_PLATFORM", "PYTHONPATH", "VIRTUAL_ENV"}}
            subprocess.run([uv, "build", "--wheel", "--no-sources", "--out-dir", str(wheels), str(destination)],
                           cwd=stage, env=environment, check=True)
        require_licenses(wheels)
        build_index(wheels, registry / "simple", base_url)
        manifest = {
            "version": version, "channel": "metropolis-testnet", "published": False,
            "build_profile": profile, "source_commit": source_commit, "dirty_source": dirty,
            "platform": sysconfig.get_platform(), "portability_audit": False,
            "platform_qualification": {
                "host": sysconfig.get_platform(),
                "wheel_tag": native_wheel_tag(wheels),
                "supported_host": True,
                "portability_audit": False,
            },
            "binaries": {name: {"sha256": sha256(path), "bytes": path.stat().st_size} for name, path in sorted(binaries.items())},
            "launchers": {"erebus-selfhost": {"sha256": sha256(ROOT / "scripts/metropolis-selfhost.sh")}},
            "wheels": {path.name: {"sha256": sha256(path), "bytes": path.stat().st_size} for path in sorted(wheels.glob("*.whl"))},
            "test_only_artifacts_included": False,
            "licenses": {"project": "Apache-2.0", "third_party": notice_summary},
        }
        (registry / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
        shutil.copytree(registry, output)
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", choices=["debug", "release"], default="release")
    parser.add_argument("--build", action="store_true", help="build native files first instead of using existing files")
    parser.add_argument("--base-url", help="absolute base URL for wheel links, e.g. a release download URL")
    parser.add_argument("--index-only", action="store_true", help="rebuild the index from an existing wheel directory")
    parser.add_argument("--wheels", type=Path, help="wheel directory for --index-only")
    args = parser.parse_args()
    try:
        if args.index_only:
            if args.wheels is None:
                raise ValueError("--index-only requires --wheels")
            build_index(args.wheels, args.output.resolve(), args.base_url)
            print(json.dumps({"status": "indexed", "output": str(args.output.resolve())}))
            return
        manifest = build(args.version, args.output.resolve(), args.profile, args.build, args.base_url)
    except (ValueError, OSError, subprocess.CalledProcessError):
        raise SystemExit("Metropolis registry build failed; no publication was attempted") from None
    print(json.dumps({"status": "built", "channel": manifest["channel"], "version": args.version,
                      "output": str(args.output.resolve()), "published": False}))


if __name__ == "__main__":
    main()
