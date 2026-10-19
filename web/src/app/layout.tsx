import type { Metadata } from "next";

import "./globals.css";

export const metadata: Metadata = {
  title: "mex-scrape",
  description:
    "Compara precios de tiendas mexicanas con Amazon.com.mx y calcula tu ROI.",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="es">
      <body>{children}</body>
    </html>
  );
}
