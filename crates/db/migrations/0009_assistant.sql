-- AI assistant: per-tenant token usage, match suggestions made by the
-- assistant and the user's match decisions (ADR 0013). Owned by the `db`
-- crate (see docs/modules/db.md). CHECKs mirror the domain rules
-- (TenantId, RetailerId, RetailerSku, Asin, MatchDecision,
-- SuggestionReason, ChatId). The global `offer_matches` table is never
-- written by the assistant: decisions only overlay the deciding tenant's
-- own views.

-- Assistant tokens used per tenant per UTC day (daily token budget).
CREATE TABLE agent_token_usage (
    tenant_id     TEXT NOT NULL CHECK (tenant_id ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    day           DATE NOT NULL,
    input_tokens  BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    PRIMARY KEY (tenant_id, day)
);

-- A verdict the assistant proposed for a `needs_review` listing of the
-- tenant. Never applied on its own: the user confirms it in the UI.
CREATE TABLE match_suggestions (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id    TEXT NOT NULL CHECK (tenant_id ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    chat_id      TEXT NOT NULL CHECK (chat_id ~ '^[A-Za-z0-9-]{8,64}$'),
    retailer     TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    retailer_sku TEXT NOT NULL CHECK (char_length(retailer_sku) BETWEEN 1 AND 100),
    asin         TEXT NOT NULL CHECK (asin ~ '^[A-Z0-9]{10}$'),
    title        TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 500),
    decision     TEXT NOT NULL CHECK (decision IN ('accept', 'reject')),
    reason       TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 300),
    created_at   TIMESTAMPTZ NOT NULL,
    -- Lets match_decisions reference a suggestion of the same tenant only.
    UNIQUE (id, tenant_id)
);

CREATE INDEX match_suggestions_tenant_chat ON match_suggestions (tenant_id, chat_id, id);

-- A user's verdict on a listing's Amazon match, scoped to their tenant.
-- One row per tenant and listing; a new verdict replaces the old one.
CREATE TABLE match_decisions (
    tenant_id          TEXT NOT NULL CHECK (tenant_id ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    retailer           TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    retailer_sku       TEXT NOT NULL CHECK (char_length(retailer_sku) BETWEEN 1 AND 100),
    asin               TEXT NOT NULL CHECK (asin ~ '^[A-Z0-9]{10}$'),
    decision           TEXT NOT NULL CHECK (decision IN ('accept', 'reject')),
    decided_by_user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    suggestion_id      BIGINT,
    decided_at         TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, retailer, retailer_sku),
    FOREIGN KEY (suggestion_id, tenant_id) REFERENCES match_suggestions (id, tenant_id)
);

CREATE INDEX match_decisions_tenant_asin ON match_decisions (tenant_id, asin);
