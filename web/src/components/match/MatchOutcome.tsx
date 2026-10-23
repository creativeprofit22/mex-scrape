import type { ReactNode, Ref } from "react";

/**
 * The saved verdict, announced politely. Focusable so the caller can move
 * focus here once the decision buttons go away.
 */
export function MatchOutcome({
  ref,
  testId,
  children,
}: {
  ref?: Ref<HTMLParagraphElement>;
  testId?: string;
  children: ReactNode;
}) {
  return (
    <p ref={ref} tabIndex={-1} role="status" className="match__outcome" data-testid={testId}>
      {children}
    </p>
  );
}
