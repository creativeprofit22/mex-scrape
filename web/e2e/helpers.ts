import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";

import AxeBuilder from "@axe-core/playwright";
import { expect, type Locator, type Page } from "@playwright/test";

import en from "../src/i18n/messages/en.json";
import es from "../src/i18n/messages/es.json";

const run = promisify(execFile);

/** Catalogs by language, so assertions follow the UI text. */
export const catalogs = { es, en } as const;
export type Lang = keyof typeof catalogs;

/** The account `seed_e2e seed` creates. */
export const account = {
  email: process.env.E2E_EMAIL ?? "e2e@example.com",
  password: process.env.E2E_PASSWORD ?? "e2e-password-123",
};

/** The seeded account that has not finished onboarding (same password). */
export const pendingAccount = {
  email: process.env.E2E_PENDING_EMAIL ?? "e2e-pending@example.com",
  password: account.password,
};

/** Product with seeded price history and search matches. */
export const ASIN = "B0E2ETEST1";

/**
 * Moves focus with Tab until `target` has it (keyboard-only navigation).
 * Fails if it is not reached within `max` presses.
 */
export async function tabTo(page: Page, target: Locator, max = 80): Promise<void> {
  for (let i = 0; i < max; i += 1) {
    await page.keyboard.press("Tab");
    if (await target.evaluate((el) => el === document.activeElement)) return;
  }
  throw new Error(`Tab never reached ${target.toString()}`);
}

/** Runs axe (WCAG 2.1 A/AA) on the current page and expects no violations. */
export async function expectAccessible(page: Page): Promise<void> {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(results.violations, page.url()).toEqual([]);
}

/** Signs in with the keyboard; lands on the account's saved language. */
export async function signIn(page: Page, lang: Lang, who = account): Promise<void> {
  const t = catalogs[lang];
  await page.goto(`/${lang}/signin`);
  await expect(page.locator("html")).toHaveAttribute("lang", lang);
  await expectAccessible(page);
  await tabTo(page, page.getByLabel(t.auth.email));
  await page.keyboard.type(who.email);
  await tabTo(page, page.getByLabel(t.auth.password));
  await page.keyboard.type(who.password);
  await page.keyboard.press("Enter");
  await page.waitForURL(/\/(es|en)\/(search|onboarding)$/);
}

/**
 * Makes `lang` the current language with the frame's toggle (keyboard; the
 * sidebar footer at desktop sizes), which also saves it on the account.
 */
export async function useLanguage(page: Page, lang: Lang): Promise<void> {
  const current = new URL(page.url()).pathname.split("/")[1] as Lang;
  if (current === lang) return;
  const toggle = page.getByRole("link", {
    name: catalogs[current].nav.switchLanguage,
    exact: true,
  });
  await tabTo(page, toggle);
  await page.keyboard.press("Enter");
  await page.waitForURL(new RegExp(`/${lang}(/|$)`));
  await expect(page.locator("html")).toHaveAttribute("lang", lang);
}

/**
 * Completes the account's pending searches with fixture results
 * (`seed_e2e complete`). `E2E_SEED_BIN` points at a prebuilt binary;
 * otherwise it goes through `cargo run`.
 */
export async function completeSearches(): Promise<void> {
  await seedE2e("complete");
}

/**
 * Deletes the seeded accounts' match decisions, suggestions and assistant
 * usage (`seed_e2e reset-assistant`), restoring the needs_review fixture.
 */
export async function resetAssistant(): Promise<void> {
  await seedE2e("reset-assistant");
}

async function seedE2e(command: "complete" | "reset-assistant"): Promise<void> {
  const bin = process.env.E2E_SEED_BIN;
  const [file, args] = bin
    ? [bin, [command]]
    : ["cargo", ["run", "-q", "--locked", "-p", "db", "--bin", "seed_e2e", "--", command]];
  await run(file, args, {
    cwd: path.resolve(__dirname, "..", ".."),
    env: {
      ...process.env,
      DATABASE_URL:
        process.env.DATABASE_URL ?? "postgres://mex:mex@127.0.0.1:5432/mex_scrape",
    },
  });
}
