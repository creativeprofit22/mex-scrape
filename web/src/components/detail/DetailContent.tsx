"use client";

import Link from "next/link";
import { useEffect, useRef, useState } from "react";

import { PriceChart } from "@/components/chart/PriceChart";
import { PriceTable } from "@/components/chart/PriceTable";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatMoney, interpolate } from "@/i18n/format";
import { api } from "@/lib/api/client";
import { productTitle } from "@/lib/product/name";
import type { MatchDecision, Row } from "@/lib/search/rows";
import type { Costs } from "@/lib/search/storage";
import { errorMessage, retailerName } from "@/lib/ui";

import { MatchDecisionButtons } from "@/components/match/MatchDecisionButtons";
import { MatchIdentity } from "@/components/match/MatchIdentity";
import { MatchMeter } from "@/components/match/MatchMeter";
import { MatchOutcome } from "@/components/match/MatchOutcome";
import { ProfitBreakdown } from "./ProfitBreakdown";
import { useDetailHistory } from "./useDetailHistory";

/** Text the panel needs, from several catalog groups. */
export type DetailDicts = {
  lang: Locale;
  detail: Dictionary["detail"];
  product: Dictionary["product"];
  search: Dictionary["search"];
  common: Dictionary["common"];
  retailers: Dictionary["retailers"];
};

/** Height of the panel's chart, axes included. */
const CHART_HEIGHT = 200;

/**
 * The selected result: store, title, link, the match (Amazon name, with
 * "ASIN …" until it loads or when none is stored) with its confidence
 * and Confirmar / Descartar, the profit breakdown and the 90-day chart.
 * Keyed by the row, so a new selection starts with fresh request state.
 */
export function DetailContent({
  row,
  decision,
  costs,
  dicts,
  titleId,
  onClose,
  onDecided,
  toSignIn,
}: {
  row: Row;
  /** This session's saved verdict on the row's match, if any. */
  decision: MatchDecision | undefined;
  costs: Costs;
  dicts: DetailDicts;
  titleId: string;
  onClose: () => void;
  onDecided: (key: string, decision: MatchDecision) => void;
  toSignIn: () => void;
}) {
  const { lang, detail, product, search, common, retailers } = dicts;
  const { result } = row;
  const amazon = result.amazon ?? null;
  const store = retailerName(result.retailer, retailers);
  const { history, retry } = useDetailHistory(amazon?.asin ?? null, toSignIn, common.errors);
  const asinOnly = amazon ? interpolate(detail.asinOnly, { asin: amazon.asin }) : "";
  const amazonName = history.status === "ready" ? productTitle(history.history.title, asinOnly) : asinOnly;

  const [saving, setSaving] = useState<MatchDecision | null>(null);
  const [failed, setFailed] = useState<{ decision: MatchDecision; message: string } | null>(null);
  const mounted = useRef(true);
  const outcomeRef = useRef<HTMLParagraphElement>(null);
  const decidedHere = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // The buttons go away once a verdict is saved: keep keyboard focus in the
  // panel by moving it to the outcome line.
  useEffect(() => {
    if (decision && decidedHere.current) outcomeRef.current?.focus();
  }, [decision]);

  const decide = async (verdict: MatchDecision) => {
    if (!amazon || saving) return;
    setSaving(verdict);
    setFailed(null);
    const { error, response } = await api.POST("/matches/decisions", {
      body: {
        asin: amazon.asin,
        retailer: result.retailer,
        retailer_sku: result.retailer_sku,
        decision: verdict,
      },
    });
    if (response.status === 401) {
      toSignIn();
      return;
    }
    if (response.ok) {
      decidedHere.current = true;
      // Saved even if the user has moved on to another row.
      onDecided(row.key, verdict);
      if (!mounted.current) return;
      setSaving(null);
      // The history counts only accepted matches, so this listing joins the
      // chart now: load it again rather than show the pre-accept series.
      if (verdict === "accept") retry();
      return;
    }
    if (!mounted.current) return;
    setSaving(null);
    setFailed({
      decision: verdict,
      message: interpolate(detail.decisionFailed, {
        reason: errorMessage(response.status, error?.error, common.errors, {
          not_reviewable: detail.notReviewable,
        }),
      }),
    });
  };

  return (
    <div className="detail">
      <header className="detail__header">
        <MatchIdentity store={store} title={result.title} titleId={titleId} level="h2" />
        <button type="button" className="plain-button detail__close" aria-label={detail.close} onClick={onClose}>
          <svg className="detail__close-icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
            <path d="M3.5 3.5l9 9M12.5 3.5l-9 9" />
          </svg>
        </button>
      </header>

      <p className="detail__links">
        <span className="num detail__price">{formatMoney(result.price_centavos, lang)}</span>
        <a
          className="result-link"
          href={result.url}
          target="_blank"
          rel="noopener noreferrer"
          aria-label={interpolate(detail.openStore, { store })}
        >
          {interpolate(detail.openStoreShort, { store })}
        </a>
      </p>

      {decision === "reject" ? (
        <MatchOutcome ref={outcomeRef} testId="detail-outcome">
          {detail.rejected}
        </MatchOutcome>
      ) : !amazon ? (
        <p className="detail__empty" data-testid="detail-no-match">
          {detail.noMatch}
        </p>
      ) : (
        <>
          <section className="detail__section" aria-labelledby="detail-match-heading">
            <h3 id="detail-match-heading" className="detail__heading">
              {detail.matchHeading}
            </h3>
            <p className="detail__amazon" data-testid="detail-amazon-name">
              <span className="detail__amazon-name">{amazonName}</span>
              {amazonName !== asinOnly ? <span className="detail__amazon-asin t-num">{asinOnly}</span> : null}
            </p>
            <MatchMeter
              confidenceBp={amazon.confidence_bp}
              lang={lang}
              label={detail.confidence}
              valueLabel={detail.confidenceValue}
            />
            {amazon.match_status === "accepted" ? (
              <MatchOutcome ref={outcomeRef} testId="detail-outcome">
                <span className="tag tag-gain">{detail.confirmed}</span>
              </MatchOutcome>
            ) : (
              <>
                <p className="hint">{detail.reviewHint}</p>
                <MatchDecisionButtons
                  saving={saving}
                  onConfirm={() => void decide("accept")}
                  onReject={() => void decide("reject")}
                  labels={{ confirm: detail.confirm, reject: detail.reject, saving: detail.saving }}
                />
                {failed ? (
                  <div className="detail__error">
                    <p role="alert" className="error" data-testid="decision-error">
                      {failed.message}
                    </p>
                    <button type="button" className="secondary" onClick={() => void decide(failed.decision)}>
                      {common.retry}
                    </button>
                  </div>
                ) : null}
              </>
            )}
          </section>

          <ProfitBreakdown
            row={row}
            costs={costs}
            lang={lang}
            dict={detail}
            search={search}
            none={common.none}
            generic={common.errors.generic}
          />

          <section className="detail__section" aria-labelledby="detail-chart-heading">
            <h3 id="detail-chart-heading" className="detail__heading">
              {detail.chartHeading}
            </h3>
            {history.status === "error" ? (
              <div className="detail__error">
                <p role="alert" className="error" data-testid="history-error">
                  {interpolate(detail.historyFailed, { reason: history.message })}
                </p>
                <button type="button" className="secondary" onClick={retry}>
                  {common.retry}
                </button>
              </div>
            ) : history.status === "ready" ? (
              <>
                {history.history.hidden_listings > 0 ? (
                  <p className="hint" data-testid="hidden-listings">
                    {interpolate(product.hidden, { count: history.history.hidden_listings })}
                  </p>
                ) : null}
                {history.history.listings.every((l) => l.points.length === 0) ? (
                  <p className="detail__empty" data-testid="history-empty">
                    {detail.historyEmpty}
                  </p>
                ) : (
                  <>
                    <PriceChart
                      history={history.history}
                      lang={lang}
                      dict={product}
                      retailers={retailers}
                      height={CHART_HEIGHT}
                    />
                    <PriceTable
                      history={history.history}
                      lang={lang}
                      dict={product}
                      none={common.none}
                      retailers={retailers}
                    />
                  </>
                )}
              </>
            ) : (
              <div className="detail__loading">
                <div className="skeleton detail__chart-skeleton" aria-hidden="true" />
                <p role="status">{detail.historyLoading}</p>
              </div>
            )}
            <p>
              <Link className="result-link" href={`/${lang}/products/${amazon.asin}`}>
                {detail.fullHistory}
              </Link>
            </p>
          </section>
        </>
      )}
    </div>
  );
}
