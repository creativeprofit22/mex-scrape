"use client";

import { usePathname } from "next/navigation";

import { activeNavItem } from "@/lib/nav";

import { NavLinkList, type NavLinkItem } from "./NavLinkList";

/** `NavLinkList` with the current page worked out from the URL. */
export function ActiveNavLinks({
  items,
  className,
}: {
  items: readonly NavLinkItem[];
  className: string;
}) {
  const pathname = usePathname();
  return <NavLinkList items={items} active={activeNavItem(pathname)} className={className} />;
}
