import type { ReactNode } from "react";

/** One summary tile: a small label over a display-face numeral. */
export function KpiTile({
  label,
  children,
  testId,
}: {
  label: string;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <div className="kpi-tile">
      <dt className="kpi-tile__label">{label}</dt>
      <dd className="kpi-tile__value" data-testid={testId}>
        {children}
      </dd>
    </div>
  );
}

/** A tile placeholder while the search runs (static, no shimmer). */
export function KpiTileSkeleton() {
  return (
    <div className="kpi-tile" aria-hidden="true">
      <span className="skeleton skeleton--label" />
      <span className="skeleton skeleton--numeral" />
    </div>
  );
}
