import { expect, test, type Page } from "@playwright/test";

import { interpolate } from "../src/i18n/format";
import type { components } from "../src/lib/api/schema";
import { ASIN, account, catalogs } from "./helpers";

// Recovery paths: every API call is answered by `page.route`, so these run
// without the seeded backend and can fake failures it can't produce.

const t = catalogs.es;

const ME = {
  id: 1,
  email: account.email,
  locale: "es",
  plan: "free",
  onboarded: true,
  has_costco: true,
  has_sams: false,
} satisfies components["schemas"]["MeResponse"];

const EMPTY_HISTORY = {
  asin: ASIN,
  as_of_ms: 0,
  since_ms: 0,
  hidden_listings: 0,
  listings: [],
} satisfies components["schemas"]["PriceHistoryResponse"];

const EMPTY_STATS = {
  asin: ASIN,
  as_of_ms: 0,
  hidden_listings: 0,
  listings: [],
} satisfies components["schemas"]["PriceStatsResponse"];

/**
 * Fakes the api: signed out until `POST /auth/signin`, then signed in.
 * `/me` can be overridden to fail.
 */
async function fakeApi(page: Page, opts: { meStatus?: number } = {}): Promise<void> {
  let signedIn = false;
  await page.route("**/backend/**", async (route) => {
    const path = new URL(route.request().url()).pathname.replace(/^\/backend/, "");
    const unauthorized = { status: 401, json: { error: "unauthorized" } };
    if (path === "/auth/csrf") return route.fulfill({ json: { csrf_token: "test-token" } });
    if (path === "/auth/signin") {
      signedIn = true;
      return route.fulfill({ json: { csrf_token: "test-token", user: ME } });
    }
    if (path === "/me") {
      if (opts.meStatus) return route.fulfill({ status: opts.meStatus, json: { error: "internal" } });
      return route.fulfill(signedIn ? { json: ME } : unauthorized);
    }
    if (!signedIn) return route.fulfill(unauthorized);
    if (path.endsWith("/price-history")) return route.fulfill({ json: EMPTY_HISTORY });
    if (path.endsWith("/price-stats")) return route.fulfill({ json: EMPTY_STATS });
    if (path === "/roi") {
      return route.fulfill({
        json: { results: [{ profit_centavos: 49_500, roi_bp: 2_905, margin_bp: 2_251 }] },
      });
    }
    return route.fulfill({ status: 404, json: { error: "not_found" } });
  });
}

/**
 * Waits until React has hydrated `selector`: text typed into a controlled
 * input before that is reset by hydration (on any page, not just here).
 */
async function waitForHydration(page: Page, selector: string): Promise<void> {
  await page.waitForFunction((css) => {
    const el = document.querySelector(css);
    return el !== null && Object.keys(el).some((key) => key.startsWith("__reactProps"));
  }, selector);
}

async function submitSignIn(page: Page): Promise<void> {
  // Next keeps visited pages mounted but hidden, so use the visible form.
  const form = page.locator("form").filter({ visible: true });
  await form.getByLabel(t.auth.email).fill(account.email);
  await form.getByLabel(t.auth.password).fill(account.password);
  await form.getByRole("button", { name: t.auth.signInSubmit }).click();
}

test("a lost connection offers a retry instead of loading forever", async ({ page }) => {
  await page.route("**/backend/**", (route) => route.abort("internetdisconnected"));
  await page.goto("/es/search");
  await expect(page.getByText(t.common.errors.network)).toBeVisible();

  await page.unroute("**/backend/**");
  await page.route("**/backend/me", (route) => route.fulfill({ json: ME }));
  await page.getByRole("button", { name: t.common.retry }).click();
  await expect(page.getByRole("heading", { name: t.search.heading })).toBeVisible();
});

test("a failed membership load shows a retry, not an empty form", async ({ page }) => {
  await fakeApi(page, { meStatus: 500 });
  await page.goto("/es/onboarding");
  await expect(page.getByText(t.common.errors.generic)).toBeVisible();
  // Saving unticked boxes here would erase the memberships the user has.
  await expect(page.getByRole("checkbox")).toHaveCount(0);

  await page.unroute("**/backend/**");
  await page.route("**/backend/me", (route) => route.fulfill({ json: ME }));
  await page.getByRole("button", { name: t.common.retry }).click();
  await expect(page.getByRole("checkbox", { name: t.onboarding.costco })).toBeChecked();
});

test("sign-in returns to the page that asked for it", async ({ page }) => {
  await fakeApi(page);
  const product = `/es/products/${ASIN}`;
  await page.goto(product);
  await expect(page).toHaveURL(`/es/signin?next=${encodeURIComponent(product)}`);

  await submitSignIn(page);
  await expect(page).toHaveURL(product);
  await expect(
    page.getByRole("heading", { name: interpolate(t.product.heading, { asin: ASIN }) }),
  ).toBeVisible();
});

for (const next of ["//evil.example/x", "https://evil.example/", "/\\evil.example", "/es/signin"]) {
  test(`sign-in ignores an unsafe next (${next})`, async ({ page }) => {
    await fakeApi(page);
    await page.goto(`/es/signin?next=${encodeURIComponent(next)}`);
    await submitSignIn(page);
    await expect(page).toHaveURL("/es/search");
  });
}

test("the calculator keeps typed values through sign-in", async ({ page }) => {
  await fakeApi(page);
  await page.goto("/es/calculator");
  await waitForHydration(page, 'input[name="sellPrice"]');
  const typed = {
    [t.calculator.sellPrice]: "2199",
    [t.calculator.buyPrice]: "1299",
    [t.calculator.fees]: "350",
    [t.calculator.prep]: "15",
  };
  for (const [label, value] of Object.entries(typed)) {
    await page.getByLabel(label, { exact: true }).fill(value);
  }
  await page.getByRole("button", { name: t.calculator.submit }).click();
  await expect(page).toHaveURL(`/es/signin?next=${encodeURIComponent("/es/calculator")}`);

  await submitSignIn(page);
  await expect(page).toHaveURL("/es/calculator");
  for (const [label, value] of Object.entries(typed)) {
    await expect(page.getByLabel(label, { exact: true })).toHaveValue(value);
  }
  await page.getByRole("button", { name: t.calculator.submit }).click();
  await expect(page.getByTestId("calc-profit")).toHaveText(/495\.00/);
});

test("the sign-in ↔ sign-up switch keeps where the user was going", async ({ page }) => {
  await fakeApi(page);
  const product = `/es/products/${ASIN}`;
  const withNext = (path: string) => `${path}?next=${encodeURIComponent(product)}`;
  await page.goto(withNext("/es/signin"));

  await page.getByRole("link", { name: t.auth.noAccount, exact: true }).click();
  await expect(page).toHaveURL(withNext("/es/signup"));
  await expect(page.getByRole("heading", { name: t.auth.signUpHeading })).toBeVisible();

  await page.getByRole("link", { name: t.auth.haveAccount, exact: true }).click();
  await expect(page).toHaveURL(withNext("/es/signin"));

  // Back undoes the switch and still remembers the detour.
  await page.goBack();
  await expect(page).toHaveURL(withNext("/es/signup"));
  await page.goBack();
  await expect(page).toHaveURL(withNext("/es/signin"));
  await submitSignIn(page);
  await expect(page).toHaveURL(product);
});

test("the sign-in ↔ sign-up switch drops an unsafe next", async ({ page }) => {
  await fakeApi(page);
  await page.goto(`/es/signin?next=${encodeURIComponent("//evil.example/x")}`);
  const toSignUp = page.getByRole("link", { name: t.auth.noAccount, exact: true });
  await expect(toSignUp).toHaveAttribute("href", "/es/signup");
  await toSignUp.click();
  await expect(page).toHaveURL("/es/signup");
});
