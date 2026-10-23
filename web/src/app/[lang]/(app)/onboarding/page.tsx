import { notFound } from "next/navigation";

import { OnboardingForm } from "@/components/auth/OnboardingForm";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function OnboardingPage({
  params,
}: PageProps<"/[lang]/onboarding">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return <OnboardingForm lang={lang} dict={dict.onboarding} common={dict.common} />;
}
