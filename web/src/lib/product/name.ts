/**
 * Pure helpers for the Amazon product name (ADR 0017): the product page
 * heading and detail panel line, and telling "no such product" apart from
 * "no history yet". No React, no I/O, so node-only specs cover them.
 */

/**
 * The stored Amazon name when there is one, else `fallback` (e.g. the
 * "price history of {asin}" heading). Blank names count as missing.
 */
export function productTitle(title: string | null | undefined, fallback: string): string {
  const name = title?.trim() ?? "";
  return name === "" ? fallback : name;
}

/**
 * True when a product request failed because the product doesn't exist for
 * us: 404 `product_not_found` (no match and no stored name) or 400
 * `invalid_asin`. Any other failure is retryable.
 */
export function isProductNotFound(status: number, code: string | undefined): boolean {
  return (status === 404 && code === "product_not_found") || (status === 400 && code === "invalid_asin");
}
