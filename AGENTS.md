# AGENTS.md

Python 3.11+ CLI (`fathom_transcript.py`) that extracts plain-text transcripts from public Fathom share links. Managed with [uv](https://docs.astral.sh/uv/).

## Commands

- Install: `uv sync --locked`
- Run: `uv run python fathom_transcript.py <share_url> [out_file]`
- Lint: `uvx ruff@0.15.20 check .`
- Format check: `uvx ruff@0.15.20 format --check .` (fix with `uvx ruff@0.15.20 format .`)
- Typecheck: `uvx pyright@1.1.414` (strict mode)
- Tests: none yet. There is also no build step.

## Rules

- CI lives in `.github/workflows/ci.yml` and must stay green.
- Never commit with `--no-verify`.
- Never commit transcript output (`transcript.txt`): it may contain private meeting data.
