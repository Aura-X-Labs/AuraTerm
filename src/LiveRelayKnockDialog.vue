<script setup lang="ts">
import { computed } from "vue";
import { t } from "./i18n";
import type { RelayKnock } from "./liveRelay";

/**
 * Provider-side approval queue for Live Relay: one of the account's own
 * devices finished the E2EE handshake and is waiting to be admitted. Only
 * the first entry is shown; App.vue pops it after the local user answers.
 * Unanswered knocks are denied by the backend after a timeout.
 */
const props = defineProps<{ queue: RelayKnock[] }>();
const emit = defineEmits<{ decide: [knock: RelayKnock, allow: boolean] }>();

const current = computed(() => props.queue[0] ?? null);
</script>

<template>
  <div v-if="current" class="knock-overlay">
    <div class="knock-dialog" role="alertdialog" :aria-label="t('liveRelay.knockTitle')">
      <div class="knock-title">{{ t('liveRelay.knockTitle') }}</div>
      <p class="knock-text">
        {{ t('liveRelay.knockText', { name: current.label || t('liveRelay.unknownDevice'), share: current.shareLabel }) }}
      </p>
      <p class="knock-fingerprint">
        {{ t('liveRelay.fingerprint') }}: <span class="knock-mono">{{ current.fingerprint }}</span>
      </p>
      <p class="knock-note">{{ t('liveRelay.knockViewOnlyNote') }}</p>
      <p v-if="queue.length > 1" class="knock-more">{{ t('liveRelay.knockMore', { n: queue.length - 1 }) }}</p>
      <div class="knock-actions">
        <button class="knock-btn primary" type="button" @click="emit('decide', current, true)">
          {{ t('liveRelay.allow') }}
        </button>
        <button class="knock-btn danger" type="button" @click="emit('decide', current, false)">
          {{ t('liveRelay.deny') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.knock-overlay { position: fixed; inset: 0; z-index: 1200; display: flex; align-items: flex-start; justify-content: center; padding-top: 12vh; background: rgba(0,0,0,.35); }
.knock-dialog { width: 420px; max-width: calc(100vw - 32px); padding: 16px 18px; color: var(--ui-fg,#ddd); background: var(--ui-panel-bg,#1e1e1e); border: 1px solid var(--ui-accent,#2f6fd0); border-radius: 10px; box-shadow: 0 18px 48px rgba(0,0,0,.55); }
.knock-title { font-size: 15px; font-weight: 600; margin-bottom: 6px; }
.knock-text { font-size: 13px; line-height: 1.5; margin: 0 0 6px; }
.knock-fingerprint,.knock-more,.knock-note { font-size: 12px; opacity: .75; margin: 0 0 6px; }
.knock-mono { font-family: ui-monospace,Consolas,monospace; }
.knock-actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 10px; }
.knock-btn { padding: 7px 12px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.knock-btn.primary { color: white; background: var(--ui-accent,#2f6fd0); }
.knock-btn.danger { color: #ff8b8f; border-color: #7a3338; }
</style>
