import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import { useSession } from "@/components/shell/SessionProvider";
import type { Locale } from "@/i18n/config";
import { api } from "@/lib/api/client";
import { currentPath, errorMessage, onboardingPath, type CommonErrors } from "@/lib/ui";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

/** Outcome of the onboarding check for a gated page. */
export type OnboardedGate =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; message: string; retry: () => void };

type GateState =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; message: string };

/**
 * Gates a page on finished onboarding: reads `GET /me` and sends a signed-out
 * user to sign-in and a not-yet-onboarded one to `/[lang]/onboarding`, so
 * membership offers are never hidden from someone who was never asked. Both
 * come back to this page afterwards. Stays `loading` while redirecting; a
 * failed check is an `error` with a `retry`, not a pass. A 401 and a retry
 * also re-read the frame's session, so its links match what the page learnt.
 */
export function useRequireOnboarded(lang: Locale, errors: CommonErrors): OnboardedGate {
  const router = useRouter();
  const { refresh } = useSession();
  const toSignIn = useSignInRedirect(lang);
  const [gate, setGate] = useState<GateState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    void api.GET("/me").then(({ data, error, response }) => {
      if (!active) return;
      if (response.status === 401) {
        toSignIn();
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
        router.replace(onboardingPath(lang, currentPath()));
        return;
      }
      setGate({ status: "ready" });
    });
    return () => {
      active = false;
    };
  }, [lang, router, toSignIn, errors, attempt]);

  if (gate.status === "error") {
    return {
      ...gate,
      retry: () => {
        setGate({ status: "loading" });
        setAttempt((n) => n + 1);
        refresh();
      },
    };
  }
  return gate;
}
