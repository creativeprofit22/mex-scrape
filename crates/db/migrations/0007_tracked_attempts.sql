-- Rotate failing tracked listings behind the others (ADR 0011).
-- Owned by the `db` crate (see docs/modules/db.md).

-- Set for every SKU of a `product_refresh` batch once its run returns,
-- whether or not an offer parsed. The due order uses
-- COALESCE(last_attempted_at, last_refreshed_at), so a listing that keeps
-- failing (blocked, robots, delisted, unparseable) no longer stays first.
ALTER TABLE tracked_products ADD COLUMN last_attempted_at TIMESTAMPTZ;

DROP INDEX tracked_products_due;
CREATE INDEX tracked_products_due ON tracked_products
    ((COALESCE(last_attempted_at, last_refreshed_at)) NULLS FIRST, retailer, retailer_sku);
