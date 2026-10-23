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
import { TextField } from "@/components/form/TextField";

const ONE_YEAR_SECONDS = 60 * 60 * 24 * 365;

type Field = "email" | "password";
type FieldErrors = Partial<Record<Field, string>>;

/** Which field an API error code belongs to; codes not listed go to the form alert. */
const FIELD_OF_CODE: Record<string, Field> = {
  invalid_email: "email",
  weak_password: "password",
  email_taken: "email",
};

/**
 * Sign-up or sign-in form in a centred card. On success goes to the
 * validated `?next=` page (in the account's language), else search;
 * onboarding comes first when it is unfinished. Field errors from the API
 * mark their field (`aria-invalid` plus an inline message); a wrong email or
 * password stays a form error that never says which one was wrong.
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
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({});
  // The switch link keeps the validated `?next=` so the detour survives.
  const next = safeNextPath(new URLSearchParams(useLocationSearch()).get(NEXT_PARAM)) ?? undefined;
  const signup = mode === "signup";
  const switchHref = signup ? signInPath(lang, next) : signUpPath(lang, next);

  const fieldMessages: Record<string, string> = {
    invalid_email: dict.errors.invalidEmail,
    weak_password: dict.errors.weakPassword,
    email_taken: dict.errors.emailTaken,
  };

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    setError(null);
    setFieldErrors({});
    const { data, error: failure, response } = signup
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
    const code = failure?.error;
    const field = code === undefined ? undefined : FIELD_OF_CODE[code];
    const fieldMessage = code === undefined ? undefined : fieldMessages[code];
    if (field !== undefined && fieldMessage !== undefined) {
      setFieldErrors({ [field]: fieldMessage });
      setError(signup ? dict.errors.signUpCheck : dict.errors.signInCheck);
      return;
    }
    setError(
      errorMessage(response.status, code, errors, {
        invalid_credentials: dict.errors.invalidCredentials,
      }),
    );
  }

  /** Editing a marked field clears its message; the server checks again on submit. */
  function clearField(field: Field) {
    setFieldErrors((current) => {
      if (current[field] === undefined) return current;
      const rest = { ...current };
      delete rest[field];
      return rest;
    });
  }

  return (
    <div className="card">
      <h1>{signup ? dict.signUpHeading : dict.signInHeading}</h1>
      <form className="stack" onSubmit={submit} noValidate>
        <TextField
          label={dict.email}
          type="email"
          name="email"
          autoComplete="email"
          required
          value={email}
          error={fieldErrors.email}
          onChange={(e) => {
            setEmail(e.target.value);
            clearField("email");
          }}
        />
        <TextField
          label={dict.password}
          type="password"
          name="password"
          autoComplete={signup ? "new-password" : "current-password"}
          required
          minLength={signup ? 8 : undefined}
          maxLength={128}
          hint={signup ? dict.passwordHint : undefined}
          value={password}
          error={fieldErrors.password}
          onChange={(e) => {
            setPassword(e.target.value);
            clearField("password");
          }}
        />
        <p role="alert" className="error">
          {error}
        </p>
        <button type="submit" disabled={pending}>
          {pending ? dict.submitting : signup ? dict.signUpSubmit : dict.signInSubmit}
        </button>
      </form>
      <p className="card__switch">
        <Link href={switchHref}>{signup ? dict.haveAccount : dict.noAccount}</Link>
      </p>
    </div>
  );
}
