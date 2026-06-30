<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import { type SavedConnection } from "./types";
import { buildBookmarkTree, flattenBookmarkTree } from "./bookmarks";
import { t } from "./i18n";
import BookmarkEditor from "./BookmarkEditor.vue";

const props = withDefaults(defineProps<{
  refreshToken?: number;
  settings?: AppSettings;
  expandGroup?: string;
}>(), {
  settings: () => DEFAULT_SETTINGS,
  expandGroup: undefined,
});

const emit = defineEmits<{
  connect: [connection: SavedConnection];
}>();

const RECENTLY_USED_LABEL = computed(() => t("bookmarks.recentlyUsed"));

function buildSubtitle(connection: SavedConnection) {
  if (connection.protocol === "serial") {
    return `${connection.portName ?? "serial"} · ${connection.baudRate ?? 9600} baud`;
  }
  if (connection.protocol === "telnet") {
    return `telnet://${connection.host}:${connection.port}`;
  }
  return `${connection.user}@${connection.host}:${connection.port}`;
}

function buildIcon(connection: SavedConnection) {
  if (connection.protocol === "serial") {
    return "🔌";
  }
  if (connection.protocol === "telnet") {
    return "🌐";
  }
  return "🖥";
}

const connections = ref<SavedConnection[]>([]);
const contextMenu = ref<{ x: number; y: number; connection: SavedConnection } | null>(null);
const editingConnection = ref<SavedConnection | null>(null);
const contextMenuRef = ref<HTMLDivElement | null>(null);
const collapsedGroups = ref<Set<string>>(new Set());
const searchQuery = ref("");
const importFileInput = ref<HTMLInputElement | null>(null);
const importMessage = ref("");

interface BookmarkImportResult {
  imported: number;
  skipped: number;
  warnings: string[];
}

const STORAGE_KEY_COLLAPSED = "auraterm:collapsed-groups";

function loadCollapsedState() {
  try {
    const saved = localStorage.getItem(STORAGE_KEY_COLLAPSED);
    if (saved) {
      collapsedGroups.value = new Set(JSON.parse(saved));
    }
  } catch (error) {
    console.error("Failed to load collapsed state", error);
  }
}

function saveCollapsedState() {
  localStorage.setItem(STORAGE_KEY_COLLAPSED, JSON.stringify(Array.from(collapsedGroups.value)));
}

function toggleGroup(group: string) {
  const next = new Set(collapsedGroups.value);
  if (next.has(group)) {
    next.delete(group);
  } else {
    next.add(group);
  }
  collapsedGroups.value = next;
  saveCollapsedState();
}

function isGroupCollapsed(group: string) {
  return collapsedGroups.value.has(group);
}

async function loadConnections(expandGroup?: string) {
  try {
    connections.value = await invoke<SavedConnection[]>("get_connections");
    // 自动展开指定分组（如新保存的连接所在分组）
    // 必须用赋值新 Set 来触发 Vue 响应式，直 .delete() 不会重新渲染
    if (expandGroup && collapsedGroups.value.has(expandGroup)) {
      const next = new Set(collapsedGroups.value);
      next.delete(expandGroup);
      collapsedGroups.value = next;
    }
  } catch (error) {
    console.error("Failed to load connections", error);
  }
}

watch(() => props.refreshToken, () => {
  // 先同步恢复折叠状态，再异步加载数据并在加载完成后展开目标分组
  // （顺序不能颠倒：loadCollapsedState 是同步的，必须在 loadConnections 的 await 之前完成）
  loadCollapsedState();
  void loadConnections(props.expandGroup);
}, { immediate: true });

watch(contextMenu, (value, _previous, onCleanup) => {
  if (!value) {
    return;
  }

  const handleClickOutside = (event: MouseEvent) => {
    if (contextMenuRef.value && !contextMenuRef.value.contains(event.target as Node)) {
      contextMenu.value = null;
    }
  };

  document.addEventListener("mousedown", handleClickOutside);
  onCleanup(() => {
    document.removeEventListener("mousedown", handleClickOutside);
  });
});

const filteredConnections = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) {
    return connections.value;
  }
  return connections.value.filter((c) => {
    return (
      c.name.toLowerCase().includes(query) ||
      c.host.toLowerCase().includes(query) ||
      (c.user && c.user.toLowerCase().includes(query)) ||
      (c.portName && c.portName.toLowerCase().includes(query)) ||
      (c.group && c.group.toLowerCase().includes(query))
    );
  });
});

const recentlyUsed = computed(() => [...connections.value]
    .filter(c => c.lastUsed)
    .sort((a, b) => (b.lastUsed || 0) - (a.lastUsed || 0))
    .slice(0, 5));

const bookmarkRows = computed(() => flattenBookmarkTree(
  buildBookmarkTree(filteredConnections.value),
  collapsedGroups.value,
));

async function handleDoubleClick(connection: SavedConnection) {
  try {
    await invoke("touch_connection", { id: connection.id, timestamp: Date.now() });
  } catch {
  }
  emit("connect", connection);
}

function handleContextMenu(event: MouseEvent, connection: SavedConnection) {
  event.preventDefault();
  contextMenu.value = { x: event.clientX, y: event.clientY, connection };
}

async function handleDelete(id: string) {
  try {
    await invoke("delete_connection", { id });
    connections.value = connections.value.filter((connection) => connection.id !== id);
  } catch (error) {
    console.error("Failed to delete connection", error);
  }
  contextMenu.value = null;
}

function openEditDialog(connection: SavedConnection) {
  editingConnection.value = connection;
  contextMenu.value = null;
}

function closeEditDialog() {
  editingConnection.value = null;
}

async function handleSaveConnection(normalized: SavedConnection) {
  try {
    await invoke("save_connection", { connection: normalized });
    connections.value = connections.value.map((connection) => (
      connection.id === editingConnection.value?.id ? normalized : connection
    ));
    closeEditDialog();
  } catch (error) {
    console.error("Failed to save connection", error);
    // Note: Error handling is now partially inside BookmarkEditor, 
    // but we catch backend errors here.
    alert(t("bookmarks.saveFailed", { error: String(error) }));
  }
}

async function handleImportFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  importMessage.value = t("bookmarks.importing");
  try {
    const format = file.name.toLowerCase().endsWith(".reg") ? "putty" : "openssh";
    const result = await invoke<BookmarkImportResult>("import_bookmarks", {
      format,
      content: await file.text(),
      group: null,
    });
    importMessage.value = t("bookmarks.importResult", { imported: result.imported, skipped: result.skipped })
      + (result.warnings.length ? `. ${result.warnings.join(" ")}` : "");
    await loadConnections();
  } catch (error) {
    importMessage.value = t("bookmarks.importFailed", { error: String(error) });
  } finally {
    input.value = "";
  }
}
</script>

<template>
  <div class="bookmark-sidebar">
    <div class="bookmark-sidebar-header">
      <span class="bookmark-sidebar-title">{{ $t('bookmarks.quickConnect') }}</span>
      <div class="bookmark-header-actions">
        <input ref="importFileInput" type="file" accept=".reg,.conf,.config,text/plain" hidden @change="handleImportFile">
        <button class="bookmark-refresh-btn" :title="$t('bookmarks.importTitle')" @click="importFileInput?.click()">{{ $t('bookmarks.import') }}</button>
        <button class="bookmark-refresh-btn" :title="$t('bookmarks.refreshList')" @click="() => loadConnections()">↻</button>
      </div>
    </div>

    <div v-if="importMessage" class="bookmark-import-message" @click="importMessage = ''">{{ importMessage }}</div>

    <div class="bookmark-search-container">
      <input
        v-model="searchQuery"
        type="text"
        class="bookmark-search-input"
        :placeholder="$t('bookmarks.searchPlaceholder')"
        autocapitalize="none"
        autocorrect="off"
        spellcheck="false"
      >
      <button v-if="searchQuery" class="bookmark-search-clear" @click="searchQuery = ''">×</button>
    </div>

    <div v-if="connections.length === 0" class="bookmark-empty">
      {{ $t('bookmarks.emptyTitle') }}
      <br>
      <br>
      {{ $t('bookmarks.emptyHint') }}
    </div>

    <div v-else-if="filteredConnections.length === 0" class="bookmark-empty">
      {{ $t('bookmarks.noMatch') }}
    </div>

    <div v-else class="bookmark-list">
      <div v-if="recentlyUsed.length && !searchQuery.trim()" class="bookmark-group" :class="{ collapsed: isGroupCollapsed(RECENTLY_USED_LABEL) }">
        <div class="bookmark-group-header" @click="toggleGroup(RECENTLY_USED_LABEL)">
          <div class="bookmark-group-header-left">
            <span class="bookmark-group-arrow">›</span>
            <span class="bookmark-group-name">{{ RECENTLY_USED_LABEL }}</span>
          </div>
          <span class="bookmark-group-count">{{ recentlyUsed.length }}</span>
        </div>
        <ul v-if="!isGroupCollapsed(RECENTLY_USED_LABEL)" class="bookmark-group-list">
          <li
            v-for="connection in recentlyUsed"
            :key="connection.id"
            class="bookmark-item"
            :title="`${buildSubtitle(connection)}\n${$t('bookmarks.doubleClickToConnect')}`"
            @dblclick="handleDoubleClick(connection)"
            @contextmenu="handleContextMenu($event, connection)"
          >
            <span class="bookmark-icon">{{ buildIcon(connection) }}</span>
            <div class="bookmark-info">
              <span class="bookmark-name">{{ connection.name }}</span>
              <span class="bookmark-host">{{ buildSubtitle(connection) }}</span>
            </div>
          </li>
        </ul>
      </div>

      <template v-for="row in bookmarkRows" :key="row.key">
        <div
          v-if="row.kind === 'folder'"
          class="bookmark-group-header bookmark-tree-folder"
          :class="{ collapsed: isGroupCollapsed(row.folder.path) }"
          :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
          @click="toggleGroup(row.folder.path)"
        >
          <div class="bookmark-group-header-left">
            <span class="bookmark-group-arrow">›</span>
            <span class="bookmark-group-name">{{ row.folder.name }}</span>
          </div>
          <span class="bookmark-group-count">{{ row.folder.count }}</span>
        </div>
        <div
          v-else
          class="bookmark-item"
          :style="{ paddingLeft: `${16 + row.depth * 14}px` }"
          :title="`${buildSubtitle(row.connection)}\n${$t('bookmarks.doubleClickToConnect')}`"
          @dblclick="handleDoubleClick(row.connection)"
          @contextmenu="handleContextMenu($event, row.connection)"
        >
          <span class="bookmark-icon">{{ buildIcon(row.connection) }}</span>
          <div class="bookmark-info">
            <span class="bookmark-name">{{ row.connection.name }}</span>
            <span class="bookmark-host">{{ buildSubtitle(row.connection) }}</span>
          </div>
        </div>
      </template>
    </div>

    <div
      v-if="contextMenu"
      ref="contextMenuRef"
      class="bookmark-context-menu"
      :style="{ top: `${contextMenu.y}px`, left: `${contextMenu.x}px` }"
    >
      <button class="bookmark-context-item" @click="openEditDialog(contextMenu.connection)">✏️ {{ $t('common.edit') }}</button>
      <button class="bookmark-context-item danger" @click="handleDelete(contextMenu.connection.id)">🗑 {{ $t('common.delete') }}</button>
    </div>

    <BookmarkEditor
      v-if="editingConnection"
      :connection="editingConnection"
      :settings="props.settings"
      @save="handleSaveConnection"
      @cancel="closeEditDialog"
    />
  </div>
</template>

<style>
.bookmark-sidebar {
  width: 260px;
  background: var(--app-sidebar-bg);
  border-right: 1px solid var(--app-border);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  user-select: none;
}

.bookmark-sidebar-header {
  padding: 12px 16px;
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.bookmark-sidebar-title {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--app-text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.bookmark-refresh-btn {
  background: none;
  border: none;
  color: var(--app-text-muted);
  cursor: pointer;
  font-size: 1.1rem;
  padding: 4px;
  border-radius: 4px;
  line-height: 1;
}

.bookmark-refresh-btn:hover {
  background: var(--app-hover);
  color: var(--app-text);
}

.bookmark-search-container {
  padding: 6px 12px 12px 12px;
  position: relative;
  border-bottom: 1px solid var(--app-border);
}

.bookmark-search-input {
  width: 100%;
  box-sizing: border-box;
  background: var(--app-input-bg);
  border: 1px solid var(--app-border);
  border-radius: 4px;
  padding: 4px 24px 4px 8px;
  color: var(--app-text-secondary);
  font-size: 0.8rem;
  outline: none;
}

.bookmark-search-input:focus {
  border-color: var(--app-border-accent);
}

.bookmark-search-clear {
  position: absolute;
  right: 18px;
  top: 4px;
  background: none;
  border: none;
  color: var(--app-text-muted);
  cursor: pointer;
  font-size: 1rem;
  padding: 0;
  line-height: 1;
}

.bookmark-search-clear:hover {
  color: var(--app-text);
}

.bookmark-empty {
  padding: 40px 20px;
  text-align: center;
  color: var(--app-text-dim);
  font-size: 0.85rem;
  line-height: 1.6;
}

.bookmark-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}

.bookmark-group {
  margin-bottom: 12px;
}

.bookmark-group-header {
  padding: 4px 12px 4px 8px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: var(--app-surface-2);
  margin-bottom: 4px;
  cursor: pointer;
}

.bookmark-group-header:hover {
  background: var(--app-hover);
}

.bookmark-group-header-left {
  display: flex;
  align-items: center;
  gap: 4px;
}

.bookmark-group-arrow {
  font-size: 1.1rem;
  color: var(--app-text-dim);
  transition: transform 0.2s;
  width: 16px;
  text-align: center;
  display: inline-block;
  transform: rotate(90deg);
}

.bookmark-group.collapsed .bookmark-group-arrow {
  transform: rotate(0deg);
}

.bookmark-group-name {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--app-text-muted);
}

.bookmark-group[data-group="Recently Used"] .bookmark-group-header {
  border-top: 1px solid var(--app-border);
}

.bookmark-group[data-group="Recently Used"] .bookmark-group-name {
  color: var(--app-accent);
  font-style: italic;
}

.bookmark-group-count {
  font-size: 0.7rem;
  color: var(--app-text-muted);
  background: var(--app-surface-3);
  padding: 1px 6px;
  border-radius: 10px;
}

.bookmark-group-list {
  list-style: none;
  padding: 0;
  margin: 0;
}

.bookmark-item {
  padding: 8px 16px;
  display: flex;
  align-items: center;
  gap: 12px;
  cursor: pointer;
  transition: background 0.1s;
}

.bookmark-item:hover {
  background: var(--app-hover-soft);
}

.bookmark-icon {
  font-size: 1.2rem;
  opacity: 0.8;
}

.bookmark-info {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.bookmark-name {
  font-size: 0.9rem;
  color: var(--app-text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bookmark-host {
  font-size: 0.75rem;
  color: var(--app-text-dim);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bookmark-context-menu {
  position: fixed;
  background: var(--app-menu-bg);
  border: 1px solid var(--app-border);
  box-shadow: 0 4px 12px var(--app-shadow);
  border-radius: 4px;
  padding: 4px;
  z-index: 1000;
  min-width: 120px;
}

.bookmark-context-item {
  display: block;
  width: 100%;
  padding: 6px 12px;
  text-align: left;
  background: none;
  border: none;
  color: var(--app-text-secondary);
  font-size: 0.85rem;
  cursor: pointer;
  border-radius: 2px;
}

.bookmark-context-item:hover {
  background: var(--app-accent);
  color: var(--app-accent-contrast);
}

.bookmark-context-item.danger:hover {
  background: var(--app-danger);
}
</style>
