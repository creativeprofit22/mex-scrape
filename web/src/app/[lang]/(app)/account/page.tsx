import { notFound } from "next/navigation";

import { AccountPanel } from "@/components/AccountPanel";
import { FrameLanguage } from "@/components/shell/FrameLanguage";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function AccountPage({ params }: PageProps<"/[lang]/account">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return (
    <AccountPanel
      lang={lang}
      dict={dict.account}
      nav={dict.nav}
      common={dict.common}
      language={<FrameLanguage lang={lang} nav={dict.nav} />}
    />
  );
}
