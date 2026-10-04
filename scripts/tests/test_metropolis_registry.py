"""Metropolis packaging must not alter stable metadata or invent a portable binary tag."""

import importlib.util
import json
import tomllib
import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("metropolis_registry", ROOT / "scripts/build-metropolis-registry.py")
registry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(registry)


@pytest.mark.parametrize("relative", registry.PACKAGE_PATHS)
def test_metadata_is_staged_and_only_workspace_dependencies_change(relative):
    source = ROOT / relative
    before = (source / "pyproject.toml").read_bytes()
    data = tomllib.loads(registry.package_metadata(source, "0.3.0.dev20261003", {"erebus-payment": Path("unused")} if relative == "packaging/erebus-cli" else None))
    assert data["project"]["version"] == "0.3.0.dev20261003"
    assert "uv" not in data.get("tool", {})
    dependencies = data["project"]["dependencies"]
    if relative == "sdk/py":
        assert dependencies == ["erebus-cli==0.3.0.dev20261003"]
    if relative == "mcp-server":
        assert dependencies == ["mcp[cli]>=2.0.0,<3.0.0", "erebus-sdk==0.3.0.dev20261003"]
    assert (source / "pyproject.toml").read_bytes() == before


@pytest.mark.parametrize("version", ["0.3.0", "1.0.0rc1", "../../escape", "0.3.0.dev1\n", "v0.3.0.dev1"])
def test_stable_or_invalid_versions_fail_before_building(version, tmp_path):
    with pytest.raises(ValueError, match="prerelease"):
        registry.build(version, tmp_path / "registry", "debug", False)
    assert not (tmp_path / "registry").exists()


def test_index_links_stay_in_the_separate_registry_and_pin_hashes(tmp_path):
    wheels = tmp_path / "wheels"
    wheels.mkdir()
    for name in registry.PROJECT_NAMES:
        (wheels / f"{name.replace('-', '_')}-0.3.0.dev1-py3-none-any.whl").write_bytes(name.encode())
    projects = registry.build_index(wheels, tmp_path / "simple")
    assert set(projects) == registry.PROJECT_NAMES
    for name, files in projects.items():
        page = (tmp_path / "simple" / name / "index.html").read_text()
        assert f"../../wheels/{files[0].name}#sha256={registry.sha256(files[0])}" in page
        assert "github.io/Erebus/simple" not in page


def test_registry_rejects_missing_and_unexpected_projects(tmp_path):
    wheels = tmp_path / "wheels"
    wheels.mkdir()
    with pytest.raises(ValueError, match="all three"):
        registry.build_index(wheels, tmp_path / "simple")
    (wheels / "unexpected-0.1-py3-none-any.whl").write_bytes(b"invalid")
    with pytest.raises(ValueError, match="unexpected"):
        registry.build_index(wheels, tmp_path / "simple")


def test_native_bundle_rejects_scripts_and_symlinks(tmp_path):
    path = tmp_path / "fake"
    path.write_bytes(b"#!/bin/sh\nexit 0\n")
    path.chmod(0o755)
    with pytest.raises(ValueError, match="architecture"):
        registry.validate_binary(path)
    link = tmp_path / "link"
    link.symlink_to(path)
    with pytest.raises(ValueError, match="regular"):
        registry.validate_binary(link)


def test_binary_inventory_matches_current_cargo_targets():
    for crate, names in registry.BINARY_GROUPS.items():
        manifest = tomllib.loads((ROOT / crate / "Cargo.toml").read_text())
        explicit = {binary["name"] for binary in manifest.get("bin", [])}
        automatic = {path.stem for path in (ROOT / crate / "src/bin").glob("*.rs")}
        assert set(names) <= explicit | automatic


def test_release_asset_links_are_absolute_and_hash_pinned(tmp_path):
    wheels = tmp_path / "wheels"
    wheels.mkdir()
    for name in registry.PROJECT_NAMES:
        (wheels / f"{name.replace('-', '_')}-0.3.0.dev1-py3-none-any.whl").write_bytes(name.encode())
    projects = registry.build_index(
        wheels, tmp_path / "simple", "https://github.com/PoulavBhowmick03/erebus-metropolis/releases/download/v0.3.0.dev1"
    )
    for name, files in projects.items():
        page = (tmp_path / "simple" / name / "index.html").read_text()
        assert (
            f"https://github.com/PoulavBhowmick03/erebus-metropolis/releases/download/v0.3.0.dev1/"
            f"{files[0].name}#sha256={registry.sha256(files[0])}"
        ) in page
        assert "github.io/Erebus/simple" not in page


def wheel_metadata(path, tag, *, pure="false"):
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("erebus_cli-0.3.0.dev1.dist-info/WHEEL",
                         f"Wheel-Version: 1.0\nRoot-Is-Purelib: {pure}\nTag: {tag}\n")


@pytest.mark.parametrize("tag", ["macosx_11_0_arm64", "linux_x86_64"])
def test_qualification_reads_the_actual_native_wheel_tag(tmp_path, tag):
    wheel_metadata(tmp_path / f"erebus_cli-0.3.0.dev1-py3-none-{tag}.whl", f"py3-none-{tag}")
    assert registry.native_wheel_tag(tmp_path) == tag


@pytest.mark.parametrize("filename,tag,pure", [
    ("macosx_15_3_arm64", "py3-none-macosx_11_0_arm64", "false"),
    ("linux_x86_64", "py3-none-linux_x86_64", "true"),
    ("any", "py3-none-any", "false"),
])
def test_native_tag_rejects_mislabeled_or_pure_wheels(tmp_path, filename, tag, pure):
    wheel_metadata(tmp_path / f"erebus_cli-0.3.0.dev1-py3-none-{filename}.whl", tag, pure=pure)
    with pytest.raises(ValueError):
        registry.native_wheel_tag(tmp_path)


def test_native_tag_requires_one_native_wheel(tmp_path):
    with pytest.raises(ValueError, match="one native"):
        registry.native_wheel_tag(tmp_path)


def load_hatch_hook(monkeypatch):
    import sys
    import types

    interface = types.ModuleType("hatchling.builders.hooks.plugin.interface")
    interface.BuildHookInterface = object
    for name in ("hatchling", "hatchling.builders", "hatchling.builders.hooks", "hatchling.builders.hooks.plugin"):
        monkeypatch.setitem(sys.modules, name, types.ModuleType(name))
    monkeypatch.setitem(sys.modules, interface.__name__, interface)
    spec = importlib.util.spec_from_file_location("metropolis_hatch_build", ROOT / "packaging/metropolis/hatch_build.py")
    hook = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(hook)
    return hook


@pytest.mark.parametrize("system,machine,tag", [("Darwin", "arm64", "macosx_11_0_arm64"), ("Linux", "x86_64", "linux_x86_64")])
def test_bundle_tag_names_the_binaries_not_a_universal2_python(monkeypatch, system, machine, tag):
    import sysconfig

    hook = load_hatch_hook(monkeypatch)
    # GitHub's macOS Python reports universal2; the tag must still name thin arm64 binaries.
    monkeypatch.setattr(sysconfig, "get_platform", lambda: "macosx-10.9-universal2")
    monkeypatch.setattr(hook.platform, "system", lambda: system)
    monkeypatch.setattr(hook.platform, "machine", lambda: machine)
    assert hook.native_tag() == tag
    assert hook.MACOS_DEPLOYMENT_TARGET == "11.0"


@pytest.mark.parametrize("system,machine", [("Darwin", "x86_64"), ("Linux", "aarch64"), ("Windows", "AMD64")])
def test_bundle_tag_refuses_unqualified_hosts(monkeypatch, system, machine):
    hook = load_hatch_hook(monkeypatch)
    monkeypatch.setattr(hook.platform, "system", lambda: system)
    monkeypatch.setattr(hook.platform, "machine", lambda: machine)
    with pytest.raises(RuntimeError, match="macOS arm64 or Linux x86_64"):
        hook.native_tag()
