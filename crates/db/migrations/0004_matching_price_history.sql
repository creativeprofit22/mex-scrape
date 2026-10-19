-- Product matching and append-only price history.
-- Owned by the `db` crate (see docs/modules/db.md).

-- One match per retailer listing to an Amazon product. CHECKs mirror the
-- domain rules (RetailerId, RetailerSku, Asin, MatchMethod, Confidence,
-- MatchStatus). `rejected` is set only by reviewers.
CREATE TABLE offer_matches (
    retailer      TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    retailer_sku  TEXT NOT NULL CHECK (char_length(retailer_sku) BETWEEN 1 AND 100),
    asin          TEXT NOT NULL CHECK (asin ~ '^[A-Z0-9]{10}$'),
    method        TEXT NOT NULL CHECK (method IN ('gtin', 'title')),
    confidence_bp INTEGER NOT NULL CHECK (confidence_bp BETWEEN 0 AND 10000),
    status        TEXT NOT NULL CHECK (status IN ('accepted', 'needs_review', 'rejected')),
    matched_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (retailer, retailer_sku)
);

CREATE INDEX offer_matches_asin_status ON offer_matches (asin, status);
CREATE INDEX offer_matches_needs_review ON offer_matches (matched_at)
    WHERE status = 'needs_review';

-- Every observed retailer price, kept indefinitely. Money is integer
-- centavos. Rows are never updated or deleted (triggers below).
CREATE TABLE price_observations (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    retailer       TEXT NOT NULL CHECK (retailer ~ '^[a-z0-9_]{1,40}$'),
    retailer_sku   TEXT NOT NULL CHECK (char_length(retailer_sku) BETWEEN 1 AND 100),
    price_mxn      BIGINT NOT NULL CHECK (price_mxn >= 0),
    list_price_mxn BIGINT CHECK (list_price_mxn > price_mxn),
    availability   TEXT NOT NULL CHECK (availability IN ('in_stock', 'out_of_stock', 'pre_order', 'unknown')),
    observed_at    TIMESTAMPTZ NOT NULL
);

-- Per-product time-range reads (`WHERE retailer = .. AND retailer_sku = ..
-- AND observed_at >= ..`).
CREATE INDEX price_observations_listing_time
    ON price_observations (retailer, retailer_sku, observed_at);

CREATE FUNCTION price_observations_append_only() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'price_observations is append-only (% refused)', TG_OP
        USING ERRCODE = 'restrict_violation';
END;
$$;

CREATE TRIGGER price_observations_no_update_delete
    BEFORE UPDATE OR DELETE ON price_observations
    FOR EACH ROW EXECUTE FUNCTION price_observations_append_only();

CREATE TRIGGER price_observations_no_truncate
    BEFORE TRUNCATE ON price_observations
    FOR EACH STATEMENT EXECUTE FUNCTION price_observations_append_only();
