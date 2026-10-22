import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";

import { FrameLanguage } from "./FrameLanguage";
import { Wordmark } from "./Wordmark";

/** Phone frame (<768px) top: wordmark and language. Session links live on the Cuenta tab. */
export function TopBar({ lang, nav }: { lang: Locale; nav: Dictionary["nav"] }) {
  return (
    <header className="top-bar">
      <Wordmark lang={lang} label={nav.wordmark} />
      <FrameLanguage lang={lang} nav={nav} />
    </header>
  );
}
