<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { buildConnectionLogContext, buildDefaultLogPath, normalizeOptionalLogPath } from "./logging";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import { t } from "./i18n";
import { isReconnectEnabled, normalizeReconnectType, type ReconnectType, type SavedConnection, type SshAuthType, type TunnelConfig } from "./types";

const props = withDefaults(defineProps<{
  connection: SavedConnection;
  settings?: AppSettings;
  /** Group paths already in use, offered in the group dropdown. */
  groups?: string[];
  /** Render inline (bookmark manager detail column) instead of as a modal:
   *  no backdrop, no header, and clicking outside does not cancel. */
  embedded?: boolean;
}>(), {
  settings: () => DEFAULT_SETTINGS,
  groups: () => [],
  embedded: false,
});

const emit = defineEmits<{
  save: [connection: SavedConnection];
  cancel: [];
}>();

const editDraft = ref<SavedConnection>({
  ...props.connection,
  jumpHosts: props.connection.jumpHosts?.map((jump) => ({ ...jump })) ?? [],
  autoLoginRules: props.connection.autoLoginRules?.map((rule) => ({ ...rule })) ?? [],
  postConnectCommands: [...(props.connection.postConnectCommands ?? [])],
});
const editError = ref("");
const generatedPublicKey = ref("");

interface GeneratedSshKeyPair {
  privateKey: string;
  publicKey: string;
  fingerprint: string;
}

// Initialize reconnect type and autoReconnect from connection
const initialReconnectType = normalizeReconnectType(props.connection);
editDraft.value.reconnectType = initialReconnectType;
editDraft.value.autoReconnect = isReconnectEnabled(initialReconnectType);

/** Sentinel `<select>` value that reveals the free-text field for a brand-new group. */
const NEW_GROUP_OPTION = "__auraterm_new_group__";

const currentGroup = (props.connection.group ?? "").trim();
const groupSelection = ref(currentGroup && !props.groups.includes(currentGroup) ? NEW_GROUP_OPTION : currentGroup);
const customGroup = ref(currentGroup);
const customGroupInput = ref<HTMLInputElement | null>(null);

/** True when the group name comes from the text field instead of the dropdown. */
const useCustomGroup = computed(() => props.groups.length === 0 || groupSelection.value === NEW_GROUP_OPTION);

function applyGroup() {
  updateDraft("group", (useCustomGroup.value ? customGroup.value : groupSelection.value).trim() || undefined);
}

function handleGroupSelectionChange(event: Event) {
  groupSelection.value = inputValue(event);
  applyGroup();
  if (groupSelection.value === NEW_GROUP_OPTION) {
    void nextTick(() => customGroupInput.value?.focus());
  }
}

function handleCustomGroupInput(event: Event) {
  customGroup.value = inputValue(event);
  applyGroup();
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

function toAuthType(value: string): SshAuthType {
  if (value === "key" || value === "agent" || value === "none") {
    return value;
  }
  return "password";
}

function addJumpHost() {
  editDraft.value.jumpHosts = [
    ...(editDraft.value.jumpHosts ?? []),
    { id: crypto.randomUUID(), host: "", port: 22, user: "", authType: "agent" },
  ];
}

function removeJumpHost(index: number) {
  editDraft.value.jumpHosts = (editDraft.value.jumpHosts ?? []).filter((_, itemIndex) => itemIndex !== index);
}

function addAutoLoginRule() {
  editDraft.value.autoLoginRules = [
    ...(editDraft.value.autoLoginRules ?? []),
    { expect: "", response: "", timeoutSecs: 30 },
  ];
}

function removeAutoLoginRule(index: number) {
  editDraft.value.autoLoginRules = (editDraft.value.autoLoginRules ?? []).filter((_, itemIndex) => itemIndex !== index);
}

function updatePostConnectCommands(value: string) {
  editDraft.value.postConnectCommands = value.split(/\r?\n/);
}

async function generatePrivateKey() {
  editError.value = "";
  try {
    const generated = await invoke<GeneratedSshKeyPair>("ssh_generate_key_pair", {
      passphrase: editDraft.value.passphrase || null,
      comment: `${editDraft.value.user}@${editDraft.value.host}`,
    });
    updateDraft("privateKey", generated.privateKey);
    generatedPublicKey.value = generated.publicKey;
  } catch (error) {
    editError.value = String(error);
  }
}

async function copyGeneratedPublicKey() {
  await navigator.clipboard.writeText(generatedPublicKey.value);
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

function handleAgentForwardingChange(event: Event) {
  updateDraft("agentForwarding", (event.target as HTMLInputElement).checked);
}

const editDraftDefaultLogPath = computed(() => {
  return buildDefaultLogPath(props.settings, buildConnectionLogContext(editDraft.value));
});

/** Tunnels are configured from the tunnel manager during a session; the editor
 *  shows what a bookmark carries so it is not invisible outside one. */
const savedTunnels = computed(() => editDraft.value.tunnels ?? []);

function tunnelFlag(tunnel: TunnelConfig) {
  return tunnel.type === "remote" ? "-R" : tunnel.type === "dynamic" ? "-D" : "-L";
}

function describeTunnel(tunnel: TunnelConfig) {
  const listen = `${tunnel.bindAddress || "127.0.0.1"}:${tunnel.bindPort}`;
  if (tunnel.type === "dynamic") {
    return `${listen} · SOCKS`;
  }
  return `${listen} → ${tunnel.destHost ?? ""}:${tunnel.destPort ?? ""}`;
}

function handleBackdropClick() {
  if (!props.embedded) {
    emit("cancel");
  }
}

function handleSave() {
  const protocol = editDraft.value.protocol ?? "ssh";
  const reconnectType = protocol === "ssh" ? normalizeReconnectType(editDraft.value) : undefined;

  if (!editDraft.value.name.trim()) {
    editError.value = t("bookmarkEditor.errNameEmpty");
    return;
  }
  if (protocol === "serial") {
    if (!editDraft.value.portName?.trim()) {
      editError.value = t("bookmarkEditor.errSerialEmpty");
      return;
    }
  } else {
    if (!editDraft.value.host.trim()) {
      editError.value = t("bookmarkEditor.errHostEmpty");
      return;
    }
    if (protocol === "ssh" && !editDraft.value.user.trim()) {
      editError.value = t("bookmarkEditor.errUserEmpty");
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
    passphrase: protocol === "ssh" && editDraft.value.authType === "key" ? editDraft.value.passphrase : undefined,
    agentForwarding: protocol === "ssh" ? editDraft.value.agentForwarding : undefined,
    jumpHosts: protocol === "ssh" ? editDraft.value.jumpHosts : undefined,
    autoLoginRules: protocol === "ssh"
      ? editDraft.value.autoLoginRules?.filter((rule) => rule.expect.trim())
      : undefined,
    postConnectCommands: protocol === "ssh"
      ? editDraft.value.postConnectCommands?.map((command) => command.trim()).filter(Boolean)
      : undefined,
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
  <div :class="props.embedded ? 'bookmark-editor-embedded' : 'bookmark-editor-overlay'" @click="handleBackdropClick">
    <div :class="props.embedded ? 'bookmark-editor-inline' : 'bookmark-editor-dialog'" @click.stop>
      <div v-if="!props.embedded" class="bookmark-editor-header">
        <div>
          <div class="bookmark-editor-title">{{ $t('bookmarkEditor.title') }}</div>
          <div class="bookmark-editor-subtitle">
            {{ editDraft.protocol === 'serial' ? $t('bookmarkEditor.serialSettings') : editDraft.protocol === 'telnet' ? $t('bookmarkEditor.telnetSettings') : $t('bookmarkEditor.sshSettings') }}
          </div>
        </div>
        <button type="button" class="bookmark-editor-close" @click="emit('cancel')">×</button>
      </div>

      <div class="bookmark-editor-body">
        <div class="bookmark-editor-grid">
          <div class="form-group">
            <label>{{ $t('bookmarkEditor.name') }}</label>
            <input type="text" :value="editDraft.name" @input="updateDraft('name', inputValue($event))">
          </div>
          <div class="form-group">
            <label>{{ $t('bookmarkEditor.group') }}</label>
            <div class="bookmark-group-field">
              <select
                v-if="props.groups.length > 0"
                :value="groupSelection"
                @change="handleGroupSelectionChange"
              >
                <option value="">{{ $t('bookmarkEditor.ungrouped') }}</option>
                <optgroup :label="$t('bookmarkEditor.groupExisting')">
                  <option v-for="group in props.groups" :key="group" :value="group">{{ group }}</option>
                </optgroup>
                <option :value="NEW_GROUP_OPTION">{{ $t('bookmarkEditor.groupNew') }}</option>
              </select>
              <input
                v-if="useCustomGroup"
                ref="customGroupInput"
                type="text"
                :value="customGroup"
                :placeholder="$t('bookmarkEditor.ungrouped')"
                @input="handleCustomGroupInput"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              >
            </div>
          </div>
        </div>

        <template v-if="editDraft.protocol === 'serial'">
          <div class="bookmark-editor-grid">
            <div class="form-group bookmark-editor-span-2">
              <label>{{ $t('bookmarkEditor.serialPort') }}</label>
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
              <label>{{ $t('bookmarkEditor.baudRate') }}</label>
              <input
                type="number"
                :value="editDraft.baudRate ?? 9600"
                @input="updateDraft('baudRate', toNumber(inputValue($event), 9600))"
              >
            </div>
            <div class="form-group">
              <label>{{ $t('bookmarkEditor.dataBits') }}</label>
              <select :value="String(editDraft.dataBits ?? 8)" @change="updateDraft('dataBits', toDataBits(inputValue($event)))">
                <option value="5">5</option>
                <option value="6">6</option>
                <option value="7">7</option>
                <option value="8">8</option>
              </select>
            </div>
            <div class="form-group">
              <label>{{ $t('bookmarkEditor.stopBits') }}</label>
              <select :value="String(editDraft.stopBits ?? 1)" @change="updateDraft('stopBits', toStopBits(inputValue($event)))">
                <option value="1">1</option>
                <option value="2">2</option>
              </select>
            </div>
            <div class="form-group">
              <label>{{ $t('bookmarkEditor.parity') }}</label>
              <select :value="editDraft.parity ?? 'none'" @change="updateDraft('parity', toParity(inputValue($event)))">
                <option value="none">{{ $t('connect.none') }}</option>
                <option value="odd">{{ $t('connect.odd') }}</option>
                <option value="even">{{ $t('connect.even') }}</option>
              </select>
            </div>
            <div class="form-group bookmark-editor-span-2">
              <label>{{ $t('bookmarkEditor.flowControl') }}</label>
              <select :value="editDraft.flowControl ?? 'none'" @change="updateDraft('flowControl', toFlowControl(inputValue($event)))">
                <option value="none">{{ $t('connect.none') }}</option>
                <option value="hardware">{{ $t('connect.hardware') }}</option>
                <option value="software">{{ $t('connect.software') }}</option>
              </select>
            </div>
          </div>
        </template>

        <template v-else>
          <div class="bookmark-editor-grid">
            <div class="form-group bookmark-editor-span-2">
              <label>{{ $t('bookmarkEditor.host') }}</label>
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
              <label>{{ $t('bookmarkEditor.port') }}</label>
              <input type="number" :value="editDraft.port" @input="updateDraft('port', toNumber(inputValue($event), 0))">
            </div>
            <div v-if="(editDraft.protocol ?? 'ssh') === 'ssh'" class="form-group">
              <label>{{ $t('bookmarkEditor.user') }}</label>
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
              <label>{{ $t('bookmarkEditor.protocol') }}</label>
              <input type="text" value="Telnet" disabled>
            </div>
          </div>

          <template v-if="(editDraft.protocol ?? 'ssh') === 'ssh'">
            <div class="bookmark-editor-grid">
              <div class="form-group">
                <label>{{ $t('bookmarkEditor.authMethod') }}</label>
                <select :value="editDraft.authType" @change="updateDraft('authType', toAuthType(inputValue($event)))">
                  <option value="password">{{ $t('connect.authPassword') }}</option>
                  <option value="key">{{ $t('connect.authKey') }}</option>
                  <option value="agent">{{ $t('connect.authAgent') }}</option>
                  <option value="none">{{ $t('connect.authKeyboard') }}</option>
                </select>
              </div>
            </div>

            <div v-if="editDraft.authType === 'password'" class="form-group">
              <label>{{ $t('bookmarkEditor.password') }}</label>
              <input
                type="password"
                :value="editDraft.password ?? ''"
                @input="updateDraft('password', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              >
            </div>

            <div v-else-if="editDraft.authType === 'key'" class="form-group">
              <label>{{ $t('bookmarkEditor.privateKeyPem') }}</label>
              <textarea
                rows="5"
                :value="editDraft.privateKey ?? ''"
                @input="updateDraft('privateKey', inputValue($event))"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              />
              <button type="button" class="bookmark-editor-btn secondary" @click="generatePrivateKey">{{ $t('bookmarkEditor.generateKey') }}</button>
              <label>{{ $t('bookmarkEditor.passphrase') }}</label>
              <input
                type="password"
                :value="editDraft.passphrase ?? ''"
                @input="updateDraft('passphrase', inputValue($event))"
              >
              <div v-if="generatedPublicKey" class="generated-key-row">
                <textarea :value="generatedPublicKey" rows="2" readonly />
                <button type="button" class="bookmark-editor-btn secondary" @click="copyGeneratedPublicKey">{{ $t('connect.copyPublicKey') }}</button>
              </div>
            </div>

            <details class="bookmark-advanced">
              <summary>{{ $t('connect.advancedSummary') }}</summary>
              <label class="bookmark-inline-check">
                <input
                  type="checkbox"
                  :checked="editDraft.agentForwarding ?? false"
                  @change="handleAgentForwardingChange"
                >
                {{ $t('connect.forwardAgent') }}
              </label>

              <div class="bookmark-advanced-heading">
                <strong>{{ $t('bookmarkEditor.jumpHosts') }}</strong>
                <button type="button" @click="addJumpHost">{{ $t('bookmarkEditor.add') }}</button>
              </div>
              <div v-for="(jump, index) in editDraft.jumpHosts ?? []" :key="jump.id" class="bookmark-advanced-card">
                <div class="bookmark-advanced-grid">
                  <input v-model="jump.host" type="text" :placeholder="$t('bookmarkEditor.host')">
                  <input v-model.number="jump.port" type="number" min="1" max="65535" :placeholder="$t('bookmarkEditor.port')">
                  <input v-model="jump.user" type="text" :placeholder="$t('bookmarkEditor.user')">
                  <select v-model="jump.authType">
                    <option value="password">{{ $t('connect.authPassword') }}</option>
                    <option value="key">{{ $t('connect.authKey') }}</option>
                    <option value="agent">{{ $t('bookmarkEditor.agent') }}</option>
                    <option value="none">{{ $t('connect.authKeyboard') }}</option>
                  </select>
                </div>
                <input v-if="jump.authType === 'password'" v-model="jump.password" type="password" :placeholder="$t('bookmarkEditor.password')">
                <template v-else-if="jump.authType === 'key'">
                  <textarea v-model="jump.privateKey" rows="3" :placeholder="$t('bookmarkEditor.privateKeyJumpPlaceholder')" />
                  <input v-model="jump.passphrase" type="password" :placeholder="$t('bookmarkEditor.passphrase')">
                </template>
                <button type="button" class="bookmark-remove" @click="removeJumpHost(index)">{{ $t('connect.remove') }}</button>
              </div>

              <div class="bookmark-advanced-heading">
                <strong>{{ $t('bookmarkEditor.expectRules') }}</strong>
                <button type="button" @click="addAutoLoginRule">{{ $t('bookmarkEditor.add') }}</button>
              </div>
              <div v-for="(rule, index) in editDraft.autoLoginRules ?? []" :key="index" class="bookmark-advanced-card">
                <div class="bookmark-automation-grid">
                  <input v-model="rule.expect" type="text" :placeholder="$t('bookmarkEditor.waitForText')">
                  <input v-model="rule.response" type="password" :placeholder="$t('connect.sendResponse')">
                  <input v-model.number="rule.timeoutSecs" type="number" min="1" max="300" :title="$t('connect.timeoutSeconds')">
                </div>
                <label class="bookmark-inline-check"><input v-model="rule.caseSensitive" type="checkbox"> {{ $t('connect.caseSensitive') }}</label>
                <button type="button" class="bookmark-remove" @click="removeAutoLoginRule(index)">{{ $t('connect.remove') }}</button>
              </div>

              <div class="form-group">
                <label>{{ $t('bookmarkEditor.commandsAfterLogin') }}</label>
                <textarea
                  rows="3"
                  :value="(editDraft.postConnectCommands ?? []).join('\n')"
                  @input="updatePostConnectCommands(inputValue($event))"
                />
              </div>
            </details>

            <div class="bookmark-editor-grid">
              <div class="form-group bookmark-editor-span-2">
                <label>{{ $t('bookmarkEditor.reconnectMode') }}</label>
                <select
                  :value="normalizeReconnectType(editDraft)"
                  @change="updateDraft('reconnectType', toReconnectType(inputValue($event)))"
                >
                  <option value="manual">{{ $t('bookmarkEditor.reconnectManual') }}</option>
                  <option value="simple">{{ $t('bookmarkEditor.reconnectSimple') }}</option>
                  <option value="tmux">tmux</option>
                  <option value="screen">screen</option>
                </select>
              </div>
            </div>

            <div v-if="savedTunnels.length" class="form-group bookmark-tunnels">
              <label>{{ $t('bookmarkEditor.tunnels') }}</label>
              <div v-for="tunnel in savedTunnels" :key="tunnel.id" class="bookmark-tunnel-row">
                <span class="bookmark-tunnel-type">{{ tunnelFlag(tunnel) }}</span>
                <span class="bookmark-tunnel-route">{{ describeTunnel(tunnel) }}</span>
                <span v-if="tunnel.autoStart" class="bookmark-tunnel-auto">{{ $t('bookmarkEditor.tunnelAutoStart') }}</span>
              </div>
              <div class="form-hint">{{ $t('bookmarkEditor.tunnelsHint') }}</div>
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
            {{ $t('bookmarkEditor.saveLog') }}
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
            {{ $t('bookmarkEditor.logHint') }}
          </div>
        </div>

        <div v-if="editError" class="bookmark-editor-error">{{ editError }}</div>
      </div>

      <div class="bookmark-editor-footer">
        <button type="button" class="bookmark-editor-btn secondary" @click="emit('cancel')">{{ $t('common.cancel') }}</button>
        <button type="button" class="bookmark-editor-btn primary" @click="handleSave">{{ $t('common.save') }}</button>
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

.bookmark-tunnel-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 3px 0;
  font-family: monospace;
  font-size: 0.78rem;
  color: var(--app-text-secondary);
}

.bookmark-tunnel-type {
  color: var(--app-accent);
}

.bookmark-tunnel-route {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bookmark-tunnel-auto {
  color: var(--app-success);
  font-size: 0.72rem;
}

.bookmark-editor-embedded {
  height: 100%;
  min-height: 0;
  display: flex;
}

.bookmark-editor-inline {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.bookmark-editor-inline .bookmark-editor-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 12px 14px;
}

/* One column: the detail rail is far narrower than the modal. */
.bookmark-editor-inline .bookmark-editor-grid {
  grid-template-columns: 1fr;
}

.bookmark-editor-inline .bookmark-editor-footer {
  padding: 10px 14px;
}

.bookmark-group-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
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

.bookmark-advanced { border: 1px solid var(--app-border); border-radius: 6px; padding: 10px 12px; margin-bottom: 16px; }
.bookmark-advanced summary { cursor: pointer; color: var(--app-text-secondary); }
.bookmark-advanced[open] summary { margin-bottom: 12px; }
.bookmark-inline-check { display: flex; align-items: center; gap: 7px; margin: 9px 0; color: var(--app-text-secondary); font-size: 0.82rem; }
.bookmark-advanced-heading { display: flex; justify-content: space-between; align-items: center; margin: 14px 0 8px; }
.bookmark-advanced-heading button, .bookmark-remove { border: 1px solid var(--app-border); border-radius: 4px; background: var(--app-input-bg); color: var(--app-text-secondary); padding: 5px 8px; cursor: pointer; }
.bookmark-advanced-card { border: 1px solid var(--app-border); border-radius: 5px; padding: 9px; margin-bottom: 8px; background: var(--app-surface-0); }
.bookmark-advanced-grid { display: grid; grid-template-columns: 2fr 70px 1.2fr 1.2fr; gap: 7px; margin-bottom: 7px; }
.bookmark-automation-grid { display: grid; grid-template-columns: 1fr 1fr 65px; gap: 7px; }
.bookmark-advanced-card input, .bookmark-advanced-card select, .bookmark-advanced-card textarea { width: 100%; box-sizing: border-box; background: var(--app-input-bg); border: 1px solid var(--app-border); border-radius: 4px; color: var(--app-text); padding: 7px; }
.bookmark-advanced-card textarea { resize: vertical; margin-bottom: 6px; }
.bookmark-remove { margin-top: 7px; color: var(--app-danger); }
.generated-key-row { display: grid; grid-template-columns: 1fr auto; gap: 8px; align-items: stretch; }

@media (max-width: 560px) {
  .bookmark-advanced-grid, .bookmark-automation-grid { grid-template-columns: 1fr; }
}
</style>
