"use client";

import { useId, useRef, useState, type KeyboardEvent } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatMoney, interpolate } from "@/i18n/format";
import { centavosToPesosInput, type Costs } from "@/lib/search/storage";
import { pesosToCentavos } from "@/lib/ui";

type Draft = { prep: string; shipping: string };

/**
 * Prep and shipping per unit behind one pill (a disclosure button). The
 * panel sits inside the search form, so Aplicar is a plain button and
 * Enter in a field applies instead of searching. Esc closes without
 * saving; both put focus back on the pill.
 */
export function CostsPill({
  costs,
  onSave,
  lang,
  dict,
}: {
  costs: Costs;
  onSave: (costs: Costs) => void;
  lang: Locale;
  dict: Dictionary["search"]["costs"];
}) {
  const id = useId();
  const pillRef = useRef<HTMLButtonElement>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [showErrors, setShowErrors] = useState(false);
  const open = draft !== null;

  const prep = draft ? pesosToCentavos(draft.prep, true) : costs.prep;
  const shipping = draft ? pesosToCentavos(draft.shipping, true) : costs.shipping;

  function openEditor() {
    setDraft({
      prep: costs.prep === 0 ? "" : centavosToPesosInput(costs.prep),
      shipping: costs.shipping === 0 ? "" : centavosToPesosInput(costs.shipping),
    });
    setShowErrors(false);
  }

  function close() {
    setDraft(null);
    pillRef.current?.focus();
  }

  function apply() {
    if (prep === null || shipping === null) {
      setShowErrors(true);
      return;
    }
    onSave({ prep, shipping });
    close();
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "Enter" && event.target instanceof HTMLInputElement) {
      event.preventDefault();
      apply();
    }
  }

  const fields = [
    { key: "prep", label: dict.prep, invalid: showErrors && prep === null },
    { key: "shipping", label: dict.shipping, invalid: showErrors && shipping === null },
  ] as const;

  return (
    <div className="costs">
      <button
        ref={pillRef}
        type="button"
        className="plain-button costs-pill"
        aria-expanded={open}
        aria-controls={`${id}-panel`}
        onClick={() => (open ? close() : openEditor())}
      >
        {interpolate(dict.pill, {
          prep: formatMoney(costs.prep, lang),
          shipping: formatMoney(costs.shipping, lang),
        })}
      </button>
      <div
        id={`${id}-panel`}
        className="costs-editor"
        role="group"
        aria-labelledby={`${id}-heading`}
        hidden={!open}
        onKeyDown={onKeyDown}
      >
        <p id={`${id}-heading`} className="costs-editor__heading">
          {dict.heading}
        </p>
        {draft
          ? fields.map((field) => (
              <label key={field.key}>
                {field.label}
                <span className="field">
                  <input
                    name={field.key}
                    inputMode="decimal"
                    autoComplete="off"
                    aria-invalid={field.invalid}
                    aria-describedby={field.invalid ? `${id}-error` : `${id}-hint`}
                    value={draft[field.key]}
                    onChange={(e) => setDraft({ ...draft, [field.key]: e.target.value })}
                  />
                </span>
              </label>
            ))
          : null}
        <p id={`${id}-hint`} className="hint">
          {dict.hint}
        </p>
        <p id={`${id}-error`} role="alert" className="error">
          {showErrors && (prep === null || shipping === null) ? dict.invalid : null}
        </p>
        <button type="button" onClick={apply}>
          {dict.apply}
        </button>
      </div>
    </div>
  );
}
