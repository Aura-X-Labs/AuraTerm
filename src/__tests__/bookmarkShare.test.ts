import { describe, expect, it } from "vitest";
import { formatShareTime, looksLikeShareCode } from "../bookmarkShare";

describe("share codes", () => {
  it("recognizes a code however the user pasted it", () => {
    expect(looksLikeShareCode("BCDF-GHJK-LMNP-QRST")).toBe(true);
    expect(looksLikeShareCode("  bcdfghjklmnpqrst ")).toBe(true);
    expect(looksLikeShareCode("https://auraxlab.com/b#BCDF-GHJK-LMNP-QRST")).toBe(true);
  });

  it("rejects anything that is not one", () => {
    expect(looksLikeShareCode("BCDF-GHJK-LMNP")).toBe(false);
    expect(looksLikeShareCode("BCDF-GHJK-LMNP-QRST-VWXZ")).toBe(false);
    // I/O/U/A/E/Y are absent from the alphabet, so a word never matches.
    expect(looksLikeShareCode("SOMETHINGELSEXYZ")).toBe(false);
    expect(looksLikeShareCode('{"format":"auraterm-bookmarks"}')).toBe(false);
  });

  it("renders share timestamps without pretending to second precision", () => {
    expect(formatShareTime("2026-09-01T15:30:45Z")).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
    expect(formatShareTime(null)).toBe("");
    expect(formatShareTime("not a date")).toBe("");
  });
});
