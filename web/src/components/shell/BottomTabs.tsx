import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { TAB_ITEMS, navHref } from "@/lib/nav";

import { PrimaryNav } from "./PrimaryNav";

/** Phone frame (<768px) bottom: workspace pages plus Cuenta, padded past the home indicator. */
export function BottomTabs({ lang, nav }: { lang: Locale; nav: Dictionary["nav"] }) {
  const items = TAB_ITEMS.map((item) => ({
    id: item.id,
    href: navHref(lang, item.path),
    label: nav[item.label],
  }));
  return (
    <PrimaryNav label={nav.label} items={items} className="bottom-tabs" listClassName="tab-list" />
  );
}
