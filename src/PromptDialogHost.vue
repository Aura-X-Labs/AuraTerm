<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { promptQueue } from "./promptDialog";
import { t } from "./i18n";

const current = computed(() => promptQueue[0] ?? null);
const value = ref("");
const inputRef = ref<HTMLInputElement | null>(null);

watch(current, async (request) => {
  if (!request) return;
  value.value = request.defaultValue;
  await nextTick();
  inputRef.value?.focus();
  inputRef.value?.select();
});

function finish(result: string | null) {
  promptQueue.shift()?.resolve(result);
}
</script>

<template>
  <div
    v-if="current"
    class="prompt-overlay"
    @click.self="finish(null)"
    @keydown.esc.stop.prevent="finish(null)"
  >
    <div class="prompt-dialog" role="dialog" :aria-label="current.message">
      <div class="prompt-message">{{ current.message }}</div>
      <input
        ref="inputRef"
        v-model="value"
        class="prompt-input"
        type="text"
        autocomplete="off"
        spellcheck="false"
        @keydown.enter.prevent="finish(value)"
      />
      <div class="prompt-actions">
        <button class="prompt-btn" type="button" @click="finish(null)">{{ t('common.cancel') }}</button>
        <button class="prompt-btn primary" type="button" @click="finish(value)">{{ t('common.ok') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.prompt-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
}
.prompt-dialog {
  width: 380px;
  max-width: calc(100vw - 32px);
  background: var(--ui-panel-bg, #1e1e1e);
  color: var(--ui-fg, #dcdcdc);
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 10px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  padding: 16px 18px;
}
.prompt-message {
  font-size: 13px;
  margin-bottom: 10px;
  overflow-wrap: anywhere;
}
.prompt-input {
  width: 100%;
  box-sizing: border-box;
  background: var(--ui-input-bg, #16161a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 7px 9px;
  font-size: 13px;
}
.prompt-input:focus {
  outline: none;
  border-color: var(--ui-accent, #4d9fff);
}
.prompt-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}
.prompt-btn {
  background: var(--ui-input-bg, #26262b);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 7px 12px;
  font-size: 13px;
  cursor: pointer;
}
.prompt-btn:hover {
  border-color: var(--ui-accent, #4d9fff);
}
.prompt-btn.primary {
  background: var(--ui-accent, #2f6fd0);
  border-color: transparent;
  color: #fff;
}
</style>
