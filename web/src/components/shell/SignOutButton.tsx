"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { useSession } from "@/components/shell/SessionProvider";
import type { Locale } from "@/i18n/config";
import { api } from "@/lib/api/client";
import { errorMessage, type CommonErrors } from "@/lib/ui";

/**
 * Ends the session, then goes to the sign-in page and re-reads the session.
 * When the API does not confirm it (CSRF, rate limit, server down, offline),
 * the user stays on the page with the error, since the session is still alive.
 */
export function SignOutButton({
  lang,
  label,
  errors,
}: {
  lang: Locale;
  label: string;
  errors: CommonErrors;
}) {
  const router = useRouter();
  const { refresh } = useSession();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  async function signOut() {
    setPending(true);
    setError("");
    try {
      const { response, error: failure } = await api.POST("/auth/signout");
      // 401: the session was already gone, which is what the user asked for.
      if (response.ok || response.status === 401) {
        refresh();
        router.push(`/${lang}/signin`);
        router.refresh();
        return;
      }
      setError(errorMessage(response.status, failure?.error, errors));
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <button type="button" className="link-button" onClick={signOut} disabled={pending}>
        {label}
      </button>
      <p role="alert" className="error">
        {error}
      </p>
    </>
  );
}
