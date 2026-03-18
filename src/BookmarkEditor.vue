<script setup lang="ts">
import { computed, ref } from "vue";
import { buildConnectionLogContext, buildDefaultLogPath, normalizeOptionalLogPath } from "./logging";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import { isReconnectEnabled, normalizeReconnectType, type ReconnectType, type SavedConnection } from "./types";

const props = withDefaults(defineProps<{
  connection: SavedConnection;
  settings?: AppSettings;
}>(), {
  settings: () => DEFAULT_SETTINGS,
});

const emit = defineEmits<{
  save: [connection: SavedConnection];
  cancel: [];
}>();

const editDraft = ref<SavedConnection>({ ...props.connection });
const editError = ref("");

// Initialize reconnect type and autoReconnect from connection
const initialReconnectType = normalizeReconnectType(props.connection);
editDraft.value.reconnectType = initialReconnectType;
editDraft.value.autoReconnect = isReconnectEnabled(initialReconnectType);

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

function toReconnectType(value: string): ReconnectType {
  if (value === "simple" || value === "screen" || value === "tmux") {
    return value;
  }
  return "manual";
}

function updateDraft<K extends keyof SavedConnection>(key: K, value: SavedConnection[K]) {
  editDraft.value = { ...editDraft.value, [key]: value };
}

function updateLogEnabled(enabled: boolean) {
  editDraft.value = {
    ...editDraft.value,
    logPath: enabled ? (editDraft.value.logPath ?? "") : undefined,
  };
}

function handleLogEnabledChange(event: Event) {
  updateLogEnabled((event.target as HTMLInputElement).checked);
}

const editDraftDefaultLogPath = computed(() => {
  return buildDefaultLogPath(props.settings, buildConnectionLogContext(editDraft.value));
});

function handleSave() {
  const protocol = editDraft.value.protocol ?? "ssh";
  const reconnectType = protocol === "ssh" ? normalizeReconnectType(editDraft.value) : undefined;

  if (!editDraft.value.name.trim()) {
    editError.value = "Name cannot be empty.";
    return;
  }
  if (protocol === "serial") {
    if (!editDraft.value.portName?.trim()) {
      editError.value = "Serial port cannot be empty.";
      return;
    }
  } else {
    if (!editDraft.value.host.trim()) {
      editError.value = "Host cannot be empty.";
      return;
    }
    if (protocol === "ssh" && !editDraft.value.user.trim()) {
      editError.value = "SSH username cannot be empty.";
      return;
    }
  }

  const normalized: SavedConnection = {
    ...editDraft.value,
    name: editDraft.value.name.trim(),
    group: editDraft.value.group?.trim() || undefined,
    logPath: normalizeOptionalLogPath(editDraft.value.logPath, editDraftDefaultLogPath.value),
    host: protocol === "serial" ? "" : editDraft.value.host.trim(),
    port: protocol === "serial" ? 0 : editDraft.value.port,
    user: protocol === "ssh" ? editDraft.value.user.trim() : "",
    authType: protocol === "ssh" ? editDraft.value.authType : "none",
    password: protocol === "ssh" ? editDraft.value.password : undefined,
    privateKey: protocol === "ssh" && editDraft.value.authType === "key" ? editDraft.value.privateKey : undefined,
    autoReconnect: protocol === "ssh" && reconnectType ? isReconnectEnabled(reconnectType) : undefined,
    reconnectType,
    portName: protocol === "serial" ? editDraft.value.portName?.trim() : undefined,
    baudRate: protocol === "serial" ? editDraft.value.baudRate : undefined,
    dataBits: protocol === "serial" ? editDraft.value.dataBits : undefined,
    stopBits: protocol === "serial" ? editDraft.value.stopBits : undefined,
    parity: protocol === "serial" ? editDraft.value.parity : undefined,
    flowControl: protocol === "serial" ? editDraft.value.flowControl : undefined,
  };

  emit("save", normalized);
}
</script>

<template>
  <div class="bookmark-editor-overlay" @click="emit('cancel')">
    <div class="bookmark-editor-dialog" @click.stop>
      <div class="bookmark-editor-header">
        <div>
          <div class="bookmark-editor-title">Edit Bookmark</div>
          <div class="bookmark-editor-subtitle">
            {{ editDraft.protocol === 'serial' ? 'Serial Settings' : editDraft.protocol === 'telnet' ? 'Telnet Settings' : 'SSH Settings' }}
          </div>
        </div>
        <button type="button" class="bookmark-editor-close" @click="emit('cancel')">×</button>
      </div>

      <div class="bookmark-editor-body">
        <div class="bookmark-editor-grid">
          <div class="form-group">
            <label>Name</label>
            <input type="text" :value="editDraft.name" @input="updateDraft('name', inputValue($event))">
          </div>
          <div class="form-group">
            <label>Group</label>
            <input
              type="text"
              :value="editDraft.group ?? ''"
              placeholder="Ungrouped"
              @input="updateDraft('group', inputValue($event))"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
          </div>
        </div>

        <template v-if="editDraft.protocol === 'serial'">
          <div class="bookmark-editor-grid">
            <div class="form-group bookmark-editor-span-2">
              <label>Serial Port</label>
              <input
                type="text"
                :value="editDraft.portName ?? ''"
                placeholder="/dev/cu.usbserial-1410"
                @input="updateDraft('portName', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
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
              <input
                type="text"
                :value="editDraft.host"
                @input="updateDraft('host', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              >
            </div>
            <div class="form-group">
              <label>Port</label>
              <input type="number" :value="editDraft.port" @input="updateDraft('port', toNumber(inputValue($event), 0))">
            </div>
            <div v-if="(editDraft.protocol ?? 'ssh') === 'ssh'" class="form-group">
              <label>User</label>
              <input
                type="text"
                :value="editDraft.user"
                @input="updateDraft('user', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              >
            </div>
            <div v-else class="form-group">
              <label>Protocol</label>
              <input type="text" value="Telnet" disabled>
            </div>
          </div>

          <template v-if="(editDraft.protocol ?? 'ssh') === 'ssh'">
            <div class="bookmark-editor-grid">
              <div class="form-group">
                <label>Auth Method</label>
                <select :value="editDraft.authType" @change="updateDraft('authType', toAuthType(inputValue($event)))">
                  <option value="password">Password</option>
                  <option value="key">Private Key</option>
                </select>
              </div>
            </div>

            <div v-if="editDraft.authType === 'password'" class="form-group">
              <label>Password</label>
              <input
                type="password"
                :value="editDraft.password ?? ''"
                @input="updateDraft('password', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              >
            </div>

            <div v-else class="form-group">
              <label>Private Key (PEM)</label>
              <textarea
                rows="5"
                :value="editDraft.privateKey ?? ''"
                @input="updateDraft('privateKey', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              />
            </div>

            <div class="bookmark-editor-grid">
              <div class="form-group bookmark-editor-span-2">
                <label>Reconnect Mode</label>
                <select
                  :value="normalizeReconnectType(editDraft)"
                  @change="updateDraft('reconnectType', toReconnectType(inputValue($event)))"
                >
                  <option value="manual">Manual</option>
                  <option value="simple">Simple</option>
                  <option value="tmux">tmux</option>
                  <option value="screen">screen</option>
                </select>
              </div>
            </div>
          </template>
        </template>

        <div class="form-group">
          <label>
            <input
              type="checkbox"
              :checked="editDraft.logPath !== undefined"
              @change="handleLogEnabledChange"
            >
            Save Session Log
          </label>
          <input
            v-if="editDraft.logPath !== undefined"
            type="text"
            :value="editDraft.logPath"
            :placeholder="editDraftDefaultLogPath"
            @input="updateDraft('logPath', inputValue($event))"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
          >
          <div v-if="editDraft.logPath !== undefined" class="form-hint">
            Leave blank to use the default log path template.
          </div>
        </div>

        <div v-if="editError" class="bookmark-editor-error">{{ editError }}</div>
      </div>

      <div class="bookmark-editor-footer">
        <button type="button" class="bookmark-editor-btn secondary" @click="emit('cancel')">Cancel</button>
        <button type="button" class="bookmark-editor-btn primary" @click="handleSave">Save</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.bookmark-editor-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: var(--app-overlay);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
  color: var(--app-text-secondary);
}

.bookmark-editor-dialog {
  background: var(--app-dialog-bg);
  border: 1px solid var(--app-border);
  border-radius: 8px;
  width: 480px;
  max-width: 90vw;
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 8px 32px var(--app-shadow);
}

.bookmark-editor-header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--app-border);
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
}

.bookmark-editor-title {
  font-size: 1.1rem;
  font-weight: 600;
  color: var(--app-text);
}

.bookmark-editor-subtitle {
  font-size: 0.8rem;
  color: var(--app-text-muted);
  margin-top: 2px;
}

.bookmark-editor-close {
  background: none;
  border: none;
  color: var(--app-text-muted);
  font-size: 1.5rem;
  cursor: pointer;
  padding: 0;
  line-height: 1;
}

.bookmark-editor-close:hover {
  color: var(--app-text);
}

.bookmark-editor-body {
  padding: 20px;
  overflow-y: auto;
}

.bookmark-editor-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
  margin-bottom: 16px;
}

.bookmark-editor-span-2 {
  grid-column: span 2;
}

.form-group {
  margin-bottom: 16px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.form-group label {
  font-size: 0.85rem;
  color: var(--app-text-muted);
}

.form-group input[type="text"],
.form-group input[type="number"],
.form-group input[type="password"],
.form-group select,
.form-group textarea {
  background: var(--app-input-bg);
  border: 1px solid var(--app-border);
  border-radius: 4px;
  padding: 8px 10px;
  color: var(--app-text);
  font-size: 0.9rem;
  outline: none;
}

.form-group input:focus,
.form-group select:focus,
.form-group textarea:focus {
  border-color: var(--app-border-accent);
}

.form-group input:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.form-group textarea {
  resize: vertical;
  font-family: monospace;
}

.form-group label:has(input[type="checkbox"]) {
  flex-direction: row;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  color: var(--app-text-secondary);
}

.form-hint {
  font-size: 0.75rem;
  color: var(--app-text-dim);
  margin-top: -4px;
}

.bookmark-editor-error {
  color: var(--app-danger);
  background: var(--app-danger-soft);
  padding: 10px;
  border-radius: 4px;
  font-size: 0.85rem;
  margin-top: 16px;
}

.bookmark-editor-footer {
  padding: 16px 20px;
  border-top: 1px solid var(--app-border);
  display: flex;
  justify-content: flex-end;
  gap: 12px;
}

.bookmark-editor-btn {
  padding: 8px 20px;
  border-radius: 4px;
  font-size: 0.9rem;
  cursor: pointer;
  border: none;
  transition: background 0.2s;
}

.bookmark-editor-btn.primary {
  background: var(--app-accent);
  color: var(--app-accent-contrast);
}

.bookmark-editor-btn.primary:hover {
  background: var(--app-accent-hover);
}

.bookmark-editor-btn.secondary {
  background: var(--app-surface-3);
  color: var(--app-text-secondary);
}

.bookmark-editor-btn.secondary:hover {
  background: var(--app-surface-4);
}
</style>
