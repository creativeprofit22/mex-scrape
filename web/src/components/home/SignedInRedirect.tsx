"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";

import { useSession } from "@/components/shell/SessionProvider";
import type { Locale } from "@/i18n/config";
import { onboardingPath } from "@/lib/ui";

/**
 * Sends a signed-in visitor from home straight to search (or onboarding when
 * they haven't finished it), style-pack §1a. `replace`, so Back never lands
 * on home again. Loading, signed-out and unknown sessions keep the home
 * content, so a signed-out visitor sees no flash. Renders nothing.
 */
export function SignedInRedirect({ lang }: { lang: Locale }) {
  const { session } = useSession();
  const router = useRouter();

  useEffect(() => {
    if (session.status !== "signedIn") return;
    router.replace(session.me.onboarded ? `/${lang}/search` : onboardingPath(lang));
  }, [session, router, lang]);

  return null;
}
