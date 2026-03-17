<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import { type SavedConnection } from "./types";
import BookmarkEditor from "./BookmarkEditor.vue";

const props = withDefaults(defineProps<{
  refreshToken?: number;
  settings?: AppSettings;
}>(), {
  settings: () => DEFAULT_SETTINGS,
});

const emit = defineEmits<{
  connect: [connection: SavedConnection];
}>();

const UNGROUPED_LABEL = "Ungrouped";
const RECENTLY_USED_LABEL = "Recently Used";

function toDisplayGroup(value?: string) {
  return value?.trim() || UNGROUPED_LABEL;
}

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
  if (collapsedGroups.value.has(group)) {
    collapsedGroups.value.delete(group);
  } else {
    collapsedGroups.value.add(group);
  }
  saveCollapsedState();
}

function isGroupCollapsed(group: string) {
  return collapsedGroups.value.has(group);
}

async function loadConnections() {
  try {
    connections.value = await invoke<SavedConnection[]>("get_connections");
  } catch (error) {
    console.error("Failed to load connections", error);
  }
}

watch(() => props.refreshToken, () => {
  void loadConnections();
  loadCollapsedState();
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

const groupedConnections = computed(() => {
  const groups = new Map<string, SavedConnection[]>();

  // Add "Recently Used" group if there is a search query or we just want them shown
  // But typically "Recently Used" should be its own special top-level group
  const recentlyUsed = [...connections.value]
    .filter(c => c.lastUsed)
    .sort((a, b) => (b.lastUsed || 0) - (a.lastUsed || 0))
    .slice(0, 5);

  if (recentlyUsed.length > 0 && !searchQuery.value.trim()) {
    groups.set(RECENTLY_USED_LABEL, recentlyUsed);
  }

  for (const connection of filteredConnections.value) {
    const group = toDisplayGroup(connection.group);
    const list = groups.get(group) ?? [];
    list.push(connection);
    groups.set(group, list);
  }
  return Array.from(groups.entries()).sort(([left], [right]) => {
    if (left === RECENTLY_USED_LABEL) {
      return -1;
    }
    if (right === RECENTLY_USED_LABEL) {
      return 1;
    }
    if (left === UNGROUPED_LABEL) {
      return 1;
    }
    if (right === UNGROUPED_LABEL) {
      return -1;
    }
    return left.localeCompare(right, "zh-CN");
  });
});

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
    alert(`Failed to save: ${error}`);
  }
}
</script>

<template>
  <div class="bookmark-sidebar">
    <div class="bookmark-sidebar-header">
      <span class="bookmark-sidebar-title">🔖 Quick Connect</span>
      <button class="bookmark-refresh-btn" title="Refresh list" @click="loadConnections">↻</button>
    </div>

    <div class="bookmark-search-container">
      <input
        v-model="searchQuery"
        type="text"
        class="bookmark-search-input"
        placeholder="Search bookmarks..."
        autocapitalize="none"
        autocorrect="off"
        spellcheck="false"
      >
      <button v-if="searchQuery" class="bookmark-search-clear" @click="searchQuery = ''">×</button>
    </div>

    <div v-if="connections.length === 0" class="bookmark-empty">
      No saved connections.
      <br>
      Check "Save this connection" when
      <br>
      creating a new session to add one.
    </div>

    <div v-else-if="filteredConnections.length === 0" class="bookmark-empty">
      No bookmarks match your search.
    </div>

    <div v-else class="bookmark-list">
        <div
          v-for="[group, items] in groupedConnections"
          :key="group"
          class="bookmark-group"
          :class="{ collapsed: isGroupCollapsed(group) }"
          :data-group="group"
        >
          <div class="bookmark-group-header" @click="toggleGroup(group)">
          <div class="bookmark-group-header-left">
            <span class="bookmark-group-arrow">›</span>
            <span class="bookmark-group-name">{{ group }}</span>
          </div>
          <span class="bookmark-group-count">{{ items.length }}</span>
        </div>
        <ul v-if="!isGroupCollapsed(group)" class="bookmark-group-list">
          <li
            v-for="connection in items"
            :key="connection.id"
            class="bookmark-item"
            :title="`${buildSubtitle(connection)}\nDouble-click to connect`"
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
    </div>

    <div
      v-if="contextMenu"
      ref="contextMenuRef"
      class="bookmark-context-menu"
      :style="{ top: `${contextMenu.y}px`, left: `${contextMenu.x}px` }"
    >
      <button class="bookmark-context-item" @click="openEditDialog(contextMenu.connection)">✏️ Edit</button>
      <button class="bookmark-context-item danger" @click="handleDelete(contextMenu.connection.id)">🗑 Delete</button>
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
  background: #252526;
  border-right: 1px solid #333;
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
  color: #ccc;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.bookmark-refresh-btn {
  background: none;
  border: none;
  color: #888;
  cursor: pointer;
  font-size: 1.1rem;
  padding: 4px;
  border-radius: 4px;
  line-height: 1;
}

.bookmark-refresh-btn:hover {
  background: #333;
  color: #fff;
}

.bookmark-search-container {
  padding: 6px 12px 12px 12px;
  position: relative;
  border-bottom: 1px solid #333;
}

.bookmark-search-input {
  width: 100%;
  box-sizing: border-box;
  background: #1e1e1e;
  border: 1px solid #3c3c3c;
  border-radius: 4px;
  padding: 4px 24px 4px 8px;
  color: #ccc;
  font-size: 0.8rem;
  outline: none;
}

.bookmark-search-input:focus {
  border-color: #0078d4;
}

.bookmark-search-clear {
  position: absolute;
  right: 18px;
  top: 4px;
  background: none;
  border: none;
  color: #888;
  cursor: pointer;
  font-size: 1rem;
  padding: 0;
  line-height: 1;
}

.bookmark-search-clear:hover {
  color: #fff;
}

.bookmark-empty {
  padding: 40px 20px;
  text-align: center;
  color: #666;
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
  background: #2d2d2d;
  margin-bottom: 4px;
  cursor: pointer;
}

.bookmark-group-header:hover {
  background: #333;
}

.bookmark-group-header-left {
  display: flex;
  align-items: center;
  gap: 4px;
}

.bookmark-group-arrow {
  font-size: 1.1rem;
  color: #666;
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
  color: #888;
}

.bookmark-group[data-group="Recently Used"] .bookmark-group-header {
  border-top: 1px solid #333;
}

.bookmark-group[data-group="Recently Used"] .bookmark-group-name {
  color: #0078d4;
  font-style: italic;
}

.bookmark-group-count {
  font-size: 0.7rem;
  color: #555;
  background: #383838;
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
  background: #2a2d2e;
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
  color: #ccc;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bookmark-host {
  font-size: 0.75rem;
  color: #666;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bookmark-context-menu {
  position: fixed;
  background: #252526;
  border: 1px solid #454545;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
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
  color: #ccc;
  font-size: 0.85rem;
  cursor: pointer;
  border-radius: 2px;
}

.bookmark-context-item:hover {
  background: #0078d4;
  color: #fff;
}

.bookmark-context-item.danger:hover {
  background: #e81123;
}
</style>
