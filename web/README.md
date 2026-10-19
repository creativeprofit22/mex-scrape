# web

Next.js (App Router, TypeScript) front end for mex-scrape. Spanish is the default language.

- Install: `npm ci`
- Dev server: `npm run dev` (proxies `/backend/*` to `API_URL`, default `http://127.0.0.1:8080`; start the API with `cargo run -p api`)
- Checks: `npm run lint`, `npm run typecheck`, `npm run build`
- Regenerate the API client after changing the Rust API: `npm run gen:api` (from the repo root: `npm --prefix web run gen:api`). It writes `openapi.json` and `src/lib/api/schema.ts`; commit both.

See `../docs/modules/web.md`.
