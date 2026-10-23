"use client";

import { useId, type InputHTMLAttributes } from "react";

type InputProps = Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "id" | "aria-invalid" | "aria-describedby"
>;

/**
 * Label, bordered field, optional hint and inline error. Every id comes from
 * one `useId()`, so `aria-describedby` always names the hint and the current
 * error (see sources#field-ids); the field turns red from `[aria-invalid]`
 * alone (sources#field-invalid). The error is not an alert: the form's own
 * alert announces the failed action.
 */
export function TextField({
  label,
  hint,
  error,
  ...input
}: InputProps & {
  label: string;
  hint?: string;
  error?: string | null;
}) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const describedBy = [hint ? hintId : null, error ? errorId : null]
    .filter((part) => part !== null)
    .join(" ");
  return (
    <div className="text-field">
      <label htmlFor={id}>{label}</label>
      <span className="field">
        <input
          {...input}
          id={id}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy === "" ? undefined : describedBy}
        />
      </span>
      {hint ? (
        <p id={hintId} className="hint">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} className="field-error">
          {error}
        </p>
      ) : null}
    </div>
  );
}
