import { notFound } from "next/navigation";

import { PublicFrame } from "@/components/shell/PublicFrame";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

/** Home, sign-in and sign-up in the minimal public frame. */
export default async function PublicLayout({ children, params }: LayoutProps<"/[lang]">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return (
    <PublicFrame lang={lang} dict={dict}>
      {children}
    </PublicFrame>
  );
}
