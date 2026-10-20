/** Entry point: load config, start the server. */
import { ConfigError, loadConfig } from "./config.ts";
import { registerScriptedProvider } from "./fake-provider.ts";
import { createAgentServer, installProcessGuards, jsonLog } from "./server.ts";

function main(): void {
  installProcessGuards();
  let config;
  try {
    config = loadConfig(process.env);
  } catch (e) {
    jsonLog("config_invalid", { variable: e instanceof ConfigError ? e.variable : "unknown" });
    process.exit(1);
  }
  if (config.provider === "palsu") {
    registerScriptedProvider();
    jsonLog("fake_provider_enabled");
  }
  const server = createAgentServer(config);
  server.requestTimeout = config.limits.runTimeoutMs + 15_000;
  server.on("error", () => {
    jsonLog("listen_failed", { port: config.port });
    process.exit(1);
  });
  server.listen(config.port, config.host, () => {
    jsonLog("listening", { host: config.host, port: config.port, provider: config.provider });
  });
  const stop = () => server.close(() => process.exit(0));
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
}

main();
