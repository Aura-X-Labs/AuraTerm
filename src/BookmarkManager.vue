<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import BookmarkEditor from "./BookmarkEditor.vue";
import BookmarkManagerList from "./BookmarkManagerList.vue";
import {
  buildBookmarkTree,
  collectGroupPaths,
  filterConnections,
  flattenBookmarkTree,
  matchesScope,
  normalizeBookmarkPath,
  rewriteGroupPaths,
  sortConnections,
  type BookmarkFolder,
  type BookmarkScope,
  type BookmarkSortKey,
  type SortDirection,
} from "./bookmarks";
import { detectImportFormat, downloadText, exportFileName, shareFileName } from "./bookmarkTransfer";
import { useBookmarkStore, CredentialsLockedError } from "./composables/useBookmarkStore";
import { t } from "./i18n";
import { alertDialog, confirmDialog } from "./nativeDialogs";
import { promptText } from "./promptDialog";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import type { SavedConnection } from "./types";

const props = withDefaults(defineProps<{
  settings?: AppSettings;
  /** Explicitly created groups, including ones that hold no bookmark yet. */
  bookmarkGroups?: string[];
}>(), {
  settings: () => DEFAULT_SETTINGS,
  bookmarkGroups: () => [],
});

const emit = defineEmits<{
  connect: [connection: SavedConnection];
  newConnection: [];
  /** Persist the explicit group list (owned by App.vue's settings). */
  updateGroups: [groups: string[]];
  close: [];
}>();

const store = useBookmarkStore();
const { connections, credentialsUnlocked, collapsedGroups } = store;

const searchQuery = ref("");
const searchInput = ref<HTMLInputElement | null>(null);
const importInput = ref<HTMLInputElement | null>(null);
const scope = ref<BookmarkScope>({ kind: "all" });
const sortKey = ref<BookmarkSortKey>("lastUsed");
const sortDirection = ref<SortDirection>("desc");
const selectedId = ref<string | null>(null);
const checkedIds = ref<Set<string>>(new Set());
const anchorId = ref<string | null>(null);
/** Bumped to re-seed the embedded editor from the store copy. */
const editorNonce = ref(0);
const statusMessage = ref("");
const busy = ref(false);
const exportMenuOpen = ref(false);
const folderMenu = ref<{ x: number; y: number; path: string } | null>(null);
const rowMenu = ref<{ x: number; y: number; connection: SavedConnection } | null>(null);
const draggingIds = ref<string[]>([]);
const dropTarget = ref<string | null>(null);
let statusTimer: number | undefined;

/* ------------------------------------------------------------- page width */

const WIDTH_STORAGE_KEY = "auraterm:bookmark-manager-width";
const MIN_PAGE_WIDTH = 760;
/** Widest the page may get, leaving the overlay's padding visible. */
function maxPageWidth() {
  return Math.max(MIN_PAGE_WIDTH, window.innerWidth - 48);
}

/** User-chosen width; null means the stylesheet default. */
const pageWidth = ref<number | null>(null);
const resizing = ref(false);

const pageStyle = computed(() => (pageWidth.value === null
  ? undefined
  : { width: `min(${pageWidth.value}px, calc(100vw - 48px))` }));

/** Remembered width, if storage is readable — it is only a convenience. */
function restoreWidth() {
  try {
    const saved = Number(localStorage.getItem(WIDTH_STORAGE_KEY));
    if (Number.isFinite(saved) && saved >= MIN_PAGE_WIDTH) {
      pageWidth.value = saved;
    }
  } catch (error) {
    console.error("Failed to read the stored width", error);
  }
}

function storeWidth(width: number | null) {
  try {
    if (width === null) {
      localStorage.removeItem(WIDTH_STORAGE_KEY);
    } else {
      localStorage.setItem(WIDTH_STORAGE_KEY, String(width));
    }
  } catch (error) {
    console.error("Failed to persist the width", error);
  }
}

function startResize(event: PointerEvent) {
  const startX = event.clientX;
  const measured = (event.currentTarget as HTMLElement).parentElement?.getBoundingClientRect().width ?? 0;
  const startWidth = measured || pageWidth.value || MIN_PAGE_WIDTH;
  resizing.value = true;
  (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);

  const onMove = (move: PointerEvent) => {
    // Centred page: the edge follows the cursor when width grows by 2 × delta.
    const next = startWidth + (move.clientX - startX) * 2;
    pageWidth.value = Math.round(Math.min(Math.max(next, MIN_PAGE_WIDTH), maxPageWidth()));
  };
  const onUp = () => {
    resizing.value = false;
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onUp);
    storeWidth(pageWidth.value);
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
}

/** Double-click the handle to go back to the default width. */
function resetWidth() {
  pageWidth.value = null;
  storeWidth(null);
}

onMounted(() => {
  restoreWidth();
  void store.refresh();
  window.addEventListener("keydown", handleKeydown);
  window.addEventListener("mousedown", handleWindowMouseDown);
});

onUnmounted(() => {
  window.removeEventListener("keydown", handleKeydown);
  window.removeEventListener("mousedown", handleWindowMouseDown);
  window.clearTimeout(statusTimer);
});

/* ---------------------------------------------------------------- listing */

const scopedConnections = computed(() => connections.value.filter((connection) => matchesScope(connection, scope.value)));

const visibleRows = computed(() => sortConnections(
  filterConnections(scopedConnections.value, searchQuery.value),
  sortKey.value,
  sortDirection.value,
));

const selected = computed(() => connections.value.find((connection) => connection.id === selectedId.value) ?? null);

const checkedConnections = computed(() => connections.value.filter((connection) => checkedIds.value.has(connection.id)));

// Keep the detail column pointed at something that is actually on screen.
watch(visibleRows, (rows) => {
  if (selectedId.value && rows.some((row) => row.id === selectedId.value)) {
    return;
  }
  selectedId.value = rows[0]?.id ?? null;
}, { immediate: true });

/** Groups in use, plus the explicitly created ones that are still empty. */
const allGroupPaths = computed(() => {
  const paths = new Set(collectGroupPaths(connections.value));
  for (const group of props.bookmarkGroups) {
    let path = "";
    for (const part of normalizeBookmarkPath(group)) {
      path = path ? `${path}/${part}` : part;
      paths.add(path);
    }
  }
  return [...paths].sort((left, right) => left.localeCompare(right, undefined, { sensitivity: "base" }));
});

const folderRows = computed(() => flattenBookmarkTree(
  buildBookmarkTree(connections.value, props.bookmarkGroups),
  collapsedGroups.value,
).filter((row): row is { kind: "folder"; key: string; depth: number; folder: BookmarkFolder } => row.kind === "folder"));

const recentCount = computed(() => connections.value.filter((connection) => connection.lastUsed).length);
const ungroupedCount = computed(() => connections.value.filter((connection) => !connection.group?.trim()).length);

/** The group the tree is pointed at, used as the default target for imports. */
const currentGroupPath = computed(() => (scope.value.kind === "group" ? scope.value.path : ""));

function isScopeActive(candidate: BookmarkScope) {
  if (candidate.kind !== scope.value.kind) {
    return false;
  }
  return candidate.kind !== "group" || candidate.path === (scope.value as { path: string }).path;
}

function selectScope(next: BookmarkScope) {
  scope.value = next;
  clearChecked();
}

function handleSort(key: BookmarkSortKey) {
  if (sortKey.value === key) {
    sortDirection.value = sortDirection.value === "asc" ? "desc" : "asc";
    return;
  }
  sortKey.value = key;
  // Recency reads newest-first; everything else reads A→Z.
  sortDirection.value = key === "lastUsed" ? "desc" : "asc";
}

/* -------------------------------------------------------------- selection */

function handleSelect(id: string) {
  selectedId.value = id;
  anchorId.value = id;
}

function toggleChecked(id: string) {
  const next = new Set(checkedIds.value);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  checkedIds.value = next;
  anchorId.value = id;
}

function handleRange(id: string) {
  const rows = visibleRows.value;
  const anchorIndex = rows.findIndex((row) => row.id === (anchorId.value ?? selectedId.value));
  const targetIndex = rows.findIndex((row) => row.id === id);
  if (anchorIndex === -1 || targetIndex === -1) {
    toggleChecked(id);
    return;
  }
  const [from, to] = anchorIndex <= targetIndex ? [anchorIndex, targetIndex] : [targetIndex, anchorIndex];
  const next = new Set(checkedIds.value);
  for (let index = from; index <= to; index += 1) {
    next.add(rows[index].id);
  }
  checkedIds.value = next;
  selectedId.value = id;
}

function toggleAll() {
  const rows = visibleRows.value;
  const everyChecked = rows.length > 0 && rows.every((row) => checkedIds.value.has(row.id));
  const next = new Set(checkedIds.value);
  for (const row of rows) {
    if (everyChecked) {
      next.delete(row.id);
    } else {
      next.add(row.id);
    }
  }
  checkedIds.value = next;
}

function clearChecked() {
  checkedIds.value = new Set();
}

/* ------------------------------------------------------------ bookmark ops */

async function connectTo(connection: SavedConnection) {
  await store.touch(connection.id);
  emit("connect", connection);
  emit("close");
}

/** Turn a rejected action into a dialog, naming the lock when that is the cause. */
async function reportFailure(messageKey: string, error: unknown) {
  if (error instanceof CredentialsLockedError) {
    await alertDialog(t("bookmarkManager.lockedBlocked"), "warning");
    return;
  }
  await alertDialog(t(messageKey, { error: String(error) }), "error");
}

async function handleSave(normalized: SavedConnection) {
  try {
    await store.save(normalized);
    statusMessage.value = t("bookmarkManager.saved", { name: normalized.name });
    editorNonce.value += 1;
  } catch (error) {
    await reportFailure("bookmarks.saveFailed", error);
  }
}

/** Delete a set of bookmarks after one confirmation naming what goes away. */
async function deleteBookmarks(targets: readonly SavedConnection[]) {
  if (targets.length === 0) {
    return;
  }
  const confirmed = await confirmDialog(targets.length === 1
    ? t("bookmarkManager.confirmDeleteOne", { name: targets[0].name })
    : t("bookmarkManager.confirmDeleteMany", { count: targets.length }));
  if (!confirmed) {
    return;
  }
  busy.value = true;
  try {
    const ids = targets.map((connection) => connection.id);
    const removed = await store.removeMany(ids);
    const next = new Set(checkedIds.value);
    for (const id of ids) {
      next.delete(id);
    }
    checkedIds.value = next;
    statusMessage.value = t("bookmarkManager.deleted", { count: removed });
  } catch (error) {
    await reportFailure("bookmarkManager.deleteFailed", error);
  } finally {
    busy.value = false;
  }
}

function deleteChecked() {
  return deleteBookmarks(checkedConnections.value);
}

async function duplicateBookmarks(targets: readonly SavedConnection[]) {
  if (targets.length === 0) {
    return;
  }
  busy.value = true;
  try {
    const created = await store.duplicateMany(
      targets.map((connection) => connection.id),
      (name) => t("bookmarkManager.copyName", { name }),
    );
    clearChecked();
    statusMessage.value = t("bookmarkManager.duplicated", { count: created.length });
  } catch (error) {
    await reportFailure("bookmarkManager.duplicateFailed", error);
  } finally {
    busy.value = false;
  }
}

function duplicateChecked() {
  return duplicateBookmarks(checkedConnections.value);
}

const MOVE_UNGROUPED = " ungrouped";
const MOVE_NEW_GROUP = " new";
const moveSelection = ref("");

async function handleMoveChange(event: Event) {
  const select = event.target as HTMLSelectElement;
  const choice = select.value;
  select.value = "";
  if (!choice) {
    return;
  }

  let group: string | undefined;
  if (choice === MOVE_UNGROUPED) {
    group = undefined;
  } else if (choice === MOVE_NEW_GROUP) {
    const entered = await promptText(t("bookmarkManager.newGroupPrompt"));
    if (entered === null || !entered.trim()) {
      return;
    }
    group = entered.trim();
  } else {
    group = choice;
  }

  await moveBookmarks(checkedConnections.value.map((connection) => connection.id), group);
}

async function moveBookmarks(ids: readonly string[], group: string | undefined) {
  busy.value = true;
  try {
    const moved = await store.moveMany(ids, group);
    statusMessage.value = t("bookmarkManager.moved", {
      count: moved,
      group: group ?? t("bookmarkEditor.ungrouped"),
    });
  } catch (error) {
    await reportFailure("bookmarkManager.moveFailed", error);
  } finally {
    busy.value = false;
  }
}

/* --------------------------------------------------------------- group ops */

function openFolderMenu(event: MouseEvent, path: string) {
  event.preventDefault();
  rowMenu.value = null;
  folderMenu.value = { x: event.clientX, y: event.clientY, path };
}

/** Right-clicking a row acts on that row, or on the whole checked selection
 *  when the row is part of it. */
function openRowMenu(event: MouseEvent, connection: SavedConnection) {
  event.preventDefault();
  folderMenu.value = null;
  if (!checkedIds.value.has(connection.id)) {
    handleSelect(connection.id);
  }
  rowMenu.value = { x: event.clientX, y: event.clientY, connection };
}

/** The bookmarks a row action applies to. */
function rowTargets(connection: SavedConnection) {
  return checkedIds.value.has(connection.id) && checkedConnections.value.length > 1
    ? checkedConnections.value
    : [connection];
}

async function runRowAction(connection: SavedConnection, action: "connect" | "duplicate" | "export" | "delete") {
  const targets = rowTargets(connection);
  rowMenu.value = null;
  if (action === "connect") {
    await connectTo(connection);
    return;
  }
  if (action === "delete") {
    await deleteBookmarks(targets);
    return;
  }
  if (action === "duplicate") {
    await duplicateBookmarks(targets);
    return;
  }
  await exportIds(targets.map((item) => item.id), false);
}

function handleWindowMouseDown(event: MouseEvent) {
  const target = event.target as HTMLElement | null;
  if (!target?.closest(".bm-context-menu")) {
    folderMenu.value = null;
    rowMenu.value = null;
  }
  if (exportMenuOpen.value && !target?.closest(".bm-menu-anchor")) {
    exportMenuOpen.value = false;
  }
}

/** Remember groups that hold no bookmark yet, so they survive in the tree. */
function rememberGroups(paths: readonly string[]) {
  const merged = new Set(props.bookmarkGroups);
  for (const path of paths) {
    const normalized = normalizeBookmarkPath(path).join("/");
    if (normalized) {
      merged.add(normalized);
    }
  }
  if (merged.size === props.bookmarkGroups.length) {
    return;
  }
  emit("updateGroups", [...merged].sort((left, right) => left.localeCompare(right, undefined, { sensitivity: "base" })));
}

async function createGroup(parent = "") {
  folderMenu.value = null;
  const entered = await promptText(t("bookmarkManager.newGroupPrompt"));
  const name = entered?.trim();
  if (!name) {
    return;
  }
  const path = normalizeBookmarkPath(parent ? `${parent}/${name}` : name).join("/");
  if (!path) {
    return;
  }
  rememberGroups([path]);
  if (parent) {
    store.expandGroup(parent);
  }
  scope.value = { kind: "group", path };
  statusMessage.value = t("bookmarkManager.groupCreated", { group: path });
}

async function applyGroupRename(from: string, to: string, messageKey: string) {
  busy.value = true;
  try {
    const affected = await store.renameGroup(from, to);
    emit("updateGroups", rewriteGroupPaths(props.bookmarkGroups, from, to));
    if (scope.value.kind === "group" && scope.value.path === from) {
      scope.value = to ? { kind: "group", path: to } : { kind: "all" };
    }
    statusMessage.value = t(messageKey, { count: affected, group: to || t("bookmarkEditor.ungrouped") });
  } catch (error) {
    await reportFailure("bookmarkManager.renameGroupFailed", error);
  } finally {
    busy.value = false;
  }
}

async function renameFolder(path: string) {
  folderMenu.value = null;
  const parts = normalizeBookmarkPath(path);
  const entered = await promptText(t("bookmarkManager.renameGroupPrompt"), parts[parts.length - 1] ?? "");
  const name = entered?.trim();
  if (!name) {
    return;
  }
  const target = normalizeBookmarkPath([...parts.slice(0, -1), name].join("/")).join("/");
  if (!target || target === path) {
    return;
  }
  await applyGroupRename(path, target, "bookmarkManager.groupRenamed");
}

/** Dissolve the folder: bookmarks move up to its parent, subfolders too. */
async function dissolveFolder(path: string) {
  folderMenu.value = null;
  if (!await confirmDialog(t("bookmarkManager.confirmDissolveGroup", { group: path }))) {
    return;
  }
  const parent = normalizeBookmarkPath(path).slice(0, -1).join("/");
  await applyGroupRename(path, parent, "bookmarkManager.groupDissolved");
}

async function deleteFolder(path: string) {
  folderMenu.value = null;
  const doomed = connections.value.filter((connection) => matchesScope(connection, { kind: "group", path }));
  const confirmed = await confirmDialog(doomed.length === 0
    ? t("bookmarkManager.confirmDeleteEmptyGroup", { group: path })
    : t("bookmarkManager.confirmDeleteGroup", { group: path, count: doomed.length }));
  if (!confirmed) {
    return;
  }
  busy.value = true;
  try {
    if (doomed.length > 0) {
      await store.removeMany(doomed.map((connection) => connection.id));
    }
    emit("updateGroups", rewriteGroupPaths(props.bookmarkGroups, path, ""));
    if (scope.value.kind === "group" && scope.value.path === path) {
      scope.value = { kind: "all" };
    }
    clearChecked();
    statusMessage.value = t("bookmarkManager.groupDeleted", { group: path, count: doomed.length });
  } catch (error) {
    await reportFailure("bookmarkManager.deleteFailed", error);
  } finally {
    busy.value = false;
  }
}

/* ------------------------------------------------------------ drag to move */

function handleRowDragStart(event: DragEvent, connection: SavedConnection) {
  // Dragging a checked row moves the whole selection; an unchecked one moves alone.
  const ids = checkedIds.value.has(connection.id) ? [...checkedIds.value] : [connection.id];
  draggingIds.value = ids;
  event.dataTransfer?.setData("text/plain", ids.join(","));
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = "move";
  }
}

function handleRowDragEnd() {
  draggingIds.value = [];
  dropTarget.value = null;
}

function handleDragOver(event: DragEvent, key: string) {
  if (draggingIds.value.length === 0) {
    return;
  }
  event.preventDefault();
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = "move";
  }
  dropTarget.value = key;
}

async function handleDrop(group: string | undefined) {
  const ids = draggingIds.value;
  handleRowDragEnd();
  if (ids.length > 0) {
    await moveBookmarks(ids, group);
  }
}

/* ----------------------------------------------------------- import/export */

function triggerImport() {
  exportMenuOpen.value = false;
  importInput.value?.click();
}

async function handleImportFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) {
    return;
  }
  const content = await file.text();
  const format = detectImportFormat(file.name, content);
  const target = await promptText(t("bookmarkManager.importGroupPrompt"), currentGroupPath.value);
  if (target === null) {
    return;
  }

  busy.value = true;
  try {
    const result = await store.importBookmarks(format, content, target.trim() || undefined);
    // Subfolders a share brought along that hold no bookmark: nothing in the
    // connection list can rebuild them, so they have to be recorded explicitly.
    rememberGroups(result.createdGroups);
    statusMessage.value = t("bookmarks.importResult", { imported: result.imported, skipped: result.skipped })
      + (result.warnings.length ? ` · ${result.warnings.join(" ")}` : "");
  } catch (error) {
    await alertDialog(t("bookmarks.importFailed", { error: String(error) }), "error");
  } finally {
    busy.value = false;
  }
}

/** Write the given bookmarks (null = all) out as a file the user can keep. */
async function exportIds(ids: readonly string[] | null, includeSecrets: boolean) {
  busy.value = true;
  try {
    const content = await store.exportBookmarks(ids, includeSecrets);
    downloadText(exportFileName(ids ? "selection" : "all"), content);
    statusMessage.value = t("bookmarkManager.exported", { count: ids ? ids.length : connections.value.length });
  } catch (error) {
    await reportFailure("bookmarkManager.exportFailed", error);
  } finally {
    busy.value = false;
  }
}

/**
 * Pack a whole group — subtree, empty subfolders and all — into a share file.
 *
 * Group paths inside are relative to `path`, so the recipient can graft it
 * anywhere and never sees the folders we keep above it.
 */
async function exportFolder(path: string) {
  folderMenu.value = null;
  exportMenuOpen.value = false;
  busy.value = true;
  try {
    const content = await store.exportGroup(path, props.bookmarkGroups);
    downloadText(shareFileName(path), content);
    statusMessage.value = t("bookmarkManager.groupExported", { group: path });
  } catch (error) {
    await reportFailure("bookmarkManager.exportFailed", error);
  } finally {
    busy.value = false;
  }
}

async function exportBookmarks(target: "all" | "selection", includeSecrets: boolean) {
  exportMenuOpen.value = false;
  if (includeSecrets && !await confirmDialog(t("bookmarkManager.confirmExportSecrets"), "warning")) {
    return;
  }
  await exportIds(
    target === "selection" ? checkedConnections.value.map((connection) => connection.id) : null,
    includeSecrets,
  );
}

/* --------------------------------------------------------------- keyboard */

function handleKeydown(event: KeyboardEvent) {
  const target = event.target as HTMLElement | null;
  const typing = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA" || target?.tagName === "SELECT";

  if (event.key === "Escape") {
    event.preventDefault();
    if (folderMenu.value || rowMenu.value || exportMenuOpen.value) {
      folderMenu.value = null;
      rowMenu.value = null;
      exportMenuOpen.value = false;
    } else if (searchQuery.value) {
      searchQuery.value = "";
    } else {
      emit("close");
    }
    return;
  }

  if (typing) {
    return;
  }

  if (event.key === "/" || ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f")) {
    event.preventDefault();
    searchInput.value?.focus();
    searchInput.value?.select();
  }
}

// Status lines are transient; they must not linger over the next action.
watch(statusMessage, (value) => {
  window.clearTimeout(statusTimer);
  if (!value) {
    return;
  }
  statusTimer = window.setTimeout(() => {
    if (statusMessage.value === value) {
      statusMessage.value = "";
    }
  }, 4000);
});
</script>

<template>
  <div class="bm-overlay" @click.self="emit('close')">
    <div class="bm-page" :class="{ resizing }" :style="pageStyle">

      <div
        class="bm-resize"
        :class="{ dragging: resizing }"
        role="separator"
        aria-orientation="vertical"
        :title="$t('bookmarkManager.resizeHint')"
        @pointerdown.prevent="startResize"
        @dblclick="resetWidth"
      />

      <header class="bm-head">
        <span class="bm-title">🔖 {{ $t('bookmarkManager.title') }}</span>
        <div class="bm-search">
          <input
            ref="searchInput"
            v-model="searchQuery"
            type="text"
            :placeholder="$t('bookmarkManager.searchPlaceholder')"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
          >
          <button v-if="searchQuery" class="bm-search-clear" type="button" @click="searchQuery = ''">×</button>
          <span v-else class="bm-kbd">/</span>
        </div>
        <button class="bm-btn bm-btn--primary" type="button" @click="emit('newConnection')">＋ {{ $t('bookmarkManager.newConnection') }}</button>
        <input ref="importInput" type="file" accept=".reg,.json,.conf,.config,text/plain" hidden @change="handleImportFile">
        <button class="bm-btn" type="button" :disabled="busy" :title="$t('bookmarkManager.importTitle')" @click="triggerImport">{{ $t('bookmarks.import') }}</button>
        <div class="bm-menu-anchor">
          <button class="bm-btn" type="button" :disabled="busy" @click="exportMenuOpen = !exportMenuOpen">{{ $t('bookmarkManager.export') }} ▾</button>
          <div v-if="exportMenuOpen" class="bm-menu">
            <button class="bm-menu-item" type="button" @click="exportBookmarks('all', false)">{{ $t('bookmarkManager.exportAll') }}</button>
            <button
              v-if="currentGroupPath"
              class="bm-menu-item"
              type="button"
              @click="exportFolder(currentGroupPath)"
            >{{ $t('bookmarkManager.exportGroup', { group: currentGroupPath }) }}</button>
            <button
              class="bm-menu-item"
              type="button"
              :disabled="checkedConnections.length === 0"
              @click="exportBookmarks('selection', false)"
            >{{ $t('bookmarkManager.exportSelection', { count: checkedConnections.length }) }}</button>
            <div class="bm-menu-sep" />
            <button class="bm-menu-item" type="button" @click="exportBookmarks('all', true)">{{ $t('bookmarkManager.exportWithSecrets') }}</button>
          </div>
        </div>
        <button class="bm-btn" type="button" :title="$t('bookmarks.refreshList')" @click="store.refresh()">↻</button>
        <button class="bm-btn bm-btn--ghost" type="button" :title="$t('common.close')" @click="emit('close')">×</button>
      </header>

      <div v-if="!credentialsUnlocked" class="bm-banner">{{ $t('bookmarkManager.lockedBanner') }}</div>
      <div v-else-if="statusMessage" class="bm-status" @click="statusMessage = ''">{{ statusMessage }}</div>

      <div class="bm-body">
        <!-- groups -->
        <nav class="bm-tree">
          <button
            class="bm-tree-item"
            type="button"
            :aria-pressed="isScopeActive({ kind: 'all' })"
            @click="selectScope({ kind: 'all' })"
          >
            {{ $t('bookmarkManager.scopeAll') }}<span class="bm-count">{{ connections.length }}</span>
          </button>
          <button
            class="bm-tree-item"
            type="button"
            :aria-pressed="isScopeActive({ kind: 'recent' })"
            @click="selectScope({ kind: 'recent' })"
          >
            {{ $t('bookmarks.recentlyUsed') }}<span class="bm-count">{{ recentCount }}</span>
          </button>
          <button
            class="bm-tree-item"
            type="button"
            :class="{ 'bm-drop-target': dropTarget === '' }"
            :aria-pressed="isScopeActive({ kind: 'ungrouped' })"
            @click="selectScope({ kind: 'ungrouped' })"
            @dragover="handleDragOver($event, '')"
            @dragleave="dropTarget = null"
            @drop.prevent="handleDrop(undefined)"
          >
            {{ $t('bookmarkEditor.ungrouped') }}<span class="bm-count">{{ ungroupedCount }}</span>
          </button>

          <div class="bm-tree-sep" />

          <div
            v-for="row in folderRows"
            :key="row.key"
            class="bm-tree-row"
            :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
          >
            <button
              class="bm-tree-arrow"
              type="button"
              :class="{ collapsed: store.isGroupCollapsed(row.folder.path) }"
              :aria-label="$t('bookmarkManager.toggleGroup')"
              @click="store.toggleGroup(row.folder.path)"
            >›</button>
            <button
              class="bm-tree-item bm-tree-item--folder"
              type="button"
              :class="{ 'bm-drop-target': dropTarget === row.folder.path }"
              :aria-pressed="isScopeActive({ kind: 'group', path: row.folder.path })"
              @click="selectScope({ kind: 'group', path: row.folder.path })"
              @contextmenu="openFolderMenu($event, row.folder.path)"
              @dragover="handleDragOver($event, row.folder.path)"
              @dragleave="dropTarget = null"
              @drop.prevent="handleDrop(row.folder.path)"
            >
              {{ row.folder.name }}<span class="bm-count">{{ row.folder.count }}</span>
            </button>
          </div>

          <button class="bm-tree-new" type="button" :disabled="busy" @click="createGroup()">＋ {{ $t('bookmarkManager.newGroup') }}</button>
        </nav>

        <!-- list -->
        <div class="bm-list">
          <div v-if="connections.length === 0" class="bm-empty">
            <p>{{ $t('bookmarks.emptyTitle') }}</p>
            <p class="bm-empty-hint">{{ $t('bookmarks.emptyHint') }}</p>
          </div>
          <div v-else-if="visibleRows.length === 0" class="bm-empty">
            <p>{{ $t('bookmarks.noMatch') }}</p>
          </div>
          <BookmarkManagerList
            v-else
            :rows="visibleRows"
            :selected-id="selectedId"
            :checked-ids="checkedIds"
            :sort-key="sortKey"
            :sort-direction="sortDirection"
            @select="handleSelect"
            @toggle="toggleChecked"
            @range="handleRange"
            @open="connectTo"
            @sort="handleSort"
            @toggle-all="toggleAll"
            @row-context="openRowMenu"
            @row-drag-start="handleRowDragStart"
            @row-drag-end="handleRowDragEnd"
          />
        </div>

        <!-- detail -->
        <aside class="bm-detail">
          <template v-if="checkedConnections.length > 1">
            <div class="bm-detail-head">
              <span class="bm-detail-name">{{ $t('bookmarkManager.selectedCount', { count: checkedConnections.length }) }}</span>
            </div>
            <ul class="bm-detail-names">
              <li v-for="connection in checkedConnections" :key="connection.id">{{ connection.name }}</li>
            </ul>
            <p class="bm-detail-note">{{ $t('bookmarkManager.batchHint') }}</p>
          </template>
          <template v-else-if="selected">
            <div class="bm-detail-head">
              <span class="bm-detail-name">{{ selected.name }}</span>
              <button class="bm-btn bm-btn--primary" type="button" @click="connectTo(selected)">{{ $t('connect.connect') }}</button>
              <button class="bm-btn bm-btn--danger" type="button" @click="deleteBookmarks([selected])">{{ $t('common.delete') }}</button>
            </div>
            <BookmarkEditor
              :key="`${selected.id}-${editorNonce}`"
              :connection="selected"
              :settings="props.settings"
              :groups="allGroupPaths"
              embedded
              @save="handleSave"
              @cancel="editorNonce += 1"
            />
          </template>
          <div v-else class="bm-empty bm-empty--detail">
            <p>{{ $t('bookmarkManager.noSelection') }}</p>
          </div>
        </aside>
      </div>

      <footer v-if="checkedConnections.length > 0" class="bm-batch">
        <span class="bm-batch-count">{{ $t('bookmarkManager.selectedCount', { count: checkedConnections.length }) }}</span>
        <span class="bm-batch-spacer" />
        <select class="bm-btn bm-move-select" :value="moveSelection" :disabled="busy" @change="handleMoveChange">
          <option value="">{{ $t('bookmarkManager.moveTo') }}</option>
          <option :value="MOVE_UNGROUPED">{{ $t('bookmarkEditor.ungrouped') }}</option>
          <option v-for="group in allGroupPaths" :key="group" :value="group">{{ group }}</option>
          <option :value="MOVE_NEW_GROUP">{{ $t('connect.groupNew') }}</option>
        </select>
        <button class="bm-btn" type="button" :disabled="busy" @click="exportBookmarks('selection', false)">{{ $t('bookmarkManager.export') }}</button>
        <button class="bm-btn" type="button" :disabled="busy" @click="duplicateChecked">{{ $t('bookmarkManager.duplicate') }}</button>
        <button class="bm-btn bm-btn--danger" type="button" :disabled="busy" @click="deleteChecked">{{ $t('common.delete') }}</button>
        <button class="bm-btn bm-btn--ghost" type="button" @click="clearChecked">{{ $t('bookmarkManager.clearSelection') }}</button>
      </footer>

      <div
        v-if="rowMenu"
        class="bm-context-menu"
        :style="{ top: `${rowMenu.y}px`, left: `${rowMenu.x}px` }"
      >
        <button class="bm-menu-item" type="button" @click="runRowAction(rowMenu.connection, 'connect')">▸ {{ $t('connect.connect') }}</button>
        <button class="bm-menu-item" type="button" @click="runRowAction(rowMenu.connection, 'duplicate')">⧉ {{ $t('bookmarkManager.duplicate') }}</button>
        <button class="bm-menu-item" type="button" @click="runRowAction(rowMenu.connection, 'export')">⤓ {{ $t('bookmarkManager.export') }}</button>
        <div class="bm-menu-sep" />
        <button class="bm-menu-item danger" type="button" @click="runRowAction(rowMenu.connection, 'delete')">🗑 {{ $t('common.delete') }}</button>
      </div>

      <div
        v-if="folderMenu"
        class="bm-context-menu"
        :style="{ top: `${folderMenu.y}px`, left: `${folderMenu.x}px` }"
      >
        <button class="bm-menu-item" type="button" @click="renameFolder(folderMenu.path)">✏️ {{ $t('bookmarkManager.renameGroup') }}</button>
        <button class="bm-menu-item" type="button" @click="createGroup(folderMenu.path)">📁 {{ $t('bookmarkManager.newSubgroup') }}</button>
        <button class="bm-menu-item" type="button" @click="exportFolder(folderMenu.path)">⤓ {{ $t('bookmarkManager.shareGroup') }}</button>
        <button class="bm-menu-item" type="button" @click="dissolveFolder(folderMenu.path)">↥ {{ $t('bookmarkManager.dissolveGroup') }}</button>
        <div class="bm-menu-sep" />
        <button class="bm-menu-item danger" type="button" @click="deleteFolder(folderMenu.path)">🗑 {{ $t('bookmarkManager.deleteGroup') }}</button>
      </div>

    </div>
  </div>
</template>
