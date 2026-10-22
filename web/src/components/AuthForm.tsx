"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { localeCookie, localizedPath, type Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { useSession } from "@/components/shell/SessionProvider";
import { api } from "@/lib/api/client";
import {
  errorMessage,
  NEXT_PARAM,
  nextFromLocation,
  onboardingPath,
  safeNextPath,
  signInPath,
  signUpPath,
} from "@/lib/ui";
import { useLocationSearch } from "@/lib/useLocationSearch";

const ONE_YEAR_SECONDS = 60 * 60 * 24 * 365;

/**
 * Sign-up or sign-in form. On success goes to the validated `?next=` page
 * (in the account's language), else search; onboarding comes first when it
 * is unfinished.
 */
export function AuthForm({
  mode,
  lang,
  dict,
  errors,
}: {
  mode: "signup" | "signin";
  lang: Locale;
  dict: Dictionary["auth"];
  errors: Dictionary["common"]["errors"];
}) {
  const router = useRouter();
  const { refresh } = useSession();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The switch link keeps the validated `?next=` so the detour survives.
  const next = safeNextPath(new URLSearchParams(useLocationSearch()).get(NEXT_PARAM)) ?? undefined;
  const switchHref = mode === "signup" ? signInPath(lang, next) : signUpPath(lang, next);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    setError(null);
    const { data, error: failure, response } =
      mode === "signup"
        ? await api.POST("/auth/signup", {
            body: { email, password, locale: lang },
          })
        : await api.POST("/auth/signin", { body: { email, password } });
    if (data) {
      const locale = data.user.locale === "en" ? "en" : "es";
      document.cookie = `${localeCookie}=${locale}; path=/; max-age=${ONE_YEAR_SECONDS}; samesite=lax`;
      const saved = nextFromLocation();
      const next = saved === null ? undefined : localizedPath(saved, locale);
      // The frame read `/me` while signed out; re-read it for the new session.
      refresh();
      router.push(
        data.user.onboarded ? (next ?? `/${locale}/search`) : onboardingPath(locale, next),
      );
      return;
    }
    setPending(false);
    setError(
      errorMessage(response.status, failure?.error, errors, {
        invalid_email: dict.errors.invalidEmail,
        weak_password: dict.errors.weakPassword,
        email_taken: dict.errors.emailTaken,
        invalid_credentials: dict.errors.invalidCredentials,
      }),
    );
  }

  const signup = mode === "signup";
  return (
    <>
      <h1>{signup ? dict.signUpHeading : dict.signInHeading}</h1>
      <form className="stack" onSubmit={submit} noValidate>
        <label>
          {dict.email}
          <span className="field">
            <input
              type="email"
              name="email"
              autoComplete="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </span>
        </label>
        <label>
          {dict.password}
          <span className="field">
            <input
              type="password"
              name="password"
              autoComplete={signup ? "new-password" : "current-password"}
              required
              minLength={signup ? 8 : undefined}
              maxLength={128}
              aria-describedby={signup ? "password-hint" : undefined}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </span>
        </label>
        {signup ? (
          <p id="password-hint" className="hint">
            {dict.passwordHint}
          </p>
        ) : null}
        <p role="alert" className="error">
          {error}
        </p>
        <button type="submit" disabled={pending}>
          {pending
            ? dict.submitting
            : signup
              ? dict.signUpSubmit
              : dict.signInSubmit}
        </button>
      </form>
      <p>
        <Link href={switchHref}>
          {signup ? dict.haveAccount : dict.noAccount}
        </Link>
      </p>
    </>
  );
}
