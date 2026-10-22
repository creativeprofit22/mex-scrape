"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { SignOutButton } from "@/components/SignOutButton";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { activeNavItem, planLabel } from "@/lib/nav";
import { NEXT_PARAM, safeNextPath, signInPath, signUpPath } from "@/lib/ui";
import { useLocationSearch } from "@/lib/useLocationSearch";

import { useSession } from "./SessionProvider";

type Variant = "sidebar" | "header";

/**
 * Same box as the loaded menu, with no links: shown until `GET /me` answers
 * (and kept when it fails), so the frame never offers the wrong links.
 */
export function AccountMenuPlaceholder({ variant }: { variant: Variant }) {
  return <div className={`account-menu account-menu--${variant} account-menu--placeholder`} aria-hidden="true" />;
}

/**
 * The session part of the frame. Signed out: sign-in and sign-up, which
 * bring the user back to this page (or keep the `?next=` already in the URL).
 * Signed in: email and plan (sidebar only), the account page and sign-out.
 */
export function AccountMenu({
  variant,
  lang,
  nav,
  plans,
  errors,
}: {
  variant: Variant;
  lang: Locale;
  nav: Dictionary["nav"];
  plans: Dictionary["account"]["plans"];
  errors: Dictionary["common"]["errors"];
}) {
  const { session } = useSession();
  const pathname = usePathname();
  const search = useLocationSearch();
  const active = activeNavItem(pathname);
  const className = `account-menu account-menu--${variant}`;

  if (session.status === "loading" || session.status === "unknown") {
    return <AccountMenuPlaceholder variant={variant} />;
  }

  if (session.status === "signedOut") {
    const onAuthPage = active === "signIn" || active === "signUp";
    const next = onAuthPage
      ? safeNextPath(new URLSearchParams(search).get(NEXT_PARAM))
      : pathname === `/${lang}`
        ? null
        : safeNextPath(pathname + search);
    return (
      <div className={className}>
        <ul className="account-menu__links">
          <li>
            <Link
              href={signInPath(lang, next ?? undefined)}
              aria-current={active === "signIn" ? "page" : undefined}
            >
              {nav.signIn}
            </Link>
          </li>
          <li>
            <Link
              href={signUpPath(lang, next ?? undefined)}
              aria-current={active === "signUp" ? "page" : undefined}
            >
              {nav.signUp}
            </Link>
          </li>
        </ul>
      </div>
    );
  }

  const { me } = session;
  return (
    <div className={className}>
      {variant === "sidebar" ? (
        <p className="account-menu__who">
          <span className="account-menu__email" title={me.email}>
            {me.email}
          </span>
          <span className="account-menu__plan">{planLabel(me.plan, plans)}</span>
        </p>
      ) : null}
      <ul className="account-menu__links">
        <li>
          <Link
            href={`/${lang}/account`}
            aria-current={active === "account" ? "page" : undefined}
          >
            {nav.account}
          </Link>
        </li>
        <li>
          <SignOutButton lang={lang} label={nav.signOut} errors={errors} />
        </li>
      </ul>
    </div>
  );
}
