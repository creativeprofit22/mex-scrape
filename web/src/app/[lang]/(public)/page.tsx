import Link from "next/link";
import { notFound } from "next/navigation";

import { SignedInRedirect } from "@/components/home/SignedInRedirect";
import { hasLocale } from "@/i18n/config";
import { getDictionary } from "@/i18n/dictionaries";
import { signInPath, signUpPath } from "@/lib/ui";

/** Signed-out home: what the product does in three steps, then sign up or sign in. */
export default async function Home({ params }: PageProps<"/[lang]">) {
  const { lang } = await params;
  if (!hasLocale(lang)) notFound();
  const dict = await getDictionary(lang);
  const { steps } = dict.home;

  return (
    <div className="home">
      <SignedInRedirect lang={lang} />
      <h1>{dict.home.heading}</h1>
      <p className="home__intro">{dict.home.intro}</p>
      <h2 id="home-steps">{dict.home.stepsHeading}</h2>
      <ol className="home-steps" aria-labelledby="home-steps">
        {[steps.search, steps.compare, steps.profit].map((step, i) => (
          <li key={step.title} className="home-steps__item">
            <span className="home-steps__number" aria-hidden="true">
              {i + 1}
            </span>
            <p className="home-steps__title">{step.title}</p>
            <p className="home-steps__body">{step.body}</p>
          </li>
        ))}
      </ol>
      <p className="actions">
        <Link className="button" href={signUpPath(lang)}>
          {dict.home.createAccount}
        </Link>
        <Link href={signInPath(lang)}>{dict.home.signIn}</Link>
      </p>
    </div>
  );
}
