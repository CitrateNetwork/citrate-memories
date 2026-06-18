import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    testTimeout: 20_000,
  },
  resolve: {
    alias: [
      { find: "@", replacement: fileURLToPath(new URL("./src", import.meta.url)) },
      // `server-only` throws on import outside an RSC bundle; neutralize it for
      // unit tests of the BFF/gateway modules.
      { find: /^server-only$/, replacement: fileURLToPath(new URL("./src/test/server-only-shim.ts", import.meta.url)) },
    ],
  },
});
