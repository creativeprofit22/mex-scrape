import { expect, test, type Locator, type Page } from "@playwright/test";

import { interpolate } from "../src/i18n/format";
import type { components } from "../src/lib/api/schema";
import { COSTS_KEY, RECENT_KEY } from "../src/lib/search/storage";
import { ASIN, account, catalogs, expectAccessible } from "./helpers";

// The pages outside search (docs/modules/web/pages.md): calculator, product,
// assistant, sign-in/sign-up, onboarding and home. Every API call is answered
// by `page.route`, so these run without the seeded backend.

type Schemas = components["schemas"];

const t = catalogs.es;
const DESKTOP = { width: 1440, height: 900 };
const PHONE = { width: 390, height: 844 };
const VIEWPORTS = [
  { name: "1440", size: DESKTOP },
  { name: "390", size: PHONE },
] as const;

const ME = {
  id: 1,
  email: account.email,
  locale: "es",
  plan: "free",
  onboarded: true,
  has_costco: true,
  has_sams: false,
} satisfies Schemas["MeResponse"];

const AS_OF_MS = 1_792_600_000_000;
const DAY = 86_400_000;

const PRODUCT_TITLE = "Licuadora de prueba 1.5 L, 600 W, vaso de vidrio";
const PRODUCT_BRAND = "Marca de prueba";

const HISTORY = {
  asin: ASIN,
  title: PRODUCT_TITLE,
  brand: PRODUCT_BRAND,
  as_of_ms: AS_OF_MS,
  since_ms: AS_OF_MS - 365 * DAY,
  hidden_listings: 2,
  listings: [
    {
      retailer: "costco",
      retailer_sku: "COS-1",
      points: Array.from({ length: 10 }, (_, i) => ({
        day_ms: AS_OF_MS - (9 - i) * DAY,
        low_centavos: 129_900 + (i % 3) * 2_000,
        high_centavos: 134_900,
        in_stock: true,
      })),
    },
  ],
} satisfies Schemas["PriceHistoryResponse"];

const STATS = {
  asin: ASIN,
  title: PRODUCT_TITLE,
  brand: PRODUCT_BRAND,
  as_of_ms: AS_OF_MS,
  hidden_listings: 2,
  listings: [
    {
      retailer: "costco",
      retailer_sku: "COS-1",
      windows: [30, 90, 365].map((days) => ({
        days,
        low_centavos: 129_900,
        high_centavos: 134_900,
        average_centavos: 131_500,
        observations: 10,
      })),
    },
  ],
} satisfies Schemas["PriceStatsResponse"];

// What the api sends for a matched product with no prices yet and no stored name.
const EMPTY_HISTORY = {
  ...HISTORY,
  title: null,
  brand: null,
  hidden_listings: 0,
  listings: [],
} satisfies Schemas["PriceHistoryResponse"];
const EMPTY_STATS = {
  ...STATS,
  title: null,
  brand: null,
  hidden_listings: 0,
  listings: [],
} satisfies Schemas["PriceStatsResponse"];

const SUGGESTIONS = [
  {
    id: 7,
    retailer: "costco",
    retailer_sku: "COS-88213",
    asin: ASIN,
    decision: "accept",
    title: "Licuadora de prueba 1.5 L",
    reason: "Mismo modelo y capacidad.",
  },
  {
    id: 8,
    retailer: "sams",
    retailer_sku: "SAMS-4471",
    asin: ASIN,
    decision: "reject",
    title: "Licuadora de prueba 2 L",
    reason: "La capacidad no coincide.",
  },
] satisfies Schemas["MatchSuggestionResponse"][];

type FakeApi = {
  /** Bodies sent to `POST /matches/decisions`. */
  decisions: unknown[];
  /** Answers the product endpoints with this error until it is `null`. */
  productError: { status: number; code: string } | null;
};

/**
 * Fakes the api. `session`: `in` (onboarded), `pending` (not onboarded) or
 * `out` (401 until a sign-in succeeds). `authError` answers sign-in and
 * sign-up with that error code. `productError` answers the product
 * endpoints with that error; `emptyProduct` answers them as the api does
 * for a matched product with no prices and no stored name (200, empty
 * `listings`, null `title`).
 */
async function fakeApi(
  page: Page,
  opts: {
    session: "in" | "pending" | "out";
    authError?: { status: number; code: string };
    productError?: { status: number; code: string } | null;
    emptyProduct?: boolean;
  },
): Promise<FakeApi> {
  let session = opts.session;
  const api: FakeApi = { decisions: [], productError: opts.productError ?? null };
  await page.route("**/backend/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace(/^\/backend/, "");
    const unauthorized = { status: 401, json: { error: "unauthorized" } };
    const me = { ...ME, onboarded: session === "in" };
    if (path === "/auth/csrf") return route.fulfill({ json: { csrf_token: "test-token" } });
    if (path === "/auth/signin" || path === "/auth/signup") {
      if (opts.authError) {
        return route.fulfill({ status: opts.authError.status, json: { error: opts.authError.code } });
      }
      session = "in";
      return route.fulfill({ json: { csrf_token: "test-token", user: { ...ME } } });
    }
    if (path === "/me") return route.fulfill(session === "out" ? unauthorized : { json: me });
    if (session === "out") return route.fulfill(unauthorized);
    if (path === "/me/onboarding") {
      session = "in";
      return route.fulfill({ json: { ...ME } });
    }
    if (path.endsWith("/price-history") || path.endsWith("/price-stats")) {
      if (api.productError !== null) {
        return route.fulfill({ status: api.productError.status, json: { error: api.productError.code } });
      }
      const history = opts.emptyProduct ? EMPTY_HISTORY : HISTORY;
      const stats = opts.emptyProduct ? EMPTY_STATS : STATS;
      return route.fulfill({ json: path.endsWith("/price-history") ? history : stats });
    }
    if (path === "/roi") {
      const body = request.postDataJSON() as { items: Schemas["RoiItemRequest"][] };
      const item = body.items[0];
      if (!item) return route.fulfill({ status: 400, json: { error: "invalid_amount" } });
      const cost =
        item.buy_price_centavos +
        item.amazon_fees_centavos +
        (item.prep_fee_centavos ?? 0) +
        (item.shipping_centavos ?? 0);
      const profit = item.sell_price_centavos - cost;
      return route.fulfill({
        json: {
          results: [
            {
              profit_centavos: profit,
              roi_bp: Math.round((profit * 10_000) / cost),
              margin_bp: Math.round((profit * 10_000) / item.sell_price_centavos),
              total_cost_centavos: cost,
              fixed_cost_per_unit_centavos: 0,
            },
          ],
        } satisfies Schemas["RoiResponse"],
      });
    }
    if (path === "/assistant/chat") {
      const body = request.postDataJSON() as { messages: { content: string }[] };
      const last = body.messages.at(-1)?.content ?? "";
      const tie = /desempat/i.test(last);
      return route.fulfill({
        json: {
          chat_id: "test",
          stop: "done",
          usage: { input_tokens: 1, output_tokens: 1 },
          reply: tie ? "Propongo aceptar la de Costco." : `Respuesta a: ${last}`,
          suggestions: tie ? SUGGESTIONS : [],
        } satisfies Schemas["ChatResponse"],
      });
    }
    if (path === "/matches/decisions") {
      const body = request.postDataJSON() as Schemas["MatchDecisionRequest"];
      api.decisions.push(body);
      return route.fulfill({
        json: { asin: body.asin, decision: body.decision, retailer: body.retailer, retailer_sku: body.retailer_sku },
      });
    }
    return route.fulfill({ status: 404, json: { error: "not_found" } });
  });
  return api;
}

/** Waits until React has hydrated `selector` (typing before that is lost). */
async function waitForHydration(page: Page, selector: string): Promise<void> {
  await page.waitForFunction((css) => {
    const el = document.querySelector(css);
    return el !== null && Object.keys(el).some((key) => key.startsWith("__reactProps"));
  }, selector);
}

type Box = { top: number; bottom: number; left: number; right: number };

async function box(locator: Locator): Promise<Box> {
  return locator.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom, left: r.left, right: r.right };
  });
}

// Next keeps visited pages mounted but hidden, so every lookup is visible-only.
const visible = (locator: Locator) => locator.filter({ visible: true });
// Next's route announcer is an alert too, so match the text.
const alert = (page: Page, text: string) => visible(page.getByRole("alert")).filter({ hasText: text });

test.describe("calculator", () => {
  const c = t.calculator;
  const form = (page: Page) => visible(page.locator(".calculator__form"));
  const result = (page: Page) => visible(page.locator(".calculator__result"));

  async function fill(page: Page, values: { sell: string; buy: string; fees: string }) {
    await waitForHydration(page, ".calculator__form input");
    await page.getByLabel(c.sellPrice).fill(values.sell);
    await page.getByLabel(c.buyPrice).fill(values.buy);
    await page.getByLabel(c.fees).fill(values.fees);
  }

  test("inputs beside the result at 1440, stacked at 390", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    await page.setViewportSize(DESKTOP);
    await page.goto("/es/calculator");
    await expect(result(page)).toContainText(c.resultEmpty);
    let f = await box(form(page));
    let r = await box(result(page));
    expect(r.left).toBeGreaterThanOrEqual(f.right);
    expect(Math.abs(r.top - f.top)).toBeLessThan(80);
    await expectAccessible(page);

    await page.setViewportSize(PHONE);
    f = await box(form(page));
    r = await box(result(page));
    expect(r.top).toBeGreaterThanOrEqual(f.bottom);
    await expectAccessible(page);
  });

  test("the result stays in place while the inputs scroll", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    // Short enough that the form is taller than the workspace.
    await page.setViewportSize({ width: 1440, height: 560 });
    await page.goto("/es/calculator");
    const workspace = page.locator("main.workspace");
    const scrollable = await workspace.evaluate((el) => el.scrollHeight > el.clientHeight + 100);
    expect(scrollable).toBe(true);
    await workspace.evaluate((el) => el.scrollTo({ top: 120 }));
    const before = await box(result(page));
    await workspace.evaluate((el) => el.scrollTo({ top: 260 }));
    const after = await box(result(page));
    expect(after.top).toBe(before.top);
  });

  for (const vp of VIEWPORTS) {
    test(`profit reads as gain or loss at ${vp.name}`, async ({ page }) => {
      await fakeApi(page, { session: "in" });
      await page.setViewportSize(vp.size);
      await page.goto("/es/calculator");
      await fill(page, { sell: "1000", buy: "500", fees: "150" });
      await visible(page.getByRole("button", { name: c.submit })).click();
      const profit = visible(page.getByTestId("calc-profit"));
      await expect(profit).toHaveText(/\+\$350\.00/);
      await expect(profit.locator(".gain")).toHaveCount(1);
      await expect(result(page)).toBeInViewport();
      await expectAccessible(page);

      await page.getByLabel(c.buyPrice).fill("1200");
      await visible(page.getByRole("button", { name: c.submit })).click();
      await expect(profit).toHaveText(/\$350\.00/);
      await expect(profit.locator(".loss")).toHaveCount(1);
      await expect(profit).toContainText("−");
      await expectAccessible(page);
    });
  }

  test("an invalid amount marks its field and says why", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    await page.setViewportSize(DESKTOP);
    await page.goto("/es/calculator");
    await fill(page, { sell: "12.345", buy: "500", fees: "" });
    await visible(page.getByRole("button", { name: c.submit })).click();
    await expect(alert(page, c.errors.invalidFields)).toBeVisible();

    const sell = page.getByLabel(c.sellPrice);
    await expect(sell).toHaveAttribute("aria-invalid", "true");
    await expect(sell).toBeFocused();
    await expect(sell).toHaveAccessibleDescription(c.fieldErrors.amount);
    await expect(page.getByLabel(c.fees)).toHaveAccessibleDescription(c.fieldErrors.required);
    await expect(page.getByLabel(c.buyPrice)).not.toHaveAttribute("aria-invalid");
    await expectAccessible(page);

    // Fixing a field clears its mark at once.
    await sell.fill("1000");
    await expect(sell).not.toHaveAttribute("aria-invalid");
  });

  test("units beyond what the api accepts are marked and never sent", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    let roiCalls = 0;
    page.on("request", (request) => {
      if (new URL(request.url()).pathname === "/backend/roi") roiCalls += 1;
    });
    await page.setViewportSize(DESKTOP);
    await page.goto("/es/calculator");
    await fill(page, { sell: "1000", buy: "500", fees: "150" });
    // Above u32::MAX (4294967295), the api's `units_per_month` type.
    const units = page.getByLabel(c.unitsPerMonth);
    await units.fill("5000000000");
    await visible(page.getByRole("button", { name: c.submit })).click();
    await expect(alert(page, c.errors.invalidFields)).toBeVisible();
    await expect(units).toHaveAttribute("aria-invalid", "true");
    await expect(units).toHaveAccessibleDescription(c.fieldErrors.units);
    expect(roiCalls).toBe(0);
  });

  test("prep and shipping start from the costs saved on search", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    await page.addInitScript(
      ([key]) => localStorage.setItem(key, JSON.stringify({ v: 1, prep: 2_500, shipping: 4_050 })),
      [COSTS_KEY],
    );
    await page.goto("/es/calculator");
    await expect(page.getByLabel(c.prep)).toHaveValue("25");
    await expect(page.getByLabel(c.shipping)).toHaveValue("40.50");
  });
});

test.describe("product page", () => {
  const p = t.product;
  const heading = (page: Page) => page.getByRole("heading", { level: 1, name: PRODUCT_TITLE });
  const fallbackHeading = (page: Page) =>
    page.getByRole("heading", { level: 1, name: interpolate(p.heading, { asin: ASIN }) });

  for (const vp of VIEWPORTS) {
    test(`header leads back to search; stats and the hidden note at ${vp.name}`, async ({ page }) => {
      await fakeApi(page, { session: "in" });
      await page.setViewportSize(vp.size);
      await page.goto(`/es/products/${ASIN}`);
      await expect(heading(page)).toBeVisible();
      // The ASIN and brand are metadata under the Amazon name.
      await expect(page.locator(".product-header__meta")).toContainText(ASIN);
      await expect(page.getByTestId("product-brand")).toContainText(PRODUCT_BRAND);
      await expect(visible(page.getByTestId("price-stats"))).toBeVisible();
      await expect(visible(page.getByTestId("hidden-listings"))).toHaveText(
        interpolate(p.hidden, { count: 2 }),
      );
      // Nothing saved in this browser: no rerun link.
      await expect(page.getByTestId("product-rerun")).toHaveCount(0);
      await expectAccessible(page);
      await visible(page.getByTestId("product-back")).click();
      await page.waitForURL(/\/es\/search$/);
    });
  }

  test("the newest recent search can be rerun from the header", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    await page.addInitScript(
      ([key]) => localStorage.setItem(key, JSON.stringify({ v: 1, items: ["licuadora", "taladro"] })),
      [RECENT_KEY],
    );
    await page.goto(`/es/products/${ASIN}`);
    const rerun = visible(page.getByTestId("product-rerun"));
    await expect(rerun).toHaveText(interpolate(p.rerunRecent, { query: "licuadora" }));
    await expect(rerun).toHaveAttribute("href", /\/es\/search\?.*licuadora/);
  });

  test("the English page also leads with the Amazon name", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    await page.goto(`/en/products/${ASIN}`);
    await expect(page.getByRole("heading", { level: 1, name: PRODUCT_TITLE })).toBeVisible();
    await expect(page.getByTestId("product-brand")).toContainText(catalogs.en.product.brandLabel);
  });

  test("a known product with no prices and no name shows the empty state under the ASIN heading", async ({
    page,
  }) => {
    await fakeApi(page, { session: "in", emptyProduct: true });
    await page.goto(`/es/products/${ASIN}`);
    await expect(visible(page.getByTestId("product-empty"))).toHaveText(p.empty);
    await expect(fallbackHeading(page)).toBeVisible();
    await expect(page.getByTestId("product-brand")).toHaveCount(0);
    await expect(page.getByTestId("product-not-found")).toHaveCount(0);
    await expect(page.getByTestId("hidden-listings")).toHaveCount(0);
    await expectAccessible(page);
    await visible(page.getByTestId("product-back")).click();
    await page.waitForURL(/\/es\/search$/);
  });

  for (const error of [
    { status: 404, code: "product_not_found" },
    { status: 400, code: "invalid_asin" },
  ]) {
    test(`an unknown product (${error.status} ${error.code}) shows not found with a way back to search`, async ({
      page,
    }) => {
      await fakeApi(page, { session: "in", productError: error });
      await page.goto(`/es/products/${ASIN}`);
      const notFound = visible(page.getByTestId("product-not-found"));
      await expect(notFound.getByRole("heading", { level: 1, name: p.notFoundHeading })).toBeVisible();
      await expect(notFound.getByRole("alert")).toHaveText(interpolate(p.notFound, { asin: ASIN }));
      await expect(notFound.getByRole("link", { name: p.searchCta })).toHaveAttribute("href", "/es/search");
      await expect(page.getByRole("button", { name: t.common.retry })).toHaveCount(0);
      await expect(page.getByTestId("product-empty")).toHaveCount(0);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      await expectAccessible(page);
    });
  }

  test("a failed load offers Reintentar, which recovers", async ({ page }) => {
    const api = await fakeApi(page, { session: "in", productError: { status: 500, code: "internal" } });
    await page.goto(`/es/products/${ASIN}`);
    const retry = visible(page.getByRole("button", { name: t.common.retry }));
    await expect(retry).toBeVisible();
    await expect(alert(page, interpolate(p.loadFailed, { reason: "" }).trim())).toBeVisible();
    await expectAccessible(page);
    api.productError = null;
    await retry.click();
    await expect(visible(page.getByTestId("price-stats"))).toBeVisible();
    await expect(heading(page)).toBeVisible();
  });
});

test.describe("assistant", () => {
  const a = t.assistant;
  const composer = (page: Page) => visible(page.locator("form.chat-composer"));
  const input = (page: Page) => visible(page.getByLabel(a.label));
  const log = (page: Page) => visible(page.getByTestId("chat-log"));

  /** Sends `text` with Enter and waits for the reply (two more log entries). */
  async function say(page: Page, text: string) {
    const entries = log(page).locator(".chat-who");
    const before = await entries.count();
    await input(page).fill(text);
    await input(page).press("Enter");
    await expect(entries).toHaveCount(before + 2);
  }

  for (const vp of VIEWPORTS) {
    test(`the composer stays in view as the log grows at ${vp.name}`, async ({ page }) => {
      await fakeApi(page, { session: "in" });
      await page.setViewportSize(vp.size);
      await page.goto("/es/assistant");
      await waitForHydration(page, "form.chat-composer textarea");
      for (const n of [1, 2, 3]) {
        await say(page, `Pregunta número ${n} sobre precios`);
        await expect(log(page).getByText(`Respuesta a: Pregunta número ${n} sobre precios`)).toBeVisible();
      }
      await expect(log(page).locator(".chat-who")).toHaveCount(6);
      const workspace = await box(page.locator("main.workspace"));
      const form = await box(composer(page));
      expect(form.top).toBeGreaterThanOrEqual(workspace.top);
      expect(form.bottom).toBeLessThanOrEqual(workspace.bottom + 1);
      // The newest reply is in view above the composer.
      const last = await box(log(page).getByText("Respuesta a: Pregunta número 3 sobre precios"));
      expect(last.bottom).toBeLessThanOrEqual(form.top + 1);
      await expectAccessible(page);
    });
  }

  test("suggestion cards sit in the log; only Confirmar writes", async ({ page }) => {
    const api = await fakeApi(page, { session: "in" });
    await page.setViewportSize(DESKTOP);
    await page.goto("/es/assistant");
    await waitForHydration(page, "form.chat-composer textarea");
    await say(page, "Ayúdame a desempatar");
    const cards = log(page).getByTestId("suggestion");
    await expect(cards).toHaveCount(2);
    await expect(log(page).getByRole("heading", { name: a.suggestionsHeading })).toBeVisible();
    await expectAccessible(page);

    const dismiss = page.getByRole("button", {
      name: interpolate(a.dismissLabel, { title: SUGGESTIONS[1].title }),
    });
    await dismiss.click();
    await expect(cards.nth(1)).toContainText(a.dismissed);
    await expect(cards.nth(1).getByRole("status")).toBeFocused();
    expect(api.decisions).toHaveLength(0);

    await page
      .getByRole("button", { name: interpolate(a.confirmLabel, { title: SUGGESTIONS[0].title }) })
      .click();
    await expect(cards.nth(0)).toContainText(a.accepted);
    expect(api.decisions).toHaveLength(1);
    expect(api.decisions[0]).toMatchObject({ suggestion_id: 7, decision: "accept" });
    await expectAccessible(page);
  });
});

test.describe("sign-in and sign-up", () => {
  const au = t.auth;

  for (const vp of VIEWPORTS) {
    test(`a weak password marks the field and keeps its hint at ${vp.name}`, async ({ page }) => {
      await fakeApi(page, { session: "out", authError: { status: 400, code: "weak_password" } });
      await page.setViewportSize(vp.size);
      await page.goto("/es/signup");
      await waitForHydration(page, ".card form input");
      await expect(visible(page.locator(".card"))).toBeVisible();
      await page.getByLabel(au.email).fill("nuevo@example.com");
      const password = page.getByLabel(au.password);
      await password.fill("12345678");
      await expect(password).toHaveAccessibleDescription(au.passwordHint);
      await password.press("Enter");

      await expect(alert(page, au.errors.signUpCheck)).toBeVisible();
      await expect(password).toHaveAttribute("aria-invalid", "true");
      const ids = ((await password.getAttribute("aria-describedby")) ?? "").split(" ");
      expect(ids).toHaveLength(2);
      await expect(page.locator(`[id="${ids[0]}"]`)).toHaveText(au.passwordHint);
      await expect(page.locator(`[id="${ids[1]}"]`)).toHaveText(au.errors.weakPassword);
      await expect(page.getByLabel(au.email)).not.toHaveAttribute("aria-invalid");
      await expectAccessible(page);
    });
  }

  test("wrong credentials are a form error that names no field", async ({ page }) => {
    await fakeApi(page, { session: "out", authError: { status: 401, code: "invalid_credentials" } });
    await page.goto("/es/signin");
    await waitForHydration(page, ".card form input");
    await page.getByLabel(au.email).fill(account.email);
    await page.getByLabel(au.password).fill("not-the-password");
    await page.getByLabel(au.password).press("Enter");
    await expect(alert(page, au.errors.invalidCredentials)).toBeVisible();
    await expect(page.locator('[aria-invalid="true"]')).toHaveCount(0);
    await expect(page).toHaveURL(/\/es\/signin/);
    await expectAccessible(page);
  });
});

test.describe("onboarding on touch", () => {
  test.use({ viewport: PHONE, hasTouch: true, isMobile: true });

  test("membership rows are large and keep native checkboxes", async ({ page }) => {
    await fakeApi(page, { session: "pending" });
    await page.goto("/es/onboarding");
    const costco = visible(page.getByRole("checkbox", { name: t.onboarding.costco }));
    const sams = visible(page.getByRole("checkbox", { name: t.onboarding.sams }));
    await expect(costco).toBeChecked();
    await expect(sams).not.toBeChecked();
    for (const row of await visible(page.locator("label.check-row")).all()) {
      const height = await row.evaluate((el) => el.getBoundingClientRect().height);
      expect(height).toBeGreaterThanOrEqual(44);
    }
    await expect(costco).toHaveJSProperty("type", "checkbox");
    // The whole row toggles the box.
    await visible(page.locator("label.check-row").filter({ hasText: t.onboarding.sams })).tap();
    await expect(sams).toBeChecked();
    await expectAccessible(page);
    await visible(page.getByRole("button", { name: t.onboarding.submit })).tap();
    await page.waitForURL(/\/es\/search$/);
  });
});

test.describe("home", () => {
  for (const vp of VIEWPORTS) {
    test(`signed out: three steps, no service status, at ${vp.name}`, async ({ page }) => {
      await fakeApi(page, { session: "out" });
      await page.setViewportSize(vp.size);
      await page.goto("/es");
      const steps = page.locator("ol.home-steps > li");
      await expect(steps).toHaveCount(3);
      await expect(steps.nth(0)).toContainText(t.home.steps.search.title);
      await expect(steps.nth(2)).toContainText(t.home.steps.profit.title);
      await expect(page.getByText(/API|health/i)).toHaveCount(0);
      await expect(page.getByRole("link", { name: t.home.createAccount, exact: true }).first()).toBeVisible();
      await expect(page).toHaveURL(/\/es$/);
      await expectAccessible(page);
    });
  }

  test("signed in goes straight to search, and Back skips home", async ({ page }) => {
    await fakeApi(page, { session: "in" });
    await page.goto("/es/calculator");
    await page.goto("/es");
    await page.waitForURL(/\/es\/search$/);
    await page.goBack();
    await page.waitForURL(/\/es\/calculator$/);
  });

  test("signed in but not onboarded goes to onboarding", async ({ page }) => {
    await fakeApi(page, { session: "pending" });
    await page.goto("/es");
    await page.waitForURL(/\/es\/onboarding/);
  });

  test("after onboarding, the wordmark leads home and on to search", async ({ page }) => {
    // `/me` says not onboarded until `PUT /me/onboarding` succeeds.
    await fakeApi(page, { session: "pending" });
    await page.goto("/es/onboarding");
    await visible(page.getByRole("button", { name: t.onboarding.submit })).click();
    await page.waitForURL(/\/es\/search$/);

    const urls: string[] = [];
    page.on("framenavigated", (frame) => {
      if (frame === page.mainFrame()) urls.push(new URL(frame.url()).pathname);
    });
    await visible(page.getByRole("link", { name: t.nav.wordmark, exact: true })).first().click();
    await expect.poll(() => urls).toContain("/es");
    await page.waitForURL(/\/es\/search$/);
    expect(urls).not.toContain("/es/onboarding");
  });
});
