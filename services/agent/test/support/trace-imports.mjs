// Preloaded with `node --import` by the headless test: reports every module
// the sidecar loads on stderr so the test can check no CLI/TUI code runs.
import { registerHooks } from "node:module";

registerHooks({
  resolve(specifier, context, nextResolve) {
    const result = nextResolve(specifier, context);
    process.stderr.write(`IMPORT ${result.url}\n`);
    return result;
  },
});
