"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatBasisPoints, formatMoney } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { errorMessage, pesosToCentavos, signInPath } from "@/lib/ui";

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
  const router = useRouter();
  const [values, setValues] = useState<Record<Field, string>>({
    sellPrice: "",
    buyPrice: "",
    fees: "",
    prep: "",
    shipping: "",
    monthlyFixed: "",
    unitsPerMonth: "",
  });
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
      router.replace(signInPath(lang));
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
            <input
              name={field}
              inputMode={field === "unitsPerMonth" ? "numeric" : "decimal"}
              required={REQUIRED.has(field)}
              value={values[field]}
              onChange={(e) =>
                setValues((old) => ({ ...old, [field]: e.target.value }))
              }
            />
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
