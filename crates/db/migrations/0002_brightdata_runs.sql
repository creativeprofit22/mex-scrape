-- Bright Data connector: safety settings and durable runs (ADR 0006).
-- Owned by the `db` crate (see docs/modules/db.md).

-- One row (id = 1). The connector starts disabled: an operator enables it
-- with SQL (docs/modules/brightdata.md). A missing row fails closed.
CREATE TABLE safety_settings (
    id                 SMALLINT PRIMARY KEY CHECK (id = 1),
    global_kill_switch BOOLEAN NOT NULL DEFAULT FALSE,
    brightdata_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO safety_settings (id, global_kill_switch, brightdata_enabled)
VALUES (1, FALSE, FALSE);

CREATE TABLE brightdata_runs (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id       TEXT NOT NULL CHECK (tenant_id ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    job             TEXT NOT NULL CHECK (job ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    mode            TEXT NOT NULL CHECK (mode IN ('unlocker', 'dataset')),
    -- The app writes at most 20 000 bytes of compact JSON; jsonb's text
    -- form adds spaces, so the backstop is 32 KiB.
    input_json      JSONB NOT NULL CHECK (octet_length(input_json::text) <= 32768),
    snapshot_id     TEXT CHECK (snapshot_id IS NULL OR snapshot_id ~ '^[A-Za-z0-9_-]{1,200}$'),
    status          TEXT NOT NULL DEFAULT 'starting'
                    CHECK (status IN ('starting', 'running', 'ready', 'imported', 'failed', 'cancelled')),
    requested_count INTEGER NOT NULL DEFAULT 0 CHECK (requested_count >= 0),
    row_count       INTEGER NOT NULL DEFAULT 0 CHECK (row_count >= 0),
    failed_count    INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    error_message   TEXT NOT NULL DEFAULT '' CHECK (char_length(error_message) <= 1000),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- At most one active run per scope (tenant + job).
CREATE UNIQUE INDEX brightdata_runs_one_active
    ON brightdata_runs (tenant_id, job)
    WHERE status IN ('starting', 'running', 'ready');

-- Daily per-tenant caps.
CREATE INDEX brightdata_runs_tenant_created ON brightdata_runs (tenant_id, created_at);

-- Start-up recovery scans active runs.
CREATE INDEX brightdata_runs_status ON brightdata_runs (status);
