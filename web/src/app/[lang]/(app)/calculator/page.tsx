import { notFound } from "next/navigation";

import { Calculator } from "@/components/calculator/Calculator";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function CalculatorPage({
  params,
}: PageProps<"/[lang]/calculator">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return <Calculator lang={lang} dict={dict.calculator} common={dict.common} sign={dict.search.sign} />;
}
