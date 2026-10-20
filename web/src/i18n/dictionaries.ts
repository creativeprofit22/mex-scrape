import type { Locale } from "./config";
import type es from "./messages/es.json";

/** The message catalog shape; `es.json` is the source of truth. */
export type Dictionary = typeof es;

const dictionaries: Record<Locale, () => Promise<Dictionary>> = {
  es: () => import("./messages/es.json").then((m) => m.default),
  en: () => import("./messages/en.json").then((m) => m.default),
};

/** Loads the message catalog for `lang` (server side). */
export function getDictionary(lang: Locale): Promise<Dictionary> {
  return dictionaries[lang]();
}
