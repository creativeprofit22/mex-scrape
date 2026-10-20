/** Supported UI languages; Spanish is the default. */
export const locales = ["es", "en"] as const;

/** A supported UI language. */
export type Locale = (typeof locales)[number];

/** Language used when nothing else says otherwise. */
export const defaultLocale: Locale = "es";

/** Cookie remembering the chosen language (read by `proxy.ts`). */
export const localeCookie = "lang";

/** Narrows a route segment or cookie value to a supported locale. */
export function hasLocale(value: string | undefined): value is Locale {
  return value !== undefined && (locales as readonly string[]).includes(value);
}

/** BCP 47 tag used for number and date formatting. */
export function intlTag(lang: Locale): string {
  return lang === "es" ? "es-MX" : "en-US";
}

/** Swaps the leading locale segment of `pathname` for `lang`. */
export function localizedPath(pathname: string, lang: Locale): string {
  const parts = pathname.split("/");
  if (hasLocale(parts[1])) {
    parts[1] = lang;
    return parts.join("/");
  }
  return `/${lang}${pathname === "/" ? "" : pathname}`;
}
