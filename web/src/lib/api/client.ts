import createClient from "openapi-fetch";

import type { paths } from "./schema";

/**
 * Typed client for the Rust API, generated from its OpenAPI spec.
 * Regenerate the types with `npm --prefix web run gen:api`.
 */
export const api = createClient<paths>({ baseUrl: "/backend" });
