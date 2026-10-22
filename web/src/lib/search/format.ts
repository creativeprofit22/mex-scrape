/** Sign glyphs from the dictionary (`search.sign`), never JSX literals. */
export type SignGlyphs = { plus: string; minus: string };

/** `gain`, `loss` or nothing for a signed amount (zero and none are neutral). */
export function signClass(value: number | null | undefined): "gain" | "loss" | undefined {
  if (value === null || value === undefined || value === 0) return undefined;
  return value > 0 ? "gain" : "loss";
}

/**
 * A signed amount: `+` or `−` (the typographic minus) before the
 * formatted absolute value, so gains and losses read the same in every
 * locale. Zero has no sign.
 */
export function formatSigned(
  value: number,
  format: (absolute: number) => string,
  glyphs: SignGlyphs,
): string {
  const text = format(Math.abs(value));
  if (value > 0) return glyphs.plus + text;
  if (value < 0) return glyphs.minus + text;
  return text;
}
