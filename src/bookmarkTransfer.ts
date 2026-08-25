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

/** `2026-08-24-1530` — sortable, and without the colons Windows rejects. */
function timeStamp(now: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
    + `-${pad(now.getHours())}${pad(now.getMinutes())}`;
}

/** `auraterm-bookmarks-2026-08-24-1530.json` — sortable, no colons (Windows). */
export function exportFileName(scope: "all" | "selection", now = new Date()): string {
  return `auraterm-bookmarks${scope === "selection" ? "-selection" : ""}-${timeStamp(now)}.json`;
}

/**
 * `auraterm-share-Prod-EU-2026-08-24-1530.json`.
 *
 * The shared group's path goes into the file name so the recipient can tell
 * what they were sent without opening it. Path separators become `-`, and the
 * characters Windows rejects in a file name are dropped — including a trailing
 * dot, which it also refuses.
 */
export function shareFileName(root: string, now = new Date()): string {
  const slug = root
    .split(/[\\/]/)
    .map((part) => part.replace(/["*:<>?|\x00-\x1f]/g, "").trim())
    .filter(Boolean)
    .join("-")
    .replace(/\s+/g, "-")
    .slice(0, 60)
    .replace(/[-.]+$/, "");
  return `auraterm-share${slug ? `-${slug}` : ""}-${timeStamp(now)}.json`;
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
