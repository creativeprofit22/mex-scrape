"use client";

import { useState, useSyncExternalStore, type FormEvent } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatBasisPoints, formatMoney } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { errorMessage, pesosToCentavos } from "@/lib/ui";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

type Field =
  | "sellPrice"
  | "buyPrice"
  | "fees"
  | "prep"
  | "shipping"
  | "monthlyFixed"
  | "unitsPerMonth";

const FIELDS: readonly Field[] = [
  "sellPrice",
  "buyPrice",
  "fees",
  "prep",
  "shipping",
  "monthlyFixed",
  "unitsPerMonth",
];
const REQUIRED = new Set<Field>(["sellPrice", "buyPrice", "fees"]);
const EMPTY: Record<Field, string> = {
  sellPrice: "",
  buyPrice: "",
  fees: "",
  prep: "",
  shipping: "",
  monthlyFixed: "",
  unitsPerMonth: "",
};

/** Tab-scoped store for typed values while the user signs in. */
const DRAFT_KEY = "mex-scrape:calculator-draft";

/** Keeps the typed values for this tab; a failure only loses the draft. */
function saveDraft(values: Record<Field, string>): void {
  try {
    sessionStorage.setItem(DRAFT_KEY, JSON.stringify(values));
  } catch {
    // Storage disabled or full: nothing to keep.
  }
}

/** Drops the saved draft once the user is typing again. */
function clearDraft(): void {
  try {
    sessionStorage.removeItem(DRAFT_KEY);
  } catch {
    // Storage disabled: there is no draft.
  }
}

/** The saved draft as stored (a string, so React can compare snapshots). */
function readDraft(): string | null {
  try {
    return sessionStorage.getItem(DRAFT_KEY);
  } catch {
    return null;
  }
}

/** Nothing else writes the draft while the page is open. */
function subscribeNever(): () => void {
  return () => undefined;
}

/** Parses a saved draft; anything malformed is ignored. */
function parseDraft(raw: string | null): Record<Field, string> | null {
  if (raw === null) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const record = parsed as Record<string, unknown>;
  const draft = {} as Record<Field, string>;
  for (const field of FIELDS) {
    const value = record[field];
    if (typeof value !== "string") return null;
    draft[field] = value;
  }
  return draft;
}

/** One-product cost calculator over `POST /roi` (same maths as search). */
export function Calculator({
  lang,
  dict,
  common,
}: {
  lang: Locale;
  dict: Dictionary["calculator"];
  common: Dictionary["common"];
}) {
  const toSignIn = useSignInRedirect(lang);
  // Values typed before a sign-in detour come back from sessionStorage; the
  // server snapshot is null so hydration matches, then the draft shows.
  const draft = useSyncExternalStore(subscribeNever, readDraft, () => null);
  const [edited, setEdited] = useState<Record<Field, string> | null>(null);
  const values = edited ?? parseDraft(draft) ?? EMPTY;
  const [result, setResult] = useState<Schemas["RoiItemResponse"] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  function parse(): Schemas["RoiItemRequest"] | null {
    const sell = pesosToCentavos(values.sellPrice);
    const buy = pesosToCentavos(values.buyPrice);
    const fees = pesosToCentavos(values.fees);
    const prep = pesosToCentavos(values.prep, true);
    const shipping = pesosToCentavos(values.shipping, true);
    const fixed = pesosToCentavos(values.monthlyFixed, true);
    const unitsText = values.unitsPerMonth.trim();
    const units = unitsText === "" ? 0 : Number(unitsText);
    if (
      sell === null ||
      buy === null ||
      fees === null ||
      prep === null ||
      shipping === null ||
      fixed === null ||
      !Number.isInteger(units) ||
      units < 0
    ) {
      return null;
    }
    return {
      sell_price_centavos: sell,
      buy_price_centavos: buy,
      amazon_fees_centavos: fees,
      prep_fee_centavos: prep,
      shipping_centavos: shipping,
      monthly_fixed_centavos: fixed,
      units_per_month: units,
    };
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    // A restored draft becomes ordinary state now; a new 401 saves it again.
    setEdited(values);
    clearDraft();
    setResult(null);
    const item = parse();
    if (item === null) {
      setError(dict.errors.invalidAmount);
      return;
    }
    setError(null);
    setPending(true);
    const { data, error: failure, response } = await api.POST("/roi", {
      body: { items: [item] },
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
        errorMessage(response.status, failure?.error, common.errors, {
          invalid_amount: dict.errors.invalidAmount,
        }),
      );
      return;
    }
    if (first.error) {
      setError(
        first.error === "zero_units" || first.error === "overflow"
          ? dict.errors[first.error]
          : common.errors.generic,
      );
      return;
    }
    setResult(first);
  }

  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? common.none : formatMoney(v, lang);
  const percent = (v: number | null | undefined) =>
    v === null || v === undefined ? common.none : formatBasisPoints(v, lang);

  return (
    <>
      <h1>{dict.heading}</h1>
      <p>{dict.intro}</p>
      <form className="stack" onSubmit={submit} noValidate>
        {FIELDS.map((field) => (
          <label key={field}>
            {dict[field]}
            <span className="field">
              <input
                name={field}
                inputMode={field === "unitsPerMonth" ? "numeric" : "decimal"}
                required={REQUIRED.has(field)}
                value={values[field]}
                onChange={(e) => {
                  clearDraft();
                  setEdited({ ...values, [field]: e.target.value });
                }}
              />
            </span>
          </label>
        ))}
        <p role="alert" className="error">
          {error}
        </p>
        <button type="submit" disabled={pending}>
          {dict.submit}
        </button>
      </form>
      <section aria-live="polite" aria-labelledby="calc-result">
        {result ? (
          <>
            <h2 id="calc-result">{dict.resultHeading}</h2>
            <dl className="facts">
              <dt>{dict.profit}</dt>
              <dd data-testid="calc-profit">{money(result.profit_centavos)}</dd>
              <dt>{dict.roi}</dt>
              <dd data-testid="calc-roi">{percent(result.roi_bp)}</dd>
              <dt>{dict.margin}</dt>
              <dd>{percent(result.margin_bp)}</dd>
              <dt>{dict.totalCost}</dt>
              <dd>{money(result.total_cost_centavos)}</dd>
              <dt>{dict.fixedPerUnit}</dt>
              <dd>{money(result.fixed_cost_per_unit_centavos)}</dd>
            </dl>
          </>
        ) : null}
      </section>
    </>
  );
}
