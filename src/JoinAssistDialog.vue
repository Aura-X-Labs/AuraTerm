<script setup lang="ts">
import { ref } from "vue";
import { t } from "./i18n";

/** Remote Assist: join another AuraTerm's session with a code or link. */
const emit = defineEmits<{ close: []; join: [code: string, displayName: string] }>();
const code = ref("");
const displayName = ref("");
const error = ref("");

function normalise(raw: string): string {
  const hash = raw.lastIndexOf("#");
  const value = (hash >= 0 ? raw.slice(hash + 1) : raw).replace(/[^0-9a-zA-Z]/g, "").toUpperCase();
  return value;
}

function submit() {
  const value = normalise(code.value);
  if (value.length !== 12 || /[^BCDFGHJKLMNPQRSTVWXZ2-9]/.test(value)) {
    error.value = t("assist.joinInvalidCode");
    return;
  }
  emit("join", value, displayName.value.trim().slice(0, 32));
}
</script>

<template>
  <div class="join-overlay" @click.self="emit('close')">
    <div class="join-dialog" role="dialog" :aria-label="t('assist.joinTitle')">
      <header class="join-header">
        <div>
          <div class="join-title">{{ t('assist.joinTitle') }}</div>
          <div class="join-subtitle">{{ t('assist.joinSubtitle') }}</div>
        </div>
        <button class="join-close" type="button" :aria-label="t('common.close')" @click="emit('close')">×</button>
      </header>
      <form class="join-body" @submit.prevent="submit">
        <label class="join-label">{{ t('assist.joinCodeLabel') }}</label>
        <input v-model="code" class="join-input join-code" placeholder="XXXX-XXXX-XXXX" autocapitalize="characters" spellcheck="false" autofocus @keydown.stop>
        <label class="join-label">{{ t('assist.joinNameLabel') }}</label>
        <input v-model="displayName" class="join-input" maxlength="32" :placeholder="t('assist.joinNamePlaceholder')" @keydown.stop>
        <p class="join-hint">{{ t('assist.joinHint') }}</p>
        <p v-if="error" class="join-error">{{ error }}</p>
        <div class="join-actions">
          <button type="button" class="join-btn" @click="emit('close')">{{ t('common.cancel') }}</button>
          <button type="submit" class="join-btn primary">{{ t('assist.joinSubmit') }}</button>
        </div>
      </form>
    </div>
  </div>
</template>

<style scoped>
.join-overlay { position: fixed; inset: 0; z-index: 1100; display: flex; align-items: center; justify-content: center; background: rgba(0,0,0,.48); }
.join-dialog { width: 460px; max-width: calc(100vw - 32px); color: var(--ui-fg,#ddd); background: var(--ui-panel-bg,#1e1e1e); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 10px; box-shadow: 0 18px 48px rgba(0,0,0,.55); }
.join-header { display: flex; justify-content: space-between; align-items: center; padding: 14px 18px; border-bottom: 1px solid var(--ui-border,#3a3a3a); }
.join-title { font-size: 16px; font-weight: 600; }
.join-subtitle,.join-hint { font-size: 12px; opacity: .7; line-height: 1.5; }
.join-close { border: 0; background: transparent; color: inherit; font-size: 22px; cursor: pointer; }
.join-body { padding: 14px 18px; display: flex; flex-direction: column; gap: 6px; }
.join-label { font-size: 12px; opacity: .75; margin-top: 6px; }
.join-input { box-sizing: border-box; width: 100%; padding: 7px 9px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; }
.join-code { font-family: ui-monospace,Consolas,monospace; font-size: 22px; letter-spacing: .1em; text-transform: uppercase; }
.join-error { color: #ff8b8f; font-size: 12px; }
.join-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 8px; }
.join-btn { padding: 7px 12px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.join-btn.primary { color: white; background: var(--ui-accent,#2f6fd0); }
</style>
