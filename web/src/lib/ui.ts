import { hasLocale, type Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { NETWORK_ERROR } from "@/lib/api/client";

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
  if (code === NETWORK_ERROR) return common.network;
  if (status === 429) return common.rateLimited;
  if (status === 503) return common.unavailable;
  if (status === 403 && code === "csrf") return common.csrf;
  return common.generic;
}

/** Query parameter naming the page to return to after sign-in or onboarding. */
export const NEXT_PARAM = "next";

/** `path` with `?next=<next>` appended when there is somewhere to return to. */
function withNext(path: string, next: string | undefined): string {
  return next ? `${path}?${NEXT_PARAM}=${encodeURIComponent(next)}` : path;
}

/** Sign-in page for `lang`; afterwards the user goes to `next` (an in-app path). */
export function signInPath(lang: Locale, next?: string): string {
  return withNext(`/${lang}/signin`, next);
}

/** Sign-up page for `lang`; afterwards the user goes to `next` (an in-app path). */
export function signUpPath(lang: Locale, next?: string): string {
  return withNext(`/${lang}/signup`, next);
}

/** Onboarding page for `lang`; afterwards the user goes to `next` (an in-app path). */
export function onboardingPath(lang: Locale, next?: string): string {
  return withNext(`/${lang}/onboarding`, next);
}

/** The current page's path and query, to come back to later. Browser only. */
export function currentPath(): string {
  return window.location.pathname + window.location.search;
}

/**
 * Returns `raw` normalised when it is a path inside this app under a
 * supported locale, else `null`. Rejects other origins (`//host`,
 * `https://…`, backslash tricks) so `?next=` can never redirect off-site,
 * and the sign-in/sign-up pages so it can't loop.
 */
export function safeNextPath(raw: string | null): string | null {
  if (!raw || !raw.startsWith("/") || raw.includes("\\")) return null;
  const origin = "http://app.invalid";
  let url: URL;
  try {
    url = new URL(raw, origin);
  } catch {
    return null;
  }
  if (url.origin !== origin) return null;
  const [, lang, page] = url.pathname.split("/");
  if (!hasLocale(lang) || page === "signin" || page === "signup") return null;
  return url.pathname + url.search;
}

/** The validated `?next=` of the current page, or `null`. Browser only. */
export function nextFromLocation(): string | null {
  return safeNextPath(new URLSearchParams(window.location.search).get(NEXT_PARAM));
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
