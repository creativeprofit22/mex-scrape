import type { NextConfig } from "next";

// Rust API base URL; requests to /backend/* are proxied there (no CORS needed).
// Baked in at `next build` (routes-manifest.json); `next start` ignores a runtime API_URL.
const apiUrl = process.env.API_URL ?? "http://127.0.0.1:8080";

const nextConfig: NextConfig = {
  cacheComponents: true,
  partialPrefetching: true,
  async rewrites() {
    return [{ source: "/backend/:path*", destination: `${apiUrl}/:path*` }];
  },
};

export default nextConfig;
