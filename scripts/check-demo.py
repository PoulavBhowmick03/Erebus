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
    "Wire v3 encrypts offer terms",
    "It does not hide transaction timing",
    "Readable for one deal",
)
FORBIDDEN_COPY = (
    "current fixed shape of the fifth salt",
    "Readable for one channel",
)


class DemoParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.attrs: list[tuple[str, dict[str, str]]] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        self.attrs.append((tag, {key: value or "" for key, value in attrs}))


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
    return errors


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    errors = check(root)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("demo: mobile, keyboard, assets, video, manifest, and truthful-copy checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
