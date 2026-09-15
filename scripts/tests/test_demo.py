from __future__ import annotations

import importlib.util
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("check_demo", ROOT / "scripts/check-demo.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_repository_demo_contract() -> None:
    assert MODULE.check(ROOT) == []


def test_stale_privacy_copy_is_rejected(tmp_path: Path) -> None:
    demo = tmp_path / "demo"
    demo.mkdir()
    for name in ("index.html", "styles.css", "app.js", "erebus-private-sprint.mp4"):
        (demo / name).write_bytes((ROOT / "demo" / name).read_bytes())
    (tmp_path / "strk20.json").write_bytes((ROOT / "strk20.json").read_bytes())
    path = demo / "index.html"
    path.write_text(path.read_text().replace("Readable for one deal", "Readable for one channel"))

    assert any("stale copy" in error for error in MODULE.check(tmp_path))


def test_pre_canary_video_copy_is_rejected(tmp_path: Path) -> None:
    demo = tmp_path / "demo"
    demo.mkdir()
    for name in ("index.html", "styles.css", "app.js", "erebus-private-sprint.mp4"):
        (demo / name).write_bytes((ROOT / "demo" / name).read_bytes())
    (tmp_path / "strk20.json").write_bytes((ROOT / "strk20.json").read_bytes())
    path = demo / "index.html"
    path.write_text(
        path.read_text().replace(
            "Public three-minute walkthrough of the complete mainnet workflow. It links the two screened canaries, recovery, observer limits, and scoped disclosure.",
            "Recorded before the later full mainnet canary; it shows setup and the complete Sepolia workflow.",
        )
    )

    assert any("stale copy" in error for error in MODULE.check(tmp_path))
