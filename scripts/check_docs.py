"""Check that the docs system matches the Cargo workspace.

Fails (exit 1) when a workspace crate:

- has no ``docs/modules/<crate>.md`` (or it lacks a required section),
- is missing from the module map in ``docs/ARCHITECTURE.md``,
- is missing from the allowed-dependencies table in ``docs/ARCHITECTURE.md``,
- depends on a workspace crate its allowed-dependencies row does not list.

Also fails when ``domain`` has any workspace dependency, or when the
allowed-dependencies table names a crate that does not exist.

Usage: ``uv run python scripts/check_docs.py [--root PATH]``
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

MODULE_SECTIONS = ("## Purpose", "## Public API", "## Dependencies", "## Owned tables")
MODULE_MAP_HEADING = "## Module map"
ALLOWED_HEADING = "## Allowed dependencies"
DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
PURE_CRATE = "domain"
NONE_MARKERS = frozenset({"", "-", "—", "none"})


class DocsCheckError(Exception):
    """Raised when a file needed by the check is missing or malformed."""


@dataclass(frozen=True)
class Crate:
    """A workspace member and the workspace crates it depends on."""

    name: str
    manifest: Path
    workspace_deps: frozenset[str]


def _as_table(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        msg = f"{where}: expected a TOML table"
        raise DocsCheckError(msg)
    table: dict[str, object] = {}
    for key, item in value.items():  # pyright: ignore[reportUnknownVariableType]
        table[str(key)] = item  # pyright: ignore[reportUnknownArgumentType]
    return table


def _as_str_list(value: object, where: str) -> list[str]:
    if not isinstance(value, list):
        msg = f"{where}: expected a list of strings"
        raise DocsCheckError(msg)
    items: list[str] = []
    for item in value:  # pyright: ignore[reportUnknownVariableType]
        if not isinstance(item, str):
            msg = f"{where}: expected a list of strings"
            raise DocsCheckError(msg)
        items.append(item)
    return items


def _load_toml(path: Path) -> dict[str, object]:
    try:
        with path.open("rb") as f:
            return _as_table(tomllib.load(f), str(path))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        msg = f"cannot read {path}: {exc}"
        raise DocsCheckError(msg) from exc


def _member_dirs(root: Path, workspace: dict[str, object]) -> list[Path]:
    dirs: set[Path] = set()
    for pattern in _as_str_list(workspace.get("members", []), "workspace.members"):
        matches = [p for p in root.glob(pattern) if (p / "Cargo.toml").is_file()]
        if not matches:
            msg = f"workspace member pattern {pattern!r} matches no crate"
            raise DocsCheckError(msg)
        dirs.update(matches)
    return sorted(dirs)


def _dependency_tables(manifest: dict[str, object], where: str) -> list[dict[str, object]]:
    tables = [_as_table(manifest[name], where) for name in DEP_TABLES if name in manifest]
    targets = _as_table(manifest.get("target", {}), where)
    for target_name in sorted(targets):
        target = _as_table(targets[target_name], where)
        tables.extend(_as_table(target[name], where) for name in DEP_TABLES if name in target)
    return tables


def load_crates(root: Path) -> list[Crate]:
    """Read every workspace member and its workspace dependencies."""
    root_manifest = _load_toml(root / "Cargo.toml")
    workspace = _as_table(root_manifest.get("workspace", {}), "Cargo.toml [workspace]")
    raw: list[tuple[str, Path, dict[str, object]]] = []
    for member in _member_dirs(root, workspace):
        manifest_path = member / "Cargo.toml"
        manifest = _load_toml(manifest_path)
        package = _as_table(manifest.get("package", {}), f"{manifest_path} [package]")
        name = package.get("name")
        if not isinstance(name, str):
            msg = f"{manifest_path}: missing package.name"
            raise DocsCheckError(msg)
        raw.append((name, manifest_path, manifest))

    names = {name for name, _, _ in raw}
    crates: list[Crate] = []
    for name, manifest_path, manifest in raw:
        deps: set[str] = set()
        for table in _dependency_tables(manifest, str(manifest_path)):
            for key, spec in table.items():
                package_name = key
                if not isinstance(spec, str):  # table form, may rename via `package`
                    renamed = _as_table(spec, str(manifest_path)).get("package")
                    if isinstance(renamed, str):
                        package_name = renamed
                if package_name in names:
                    deps.add(package_name)
        crates.append(Crate(name, manifest_path, frozenset(deps)))
    return sorted(crates, key=lambda c: c.name)


def _section_rows(text: str, heading: str, where: str) -> list[list[str]]:
    lines = text.splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.strip() == heading)
    except StopIteration:
        msg = f"{where}: missing section {heading!r}"
        raise DocsCheckError(msg) from None
    rows: list[list[str]] = []
    for line in lines[start + 1 :]:
        stripped = line.strip()
        if stripped.startswith("## "):
            break
        if not stripped.startswith("|"):
            if rows:
                break
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        rows.append(cells)
    # Drop the header row and the |---| separator.
    return [r for r in rows[2:] if r and r[0]]


def _cell_name(cell: str) -> str:
    return cell.strip().strip("`").strip()


def _allowed_table(text: str, where: str) -> dict[str, frozenset[str]]:
    allowed: dict[str, frozenset[str]] = {}
    for cells in _section_rows(text, ALLOWED_HEADING, where):
        name = _cell_name(cells[0])
        targets = cells[1] if len(cells) > 1 else ""
        deps = {_cell_name(t) for t in targets.split(",")}
        allowed[name] = frozenset(d for d in deps if d.lower() not in NONE_MARKERS)
    return allowed


def check(root: Path) -> list[str]:
    """Return every docs problem found under ``root`` (empty means OK)."""
    crates = load_crates(root)
    names = {c.name for c in crates}
    arch_path = root / "docs" / "ARCHITECTURE.md"
    try:
        arch = arch_path.read_text(encoding="utf-8")
    except OSError as exc:
        msg = f"cannot read {arch_path}: {exc}"
        raise DocsCheckError(msg) from exc

    module_map = {
        _cell_name(r[0]) for r in _section_rows(arch, MODULE_MAP_HEADING, "ARCHITECTURE.md")
    }
    allowed = _allowed_table(arch, "ARCHITECTURE.md")
    errors: list[str] = []

    for crate in crates:
        doc = root / "docs" / "modules" / f"{crate.name}.md"
        if not doc.is_file():
            errors.append(f"{crate.name}: missing docs/modules/{crate.name}.md")
        else:
            text = doc.read_text(encoding="utf-8")
            lines = {line.strip() for line in text.splitlines()}
            errors.extend(
                f"{crate.name}: docs/modules/{crate.name}.md lacks section {section!r}"
                for section in MODULE_SECTIONS
                if section not in lines
            )
        if crate.name not in module_map:
            errors.append(f"{crate.name}: missing from the module map in docs/ARCHITECTURE.md")
        if crate.name not in allowed:
            errors.append(
                f"{crate.name}: missing from the allowed-dependencies table in docs/ARCHITECTURE.md"
            )
        else:
            errors.extend(
                f"{crate.name}: depends on {dep}, which docs/ARCHITECTURE.md does not allow"
                for dep in sorted(crate.workspace_deps - allowed[crate.name])
            )
        if crate.name == PURE_CRATE and crate.workspace_deps:
            deps = ", ".join(sorted(crate.workspace_deps))
            errors.append(f"{PURE_CRATE}: must not depend on workspace crates (found: {deps})")

    for name in sorted(allowed):
        if name not in names:
            errors.append(f"allowed-dependencies table names unknown crate {name!r}")
        errors.extend(
            f"allowed-dependencies row {name!r} names unknown crate {dep!r}"
            for dep in sorted(allowed[name] - names)
        )
    return errors


def main(argv: list[str] | None = None) -> int:
    """Run the check and report problems on stderr."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0] if __doc__ else None)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository root (default: parent of scripts/)",
    )
    args = parser.parse_args(argv)
    root: Path = args.root
    try:
        errors = check(root)
    except DocsCheckError as exc:
        sys.stderr.write(f"docs check: {exc}\n")
        return 1
    if errors:
        for error in errors:
            sys.stderr.write(f"docs check: {error}\n")
        sys.stderr.write(f"docs check failed with {len(errors)} problem(s)\n")
        return 1
    sys.stdout.write("docs check passed\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
