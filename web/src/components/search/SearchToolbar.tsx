"use client";

import { useEffect, type FormEvent, type RefObject } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import type { Costs } from "@/lib/search/storage";
import { MAX_QUERY_CHARS } from "@/lib/search/storage";

import { CostsPill } from "./CostsPill";

/** True when a keystroke there is text entry, so `/` must not steal focus. */
function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  );
}

/**
 * The search form: query field (`/` focuses it from anywhere but a
 * field), the costs pill, then Buscar.
 */
export function SearchToolbar({
  query,
  onQueryChange,
  onSubmit,
  pending,
  costs,
  onSaveCosts,
  inputRef,
  lang,
  dict,
}: {
  query: string;
  onQueryChange: (query: string) => void;
  onSubmit: () => void;
  pending: boolean;
  costs: Costs;
  onSaveCosts: (costs: Costs) => void;
  inputRef: RefObject<HTMLInputElement | null>;
  lang: Locale;
  dict: Dictionary["search"];
}) {
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key !== "/" || event.defaultPrevented) return;
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      if (isEditable(event.target)) return;
      const input = inputRef.current;
      // A hidden route copy (kept by Activity) has no layout box.
      if (!input || input.offsetParent === null) return;
      event.preventDefault();
      input.focus();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [inputRef]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSubmit();
  }

  return (
    <form className="search-toolbar" role="search" onSubmit={submit}>
      <span className="field field--toolbar">
        <input
          ref={inputRef}
          type="search"
          name="query"
          data-testid="search-query"
          required
          maxLength={MAX_QUERY_CHARS}
          aria-label={dict.label}
          aria-keyshortcuts="/"
          placeholder={dict.placeholder}
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
        />
      </span>
      <CostsPill costs={costs} onSave={onSaveCosts} lang={lang} dict={dict.costs} />
      <button type="submit" className="search-toolbar__submit" disabled={pending || query.trim() === ""}>
        {dict.submit}
      </button>
    </form>
  );
}
