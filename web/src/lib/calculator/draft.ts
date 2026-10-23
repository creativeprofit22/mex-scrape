import type { Schemas } from "@/lib/api/client";
import { pesosToCentavos } from "@/lib/ui";

/** One calculator input, named as in the form and the catalog. */
export type Field =
  | "sellPrice"
  | "buyPrice"
  | "fees"
  | "prep"
  | "shipping"
  | "monthlyFixed"
  | "unitsPerMonth";

export type Values = Record<Field, string>;

/** Prices from the stores; then the seller's own costs. */
export const PRICE_FIELDS: readonly Field[] = ["sellPrice", "buyPrice", "fees"];
export const COST_FIELDS: readonly Field[] = ["prep", "shipping", "monthlyFixed", "unitsPerMonth"];
const FIELDS: readonly Field[] = [...PRICE_FIELDS, ...COST_FIELDS];

/** Fields the calculation cannot run without. */
export const REQUIRED: ReadonlySet<Field> = new Set<Field>(PRICE_FIELDS);

export const EMPTY: Values = {
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
export function saveDraft(values: Values): void {
  try {
    sessionStorage.setItem(DRAFT_KEY, JSON.stringify(values));
  } catch {
    // Storage disabled or full: nothing to keep.
  }
}

/** Drops the saved draft once the user is typing again. */
export function clearDraft(): void {
  try {
    sessionStorage.removeItem(DRAFT_KEY);
  } catch {
    // Storage disabled: there is no draft.
  }
}

/** The saved draft as stored (a string, so React can compare snapshots). */
export function readDraft(): string | null {
  try {
    return sessionStorage.getItem(DRAFT_KEY);
  } catch {
    return null;
  }
}

/** Nothing else writes the draft while the page is open. */
export function subscribeNever(): () => void {
  return () => undefined;
}

/** Parses a saved draft; anything malformed is ignored. */
export function parseDraft(raw: string | null): Values | null {
  if (raw === null) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const record = parsed as Record<string, unknown>;
  const draft = { ...EMPTY };
  for (const field of FIELDS) {
    const value = record[field];
    if (typeof value !== "string") return null;
    draft[field] = value;
  }
  return draft;
}

/** Why a field was rejected: left empty, or not a valid amount. */
export type Problem = "required" | "amount" | "units";

/** `units_per_month` is a u32 in the api; anything larger is a 422. */
const MAX_UNITS = 4_294_967_295;

/**
 * The `POST /roi` item for the typed values, or every field that is
 * missing or malformed (in form order) so each can be marked.
 */
export function parseInputs(
  values: Values,
): { item: Schemas["RoiItemRequest"] } | { invalid: [Field, Problem][] } {
  const invalid: [Field, Problem][] = [];
  const money = (field: Field): number => {
    const optional = !REQUIRED.has(field);
    const parsed = pesosToCentavos(values[field], optional);
    if (parsed === null) {
      invalid.push([field, values[field].trim() === "" ? "required" : "amount"]);
      return 0;
    }
    // Past this the centavos lose float precision (and near i64 the api rejects them).
    if (!Number.isSafeInteger(parsed)) {
      invalid.push([field, "amount"]);
      return 0;
    }
    return parsed;
  };
  const sell = money("sellPrice");
  const buy = money("buyPrice");
  const fees = money("fees");
  const prep = money("prep");
  const shipping = money("shipping");
  const fixed = money("monthlyFixed");
  const unitsText = values.unitsPerMonth.trim();
  const units = unitsText === "" ? 0 : Number(unitsText);
  if (!/^\d*$/.test(unitsText) || !Number.isSafeInteger(units) || units > MAX_UNITS) {
    invalid.push(["unitsPerMonth", "units"]);
  }
  if (invalid.length > 0) return { invalid };
  return {
    item: {
      sell_price_centavos: sell,
      buy_price_centavos: buy,
      amazon_fees_centavos: fees,
      prep_fee_centavos: prep,
      shipping_centavos: shipping,
      monthly_fixed_centavos: fixed,
      units_per_month: units,
    },
  };
}
