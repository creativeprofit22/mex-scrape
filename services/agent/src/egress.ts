/**
 * The provider transport's only way out: HTTPS to an allowlisted host on the
 * default port. Anything else is refused before a socket opens.
 */

export class EgressRefused extends Error {
  constructor() {
    super("egress refused");
    this.name = "EgressRefused";
  }
}

function targetUrl(input: string | URL | Request): URL {
  if (input instanceof Request) return new URL(input.url);
  return new URL(input);
}

/** A `fetch` that only reaches `hosts` over HTTPS. */
export function allowlistedFetch(
  hosts: readonly string[],
  inner: typeof fetch = globalThis.fetch,
): typeof fetch {
  const allowed = new Set(hosts.map((h) => h.toLowerCase()));
  const guarded = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    let url: URL;
    try {
      url = targetUrl(input);
    } catch {
      throw new EgressRefused();
    }
    if (
      url.protocol !== "https:" ||
      url.port !== "" ||
      url.username !== "" ||
      url.password !== "" ||
      !allowed.has(url.hostname.toLowerCase())
    ) {
      throw new EgressRefused();
    }
    return inner(input, { ...init, redirect: "error" });
  };
  return guarded as typeof fetch;
}
