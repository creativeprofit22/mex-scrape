import { notFound } from "next/navigation";

import { SearchPanel } from "@/components/SearchPanel";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function SearchPage({ params }: PageProps<"/[lang]/search">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return (
    <SearchPanel
      lang={lang}
      dict={dict.search}
      common={dict.common}
      retailers={dict.retailers}
    />
  );
}
