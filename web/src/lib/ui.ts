import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";

/** Messages for errors any API call can return. */
export type CommonErrors = Dictionary["common"]["errors"];

/**
 * Picks the message for a failed API call: a code-specific one from
 * `specific` when present, else one of the shared messages.
 */
export function errorMessage(
  status: number,
  code: string | undefined,
  common: CommonErrors,
  specific: Record<string, string> = {},
): string {
  if (code !== undefined && code in specific) return specific[code];
  if (status === 429) return common.rateLimited;
  if (status === 503) return common.unavailable;
  if (status === 403 && code === "csrf") return common.csrf;
  return common.generic;
}

/** Sign-in page for `lang`. */
export function signInPath(lang: Locale): string {
  return `/${lang}/signin`;
}

/**
 * Parses a peso amount typed by the user into centavos. Empty means zero
 * when `optional`; anything negative, non-numeric or with more than two
 * decimals is `null`.
 */
export function pesosToCentavos(value: string, optional = false): number | null {
  const trimmed = value.trim().replace(",", ".");
  if (trimmed === "") return optional ? 0 : null;
  if (!/^\d+(\.\d{1,2})?$/.test(trimmed)) return null;
  return Math.round(Number(trimmed) * 100);
}

/** Display name of a retailer id, falling back to the id. */
export function retailerName(id: string, names: Dictionary["retailers"]): string {
  return (names as Record<string, string>)[id] ?? id;
}
