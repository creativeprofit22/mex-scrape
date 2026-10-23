import { notFound } from "next/navigation";

import { AuthForm } from "@/components/auth/AuthForm";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function SignInPage({ params }: PageProps<"/[lang]/signin">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  return (
    <AuthForm mode="signin" lang={lang} dict={dict.auth} errors={dict.common.errors} />
  );
}
