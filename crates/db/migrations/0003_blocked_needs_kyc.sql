-- Bright Data access mode: runs refused for KYC reasons get their own
-- terminal status, and per-URL access outcomes are counted per host.
-- Owned by the `db` crate (see docs/modules/db.md).

-- Widen the status CHECK. The new value is terminal, so the partial unique
-- index on active statuses stays unchanged.
ALTER TABLE brightdata_runs DROP CONSTRAINT brightdata_runs_status_check;
ALTER TABLE brightdata_runs ADD CONSTRAINT brightdata_runs_status_check
    CHECK (status IN ('starting', 'running', 'ready', 'imported', 'failed', 'cancelled', 'blocked_needs_kyc'));

-- One row per URL attempt. Only the host is stored (no path or query).
CREATE TABLE brightdata_access_events (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id     BIGINT NOT NULL REFERENCES brightdata_runs (id) ON DELETE CASCADE,
    host       TEXT NOT NULL CHECK (char_length(host) BETWEEN 1 AND 253),
    outcome    TEXT NOT NULL CHECK (outcome IN ('ok', 'blocked_needs_kyc', 'skipped_robots', 'failed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX brightdata_access_events_host_outcome ON brightdata_access_events (host, outcome);
CREATE INDEX brightdata_access_events_run ON brightdata_access_events (run_id);
