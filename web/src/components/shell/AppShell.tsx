import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";

import { BottomTabs } from "./BottomTabs";
import { Sidebar } from "./Sidebar";
import { TopBar } from "./TopBar";

/**
 * Signed-in frame: a 100dvh grid where only the workspace scrolls. Sidebar at
 * ≥768px; top bar and bottom tabs below. `main` takes focus from the skip link.
 */
export function AppShell({
  lang,
  dict,
  children,
}: {
  lang: Locale;
  dict: Dictionary;
  children: React.ReactNode;
}) {
  return (
    <div className="app-shell">
      <Sidebar lang={lang} dict={dict} />
      <TopBar lang={lang} nav={dict.nav} />
      <main id="main" className="workspace" tabIndex={-1}>
        <div className="workspace__content">{children}</div>
      </main>
      <BottomTabs lang={lang} nav={dict.nav} />
    </div>
  );
}
