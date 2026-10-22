import { hasLocale, type Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";

/** A place the app frame can mark as the current page. */
export type NavId = "search" | "calculator" | "assistant" | "account" | "signIn" | "signUp";

/** A primary navigation entry: its id, its path under `/[lang]` and its label key. */
export type NavItem = {
  id: NavId;
  path: string;
  label: keyof Dictionary["nav"];
};

/** Workspace pages in the sidebar, in order. */
export const NAV_ITEMS = [
  { id: "search", path: "/search", label: "search" },
  { id: "calculator", path: "/calculator", label: "calculator" },
  { id: "assistant", path: "/assistant", label: "assistant" },
] as const satisfies readonly NavItem[];

/** Phone bottom tabs: the workspace pages plus the account page. */
export const TAB_ITEMS = [
  ...NAV_ITEMS,
  { id: "account", path: "/account", label: "account" },
] as const satisfies readonly NavItem[];

/** The page each first path segment belongs to; products count as search. */
const SECTIONS: Record<string, NavId> = {
  search: "search",
  products: "search",
  calculator: "calculator",
  assistant: "assistant",
  account: "account",
  signin: "signIn",
  signup: "signUp",
};

/**
 * The nav item to mark `aria-current="page"` for `pathname`, or `null`
 * (home, onboarding, unknown pages). Pure; the locale segment is skipped.
 */
export function activeNavItem(pathname: string): NavId | null {
  const parts = pathname.split("/").filter(Boolean);
  const section = hasLocale(parts[0]) ? parts[1] : parts[0];
  if (section === undefined) return null;
  return Object.hasOwn(SECTIONS, section) ? SECTIONS[section] : null;
}

/** `/[lang]` + `path`. */
export function navHref(lang: Locale, path: string): string {
  return `/${lang}${path}`;
}

/** Display name of a plan id, falling back to the id itself. */
export function planLabel(plan: string, plans: Dictionary["account"]["plans"]): string {
  return Object.hasOwn(plans, plan) ? (plans as Record<string, string>)[plan] : plan;
}
