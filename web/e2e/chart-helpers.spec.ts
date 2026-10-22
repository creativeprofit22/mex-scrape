import { expect, test } from "@playwright/test";

// Node-only: the price chart's pure helpers (src/lib/chart), no browser.
import { goodPriceBand } from "../src/lib/chart/band";
import { DAY_MS, dateTicks } from "../src/lib/chart/dates";
import { linearScale, niceStep, niceTicks } from "../src/lib/chart/scale";

/** True when `step` is 1, 2 or 5 × 10ⁿ. */
function onLadder(step: number): boolean {
  const magnitude = 10 ** Math.floor(Math.log10(step) + 1e-9);
  const rung = Math.round((step / magnitude) * 1e6) / 1e6;
  return rung === 1 || rung === 2 || rung === 5;
}

function steps(ticks: number[]): number[] {
  return ticks.slice(1).map((t, i) => Math.round((t - (ticks[i] ?? 0)) * 1e6) / 1e6);
}

test.describe("niceStep", () => {
  test("picks the smallest 1-2-5 rung that fits", () => {
    expect(niceStep(10, 5)).toBe(2);
    expect(niceStep(10, 10)).toBe(1);
    expect(niceStep(100, 4)).toBe(50);
    expect(niceStep(0.3, 3)).toBeCloseTo(0.1);
    expect(niceStep(7_000, 3)).toBe(5_000);
  });

  test("returns 0 for an empty or invalid range", () => {
    expect(niceStep(0, 5)).toBe(0);
    expect(niceStep(-3, 5)).toBe(0);
    expect(niceStep(Number.NaN, 5)).toBe(0);
  });
});

test.describe("niceTicks", () => {
  const cases: [number, number, number][] = [
    [0, 10, 5],
    [123, 987, 6],
    [149_900, 189_900, 5],
    [1, 2, 2],
    [0.1, 0.35, 4],
    [-50, 75, 5],
    [12_345_678_900, 98_765_432_100, 6],
    [99_999, 100_001, 3],
  ];

  for (const [min, max, cap] of cases) {
    test(`covers [${min}, ${max}] with at most ${cap} round ticks`, () => {
      const ticks = niceTicks(min, max, cap);
      expect(ticks.length).toBeGreaterThanOrEqual(2);
      expect(ticks.length).toBeLessThanOrEqual(cap);
      expect(ticks[0]).toBeLessThanOrEqual(min);
      expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(max);
      const gaps = steps(ticks);
      expect(new Set(gaps).size).toBe(1);
      expect(onLadder(gaps[0] ?? 0)).toBe(true);
    });
  }

  test("ticks are clean decimals, not float noise", () => {
    expect(niceTicks(0.1, 0.35, 4)).toEqual([0.1, 0.2, 0.3, 0.4]);
    expect(niceTicks(0.3, 0.7, 5)).toEqual([0.3, 0.4, 0.5, 0.6, 0.7]);
  });

  test("a flat series is padded around its value", () => {
    const ticks = niceTicks(150_000, 150_000, 5);
    expect(ticks.length).toBeGreaterThanOrEqual(2);
    expect(ticks[0]).toBeLessThan(150_000);
    expect(ticks[ticks.length - 1]).toBeGreaterThan(150_000);
  });

  test("a flat series at zero still gets two ticks", () => {
    const ticks = niceTicks(0, 0, 5);
    expect(ticks[0]).toBeLessThan(0);
    expect(ticks[ticks.length - 1]).toBeGreaterThan(0);
  });

  test("large peso values keep whole steps", () => {
    // $1,234,567.89 to $1,250,000.00 in centavos.
    const ticks = niceTicks(123_456_789, 125_000_000, 5);
    expect(ticks.every(Number.isInteger)).toBe(true);
    expect(ticks[0]).toBeLessThanOrEqual(123_456_789);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(125_000_000);
  });

  test("minStep keeps ticks on whole pesos", () => {
    const ticks = niceTicks(1_000, 1_050, 6, { minStep: 100 });
    expect(steps(ticks).every((s) => s >= 100)).toBe(true);
    expect(ticks.every((t) => t % 100 === 0)).toBe(true);
  });

  test("swapped bounds are accepted", () => {
    expect(niceTicks(10, 0, 5)).toEqual(niceTicks(0, 10, 5));
  });

  test("non-finite input gives no ticks", () => {
    expect(niceTicks(Number.NaN, 10, 5)).toEqual([]);
    expect(niceTicks(0, Number.POSITIVE_INFINITY, 5)).toEqual([]);
    // Math.min/max of an empty series.
    expect(niceTicks(Math.min(), Math.max(), 5)).toEqual([]);
  });

  test("a cap below two is treated as two", () => {
    const ticks = niceTicks(0, 1_000, 1);
    expect(ticks).toHaveLength(2);
  });
});

test.describe("linearScale", () => {
  test("maps the domain onto the range, including inverted ranges", () => {
    const y = linearScale([0, 100], [200, 0]);
    expect(y(0)).toBe(200);
    expect(y(100)).toBe(0);
    expect(y(25)).toBe(150);
  });

  test("a zero-width domain maps to the middle", () => {
    const x = linearScale([5, 5], [0, 640]);
    expect(x(5)).toBe(320);
    expect(x(999)).toBe(320);
  });
});

test.describe("dateTicks", () => {
  const from = Date.UTC(2026, 6, 24, 15, 30); // mid-day, 2026-07-24
  const to = Date.UTC(2026, 9, 22, 15, 30); // 90 days later

  test("ticks are whole UTC days inside the range, under the cap", () => {
    for (const cap of [3, 5, 8, 14]) {
      const ticks = dateTicks(from, to, cap);
      expect(ticks.length).toBeGreaterThan(0);
      expect(ticks.length).toBeLessThanOrEqual(cap);
      for (const t of ticks) {
        expect(t % DAY_MS).toBe(0);
        expect(t).toBeGreaterThanOrEqual(from);
        expect(t).toBeLessThanOrEqual(to);
      }
    }
  });

  test("90 days at 14 ticks is weekly, on Mondays", () => {
    const ticks = dateTicks(from, to, 14);
    expect(steps(ticks).every((s) => s === 7 * DAY_MS)).toBe(true);
    expect(ticks.every((t) => new Date(t).getUTCDay() === 1)).toBe(true);
  });

  test("365 days at 12 ticks is monthly, on the 1st", () => {
    const ticks = dateTicks(to - 365 * DAY_MS, to, 12);
    expect(ticks).toHaveLength(12);
    expect(ticks.every((t) => new Date(t).getUTCDate() === 1)).toBe(true);
  });

  test("a single day gives that day", () => {
    const day = Date.UTC(2026, 9, 22);
    expect(dateTicks(day, day, 5)).toEqual([day]);
  });

  test("an empty or invalid range gives no ticks", () => {
    expect(dateTicks(to, from, 5)).toEqual([]);
    expect(dateTicks(Number.NaN, to, 5)).toEqual([]);
  });
});

test.describe("goodPriceBand", () => {
  const lows = (...values: number[]) => values.map((low_centavos) => ({ low_centavos }));

  test("runs from the lowest low to the 25th percentile", () => {
    expect(goodPriceBand(lows(500, 100, 300, 200, 400))).toEqual({ low: 100, high: 200 });
    // Rank 0.75 between 100 and 200.
    expect(goodPriceBand(lows(100, 200, 300, 400))).toEqual({ low: 100, high: 175 });
  });

  test("needs at least three points", () => {
    expect(goodPriceBand(lows())).toBeNull();
    expect(goodPriceBand(lows(100))).toBeNull();
    expect(goodPriceBand(lows(100, 200))).toBeNull();
  });

  test("a flat series has no band", () => {
    expect(goodPriceBand(lows(150_000, 150_000, 150_000))).toBeNull();
  });

  test("large peso values round to whole centavos", () => {
    const band = goodPriceBand(lows(123_456_789, 123_456_790, 125_000_000, 130_000_000));
    expect(band?.low).toBe(123_456_789);
    expect(Number.isInteger(band?.high)).toBe(true);
  });
});
