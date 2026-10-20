// Fails when the message catalogs drift: every locale must have exactly the
// keys of es.json (the source of truth), and no value may be empty.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const dir = new URL("../src/i18n/messages/", import.meta.url);
const locales = ["es", "en"];

/** Flattens nested objects to `a.b.c` keys; returns [key, value] pairs. */
function flatten(value, prefix = "") {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    return Object.entries(value).flatMap(([k, v]) =>
      flatten(v, prefix ? `${prefix}.${k}` : k),
    );
  }
  return [[prefix, value]];
}

const catalogs = Object.fromEntries(
  locales.map((lang) => {
    const file = fileURLToPath(new URL(`${lang}.json`, dir));
    return [lang, new Map(flatten(JSON.parse(readFileSync(file, "utf8"))))];
  }),
);

const problems = [];
const reference = catalogs.es;
for (const lang of locales) {
  const catalog = catalogs[lang];
  for (const key of [...reference.keys()].sort()) {
    if (!catalog.has(key)) problems.push(`${lang}.json: missing ${key}`);
  }
  for (const [key, value] of [...catalog.entries()].sort()) {
    if (!reference.has(key)) problems.push(`${lang}.json: extra ${key}`);
    if (typeof value !== "string" || value.trim() === "") {
      problems.push(`${lang}.json: ${key} must be a non-empty string`);
    }
  }
}

if (problems.length > 0) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log(`messages ok: ${reference.size} keys in ${locales.join(", ")}`);
