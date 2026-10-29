import { describe, expect, it } from "vitest";

import en from "../i18n/locales/en";
import zhCN from "../i18n/locales/zh-CN";

// Every app source file's text, keyed by path (tests and catalogs excluded).
const sources = import.meta.glob<string>(
  ["../**/*.{ts,vue}", "!../__tests__/**", "!../i18n/locales/**"],
  { query: "?raw", import: "default", eager: true },
);

function lookup(catalog: unknown, key: string): unknown {
  return key.split(".").reduce<unknown>(
    (node, part) => (node && typeof node === "object" ? (node as Record<string, unknown>)[part] : undefined),
    catalog,
  );
}

/** Every key passed to t()/$t() as a plain string literal. */
function literalKeys(): Map<string, string> {
  const keys = new Map<string, string>();
  const call = /(?<![\w.])\$?t\(\s*(["'])([a-zA-Z][\w-]*(?:\.[\w-]+)+)\1/g;
  for (const [file, text] of Object.entries(sources)) {
    for (const match of text.matchAll(call)) {
      keys.set(match[2], file.replace(/^\.\.\//, ""));
    }
  }
  return keys;
}

describe("i18n keys", () => {
  const keys = literalKeys();

  it("finds the translated strings in the source", () => {
    expect(keys.size).toBeGreaterThan(500);
  });

  it.each([["en", en], ["zh-CN", zhCN]] as const)("resolves every literal key in %s", (_locale, catalog) => {
    const missing = [...keys].filter(([key]) => typeof lookup(catalog, key) !== "string")
      .map(([key, file]) => `${key} (${file})`);
    expect(missing).toEqual([]);
  });
});
