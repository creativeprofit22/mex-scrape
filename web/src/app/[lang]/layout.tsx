import type { Metadata, Viewport } from "next";
import { Archivo } from "next/font/google";
import { notFound } from "next/navigation";

import { SessionProvider } from "@/components/shell/SessionProvider";
import { ThemeScript } from "@/components/shell/ThemeScript";
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

// viewport-fit=cover makes env(safe-area-inset-*) non-zero, so the bottom tabs
// can pad past the home indicator. Zoom stays enabled. see sources#next-viewport
export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  viewportFit: "cover",
};

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

/**
 * Document root. The frame (`(app)` shell or `(public)` header) comes from the
 * route group's layout; each renders `<main id="main">` for the skip link.
 * The session lives here so both frames share one `GET /me`. The head script
 * applies a saved light/dark choice before paint; it sets `data-theme` on
 * `<html>`, hence `suppressHydrationWarning` there (ADR 0015).
 */
export default async function RootLayout({
  children,
  params,
}: LayoutProps<"/[lang]">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);

  return (
    <html lang={lang} className={archivo.variable} suppressHydrationWarning>
      <head>
        <ThemeScript />
      </head>
      <body>
        <a className="skip-link" href="#main">
          {dict.nav.skipToContent}
        </a>
        <SessionProvider>{children}</SessionProvider>
      </body>
    </html>
  );
}
