-- User accounts, web sessions and search-result stock/membership columns
-- (ADR 0012). Owned by the `db` crate (see docs/modules/db.md).

-- One row per account. The tenant owning the user's data is derived from
-- the id, so it can never be chosen by a client. `password_hash` is a PHC
-- string (argon2) and is never returned by reads other than sign-in.
CREATE TABLE users (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE
                  CHECK (char_length(email) BETWEEN 3 AND 254 AND email = lower(email)),
    password_hash TEXT NOT NULL CHECK (char_length(password_hash) BETWEEN 1 AND 512),
    tenant_id     TEXT GENERATED ALWAYS AS ('user-' || id::text) STORED UNIQUE,
    plan          TEXT NOT NULL DEFAULT 'free' CHECK (plan IN ('free', 'pro')),
    has_costco    BOOLEAN NOT NULL DEFAULT false,
    has_sams      BOOLEAN NOT NULL DEFAULT false,
    onboarded     BOOLEAN NOT NULL DEFAULT false,
    locale        TEXT NOT NULL DEFAULT 'es' CHECK (locale IN ('es', 'en')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Server-side web sessions (tower-sessions records, serialized by `api`).
-- Only the opaque id travels in the cookie. Expired rows are ignored on
-- read and deleted by `db::sessions::delete_expired`.
CREATE TABLE sessions (
    id         TEXT PRIMARY KEY CHECK (char_length(id) BETWEEN 1 AND 64),
    data       BYTEA NOT NULL CHECK (octet_length(data) <= 65536),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX sessions_expires_at ON sessions (expires_at);

-- Stock state and membership flag of each search result, so the API can
-- hide offers from stores the user has no membership for.
ALTER TABLE search_results
    ADD COLUMN availability TEXT NOT NULL DEFAULT 'unknown'
        CHECK (availability IN ('in_stock', 'out_of_stock', 'pre_order', 'unknown')),
    ADD COLUMN membership_required BOOLEAN NOT NULL DEFAULT false;
