import type { Metadata } from "next";
import { Archivo } from "next/font/google";
import Link from "next/link";
import { notFound } from "next/navigation";
import { Suspense } from "react";

import { LanguageToggle } from "@/components/LanguageToggle";
import { SignOutButton } from "@/components/SignOutButton";
import { hasLocale, locales } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";

import "../globals.css";

// Single family; the wdth axis carries the condensed display role.
// see sources#archivo
const archivo = Archivo({
  subsets: ["latin"],
  axes: ["wdth"],
  variable: "--font-archivo",
  display: "swap",
});

export function generateStaticParams() {
  return locales.map((lang) => ({ lang }));
}

export async function generateMetadata({
  params,
}: LayoutProps<"/[lang]">): Promise<Metadata> {
  const { lang } = await params;
  if (!hasLocale(lang)) return {};
  const dict = await getDictionary(lang);
  return { title: dict.meta.title, description: dict.meta.description };
}

export default async function RootLayout({
  children,
  params,
}: LayoutProps<"/[lang]">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  const other = lang === "es" ? "en" : "es";

  return (
    <html lang={lang} className={archivo.variable}>
      <body>
        <a className="skip-link" href="#main">
          {dict.nav.skipToContent}
        </a>
        <header className="site-header">
          <nav aria-label={dict.nav.label}>
            <ul>
              <li>
                <Link href={`/${lang}`}>{dict.nav.home}</Link>
              </li>
              <li>
                <Link href={`/${lang}/search`}>{dict.nav.search}</Link>
              </li>
              <li>
                <Link href={`/${lang}/calculator`}>{dict.nav.calculator}</Link>
              </li>
              <li>
                <Link href={`/${lang}/assistant`}>{dict.nav.assistant}</Link>
              </li>
              <li>
                <Link href={`/${lang}/signin`}>{dict.nav.signIn}</Link>
              </li>
              <li>
                <SignOutButton lang={lang} label={dict.nav.signOut} />
              </li>
            </ul>
          </nav>
          <Suspense
            fallback={
              <a
                href={`/${other}`}
                hrefLang={other}
                lang={other}
                title={dict.nav.switchLanguageLabel}
              >
                {dict.nav.switchLanguage}
              </a>
            }
          >
            <LanguageToggle
              target={other}
              label={dict.nav.switchLanguage}
              title={dict.nav.switchLanguageLabel}
            />
          </Suspense>
        </header>
        <main id="main">{children}</main>
      </body>
    </html>
  );
}
