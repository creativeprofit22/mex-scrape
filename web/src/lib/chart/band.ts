// The chart's "good price" band (docs/modules/web/detail-panel.md#chart).

/** Price range shaded as a good buy, in centavos (`low` ≤ `high`). */
export type PriceBand = { low: number; high: number };

/** Fewer daily lows than this and the band is not drawn. */
export const MIN_BAND_POINTS = 3;

/**
 * From the lowest daily low up to the 25th percentile of the daily lows in
 * the window (linear interpolation between ranks, rounded to whole centavos).
 * `null` with fewer than 3 points or when every low is the same price.
 */
export function goodPriceBand(points: readonly { low_centavos: number }[]): PriceBand | null {
  const lows = points
    .map((p) => p.low_centavos)
    .filter((v) => Number.isFinite(v))
    .sort((a, b) => a - b);
  const low = lows[0];
  if (low === undefined || lows.length < MIN_BAND_POINTS) return null;
  const rank = 0.25 * (lows.length - 1);
  const below = lows[Math.floor(rank)] ?? low;
  const above = lows[Math.ceil(rank)] ?? below;
  const high = Math.round(below + (above - below) * (rank - Math.floor(rank)));
  return high > low ? { low, high } : null;
}
