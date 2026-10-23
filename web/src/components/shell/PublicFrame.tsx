import { Suspense } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";

import { AccountMenu, AccountMenuPlaceholder } from "./AccountMenu";
import { FrameLanguage } from "./FrameLanguage";
import { ThemeToggle } from "./ThemeToggle";
import { Wordmark } from "./Wordmark";

/**
 * Minimal frame for home, sign-in and sign-up (style-pack §1a): wordmark,
 * session links, theme and language; no sidebar, normal document scroll.
 */
export function PublicFrame({
  lang,
  dict,
  children,
}: {
  lang: Locale;
  dict: Dictionary;
  children: React.ReactNode;
}) {
  return (
    <div className="public-frame">
      <header className="public-frame__header">
        <Wordmark lang={lang} label={dict.nav.wordmark} />
        <div className="public-frame__end">
          <Suspense fallback={<AccountMenuPlaceholder variant="header" />}>
            <AccountMenu variant="header" lang={lang} nav={dict.nav} plans={dict.account.plans} errors={dict.common.errors} />
          </Suspense>
          <ThemeToggle nav={dict.nav} />
          <FrameLanguage lang={lang} nav={dict.nav} />
        </div>
      </header>
      <main id="main" className="public-frame__main" tabIndex={-1}>
        {children}
      </main>
    </div>
  );
}
