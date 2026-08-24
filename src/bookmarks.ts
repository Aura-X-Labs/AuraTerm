import { normalizeReconnectType, type SavedConnection } from "./types";
import { isSerialProtocol } from "./serialTransport";

export interface BookmarkFolder {
  name: string;
  path: string;
  folders: BookmarkFolder[];
  connections: SavedConnection[];
  count: number;
}

export type BookmarkTreeRow =
  | { kind: "folder"; key: string; depth: number; folder: BookmarkFolder }
  | { kind: "connection"; key: string; depth: number; connection: SavedConnection };

export function normalizeBookmarkPath(value?: string): string[] {
  return (value ?? "")
    .replace(/\\/g, "/")
    .split("/")
    .map((part) => part.trim())
    .filter(Boolean);
}

/**
 * Every group path in use by the given connections, including the intermediate
 * folders of nested paths (`prod/db` also yields `prod`), de-duplicated and
 * sorted case-insensitively. Used to populate the group pickers.
 */
export function collectGroupPaths(connections: readonly SavedConnection[]): string[] {
  const groups = new Set<string>();
  for (const connection of connections) {
    let path = "";
    for (const part of normalizeBookmarkPath(connection.group)) {
      path = path ? `${path}/${part}` : part;
      groups.add(path);
    }
  }
  return [...groups].sort((left, right) => left.localeCompare(right, undefined, { sensitivity: "base" }));
}

/** How the bookmark manager's left column narrows the list. */
export type BookmarkScope =
  | { kind: "all" }
  | { kind: "recent" }
  | { kind: "ungrouped" }
  | { kind: "group"; path: string };

export type BookmarkSortKey = "name" | "protocol" | "target" | "auth" | "group" | "lastUsed";
export type SortDirection = "asc" | "desc";

/** The connection target as shown in list rows: what you would type to reach it. */
export function connectionTarget(connection: SavedConnection): string {
  if (isSerialProtocol(connection.protocol)) {
    return `${connection.portName ?? "serial"} @ ${connection.baudRate ?? 9600}`;
  }
  if (connection.protocol === "telnet") {
    return `${connection.host}:${connection.port}`;
  }
  return `${connection.user}@${connection.host}:${connection.port}`;
}

/** True when the connection belongs under `scope`; a group scope includes subfolders. */
export function matchesScope(connection: SavedConnection, scope: BookmarkScope): boolean {
  switch (scope.kind) {
    case "all":
      return true;
    case "recent":
      return Boolean(connection.lastUsed);
    case "ungrouped":
      return normalizeBookmarkPath(connection.group).length === 0;
    case "group": {
      const path = normalizeBookmarkPath(connection.group).join("/");
      return path === scope.path || path.startsWith(`${scope.path}/`);
    }
  }
}

/** Free-text filter over the fields a user would search by. */
export function filterConnections(connections: readonly SavedConnection[], query: string): SavedConnection[] {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return [...connections];
  }
  return connections.filter((connection) => [
    connection.name,
    connection.host,
    connection.user,
    connection.portName,
    connection.group,
    connection.protocol ?? "ssh",
    normalizeReconnectType(connection),
  ].some((field) => field?.toLowerCase().includes(needle)));
}

function sortValue(connection: SavedConnection, key: BookmarkSortKey): string | number {
  switch (key) {
    case "name":
      return connection.name;
    case "protocol":
      return connection.protocol ?? "ssh";
    case "target":
      return connectionTarget(connection);
    case "auth":
      return connection.authType ?? "";
    case "group":
      return normalizeBookmarkPath(connection.group).join("/");
    case "lastUsed":
      // Never-used bookmarks sort as oldest rather than jumping to the top.
      return connection.lastUsed ?? 0;
  }
}

/** Stable sort by one column. Returns a new array; the input is untouched. */
export function sortConnections(
  connections: readonly SavedConnection[],
  key: BookmarkSortKey,
  direction: SortDirection,
): SavedConnection[] {
  const factor = direction === "asc" ? 1 : -1;
  return [...connections].sort((left, right) => {
    const leftValue = sortValue(left, key);
    const rightValue = sortValue(right, key);
    if (typeof leftValue === "number" && typeof rightValue === "number") {
      return (leftValue - rightValue) * factor || left.name.localeCompare(right.name, undefined, { sensitivity: "base" });
    }
    return String(leftValue).localeCompare(String(rightValue), undefined, { sensitivity: "base" }) * factor
      || left.name.localeCompare(right.name, undefined, { sensitivity: "base" });
  });
}

/**
 * Where `path` lands when group `from` is renamed to `to`, keeping subfolders
 * attached (`Prod/EU` → `Production/EU` when renaming `Prod` to `Production`).
 * Returns null when `path` sits outside the renamed subtree; an empty `to`
 * dissolves the group, promoting its children one level. Mirrors
 * `renamed_group_path` in `connections.rs`.
 */
export function rewriteGroupPath(path: string, from: string, to: string): string | null {
  const current = normalizeBookmarkPath(path).join("/");
  const source = normalizeBookmarkPath(from).join("/");
  const target = normalizeBookmarkPath(to).join("/");
  if (!source) {
    return null;
  }
  if (current === source) {
    return target;
  }
  if (!current.startsWith(`${source}/`)) {
    return null;
  }
  const rest = current.slice(source.length + 1);
  return target ? `${target}/${rest}` : rest;
}

/**
 * Apply a group rename across a list of group paths, dropping the ones that
 * dissolve into "ungrouped". Used for the explicitly created (possibly empty)
 * groups the app persists alongside the bookmarks.
 */
export function rewriteGroupPaths(paths: readonly string[], from: string, to: string): string[] {
  const rewritten = paths
    .map((path) => rewriteGroupPath(path, from, to) ?? normalizeBookmarkPath(path).join("/"))
    .filter(Boolean);
  return [...new Set(rewritten)].sort((left, right) => left.localeCompare(right, undefined, { sensitivity: "base" }));
}

/**
 * Rewrite one group path to another across a list of bookmarks. Returns only
 * the connections whose group actually changed.
 */
export function renameGroupPath(
  connections: readonly SavedConnection[],
  from: string,
  to: string,
): SavedConnection[] {
  const moved: SavedConnection[] = [];
  for (const connection of connections) {
    const current = normalizeBookmarkPath(connection.group).join("/");
    const next = rewriteGroupPath(current, from, to);
    if (next === null || next === current) {
      continue;
    }
    moved.push({ ...connection, group: next || undefined });
  }
  return moved;
}

/**
 * Build the folder tree. `extraGroups` seeds folders that hold no bookmark yet
 * (groups the user created explicitly) so they do not vanish from the tree.
 */
export function buildBookmarkTree(
  connections: SavedConnection[],
  extraGroups: readonly string[] = [],
): BookmarkFolder {
  const root: BookmarkFolder = { name: "", path: "", folders: [], connections: [], count: 0 };

  const descend = (parts: string[]) => {
    let folder = root;
    for (const part of parts) {
      const path = folder.path ? `${folder.path}/${part}` : part;
      let child = folder.folders.find((candidate) => candidate.name === part);
      if (!child) {
        child = { name: part, path, folders: [], connections: [], count: 0 };
        folder.folders.push(child);
      }
      folder = child;
    }
    return folder;
  };

  for (const group of extraGroups) {
    descend(normalizeBookmarkPath(group));
  }
  for (const connection of connections) {
    descend(normalizeBookmarkPath(connection.group)).connections.push(connection);
  }

  const finalize = (folder: BookmarkFolder): number => {
    folder.folders.sort((left, right) => left.name.localeCompare(right.name, undefined, { sensitivity: "base" }));
    folder.connections.sort((left, right) => left.name.localeCompare(right.name, undefined, { sensitivity: "base" }));
    folder.count = folder.connections.length + folder.folders.reduce((total, child) => total + finalize(child), 0);
    return folder.count;
  };
  finalize(root);
  return root;
}

export function flattenBookmarkTree(root: BookmarkFolder, collapsed: ReadonlySet<string>): BookmarkTreeRow[] {
  const rows: BookmarkTreeRow[] = [];
  const visit = (folder: BookmarkFolder, depth: number) => {
    if (folder.path) {
      rows.push({ kind: "folder", key: `folder:${folder.path}`, depth, folder });
      if (collapsed.has(folder.path)) return;
    }
    const childDepth = folder.path ? depth + 1 : depth;
    for (const child of folder.folders) visit(child, childDepth);
    for (const connection of folder.connections) {
      rows.push({ kind: "connection", key: `connection:${connection.id}`, depth: childDepth, connection });
    }
  };
  visit(root, 0);
  return rows;
}
