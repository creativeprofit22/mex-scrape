"use client";

import { useEffect, useRef, useState, useSyncExternalStore, type FormEvent } from "react";

import { TextField } from "@/components/form/TextField";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import {
  clearDraft,
  COST_FIELDS,
  EMPTY,
  parseDraft,
  parseInputs,
  PRICE_FIELDS,
  readDraft,
  REQUIRED,
  saveDraft,
  subscribeNever,
  type Field,
  type Values,
} from "@/lib/calculator/draft";
import type { SignGlyphs } from "@/lib/search/format";
import { centavosToPesosInput } from "@/lib/search/storage";
import { useCosts } from "@/lib/search/useSearchStore";
import { errorMessage } from "@/lib/ui";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

import { CalculatorResult } from "./CalculatorResult";

/**
 * One-product cost calculator over `POST /roi` (same maths as search):
 * inputs beside a sticky result on wide workspaces, stacked otherwise.
 */
export function Calculator({
  lang,
  dict,
  common,
  sign,
}: {
  lang: Locale;
  dict: Dictionary["calculator"];
  common: Dictionary["common"];
  sign: SignGlyphs & { down: string };
}) {
  const toSignIn = useSignInRedirect(lang);
  // Values typed before a sign-in detour come back from sessionStorage; the
  // server snapshot is null so hydration matches, then the draft shows.
  const draft = useSyncExternalStore(subscribeNever, readDraft, () => null);
  const [edited, setEdited] = useState<Values | null>(null);
  // With no draft, prep and shipping start from the costs saved on search.
  const [savedCosts] = useCosts();
  const values =
    edited ??
    parseDraft(draft) ??
    (savedCosts
      ? {
          ...EMPTY,
          prep: centavosToPesosInput(savedCosts.prep),
          shipping: centavosToPesosInput(savedCosts.shipping),
        }
      : EMPTY);
  const [result, setResult] = useState<Schemas["RoiItemResponse"] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [fieldErrors, setFieldErrors] = useState<Partial<Record<Field, string>>>({});
  const [pending, setPending] = useState(false);
  const resultRef = useRef<HTMLElement>(null);

  // Stacked (phones), the result sits under the form: bring it into view.
  // Beside the form it is sticky and already visible, so nothing moves.
  useEffect(() => {
    if (result) resultRef.current?.scrollIntoView({ block: "nearest", behavior: "auto" });
  }, [result]);

  /** Marks fields and moves focus to the first, so the fix starts there. */
  function markInvalid(form: HTMLFormElement, marked: Partial<Record<Field, string>>, message: string) {
    setFieldErrors(marked);
    setError(message);
    const first = Object.keys(marked)[0];
    if (first !== undefined) {
      const input = form.elements.namedItem(first);
      if (input instanceof HTMLInputElement) input.focus();
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    // A restored draft becomes ordinary state now; a new 401 saves it again.
    setEdited(values);
    clearDraft();
    setResult(null);
    const parsed = parseInputs(values);
    if ("invalid" in parsed) {
      const marked: Partial<Record<Field, string>> = {};
      for (const [field, problem] of parsed.invalid) marked[field] = dict.fieldErrors[problem];
      markInvalid(form, marked, dict.errors.invalidFields);
      return;
    }
    setError(null);
    setFieldErrors({});
    setPending(true);
    const { data, error: failure, response } = await api.POST("/roi", {
      body: { items: [parsed.item] },
    });
    setPending(false);
    if (response.status === 401) {
      saveDraft(values);
      toSignIn();
      return;
    }
    const first = data?.results[0];
    if (!first) {
      setError(
        failure?.error === "invalid_amount"
          ? dict.errors.invalidAmount
          : interpolate(dict.errors.failed, {
              reason: errorMessage(response.status, failure?.error, common.errors),
            }),
      );
      return;
    }
    if (first.error === "zero_units") {
      markInvalid(form, { unitsPerMonth: dict.fieldErrors.zeroUnits }, dict.errors.zero_units);
      return;
    }
    if (first.error) {
      setError(
        first.error === "overflow"
          ? dict.errors.overflow
          : interpolate(dict.errors.failed, { reason: common.errors.generic }),
      );
      return;
    }
    setResult(first);
  }

  function field(name: Field) {
    return (
      <TextField
        key={name}
        label={dict[name]}
        name={name}
        inputMode={name === "unitsPerMonth" ? "numeric" : "decimal"}
        autoComplete="off"
        required={REQUIRED.has(name)}
        value={values[name]}
        error={fieldErrors[name]}
        onChange={(e) => {
          clearDraft();
          setEdited({ ...values, [name]: e.target.value });
          if (fieldErrors[name]) {
            setFieldErrors((current) => {
              const rest = { ...current };
              delete rest[name];
              return rest;
            });
          }
        }}
      />
    );
  }

  return (
    <div className="calculator">
      <h1>{dict.heading}</h1>
      <p className="calculator__intro">{dict.intro}</p>
      <div className="calculator__layout">
        <form className="calculator__form" onSubmit={submit} noValidate>
          <fieldset>
            <legend>{dict.pricesLegend}</legend>
            {PRICE_FIELDS.map(field)}
          </fieldset>
          <fieldset>
            <legend>{dict.costsLegend}</legend>
            {COST_FIELDS.map(field)}
          </fieldset>
          <p role="alert" className="error">
            {error}
          </p>
          <button type="submit" disabled={pending} aria-busy={pending || undefined}>
            {pending ? dict.calculating : dict.submit}
          </button>
        </form>
        <CalculatorResult
          ref={resultRef}
          result={result}
          pending={pending}
          lang={lang}
          dict={dict}
          none={common.none}
          sign={sign}
        />
      </div>
    </div>
  );
}
