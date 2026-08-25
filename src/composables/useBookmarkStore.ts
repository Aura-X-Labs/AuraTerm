import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { collectGroupPaths } from "../bookmarks";
import type { SavedConnection } from "../types";

/**
 * Shared bookmark state.
 *
 * The refs live at module scope on purpose: the sidebar and the bookmark
 * manager page must observe the same list, otherwise an edit made in one is
 * invisible in the other until a manual refresh.
 */
const connections = ref<SavedConnection[]>([]);
const loading = ref(false);
const loadError = ref("");
/** False in master-password mode while locked — `get_connections` then returns
 *  metadata with every secret stripped. */
const credentialsUnlocked = ref(true);
const collapsedGroups = ref<Set<string>>(new Set());

const STORAGE_KEY_COLLAPSED = "auraterm:collapsed-groups";
let collapsedRestored = false;

/** Thrown for the operations that need the credential store — deleting,
 *  duplicating, exporting secrets — while the master password is locked.
 *  Moving and renaming groups touch metadata only and stay available. */
export class CredentialsLockedError extends Error {
  constructor() {
    super("Credentials are locked");
    this.name = "CredentialsLockedError";
  }
}

/** Result of `import_bookmarks`. */
export interface BookmarkImportResult {
  imported: number;
  skipped: number;
  warnings: string[];
  /** Bookmark-free subfolders a share bundle asked for, as absolute paths.
   *  They only survive in `settings.bookmarkGroups`, so the caller has to
   *  merge them there — nothing in `connections.json` can rebuild them. */
  createdGroups: string[];
}

function requireUnlocked() {
  if (!credentialsUnlocked.value) {
    throw new CredentialsLockedError();
  }
}

function restoreCollapsedState() {
  if (collapsedRestored) {
    return;
  }
  collapsedRestored = true;
  try {
    const saved = localStorage.getItem(STORAGE_KEY_COLLAPSED);
    if (saved) {
      collapsedGroups.value = new Set(JSON.parse(saved));
    }
  } catch (error) {
    console.error("Failed to load collapsed state", error);
  }
}

function persistCollapsedState() {
  // Storage can be unavailable (blocked site data, private windows): folding
  // state is a convenience, never worth throwing into a click handler over.
  try {
    localStorage.setItem(STORAGE_KEY_COLLAPSED, JSON.stringify([...collapsedGroups.value]));
  } catch (error) {
    console.error("Failed to persist collapsed state", error);
  }
}

function toggleGroup(path: string) {
  // Always assign a fresh Set — mutating in place does not trigger reactivity.
  const next = new Set(collapsedGroups.value);
  if (next.has(path)) {
    next.delete(path);
  } else {
    next.add(path);
  }
  collapsedGroups.value = next;
  persistCollapsedState();
}

function expandGroup(path: string) {
  if (!collapsedGroups.value.has(path)) {
    return;
  }
  const next = new Set(collapsedGroups.value);
  next.delete(path);
  collapsedGroups.value = next;
  persistCollapsedState();
}

function isGroupCollapsed(path: string) {
  return collapsedGroups.value.has(path);
}

async function refreshCredentialState() {
  try {
    const state = await invoke<{ unlocked: boolean }>("get_credential_security_state");
    credentialsUnlocked.value = state.unlocked;
  } catch (error) {
    console.error("Failed to read credential security state", error);
  }
}

/** Reload the whole list from the backend, plus the credential lock state. */
async function refresh(expandPath?: string) {
  restoreCollapsedState();
  loading.value = true;
  try {
    connections.value = await invoke<SavedConnection[]>("get_connections");
    loadError.value = "";
    if (expandPath) {
      expandGroup(expandPath);
    }
  } catch (error) {
    console.error("Failed to load connections", error);
    loadError.value = String(error);
  } finally {
    loading.value = false;
  }
  void refreshCredentialState();
}

function replaceLocal(connection: SavedConnection) {
  const index = connections.value.findIndex((candidate) => candidate.id === connection.id);
  connections.value = index === -1
    ? [...connections.value, connection]
    : connections.value.map((candidate) => (candidate.id === connection.id ? connection : candidate));
}

/**
 * Create or update one bookmark.
 *
 * Requires unlocked credentials: `save_connection` rewrites the credential
 * entry from whatever the frontend sends, and while locked `get_connections`
 * hands out connections with their secrets stripped — saving one back would
 * erase its stored password and private key.
 */
async function save(connection: SavedConnection) {
  requireUnlocked();
  await invoke("save_connection", { connection });
  replaceLocal(connection);
}

async function remove(id: string) {
  requireUnlocked();
  await invoke("delete_connection", { id });
  connections.value = connections.value.filter((connection) => connection.id !== id);
}

/** Delete several bookmarks in one backend call — one rewrite of the metadata
 *  file and one of the credential store, instead of one pair per bookmark. */
async function removeMany(ids: readonly string[]): Promise<number> {
  if (ids.length === 0) {
    return 0;
  }
  requireUnlocked();
  const removed = await invoke<number>("delete_connections", { ids: [...ids] });
  const deleted = new Set(ids);
  connections.value = connections.value.filter((connection) => !deleted.has(connection.id));
  return removed;
}

/** Move bookmarks into `group` (undefined = ungrouped). Metadata only, so this
 *  works while credentials are locked. */
async function moveMany(ids: readonly string[], group: string | undefined): Promise<number> {
  if (ids.length === 0) {
    return 0;
  }
  const moved = await invoke<number>("move_connections", { ids: [...ids], group: group ?? null });
  if (moved > 0) {
    await refresh();
  }
  return moved;
}

/** Rename or move a group; subfolders follow. An empty `to` dissolves it. */
async function renameGroup(from: string, to: string): Promise<number> {
  const renamed = await invoke<number>("rename_group", { from, to });
  if (renamed > 0) {
    await refresh();
  }
  return renamed;
}

/** Copy bookmarks (credentials included), returning the new ids. */
async function duplicateMany(ids: readonly string[], nameFor: (name: string) => string): Promise<string[]> {
  requireUnlocked();
  const created: string[] = [];
  for (const id of ids) {
    const source = connections.value.find((connection) => connection.id === id);
    created.push(await invoke<string>("duplicate_connection", {
      id,
      name: source ? nameFor(source.name) : null,
    }));
  }
  if (created.length > 0) {
    await refresh();
  }
  return created;
}

/** Serialize bookmarks to the AuraTerm exchange format. Secrets are opt-in. */
async function exportBookmarks(ids: readonly string[] | null, includeSecrets: boolean): Promise<string> {
  if (includeSecrets) {
    requireUnlocked();
  }
  return invoke<string>("export_bookmarks", {
    ids: ids ? [...ids] : null,
    includeSecrets,
  });
}

/**
 * Pack one group — its subtree plus the subfolders that hold no bookmark —
 * into a share bundle.
 *
 * Unlike `exportBookmarks` this never carries credentials, so it stays
 * available while the master password is locked: everything it strips
 * (`connections.rs` `sanitize_connection_for_sharing`) is exactly what a
 * locked store would not hand out anyway.
 */
async function exportGroup(
  root: string,
  explicitGroups: readonly string[],
  label?: string,
  note?: string,
): Promise<string> {
  return invoke<string>("export_group_bookmarks", {
    root,
    explicitGroups: [...explicitGroups],
    label: label ?? null,
    note: note ?? null,
  });
}

async function importBookmarks(format: string, content: string, group: string | undefined): Promise<BookmarkImportResult> {
  const result = await invoke<BookmarkImportResult>("import_bookmarks", {
    format,
    content,
    group: group ?? null,
  });
  await refresh(group);
  return result;
}

/** Record a connection as just used, so recency ordering stays truthful. */
async function touch(id: string) {
  const timestamp = Date.now();
  try {
    await invoke("touch_connection", { id, timestamp });
    const connection = connections.value.find((candidate) => candidate.id === id);
    if (connection) {
      replaceLocal({ ...connection, lastUsed: timestamp });
    }
  } catch (error) {
    console.error("Failed to touch connection", id, error);
  }
}

const groupPaths = computed(() => collectGroupPaths(connections.value));

/** Shared bookmark list, group folding state, and the mutations both the
 *  sidebar and the manager page perform. */
export function useBookmarkStore() {
  return {
    connections,
    loading,
    loadError,
    credentialsUnlocked,
    collapsedGroups,
    groupPaths,
    refresh,
    refreshCredentialState,
    save,
    remove,
    removeMany,
    moveMany,
    renameGroup,
    duplicateMany,
    exportBookmarks,
    exportGroup,
    importBookmarks,
    touch,
    toggleGroup,
    expandGroup,
    isGroupCollapsed,
  };
}
