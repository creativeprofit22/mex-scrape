import { expect, test } from "@playwright/test";

// Node-only: the product name helpers (src/lib/product), no browser.
import { isProductNotFound, productTitle } from "../src/lib/product/name";

test.describe("productTitle", () => {
  test("uses the stored Amazon name, trimmed", () => {
    expect(productTitle("Licuadora Oster 600 W", "fallback")).toBe("Licuadora Oster 600 W");
    expect(productTitle("  Licuadora  ", "fallback")).toBe("Licuadora");
  });

  test("falls back when no name is stored", () => {
    expect(productTitle(null, "Historial de precios de B0E2ETEST1")).toBe("Historial de precios de B0E2ETEST1");
    expect(productTitle(undefined, "ASIN B0E2ETEST1")).toBe("ASIN B0E2ETEST1");
    expect(productTitle("   ", "ASIN B0E2ETEST1")).toBe("ASIN B0E2ETEST1");
  });
});

test.describe("isProductNotFound", () => {
  test("is true for an unknown or rejected ASIN", () => {
    expect(isProductNotFound(404, "product_not_found")).toBe(true);
    expect(isProductNotFound(400, "invalid_asin")).toBe(true);
  });

  test("is false for failures worth a retry", () => {
    expect(isProductNotFound(404, undefined)).toBe(false);
    expect(isProductNotFound(404, "not_found")).toBe(false);
    expect(isProductNotFound(500, "internal")).toBe(false);
    expect(isProductNotFound(503, "unavailable")).toBe(false);
    expect(isProductNotFound(400, "invalid_days")).toBe(false);
  });
});
