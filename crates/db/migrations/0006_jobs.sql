-- Background jobs: pgmq queues, searches and their results, tracked
-- products and the Amazon request budget (ADR 0011).
-- Owned by the `db` crate (see docs/modules/db.md).

-- Queues (pgmq, installed by 0005). `refresh_jobs` carries both
-- `product_refresh` and `category_browse` messages. Each queue has a
-- dead-letter twin for messages that exhausted their attempts or are
-- malformed.
SELECT pgmq.create('search_jobs');
SELECT pgmq.create('search_jobs_dlq');
SELECT pgmq.create('refresh_jobs');
SELECT pgmq.create('refresh_jobs_dlq');

-- One row per search started through the API. `reason` holds a fixed,
-- secret-free code (never an upstream message).
CREATE TABLE searches (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id   TEXT NOT NULL CHECK (tenant_id ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    query       TEXT NOT NULL CHECK (char_length(query) BETWEEN 1 AND 100),
    status      TEXT NOT NULL DEFAULT 'queued'
                CHECK (status IN ('queued', 'running', 'done', 'partial', 'failed', 'refused')),
    reason      TEXT NOT NULL DEFAULT '' CHECK (char_length(reason) <= 200),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX searches_tenant_created ON searches (tenant_id, created_at);

-- Per-retailer outcome of a search. `category_only` marks stores without
-- keyword search (ADR 0008) so a search never returns nothing silently.
CREATE TABLE search_retailers (
    search_id BIGINT NOT NULL REFERENCES searches (id) ON DELETE CASCADE,
    retailer  TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    outcome   TEXT NOT NULL CHECK (outcome IN
              ('ok', 'category_only', 'blocked_needs_kyc', 'skipped_robots', 'failed', 'no_offers')),
    offers    INTEGER NOT NULL DEFAULT 0 CHECK (offers >= 0),
    PRIMARY KEY (search_id, retailer)
);

-- One row per retailer offer found by a search, with its Amazon match and
-- ROI when one was found. Money is integer centavos, ratios basis points.
CREATE TABLE search_results (
    search_id        BIGINT NOT NULL REFERENCES searches (id) ON DELETE CASCADE,
    retailer         TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    retailer_sku     TEXT NOT NULL CHECK (char_length(retailer_sku) BETWEEN 1 AND 100),
    title            TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 500),
    url              TEXT NOT NULL CHECK (char_length(url) BETWEEN 1 AND 2048),
    price_mxn        BIGINT NOT NULL CHECK (price_mxn >= 0),
    asin             TEXT CHECK (asin IS NULL OR asin ~ '^[A-Z0-9]{10}$'),
    match_status     TEXT CHECK (match_status IS NULL OR match_status IN ('accepted', 'needs_review', 'rejected')),
    confidence_bp    INTEGER CHECK (confidence_bp IS NULL OR confidence_bp BETWEEN 0 AND 10000),
    amazon_price_mxn BIGINT CHECK (amazon_price_mxn IS NULL OR amazon_price_mxn >= 0),
    fees_mxn         BIGINT CHECK (fees_mxn IS NULL OR fees_mxn >= 0),
    profit_centavos  BIGINT,
    roi_bp           BIGINT,
    margin_bp        BIGINT,
    PRIMARY KEY (search_id, retailer, retailer_sku)
);

-- Listings refreshed on a schedule. A listing is tracked once it has an
-- accepted Amazon match.
CREATE TABLE tracked_products (
    retailer          TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    retailer_sku      TEXT NOT NULL CHECK (char_length(retailer_sku) BETWEEN 1 AND 100),
    tracked_at        TIMESTAMPTZ NOT NULL,
    last_refreshed_at TIMESTAMPTZ,
    PRIMARY KEY (retailer, retailer_sku)
);

CREATE INDEX tracked_products_due ON tracked_products (last_refreshed_at NULLS FIRST, retailer, retailer_sku);

-- Paid provider requests per tenant per UTC day (Amazon request budget).
CREATE TABLE provider_usage (
    tenant_id TEXT NOT NULL CHECK (tenant_id ~ '^[A-Za-z0-9_.:-]{1,100}$'),
    provider  TEXT NOT NULL CHECK (provider IN ('amazon')),
    day       DATE NOT NULL,
    requests  INTEGER NOT NULL DEFAULT 0 CHECK (requests >= 0),
    PRIMARY KEY (tenant_id, provider, day)
);
