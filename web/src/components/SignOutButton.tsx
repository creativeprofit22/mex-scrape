"use client";

import { useRouter } from "next/navigation";

import type { Locale } from "@/i18n/config";
import { api } from "@/lib/api/client";

/** Ends the session, then goes to the sign-in page. */
export function SignOutButton({ lang, label }: { lang: Locale; label: string }) {
  const router = useRouter();

  async function signOut() {
    await api.POST("/auth/signout").catch(() => undefined);
    router.push(`/${lang}/signin`);
    router.refresh();
  }

  return (
    <button type="button" className="link-button" onClick={signOut}>
      {label}
    </button>
  );
}
