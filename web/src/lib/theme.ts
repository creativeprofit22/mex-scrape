/**
 * Light/dark override (ADR 0015). With nothing saved the page follows the
 * system scheme (`prefers-color-scheme`); a saved choice sets
 * `<html data-theme>`, which `tokens.css` reads.
 */

/** A colour scheme the user can pick. */
export type Theme = "light" | "dark";

/** `localStorage` key holding the saved choice. */
export const themeStorageKey = "theme";

/** Narrows a stored value to a theme. */
export function isTheme(value: unknown): value is Theme {
  return value === "light" || value === "dark";
}

/**
 * Inline `<head>` script: applies the saved choice while the HTML is parsed,
 * before the first paint, so a saved theme never flashes the other one.
 */
export const themeScript = `try{var t=localStorage.getItem(${JSON.stringify(themeStorageKey)});if(t==="light"||t==="dark")document.documentElement.dataset.theme=t}catch(e){}`;

/** The saved choice, or null when none (or storage is unavailable). Browser only. */
export function savedTheme(): Theme | null {
  try {
    const value = window.localStorage.getItem(themeStorageKey);
    return isTheme(value) ? value : null;
  } catch {
    return null;
  }
}

/** The scheme on screen now: the override, else the system's. Browser only. */
export function currentTheme(): Theme {
  const forced = document.documentElement.dataset.theme;
  if (isTheme(forced)) return forced;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** Shows `theme` without saving it. Browser only. */
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
}

/** Shows `theme` and remembers it in this browser. Browser only. */
export function saveTheme(theme: Theme): void {
  applyTheme(theme);
  try {
    window.localStorage.setItem(themeStorageKey, theme);
  } catch {
    // Storage blocked: the choice still applies to this page.
  }
}
