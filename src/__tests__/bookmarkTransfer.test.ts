import { describe, expect, it } from "vitest";
import { detectImportFormat, exportFileName, shareFileName } from "../bookmarkTransfer";

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

  it("puts the shared group in the file name so the recipient can tell what it is", () => {
    const stamp = new Date(2026, 7, 24, 15, 30);
    expect(shareFileName("Prod/EU", stamp)).toBe("auraterm-share-Prod-EU-2026-08-24-1530.json");
    expect(shareFileName("Prod\\EU\\Web", stamp)).toBe("auraterm-share-Prod-EU-Web-2026-08-24-1530.json");
  });

  it("keeps share file names legal on Windows", () => {
    const stamp = new Date(2026, 7, 24, 15, 30);
    // Reserved characters are dropped, spaces folded, trailing dots removed.
    expect(shareFileName('Prod: "EU" <1>?', stamp)).toBe("auraterm-share-Prod-EU-1-2026-08-24-1530.json");
    expect(shareFileName("Lab.", stamp)).toBe("auraterm-share-Lab-2026-08-24-1530.json");
    // A group made entirely of illegal characters still yields a usable name.
    expect(shareFileName("??", stamp)).toBe("auraterm-share-2026-08-24-1530.json");
  });
});
