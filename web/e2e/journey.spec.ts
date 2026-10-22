import { expect, test } from "@playwright/test";

import { interpolate } from "../src/i18n/format";
import {
  ASIN,
  catalogs,
  completeSearches,
  expectAccessible,
  pendingAccount,
  signIn,
  tabTo,
  useLanguage,
  type Lang,
} from "./helpers";

// Seeded fixture (crates/db/src/bin/seed_e2e.rs): Liverpool sells at
// $1,299.00 against Amazon's $1,999.00 with $350.00 fees, so with no prep
// or shipping the profit is $350.00. The account holds Costco only, so the
// Sam's Club offer is hidden.
const LIVERPOOL_PROFIT = /350\.00/;

for (const lang of ["es", "en"] as const satisfies readonly Lang[]) {
  test(`sign in → search → product page (${lang}, keyboard only)`, async ({ page }) => {
    const t = catalogs[lang];
    await signIn(page, lang);
    await useLanguage(page, lang);
    await expect(page).toHaveURL(`/${lang}/search`);
    await expectAccessible(page);

    // Search with the keyboard; the fixture completer plays the worker.
    await tabTo(page, page.getByLabel(t.search.label));
    await page.keyboard.type("taladro");
    await page.keyboard.press("Enter");
    const heading = page.getByRole("heading", {
      name: interpolate(t.search.resultsHeading, { query: "taladro" }),
    });
    await expect(async () => {
      await completeSearches();
      await expect(heading).toBeVisible({ timeout: 3_000 });
    }).toPass({ timeout: 60_000 });
    // Next keeps the previous route mounted but hidden (Activity), so
    // after the language switch only visible elements count.
    await expect(page.getByTestId("search-status").filter({ visible: true })).toHaveText(
      t.search.status.done,
    );

    // Membership hiding, match badges and the /roi-computed profit.
    await expect(page.getByText(interpolate(t.search.hidden, { count: 1 }))).toBeVisible();
    await expect(page.locator("tbody tr").filter({ visible: true })).toHaveCount(3);
    const liverpool = page.getByRole("row", { name: /E2E-LIV-1/ });
    await expect(liverpool.getByText(t.search.match.accepted)).toBeVisible();
    await expect(liverpool.getByTestId("result-profit")).toHaveText(LIVERPOOL_PROFIT);
    const costco = page.getByRole("row", { name: /E2E-COS-1/ });
    await expect(costco.getByText(t.search.match.needsReview)).toBeVisible();
    await expect(costco.getByText(t.search.membership)).toBeVisible();
    await expect(
      page.getByRole("row", { name: /E2E-WAL-9/ }).getByText(t.search.match.none),
    ).toBeVisible();
    const store = liverpool.getByRole("link", { name: t.search.storeLink });
    await expect(store).toHaveAttribute("target", "_blank");
    await expect(store).toHaveAttribute("rel", "noopener noreferrer");
    await expectAccessible(page);

    // On to the product's price history.
    await tabTo(
      page,
      liverpool.getByRole("link", { name: interpolate(t.search.match.history, { asin: ASIN }) }),
    );
    await page.keyboard.press("Enter");
    await page.waitForURL(`/${lang}/products/${ASIN}`);
    await expect(
      page.getByRole("heading", { level: 1, name: interpolate(t.product.heading, { asin: ASIN }) }),
    ).toBeVisible();
    await expect(page.getByRole("img")).toBeVisible();
    // Three seeded listings, each with 30/90/365-day windows.
    await expect(page.getByTestId("price-stats")).toHaveCount(3);
    await expect(page.getByTestId("price-stats").first().locator("tbody tr")).toHaveCount(3);
    // The user has Costco, so its listing shows and nothing is hidden.
    await expect(
      page.getByText(
        interpolate(t.product.statsCaption, { retailer: t.retailers.costco_mx, sku: "E2E-COS-1" }),
      ),
    ).toBeVisible();
    await expect(page.getByTestId("hidden-listings")).toHaveCount(0);

    // The table fallback opens from the keyboard too.
    await tabTo(page, page.getByText(t.product.tableToggle));
    await page.keyboard.press("Enter");
    await expect(page.locator("details")).toHaveAttribute("open", "");
    await expect(page.locator("details table")).toHaveCount(3);
    await expectAccessible(page);
  });

  test(`a failed /roi call shows an error, not blank cells (${lang})`, async ({ page }) => {
    const t = catalogs[lang];
    await page.route("**/backend/roi", (route) =>
      route.fulfill({
        status: 429,
        contentType: "application/json",
        body: JSON.stringify({ error: "rate_limited" }),
      }),
    );
    await signIn(page, lang);
    await useLanguage(page, lang);
    await page.goto(`/${lang}/search`);
    await page.getByLabel(t.search.label).fill("taladro");
    await page.keyboard.press("Enter");
    const heading = page.getByRole("heading", {
      name: interpolate(t.search.resultsHeading, { query: "taladro" }),
    });
    await expect(async () => {
      await completeSearches();
      await expect(heading).toBeVisible({ timeout: 3_000 });
    }).toPass({ timeout: 60_000 });
    await expect(page.getByTestId("roi-error").filter({ visible: true })).toHaveText(
      interpolate(t.search.roiFailed, { reason: t.common.errors.rateLimited }),
    );
  });

  test(`search and product pages send a not-onboarded user to onboarding (${lang})`, async ({
    page,
  }) => {
    const t = catalogs[lang];
    // The account's saved language is Spanish, so sign-in lands on /es.
    await signIn(page, lang, pendingAccount);
    await expect(page).toHaveURL("/es/onboarding");
    // A still-valid session that skips onboarding is sent back to it, with
    // the page it asked for kept as `?next=` so finishing returns there.
    for (const path of ["/search", `/products/${ASIN}`]) {
      await page.goto(`/${lang}${path}`);
      await page.waitForURL(`/${lang}/onboarding?next=${encodeURIComponent(`/${lang}${path}`)}`);
      await expect(page.getByRole("checkbox", { name: t.onboarding.costco })).not.toBeChecked();
    }
    await expect(page.getByLabel(t.search.label)).toHaveCount(0);
    // The calculator needs no memberships, so it stays open.
    await page.goto(`/${lang}/calculator`);
    await expect(page).toHaveURL(`/${lang}/calculator`);
  });

  test(`calculator matches search ROI; other pages pass axe (${lang})`, async ({ page }) => {
    const t = catalogs[lang];
    for (const path of ["", "/signup"]) {
      await page.goto(`/${lang}${path}`);
      await expectAccessible(page);
    }
    await signIn(page, lang);
    await useLanguage(page, lang);

    await page.goto(`/${lang}/onboarding`);
    await expect(page.getByRole("checkbox", { name: t.onboarding.costco })).toBeChecked();
    await expect(page.getByRole("checkbox", { name: t.onboarding.sams })).not.toBeChecked();
    await expectAccessible(page);

    await page.goto(`/${lang}/calculator`);
    for (const [label, value] of [
      [t.calculator.sellPrice, "1999"],
      [t.calculator.buyPrice, "1299"],
      [t.calculator.fees, "350"],
    ] as const) {
      await tabTo(page, page.getByLabel(label));
      await page.keyboard.type(value);
    }
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("calc-profit")).toHaveText(LIVERPOOL_PROFIT);
    await expectAccessible(page);
  });
}
