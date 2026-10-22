import { notFound } from "next/navigation";

import { AppShell } from "@/components/shell/AppShell";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

/** Workspace pages (and onboarding) in the app frame. */
export default async function AppLayout({ children, params }: LayoutProps<"/[lang]">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return (
    <AppShell lang={lang} dict={dict}>
      {children}
    </AppShell>
  );
}
