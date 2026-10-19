# secrets

## Purpose

Server-side secret values and the redaction applied to every error string. It has no network code, so any crate (including `db`) can hold a secret without depending on the HTTP client. `http-guard` re-exports everything here, so adapters keep using `http_guard::Secret`.

- `Secret` has no `Debug`, `Display`, `Serialize`, `Default` or sqlx `Encode`, so it cannot be logged, returned in an API response or bound into a DB row (enforced by `compile_fail` doctests).
- Secrets load from the environment (or a secret store via `Secret::new`) at start-up. An absent variable is `None`; an empty, whitespace-only, control-character or non-UTF-8 value is an error that names the variable, never the value.
- `Redactor` replaces known secret values with `[redacted]`, longest first. `sanitize` redacts and then truncates to 500 characters, so truncation never leaves part of a secret behind.

## Public API

- `Secret`: `new`, `from_lookup`, `from_env`, `expose_secret`.
- `SecretError`.
- `Redactor`: `new`, `with`, `redact`, `sanitize`; `Debug` shows only the number of secrets.
- `truncate_error_message`, `MAX_ERROR_MESSAGE_CHARS` (500).

## Dependencies

- Workspace: none.
- External: `thiserror` (dev: `serde_json`, for the doctest proving `Secret` is not `Serialize`).

## Owned tables

None.
