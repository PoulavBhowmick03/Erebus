"""A new release must preserve older pinned installs and reject corrupt assets."""

import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("metropolis_release_index", ROOT / "packaging/metropolis/build_index.py")
index = importlib.util.module_from_spec(spec)
spec.loader.exec_module(index)


def release(root, version):
    directory = root / f"v{version}"
    directory.mkdir(parents=True)
    manifest = {"tag": directory.name, "platforms": {}}
    for platform in ("linux_x86_64", "macosx_11_0_arm64"):
        data = {"platform_qualification": {"wheel_tag": platform}, "wheels": {}}
        for project in sorted(index.PROJECTS):
            tag = platform if project == "erebus-cli" else "any"
            name = f"{project.replace('-', '_')}-{version}-py3-none-{tag}.whl"
            content = name.encode()
            (directory / name).write_bytes(content)
            data["wheels"][name] = {"sha256": hashlib.sha256(content).hexdigest()}
        manifest["platforms"][platform] = data
    (directory / "release-manifest.json").write_text(json.dumps(manifest))
    return directory, manifest


def test_two_releases_keep_every_platform_and_version_hash_pinned(tmp_path):
    releases = tmp_path / "releases"
    release(releases, "0.3.0.dev4")
    release(releases, "0.3.0.dev5")
    output = tmp_path / "simple"
    index.build_index(releases, output, "v0.3.0.dev5")
    for project in index.PROJECTS:
        page = (output / project / "index.html").read_text()
        expected = 4 if project == "erebus-cli" else 2
        assert page.count("#sha256=") == expected
        for version in ("0.3.0.dev4", "0.3.0.dev5"):
            assert f"{index.RELEASE_URL}/v{version}/" in page
        assert "github.io/Erebus" not in page


@pytest.mark.parametrize("mutation,message", [
    ("tag", "different tag"), ("platforms", "two platform"),
    ("duplicate_platform", "supported pair"), ("missing_project", "all three"),
    ("missing_asset", "missing regular"), ("tamper", "hash does not match"),
    ("bad_hash", "invalid manifest hash"), ("path", "invalid wheel filename"),
    ("wrong_version", "version or filename"), ("unexpected", "unexpected project"),
    ("symlink", "missing regular"), ("platform_tag", "wheel tag"),
])
def test_bad_release_never_creates_an_index(tmp_path, mutation, message):
    root = tmp_path / "releases"
    directory, manifest = release(root, "0.3.0.dev4")
    data = manifest["platforms"]["linux_x86_64"]
    name = next(name for name in data["wheels"] if name.startswith("erebus_cli-"))
    if mutation == "tag":
        manifest["tag"] = "v0.3.0.dev5"
    elif mutation == "platforms":
        manifest["platforms"].pop("macosx_11_0_arm64")
    elif mutation == "duplicate_platform":
        data["platform_qualification"]["wheel_tag"] = "macosx_11_0_arm64"
    elif mutation == "missing_project":
        data["wheels"].pop(next(key for key in data["wheels"] if key.startswith("erebus_sdk-")))
    elif mutation == "missing_asset":
        (directory / name).unlink()
    elif mutation == "tamper":
        (directory / name).write_bytes(b"changed")
    elif mutation == "bad_hash":
        data["wheels"][name]["sha256"] = "not-a-hash"
    elif mutation in {"path", "wrong_version", "unexpected", "platform_tag"}:
        changed = {"path": "../" + name, "wrong_version": name.replace("dev4", "dev5"),
                   "unexpected": name.replace("erebus_cli", "unknown"),
                   "platform_tag": name.replace("linux_x86_64", "any")}[mutation]
        data["wheels"][changed] = data["wheels"].pop(name)
    elif mutation == "symlink":
        target = tmp_path / "outside.whl"
        (directory / name).rename(target)
        (directory / name).symlink_to(target)
    (directory / "release-manifest.json").write_text(json.dumps(manifest))
    output = tmp_path / "simple"
    with pytest.raises(ValueError, match=message):
        index.build_index(root, output, "v0.3.0.dev4")
    assert not output.exists()


def test_conflicting_shared_wheel_is_rejected(tmp_path):
    root = tmp_path / "releases"
    directory, manifest = release(root, "0.3.0.dev4")
    data = manifest["platforms"]["macosx_11_0_arm64"]
    name = next(name for name in data["wheels"] if name.startswith("erebus_sdk-"))
    data["wheels"][name]["sha256"] = "0" * 64
    (directory / "release-manifest.json").write_text(json.dumps(manifest))
    with pytest.raises(ValueError, match="hash does not match"):
        index.build_index(root, tmp_path / "simple", "v0.3.0.dev4")
    assert not (tmp_path / "simple").exists()


@pytest.mark.parametrize("tag", ["v0.3.0.dev5", "../escape", "v0.3.0", "v0.3.0.dev4\n"])
def test_dispatched_tag_must_be_a_downloaded_prerelease(tmp_path, tag):
    root = tmp_path / "releases"
    release(root, "0.3.0.dev4")
    with pytest.raises(ValueError):
        index.build_index(root, tmp_path / "simple", tag)
    assert not (tmp_path / "simple").exists()


def test_existing_index_is_never_overwritten(tmp_path):
    root = tmp_path / "releases"
    release(root, "0.3.0.dev4")
    output = tmp_path / "simple"
    output.mkdir()
    previous = output / "index.html"
    previous.write_text("previous published index")
    with pytest.raises(FileExistsError):
        index.build_index(root, output, "v0.3.0.dev4")
    assert previous.read_text() == "previous published index"
