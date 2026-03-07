<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SavedConnection } from "./types";

interface ContextMenu {
  x: number;
  y: number;
  connection: SavedConnection;
}

const props = defineProps<{
  refreshToken?: number;
}>();

const emit = defineEmits<{
  connect: [connection: SavedConnection];
}>();

const UNGROUPED_LABEL = "未分组";

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

function inputValue(event: Event) {
  return (event.target as HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement).value;
}

function toNumber(value: string, fallback: number) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function toDataBits(value: string): 5 | 6 | 7 | 8 {
  const parsed = Number(value);
  if (parsed === 5 || parsed === 6 || parsed === 7) {
    return parsed;
  }
  return 8;
}

function toStopBits(value: string): 1 | 2 {
  return Number(value) === 2 ? 2 : 1;
}

function toParity(value: string): "none" | "odd" | "even" {
  if (value === "odd" || value === "even") {
    return value;
  }
  return "none";
}

function toFlowControl(value: string): "none" | "hardware" | "software" {
  if (value === "hardware" || value === "software") {
    return value;
  }
  return "none";
}

function toAuthType(value: string): "password" | "key" | "none" {
  if (value === "key" || value === "none") {
    return value;
  }
  return "password";
}

const connections = ref<SavedConnection[]>([]);
const contextMenu = ref<ContextMenu | null>(null);
const editingConnection = ref<SavedConnection | null>(null);
const editDraft = ref<SavedConnection | null>(null);
const editError = ref("");
const contextMenuRef = ref<HTMLDivElement | null>(null);

async function loadConnections() {
  try {
    connections.value = await invoke<SavedConnection[]>("get_connections");
  } catch (error) {
    console.error("Failed to load connections", error);
  }
}

watch(() => props.refreshToken, () => {
  void loadConnections();
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

const groupedConnections = computed(() => {
  const groups = new Map<string, SavedConnection[]>();
  for (const connection of connections.value) {
    const group = toDisplayGroup(connection.group);
    const list = groups.get(group) ?? [];
    list.push(connection);
    groups.set(group, list);
  }
  return Array.from(groups.entries()).sort(([left], [right]) => {
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
  editDraft.value = { ...connection };
  editError.value = "";
  contextMenu.value = null;
}

function closeEditDialog() {
  editingConnection.value = null;
  editDraft.value = null;
  editError.value = "";
}

function updateDraft<K extends keyof SavedConnection>(key: K, value: SavedConnection[K]) {
  if (!editDraft.value) {
    return;
  }
  editDraft.value = { ...editDraft.value, [key]: value };
}

async function saveDraft() {
  if (!editDraft.value || !editingConnection.value) {
    return;
  }

  const protocol = editDraft.value.protocol ?? "ssh";
  if (!editDraft.value.name.trim()) {
    editError.value = "名称不能为空。";
    return;
  }
  if (protocol === "serial") {
    if (!editDraft.value.portName?.trim()) {
      editError.value = "串口设备不能为空。";
      return;
    }
  } else {
    if (!editDraft.value.host.trim()) {
      editError.value = "主机地址不能为空。";
      return;
    }
    if (protocol === "ssh" && !editDraft.value.user.trim()) {
      editError.value = "SSH 用户名不能为空。";
      return;
    }
  }

  const normalized: SavedConnection = {
    ...editDraft.value,
    name: editDraft.value.name.trim(),
    group: editDraft.value.group?.trim() || undefined,
    host: protocol === "serial" ? "" : editDraft.value.host.trim(),
    port: protocol === "serial" ? 0 : editDraft.value.port,
    user: protocol === "ssh" ? editDraft.value.user.trim() : "",
    authType: protocol === "ssh" ? editDraft.value.authType : "none",
    password: protocol === "ssh" && editDraft.value.authType === "password" ? editDraft.value.password : undefined,
    privateKey: protocol === "ssh" && editDraft.value.authType === "key" ? editDraft.value.privateKey : undefined,
    portName: protocol === "serial" ? editDraft.value.portName?.trim() : undefined,
    baudRate: protocol === "serial" ? editDraft.value.baudRate : undefined,
    dataBits: protocol === "serial" ? editDraft.value.dataBits : undefined,
    stopBits: protocol === "serial" ? editDraft.value.stopBits : undefined,
    parity: protocol === "serial" ? editDraft.value.parity : undefined,
    flowControl: protocol === "serial" ? editDraft.value.flowControl : undefined,
  };

  try {
    await invoke("save_connection", { connection: normalized });
    connections.value = connections.value.map((connection) => (
      connection.id === editingConnection.value?.id ? normalized : connection
    ));
    closeEditDialog();
  } catch (error) {
    console.error("Failed to save connection", error);
    editError.value = String(error);
  }
}
</script>

<template>
  <div class="bookmark-sidebar">
    <div class="bookmark-sidebar-header">
      <span class="bookmark-sidebar-title">🔖 快捷连接</span>
      <button class="bookmark-refresh-btn" title="刷新列表" @click="loadConnections">↻</button>
    </div>

    <div v-if="connections.length === 0" class="bookmark-empty">
      暂无保存的连接。
      <br>
      新建会话时勾选
      <br>
      “保存此连接”即可添加。
    </div>

    <div v-else class="bookmark-list">
      <div v-for="[group, items] in groupedConnections" :key="group" class="bookmark-group">
        <div class="bookmark-group-header">
          <span class="bookmark-group-name">{{ group }}</span>
          <span class="bookmark-group-count">{{ items.length }}</span>
        </div>
        <ul class="bookmark-group-list">
          <li
            v-for="connection in items"
            :key="connection.id"
            class="bookmark-item"
            :title="`${buildSubtitle(connection)}\n双击连接`"
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
      <button class="bookmark-context-item" @click="openEditDialog(contextMenu.connection)">✏️ 编辑</button>
      <button class="bookmark-context-item danger" @click="handleDelete(contextMenu.connection.id)">🗑 删除</button>
    </div>

    <div v-if="editingConnection && editDraft" class="bookmark-editor-overlay" @click="closeEditDialog">
      <div class="bookmark-editor-dialog" @click.stop>
        <div class="bookmark-editor-header">
          <div>
            <div class="bookmark-editor-title">编辑书签</div>
            <div class="bookmark-editor-subtitle">
              {{ editDraft.protocol === 'serial' ? '串口参数' : editDraft.protocol === 'telnet' ? 'Telnet 参数' : 'SSH 参数' }}
            </div>
          </div>
          <button type="button" class="bookmark-editor-close" @click="closeEditDialog">×</button>
        </div>

        <div class="bookmark-editor-body">
          <div class="bookmark-editor-grid">
            <div class="form-group">
              <label>名称</label>
              <input type="text" :value="editDraft.name" @input="updateDraft('name', inputValue($event))">
            </div>
            <div class="form-group">
              <label>分组</label>
              <input type="text" :value="editDraft.group ?? ''" placeholder="未分组" @input="updateDraft('group', inputValue($event))">
            </div>
          </div>

          <template v-if="editDraft.protocol === 'serial'">
            <div class="bookmark-editor-grid">
              <div class="form-group bookmark-editor-span-2">
                <label>串口设备</label>
                <input
                  type="text"
                  :value="editDraft.portName ?? ''"
                  placeholder="/dev/cu.usbserial-1410"
                  @input="updateDraft('portName', inputValue($event))"
                >
              </div>
              <div class="form-group">
                <label>Baud Rate</label>
                <input
                  type="number"
                  :value="editDraft.baudRate ?? 9600"
                  @input="updateDraft('baudRate', toNumber(inputValue($event), 9600))"
                >
              </div>
              <div class="form-group">
                <label>Data Bits</label>
                <select :value="String(editDraft.dataBits ?? 8)" @change="updateDraft('dataBits', toDataBits(inputValue($event)))">
                  <option value="5">5</option>
                  <option value="6">6</option>
                  <option value="7">7</option>
                  <option value="8">8</option>
                </select>
              </div>
              <div class="form-group">
                <label>Stop Bits</label>
                <select :value="String(editDraft.stopBits ?? 1)" @change="updateDraft('stopBits', toStopBits(inputValue($event)))">
                  <option value="1">1</option>
                  <option value="2">2</option>
                </select>
              </div>
              <div class="form-group">
                <label>Parity</label>
                <select :value="editDraft.parity ?? 'none'" @change="updateDraft('parity', toParity(inputValue($event)))">
                  <option value="none">None</option>
                  <option value="odd">Odd</option>
                  <option value="even">Even</option>
                </select>
              </div>
              <div class="form-group bookmark-editor-span-2">
                <label>Flow Control</label>
                <select :value="editDraft.flowControl ?? 'none'" @change="updateDraft('flowControl', toFlowControl(inputValue($event)))">
                  <option value="none">None</option>
                  <option value="hardware">Hardware</option>
                  <option value="software">Software</option>
                </select>
              </div>
            </div>
          </template>

          <template v-else>
            <div class="bookmark-editor-grid">
              <div class="form-group bookmark-editor-span-2">
                <label>Host</label>
                <input type="text" :value="editDraft.host" @input="updateDraft('host', inputValue($event))">
              </div>
              <div class="form-group">
                <label>Port</label>
                <input type="number" :value="editDraft.port" @input="updateDraft('port', toNumber(inputValue($event), 0))">
              </div>
              <div v-if="(editDraft.protocol ?? 'ssh') === 'ssh'" class="form-group">
                <label>User</label>
                <input type="text" :value="editDraft.user" @input="updateDraft('user', inputValue($event))">
              </div>
              <div v-else class="form-group">
                <label>协议</label>
                <input type="text" value="Telnet" disabled>
              </div>
            </div>

            <template v-if="(editDraft.protocol ?? 'ssh') === 'ssh'">
              <div class="bookmark-editor-grid">
                <div class="form-group">
                  <label>认证方式</label>
                  <select :value="editDraft.authType" @change="updateDraft('authType', toAuthType(inputValue($event)))">
                    <option value="password">Password</option>
                    <option value="key">Private Key</option>
                  </select>
                </div>
              </div>

              <div v-if="editDraft.authType === 'password'" class="form-group">
                <label>Password</label>
                <input type="password" :value="editDraft.password ?? ''" @input="updateDraft('password', inputValue($event))">
              </div>

              <div v-else class="form-group">
                <label>Private Key (PEM)</label>
                <textarea rows="5" :value="editDraft.privateKey ?? ''" @input="updateDraft('privateKey', inputValue($event))" />
              </div>
            </template>
          </template>

          <div v-if="editError" class="bookmark-editor-error">{{ editError }}</div>
        </div>

        <div class="bookmark-editor-footer">
          <button type="button" class="bookmark-editor-btn secondary" @click="closeEditDialog">取消</button>
          <button type="button" class="bookmark-editor-btn primary" @click="saveDraft">保存</button>
        </div>
      </div>
    </div>
  </div>
</template>