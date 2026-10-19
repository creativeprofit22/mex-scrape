"""Tests for ``check_docs``: the ``app`` layer dependency rule (ADR 0009).

Run with ``uv run python -m unittest discover -s scripts -p "test_*.py"``.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import check_docs

SECTIONS = "\n\n".join(check_docs.MODULE_SECTIONS) + "\n"


def _write_workspace(root: Path, app_deps: str) -> None:
    """Write a minimal workspace with ``domain``, ``db`` and ``app`` crates.

    ``app_deps`` is the body of app's ``[dependencies]`` table. The docs allow
    app to depend on ``domain`` and ``db``, so only the app rule can object.
    """
    (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*"]\n', encoding="utf-8")
    manifests = {
        "domain": "",
        "db": 'domain = { path = "../domain" }\n',
        "app": app_deps,
    }
    for name, deps in manifests.items():
        crate = root / "crates" / name
        crate.mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            f'[package]\nname = "{name}"\n\n[dependencies]\n{deps}', encoding="utf-8"
        )
    modules = root / "docs" / "modules"
    modules.mkdir(parents=True)
    for name in manifests:
        (modules / f"{name}.md").write_text(SECTIONS, encoding="utf-8")
    (root / "docs" / "ARCHITECTURE.md").write_text(
        "## Module map\n\n| Crate | Role |\n|---|---|\n"
        "| `domain` | d |\n| `db` | d |\n| `app` | a |\n\n"
        "## Allowed dependencies\n\n| Crate | May depend on |\n|---|---|\n"
        "| `domain` | none |\n| `db` | `domain` |\n| `app` | `domain`, `db` |\n",
        encoding="utf-8",
    )


def _app_errors(app_deps: str) -> list[str]:
    """Return the ``app:`` errors ``check`` reports for ``app_deps``."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _write_workspace(root, app_deps)
        return [e for e in check_docs.check(root) if e.startswith("app:")]


class AppRuleTest(unittest.TestCase):
    """The application layer may depend only on ``domain`` and non-I/O libraries."""

    def test_infrastructure_and_framework_deps_are_reported(self) -> None:
        """Depending on ``db``, ``sqlx`` and ``axum`` reports each one."""
        errors = _app_errors(
            'domain = { path = "../domain" }\ndb = { path = "../db" }\nsqlx = "0.9"\naxum = "0.8"\n'
        )
        self.assertEqual(len(errors), 3, errors)
        for dep in ("depends on db;", "depends on sqlx;", "depends on axum;"):
            self.assertTrue(any(dep in e for e in errors), (dep, errors))

    def test_http_client_dep_is_reported(self) -> None:
        """Depending on ``reqwest`` is reported by the app rule."""
        errors = _app_errors('domain = { path = "../domain" }\nreqwest = "0.13"\n')
        self.assertTrue(any("I/O and framework crates" in e for e in errors), errors)

    def test_domain_only_passes(self) -> None:
        """Depending on ``domain`` (plus non-I/O libraries) reports nothing."""
        errors = _app_errors('domain = { path = "../domain" }\nthiserror = "2"\n')
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
