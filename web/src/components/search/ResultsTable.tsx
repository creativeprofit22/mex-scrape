import type { Row } from "@/lib/search/rows";
import type { Sort, SortKey } from "@/lib/search/sort";

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
import { SortHeader } from "./SortHeader";

/**
 * Results on screens ≥768px. The title button selects a row; its hit area
 * is stretched over the row, with the links lifted above it.
 */
export function ResultsTable({
  rows,
  view,
  sort,
  onSort,
  selectedKey,
  onSelect,
}: {
  rows: readonly Row[];
  view: ResultView;
  sort: Sort;
  onSort: (column: SortKey) => void;
  selectedKey: string | null;
  onSelect: (key: string | null) => void;
}) {
  const { columns } = view.dict;
  return (
    <table className="results-table">
      <thead>
        <tr>
          <th scope="col">{columns.product}</th>
          <SortHeader column="price" label={columns.price} sort={sort} onSort={onSort} className="num" />
          <th scope="col">{columns.availability}</th>
          <th scope="col">{columns.amazon}</th>
          <SortHeader column="profit" label={columns.profit} sort={sort} onSort={onSort} className="num" />
          <SortHeader column="roi" label={columns.roi} sort={sort} onSort={onSort} className="num" />
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => {
          const selected = row.key === selectedKey;
          return (
            <tr key={row.key} className={selected ? "is-selected" : undefined}>
              <th scope="row">
                <ResultTitle row={row} selected={selected} onSelect={onSelect} />
                <ResultMeta row={row} view={view} />
              </th>
              <td className="num">
                <Price row={row} view={view} />
              </td>
              <td>
                <Availability row={row} view={view} />
              </td>
              <td>
                <AmazonMatch row={row} view={view} />
              </td>
              <td className="num" data-testid="result-profit">
                <Profit row={row} view={view} />
              </td>
              <td className="num" data-testid="result-roi">
                <Roi row={row} view={view} />
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
