-- Baseline schema. Owned by the `db` crate (see docs/modules/db.md).
CREATE TABLE app_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
