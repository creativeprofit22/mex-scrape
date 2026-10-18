"""Extract a plain-text transcript from a public Fathom share link.

Usage: python fathom_transcript.py <share_url> [out_file]
"""

from __future__ import annotations

import argparse
import html
import json
import re
import sys
import urllib.request
from html.parser import HTMLParser
from pathlib import Path
from typing import ClassVar, cast
from urllib.parse import urlparse

DESCRIPTION = "Extract a plain-text transcript from a public Fathom share link."
USER_AGENT = "Mozilla/5.0"
ALLOWED_HOST = "fathom.video"
SHARE_URL_RE = re.compile(r"^https://fathom\.video/share/[\w-]+$")
TIMEOUT_SECONDS = 30


class TranscriptError(Exception):
    """Raised when the transcript cannot be located or downloaded."""


def ensure_fathom_url(url: str) -> str:
    """Return ``url`` if it is an https URL on fathom.video, else raise."""
    parsed = urlparse(url)
    if parsed.scheme != "https" or parsed.hostname != ALLOWED_HOST:
        msg = f"Refusing to fetch non-Fathom URL: {url}"
        raise TranscriptError(msg)
    return url


def fetch(url: str) -> str:
    """Download ``url`` (Fathom https only) and return the decoded body."""
    req = urllib.request.Request(  # noqa: S310 - scheme/host checked above
        ensure_fathom_url(url), headers={"User-Agent": USER_AGENT}
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT_SECONDS) as resp:  # noqa: S310
        body: bytes = resp.read()
    return body.decode("utf-8", "replace")


class TextExtractor(HTMLParser):
    """Collect visible text from HTML, keeping block elements on new lines."""

    BLOCK: ClassVar[frozenset[str]] = frozenset(
        {"p", "div", "br", "li", "h1", "h2", "h3", "h4", "tr", "section"}
    )
    SKIP: ClassVar[frozenset[str]] = frozenset({"script", "style", "noscript"})

    def __init__(self) -> None:
        """Start with no collected text."""
        super().__init__()
        self.parts: list[str] = []
        self.skip_depth = 0

    def handle_starttag(
        self,
        tag: str,
        attrs: list[tuple[str, str | None]],  # noqa: ARG002 - HTMLParser signature
    ) -> None:
        """Track skipped elements and break lines at block starts."""
        if tag in self.SKIP:
            self.skip_depth += 1
        elif tag in self.BLOCK:
            self.parts.append("\n")

    def handle_endtag(self, tag: str) -> None:
        """Leave skipped elements and break lines at block ends."""
        if tag in self.SKIP and self.skip_depth:
            self.skip_depth -= 1
        elif tag in self.BLOCK:
            self.parts.append("\n")

    def handle_data(self, data: str) -> None:
        """Keep text that is not inside a skipped element."""
        if not self.skip_depth:
            self.parts.append(data)

    def text(self) -> str:
        """Return collected text with whitespace normalised."""
        joined = "".join(self.parts)
        joined = re.sub(r"[ \t]+", " ", joined)
        return re.sub(r"\n\s*\n+", "\n\n", joined).strip()


def strip_html(markup: str) -> str:
    """Convert HTML to readable plain text."""
    parser = TextExtractor()
    parser.feed(markup)
    parser.close()
    return parser.text()


def json_field(obj: object, key: str) -> object:
    """Return ``obj[key]`` if ``obj`` is a decoded JSON object, else ``None``."""
    if isinstance(obj, dict):
        return cast("dict[str, object]", obj).get(key)
    return None


def transcript_url_from_page(page: str) -> str:
    """Find the transcript endpoint in the share page's embedded JSON."""
    match = re.search(r'data-page="([^"]+)"', page)
    if not match:
        msg = "No embedded page data found (link private or page format changed)"
        raise TranscriptError(msg)
    data: object = json.loads(html.unescape(match.group(1)))
    url = json_field(json_field(data, "props"), "copyTranscriptUrl")
    if not isinstance(url, str):
        msg = "No transcript URL found in page data"
        raise TranscriptError(msg)
    return ensure_fathom_url(url)


def transcript_body(raw: str) -> str:
    """Unwrap a JSON response if present, then strip any HTML."""
    try:
        data: object = json.loads(raw)
    except json.JSONDecodeError:
        data = None
    for key in ("html", "transcript", "text"):
        value = json_field(data, key)
        if isinstance(value, str) and value:
            raw = value
            break
    return strip_html(raw) if "<" in raw else raw.strip()


def parse_args(argv: list[str]) -> argparse.Namespace:
    """Parse and validate command-line arguments."""
    parser = argparse.ArgumentParser(description=DESCRIPTION)
    parser.add_argument("share_url", help="https://fathom.video/share/<id>")
    parser.add_argument("out_file", nargs="?", type=Path, default=Path("transcript.txt"))
    args = parser.parse_args(argv)
    if not SHARE_URL_RE.match(args.share_url):
        parser.error("Expected a https://fathom.video/share/<id> URL")
    return args


def main(argv: list[str]) -> int:
    """Fetch the transcript for a share link and write it as plain text."""
    args = parse_args(argv)
    out: Path = args.out_file
    try:
        page = fetch(args.share_url)
        text = transcript_body(fetch(transcript_url_from_page(page)))
    except (TranscriptError, OSError, json.JSONDecodeError) as exc:
        sys.stderr.write(f"Error: {exc}\n")
        return 1
    out.write_text(text + "\n", encoding="utf-8")
    sys.stdout.write(f"Wrote {len(text.split())} words to {out}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
