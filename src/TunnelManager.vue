<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { t } from "./i18n";
import type { SshConfig, TunnelConfig, TunnelType } from "./types";
import type { SshTunnelsApi } from "./composables/useSshTunnels";

const props = defineProps<{
  sessionId: string;
  sshConfig: SshConfig;
  tunnels: TunnelConfig[];
  api: SshTunnelsApi;
}>();

const emit = defineEmits<{
  close: [];
  updateTunnels: [tunnels: TunnelConfig[]];
}>();

type DraftTunnel = {
  id: string;
  type: TunnelType;
  name: string;
  bindAddress: string;
  bindPort: string;
  destHost: string;
  destPort: string;
  autoStart: boolean;
};

const draft = ref<DraftTunnel | null>(null);
const formError = ref("");
const busyTunnelId = ref<string | null>(null);

const list = computed(() => props.tunnels ?? []);

onMounted(() => {
  void props.api.refreshTunnels(props.sessionId);
});

function typeLabel(type: TunnelType): string {
  switch (type) {
    case "local":
      return t("tunnels.typeLocal");
    case "remote":
      return t("tunnels.typeRemote");
    case "dynamic":
      return t("tunnels.typeDynamic");
  }
}

function describeTunnel(tunnel: TunnelConfig): string {
  const bind = `${tunnel.bindAddress?.trim() || "127.0.0.1"}:${tunnel.bindPort}`;
  if (tunnel.type === "dynamic") {
    return `${bind}  ·  ${t("tunnels.socks5Proxy")}`;
  }
  const dest = `${tunnel.destHost ?? "?"}:${tunnel.destPort ?? "?"}`;
  return tunnel.type === "local" ? `${bind} → ${dest}` : `${dest} ← ${bind} (${t("tunnels.server")})`;
}

function statusOf(tunnel: TunnelConfig) {
  return props.api.statusFor(props.sessionId, tunnel.id);
}

function statusLabel(tunnel: TunnelConfig): string {
  const status = statusOf(tunnel) ?? "idle";
  // Localize known statuses; fall back to the raw backend value otherwise.
  const known = ["idle", "starting", "running", "stopped", "error", "reconnecting"];
  return known.includes(status) ? t(`tunnels.status.${status}`) : status;
}

function newDraft(): DraftTunnel {
  return {
    id: crypto.randomUUID(),
    type: "local",
    name: "",
    bindAddress: "127.0.0.1",
    bindPort: "",
    destHost: "",
    destPort: "",
    autoStart: false,
  };
}

function startAdd() {
  formError.value = "";
  draft.value = reactive(newDraft());
}

function startEdit(tunnel: TunnelConfig) {
  formError.value = "";
  draft.value = reactive({
    id: tunnel.id,
    type: tunnel.type,
    name: tunnel.name ?? "",
    bindAddress: tunnel.bindAddress ?? "127.0.0.1",
    bindPort: String(tunnel.bindPort ?? ""),
    destHost: tunnel.destHost ?? "",
    destPort: tunnel.destPort != null ? String(tunnel.destPort) : "",
    autoStart: Boolean(tunnel.autoStart),
  });
}

function cancelDraft() {
  draft.value = null;
  formError.value = "";
}

function parsePort(value: string): number | null {
  const port = Number.parseInt(value, 10);
  if (!Number.isFinite(port) || port < 1 || port > 65535) {
    return null;
  }
  return port;
}

function saveDraft() {
  const current = draft.value;
  if (!current) {
    return;
  }

  const bindPort = parsePort(current.bindPort);
  if (bindPort === null) {
    formError.value = t("tunnels.errListenPort");
    return;
  }

  let destHost: string | undefined;
  let destPort: number | undefined;
  if (current.type !== "dynamic") {
    destHost = current.destHost.trim();
    if (!destHost) {
      formError.value = t("tunnels.errDestHost");
      return;
    }
    const parsedDestPort = parsePort(current.destPort);
    if (parsedDestPort === null) {
      formError.value = t("tunnels.errDestPort");
      return;
    }
    destPort = parsedDestPort;
  }

  const tunnel: TunnelConfig = {
    id: current.id,
    type: current.type,
    name: current.name.trim() || undefined,
    bindAddress: current.bindAddress.trim() || undefined,
    bindPort,
    destHost,
    destPort,
    autoStart: current.autoStart,
  };

  const next = [...list.value];
  const index = next.findIndex((item) => item.id === tunnel.id);
  if (index >= 0) {
    next[index] = tunnel;
  } else {
    next.push(tunnel);
  }
  emit("updateTunnels", next);
  draft.value = null;
  formError.value = "";
}

async function removeTunnel(tunnel: TunnelConfig) {
  if (props.api.isRunning(props.sessionId, tunnel.id)) {
    await props.api.stopTunnel(props.sessionId, tunnel.id).catch(() => undefined);
  }
  emit("updateTunnels", list.value.filter((item) => item.id !== tunnel.id));
}

async function toggleTunnel(tunnel: TunnelConfig) {
  busyTunnelId.value = tunnel.id;
  try {
    if (props.api.isRunning(props.sessionId, tunnel.id)) {
      await props.api.stopTunnel(props.sessionId, tunnel.id);
    } else {
      await props.api.startTunnel(props.sessionId, tunnel);
    }
  } catch {
    // Errors surface through the status badge / message.
  } finally {
    busyTunnelId.value = null;
  }
}
</script>

<template>
  <div class="tunnel-overlay" @click.self="emit('close')">
    <div class="tunnel-dialog" role="dialog" :aria-label="$t('tunnels.ariaLabel')">
      <div class="tunnel-header">
        <div>
          <div class="tunnel-title">{{ $t('tunnels.title') }}</div>
          <div class="tunnel-subtitle">{{ sshConfig.user }}@{{ sshConfig.host }}:{{ sshConfig.port }}</div>
        </div>
        <button class="tunnel-close" type="button" :aria-label="$t('common.close')" @click="emit('close')">×</button>
      </div>

      <div class="tunnel-body">
        <div v-if="list.length === 0 && !draft" class="tunnel-empty">
          {{ $t('tunnels.empty') }}
        </div>

        <ul v-if="list.length > 0" class="tunnel-list">
          <li v-for="tunnel in list" :key="tunnel.id" class="tunnel-row">
            <div class="tunnel-row-main">
              <div class="tunnel-row-line">
                <span class="tunnel-type" :data-type="tunnel.type">{{ typeLabel(tunnel.type) }}</span>
                <span v-if="tunnel.name" class="tunnel-name">{{ tunnel.name }}</span>
                <span class="tunnel-status" :data-status="statusOf(tunnel) ?? 'idle'">{{ statusLabel(tunnel) }}</span>
              </div>
              <div class="tunnel-route">{{ describeTunnel(tunnel) }}</div>
              <div v-if="api.messageFor(sessionId, tunnel.id)" class="tunnel-message">
                {{ api.messageFor(sessionId, tunnel.id) }}
              </div>
            </div>
            <div class="tunnel-row-actions">
              <button
                class="tunnel-btn"
                :class="{ primary: !api.isRunning(sessionId, tunnel.id), danger: api.isRunning(sessionId, tunnel.id) }"
                type="button"
                :disabled="busyTunnelId === tunnel.id"
                @click="toggleTunnel(tunnel)"
              >
                {{ api.isRunning(sessionId, tunnel.id) ? $t('tunnels.stop') : $t('tunnels.start') }}
              </button>
              <button class="tunnel-btn" type="button" :disabled="!!draft" @click="startEdit(tunnel)">{{ $t('common.edit') }}</button>
              <button class="tunnel-btn ghost" type="button" @click="removeTunnel(tunnel)">{{ $t('common.delete') }}</button>
            </div>
          </li>
        </ul>

        <form v-if="draft" class="tunnel-form" @submit.prevent="saveDraft">
          <div class="tunnel-form-title">{{ list.some((t) => t.id === draft!.id) ? $t('tunnels.editTunnel') : $t('tunnels.addTunnel') }}</div>

          <div class="tunnel-field tunnel-types">
            <label :class="{ selected: draft.type === 'local' }">
              <input v-model="draft.type" type="radio" value="local" /> {{ $t('tunnels.typeLocal') }}
            </label>
            <label :class="{ selected: draft.type === 'remote' }">
              <input v-model="draft.type" type="radio" value="remote" /> {{ $t('tunnels.typeRemote') }}
            </label>
            <label :class="{ selected: draft.type === 'dynamic' }">
              <input v-model="draft.type" type="radio" value="dynamic" /> {{ $t('tunnels.typeDynamic') }}
            </label>
          </div>

          <label class="tunnel-field">
            <span>{{ $t('tunnels.nameOptional') }}</span>
            <input v-model="draft.name" type="text" :placeholder="$t('tunnels.namePlaceholder')" />
          </label>

          <div class="tunnel-field-row">
            <label class="tunnel-field">
              <span>{{ draft.type === "remote" ? $t('tunnels.serverBindAddress') : $t('tunnels.listenAddress') }}</span>
              <input v-model="draft.bindAddress" type="text" placeholder="127.0.0.1" />
            </label>
            <label class="tunnel-field tunnel-field-port">
              <span>{{ draft.type === "remote" ? $t('tunnels.serverPort') : $t('tunnels.listenPort') }}</span>
              <input v-model="draft.bindPort" type="text" inputmode="numeric" placeholder="8080" />
            </label>
          </div>

          <div v-if="draft.type !== 'dynamic'" class="tunnel-field-row">
            <label class="tunnel-field">
              <span>{{ $t('tunnels.destHost') }}</span>
              <input v-model="draft.destHost" type="text" placeholder="dbhost.internal" />
            </label>
            <label class="tunnel-field tunnel-field-port">
              <span>{{ $t('tunnels.destPort') }}</span>
              <input v-model="draft.destPort" type="text" inputmode="numeric" placeholder="5432" />
            </label>
          </div>
          <p v-else class="tunnel-hint">
            {{ $t('tunnels.dynamicHint') }}
          </p>

          <label class="tunnel-checkbox">
            <input v-model="draft.autoStart" type="checkbox" />
            <span>{{ $t('tunnels.autoStart') }}</span>
          </label>

          <p v-if="formError" class="tunnel-form-error">{{ formError }}</p>

          <div class="tunnel-form-actions">
            <button class="tunnel-btn ghost" type="button" @click="cancelDraft">{{ $t('common.cancel') }}</button>
            <button class="tunnel-btn primary" type="submit">{{ $t('common.save') }}</button>
          </div>
        </form>
      </div>

      <div class="tunnel-footer">
        <button v-if="!draft" class="tunnel-btn primary" type="button" @click="startAdd">{{ $t('tunnels.addTunnelBtn') }}</button>
        <span class="tunnel-footer-spacer" />
        <button class="tunnel-btn ghost" type="button" @click="emit('close')">{{ $t('common.close') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tunnel-overlay {
  position: fixed;
  inset: 0;
  z-index: 60;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
  padding: 24px;
}

.tunnel-dialog {
  width: 640px;
  max-width: 92vw;
  max-height: 86vh;
  display: flex;
  flex-direction: column;
  background: var(--app-surface-1, var(--app-bg));
  color: var(--app-text);
  border: 1px solid var(--app-border);
  border-radius: 12px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}

.tunnel-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 18px 14px;
  border-bottom: 1px solid var(--app-border);
}

.tunnel-title {
  font-size: 15px;
  font-weight: 700;
}

.tunnel-subtitle {
  margin-top: 4px;
  font-size: 11px;
  color: var(--app-text-muted);
  word-break: break-all;
}

.tunnel-close {
  width: 28px;
  height: 28px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--app-text-muted);
  font-size: 18px;
  cursor: pointer;
}

.tunnel-close:hover {
  background: var(--app-hover);
  color: var(--app-text);
}

.tunnel-body {
  padding: 14px 18px;
  overflow-y: auto;
  flex: 1;
}

.tunnel-empty {
  padding: 24px 8px;
  text-align: center;
  color: var(--app-text-muted);
  font-size: 12px;
}

.tunnel-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.tunnel-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border: 1px solid var(--app-border);
  border-radius: 10px;
  background: var(--app-surface-2);
}

.tunnel-row-main {
  min-width: 0;
}

.tunnel-row-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.tunnel-type {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.02em;
  padding: 2px 6px;
  border-radius: 4px;
  background: var(--app-accent);
  color: var(--app-accent-contrast);
}

.tunnel-type[data-type="remote"] {
  background: var(--app-warning, #b8860b);
}

.tunnel-type[data-type="dynamic"] {
  background: var(--app-info, #2563eb);
}

.tunnel-name {
  font-size: 12px;
  font-weight: 600;
}

.tunnel-status {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--app-text-muted);
}

.tunnel-status[data-status="active"] {
  color: var(--app-success, #16a34a);
}

.tunnel-status[data-status="starting"] {
  color: var(--app-warning, #b8860b);
}

.tunnel-status[data-status="error"] {
  color: var(--app-danger);
}

.tunnel-route {
  margin-top: 4px;
  font-size: 12px;
  font-family: var(--app-mono, monospace);
  color: var(--app-text);
  word-break: break-all;
}

.tunnel-message {
  margin-top: 4px;
  font-size: 11px;
  color: var(--app-danger);
  word-break: break-word;
}

.tunnel-row-actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}

.tunnel-btn {
  border: 1px solid var(--app-border);
  background: var(--app-surface-2);
  color: var(--app-text);
  border-radius: 8px;
  padding: 6px 12px;
  font-size: 12px;
  cursor: pointer;
  transition: background 0.15s;
}

.tunnel-btn:hover:not(:disabled) {
  background: var(--app-surface-3);
}

.tunnel-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.tunnel-btn.primary {
  background: var(--app-accent);
  color: var(--app-accent-contrast);
  border-color: var(--app-accent);
}

.tunnel-btn.danger {
  border-color: var(--app-danger);
  color: var(--app-danger);
  background: var(--app-danger-soft);
}

.tunnel-btn.ghost {
  background: transparent;
}

.tunnel-form {
  margin-top: 12px;
  padding: 14px;
  border: 1px solid var(--app-border);
  border-radius: 10px;
  background: var(--app-hover-soft);
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.tunnel-form-title {
  font-size: 13px;
  font-weight: 700;
}

.tunnel-types {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.tunnel-types label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  font-size: 12px;
  border: 1px solid var(--app-border);
  border-radius: 8px;
  cursor: pointer;
}

.tunnel-types label.selected {
  border-color: var(--app-accent);
  background: var(--app-surface-2);
}

.tunnel-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  color: var(--app-text-muted);
}

.tunnel-field-row {
  display: flex;
  gap: 10px;
}

.tunnel-field-row .tunnel-field {
  flex: 1;
}

.tunnel-field-port {
  max-width: 140px;
}

.tunnel-field input[type="text"] {
  background: var(--app-input-bg);
  border: 1px solid var(--app-border);
  border-radius: 8px;
  padding: 8px 10px;
  font-size: 13px;
  color: var(--app-text);
}

.tunnel-field input[type="text"]:focus {
  outline: none;
  border-color: var(--app-accent);
}

.tunnel-checkbox {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--app-text);
}

.tunnel-hint {
  margin: 0;
  font-size: 11px;
  color: var(--app-text-muted);
}

.tunnel-form-error {
  margin: 0;
  font-size: 12px;
  color: var(--app-danger);
}

.tunnel-form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.tunnel-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--app-border);
}

.tunnel-footer-spacer {
  flex: 1;
}
</style>
