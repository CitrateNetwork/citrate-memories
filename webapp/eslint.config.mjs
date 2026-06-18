import { defineConfig, globalIgnores } from "eslint/config";
import nextVitals from "eslint-config-next/core-web-vitals";
import nextTs from "eslint-config-next/typescript";

const eslintConfig = defineConfig([
  ...nextVitals,
  ...nextTs,
  globalIgnores([
    ".next/**",
    "out/**",
    "build/**",
    "next-env.d.ts",
    // Vendored prototype from Claude Design — the 1:1 source of truth, kept
    // verbatim (loose JSX-in-browser). Not linted here.
    "design-prototype/**",
  ]),
]);

export default eslintConfig;
