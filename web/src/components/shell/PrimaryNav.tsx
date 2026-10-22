import { Suspense } from "react";

import { ActiveNavLinks } from "./ActiveNavLinks";
import { NavLinkList, type NavLinkItem } from "./NavLinkList";

/**
 * A labelled nav whose current item comes from the URL. The URL is unknown
 * while prerendering pages with request-time params (products), so the
 * fallback is the same links with nothing marked current.
 */
export function PrimaryNav({
  label,
  items,
  className,
  listClassName,
}: {
  label: string;
  items: readonly NavLinkItem[];
  className: string;
  listClassName: string;
}) {
  return (
    <nav aria-label={label} className={className}>
      <Suspense fallback={<NavLinkList items={items} active={null} className={listClassName} />}>
        <ActiveNavLinks items={items} className={listClassName} />
      </Suspense>
    </nav>
  );
}
