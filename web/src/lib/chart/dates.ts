// Pure time-axis helpers for the price chart. Days are UTC midnights, like the
// API's `day_ms` and `formatDay` (which formats in UTC).

export const DAY_MS = 86_400_000;

/** Day-sized steps, then calendar-month steps (1, 2, 3, 6, 12 months). */
const DAY_STEPS = [1, 2, 7, 14] as const;
const MONTH_STEPS = [1, 2, 3, 6, 12] as const;

/** UTC midnight at or after `ms`. */
function ceilDay(ms: number): number {
  return Math.ceil(ms / DAY_MS) * DAY_MS;
}

function dayTicks(fromMs: number, toMs: number, days: number): number[] {
  const step = days * DAY_MS;
  // Weekly steps start on Mondays (1970-01-01 was a Thursday, so +3 days).
  const offset = days % 7 === 0 ? 3 * DAY_MS : 0;
  const first = Math.ceil((ceilDay(fromMs) + offset) / step) * step - offset;
  const ticks: number[] = [];
  for (let t = first; t <= toMs; t += step) ticks.push(t);
  return ticks;
}

function monthTicks(fromMs: number, toMs: number, months: number): number[] {
  const start = new Date(ceilDay(fromMs));
  // First month start (1st, UTC) at or after `from`, aligned to the step.
  let year = start.getUTCFullYear();
  let month = start.getUTCMonth() + (start.getUTCDate() === 1 ? 0 : 1);
  month = Math.ceil(month / months) * months;
  const ticks: number[] = [];
  for (;;) {
    const t = Date.UTC(year, month, 1);
    if (t > toMs) return ticks;
    ticks.push(t);
    month += months;
    if (month >= 12) {
      year += Math.floor(month / 12);
      month %= 12;
    }
  }
}

/**
 * Whole-day ticks (UTC midnights) inside `[fromMs, toMs]`, at most `maxTicks`
 * of them. Picks the finest step that fits: 1, 2, 7 or 14 days (weeks start on
 * Monday), then the 1st of every 1, 2, 3, 6 or 12 months. Empty or invalid
 * ranges give `[]`.
 */
export function dateTicks(fromMs: number, toMs: number, maxTicks: number): number[] {
  if (!Number.isFinite(fromMs) || !Number.isFinite(toMs) || toMs < fromMs) return [];
  const cap = Math.max(1, Math.floor(Number.isFinite(maxTicks) ? maxTicks : 1));
  for (const days of DAY_STEPS) {
    const ticks = dayTicks(fromMs, toMs, days);
    if (ticks.length <= cap) return ticks;
  }
  for (const months of MONTH_STEPS) {
    const ticks = monthTicks(fromMs, toMs, months);
    if (ticks.length <= cap) return ticks;
  }
  // Over a decade at a tiny cap: yearly ticks, thinned to fit.
  const yearly = monthTicks(fromMs, toMs, 12);
  const every = Math.ceil(yearly.length / cap);
  return yearly.filter((_, i) => i % every === 0);
}
