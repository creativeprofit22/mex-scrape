import { notFound } from "next/navigation";
import { Suspense } from "react";

import { PriceHistory } from "@/components/product/PriceHistory";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

type Params = PageProps<"/[lang]/products/[asin]">["params"];

/** ASINs are 10 upper-case letters or digits (the API validates too). */
const ASIN = /^[A-Z0-9]{10}$/;

// The ASIN is only known at request time, so it is read inside <Suspense>
// (Cache Components keeps the rest of the page in the static shell).
export default function ProductPage({ params }: PageProps<"/[lang]/products/[asin]">) {
  return (
    <Suspense fallback={<p role="status" aria-busy="true" />}>
      <Product params={params} />
    </Suspense>
  );
}

async function Product({ params }: { params: Params }) {
  const { lang, asin } = await params;
  if (!hasLocale(lang) || !ASIN.test(asin)) notFound();
  const dict = await getDictionary(lang);
  return (
    <PriceHistory
      asin={asin}
      lang={lang}
      dict={dict.product}
      common={dict.common}
      retailers={dict.retailers}
    />
  );
}
