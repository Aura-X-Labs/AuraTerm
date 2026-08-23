<script setup lang="ts">
import { computed } from "vue";
import type { AssistKnock } from "./assist";
import { t } from "./i18n";

/**
 * Queue of host decisions for Remote Assist: a guest waiting to be admitted
 * ("join") or an admitted guest asking for control ("control"). Only the
 * first entry is shown; App.vue pops it after the host answers.
 */
const props = defineProps<{ queue: AssistKnock[]; allowControl: boolean }>();
const emit = defineEmits<{
  decide: [knock: AssistKnock, decision: "allow_view" | "allow_control" | "deny"];
}>();

const current = computed(() => props.queue[0] ?? null);
const label = computed(() => {
  if (!current.value) return "";
  const name = current.value.displayName || t("assist.anonymousGuest");
  const client = current.value.client === "web" ? "Web" : current.value.client === "auraterm" ? "AuraTerm" : "";
  return client ? `${name} (${client})` : name;
});
</script>

<template>
  <div v-if="current" class="knock-overlay">
    <div class="knock-dialog" role="alertdialog" :aria-label="t('assist.knockTitle')">
      <div class="knock-title">{{ current.kind === 'control' ? t('assist.knockControlTitle') : t('assist.knockTitle') }}</div>
      <p class="knock-text">
        {{ current.kind === 'control' ? t('assist.knockControlText', { name: label }) : t('assist.knockJoinText', { name: label }) }}
      </p>
      <p class="knock-fingerprint">{{ t('assist.fingerprint') }}: <span class="knock-mono">{{ current.fingerprint }}</span></p>
      <p v-if="queue.length > 1" class="knock-more">{{ t('assist.knockMore', { n: queue.length - 1 }) }}</p>
      <div class="knock-actions">
        <template v-if="current.kind === 'join'">
          <button class="knock-btn" type="button" @click="emit('decide', current, 'allow_view')">{{ t('assist.allowView') }}</button>
          <button v-if="allowControl" class="knock-btn primary" type="button" @click="emit('decide', current, 'allow_control')">{{ t('assist.allowControl') }}</button>
        </template>
        <template v-else>
          <button class="knock-btn primary" type="button" @click="emit('decide', current, 'allow_control')">{{ t('assist.grantControl') }}</button>
        </template>
        <button class="knock-btn danger" type="button" @click="emit('decide', current, 'deny')">{{ current.kind === 'join' ? t('assist.deny') : t('assist.keepViewOnly') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.knock-overlay { position: fixed; inset: 0; z-index: 1200; display: flex; align-items: flex-start; justify-content: center; padding-top: 12vh; background: rgba(0,0,0,.35); }
.knock-dialog { width: 420px; max-width: calc(100vw - 32px); padding: 16px 18px; color: var(--ui-fg,#ddd); background: var(--ui-panel-bg,#1e1e1e); border: 1px solid var(--ui-accent,#2f6fd0); border-radius: 10px; box-shadow: 0 18px 48px rgba(0,0,0,.55); }
.knock-title { font-size: 15px; font-weight: 600; margin-bottom: 6px; }
.knock-text { font-size: 13px; line-height: 1.5; margin: 0 0 6px; }
.knock-fingerprint,.knock-more { font-size: 12px; opacity: .75; margin: 0 0 6px; }
.knock-mono { font-family: ui-monospace,Consolas,monospace; }
.knock-actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 10px; }
.knock-btn { padding: 7px 12px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.knock-btn.primary { color: white; background: var(--ui-accent,#2f6fd0); }
.knock-btn.danger { color: #ff8b8f; border-color: #7a3338; }
</style>
