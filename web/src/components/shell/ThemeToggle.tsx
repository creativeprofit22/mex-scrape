"use client";

import { useLayoutEffect } from "react";

import type { Dictionary } from "@/i18n/dictionaries";
import { applyTheme, currentTheme, saveTheme, savedTheme } from "@/lib/theme";

/**
 * Switches between light and dark and remembers the choice. Like the
 * language switch it names where it goes ("Modo oscuro" while light). Both
 * labels are rendered and CSS shows the one for the scheme on screen, so the
 * label is right before hydration with no client state.
 */
export function ThemeToggle({ nav }: { nav: Dictionary["nav"] }) {
  // React's dev remount resets <html> attributes; put the saved choice back
  // before paint. A no-op in production, where the head script already ran.
  useLayoutEffect(() => {
    const saved = savedTheme();
    if (saved) applyTheme(saved);
  }, []);

  function toggle() {
    saveTheme(currentTheme() === "dark" ? "light" : "dark");
  }

  return (
    <button type="button" className="link-button theme-toggle" onClick={toggle}>
      <span className="theme-toggle__to-dark">{nav.darkMode}</span>
      <span className="theme-toggle__to-light">{nav.lightMode}</span>
    </button>
  );
}
