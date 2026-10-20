import { defineConfig, devices } from "@playwright/test";

// End-to-end tests against a seeded backend (docs/modules/web.md):
//   DATABASE_URL=... cargo run -p db --bin migrate
//   DATABASE_URL=... cargo run -p db --bin seed_e2e -- seed
//   npm --prefix web run build && npm --prefix web run e2e
// Playwright starts the api and `next start` unless they are already up
// (locally); searches are completed by `seed_e2e complete`, so no worker,
// Bright Data or Amazon credentials are needed. The assistant sidecar runs
// with the scripted fake model (needs `npm --prefix services/agent ci`).

const databaseUrl =
  process.env.DATABASE_URL ?? "postgres://mex:mex@127.0.0.1:5432/mex_scrape";
const ci = Boolean(process.env.CI);

// The assistant sidecar (services/agent) with its scripted fake model. These
// values are e2e-only fixtures shared by the api and the sidecar, not secrets.
const agentServiceToken = "e2e-agent-service-token-0123456789abcdef";
const agentGrantKey = "e2e-agent-grant-key-0123456789abcdef0123";

export default defineConfig({
  testDir: "./e2e",
  // One seeded account is shared, so tests run one at a time.
  fullyParallel: false,
  workers: 1,
  forbidOnly: ci,
  retries: 0,
  reporter: ci ? [["list"], ["html", { open: "never" }]] : "list",
  timeout: 120_000,
  use: {
    // localhost is a secure context, so the api's Secure cookie is kept.
    baseURL: "http://localhost:3000",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: [
    {
      command: "cargo run --locked -p api",
      cwd: "..",
      url: "http://127.0.0.1:8080/health",
      reuseExistingServer: !ci,
      timeout: 600_000,
      stdout: "pipe",
      env: {
        DATABASE_URL: databaseUrl,
        API_ADDR: "127.0.0.1:8080",
        // Polling a search is several requests a second; keep the plan
        // limits out of the way (they have their own api tests).
        RATE_LIMIT_FREE_PER_MINUTE: "6000",
        RATE_LIMIT_FREE_BURST: "1000",
        RATE_LIMIT_ANON_PER_MINUTE: "6000",
        RATE_LIMIT_ANON_BURST: "1000",
        // private_service refuses IP literals, so the sidecar is `localhost`.
        AGENT_URL: "http://localhost:8090",
        AGENT_SERVICE_TOKEN: agentServiceToken,
        AGENT_GRANT_KEY: agentGrantKey,
      },
    },
    {
      command: "npm run start",
      cwd: "../services/agent",
      url: "http://127.0.0.1:8090/health",
      reuseExistingServer: !ci,
      timeout: 120_000,
      stdout: "pipe",
      env: {
        AGENT_ADDR: "127.0.0.1:8090",
        AGENT_API_URL: "http://127.0.0.1:8080",
        AGENT_SERVICE_TOKEN: agentServiceToken,
        AGENT_PROVIDER: "palsu",
        AGENT_ALLOW_FAKE_PROVIDER: "true",
      },
    },
    {
      command: "npm run start",
      url: "http://localhost:3000/es",
      reuseExistingServer: !ci,
      timeout: 120_000,
    },
  ],
});
