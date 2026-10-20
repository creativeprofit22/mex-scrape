import createClient, { type Middleware } from "openapi-fetch";

import type { components, paths } from "./schema";

/** Base path the Next.js rewrite proxies to the Rust API. */
const BASE_URL = "/backend";

/** Header the API checks on every non-GET request. */
const CSRF_HEADER = "X-CSRF-Token";

const SAFE_METHODS = new Set(["GET", "HEAD", "OPTIONS"]);

/**
 * Fetches the session's CSRF token. The API returns the same token for the
 * session until sign-in rotates it, so asking each time keeps no client state.
 */
async function csrfToken(): Promise<string | undefined> {
  const response = await fetch(`${BASE_URL}/auth/csrf`, {
    credentials: "same-origin",
  });
  if (!response.ok) return undefined;
  const body = (await response.json()) as components["schemas"]["CsrfResponse"];
  return body.csrf_token;
}

/** Attaches the CSRF token to mutating requests. */
const csrf: Middleware = {
  async onRequest({ request }) {
    if (SAFE_METHODS.has(request.method)) return request;
    const token = await csrfToken();
    if (token) request.headers.set(CSRF_HEADER, token);
    return request;
  },
};

/**
 * Typed client for the Rust API, generated from its OpenAPI spec.
 * Regenerate the types with `npm --prefix web run gen:api`.
 * Sends the session cookie (same origin through the `/backend` rewrite).
 */
export const api = createClient<paths>({
  baseUrl: BASE_URL,
  credentials: "same-origin",
});
api.use(csrf);

/** API error body. */
export type ApiError = components["schemas"]["ErrorResponse"];

/** Shared schema types used by the pages. */
export type Schemas = components["schemas"];
