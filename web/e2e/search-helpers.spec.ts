import { expect, test } from "@playwright/test";

// Node-only: the search page's pure helpers (src/lib/search), no browser.
import { filterRows, NO_FILTERS } from "../src/lib/search/filter";
import { formatSigned, signClass } from "../src/lib/search/format";
import { deriveKpis } from "../src/lib/search/kpis";
import { buildRows, resultKey, type RoiItem, type Row, type SearchResult } from "../src/lib/search/rows";
import { ariaSort, DEFAULT_SORT, nextSort, sortRows } from "../src/lib/search/sort";
import { stateFamily, viewState } from "../src/lib/search/status";
import {
  addRecent,
  centavosToPesosInput,
  cleanQuery,
  MAX_RECENT,
  parseCosts,
  parseRecent,
  serializeCosts,
  serializeRecent,
} from "../src/lib/search/storage";

function result(sku: string, price: number, extra: Partial<SearchResult> = {}): SearchResult {
  return {
    retailer: "walmart",
    retailer_sku: sku,
    title: `Producto ${sku}`,
    url: `https://example.com/${sku}`,
    price_centavos: price,
    availability: "in_stock",
    membership_required: false,
    ...extra,
  };
}

function roi(profit: number, extra: Partial<RoiItem> = {}): RoiItem {
  return {
    profit_centavos: profit,
    roi_bp: profit * 10,
    margin_bp: profit * 5,
    fixed_cost_per_unit_centavos: 0,
    total_cost_centavos: 0,
    ...extra,
  };
}

const AMAZON: NonNullable<SearchResult["amazon"]> = {
  asin: "B000TEST01",
  match_status: "accepted",
  confidence_bp: 9000,
};

/** a: +50, b: none, c: −20, d: +50 (tie with a), e: ROI error. */
function sample(): Row[] {
  const results = [
    result("a", 300, { amazon: AMAZON }),
    result("b", 100),
    result("c", 200, { amazon: AMAZON, availability: "out_of_stock" }),
    result("d", 400, { amazon: AMAZON }),
    result("e", 500, { amazon: AMAZON }),
  ];
  const map = new Map<number, RoiItem>([
    [0, roi(5000)],
    [2, roi(-2000)],
    [3, roi(5000)],
    [4, roi(9999, { error: "invalid_input" })],
  ]);
  return buildRows(results, map);
}

const skus = (rows: Row[]) => rows.map((r) => r.result.retailer_sku);

test("search helpers: rows are keyed by retailer and SKU", () => {
  expect(resultKey({ retailer: "costco", retailer_sku: "123" })).toBe("costco/123");
  const rows = sample();
  expect(rows.map((r) => r.index)).toEqual([0, 1, 2, 3, 4]);
  expect(rows[1]?.roi).toBeUndefined();
});

test("search helpers: sort puts missing values last and keeps API order on ties", () => {
  const rows = sample();
  expect(DEFAULT_SORT).toEqual({ key: "profit", dir: "desc" });
  expect(skus(sortRows(rows, DEFAULT_SORT))).toEqual(["a", "d", "c", "b", "e"]);
  expect(skus(sortRows(rows, { key: "profit", dir: "asc" }))).toEqual(["c", "a", "d", "b", "e"]);
  expect(skus(sortRows(rows, { key: "price", dir: "asc" }))).toEqual(["b", "c", "a", "d", "e"]);
  expect(skus(sortRows(rows, { key: "roi", dir: "desc" }))).toEqual(["a", "d", "c", "b", "e"]);
  // The input is not mutated.
  expect(skus(rows)).toEqual(["a", "b", "c", "d", "e"]);
});

test("search helpers: header activation toggles and aria-sort follows", () => {
  expect(nextSort(DEFAULT_SORT, "profit")).toEqual({ key: "profit", dir: "asc" });
  expect(nextSort({ key: "profit", dir: "asc" }, "profit")).toEqual({ key: "profit", dir: "desc" });
  expect(nextSort(DEFAULT_SORT, "price")).toEqual({ key: "price", dir: "asc" });
  expect(nextSort(DEFAULT_SORT, "roi")).toEqual({ key: "roi", dir: "desc" });
  expect(ariaSort(DEFAULT_SORT, "profit")).toBe("descending");
  expect(ariaSort({ key: "price", dir: "asc" }, "price")).toBe("ascending");
  expect(ariaSort(DEFAULT_SORT, "price")).toBe("none");
});

test("search helpers: filters combine and keep order", () => {
  const rows = sample();
  expect(skus(filterRows(rows, NO_FILTERS))).toEqual(["a", "b", "c", "d", "e"]);
  expect(skus(filterRows(rows, { ...NO_FILTERS, matched: true }))).toEqual(["a", "c", "d", "e"]);
  expect(skus(filterRows(rows, { ...NO_FILTERS, profitable: true }))).toEqual(["a", "d"]);
  expect(skus(filterRows(rows, { ...NO_FILTERS, inStock: true }))).toEqual(["a", "b", "d", "e"]);
  expect(skus(filterRows(rows, { matched: true, profitable: true, inStock: true }))).toEqual(["a", "d"]);
});

test("search helpers: KPIs cover the whole search", () => {
  const rows = sample();
  const kpis = deriveKpis(
    {
      results: rows.map((r) => r.result),
      retailers: [
        { retailer: "walmart", outcome: "ok", offers: 5 },
        { retailer: "costco", outcome: "ok", offers: 0 },
        { retailer: "sams", outcome: "failed", offers: 0 },
      ],
      hidden_results: 2,
    },
    rows,
  );
  // The ROI error's 9999 is ignored; the tie keeps the earlier row.
  expect(kpis.bestProfit).toEqual({ centavos: 5000, key: "walmart/a" });
  expect(kpis.offers).toBe(5);
  expect(kpis.stores).toEqual({ withOffers: 1, total: 3 });
  expect(kpis.hidden).toBe(2);

  const none = deriveKpis({ results: [], retailers: [], hidden_results: 0 }, []);
  expect(none.bestProfit).toBeNull();
});

test("search helpers: signed amounts and classes", () => {
  const glyphs = { plus: "+", minus: "−" };
  const fmt = (n: number) => `$${n}`;
  expect(formatSigned(50, fmt, glyphs)).toBe("+$50");
  expect(formatSigned(-20, fmt, glyphs)).toBe("−$20");
  expect(formatSigned(0, fmt, glyphs)).toBe("$0");
  expect(signClass(1)).toBe("gain");
  expect(signClass(-1)).toBe("loss");
  expect(signClass(0)).toBeUndefined();
  expect(signClass(null)).toBeUndefined();
});

test("search helpers: saved costs are validated", () => {
  expect(parseCosts(serializeCosts({ prep: 1500, shipping: 0 }))).toEqual({ prep: 1500, shipping: 0 });
  for (const raw of [
    null,
    "",
    "{",
    "null",
    "[]",
    '"x"',
    '{"prep":1500,"shipping":0}',
    '{"v":2,"prep":1500,"shipping":0}',
    '{"v":1,"prep":-1,"shipping":0}',
    '{"v":1,"prep":15.5,"shipping":0}',
    '{"v":1,"prep":"1500","shipping":0}',
    '{"v":1,"prep":1500}',
    '{"v":1,"prep":1e300,"shipping":0}',
  ]) {
    expect(parseCosts(raw), String(raw)).toBeNull();
  }
  expect(centavosToPesosInput(1500)).toBe("15");
  expect(centavosToPesosInput(1550)).toBe("15.50");
  expect(centavosToPesosInput(5)).toBe("0.05");
  expect(centavosToPesosInput(0)).toBe("0");
});

test("search helpers: recent searches dedupe, cap and reject bad storage", () => {
  expect(cleanQuery("  licuadora  ")).toBe("licuadora");
  expect(cleanQuery("   ")).toBeNull();
  expect(cleanQuery("x".repeat(101))).toBeNull();
  expect(cleanQuery("x".repeat(100))).toBe("x".repeat(100));

  let list: string[] = [];
  list = addRecent(list, "Licuadora");
  list = addRecent(list, "audífonos");
  list = addRecent(list, "  licuadora ");
  expect(list).toEqual(["licuadora", "audífonos"]);
  expect(addRecent(list, "   ")).toEqual(list);

  for (let i = 0; i < 12; i++) list = addRecent(list, `q${i}`);
  expect(list).toHaveLength(MAX_RECENT);
  expect(list[0]).toBe("q11");

  expect(parseRecent(serializeRecent(["b", "a"]))).toEqual(["b", "a"]);
  // Duplicates and overlong entries in storage are cleaned on read.
  expect(parseRecent(serializeRecent(["A", "a", "x".repeat(101), " b "]))).toEqual(["A", "b"]);
  for (const raw of [null, "", "{", "[]", '{"v":1}', '{"v":2,"items":[]}', '{"v":1,"items":"a"}', '{"v":1,"items":[1]}']) {
    expect(parseRecent(raw), String(raw)).toEqual([]);
  }
  expect(parseRecent(serializeRecent(Array.from({ length: 20 }, (_, i) => `q${i}`)))).toHaveLength(MAX_RECENT);
});

test("search helpers: view state for every case", () => {
  type Input = Parameters<typeof viewState>[0];
  const base: NonNullable<Input["search"]> = { status: "running", retailers: [], results: [], hidden_results: 0 };
  const stores = [{ retailer: "walmart", outcome: "ok", offers: 1 }];
  const one = [result("a", 100)];
  const v = (search: Input["search"], more: Partial<Input> = {}) =>
    viewState({ search, pending: false, timedOut: false, failure: null, ...more });

  expect(v(null)).toBe("idle");
  expect(v(null, { pending: true })).toBe("queued");
  expect(v({ ...base, status: "queued" })).toBe("queued");
  expect(v(base)).toBe("running");
  expect(v({ ...base, retailers: stores })).toBe("running-stores");
  expect(v({ ...base, status: "done", results: one })).toBe("done");
  expect(v({ ...base, status: "done" })).toBe("empty");
  expect(v({ ...base, status: "done", hidden_results: 3 })).toBe("all-hidden");
  expect(v({ ...base, status: "partial", results: one })).toBe("partial");
  expect(v({ ...base, status: "failed" })).toBe("failed");
  expect(v({ ...base, status: "refused" })).toBe("refused");
  expect(v(base, { timedOut: true })).toBe("timeout");
  expect(v(base, { failure: "network" })).toBe("network");
  expect(v(base, { failure: "other", timedOut: true })).toBe("error");

  expect(stateFamily("running-stores")).toBe("loading");
  expect(stateFamily("all-hidden")).toBe("empty");
  expect(stateFamily("partial")).toBe("done");
  expect(stateFamily("refused")).toBe("error");
});
