import { expect, test, type Page } from "@playwright/test";

import { formatMoney, interpolate } from "../src/i18n/format";
import type { components } from "../src/lib/api/schema";
import { COSTS_KEY, RECENT_KEY } from "../src/lib/search/storage";
import { ASIN, account, catalogs, expectAccessible } from "./helpers";

// The search workspace against a faked api (`page.route`), so every search
// state can be produced without the seeded backend or a scraper.

const t = catalogs.es;
const s = t.search;

type Schemas = components["schemas"];
type Scenario =
  | "done"
  | "partial"
  | "empty"
  | "allhidden"
  | "failed"
  | "refused"
  | "queued"
  | "slow"
  | "stores";

const ME = {
  id: 1,
  email: account.email,
  locale: "es",
  plan: "free",
  onboarded: true,
  has_costco: true,
  has_sams: false,
} satisfies Schemas["MeResponse"];

const STORES = ["walmart_mx", "bodega_aurrera", "chedraui", "sanborns", "liverpool", "costco_mx", "sears_mx"];

function match(
  asin: string,
  sell: number,
  fees: number,
  status = "accepted",
): Schemas["SearchMatchResponse"] {
  return {
    asin,
    confidence_bp: status === "accepted" ? 9_200 : 6_100,
    match_status: status,
    amazon_price_centavos: sell,
    fees_centavos: fees,
    profit_centavos: null,
    roi_bp: null,
    margin_bp: null,
  };
}

function result(
  retailer: string,
  sku: string,
  title: string,
  price: number,
  availability: string,
  amazon: Schemas["SearchMatchResponse"] | null,
): Schemas["SearchResultResponse"] {
  return {
    retailer,
    retailer_sku: sku,
    title,
    url: `https://example.com/${retailer}/${sku}`,
    price_centavos: price,
    availability,
    membership_required: false,
    amazon,
  };
}

// Synthetic offers. Profit with no costs = Amazon price − fees − store price:
// Alfa +290, Gamma +180, Beta −28, Épsilon −200, Delta none (no match).
const RESULTS = [
  result("walmart_mx", "WM-1", "Licuadora Alfa", 79_900, "in_stock", match(ASIN, 129_900, 21_000)),
  result("bodega_aurrera", "BA-1", "Batidora Beta", 45_900, "out_of_stock", match("B0SYNBETA1", 52_900, 9_800)),
  result("chedraui", "CH-1", "Cafetera Gamma", 64_900, "in_stock", match("B0SYNGAMA1", 99_900, 17_000, "needs_review")),
  result("sanborns", "SA-1", "Hervidor Delta", 39_900, "in_stock", null),
  result("liverpool", "LI-1", "Tostador Épsilon", 89_900, "in_stock", match("B0SYNEPSI1", 84_900, 15_000)),
];
const BY_PROFIT = ["Licuadora Alfa", "Cafetera Gamma", "Batidora Beta", "Tostador Épsilon", "Hervidor Delta"];

function retailers(failed: ReadonlySet<string> = new Set()): Schemas["SearchRetailerResponse"][] {
  return STORES.map((retailer) => {
    if (failed.has(retailer)) return { retailer, offers: 0, outcome: "failed" };
    const offers = RESULTS.filter((r) => r.retailer === retailer).length;
    return { retailer, offers, outcome: offers > 0 ? "ok" : "no_offers" };
  });
}

function searchBody(id: number, query: string, scenario: Scenario): Schemas["SearchResponse"] {
  const base = {
    id,
    query,
    created_at_ms: 1_792_600_000_000,
    finished_at_ms: null,
    hidden_results: 0,
    reason: null,
    results: [],
    retailers: [],
  };
  const done = { ...base, finished_at_ms: 1_792_600_030_000 };
  switch (scenario) {
    case "queued":
      return { ...base, status: "queued" };
    case "slow":
      return { ...base, status: "running" };
    case "stores":
      return { ...base, status: "running", retailers: retailers() };
    case "partial":
      return {
        ...done,
        status: "partial",
        reason: "retailers_failed",
        retailers: retailers(new Set(["sears_mx"])),
        results: RESULTS,
      };
    case "empty":
      return {
        ...done,
        status: "done",
        retailers: STORES.map((retailer) => ({ retailer, offers: 0, outcome: "no_offers" })),
      };
    case "allhidden":
      return { ...done, status: "done", hidden_results: 3, retailers: retailers() };
    case "failed":
      return {
        ...done,
        status: "failed",
        reason: "all_retailers_failed",
        retailers: retailers(new Set(STORES)),
      };
    case "refused":
      // An unknown refusal code falls back to the generic message.
      return { ...done, status: "refused", reason: "query_not_allowed" };
    case "done":
      return { ...done, status: "done", hidden_results: 2, retailers: retailers(), results: RESULTS };
  }
}

function roi(item: Schemas["RoiItemRequest"]): Schemas["RoiItemResponse"] {
  const total = item.buy_price_centavos + (item.prep_fee_centavos ?? 0) + (item.shipping_centavos ?? 0);
  const profit = item.sell_price_centavos - item.amazon_fees_centavos - total;
  return {
    error: null,
    fixed_cost_per_unit_centavos: 0,
    total_cost_centavos: total,
    profit_centavos: profit,
    roi_bp: Math.round((profit * 10_000) / total),
    margin_bp: Math.round((profit * 10_000) / item.sell_price_centavos),
  };
}

/** What the fake api saw, plus switches a test flips mid-run. */
type FakeApi = {
  /** Scenario for the next `POST /searches` (each search keeps its own). */
  scenario: Scenario;
  /** `POST /roi` answers 500. */
  roiFails: boolean;
  /** `GET /searches/{id}` never reaches the api (connection lost). */
  dropPolls: boolean;
  queries: string[];
  roiBodies: Schemas["RoiRequest"][];
  /** `GET /searches/{id}` count per search id. */
  polls: Map<number, number>;
};

async function fakeApi(page: Page, scenario: Scenario = "done"): Promise<FakeApi> {
  const api: FakeApi = { scenario, roiFails: false, dropPolls: false, queries: [], roiBodies: [], polls: new Map() };
  const searches = new Map<number, { query: string; scenario: Scenario }>();
  await page.route("**/backend/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace(/^\/backend/, "");
    if (path === "/auth/csrf") return route.fulfill({ json: { csrf_token: "test-token" } });
    if (path === "/me") return route.fulfill({ json: ME });
    if (path === "/memberships") return route.fulfill({ json: { costco: true, sams: false } });
    if (path === "/searches" && request.method() === "POST") {
      const { query } = request.postDataJSON() as Schemas["StartSearchRequest"];
      api.queries.push(query);
      const id = searches.size + 1;
      searches.set(id, { query, scenario: api.scenario });
      return route.fulfill({ status: 201, json: { id } });
    }
    const polled = /^\/searches\/(\d+)$/.exec(path);
    if (polled) {
      const id = Number(polled[1]);
      api.polls.set(id, (api.polls.get(id) ?? 0) + 1);
      if (api.dropPolls) return route.abort("internetdisconnected");
      const search = searches.get(id);
      if (!search) return route.fulfill({ status: 404, json: { error: "not_found" } });
      return route.fulfill({ json: searchBody(id, search.query, search.scenario) });
    }
    if (path === "/roi") {
      const body = request.postDataJSON() as Schemas["RoiRequest"];
      api.roiBodies.push(body);
      if (api.roiFails) return route.fulfill({ status: 500, json: { error: "internal" } });
      return route.fulfill({ json: { results: body.items.map(roi) } });
    }
    if (path.endsWith("/price-history")) {
      return route.fulfill({ json: { asin: ASIN, as_of_ms: 0, since_ms: 0, hidden_listings: 0, listings: [] } });
    }
    if (path.endsWith("/price-stats")) {
      return route.fulfill({ json: { asin: ASIN, as_of_ms: 0, hidden_listings: 0, listings: [] } });
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

// Next keeps visited pages mounted but hidden, so every lookup is visible-only.
const searchbox = (page: Page) => page.getByRole("searchbox", { name: s.label }).filter({ visible: true });
const status = (page: Page) => page.getByTestId("search-status").filter({ visible: true });
const titles = (page: Page) => page.locator(".results-table tbody .row-select").filter({ visible: true });
const costsPill = (page: Page) => page.locator(".costs-pill").filter({ visible: true });
const sortHeader = (page: Page, name: string) =>
  page.getByRole("columnheader", { name }).filter({ visible: true });
const chip = (page: Page, name: string) =>
  page.getByRole("button", { name, exact: true }).filter({ visible: true });
const resultsHeading = (page: Page, query: string) =>
  page.getByRole("heading", { name: interpolate(s.resultsHeading, { query }) }).filter({ visible: true });
const rowFor = (page: Page, title: string) =>
  // The table row on desktop, the card on phones: only one of them is visible.
  page.locator(".results-table tbody tr, .result-card").filter({ visible: true, hasText: title });

async function openSearch(page: Page, path = "/es/search"): Promise<void> {
  await page.goto(path);
  await waitForHydration(page, 'input[name="query"]');
}

async function runSearch(page: Page, query: string): Promise<void> {
  await searchbox(page).fill(query);
  await page.getByRole("button", { name: s.submit, exact: true }).filter({ visible: true }).click();
}

function pill(prep: number, shipping: number): string {
  return interpolate(s.costs.pill, { prep: formatMoney(prep, "es"), shipping: formatMoney(shipping, "es") });
}

function count(shown: number, total: number): string {
  return interpolate(s.filters.count, { shown, total });
}

async function setCosts(page: Page, prep: string, shipping: string): Promise<void> {
  await costsPill(page).click();
  const editor = page.getByRole("group", { name: s.costs.heading });
  await editor.getByLabel(s.costs.prep).fill(prep);
  await editor.getByLabel(s.costs.shipping).fill(shipping);
  await editor.getByRole("button", { name: s.costs.apply }).click();
}

test("results default to profit, highest first, and re-sort from the headers", async ({ page }) => {
  await fakeApi(page);
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(resultsHeading(page, "licuadora")).toBeVisible();
  await expect(status(page)).toHaveText(s.status.done);

  await expect(titles(page)).toHaveText(BY_PROFIT);
  await expect(sortHeader(page, s.columns.profit)).toHaveAttribute("aria-sort", "descending");
  await expect(sortHeader(page, s.columns.price)).toHaveAttribute("aria-sort", "none");

  // Profit and ROI carry a sign, ↓ on a loss and the gain/loss colour.
  const alfa = rowFor(page, "Licuadora Alfa").getByTestId("result-profit");
  await expect(alfa).toHaveText(s.sign.plus + formatMoney(29_000, "es"));
  await expect(alfa.locator(".gain")).toHaveCount(1);
  const beta = rowFor(page, "Batidora Beta").getByTestId("result-profit");
  await expect(beta).toHaveText(s.sign.down + s.sign.minus + formatMoney(2_800, "es"));
  await expect(beta.locator(".loss")).toHaveCount(1);

  // The KPIs summarise the whole search.
  await expect(page.getByTestId("kpi-best-profit")).toHaveText(s.sign.plus + formatMoney(29_000, "es"));
  await expect(page.getByTestId("kpi-offers")).toHaveText("5");
  await expect(page.getByTestId("kpi-stores")).toHaveText(interpolate(s.kpi.storesValue, { withOffers: 5, total: 7 }));
  await expect(page.getByTestId("kpi-hidden")).toHaveText("2");

  // Same column again flips the direction; nulls stay last.
  await sortHeader(page, s.columns.profit).getByRole("button").click();
  await expect(sortHeader(page, s.columns.profit)).toHaveAttribute("aria-sort", "ascending");
  await expect(titles(page)).toHaveText([
    "Tostador Épsilon",
    "Batidora Beta",
    "Cafetera Gamma",
    "Licuadora Alfa",
    "Hervidor Delta",
  ]);

  // A new column starts at its natural direction: price lowest first.
  await sortHeader(page, s.columns.price).getByRole("button").click();
  await expect(sortHeader(page, s.columns.price)).toHaveAttribute("aria-sort", "ascending");
  await expect(sortHeader(page, s.columns.profit)).toHaveAttribute("aria-sort", "none");
  await expect(titles(page)).toHaveText([
    "Hervidor Delta",
    "Batidora Beta",
    "Cafetera Gamma",
    "Licuadora Alfa",
    "Tostador Épsilon",
  ]);

  await sortHeader(page, s.columns.roi).getByRole("button").click();
  await expect(sortHeader(page, s.columns.roi)).toHaveAttribute("aria-sort", "descending");
  await expect(titles(page)).toHaveText(BY_PROFIT);
  await expectAccessible(page);
});

test("filter chips toggle with aria-pressed and announce the count", async ({ page }) => {
  await fakeApi(page);
  await openSearch(page);
  await runSearch(page, "licuadora");
  const shown = page.getByRole("status").filter({ hasText: count(5, 5) });
  await expect(shown).toBeVisible();

  const matched = chip(page, s.filters.matched);
  await expect(matched).toHaveAttribute("aria-pressed", "false");
  await matched.click();
  await expect(matched).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("result-count").filter({ visible: true })).toHaveText(count(4, 5));
  await expect(titles(page)).not.toContainText(["Hervidor Delta"]);

  await chip(page, s.filters.profitable).click();
  await expect(page.getByTestId("result-count").filter({ visible: true })).toHaveText(count(2, 5));
  await expect(titles(page)).toHaveText(["Licuadora Alfa", "Cafetera Gamma"]);

  await matched.click();
  await chip(page, s.filters.profitable).click();
  await chip(page, s.filters.inStock).click();
  await expect(page.getByTestId("result-count").filter({ visible: true })).toHaveText(count(4, 5));
  await expect(titles(page)).not.toContainText(["Batidora Beta"]);
  // KPIs still describe the whole search, whatever the filters.
  await expect(page.getByTestId("kpi-offers")).toHaveText("5");

  // Filters stay on for the next search on the page.
  await runSearch(page, "batidora");
  await expect(resultsHeading(page, "batidora")).toBeVisible();
  await expect(chip(page, s.filters.inStock)).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("result-count").filter({ visible: true })).toHaveText(count(4, 5));
});

test("a selected row survives a re-sort and a filter, and the keyboard selects", async ({ page }) => {
  await fakeApi(page);
  await openSearch(page);
  await runSearch(page, "licuadora");
  const gamma = titles(page).filter({ hasText: "Cafetera Gamma" });
  await gamma.click();
  await expect(gamma).toHaveAttribute("aria-pressed", "true");
  await expect(rowFor(page, "Cafetera Gamma")).toHaveClass(/is-selected/);

  await sortHeader(page, s.columns.price).getByRole("button").click();
  await expect(gamma).toHaveAttribute("aria-pressed", "true");
  await chip(page, s.filters.profitable).click();
  await expect(gamma).toHaveAttribute("aria-pressed", "true");

  const alfa = titles(page).filter({ hasText: "Licuadora Alfa" });
  await alfa.focus();
  await page.keyboard.press("Enter");
  await expect(alfa).toHaveAttribute("aria-pressed", "true");
  await expect(gamma).toHaveAttribute("aria-pressed", "false");
  await page.keyboard.press("Space");
  await expect(alfa).toHaveAttribute("aria-pressed", "false");

  // The store link sits above the stretched title button.
  const storeLink = rowFor(page, "Licuadora Alfa").getByRole("link", { name: s.storeLink });
  await expect(storeLink).toHaveAttribute("href", "https://example.com/walmart_mx/WM-1");
  const hit = await storeLink.evaluate((el) => {
    const box = el.getBoundingClientRect();
    const top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
    return top !== null && el.contains(top);
  });
  expect(hit).toBe(true);
});

test("costs are validated, saved, sent to /roi and prefill the calculator", async ({ page }) => {
  const api = await fakeApi(page);
  await openSearch(page);
  await expect(costsPill(page)).toHaveText(pill(0, 0));

  // Invalid input keeps the editor open with an error and saves nothing.
  await costsPill(page).click();
  await expect(costsPill(page)).toHaveAttribute("aria-expanded", "true");
  const editor = page.getByRole("group", { name: s.costs.heading });
  await editor.getByLabel(s.costs.prep).fill("quince");
  await editor.getByRole("button", { name: s.costs.apply }).click();
  await expect(editor.getByRole("alert")).toHaveText(s.costs.invalid);
  await expect(editor.getByLabel(s.costs.prep)).toHaveAttribute("aria-invalid", "true");
  // Esc closes without saving and returns focus to the pill.
  await page.keyboard.press("Escape");
  await expect(costsPill(page)).toBeFocused();
  await expect(costsPill(page)).toHaveAttribute("aria-expanded", "false");
  expect(await page.evaluate((key) => localStorage.getItem(key), COSTS_KEY)).toBeNull();

  await setCosts(page, "15", "22.50");
  await expect(costsPill(page)).toBeFocused();
  await expect(costsPill(page)).toHaveText(pill(1_500, 2_250));
  expect(await page.evaluate((key) => localStorage.getItem(key), COSTS_KEY)).toBe(
    JSON.stringify({ v: 1, prep: 1_500, shipping: 2_250 }),
  );

  await page.reload();
  await waitForHydration(page, 'input[name="query"]');
  await expect(costsPill(page)).toHaveText(pill(1_500, 2_250));

  await runSearch(page, "licuadora");
  await expect(resultsHeading(page, "licuadora")).toBeVisible();
  await expect(rowFor(page, "Licuadora Alfa").getByTestId("result-profit")).toHaveText(
    s.sign.plus + formatMoney(29_000 - 3_750, "es"),
  );
  const items = api.roiBodies.flatMap((body) => body.items);
  expect(items.length).toBe(4);
  for (const item of items) {
    expect(item).toMatchObject({ prep_fee_centavos: 1_500, shipping_centavos: 2_250 });
  }

  await page.goto("/es/calculator");
  await expect(page.getByLabel(t.calculator.prep).filter({ visible: true })).toHaveValue("15");
  await expect(page.getByLabel(t.calculator.shipping).filter({ visible: true })).toHaveValue("22.50");
});

test("recent searches are saved, listed in the sidebar and rerun once", async ({ page }) => {
  const api = await fakeApi(page);
  await openSearch(page);
  const recent = page.getByRole("navigation", { name: s.recent.heading }).filter({ visible: true });
  await expect(recent).toHaveCount(0);

  await runSearch(page, "licuadora");
  await expect(resultsHeading(page, "licuadora")).toBeVisible();
  await runSearch(page, "  Batidora  ");
  await expect(resultsHeading(page, "Batidora")).toBeVisible();
  await expect(recent.getByRole("link")).toHaveText(["Batidora", "licuadora"]);
  expect(await page.evaluate((key) => localStorage.getItem(key), RECENT_KEY)).toBe(
    JSON.stringify({ v: 1, items: ["Batidora", "licuadora"] }),
  );

  await recent.getByRole("link", { name: "licuadora" }).click();
  await expect(resultsHeading(page, "licuadora")).toBeVisible();
  // The rerun parameter is dropped, so back never repeats the POST.
  await expect(page).toHaveURL("/es/search");
  await expect(searchbox(page)).toHaveValue("licuadora");
  expect(api.queries).toEqual(["licuadora", "Batidora", "licuadora"]);
  await expect(recent.getByRole("link")).toHaveText(["licuadora", "Batidora"]);

  await page.goBack();
  await expect(page).toHaveURL("/es/search");
  await expect(searchbox(page)).toBeVisible();
  expect(api.queries).toHaveLength(3);
});

for (const [name, costs, recent] of [
  ["not JSON", "{prep: 15", "[oops"],
  ["wrong shape", JSON.stringify({ v: 2, prep: 1_500, shipping: 0 }), JSON.stringify({ v: 1, items: [42] })],
  ["negative and fractional centavos", JSON.stringify({ v: 1, prep: -5, shipping: 1.5 }), JSON.stringify(["x"])],
] as const) {
  test(`malformed storage is ignored (${name})`, async ({ page }) => {
    const api = await fakeApi(page);
    await page.addInitScript(
      ([costsKey, costsValue, recentKey, recentValue]) => {
        localStorage.setItem(costsKey, costsValue);
        localStorage.setItem(recentKey, recentValue);
      },
      [COSTS_KEY, costs, RECENT_KEY, recent] as const,
    );
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await openSearch(page);
    await expect(costsPill(page)).toHaveText(pill(0, 0));
    await expect(page.getByRole("navigation", { name: s.recent.heading }).filter({ visible: true })).toHaveCount(0);
    await runSearch(page, "licuadora");
    await expect(resultsHeading(page, "licuadora")).toBeVisible();
    await expect(rowFor(page, "Licuadora Alfa").getByTestId("result-profit")).toHaveText(
      s.sign.plus + formatMoney(29_000, "es"),
    );
    expect(api.roiBodies.flatMap((b) => b.items).every((i) => i.prep_fee_centavos === 0)).toBe(true);
    expect(errors).toEqual([]);
  });
}

test("/ focuses the search field unless typing in a field", async ({ page }) => {
  await fakeApi(page);
  await openSearch(page);
  await page.getByRole("heading", { level: 1, name: s.heading }).filter({ visible: true }).click();
  await page.keyboard.press("/");
  await expect(searchbox(page)).toBeFocused();
  await page.keyboard.type("a/b");
  await expect(searchbox(page)).toHaveValue("a/b");
});

test("queued and running searches show progress, then per-store outcomes", async ({ page }) => {
  const api = await fakeApi(page, "queued");
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(status(page)).toHaveText(s.status.queued);
  await expect(page.locator('.kpi-strip[aria-hidden="true"]').filter({ visible: true })).toBeVisible();

  api.scenario = "slow";
  await runSearch(page, "cafetera");
  await expect(status(page)).toHaveText(s.status.running);

  api.scenario = "stores";
  await runSearch(page, "tostador");
  await expect(status(page)).toHaveText(interpolate(s.status.runningStores, { count: 7 }));
  const outcomes = page.getByRole("list", { name: s.outcomesHeading }).filter({ visible: true });
  await expect(outcomes.getByRole("listitem")).toHaveCount(7);
  await expect(outcomes).toContainText(interpolate(s.outcomeLine, { retailer: t.retailers.walmart_mx, detail: s.offersOne }));
  await expectAccessible(page);
});

test("a partial search shows its results and which stores failed", async ({ page }) => {
  await fakeApi(page, "partial");
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(status(page)).toHaveText(s.status.partial);
  await expect(page.getByText(s.reasons.retailers_failed)).toBeVisible();
  await expect(page.getByRole("list", { name: s.outcomesHeading }).filter({ visible: true })).toContainText(
    interpolate(s.outcomeLine, { retailer: t.retailers.sears_mx, detail: s.outcomes.failed }),
  );
  await expect(titles(page)).toHaveCount(5);
});

test("a failed search explains why and retries the same query", async ({ page }) => {
  const api = await fakeApi(page, "failed");
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(status(page)).toHaveText(s.status.failed);
  await expect(page.getByText(s.reasons.all_retailers_failed)).toBeVisible();
  await expect(page.locator(".kpi-strip").filter({ visible: true })).toHaveCount(0);
  await expectAccessible(page);

  api.scenario = "done";
  await page.getByRole("button", { name: t.common.retry }).filter({ visible: true }).click();
  await expect(resultsHeading(page, "licuadora")).toBeVisible();
  expect(api.queries).toEqual(["licuadora", "licuadora"]);
});

test("a refused search suggests other words, without a retry", async ({ page }) => {
  await fakeApi(page, "refused");
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(status(page)).toHaveText(s.status.refused);
  await expect(page.getByText(s.reasons.refused)).toBeVisible();
  await expect(page.getByText(s.next.refused)).toBeVisible();
  await expect(page.getByRole("button", { name: t.common.retry }).filter({ visible: true })).toHaveCount(0);
});

test("no offers and all-hidden searches each offer a next step", async ({ page }) => {
  const api = await fakeApi(page, "empty");
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(status(page)).toHaveText(s.status.empty);
  await expect(page.getByText(s.next.empty)).toBeVisible();
  await expectAccessible(page);

  api.scenario = "allhidden";
  await runSearch(page, "freidora");
  await expect(status(page)).toHaveText(s.status.allHidden);
  await expect(page.getByText(s.next.allHidden)).toBeVisible();
  await expect(page.getByRole("link", { name: s.next.memberships })).toHaveAttribute("href", "/es/onboarding");
  await expectAccessible(page);
});

test("a failed ROI calculation shows an error and recalculates", async ({ page }) => {
  const api = await fakeApi(page);
  api.roiFails = true;
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(page.getByTestId("roi-error").filter({ visible: true })).toBeVisible();
  await expect(rowFor(page, "Licuadora Alfa").getByTestId("result-profit")).toHaveText(t.common.none);

  api.roiFails = false;
  await page.getByRole("button", { name: s.roiRetry }).filter({ visible: true }).click();
  await expect(rowFor(page, "Licuadora Alfa").getByTestId("result-profit")).toHaveText(
    s.sign.plus + formatMoney(29_000, "es"),
  );
  await expect(page.getByTestId("roi-error").filter({ visible: true })).toHaveCount(0);
});

test("a search that runs too long times out with a retry", async ({ page }) => {
  await page.clock.install();
  await fakeApi(page, "slow");
  await openSearch(page);
  await runSearch(page, "licuadora");
  await expect(status(page)).toHaveText(s.status.running);
  await page.clock.fastForward("03:05");
  await expect(status(page)).toHaveText(s.status.timeout);
  await expect(page.getByText(s.next.timeout)).toBeVisible();
  await expect(page.getByRole("button", { name: t.common.retry }).filter({ visible: true })).toBeVisible();
});

// Journey "search a product → compare → pick one": its recovery and
// back-or-cancel paths run on the desktop and the phone layout.
for (const [layout, viewport] of [
  ["desktop", { width: 1440, height: 900 }],
  ["phone", { width: 390, height: 844 }],
] as const) {
  test.describe(`journey on ${layout}`, () => {
    test.use({ viewport });

    test("a dropped connection mid-poll retries with the same query and costs", async ({ page }) => {
      const api = await fakeApi(page, "stores");
      await openSearch(page);
      await setCosts(page, "15", "0");
      await runSearch(page, "licuadora");
      await expect(status(page)).toHaveText(interpolate(s.status.runningStores, { count: 7 }));

      api.dropPolls = true;
      await expect(status(page)).toHaveText(s.status.network);
      await expect(page.getByText(s.next.network)).toBeVisible();
      await expect(page.getByRole("alert").filter({ hasText: t.common.errors.network })).toBeVisible();
      await expectAccessible(page);

      api.dropPolls = false;
      api.scenario = "done";
      await page.getByRole("button", { name: t.common.retry }).filter({ visible: true }).click();
      await expect(resultsHeading(page, "licuadora")).toBeVisible();
      expect(api.queries).toEqual(["licuadora", "licuadora"]);
      await expect(searchbox(page)).toHaveValue("licuadora");
      await expect(costsPill(page)).toHaveText(pill(1_500, 0));
      expect(api.roiBodies.flatMap((b) => b.items).every((i) => i.prep_fee_centavos === 1_500)).toBe(true);
    });

    test("a new search mid-poll replaces the old one", async ({ page }) => {
      const api = await fakeApi(page, "slow");
      await openSearch(page);
      await runSearch(page, "lento");
      await expect(status(page)).toHaveText(s.status.running);
      await expect.poll(() => api.polls.get(1) ?? 0).toBeGreaterThan(1);

      // Buscar is blocked only for the query already running.
      await expect(page.getByRole("button", { name: s.submit, exact: true }).filter({ visible: true })).toBeDisabled();
      api.scenario = "done";
      await runSearch(page, "licuadora");
      await expect(resultsHeading(page, "licuadora")).toBeVisible();
      const oldPolls = api.polls.get(1) ?? 0;
      // Two poll intervals later the old search is still silent and nothing reverted.
      await page.waitForTimeout(3_500);
      expect(api.polls.get(1) ?? 0).toBe(oldPolls);
      await expect(status(page)).toHaveText(s.status.done);
      await expect(resultsHeading(page, "licuadora")).toBeVisible();
    });

    test("browser back from a product returns to the search without searching again", async ({ page }) => {
      const api = await fakeApi(page);
      await openSearch(page);
      await runSearch(page, "licuadora");
      await rowFor(page, "Licuadora Alfa")
        .getByRole("link", { name: interpolate(s.match.history, { asin: ASIN }) })
        .click();
      await expect(page).toHaveURL(`/es/products/${ASIN}`);

      await page.goBack();
      await expect(page).toHaveURL("/es/search");
      await expect(searchbox(page)).toBeVisible();
      expect(api.queries).toEqual(["licuadora"]);
    });
  });
}

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

  test("results become labelled cards without sideways scroll; recent searches rerun", async ({ page }) => {
    const api = await fakeApi(page);
    await page.addInitScript(
      ([key, value]) => {
        if (localStorage.getItem(key) === null) localStorage.setItem(key, value);
      },
      [RECENT_KEY, JSON.stringify({ v: 1, items: ["licuadora"] })] as const,
    );
    await openSearch(page);
    const recent = page.locator(".recent-searches--phone");
    await expect(recent).toBeVisible();
    await recent.getByRole("link", { name: "licuadora" }).click();
    await expect(resultsHeading(page, "licuadora")).toBeVisible();
    await expect(page).toHaveURL("/es/search");
    expect(api.queries).toEqual(["licuadora"]);

    await expect(page.locator(".results-table")).toBeHidden();
    const cards = page.locator(".result-card").filter({ visible: true });
    await expect(cards).toHaveCount(5);
    const first = cards.first();
    await expect(first.locator(".row-select")).toHaveText("Licuadora Alfa");
    await expect(first.locator("dt")).toHaveText([
      s.columns.profit,
      s.columns.roi,
      s.columns.price,
      s.columns.availability,
      s.columns.amazon,
    ]);
    await expect(first.getByTestId("result-profit")).toHaveText(s.sign.plus + formatMoney(29_000, "es"));
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    await expectAccessible(page);
  });
});
