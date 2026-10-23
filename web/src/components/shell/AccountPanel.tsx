"use client";

import Link from "next/link";

import { LoadError } from "@/components/feedback/LoadError";
import { useSession } from "@/components/shell/SessionProvider";
import { SignOutButton } from "@/components/shell/SignOutButton";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { planLabel } from "@/lib/nav";
import { errorMessage, signInPath, signUpPath } from "@/lib/ui";

/**
 * The account page (the phone Cuenta tab): email, plan, language and sign-out
 * when signed in; sign-in and sign-up when signed out. `language` is the
 * frame's language switch.
 */
export function AccountPanel({
  lang,
  dict,
  nav,
  common,
  language,
}: {
  lang: Locale;
  dict: Dictionary["account"];
  nav: Dictionary["nav"];
  common: Dictionary["common"];
  language: React.ReactNode;
}) {
  const { session, refresh } = useSession();
  const next = `/${lang}/account`;

  return (
    <>
      <h1>{dict.heading}</h1>
      {session.status === "loading" ? (
        <p role="status">{common.loading}</p>
      ) : session.status === "unknown" ? (
        <LoadError
          message={errorMessage(session.httpStatus, session.code, common.errors)}
          retryLabel={common.retry}
          onRetry={refresh}
        />
      ) : session.status === "signedOut" ? (
        <>
          <p>{dict.signedOutIntro}</p>
          <p className="actions">
            <Link className="button" href={signInPath(lang, next)}>
              {nav.signIn}
            </Link>
            <Link href={signUpPath(lang, next)}>{nav.signUp}</Link>
          </p>
          <dl className="account-facts">
            <dt>{dict.language}</dt>
            <dd>{language}</dd>
          </dl>
        </>
      ) : (
        <>
          <dl className="account-facts">
            <dt>{dict.email}</dt>
            <dd>{session.me.email}</dd>
            <dt>{dict.plan}</dt>
            <dd>{planLabel(session.me.plan, dict.plans)}</dd>
            <dt>{dict.language}</dt>
            <dd>{language}</dd>
          </dl>
          <div className="account-actions">
            <SignOutButton lang={lang} label={nav.signOut} errors={common.errors} />
          </div>
        </>
      )}
    </>
  );
}
