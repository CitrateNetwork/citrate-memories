/**
 * 1:1 design-fidelity guard (planset §5.3 — the "to the T" gate). The brand,
 * material-lane, and trust-ring colors in the app's ported tokens
 * (src/app/globals.css) MUST exactly equal the design prototype's source of truth
 * (design-prototype/colors_and_type.css + the taxonomy). If a hue drifts, this
 * fails — design parity is a ratchet, not a one-time check.
 */
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { LANES, TRUST } from "@/lib/data/taxonomy";

const ROOT = join(__dirname, "..", "..", "..");
const globals = readFileSync(join(ROOT, "src", "app", "globals.css"), "utf8");
const tokens = readFileSync(
  join(ROOT, "design-prototype", "colors_and_type.css"),
  "utf8",
).toLowerCase();

/** Pull a `--var: #hex;` value out of a CSS string. */
function cssVar(css: string, name: string): string | null {
  const m = css.match(new RegExp(`--${name}\\s*:\\s*(#[0-9a-fA-F]{3,8})`));
  return m ? m[1].toLowerCase() : null;
}

describe("design token parity — globals.css ⇄ prototype", () => {
  it("brand colors match the prototype exactly", () => {
    for (const name of [
      "citrate-green", "citrate-green-deep", "citrate-green-dark",
      "citrate-yellow", "citrate-yellow-deep", "citrate-yellow-dark",
      "ink", "paper",
    ]) {
      expect(cssVar(globals, name), name).toBe(cssVar(tokens, name));
    }
  });

  it("the dark constellation field is the prototype's evergreen", () => {
    expect(cssVar(globals, "field")).toBe("#0a1810");
  });

  it("every material-lane color is present in the ported tokens", () => {
    for (const lane of LANES) {
      expect(globals.toLowerCase(), lane.id).toContain(lane.color.toLowerCase());
    }
  });

  it("every trust-ring color is present in the ported tokens", () => {
    for (const t of TRUST) {
      expect(globals.toLowerCase(), t.id).toContain(t.ring.toLowerCase());
    }
  });
});
