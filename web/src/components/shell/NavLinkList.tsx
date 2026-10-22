import Link from "next/link";

import type { NavId } from "@/lib/nav";

/** One rendered nav entry. */
export type NavLinkItem = { id: NavId; href: string; label: string };

/**
 * The links of a nav, with `aria-current="page"` on the `active` one only
 * (omitted, never "false", elsewhere). The current item stays a link, as in
 * the WAI navigation pattern; its look changes in weight and bar, not only
 * colour. see sources#shadcn-nav-current
 */
export function NavLinkList({
  items,
  active,
  className,
}: {
  items: readonly NavLinkItem[];
  active: NavId | null;
  className: string;
}) {
  return (
    <ul className={className}>
      {items.map((item) => (
        <li key={item.id}>
          <Link
            href={item.href}
            aria-current={item.id === active ? "page" : undefined}
          >
            {item.label}
          </Link>
        </li>
      ))}
    </ul>
  );
}
