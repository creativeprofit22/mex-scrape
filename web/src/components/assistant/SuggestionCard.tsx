"use client";

import { useEffect, useRef } from "react";

import { MatchDecisionButtons } from "@/components/match/MatchDecisionButtons";
import { MatchIdentity } from "@/components/match/MatchIdentity";
import { MatchOutcome } from "@/components/match/MatchOutcome";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import type { Schemas } from "@/lib/api/client";
import { retailerName } from "@/lib/ui";

export type Suggestion = Schemas["MatchSuggestionResponse"];
export type CardState =
  | { status: "open" }
  | { status: "busy" }
  | { status: "done"; note: string }
  | { status: "error"; note: string };

/**
 * One proposal from the assistant, built from the detail panel's match
 * pieces. Confirmar is the only write (ADR 0013); Descartar is local.
 */
export function SuggestionCard({
  suggestion,
  state,
  onConfirm,
  onDismiss,
  dict,
  retailers,
}: {
  suggestion: Suggestion;
  state: CardState;
  onConfirm: () => void;
  onDismiss: () => void;
  dict: Dictionary["assistant"];
  retailers: Dictionary["retailers"];
}) {
  const s = suggestion;
  const title = s.title || s.retailer_sku;
  const outcomeRef = useRef<HTMLParagraphElement>(null);
  const decidedHere = useRef(false);

  // The buttons go away once decided; keep focus in the card on the outcome.
  useEffect(() => {
    if (state.status === "done" && decidedHere.current) outcomeRef.current?.focus();
  }, [state.status]);

  const decide = (action: () => void) => () => {
    decidedHere.current = true;
    action();
  };

  return (
    <li className="suggestion-card" data-testid="suggestion">
      <MatchIdentity store={retailerName(s.retailer, retailers)} title={title} level="h3" />
      <p className="suggestion-card__ids t-num">
        {interpolate(dict.suggestion, { sku: s.retailer_sku, asin: s.asin })}
      </p>
      <p className="suggestion-card__reason">
        <span className={s.decision === "accept" ? "tag tag-gain" : "tag tag-warn"}>
          {s.decision === "accept" ? dict.proposeAccept : dict.proposeReject}
        </span>{" "}
        {interpolate(dict.reason, { reason: s.reason })}
      </p>
      {state.status === "done" ? (
        <MatchOutcome ref={outcomeRef}>{state.note}</MatchOutcome>
      ) : (
        <>
          <MatchDecisionButtons
            saving={state.status === "busy" ? "accept" : null}
            onConfirm={decide(onConfirm)}
            onReject={decide(onDismiss)}
            labels={{
              confirm: dict.confirm,
              reject: dict.dismiss,
              saving: dict.saving,
              confirmName: interpolate(dict.confirmLabel, { title }),
              rejectName: interpolate(dict.dismissLabel, { title }),
            }}
          />
          <p role="alert" className="error">
            {state.status === "error" ? state.note : null}
          </p>
        </>
      )}
    </li>
  );
}
