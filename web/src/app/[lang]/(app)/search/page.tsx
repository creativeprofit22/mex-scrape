import { notFound } from "next/navigation";
import { Suspense } from "react";

import { SearchPanel } from "@/components/SearchPanel";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function SearchPage({ params }: PageProps<"/[lang]/search">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  // SearchPanel reads `?q=` (a recent-search rerun) with useSearchParams,
  // which needs a Suspense boundary on a prerendered page.
  return (
    <Suspense fallback={<p role="status">{dict.common.loading}</p>}>
      <SearchPanel
        lang={lang}
        dict={dict.search}
        common={dict.common}
        retailers={dict.retailers}
      />
    </Suspense>
  );
}
