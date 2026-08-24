import { describe, expect, it } from "vitest";
import { detectImportFormat, exportFileName } from "../bookmarkTransfer";

describe("bookmark transfer", () => {
  it("recognizes an AuraTerm export by its marker, whatever the file is called", () => {
    const exported = JSON.stringify({ format: "auraterm-bookmarks", version: 1, connections: [] });
    expect(detectImportFormat("backup.json", exported)).toBe("auraterm");
    expect(detectImportFormat("backup.txt", exported)).toBe("auraterm");
  });

  it("falls back to PuTTY for .reg and OpenSSH for everything else", () => {
    expect(detectImportFormat("sessions.reg", "Windows Registry Editor Version 5.00")).toBe("putty");
    expect(detectImportFormat("config", "Host web\n  HostName 10.0.0.1")).toBe("openssh");
    // JSON that is not ours must not be parsed as an AuraTerm export.
    expect(detectImportFormat("other.json", '{"format":"something-else"}')).toBe("openssh");
  });

  it("names exports with a sortable, Windows-safe timestamp", () => {
    const stamp = new Date(2026, 7, 24, 15, 30);
    expect(exportFileName("all", stamp)).toBe("auraterm-bookmarks-2026-08-24-1530.json");
    expect(exportFileName("selection", stamp)).toBe("auraterm-bookmarks-selection-2026-08-24-1530.json");
  });
});
