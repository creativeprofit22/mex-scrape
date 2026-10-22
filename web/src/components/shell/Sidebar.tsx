import { Suspense } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { NAV_ITEMS, navHref } from "@/lib/nav";

import { AccountMenu, AccountMenuPlaceholder } from "./AccountMenu";
import { FrameLanguage } from "./FrameLanguage";
import { PrimaryNav } from "./PrimaryNav";
import { Wordmark } from "./Wordmark";

/**
 * Desktop frame (≥768px): wordmark, workspace pages, then the account footer
 * (session links and language). Hidden below 768px, where the top bar and
 * bottom tabs take over.
 */
export function Sidebar({ lang, dict }: { lang: Locale; dict: Dictionary }) {
  const items = NAV_ITEMS.map((item) => ({
    id: item.id,
    href: navHref(lang, item.path),
    label: dict.nav[item.label],
  }));
  return (
    <header className="sidebar">
      <Wordmark lang={lang} label={dict.nav.wordmark} />
      <PrimaryNav
        label={dict.nav.label}
        items={items}
        className="sidebar__nav"
        listClassName="nav-list"
      />
      {/* Recent searches slot: filled in a later phase (style-pack §1a). */}
      <div className="sidebar__footer">
        <Suspense fallback={<AccountMenuPlaceholder variant="sidebar" />}>
          <AccountMenu variant="sidebar" lang={lang} nav={dict.nav} plans={dict.account.plans} errors={dict.common.errors} />
        </Suspense>
        <FrameLanguage lang={lang} nav={dict.nav} />
      </div>
    </header>
  );
}
