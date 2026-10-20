import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    // Each file gets its own process: gg-ai's provider registry is global.
    pool: "forks",
    isolate: true,
    fileParallelism: false,
    testTimeout: 20_000,
  },
});
