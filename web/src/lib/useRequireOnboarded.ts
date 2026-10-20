import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import type { Locale } from "@/i18n/config";
import { api } from "@/lib/api/client";
import { errorMessage, signInPath, type CommonErrors } from "@/lib/ui";

/** Outcome of the onboarding check for a gated page. */
export type OnboardedGate =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; message: string };

/**
 * Gates a page on finished onboarding: reads `GET /me` and sends a signed-out
 * user to sign-in and a not-yet-onboarded one to `/[lang]/onboarding`, so
 * membership offers are never hidden from someone who was never asked.
 * Stays `loading` while redirecting; a failed check is an `error`, not a pass.
 */
export function useRequireOnboarded(lang: Locale, errors: CommonErrors): OnboardedGate {
  const router = useRouter();
  const [gate, setGate] = useState<OnboardedGate>({ status: "loading" });

  useEffect(() => {
    let active = true;
    void api.GET("/me").then(({ data, error, response }) => {
      if (!active) return;
      if (response.status === 401) {
        router.replace(signInPath(lang));
        return;
      }
      if (!data) {
        setGate({
          status: "error",
          message: errorMessage(response.status, error?.error, errors),
        });
        return;
      }
      if (!data.onboarded) {
        router.replace(`/${lang}/onboarding`);
        return;
      }
      setGate({ status: "ready" });
    });
    return () => {
      active = false;
    };
  }, [lang, router, errors]);

  return gate;
}
