/** Import/export plumbing for bookmark files, kept out of the components so the
 *  format sniffing and file naming can be unit-tested. */

export type BookmarkImportFormat = "auraterm" | "putty" | "openssh";

/** AuraTerm's own export marker, mirroring `BOOKMARK_EXPORT_FORMAT` in Rust. */
const AURATERM_MARKER = "auraterm-bookmarks";

/**
 * Pick the parser for a dropped file. AuraTerm's own export is JSON carrying a
 * format marker; PuTTY ships a `.reg` registry dump; anything else is treated
 * as an OpenSSH config, which is what `ssh_config` files look like.
 */
export function detectImportFormat(fileName: string, content: string): BookmarkImportFormat {
  const head = content.slice(0, 4096);
  if (head.trimStart().startsWith("{") && head.includes(AURATERM_MARKER)) {
    return "auraterm";
  }
  return fileName.toLowerCase().endsWith(".reg") ? "putty" : "openssh";
}

/** `auraterm-bookmarks-2026-08-24-1530.json` — sortable, no colons (Windows). */
export function exportFileName(scope: "all" | "selection", now = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  const stamp = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
    + `-${pad(now.getHours())}${pad(now.getMinutes())}`;
  return `auraterm-bookmarks${scope === "selection" ? "-selection" : ""}-${stamp}.json`;
}

/**
 * Hand a generated file to the user. The WebView has no filesystem access of
 * its own, so this goes through an object URL and a synthetic anchor click —
 * the same route the credential backup in the settings dialog takes.
 */
export function downloadText(fileName: string, content: string, mimeType = "application/json") {
  const url = URL.createObjectURL(new Blob([content], { type: mimeType }));
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}
