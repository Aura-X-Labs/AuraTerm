<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { t } from "./i18n";
import {
  canAttachTo,
  relayListDevices,
  type RelayAttachTarget,
  type RelayDeviceEntry,
  type RelayProviderStatus,
} from "./liveRelay";

/** Live Relay: pick one of the account's own devices and attach to a
 *  session it already shares (design §5.13, consumer side). */
const props = defineProps<{ enrolled: boolean; status: RelayProviderStatus }>();
const emit = defineEmits<{
  close: [];
  attach: [device: RelayDeviceEntry, target: RelayAttachTarget];
  kick: [connectionId: string];
  openAccount: [];
}>();

const devices = ref<RelayDeviceEntry[]>([]);
const selectedId = ref<string | null>(null);
const loading = ref(false);
const error = ref("");

const selected = computed(() => devices.value.find((d) => d.device_id === selectedId.value) ?? null);

async function refresh() {
  if (!props.enrolled) return;
  loading.value = true;
  error.value = "";
  try {
    devices.value = await relayListDevices();
    if (!devices.value.some((d) => d.device_id === selectedId.value)) {
      selectedId.value = devices.value.find(canAttachTo)?.device_id ?? devices.value[0]?.device_id ?? null;
    }
  } catch (cause) {
    error.value = String(cause);
  } finally {
    loading.value = false;
  }
}

/** Why a selected device cannot be attached to right now, if anything. */
function blockedReason(device: RelayDeviceEntry): string {
  if (device.presence !== "online") return t("liveRelay.deviceOffline");
  if (!device.relay_policy?.enabled) return t("liveRelay.deviceRelayOff");
  if (!device.relay_policy.allow_attach) return t("liveRelay.deviceAttachOff");
  if (device.attach_targets.length === 0) return t("liveRelay.deviceNoShares");
  return "";
}

function presenceLabel(device: RelayDeviceEntry): string {
  if (device.presence === "online") return t("liveRelay.presenceOnline");
  if (device.presence === "idle") return t("liveRelay.presenceIdle");
  return t("liveRelay.presenceOffline");
}

onMounted(refresh);
</script>

<template>
  <div class="relay-overlay" @click.self="emit('close')">
    <div class="relay-dialog" role="dialog" :aria-label="t('liveRelay.title')">
      <header class="relay-header">
        <div>
          <div class="relay-title">{{ t('liveRelay.title') }}</div>
          <div class="relay-subtitle">{{ t('liveRelay.subtitle') }}</div>
        </div>
        <button class="relay-close" type="button" :aria-label="t('common.close')" @click="emit('close')">×</button>
      </header>

      <div v-if="!props.enrolled" class="relay-body">
        <p class="relay-hint">{{ t('liveRelay.signInRequired') }}</p>
        <div class="relay-actions">
          <button type="button" class="relay-btn primary" @click="emit('openAccount')">
            {{ t('liveRelay.openAccount') }}
          </button>
        </div>
      </div>

      <div v-else class="relay-body">
        <div class="relay-section-head">
          <span class="relay-label">{{ t('liveRelay.myDevices') }}</span>
          <button type="button" class="relay-link" :disabled="loading" @click="refresh">
            {{ loading ? t('liveRelay.refreshing') : t('liveRelay.refresh') }}
          </button>
        </div>

        <p v-if="error" class="relay-error">{{ error }}</p>
        <p v-else-if="!loading && devices.length === 0" class="relay-hint">{{ t('liveRelay.noDevices') }}</p>

        <ul v-else class="relay-devices">
          <li v-for="device in devices" :key="device.device_id">
            <button
              type="button"
              class="relay-device"
              :class="{ selected: device.device_id === selectedId, dim: !canAttachTo(device) }"
              @click="selectedId = device.device_id"
            >
              <span class="relay-dot" :class="device.presence" aria-hidden="true" />
              <span class="relay-device-main">
                <span class="relay-device-label">{{ device.label }}</span>
                <span class="relay-device-meta">
                  {{ device.platform || '—' }} · {{ presenceLabel(device) }}
                  <template v-if="device.attach_targets.length">
                    · {{ t('liveRelay.shareCount', { n: device.attach_targets.length }) }}
                  </template>
                </span>
              </span>
            </button>
          </li>
        </ul>

        <template v-if="selected">
          <div class="relay-divider" />
          <span class="relay-label">{{ t('liveRelay.attachTargets') }}</span>
          <p v-if="blockedReason(selected)" class="relay-hint">{{ blockedReason(selected) }}</p>
          <ul v-else class="relay-targets">
            <li v-for="target in selected.attach_targets" :key="target.session_id">
              <button type="button" class="relay-target" @click="emit('attach', selected, target)">
                <span class="relay-target-label">{{ target.share_label || t('liveRelay.untitledShare') }}</span>
                <span class="relay-target-meta">
                  {{ target.source_protocol || '—' }} ·
                  {{ target.read_only ? t('liveRelay.readOnly') : t('liveRelay.readWrite') }}
                </span>
              </button>
            </li>
          </ul>
          <p class="relay-hint">{{ t('liveRelay.viewOnlyNote') }}</p>
        </template>

        <!-- Provider side: who is attached to *this* device right now. -->
        <template v-if="props.status.peers.length">
          <div class="relay-divider" />
          <span class="relay-label">{{ t('liveRelay.incomingPeers') }}</span>
          <ul class="relay-targets">
            <li v-for="peer in props.status.peers" :key="peer.connectionId" class="relay-peer">
              <span class="relay-device-main">
                <span class="relay-device-label">{{ peer.label || t('liveRelay.unknownDevice') }}</span>
                <span class="relay-device-meta">
                  {{ peer.shareLabel }} ·
                  {{ peer.state === 'pending' ? t('liveRelay.peerPending') : t('liveRelay.peerViewing') }} ·
                  <span class="relay-mono">{{ peer.fingerprint }}</span>
                </span>
              </span>
              <button type="button" class="relay-btn danger" @click="emit('kick', peer.connectionId)">
                {{ t('liveRelay.kick') }}
              </button>
            </li>
          </ul>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.relay-overlay { position: fixed; inset: 0; z-index: 1100; display: flex; align-items: center; justify-content: center; background: rgba(0,0,0,.48); }
.relay-dialog { width: 520px; max-width: calc(100vw - 32px); max-height: calc(100vh - 64px); overflow: auto; color: var(--ui-fg,#ddd); background: var(--ui-panel-bg,#1e1e1e); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 10px; box-shadow: 0 18px 48px rgba(0,0,0,.55); }
.relay-header { display: flex; justify-content: space-between; align-items: center; padding: 14px 18px; border-bottom: 1px solid var(--ui-border,#3a3a3a); }
.relay-title { font-size: 16px; font-weight: 600; }
.relay-subtitle,.relay-hint { font-size: 12px; opacity: .7; line-height: 1.5; }
.relay-close { border: 0; background: transparent; color: inherit; font-size: 22px; cursor: pointer; }
.relay-body { padding: 14px 18px; display: flex; flex-direction: column; gap: 8px; }
.relay-section-head { display: flex; justify-content: space-between; align-items: baseline; }
.relay-label { font-size: 12px; opacity: .75; }
.relay-link { border: 0; background: transparent; color: var(--ui-accent,#2f6fd0); font-size: 12px; cursor: pointer; }
.relay-link:disabled { opacity: .5; cursor: default; }
.relay-devices,.relay-targets { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
.relay-device,.relay-target { width: 100%; display: flex; align-items: center; gap: 10px; padding: 8px 10px; text-align: left; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.relay-device.selected { border-color: var(--ui-accent,#2f6fd0); }
.relay-device.dim { opacity: .55; }
.relay-device-main { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.relay-device-label,.relay-target-label { font-size: 13px; }
.relay-device-meta,.relay-target-meta { font-size: 11px; opacity: .7; }
.relay-target { flex-direction: column; align-items: flex-start; gap: 2px; }
.relay-dot { width: 8px; height: 8px; border-radius: 50%; background: #777; flex: none; }
.relay-dot.online { background: #3fbf6f; }
.relay-dot.idle { background: #d8a13a; }
.relay-divider { height: 1px; background: var(--ui-border,#3a3a3a); margin: 4px 0; }
.relay-error { color: #ff8b8f; font-size: 12px; }
.relay-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 8px; }
.relay-btn { padding: 7px 12px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.relay-btn.primary { color: white; background: var(--ui-accent,#2f6fd0); }
.relay-btn.danger { color: #ff8b8f; border-color: #7a3338; }
.relay-peer { display: flex; align-items: center; gap: 10px; padding: 8px 10px; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; }
.relay-peer .relay-device-main { flex: 1; }
.relay-mono { font-family: ui-monospace,Consolas,monospace; }
</style>
