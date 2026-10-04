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
             f"requires-python = {q(project['requires-python'])}", f"dependencies = {q(dependencies)}"]
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
            if native is not None:
                (destination / "bin").mkdir()
                for name, path in native.items():
                    shutil.copy2(path, destination / "bin" / name)
                shutil.copy2(ROOT / "packaging/metropolis/hatch_build.py", destination / "hatch_build.py")
                (destination / "binaries.json").write_text(json.dumps(sorted(native)))
            environment = {key: value for key, value in os.environ.items() if key not in {"EREBUS_WHEEL_PLATFORM", "PYTHONPATH", "VIRTUAL_ENV"}}
            subprocess.run([uv, "build", "--wheel", "--no-sources", "--out-dir", str(wheels), str(destination)],
                           cwd=stage, env=environment, check=True)
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
