// Pure value-axis helpers for the price chart (no React, no DOM).
// Ticks sit on the 1-2-5 × 10ⁿ ladder. /* see sources#nice-step */

const LADDER = [1, 2, 5] as const;

/**
 * The smallest 1-2-5 × 10ⁿ step that splits `range` into at most `maxTicks`
 * intervals. Returns 0 for an empty or invalid range.
 */
export function niceStep(range: number, maxTicks: number): number {
  if (!Number.isFinite(range) || range <= 0) return 0;
  const intervals = Math.max(1, Math.floor(maxTicks));
  const raw = range / intervals;
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  for (const rung of LADDER) {
    // Small tolerance so 2.0000000001 × 10ⁿ still picks the 2 rung.
    if (rung * magnitude >= raw * (1 - 1e-9)) return rung * magnitude;
  }
  return 10 * magnitude;
}

/** The next rung up the 1-2-5 ladder (1 → 2 → 5 → 10 → 20 …). */
function nextRung(step: number): number {
  const magnitude = 10 ** Math.floor(Math.log10(step) + 1e-9);
  const rung = Math.round(step / magnitude);
  if (rung < 2) return 2 * magnitude;
  if (rung < 5) return 5 * magnitude;
  return 10 * magnitude;
}

/** Rounds away float noise (0.30000000000000004 → 0.3) at the step's precision. */
function clean(value: number, step: number): number {
  const decimals = Math.max(0, -Math.floor(Math.log10(step)));
  return Number(value.toFixed(Math.min(decimals, 20)));
}

export type NiceTicksOptions = {
  /** Smallest allowed step (e.g. 100 centavos for whole pesos). Default 0. */
  minStep?: number;
};

/**
 * Round tick values covering `[min, max]`: the first tick is ≤ min, the last
 * ≥ max, the step is 1-2-5 × 10ⁿ, and there are at most `maxTicks` (≥ 2) of
 * them. A flat range (min = max, e.g. a single point) is padded so the value
 * sits between two ticks. Non-finite input gives `[]`.
 */
export function niceTicks(
  min: number,
  max: number,
  maxTicks: number,
  options: NiceTicksOptions = {},
): number[] {
  if (!Number.isFinite(min) || !Number.isFinite(max)) return [];
  let lo = Math.min(min, max);
  let hi = Math.max(min, max);
  const cap = Math.max(2, Math.floor(Number.isFinite(maxTicks) ? maxTicks : 2));
  const minStep = Math.max(0, options.minStep ?? 0);
  if (hi === lo) {
    const pad = Math.max(Math.abs(lo) * 0.05, minStep, 1);
    lo -= pad;
    hi += pad;
  }
  let step = Math.max(niceStep(hi - lo, cap - 1), minStep > 0 ? niceStep(minStep, 1) : 0);
  // Widen the step until the covering ticks fit under the cap.
  for (;;) {
    // Tolerance: 0.3 / 0.1 is 2.9999999999999996, which must count as 3.
    const first = Math.floor(lo / step + 1e-9);
    const last = Math.ceil(hi / step - 1e-9);
    if (last - first + 1 <= cap) {
      const ticks: number[] = [];
      for (let k = first; k <= last; k++) ticks.push(clean(k * step, step));
      return ticks;
    }
    step = nextRung(step);
  }
}

/**
 * Linear map from `domain` to `range` (e.g. centavos → SVG y). A zero-width
 * domain maps every value to the middle of the range.
 */
export function linearScale(
  domain: readonly [number, number],
  range: readonly [number, number],
): (value: number) => number {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  const span = d1 - d0;
  if (span === 0 || !Number.isFinite(span)) {
    const mid = (r0 + r1) / 2;
    return () => mid;
  }
  return (value) => r0 + ((value - d0) / span) * (r1 - r0);
}
