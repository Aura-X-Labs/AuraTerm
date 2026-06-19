import type { SavedConnection } from "./types";

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

export function buildBookmarkTree(connections: SavedConnection[]): BookmarkFolder {
  const root: BookmarkFolder = { name: "", path: "", folders: [], connections: [], count: 0 };
  for (const connection of connections) {
    const parts = normalizeBookmarkPath(connection.group);
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
    folder.connections.push(connection);
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
