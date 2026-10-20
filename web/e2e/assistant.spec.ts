import { expect, test, type Page } from "@playwright/test";

import { interpolate } from "../src/i18n/format";
import {
  buildChatHistory,
  MAX_HISTORY_CHARS,
  MAX_MESSAGE_CHARS,
  MAX_TURNS,
  type ChatMessage,
} from "../src/lib/chatHistory";
import {
  ASIN,
  catalogs,
  completeSearches,
  expectAccessible,
  resetAssistant,
  signIn,
  useLanguage,
} from "./helpers";

// The sidecar runs the scripted fake model (services/agent/src/fake-provider.ts):
// a question naming an ASIN calls get_price_history and answers with the
// lowest 365-day price; a tie-break request calls search_offers and proposes
// accepting the first needs_review match (the seeded Costco listing).

/** Formats centavos like the scripted model: `$1,234.50`. */
function pesos(centavos: number): string {
  const [whole, frac] = (centavos / 100).toFixed(2).split(".");
  return `$${whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}.${frac}`;
}

/** The 365-day low across the product's listings, read from the public api. */
async function yearLow(page: Page): Promise<number> {
  const stats = await page.evaluate(async (asin) => {
    const response = await fetch(`/backend/products/${asin}/price-stats`, {
      credentials: "same-origin",
    });
    return (await response.json()) as {
      listings: { windows: { days: number; low_centavos?: number | null }[] }[];
    };
  }, ASIN);
  const lows = stats.listings.flatMap((l) =>
    l.windows.filter((w) => w.days === 365 && typeof w.low_centavos === "number").map((w) => w.low_centavos as number),
  );
  expect(lows.length).toBeGreaterThan(0);
  return Math.min(...lows);
}

async function runSearch(page: Page, query: string): Promise<void> {
  const t = catalogs.es;
  await page.goto("/es/search");
  await page.getByLabel(t.search.label).fill(query);
  await page.getByLabel(t.search.label).press("Enter");
  const heading = page.getByRole("heading", {
    name: interpolate(t.search.resultsHeading, { query }),
  });
  await expect(async () => {
    await completeSearches();
    await expect(heading).toBeVisible({ timeout: 3_000 });
  }).toPass({ timeout: 60_000 });
}

test.beforeAll(async () => {
  await resetAssistant();
});
test.afterAll(async () => {
  // Leave the needs_review fixture for the other specs and the next run.
  await resetAssistant();
});

test("assistant: season prep answer and a confirmed tie-break (es)", async ({ page }) => {
  const t = catalogs.es;
  await signIn(page, "es");
  await useLanguage(page, "es");

  // The assistant only sees listings from the user's own searches.
  await runSearch(page, "taladro");
  const costco = page.getByRole("row", { name: /E2E-COS-1/ });
  await expect(costco.getByText(t.search.match.needsReview)).toBeVisible();

  await page.goto("/es/assistant");
  await expect(page.getByRole("heading", { name: t.assistant.heading })).toBeVisible();
  await expectAccessible(page);

  // Season prep: the figure in the answer is the tool's 365-day low.
  const low = pesos(await yearLow(page));
  const input = page.getByLabel(t.assistant.label);
  await input.fill(`¿Cuándo conviene comprar ${ASIN} para el Buen Fin?`);
  await input.press("Enter");
  const log = page.getByTestId("chat-log");
  await expect(log.locator(".chat-bot").last()).toContainText(low, { timeout: 30_000 });
  await expect(log.locator(".chat-bot").last()).toContainText("Buen Fin");

  // Tie-break: the agent proposes, nothing changes until the user confirms.
  await input.fill("Ayúdame a desempatar las coincidencias por revisar");
  await input.press("Enter");
  const card = page.getByTestId("suggestion").filter({ hasText: "E2E-COS-1" });
  await expect(card).toBeVisible({ timeout: 30_000 });
  await expect(card.getByText(t.assistant.proposeAccept)).toBeVisible();
  await expectAccessible(page);
  await card.getByRole("button", { name: new RegExp(`^${t.assistant.confirm}`) }).click();
  await expect(card.getByRole("status")).toHaveText(t.assistant.accepted);

  // The user's search results now show the match as accepted.
  await runSearch(page, "taladro");
  await expect(
    page.getByRole("row", { name: /E2E-COS-1/ }).getByText(t.search.match.accepted),
  ).toBeVisible();
});

for (const lang of ["es", "en"] as const) {
  test(`assistant page is accessible (${lang})`, async ({ page }) => {
    await signIn(page, lang);
    await useLanguage(page, lang);
    await page.goto(`/${lang}/assistant`);
    await expect(page.locator("html")).toHaveAttribute("lang", lang);
    await expect(page.getByRole("heading", { name: catalogs[lang].assistant.heading })).toBeVisible();
    await expectAccessible(page);
  });
}

/** Code points, as the api counts them. */
const chars = (text: string) => Array.from(text).length;

test("chat history: long replies are truncated and old turns dropped", () => {
  const long = "ñ".repeat(3000);
  const sent = buildChatHistory([
    { role: "user", content: "hola" },
    { role: "assistant", content: long },
    { role: "user", content: "¿y luego?" },
  ]);
  expect(sent.map((m) => m.role)).toEqual(["user", "assistant", "user"]);
  expect(chars(sent[1].content)).toBe(MAX_MESSAGE_CHARS);
  expect(sent[1].content.endsWith("…")).toBe(true);
  expect(sent[2].content).toBe("¿y luego?");

  // Short replies pass through untouched.
  expect(buildChatHistory([{ role: "assistant", content: "ok" }, { role: "user", content: "a" }])).toEqual([
    { role: "user", content: "a" },
  ]);

  // Ten long exchanges: under the turn cap, over the total cap.
  const many: ChatMessage[] = [];
  for (let i = 0; i < 10; i++) {
    many.push({ role: "user", content: `q${i}` }, { role: "assistant", content: long });
  }
  many.push({ role: "user", content: "última" });
  const trimmed = buildChatHistory(many);
  expect(trimmed.length).toBeLessThanOrEqual(MAX_TURNS);
  expect(trimmed.reduce((n, m) => n + chars(m.content), 0)).toBeLessThanOrEqual(MAX_HISTORY_CHARS);
  expect(trimmed[0].role).toBe("user");
  expect(trimmed.at(-1)).toEqual({ role: "user", content: "última" });
  expect(trimmed.every((m) => chars(m.content) <= MAX_MESSAGE_CHARS)).toBe(true);

  // Many short turns: the count cap applies.
  const chatty: ChatMessage[] = Array.from({ length: 31 }, (_, i) => ({
    role: i % 2 === 0 ? ("user" as const) : ("assistant" as const),
    content: `m${i}`,
  }));
  const counted = buildChatHistory(chatty);
  expect(counted.length).toBeLessThanOrEqual(MAX_TURNS);
  expect(counted[0].role).toBe("user");
  expect(counted.at(-1)?.content).toBe("m30");
});

test("assistant: a follow-up after a 3000-character reply is accepted", async ({ page }) => {
  const t = catalogs.es;
  await signIn(page, "es");
  await useLanguage(page, "es");

  // The first reply is faked as a long answer; the follow-up goes to the real
  // api, which answers 422 if the echoed history breaks its caps.
  const longReply = "Temporada ".repeat(300).trim();
  const bodies: { messages: ChatMessage[] }[] = [];
  await page.route("**/backend/assistant/chat", async (route) => {
    bodies.push(route.request().postDataJSON() as { messages: ChatMessage[] });
    if (bodies.length === 1) {
      await route.fulfill({
        json: {
          chat_id: "e2e-long",
          reply: longReply,
          stop: "done",
          suggestions: [],
          usage: { input_tokens: 1, output_tokens: 1 },
        },
      });
    } else {
      await route.continue();
    }
  });

  await page.goto("/es/assistant");
  const input = page.getByLabel(t.assistant.label);
  await input.fill("Resume la temporada");
  await input.press("Enter");
  const log = page.getByTestId("chat-log");
  // The log shows the whole reply.
  await expect(log.locator(".chat-bot .chat-text").last()).toHaveText(longReply);

  await input.fill(`¿Cuándo conviene comprar ${ASIN} para el Buen Fin?`);
  await input.press("Enter");
  await expect(log.locator(".chat-bot")).toHaveCount(2, { timeout: 30_000 });
  await expect(page.locator(".chat [role=alert]")).toHaveText("");

  expect(bodies).toHaveLength(2);
  const sent = bodies[1].messages;
  expect(sent.map((m) => m.role)).toEqual(["user", "assistant", "user"]);
  expect(chars(sent[1].content)).toBeLessThanOrEqual(MAX_MESSAGE_CHARS);
  expect(sent[1].content.endsWith("…")).toBe(true);
  // The full reply is still on screen.
  await expect(log.locator(".chat-bot .chat-text").first()).toHaveText(longReply);
});
