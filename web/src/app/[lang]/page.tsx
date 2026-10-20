import Link from "next/link";
import { notFound } from "next/navigation";

import { HealthStatus } from "@/components/HealthStatus";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

export default async function Home({ params }: PageProps<"/[lang]">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);

  return (
    <>
      <h1>{dict.home.heading}</h1>
      <p>{dict.home.intro}</p>
      <p className="actions">
        <Link className="button" href={`/${lang}/search`}>
          {dict.home.start}
        </Link>
        <Link href={`/${lang}/signup`}>{dict.home.createAccount}</Link>
      </p>
      <HealthStatus labels={dict.health} />
    </>
  );
}
