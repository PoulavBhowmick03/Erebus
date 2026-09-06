#!/usr/bin/env python3
"""Static release gate for the public Erebus evidence page."""

from __future__ import annotations

import json
import sys
from html.parser import HTMLParser
from pathlib import Path


REQUIRED_COPY = (
    "This browser simulation",
    "It does not submit a transaction or use a wallet.",
    "Two screened 1 STRK canaries settled through MCP",
    "Public three-minute walkthrough of the complete mainnet workflow.",
    "Wire v3 encrypts offer terms",
    "It does not hide transaction timing",
    "Readable for one deal",
)
FORBIDDEN_COPY = (
    "current fixed shape of the fifth salt",
    "Readable for one channel",
    "Recorded before the later full mainnet canary",
)
# The same claim, loosened, so a reworded version cannot slip through. The README
# carried "records the sprint state before the later full mainnet canary" for two
# days after the video was replaced, because this gate only read demo/index.html.
STALE_CLAIMS = ("before the later full mainnet canary",)
PROSE = ("README.md", "web/README.md")
WEB_SOURCE_DIRS = ("app", "components", "lib")


class DemoParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.attrs: list[tuple[str, dict[str, str]]] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        self.attrs.append((tag, {key: value or "" for key, value in attrs}))


def check_web(root: Path) -> list[str]:
    """The published page.

    `demo/` is the archived sprint page; `web/` is what the pinned demo URL actually
    serves. The truthful-copy contract has to follow the thing people can read, so it
    is asserted against the JSX sources — no build step, so CI needs no Node.
    """
    errors: list[str] = []
    web = root / "web"
    if not web.is_dir():
        return errors

    sources: list[Path] = []
    for name in WEB_SOURCE_DIRS:
        directory = web / name
        if not directory.is_dir():
            continue
        for suffix in ("*.tsx", "*.ts"):
            sources.extend(
                path for path in directory.rglob(suffix) if "node_modules" not in path.parts
            )
    if not sources:
        errors.append("web: no page sources found")
        return errors

    normalized = " ".join(" ".join(path.read_text().split()) for path in sources)
    for phrase in REQUIRED_COPY:
        if phrase not in normalized:
            errors.append(f"web: missing truthful copy {phrase!r}")
    for phrase in FORBIDDEN_COPY:
        if phrase in normalized:
            errors.append(f"web: stale copy {phrase!r}")

    # strk20.json pins demo_video to this host, and web/ is the host now.
    video = web / "public" / "erebus-private-sprint.mp4"
    if not video.is_file() or video.stat().st_size < 1_000_000:
        errors.append("web/public: the pinned demo video is missing or unexpectedly small")
    return errors


def check_prose(root: Path) -> list[str]:
    """Claims in the README drift out of step with the page they describe."""
    errors: list[str] = []
    for name in PROSE:
        path = root / name
        if not path.is_file():
            continue
        text = " ".join(path.read_text().split())
        for phrase in STALE_CLAIMS:
            if phrase in text:
                errors.append(f"{name}: stale claim {phrase!r}")
    return errors


def check(root: Path) -> list[str]:
    errors: list[str] = []
    demo = root / "demo"
    html = (demo / "index.html").read_text()
    css = (demo / "styles.css").read_text()
    script = (demo / "app.js").read_text()
    manifest = json.loads((root / "strk20.json").read_text())
    parser = DemoParser()
    parser.feed(html)
    normalized_html = " ".join(html.split())

    for phrase in REQUIRED_COPY:
        if phrase not in normalized_html:
            errors.append(f"demo/index.html: missing truthful copy {phrase!r}")
    for phrase in FORBIDDEN_COPY:
        if phrase in normalized_html:
            errors.append(f"demo/index.html: stale copy {phrase!r}")

    if 'name="viewport"' not in html:
        errors.append("demo/index.html: missing mobile viewport")
    if "@media (max-width: 850px)" not in css:
        errors.append("demo/styles.css: missing mobile layout")
    if "@media (prefers-reduced-motion: reduce)" not in css:
        errors.append("demo/styles.css: missing reduced-motion behavior")
    if ":focus-visible" not in css:
        errors.append("demo/styles.css: missing visible keyboard focus")

    ids = {attrs.get("id") for _, attrs in parser.attrs if "id" in attrs}
    for control in ("budget", "reserve", "run-demo"):
        if control not in ids:
            errors.append(f"demo/index.html: missing control #{control}")
    label_targets = {
        attrs.get("for") for tag, attrs in parser.attrs if tag == "label" and attrs.get("for")
    }
    for control in ("budget", "reserve"):
        if control not in label_targets:
            errors.append(f"demo/index.html: #{control} has no label")
    if 'aria-live="polite"' not in html:
        errors.append("demo/index.html: transcript is not announced")

    local_refs = []
    for tag, attrs in parser.attrs:
        value = attrs.get("href") if tag in {"a", "link"} else attrs.get("src")
        if not value or value.startswith(("#", "https://")):
            continue
        local_refs.append(value)
    for reference in local_refs:
        target = demo / reference.lstrip("/")
        if not target.is_file():
            errors.append(f"demo/index.html: missing local asset {reference!r}")

    video = demo / "erebus-private-sprint.mp4"
    if not video.is_file() or video.stat().st_size < 1_000_000:
        errors.append("demo video is missing or unexpectedly small")
    if manifest.get("demo_url") != "https://erebus-private-agents.vercel.app":
        errors.append("strk20.json: unexpected public demo URL")
    if manifest.get("demo_video") != (
        "https://erebus-private-agents.vercel.app/erebus-private-sprint.mp4"
    ):
        errors.append("strk20.json: unexpected public video URL")
    if "deal-scoped viewing grant" not in script:
        errors.append("demo/app.js: disclosure simulation is not deal-scoped")

    errors.extend(check_web(root))
    errors.extend(check_prose(root))
    return errors


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    errors = check(root)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(
        "demo + web: mobile, keyboard, assets, video, manifest, prose, "
        "and truthful-copy checks passed"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
