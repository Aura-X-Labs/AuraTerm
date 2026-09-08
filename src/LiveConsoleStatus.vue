<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import type { CloudBridgeStatus } from "./cloudBridge";
import { cloudStatusPill } from "./liveSyncStatus";
import { t } from "./i18n";

const props = defineProps<{
  bridge: CloudBridgeStatus;
  enabled: boolean;
  allowRemoteSend: boolean;
  canManage: boolean;
}>();
const emit = defineEmits<{
  refresh: [];
  toggleConsole: [];
  toggleRemoteSend: [];
  openWeb: [];
  openAccount: [];
}>();
const pill = computed(() => cloudStatusPill(props.bridge, props.enabled));
const open = ref(false);
const trigger = ref<HTMLButtonElement | null>(null);
const dialog = ref<HTMLDivElement | null>(null);
async function show() {
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
  const items = dialog.value?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)');
  if (!items?.length) return;
  const first = items[0]!;
  const last = items[items.length - 1]!;
  if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.value)) {
    event.preventDefault(); last.focus();
  } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === dialog.value)) {
    event.preventDefault(); first.focus();
  }
}
</script>

<template>
  <button
    v-if="pill"
    ref="trigger"
    type="button"
    class="terminal-status-cloud"
    :class="`cloud-${pill.kind}`"
    :title="t('cloudShare.statusTooltip')"
    aria-haspopup="dialog"
    :aria-expanded="open"
    @click="show"
  >
    <span class="cloud-status-dot" aria-hidden="true" />
    {{ pill.text }}
  </button>
  <Teleport to="body">
    <div v-if="open" class="console-status-overlay" @click.self="close" @keydown.esc.stop.prevent="close" @keydown="trapFocus">
      <div ref="dialog" class="console-status-dialog" role="dialog" aria-modal="true" :aria-label="t('cloudShare.manageTitle')" tabindex="-1">
        <header>
          <strong>{{ t('cloudShare.manageTitle') }}</strong>
          <button type="button" :aria-label="t('common.close')" @click="close">×</button>
        </header>
        <p role="status">{{ pill?.text || t('account.statusNotBound') }}</p>
        <label>
          <input type="checkbox" :checked="enabled" :disabled="!canManage" @change="emit('toggleConsole')" />
          {{ t('cloudShare.autoShareLabel') }}
        </label>
        <p class="hint">{{ t('cloudShare.switchOffHint') }}</p>
        <label>
          <input type="checkbox" :checked="allowRemoteSend" :disabled="!canManage" @change="emit('toggleRemoteSend')" />
          {{ t('cloudShare.remoteSendLabel') }}
        </label>
        <p v-if="!canManage" class="hint">{{ t('cloudShare.mainWindowOnly') }}</p>
        <p class="hint">{{ t('cloudShare.manageHint') }}</p>
        <strong>{{ t('cloudShare.sharedSessions') }}</strong>
        <ul v-if="bridge.shares.length">
          <li v-for="share in bridge.shares" :key="share.localSessionId">
            <span>{{ share.label }}</span>
            <span class="hint">{{ allowRemoteSend && share.txAllowed ? t('liveRelay.readWrite') : t('liveRelay.readOnly') }}</span>
          </li>
        </ul>
        <p v-else class="hint">{{ t('cloudShare.noSessions') }}</p>
        <footer>
          <button type="button" @click="close(); emit('openAccount')">{{ t('cloudShare.openAccount') }}</button>
          <button type="button" @click="emit('openWeb')">{{ t('cloudShare.openWeb') }}</button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.console-status-overlay { position: fixed; inset: 0; z-index: 1100; display: grid; place-items: center; background: var(--app-overlay); }
.console-status-dialog { width: 480px; max-width: calc(100vw - 32px); max-height: calc(100vh - 64px); overflow: auto; box-sizing: border-box; padding: 18px; border: 1px solid var(--app-border); border-radius: 10px; background: var(--app-dialog-bg); color: var(--app-text); box-shadow: 0 18px 48px var(--app-shadow-strong); font-size: 13px; line-height: 1.5; }
header, footer, li { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
header { font-size: 16px; }
label { display: flex; align-items: center; gap: 8px; }
input { accent-color: var(--app-accent); }
.hint { color: var(--app-text-secondary); font-size: 12px; }
ul { list-style: none; padding: 0; }
li { padding: 4px 0; overflow-wrap: anywhere; }
button { cursor: pointer; }
.console-status-dialog button { border: 1px solid var(--app-border); border-radius: 5px; padding: 6px 10px; background: var(--app-surface-2); color: var(--app-text); }
footer { margin-top: 16px; flex-wrap: wrap; }
</style>
