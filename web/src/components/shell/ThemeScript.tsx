"use client";

import { themeScript } from "@/lib/theme";

/**
 * The `<head>` script that applies a saved theme before paint (ADR 0015).
 * It runs from the server HTML only; on the client it is `text/plain`, so
 * React does not warn about rendering a script, and `suppressHydrationWarning`
 * accepts the type difference. see node_modules/next/dist/docs/01-app/02-guides/preventing-flash-before-hydration.md
 */
export function ThemeScript() {
  return (
    <script
      type={typeof window === "undefined" ? "text/javascript" : "text/plain"}
      suppressHydrationWarning
      dangerouslySetInnerHTML={{ __html: themeScript }}
    />
  );
}
