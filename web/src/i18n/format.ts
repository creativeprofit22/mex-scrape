import { intlTag, type Locale } from "./config";

/** Formats centavos as Mexican pesos for the given UI language. */
export function formatMoney(centavos: number, lang: Locale): string {
  return new Intl.NumberFormat(intlTag(lang), {
    style: "currency",
    currency: "MXN",
  }).format(centavos / 100);
}

/** Formats basis points (1 bp = 0.01 %) as a percentage. */
export function formatBasisPoints(bp: number, lang: Locale): string {
  return new Intl.NumberFormat(intlTag(lang), {
    style: "percent",
    maximumFractionDigits: 1,
  }).format(bp / 10_000);
}

/** Formats epoch milliseconds as a calendar date (UTC). */
export function formatDay(ms: number, lang: Locale): string {
  return new Intl.DateTimeFormat(intlTag(lang), {
    dateStyle: "medium",
    timeZone: "UTC",
  }).format(new Date(ms));
}

/** Replaces `{name}` placeholders in a catalog message. */
export function interpolate(
  message: string,
  values: Record<string, string | number>,
): string {
  return message.replace(/\{(\w+)\}/g, (whole, key: string) =>
    key in values ? String(values[key]) : whole,
  );
}
