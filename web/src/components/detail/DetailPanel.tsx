"use client";

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";

import type { MatchDecision, Row } from "@/lib/search/rows";
import type { Costs } from "@/lib/search/storage";

import { DetailContent, type DetailDicts } from "./DetailContent";
import { DetailSheet } from "./DetailSheet";

/** At and above this width the panel sits beside the results (style-pack §4.3). */
const WIDE_QUERY = "(min-width: 75rem)";
const TITLE_ID = "detail-title";

function subscribe(onChange: () => void): () => void {
  const query = window.matchMedia(WIDE_QUERY);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

/** `null` on the server and during hydration: render no panel until known. */
function useWide(): boolean | null {
  return useSyncExternalStore<boolean | null>(
    subscribe,
    () => window.matchMedia(WIDE_QUERY).matches,
    () => null,
  );
}

/** Focuses the visible title button of the row `key` (table or card). */
function focusRow(key: string) {
  const buttons = document.querySelectorAll<HTMLElement>(`[data-row-key="${CSS.escape(key)}"]`);
  for (const button of buttons) {
    if (button.getClientRects().length > 0) {
      button.focus();
      return;
    }
  }
}

/**
 * The selected result's details: an inline `<aside>` beside the results at
 * ≥1200px, a modal sheet below. Exactly one of the two renders, so ids and
 * headings are never duplicated. Closing (×, Esc, backdrop) clears the
 * selection and returns focus to the row's title button; resizing across
 * 1200px keeps the selection and swaps the shell.
 */
export function DetailPanel({
  row,
  decision,
  costs,
  dicts,
  onClose,
  onDecided,
  toSignIn,
}: {
  row: Row | null;
  decision: MatchDecision | undefined;
  costs: Costs;
  dicts: DetailDicts;
  onClose: () => void;
  onDecided: (key: string, decision: MatchDecision) => void;
  toSignIn: () => void;
}) {
  const wide = useWide();
  const keyRef = useRef<string | null>(null);
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    keyRef.current = row?.key ?? null;
  }, [row]);

  const close = useCallback(() => {
    const key = keyRef.current;
    if (key === null) return;
    keyRef.current = null;
    onClose();
    focusRow(key);
  }, [onClose]);

  if (wide === null) return null;

  const content = row ? (
    <DetailContent
      key={row.key}
      row={row}
      decision={decision}
      costs={costs}
      dicts={dicts}
      titleId={TITLE_ID}
      // In the sheet, × closes the dialog; its close event then runs `close`.
      onClose={wide ? close : () => dialogRef.current?.close()}
      onDecided={onDecided}
      toSignIn={toSignIn}
    />
  ) : null;

  if (wide) {
    // Nothing picked: the reserved column says how to fill it.
    return row ? (
      <aside className="detail-aside" aria-labelledby={TITLE_ID} data-testid="detail-panel">
        {content}
      </aside>
    ) : (
      <aside className="detail-aside detail-aside--empty" aria-label={dicts.detail.emptyLabel}>
        <p>{dicts.detail.empty}</p>
      </aside>
    );
  }
  return (
    <DetailSheet dialogRef={dialogRef} open={row !== null} labelledBy={TITLE_ID} onClose={close}>
      {content}
    </DetailSheet>
  );
}
