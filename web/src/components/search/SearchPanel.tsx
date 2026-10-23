"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { api, NETWORK_ERROR, type Schemas } from "@/lib/api/client";
import { DetailPanel } from "@/components/detail/DetailPanel";
import { LoadError } from "@/components/feedback/LoadError";
import { FilterChips } from "@/components/search/FilterChips";
import { KpiStrip } from "@/components/search/KpiStrip";
import { RecentSearches, RERUN_PARAM } from "@/components/search/RecentSearches";
import { ResultCards } from "@/components/search/ResultCards";
import { ResultsTable } from "@/components/search/ResultsTable";
import { SearchSkeleton } from "@/components/search/SearchSkeleton";
import { SearchStatus } from "@/components/search/SearchStatus";
import { SearchToolbar } from "@/components/search/SearchToolbar";
import { filterRows, NO_FILTERS, type FilterKey } from "@/lib/search/filter";
import { deriveKpis } from "@/lib/search/kpis";
import { applyDecisions, buildRows, type MatchDecision } from "@/lib/search/rows";
import { DEFAULT_SORT, nextSort, sortRows, type SortKey } from "@/lib/search/sort";
import { FINISHED_STATUSES, viewState } from "@/lib/search/status";
import { cleanQuery, NO_COSTS } from "@/lib/search/storage";
import { useCosts, useRecentSearches } from "@/lib/search/useSearchStore";
import { errorMessage } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

/** Delay between `GET /searches/{id}` polls. */
const POLL_MS = 1_500;
/** Give up polling after this long. */
const POLL_LIMIT_MS = 180_000;
/** Most items one `POST /roi` accepts; mirrors `MAX_ROI_BATCH` in crates/app/src/roi.rs. */
const MAX_ROI_BATCH = 100;

type Search = Schemas["SearchResponse"];
type Roi = Schemas["RoiItemResponse"];
type Failure = { kind: "network" | "other"; message: string };

/**
 * The search workspace. Orchestrates only: the onboarding gate, polling,
 * the ROI batch, the `?q=` rerun and the sort, filter and selection state;
 * src/components/search renders the parts, src/lib/search holds the logic.
 *
 * Profit and ROI come from `POST /roi` (the calculator's endpoint) with the
 * saved prep and shipping costs, run once the search has finished. The
 * selected result (`selectedKey`, `retailer/retailer_sku`) lives here so
 * the detail panel renders it; a new search clears it, a re-sort or
 * filter keeps it. Match verdicts saved from the panel (`decisions`) are
 * applied over the results until the next search
 * (docs/modules/web/detail-panel.md).
 */
export function SearchPanel({
  lang,
  dict,
  detail,
  product,
  common,
  retailers,
}: {
  lang: Locale;
  dict: Dictionary["search"];
  detail: Dictionary["detail"];
  product: Dictionary["product"];
  common: Dictionary["common"];
  retailers: Dictionary["retailers"];
}) {
  const toSignIn = useSignInRedirect(lang);
  const gate = useRequireOnboarded(lang, common.errors);
  const router = useRouter();
  const rerun = useSearchParams().get(RERUN_PARAM);
  const [savedCosts, saveCosts] = useCosts();
  const costs = savedCosts ?? NO_COSTS;
  const [, remember] = useRecentSearches();

  const [query, setQuery] = useState("");
  const [lastQuery, setLastQuery] = useState<string | null>(null);
  const [search, setSearch] = useState<Search | null>(null);
  const [timedOut, setTimedOut] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [pending, setPending] = useState(false);
  const [roi, setRoi] = useState<Map<number, Roi>>(new Map());
  const [roiError, setRoiError] = useState<string | null>(null);
  const [roiAttempt, setRoiAttempt] = useState(0);
  const [sort, setSort] = useState(DEFAULT_SORT);
  const [filters, setFilters] = useState(NO_FILTERS);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [decisions, setDecisions] = useState<ReadonlyMap<string, MatchDecision>>(new Map());
  const pollRef = useRef(0);
  const consumedRef = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(
    () => () => {
      pollRef.current += 1;
    },
    [],
  );

  const unauthorized = useCallback(
    (status: number): boolean => {
      if (status !== 401) return false;
      toSignIn();
      return true;
    },
    [toSignIn],
  );

  const fail = useCallback(
    (status: number, code: string | undefined) => {
      setFailure({
        kind: code === NETWORK_ERROR ? "network" : "other",
        message: errorMessage(status, code, common.errors),
      });
      setPending(false);
    },
    [common.errors],
  );

  const poll = useCallback(
    async (id: number, run: number, startedAt: number) => {
      while (pollRef.current === run) {
        const { data, error, response } = await api.GET("/searches/{id}", {
          params: { path: { id } },
        });
        if (pollRef.current !== run) return;
        if (unauthorized(response.status)) return;
        if (!data) {
          fail(response.status, error?.error);
          return;
        }
        setSearch(data);
        if (FINISHED_STATUSES.has(data.status)) {
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
    },
    [unauthorized, fail],
  );

  /** Starts a search for `raw`, replacing any search still polling. */
  const start = useCallback(
    async (raw: string) => {
      const q = cleanQuery(raw);
      if (q === null) return;
      const run = ++pollRef.current;
      remember(q);
      setLastQuery(q);
      setFailure(null);
      setTimedOut(false);
      setSearch(null);
      setRoi(new Map());
      setRoiError(null);
      setSelectedKey(null);
      setDecisions(new Map());
      setPending(true);
      const { data, error, response } = await api.POST("/searches", { body: { query: q } });
      if (pollRef.current !== run) return;
      if (unauthorized(response.status)) return;
      if (!data) {
        fail(response.status, error?.error);
        return;
      }
      await poll(data.id, run, Date.now());
    },
    [remember, unauthorized, fail, poll],
  );

  // A recent-search link lands here with `?q=`: run it once, then drop the
  // parameter so browser back never reruns a search (a POST and scraping).
  useEffect(() => {
    if (gate.status !== "ready") return;
    if (rerun === null) {
      consumedRef.current = null;
      return;
    }
    if (consumedRef.current === rerun) return;
    consumedRef.current = rerun;
    router.replace(`/${lang}/search`, { scroll: false });
    const q = cleanQuery(rerun);
    if (q === null) return;
    queueMicrotask(() => {
      setQuery(q);
      void start(q);
    });
  }, [gate.status, rerun, router, lang, start]);

  // Results are final only once the search has finished; polls stop there,
  // so this identity is stable and sort/filter changes never re-run /roi.
  const finalResults = search && FINISHED_STATUSES.has(search.status) ? search.results : undefined;
  const { prep, shipping } = costs;

  useEffect(() => {
    if (!finalResults) return;
    const indexes: number[] = [];
    const items: Schemas["RoiItemRequest"][] = [];
    finalResults.forEach((result, index) => {
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
        prep_fee_centavos: prep,
        shipping_centavos: shipping,
      });
    });
    if (items.length === 0) return;
    let active = true;
    const batches: number[] = [];
    for (let first = 0; first < items.length; first += MAX_ROI_BATCH) batches.push(first);
    void Promise.all(
      batches.map((first) =>
        api
          .POST("/roi", { body: { items: items.slice(first, first + MAX_ROI_BATCH) } })
          .then((reply) => ({ first, ...reply })),
      ),
    ).then((replies) => {
      if (!active) return;
      if (replies.some(({ response }) => response.status === 401)) {
        toSignIn();
        return;
      }
      const next = new Map<number, Roi>();
      let failed: string | null = null;
      for (const { first, data, error, response } of replies) {
        if (!data) {
          failed ??= errorMessage(response.status, error?.error, common.errors);
          continue;
        }
        data.results.forEach((item, i) => {
          const index = indexes[first + i];
          if (index !== undefined) next.set(index, item);
        });
      }
      setRoi(next);
      setRoiError(failed === null ? null : interpolate(dict.roiFailed, { reason: failed }));
    });
    return () => {
      active = false;
    };
  }, [finalResults, prep, shipping, roiAttempt, toSignIn, common.errors, dict.roiFailed]);

  const rows = useMemo(
    () => applyDecisions(buildRows(finalResults ?? [], roi), decisions),
    [finalResults, roi, decisions],
  );
  const selectedRow = useMemo(() => rows.find((r) => r.key === selectedKey) ?? null, [rows, selectedKey]);
  const closeDetail = useCallback(() => setSelectedKey(null), []);
  const decided = useCallback((key: string, decision: MatchDecision) => {
    setDecisions((current) => new Map(current).set(key, decision));
  }, []);
  const kpis = useMemo(() => (search && finalResults ? deriveKpis(search, rows) : null), [search, finalResults, rows]);
  const shown = useMemo(() => sortRows(filterRows(rows, filters), sort), [rows, filters, sort]);

  const state = viewState({ search, pending, timedOut, failure: failure?.kind ?? null });
  const running = state === "queued" || state === "running" || state === "running-stores";

  if (gate.status === "error") {
    return <LoadError message={gate.message} retryLabel={common.retry} onRetry={gate.retry} />;
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }

  const view = { lang, dict, common, retailers };
  const detailDicts = { lang, detail, product, search: dict, common, retailers };
  return (
    <div className="search-workspace">
      <h1>{dict.heading}</h1>
      <SearchToolbar
        query={query}
        onQueryChange={setQuery}
        onSubmit={() => void start(query)}
        // Only the query already running is blocked: a different one
        // replaces the search mid-poll.
        pending={pending && cleanQuery(query) === lastQuery}
        costs={costs}
        onSaveCosts={saveCosts}
        inputRef={inputRef}
        lang={lang}
        dict={dict}
      />
      <SearchStatus
        state={state}
        search={search}
        error={failure?.message ?? null}
        lang={lang}
        dict={dict}
        retryLabel={common.retry}
        retailers={retailers}
        onRetry={() => {
          if (lastQuery !== null) void start(lastQuery);
        }}
      />
      {state === "idle" ? (
        <RecentSearches lang={lang} heading={dict.recent.heading} className="recent-searches--phone" />
      ) : null}
      {running ? (
        <>
          <KpiStrip kpis={null} lang={lang} dict={dict} none={common.none} />
          <SearchSkeleton />
        </>
      ) : null}
      {/* Failed, refused, empty and all-hidden searches have nothing to
          summarise: the status line carries the outcome and next action. */}
      {search && finalResults && finalResults.length > 0 && !failure && !timedOut ? (
        <>
          <KpiStrip kpis={kpis} lang={lang} dict={dict} none={common.none} />
          <div className="results-layout">
            <section className="results-section" aria-labelledby="results-heading">
              <h2 id="results-heading">{interpolate(dict.resultsHeading, { query: search.query })}</h2>
              {search.hidden_results > 0 ? (
                <p className="hint">{interpolate(dict.hidden, { count: search.hidden_results })}</p>
              ) : null}
              {roiError ? (
                <div className="roi-error">
                  <p role="alert" className="error" data-testid="roi-error">
                    {roiError}
                  </p>
                  <button type="button" className="secondary" onClick={() => setRoiAttempt((n) => n + 1)}>
                    {dict.roiRetry}
                  </button>
                </div>
              ) : null}
              {rows.length > 0 ? (
                <>
                  <FilterChips
                    dict={dict.filters}
                    filters={filters}
                    onToggle={(key: FilterKey) => setFilters((f) => ({ ...f, [key]: !f[key] }))}
                    shown={shown.length}
                    total={rows.length}
                  />
                  {shown.length > 0 ? (
                    <div className="results">
                      <ResultsTable
                        rows={shown}
                        view={view}
                        sort={sort}
                        onSort={(key: SortKey) => setSort((s) => nextSort(s, key))}
                        selectedKey={selectedKey}
                        onSelect={setSelectedKey}
                      />
                      <ResultCards rows={shown} view={view} selectedKey={selectedKey} onSelect={setSelectedKey} />
                    </div>
                  ) : (
                    <div className="results-empty">
                      <p>{dict.filters.noMatches}</p>
                      <button type="button" className="secondary" onClick={() => setFilters(NO_FILTERS)}>
                        {dict.filters.clear}
                      </button>
                    </div>
                  )}
                </>
              ) : null}
            </section>
            <DetailPanel
              row={selectedRow}
              decision={selectedRow ? decisions.get(selectedRow.key) : undefined}
              costs={costs}
              dicts={detailDicts}
              onClose={closeDetail}
              onDecided={decided}
              toSignIn={toSignIn}
            />
          </div>
        </>
      ) : null}
    </div>
  );
}
