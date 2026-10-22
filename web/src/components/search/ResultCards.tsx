import type { Row } from "@/lib/search/rows";

import {
  AmazonMatch,
  Availability,
  Price,
  Profit,
  ResultMeta,
  ResultTitle,
  Roi,
  type ResultView,
} from "./ResultParts";

/**
 * Results below 768px: one card per result with labelled values, in the
 * same order as the table. The title button selects the card.
 */
export function ResultCards({
  rows,
  view,
  selectedKey,
  onSelect,
}: {
  rows: readonly Row[];
  view: ResultView;
  selectedKey: string | null;
  onSelect: (key: string | null) => void;
}) {
  const { columns } = view.dict;
  return (
    <ul className="result-cards">
      {rows.map((row) => {
        const selected = row.key === selectedKey;
        return (
          <li key={row.key} className={selected ? "result-card is-selected" : "result-card"}>
            <div className="result-card__title">
              <ResultTitle row={row} selected={selected} onSelect={onSelect} />
              <ResultMeta row={row} view={view} />
            </div>
            <dl className="result-card__values">
              <div>
                <dt>{columns.profit}</dt>
                <dd className="num" data-testid="result-profit">
                  <Profit row={row} view={view} />
                </dd>
              </div>
              <div>
                <dt>{columns.roi}</dt>
                <dd className="num" data-testid="result-roi">
                  <Roi row={row} view={view} />
                </dd>
              </div>
              <div>
                <dt>{columns.price}</dt>
                <dd className="num">
                  <Price row={row} view={view} />
                </dd>
              </div>
              <div>
                <dt>{columns.availability}</dt>
                <dd>
                  <Availability row={row} view={view} />
                </dd>
              </div>
              <div className="result-card__wide">
                <dt>{columns.amazon}</dt>
                <dd>
                  <AmazonMatch row={row} view={view} />
                </dd>
              </div>
            </dl>
          </li>
        );
      })}
    </ul>
  );
}
