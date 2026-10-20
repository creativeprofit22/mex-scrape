import { NextResponse, type NextRequest } from "next/server";

import {
  defaultLocale,
  hasLocale,
  localeCookie,
  localizedPath,
} from "@/i18n/config";

/**
 * Sends paths without a language segment to the saved language (cookie set
 * by the language toggle and at sign-in), else Spanish.
 */
export function proxy(request: NextRequest) {
  const { pathname } = request.nextUrl;
  if (hasLocale(pathname.split("/")[1])) return;

  const saved = request.cookies.get(localeCookie)?.value;
  const lang = hasLocale(saved) ? saved : defaultLocale;
  const url = request.nextUrl.clone();
  url.pathname = localizedPath(pathname, lang);
  return NextResponse.redirect(url);
}

export const config = {
  // Skip Next internals, the API rewrite and files with an extension.
  matcher: ["/((?!_next|backend|.*\\..*).*)"],
};
