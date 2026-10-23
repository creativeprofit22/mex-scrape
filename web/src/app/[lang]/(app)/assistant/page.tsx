import { notFound } from "next/navigation";

import { AssistantChat } from "@/components/assistant/AssistantChat";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function AssistantPage({ params }: PageProps<"/[lang]/assistant">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return (
    <AssistantChat
      lang={lang}
      dict={dict.assistant}
      common={dict.common}
      retailers={dict.retailers}
    />
  );
}
