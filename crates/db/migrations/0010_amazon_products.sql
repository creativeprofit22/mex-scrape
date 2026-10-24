-- Amazon catalog data seen during matching (ADR 0017): the product name
-- and brand of each matched ASIN. Owned by the `db` crate (see
-- docs/modules/db.md). Global like `offer_matches`: catalog data is the
-- same for every tenant. CHECKs mirror app::AmazonProduct.
CREATE TABLE amazon_products (
    asin    TEXT PRIMARY KEY CHECK (asin ~ '^[A-Z0-9]{10}$'),
    title   TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 500),
    brand   TEXT CHECK (brand IS NULL OR char_length(brand) BETWEEN 1 AND 200),
    source  TEXT NOT NULL CHECK (source IN ('sp_api', 'bright_data')),
    seen_at TIMESTAMPTZ NOT NULL
);
