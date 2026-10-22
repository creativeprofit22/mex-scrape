"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState, type FormEvent } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatBasisPoints, formatMoney, interpolate } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { LoadError } from "@/components/LoadError";
import { currentPath, errorMessage, pesosToCentavos, retailerName, signInPath } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";

/** Delay between `GET /searches/{id}` polls. */
const POLL_MS = 1_500;
/** Give up polling after this long. */
const POLL_LIMIT_MS = 180_000;
const FINISHED = new Set(["done", "partial", "failed", "refused"]);
/** Most items one `POST /roi` accepts; mirrors `MAX_ROI_BATCH` in crates/app/src/roi.rs. */
const MAX_ROI_BATCH = 100;

type Search = Schemas["SearchResponse"];
type Roi = Schemas["RoiItemResponse"];

/**
 * Search bar, polling and the results table. Profit and ROI come from one
 * `POST /roi` batch (the calculator's endpoint) using the user's prep and
 * shipping costs, so both pages share the same maths. Users who have not
 * finished onboarding are sent there first (memberships decide what shows).
 */
export function SearchPanel({
  lang,
  dict,
  common,
  retailers,
}: {
  lang: Locale;
  dict: Dictionary["search"];
  common: Dictionary["common"];
  retailers: Dictionary["retailers"];
}) {
  const router = useRouter();
  const gate = useRequireOnboarded(lang, common.errors);
  const [query, setQuery] = useState("");
  const [prep, setPrep] = useState("");
  const [shipping, setShipping] = useState("");
  const [search, setSearch] = useState<Search | null>(null);
  const [timedOut, setTimedOut] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [roi, setRoi] = useState<Map<number, Roi>>(new Map());
  const [roiError, setRoiError] = useState<string | null>(null);
  const pollRef = useRef(0);

  useEffect(
    () => () => {
      pollRef.current += 1;
    },
    [],
  );

  function unauthorized(status: number): boolean {
    if (status !== 401) return false;
    router.replace(signInPath(lang, currentPath()));
    return true;
  }

  async function poll(id: number, run: number, startedAt: number) {
    while (pollRef.current === run) {
      const { data, error: failure, response } = await api.GET("/searches/{id}", {
        params: { path: { id } },
      });
      if (pollRef.current !== run) return;
      if (unauthorized(response.status)) return;
      if (!data) {
        setError(errorMessage(response.status, failure?.error, common.errors));
        setPending(false);
        return;
      }
      setSearch(data);
      if (FINISHED.has(data.status)) {
        setPending(false);
        return;
      }
      if (Date.now() - startedAt > POLL_LIMIT_MS) {
        setTimedOut(true);
        setPending(false);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const run = ++pollRef.current;
    setError(null);
    setTimedOut(false);
    setSearch(null);
    setRoi(new Map());
    setRoiError(null);
    setPending(true);
    const { data, error: failure, response } = await api.POST("/searches", {
      body: { query },
    });
    if (pollRef.current !== run) return;
    if (unauthorized(response.status)) return;
    if (!data) {
      setPending(false);
      setError(errorMessage(response.status, failure?.error, common.errors));
      return;
    }
    await poll(data.id, run, Date.now());
  }

  const prepCentavos = pesosToCentavos(prep, true);
  const shippingCentavos = pesosToCentavos(shipping, true);
  const costsValid = prepCentavos !== null && shippingCentavos !== null;
  const results = search?.results;

  useEffect(() => {
    if (!results || prepCentavos === null || shippingCentavos === null) return;
    const indexes: number[] = [];
    const items: Schemas["RoiItemRequest"][] = [];
    results.forEach((result, index) => {
      const amazon = result.amazon;
      if (
        amazon?.amazon_price_centavos === null ||
        amazon?.amazon_price_centavos === undefined ||
        amazon.fees_centavos === null ||
        amazon.fees_centavos === undefined
      ) {
        return;
      }
      indexes.push(index);
      items.push({
        sell_price_centavos: amazon.amazon_price_centavos,
        buy_price_centavos: result.price_centavos,
        amazon_fees_centavos: amazon.fees_centavos,
        prep_fee_centavos: prepCentavos,
        shipping_centavos: shippingCentavos,
      });
    });
    if (items.length === 0) return;
    let active = true;
    const batches: number[] = [];
    for (let start = 0; start < items.length; start += MAX_ROI_BATCH) batches.push(start);
    void Promise.all(
      batches.map((start) =>
        api
          .POST("/roi", { body: { items: items.slice(start, start + MAX_ROI_BATCH) } })
          .then((reply) => ({ start, ...reply })),
      ),
    ).then((replies) => {
      if (!active) return;
      if (replies.some(({ response }) => response.status === 401)) {
        router.replace(signInPath(lang, currentPath()));
        return;
      }
      const next = new Map<number, Roi>();
      let failed: string | null = null;
      for (const { start, data, error: failure, response } of replies) {
        if (!data) {
          failed ??= errorMessage(response.status, failure?.error, common.errors);
          continue;
        }
        data.results.forEach((item, i) => next.set(indexes[start + i], item));
      }
      setRoi(next);
      setRoiError(failed === null ? null : interpolate(dict.roiFailed, { reason: failed }));
    });
    return () => {
      active = false;
    };
  }, [results, prepCentavos, shippingCentavos, router, lang, common.errors, dict.roiFailed]);

  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? common.none : formatMoney(v, lang);
  const percent = (v: number | null | undefined) =>
    v === null || v === undefined ? common.none : formatBasisPoints(v, lang);
  const statusText = timedOut
    ? dict.status.timeout
    : search
      ? (dict.status as Record<string, string>)[search.status] ?? search.status
      : pending
        ? dict.status.queued
        : "";

  if (gate.status === "error") {
    return <LoadError message={gate.message} retryLabel={common.retry} onRetry={gate.retry} />;
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }
  return (
    <>
      <h1>{dict.heading}</h1>
      <form className="stack" onSubmit={submit} role="search">
        <label>
          {dict.label}
          <input
            type="search"
            name="query"
            required
            maxLength={100}
            placeholder={dict.placeholder}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <fieldset>
          <legend>{dict.costsLegend}</legend>
          <label>
            {dict.prep}
            <input
              name="prep"
              inputMode="decimal"
              aria-invalid={pesosToCentavos(prep, true) === null}
              value={prep}
              onChange={(e) => setPrep(e.target.value)}
            />
          </label>
          <label>
            {dict.shipping}
            <input
              name="shipping"
              inputMode="decimal"
              aria-invalid={pesosToCentavos(shipping, true) === null}
              value={shipping}
              onChange={(e) => setShipping(e.target.value)}
            />
          </label>
        </fieldset>
        <button type="submit" disabled={pending || query.trim() === ""}>
          {dict.submit}
        </button>
      </form>
      <p role="alert" className="error">
        {error}
      </p>
      <p role="status" data-testid="search-status">
        {statusText}
      </p>

      {search && FINISHED.has(search.status) ? (
        <section aria-labelledby="results-heading">
          <h2 id="results-heading">
            {interpolate(dict.resultsHeading, { query: search.query })}
          </h2>
          {search.hidden_results > 0 ? (
            <p className="hint">
              {interpolate(dict.hidden, { count: search.hidden_results })}
            </p>
          ) : null}
          <p role="alert" className="error" data-testid="roi-error">
            {costsValid ? roiError : null}
          </p>
          {search.results.length === 0 ? (
            <p>{dict.noResults}</p>
          ) : (
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th scope="col">{dict.columns.product}</th>
                    <th scope="col">{dict.columns.price}</th>
                    <th scope="col">{dict.columns.availability}</th>
                    <th scope="col">{dict.columns.amazon}</th>
                    <th scope="col">{dict.columns.profit}</th>
                    <th scope="col">{dict.columns.roi}</th>
                  </tr>
                </thead>
                <tbody>
                  {search.results.map((result, index) => {
                    const amazon = result.amazon;
                    const item = costsValid ? roi.get(index) : undefined;
                    return (
                      <tr key={`${result.retailer}/${result.retailer_sku}`}>
                        <th scope="row">
                          <span className="retailer">
                            {retailerName(result.retailer, retailers)}
                          </span>
                          <a
                            href={result.url}
                            target="_blank"
                            rel="noopener noreferrer"
                            aria-label={`${result.title} · ${dict.storeLink}`}
                          >
                            {result.title}
                          </a>
                          {result.membership_required ? (
                            <span className="badge">{dict.membership}</span>
                          ) : null}
                        </th>
                        <td>{money(result.price_centavos)}</td>
                        <td>
                          {(dict.availability as Record<string, string>)[
                            result.availability
                          ] ?? dict.availability.unknown}
                        </td>
                        <td>
                          {amazon ? (
                            <>
                              <span
                                className={
                                  amazon.match_status === "accepted"
                                    ? "badge ok"
                                    : "badge warn"
                                }
                              >
                                {amazon.match_status === "accepted"
                                  ? dict.match.accepted
                                  : dict.match.needsReview}
                              </span>{" "}
                              <Link href={`/${lang}/products/${amazon.asin}`}>
                                {interpolate(dict.match.history, { asin: amazon.asin })}
                              </Link>
                              <span className="hint">
                                {money(amazon.amazon_price_centavos)}
                              </span>
                            </>
                          ) : (
                            dict.match.none
                          )}
                        </td>
                        <td data-testid="result-profit">
                          {item?.error
                            ? ((dict.roiError as Record<string, string>)[item.error] ??
                              common.errors.generic)
                            : money(item?.profit_centavos ?? null)}
                        </td>
                        <td data-testid="result-roi">{percent(item?.roi_bp ?? null)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
          <h3>{dict.outcomesHeading}</h3>
          <ul className="outcomes">
            {search.retailers.map((r) => (
              <li key={r.retailer}>
                {interpolate(dict.outcomeLine, {
                  retailer: retailerName(r.retailer, retailers),
                  outcome:
                    (dict.outcomes as Record<string, string>)[r.outcome] ?? r.outcome,
                  offers: interpolate(dict.offers, { count: r.offers }),
                })}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </>
  );
}
