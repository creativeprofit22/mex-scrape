import { formatSigned, signClass, type SignGlyphs } from "@/lib/search/format";

/**
 * A signed gain or loss: `+`/`−` before the amount, a `↓` glyph before a
 * loss, and the gain/loss colour, so meaning never rests on colour alone.
 * Glyphs come from the dictionary (`search.sign`).
 */
export function Signed({
  value,
  format,
  sign,
  className,
}: {
  value: number;
  format: (absolute: number) => string;
  sign: SignGlyphs & { down: string };
  className?: string;
}) {
  const tone = signClass(value);
  const classes = [className, tone].filter(Boolean).join(" ") || undefined;
  return (
    <span className={classes}>
      {value < 0 ? (
        <span className="signed__down" aria-hidden="true">
          {sign.down}
        </span>
      ) : null}
      {formatSigned(value, format, sign)}
    </span>
  );
}
