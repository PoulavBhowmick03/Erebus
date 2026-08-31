#!/usr/bin/env python3
"""Builds a PEP 503 "simple repository" index over a directory of wheels.

GitHub has no Python package registry -- GitHub Packages covers npm, RubyGems, Maven,
Gradle, NuGet, and Docker, and not PyPI. So wheels attached to a Release are files at URLs,
and `uv`/`pip` cannot resolve a dependency chain from files alone. This generates the static
index that makes them resolvable, to be served from GitHub Pages:

    uv tool install --index https://<owner>.github.io/<repo>/simple erebus-mcp-server

Use ``--index`` rather than ``--index-url`` so PyPI remains available for third-party
dependencies that are deliberately not mirrored here.

Usage:

    python3 scripts/build-index.py --dist dist --out site/simple --base-url https://...

`--base-url` is where the wheels actually live, normally the Release download URL. The index
only points at them; it does not host them, so a Pages deployment stays small no matter how
many releases accumulate.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import re
from pathlib import Path

# PEP 503: lowercase, and runs of -_. collapse to a single -.
_NORMALISE = re.compile(r"[-_.]+")


def normalise(name: str) -> str:
    return _NORMALISE.sub("-", name).lower()


def project_of(wheel: Path) -> str:
    """The distribution name from a wheel filename.

    Wheel names are `{name}-{version}(-{build})?-{python}-{abi}-{platform}.whl`, and the
    name is everything before the first hyphen that starts the version.
    """
    return normalise(wheel.name.split("-")[0])


def build(dist: Path, out: Path, base_url: str) -> dict[str, list[Path]]:
    wheels = sorted(p for p in dist.glob("*.whl"))
    if not wheels:
        raise SystemExit(f"no wheels in {dist}")

    projects: dict[str, list[Path]] = {}
    for wheel in wheels:
        projects.setdefault(project_of(wheel), []).append(wheel)

    out.mkdir(parents=True, exist_ok=True)

    # The root index lists every project.
    rows = "\n".join(
        f'    <a href="{name}/">{html.escape(name)}</a><br>' for name in sorted(projects)
    )
    (out / "index.html").write_text(
        "<!DOCTYPE html>\n<html>\n  <body>\n"
        f"{rows}\n"
        "  </body>\n</html>\n"
    )

    for name, files in sorted(projects.items()):
        page = out / name
        page.mkdir(parents=True, exist_ok=True)
        links = []
        for wheel in sorted(files):
            # The hash in the fragment is what makes an installer verify the download
            # rather than trust the URL, which matters when the index and the files are
            # served from different hosts.
            digest = hashlib.sha256(wheel.read_bytes()).hexdigest()
            url = f"{base_url.rstrip('/')}/{wheel.name}#sha256={digest}"
            links.append(f'    <a href="{url}">{html.escape(wheel.name)}</a><br>')
        page.joinpath("index.html").write_text(
            "<!DOCTYPE html>\n<html>\n  <body>\n"
            + "\n".join(links)
            + "\n  </body>\n</html>\n"
        )

    return projects


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    parser.add_argument("--out", type=Path, default=Path("site/simple"))
    parser.add_argument(
        "--base-url",
        required=True,
        help="where the wheel files are served from, normally the Release download URL",
    )
    args = parser.parse_args()

    projects = build(args.dist, args.out, args.base_url)
    for name, files in sorted(projects.items()):
        print(f"{name}: {len(files)} file(s)")
        for wheel in sorted(files):
            print(f"  {wheel.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
