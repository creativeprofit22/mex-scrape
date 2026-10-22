"use client";

import { useRouter } from "next/navigation";
import { useEffect, useState, type FormEvent } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { LoadError } from "@/components/LoadError";
import { api } from "@/lib/api/client";
import { errorMessage, nextFromLocation } from "@/lib/ui";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

/**
 * Store memberships (Costco, Sam's Club); offers needing others are hidden.
 * Saving goes on to `?next=` (the page that sent the user here) or search.
 */
export function OnboardingForm({
  lang,
  dict,
  common,
}: {
  lang: Locale;
  dict: Dictionary["onboarding"];
  common: Dictionary["common"];
}) {
  const router = useRouter();
  const toSignIn = useSignInRedirect(lang);
  const [loaded, setLoaded] = useState(false);
  // A failed load must not show the form: saving its unticked boxes would
  // overwrite memberships the user already has.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [costco, setCostco] = useState(false);
  const [sams, setSams] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void api.GET("/me").then(({ data, error: failure, response }) => {
      if (!active) return;
      if (response.status === 401) {
        toSignIn();
        return;
      }
      if (!data) {
        setLoadError(errorMessage(response.status, failure?.error, common.errors));
        return;
      }
      setCostco(data.has_costco);
      setSams(data.has_sams);
      setLoaded(true);
    });
    return () => {
      active = false;
    };
  }, [toSignIn, common.errors, attempt]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    setError(null);
    const { data, error: failure, response } = await api.PUT("/me/onboarding", {
      body: { has_costco: costco, has_sams: sams },
    });
    if (data) {
      router.push(nextFromLocation() ?? `/${lang}/search`);
      return;
    }
    if (response.status === 401) {
      toSignIn();
      return;
    }
    setPending(false);
    setError(errorMessage(response.status, failure?.error, common.errors));
  }

  if (loadError) {
    return (
      <LoadError
        message={loadError}
        retryLabel={common.retry}
        onRetry={() => {
          setLoadError(null);
          setAttempt((n) => n + 1);
        }}
      />
    );
  }
  if (!loaded) {
    return <p role="status">{common.loading}</p>;
  }
  return (
    <>
      <h1>{dict.heading}</h1>
      <p>{dict.intro}</p>
      <form className="stack" onSubmit={submit}>
        <fieldset>
          <legend>{dict.legend}</legend>
          <label className="check">
            <input
              type="checkbox"
              name="has_costco"
              checked={costco}
              onChange={(e) => setCostco(e.target.checked)}
            />
            {dict.costco}
          </label>
          <label className="check">
            <input
              type="checkbox"
              name="has_sams"
              checked={sams}
              onChange={(e) => setSams(e.target.checked)}
            />
            {dict.sams}
          </label>
        </fieldset>
        <p role="alert" className="error">
          {error}
        </p>
        <button type="submit" disabled={pending}>
          {pending ? dict.saving : dict.submit}
        </button>
      </form>
    </>
  );
}
