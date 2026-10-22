import { expect, test, type Locator, type Page } from "@playwright/test";

import { interpolate } from "../src/i18n/format";
import type { components } from "../src/lib/api/schema";
import { ASIN, account, catalogs, expectAccessible, tabTo } from "./helpers";

// The app frame (docs/modules/web/app-shell.md): session links, current page,
// skip link, phone layout and the language switch. Every API call is answered
// by `page.route`, so these run without the seeded backend.

const t = catalogs.es;
const DESKTOP = { width: 1440, height: 900 };
const PHONE = { width: 390, height: 844 };

const ME = {
  id: 1,
  email: account.email,
  locale: "es",
  plan: "free",
  onboarded: true,
  has_costco: true,
  has_sams: false,
} satisfies components["schemas"]["MeResponse"];

const SEARCH = {
  id: 7,
  query: "taladro",
  status: "done",
  created_at_ms: 0,
  finished_at_ms: 0,
  hidden_results: 0,
  reason: null,
  retailers: [{ retailer: "costco", outcome: "ok", offers: 1 }],
  results: [
    {
      retailer: "costco",
      retailer_sku: "SKU-1",
      title: "Taladro inalámbrico de prueba",
      url: "https://example.com/taladro",
      price_centavos: 129_900,
      availability: "in_stock",
      membership_required: false,
      amazon: {
        asin: ASIN,
        match_status: "accepted",
        confidence_bp: 9_500,
        amazon_price_centavos: 219_900,
        fees_centavos: 35_000,
        profit_centavos: 49_500,
        roi_bp: 2_905,
        margin_bp: 2_251,
      },
    },
  ],
} satisfies components["schemas"]["SearchResponse"];

type FakeApi = {
  localeSaves: string[];
  releaseMe: () => void;
  expireSession: () => void;
  restoreMe: () => void;
};

/**
 * Fakes the api. `signedIn` sets the starting session; `holdMe` leaves
 * `GET /me` unanswered until `releaseMe()` (then it answers normally);
 * `signInFailures` answers that many sign-ins with 503 before one succeeds;
 * `meUnavailable` answers `GET /me` with 503 until `restoreMe()`;
 * `signOutUnavailable` answers sign-out with 503 and keeps the session.
 * `expireSession()` ends the session on the server side only, so every
 * later call answers 401.
 */
async function fakeApi(
  page: Page,
  opts: {
    signedIn: boolean;
    holdMe?: boolean;
    signInFailures?: number;
    meUnavailable?: boolean;
    signOutUnavailable?: boolean;
  },
): Promise<FakeApi> {
  let signedIn = opts.signedIn;
  let meUnavailable = opts.meUnavailable ?? false;
  let signInFailures = opts.signInFailures ?? 0;
  let release: () => void = () => {};
  const held = opts.holdMe
    ? new Promise<void>((resolve) => {
        release = resolve;
      })
    : Promise.resolve();
  const localeSaves: string[] = [];
  await page.route("**/backend/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace(/^\/backend/, "");
    const unauthorized = { status: 401, json: { error: "unauthorized" } };
    if (path === "/auth/csrf") return route.fulfill({ json: { csrf_token: "test-token" } });
    if (path === "/auth/signin") {
      if (signInFailures > 0) {
        signInFailures -= 1;
        return route.fulfill({ status: 503, json: { error: "unavailable" } });
      }
      signedIn = true;
      return route.fulfill({ json: { csrf_token: "test-token", user: ME } });
    }
    if (path === "/auth/signout") {
      if (opts.signOutUnavailable) {
        return route.fulfill({ status: 503, json: { error: "unavailable" } });
      }
      signedIn = false;
      return route.fulfill({ status: 204 });
    }
    if (path === "/me") {
      await held;
      if (meUnavailable) return route.fulfill({ status: 503, json: { error: "unavailable" } });
      return route.fulfill(signedIn ? { json: ME } : unauthorized);
    }
    if (path === "/me/locale") {
      const body: unknown = request.postDataJSON();
      if (typeof body === "object" && body !== null && "locale" in body) {
        localeSaves.push(String(body.locale));
      }
      return route.fulfill(signedIn ? { status: 204 } : unauthorized);
    }
    if (!signedIn) return route.fulfill(unauthorized);
    if (path === "/searches") return route.fulfill({ json: { id: SEARCH.id } });
    if (path === `/searches/${SEARCH.id}`) return route.fulfill({ json: SEARCH });
    if (path.endsWith("/price-history")) {
      return route.fulfill({
        json: { asin: ASIN, as_of_ms: 0, since_ms: 0, hidden_listings: 0, listings: [] },
      });
    }
    if (path.endsWith("/price-stats")) {
      return route.fulfill({ json: { asin: ASIN, as_of_ms: 0, hidden_listings: 0, listings: [] } });
    }
    if (path === "/roi") {
      return route.fulfill({
        json: { results: [{ profit_centavos: 49_500, roi_bp: 2_905, margin_bp: 2_251 }] },
      });
    }
    return route.fulfill({ status: 404, json: { error: "not_found" } });
  });
  return {
    localeSaves,
    releaseMe: () => release(),
    expireSession: () => {
      signedIn = false;
    },
    restoreMe: () => {
      meUnavailable = false;
    },
  };
}

/** Visible links named `name` (Next keeps visited pages mounted but hidden). */
function links(page: Page, name: string): Locator {
  return page.getByRole("link", { name, exact: true }).filter({ visible: true });
}

function signOutButton(page: Page): Locator {
  return page.getByRole("button", { name: t.nav.signOut, exact: true }).filter({ visible: true });
}

/** The visible primary navs and the label of each one's `aria-current` item. */
async function currentItems(page: Page, label = t.nav.label): Promise<(string | null)[]> {
  const navs = page.getByRole("navigation", { name: label }).filter({ visible: true });
  await expect(navs.first()).toBeVisible();
  const result: (string | null)[] = [];
  for (const nav of await navs.all()) {
    const current = nav.locator('a[aria-current="page"]');
    const count = await current.count();
    expect(count, "at most one current item per nav").toBeLessThanOrEqual(1);
    result.push(count === 1 ? await current.innerText() : null);
    // Inactive items omit the attribute (never aria-current="false").
    await expect(nav.locator('a[aria-current]:not([aria-current="page"])')).toHaveCount(0);
  }
  return result;
}

async function expectSignedOutLinks(page: Page): Promise<void> {
  await expect(links(page, t.nav.signIn).first()).toBeVisible();
  await expect(links(page, t.nav.signUp).first()).toBeVisible();
  await expect(links(page, t.nav.account)).toHaveCount(0);
  await expect(signOutButton(page)).toHaveCount(0);
}

test.describe("desktop frame", () => {
  test.use({ viewport: DESKTOP });

  test("signed out: sign-in and sign-up only", async ({ page }) => {
    await fakeApi(page, { signedIn: false });
    await page.goto("/es");
    await expectSignedOutLinks(page);
    // Public pages have no app sidebar.
    await expect(page.getByRole("navigation", { name: t.nav.label })).toHaveCount(0);
    await expectAccessible(page);

    await page.goto("/es/calculator");
    await expectSignedOutLinks(page);
    // Sign-in from the frame comes back to this page.
    await expect(links(page, t.nav.signIn)).toHaveAttribute(
      "href",
      `/es/signin?next=${encodeURIComponent("/es/calculator")}`,
    );
    await expectAccessible(page);
  });

  test("signed in: account and sign-out only", async ({ page }) => {
    await fakeApi(page, { signedIn: true });
    await page.goto("/es/calculator");
    await expect(page.getByText(account.email, { exact: true })).toBeVisible();
    await expect(page.getByText(t.account.plans.free, { exact: true })).toBeVisible();
    await expect(links(page, t.nav.account)).toBeVisible();
    await expect(signOutButton(page)).toBeVisible();
    await expect(links(page, t.nav.signIn)).toHaveCount(0);
    await expect(links(page, t.nav.signUp)).toHaveCount(0);
    await expectAccessible(page);
  });

  test("while the session loads, no session links show", async ({ page }) => {
    const api = await fakeApi(page, { signedIn: false, holdMe: true });
    await page.goto("/es/calculator");
    await expect(page.locator(".account-menu--placeholder").filter({ visible: true })).toHaveCount(1);
    for (const name of [t.nav.signIn, t.nav.signUp, t.nav.account]) {
      await expect(links(page, name)).toHaveCount(0);
    }
    await expect(signOutButton(page)).toHaveCount(0);
    await expectAccessible(page);

    api.releaseMe();
    await expectSignedOutLinks(page);
    await expect(page.locator(".account-menu--placeholder").filter({ visible: true })).toHaveCount(0);
  });

  test("a session that expires mid-use signs the frame out too", async ({ page }) => {
    const api = await fakeApi(page, { signedIn: true });
    await page.goto("/es/search");
    await expect(signOutButton(page)).toBeVisible();

    api.expireSession();
    await page.getByLabel(t.search.label).filter({ visible: true }).fill(SEARCH.query);
    await page
      .getByRole("button", { name: t.search.submit, exact: true })
      .filter({ visible: true })
      .click();
    await expect(page).toHaveURL(`/es/signin?next=${encodeURIComponent("/es/search")}`);
    // No stale "Cuenta" / "Cerrar sesión" in the public header.
    await expectSignedOutLinks(page);
    await expectAccessible(page);
  });

  test("a page retry after a failed session read restores the frame", async ({ page }) => {
    const api = await fakeApi(page, { signedIn: true, meUnavailable: true });
    await page.goto("/es/search");
    const main = page.locator("main#main").filter({ visible: true });
    await expect(main.getByRole("alert")).toHaveText(t.common.errors.unavailable);
    await expect(page.locator(".account-menu--placeholder").filter({ visible: true })).toHaveCount(1);
    await expect(signOutButton(page)).toHaveCount(0);

    api.restoreMe();
    await main.getByRole("button", { name: t.common.retry, exact: true }).click();
    await expect(page.getByLabel(t.search.label).filter({ visible: true })).toBeVisible();
    // The frame re-read the session with the page, without a reload.
    await expect(signOutButton(page)).toBeVisible();
    await expect(links(page, t.nav.account)).toBeVisible();
    await expectAccessible(page);
  });

  test("a failed sign-out stays on the page and says so", async ({ page }) => {
    await fakeApi(page, { signedIn: true, signOutUnavailable: true });
    await page.goto("/es/search");
    await signOutButton(page).click();
    // The session is still alive: no trip to sign-in, and the error shows.
    await expect(page.locator(".sidebar").getByRole("alert")).toHaveText(
      t.common.errors.unavailable,
    );
    await expect(page).toHaveURL("/es/search");
    await expect(signOutButton(page)).toBeVisible();
    await expect(signOutButton(page)).toBeEnabled();
    await expect(links(page, t.nav.account)).toBeVisible();
    await expectAccessible(page);
  });

  test("the skip link is the first stop and lands on main", async ({ page }) => {
    await fakeApi(page, { signedIn: true });
    await page.goto("/es/calculator");
    await expect(signOutButton(page)).toBeVisible();
    await page.keyboard.press("Tab");
    await expect(page.getByRole("link", { name: t.nav.skipToContent })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("main#main").filter({ visible: true })).toBeFocused();
  });

  test("tab order follows the DOM order", async ({ page }) => {
    await fakeApi(page, { signedIn: true });
    await page.goto("/es/calculator");
    await expect(signOutButton(page)).toBeVisible();
    // Number every visible, tabbable element in DOM order.
    const count = await page.evaluate(() => {
      const selector =
        'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
      const tabbable = [...document.body.querySelectorAll<HTMLElement>(selector)].filter(
        (el) => el.checkVisibility({ checkVisibilityCSS: true }) && el.tabIndex >= 0,
      );
      tabbable.forEach((el, index) => {
        el.dataset.e2eOrder = String(index);
      });
      return tabbable.length;
    });
    expect(count).toBeGreaterThan(5);
    for (let index = 0; index < count; index += 1) {
      await page.keyboard.press("Tab");
      const order = await page.evaluate(
        () => (document.activeElement as HTMLElement | null)?.dataset.e2eOrder ?? null,
      );
      expect(order, `Tab stop ${index + 1}`).toBe(String(index));
    }
  });

  test("the current page is marked in the sidebar", async ({ page }) => {
    await fakeApi(page, { signedIn: true });
    for (const [path, label] of [
      ["/es/search", t.nav.search],
      ["/es/calculator", t.nav.calculator],
      ["/es/assistant", t.nav.assistant],
      [`/es/products/${ASIN}`, t.nav.search],
    ] as const) {
      await page.goto(path);
      expect(await currentItems(page), path).toEqual([label]);
      await expectAccessible(page);
    }
    // The account page has no sidebar item: nothing is marked there, but the
    // footer's account link is.
    await page.goto("/es/account");
    expect(await currentItems(page)).toEqual([null]);
    await expect(links(page, t.nav.account)).toHaveAttribute("aria-current", "page");
    await expectAccessible(page);
  });

  test("the language switch keeps the page and saves the choice", async ({ page }) => {
    const api = await fakeApi(page, { signedIn: true });
    await page.goto("/es/calculator");
    await expect(signOutButton(page)).toBeVisible();
    await tabTo(page, links(page, t.nav.switchLanguage));
    await page.keyboard.press("Enter");
    await page.waitForURL("/en/calculator");
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect.poll(() => api.localeSaves).toEqual(["en"]);
    expect(await currentItems(page, catalogs.en.nav.label)).toEqual([catalogs.en.nav.calculator]);
    await expectAccessible(page);
  });
});

test.describe("phone frame", () => {
  test.use({ viewport: PHONE, hasTouch: true, isMobile: true });

  test("bottom tabs, no sidebar, and only the workspace scrolls", async ({ page }) => {
    await fakeApi(page, { signedIn: true });
    await page.goto("/es/calculator");
    await expect(page.locator(".bottom-tabs")).toBeVisible();
    await expect(page.locator(".top-bar")).toBeVisible();
    await expect(page.locator(".sidebar")).toBeHidden();
    expect(await currentItems(page)).toEqual([t.nav.calculator]);

    const sizes = await page.evaluate(() => ({
      doc: document.documentElement.scrollHeight,
      viewport: window.innerHeight,
    }));
    expect(sizes.doc).toBeLessThanOrEqual(sizes.viewport);

    const workspace = page.locator(".workspace").filter({ visible: true });
    await expect(workspace).toHaveCSS("overflow-y", "auto");
    await expect(workspace).toHaveCSS("overscroll-behavior-y", "contain");
    const scrolled = await workspace.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
      return { workspace: el.scrollTop, window: window.scrollY };
    });
    expect(scrolled.workspace).toBeGreaterThan(0);
    expect(scrolled.window).toBe(0);
    await expectAccessible(page);
  });

  test("the Cuenta tab shows the account, language and sign-out", async ({ page }) => {
    const api = await fakeApi(page, { signedIn: true });
    // Invalid nesting (e.g. the sign-out alert inside a <p>) breaks hydration.
    const nesting: string[] = [];
    page.on("console", (message) => {
      if (message.text().includes("cannot be a descendant of")) nesting.push(message.text());
    });
    await page.goto("/es/search");
    await links(page, t.nav.account).click();
    await expect(page).toHaveURL("/es/account");
    expect(await currentItems(page)).toEqual([t.nav.account]);
    await expect(page.getByRole("heading", { name: t.account.heading })).toBeVisible();
    const main = page.locator("main#main").filter({ visible: true });
    await expect(main.getByText(account.email, { exact: true })).toBeVisible();
    await expect(main.getByText(t.account.plans.free, { exact: true })).toBeVisible();
    await expectAccessible(page);

    // The top bar also has the language switch; both keep the page.
    await expect(links(page, t.nav.switchLanguage)).toHaveCount(2);

    await signOutButton(page).click();
    await expect(page).toHaveURL("/es/signin");
    await expectSignedOutLinks(page);
    expect(api.localeSaves).toEqual([]);
    expect(nesting).toEqual([]);
    await expectAccessible(page);
  });
});

// Methodology 14 journey: sign in → search → product → back, at both sizes.
for (const [name, viewport] of [
  ["desktop", DESKTOP],
  ["phone", PHONE],
] as const) {
  test(`journey: sign in → search → product → back (${name})`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await fakeApi(page, { signedIn: false });
    await page.goto("/es/search");
    await expect(page).toHaveURL(`/es/signin?next=${encodeURIComponent("/es/search")}`);
    await expectSignedOutLinks(page);
    await expectAccessible(page);

    const form = page.locator("form").filter({ visible: true });
    await form.getByLabel(t.auth.email).fill(account.email);
    await form.getByLabel(t.auth.password).fill(account.password);
    await form.getByRole("button", { name: t.auth.signInSubmit }).click();
    await expect(page).toHaveURL("/es/search");
    expect(await currentItems(page)).toEqual([t.nav.search]);
    if (name === "desktop") await expect(signOutButton(page)).toBeVisible();
    else await expect(links(page, t.nav.account)).toBeVisible();
    await expect(links(page, t.nav.signIn)).toHaveCount(0);

    await page.getByLabel(t.search.label).filter({ visible: true }).fill(SEARCH.query);
    await page
      .getByRole("button", { name: t.search.submit, exact: true })
      .filter({ visible: true })
      .click();
    const toProduct = links(page, interpolate(t.search.match.history, { asin: ASIN }));
    await expect(toProduct).toBeVisible();
    await expectAccessible(page);

    await toProduct.click();
    await expect(page).toHaveURL(`/es/products/${ASIN}`);
    await expect(
      page.getByRole("heading", { name: interpolate(t.product.heading, { asin: ASIN }) }),
    ).toBeVisible();
    expect(await currentItems(page)).toEqual([t.nav.search]);
    await expectAccessible(page);

    await page.goBack();
    await expect(page).toHaveURL("/es/search");
    expect(await currentItems(page)).toEqual([t.nav.search]);
    // Back keeps the results the user left.
    await expect(toProduct).toBeVisible();
    await expectAccessible(page);
  });

  test(`journey failure: sign-in fails, retry reaches search (${name})`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await fakeApi(page, { signedIn: false, signInFailures: 1 });
    await page.goto("/es/search");
    await expect(page).toHaveURL(`/es/signin?next=${encodeURIComponent("/es/search")}`);

    const form = page.locator("form").filter({ visible: true });
    await form.getByLabel(t.auth.email).fill(account.email);
    await form.getByLabel(t.auth.password).fill(account.password);
    const submit = form.getByRole("button", { name: t.auth.signInSubmit });
    await submit.click();
    await expect(form.getByRole("alert")).toHaveText(t.common.errors.unavailable);
    // Still signed out, on the same page, with the typed values kept.
    await expect(page).toHaveURL(`/es/signin?next=${encodeURIComponent("/es/search")}`);
    await expect(form.getByLabel(t.auth.email)).toHaveValue(account.email);
    await expectSignedOutLinks(page);
    await expectAccessible(page);

    await submit.click();
    await expect(page).toHaveURL("/es/search");
    expect(await currentItems(page)).toEqual([t.nav.search]);
    await expect(links(page, t.nav.signIn)).toHaveCount(0);
  });
}
