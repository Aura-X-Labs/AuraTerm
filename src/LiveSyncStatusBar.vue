<script setup lang="ts">
/**
 * The Live Sync status cluster and its one panel (design
 * docs/plans/live-sync-status-bar-design.md §2–3).
 *
 * The entry button is always there; activity pills are added only for what is
 * actually happening or needs an answer. Every one of them opens this same
 * panel — a status indicator never doubles as a switch. Child windows may read
 * everything here; the two global switches stay with the main window.
 */
import { computed, nextTick, ref } from "vue";
import type { CloudBridgeStatus } from "./cloudBridge";
import type { AssistStatus } from "./assist";
import type { RelayProviderStatus } from "./liveRelay";
import type { SyncConfigView } from "./cloudSync";
import { liveSyncLabel, liveSyncTooltip, type FeatureStatus, type LiveSyncFeature } from "./liveSyncStatus";
import { t } from "./i18n";

const props = defineProps<{
  statuses: FeatureStatus[];
  bridge: CloudBridgeStatus;
  assist: AssistStatus | null;
  relay: RelayProviderStatus;
  sync: SyncConfigView | null;
  syncBusy: boolean;
  consoleEnabled: boolean;
  allowRemoteSend: boolean;
  /** Relay tabs this machine opened towards other devices (consumer side). */
  outbound: { id: string; title: string }[];
  canManage: boolean;
  /** Narrow window: drop the feature name and plain viewer counts. */
  compact: boolean;
}>();

const emit = defineEmits<{
  refresh: [];
  toggleConsole: [];
  toggleRemoteSend: [];
  openWeb: [];
  openAccount: [];
  syncNow: [];
  openSync: [];
  manageShare: [];
  stopShare: [];
  revokeShareControl: [];
  manageRelay: [];
  revokeRelayControl: [];
}>();

// One control, always: the entry carries whatever is happening (design §7).
const status = computed(() => liveSyncLabel(props.statuses, !props.compact));
const tooltip = computed(() => `${t("liveSync.entryTooltip")}\n${liveSyncTooltip(props.statuses)}`);

const open = ref(false);
const focused = ref<LiveSyncFeature | null>(null);
const trigger = ref<HTMLButtonElement | null>(null);
const dialog = ref<HTMLDivElement | null>(null);

async function show(feature: LiveSyncFeature | null) {
  focused.value = feature;
  open.value = true;
  emit("refresh");
  await nextTick();
  dialog.value?.focus();
}

async function close() {
  open.value = false;
  await nextTick();
  trigger.value?.focus();
}

function trapFocus(event: KeyboardEvent) {
  if (event.key !== "Tab") return;
  const items = dialog.value?.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)");
  if (!items?.length) return;
  const first = items[0]!;
  const last = items[items.length - 1]!;
  if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.value)) {
    event.preventDefault(); last.focus();
  } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === dialog.value)) {
    event.preventDefault(); first.focus();
  }
}

const rowTitles: Record<LiveSyncFeature, string> = {
  sync: "liveSync.rowSync",
  console: "liveSync.rowConsole",
  share: "liveSync.rowShare",
  relay: "liveSync.rowRelay",
};
</script>

<template>
  <button
    ref="trigger"
    type="button"
    class="terminal-status-cloud live-sync-entry"
    :class="`cloud-${status.kind}`"
    :title="tooltip"
    aria-haspopup="dialog"
    :aria-expanded="open"
    @click="show(status.feature)"
  >
    <span class="cloud-status-dot" aria-hidden="true" />
    {{ status.text }}
    <span class="live-sync-caret" aria-hidden="true">▾</span>
  </button>

  <Teleport to="body">
    <div v-if="open" class="live-sync-overlay" @click.self="close" @keydown.esc.stop.prevent="close" @keydown="trapFocus">
      <div
        ref="dialog"
        class="live-sync-dialog"
        role="dialog"
        aria-modal="true"
        :aria-label="t('liveSync.panelTitle')"
        tabindex="-1"
      >
        <header>
          <strong>{{ t('liveSync.panelTitle') }}</strong>
          <button type="button" :aria-label="t('common.close')" @click="close">×</button>
        </header>
        <p v-if="!canManage" class="hint">{{ t('cloudShare.mainWindowOnly') }}</p>

        <section
          v-for="status in statuses"
          :key="status.feature"
          class="live-sync-row"
          :class="{ focused: focused === status.feature, stale: status.stale }"
        >
          <div class="live-sync-row-head">
            <strong>{{ t(rowTitles[status.feature]) }}</strong>
            <span class="live-sync-row-status" role="status">{{ status.detail }}</span>
          </div>
          <p v-if="status.stale" class="hint">{{ t('liveSync.staleHint') }}</p>

          <template v-if="status.feature === 'sync'">
            <div class="live-sync-actions">
              <button type="button" :disabled="!canManage || syncBusy" @click="emit('syncNow')">
                {{ t('liveSync.syncNow') }}
              </button>
              <button type="button" @click="close(); emit('openSync')">
                {{ sync && !sync.auraxlab.tokenSet ? t('liveSync.signIn') : t('liveSync.syncSettings') }}
              </button>
            </div>
          </template>

          <template v-else-if="status.feature === 'console'">
            <label>
              <input type="checkbox" :checked="consoleEnabled" :disabled="!canManage" @change="emit('toggleConsole')" />
              {{ t('cloudShare.autoShareLabel') }}
            </label>
            <p class="hint">{{ t('cloudShare.switchOffHint') }}</p>
            <label>
              <input type="checkbox" :checked="allowRemoteSend" :disabled="!canManage" @change="emit('toggleRemoteSend')" />
              {{ t('cloudShare.remoteSendLabel') }}
            </label>
            <strong class="live-sync-subhead">{{ t('cloudShare.sharedSessions') }}</strong>
            <ul v-if="bridge.shares.length">
              <li v-for="share in bridge.shares" :key="share.localSessionId">
                <span>{{ share.label }}</span>
                <span class="hint">{{ allowRemoteSend && share.txAllowed ? t('liveRelay.readWrite') : t('liveRelay.readOnly') }}</span>
              </li>
            </ul>
            <p v-else class="hint">{{ t('cloudShare.noSessions') }}</p>
            <div class="live-sync-actions">
              <button type="button" @click="close(); emit('openAccount')">{{ t('cloudShare.openAccount') }}</button>
              <button type="button" @click="emit('openWeb')">{{ t('cloudShare.openWeb') }}</button>
            </div>
          </template>

          <template v-else-if="status.feature === 'share'">
            <ul v-if="assist?.guests.length">
              <li v-for="guest in assist.guests" :key="guest.connectionId">
                <span>{{ guest.displayName || t('liveRelay.unknownDevice') }}</span>
                <span class="hint">{{ guest.controlRequested ? t('liveRelay.peerAsking') : guest.role }}</span>
              </li>
            </ul>
            <div class="live-sync-actions">
              <button type="button" @click="close(); emit('manageShare')">{{ t('liveSync.manageShare') }}</button>
              <button type="button" :disabled="!canManage || !status.controllers" @click="emit('revokeShareControl')">
                {{ t('liveSync.revokeControl') }}
              </button>
              <button type="button" :disabled="!canManage || !assist" @click="emit('stopShare')">
                {{ t('liveSync.stopShare') }}
              </button>
            </div>
          </template>

          <template v-else>
            <strong class="live-sync-subhead">{{ t('liveSync.inboundTitle') }}</strong>
            <ul v-if="relay.peers.length">
              <li v-for="peer in relay.peers" :key="peer.connectionId">
                <span>{{ peer.label }} · {{ peer.shareLabel }}</span>
                <span class="hint">{{ peer.controlRequested ? t('liveRelay.peerAsking') : peer.state }}</span>
              </li>
            </ul>
            <p v-else class="hint">{{ t('liveSync.noInbound') }}</p>
            <strong class="live-sync-subhead">{{ t('liveSync.outboundTitle') }}</strong>
            <ul v-if="outbound.length">
              <li v-for="tab in outbound" :key="tab.id"><span>{{ tab.title }}</span></li>
            </ul>
            <p v-else class="hint">{{ t('liveSync.noOutbound') }}</p>
            <div class="live-sync-actions">
              <button type="button" @click="close(); emit('manageRelay')">{{ t('liveSync.manageRelay') }}</button>
              <button type="button" :disabled="!canManage || !status.controllers" @click="emit('revokeRelayControl')">
                {{ t('liveSync.revokeControl') }}
              </button>
            </div>
          </template>
        </section>

        <p class="hint">{{ t('cloudShare.manageHint') }}</p>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.live-sync-caret { opacity: 0.7; }
.live-sync-overlay { position: fixed; inset: 0; z-index: 1100; display: grid; place-items: center; background: var(--app-overlay); }
.live-sync-dialog { width: 520px; max-width: calc(100vw - 32px); max-height: calc(100vh - 64px); overflow: auto; box-sizing: border-box; padding: 18px; border: 1px solid var(--app-border); border-radius: 10px; background: var(--app-dialog-bg); color: var(--app-text); box-shadow: 0 18px 48px var(--app-shadow-strong); font-size: 13px; line-height: 1.5; }
header, li { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
header { font-size: 16px; margin-bottom: 8px; }
.live-sync-row { padding: 10px 0; border-top: 1px solid var(--app-border); }
.live-sync-row.focused { border-left: 2px solid var(--app-accent); margin-left: -10px; padding-left: 8px; }
.live-sync-row.stale .live-sync-row-status { color: var(--app-warning); }
.live-sync-row-head { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; flex-wrap: wrap; }
.live-sync-row-status { overflow-wrap: anywhere; }
.live-sync-subhead { display: block; margin-top: 8px; font-weight: 600; }
.live-sync-actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
label { display: flex; align-items: center; gap: 8px; margin-top: 6px; }
input { accent-color: var(--app-accent); }
.hint { color: var(--app-text-secondary); font-size: 12px; margin: 4px 0 0; }
ul { list-style: none; padding: 0; margin: 4px 0 0; }
li { padding: 3px 0; overflow-wrap: anywhere; }
button { cursor: pointer; }
.live-sync-dialog button { border: 1px solid var(--app-border); border-radius: 5px; padding: 6px 10px; background: var(--app-surface-2); color: var(--app-text); }
.live-sync-dialog button:disabled { cursor: default; opacity: 0.5; }
</style>
