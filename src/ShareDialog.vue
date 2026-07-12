<script setup lang="ts">
import { ref } from "vue";
import { t } from "./i18n";

type TxPolicy = "read_only" | "read_write" | "temporary";

const props = defineProps<{ sessionLabel: string }>();
const emit = defineEmits<{
  cancel: [];
  share: [payload: { policy: TxPolicy; minutes: number }];
}>();

const policy = ref<TxPolicy>("read_only");
const minutes = ref(15);

const policies: Array<{ value: TxPolicy; label: string; hint: string }> = [
  { value: "read_only", label: t("cloudShare.readOnly"), hint: t("cloudShare.readOnlyHint") },
  { value: "read_write", label: t("cloudShare.readWrite"), hint: t("cloudShare.readWriteHint") },
  { value: "temporary", label: t("cloudShare.temporary"), hint: t("cloudShare.temporaryHint") },
];

function confirm() {
  emit("share", { policy: policy.value, minutes: minutes.value });
}
</script>

<template>
  <div class="share-overlay" @click.self="emit('cancel')">
    <div class="share-dialog" role="dialog" :aria-label="t('cloudShare.dialogTitle')">
      <div class="share-header">
        <div class="share-title">{{ t('cloudShare.dialogTitle') }}</div>
        <div class="share-subtitle">{{ t('cloudShare.dialogSubtitle') }}</div>
      </div>
      <div class="share-body">
        <div class="share-session">
          <span class="share-session-label">{{ t('cloudShare.session') }}</span>
          <span class="share-session-name">{{ props.sessionLabel }}</span>
        </div>
        <fieldset class="share-policies">
          <legend>{{ t('cloudShare.policyLabel') }}</legend>
          <label
            v-for="option in policies"
            :key="option.value"
            class="share-policy"
            :class="{ active: policy === option.value }"
          >
            <input type="radio" name="tx-policy" :value="option.value" v-model="policy" />
            <span class="share-policy-text">
              <span class="share-policy-name">{{ option.label }}</span>
              <span class="share-policy-hint">{{ option.hint }}</span>
            </span>
          </label>
        </fieldset>
        <div v-if="policy === 'temporary'" class="share-duration">
          <label for="share-minutes">{{ t('cloudShare.durationLabel') }}</label>
          <select id="share-minutes" v-model.number="minutes">
            <option :value="5">{{ t('cloudShare.minutes', { n: 5 }) }}</option>
            <option :value="15">{{ t('cloudShare.minutes', { n: 15 }) }}</option>
            <option :value="30">{{ t('cloudShare.minutes', { n: 30 }) }}</option>
            <option :value="60">{{ t('cloudShare.minutes', { n: 60 }) }}</option>
          </select>
        </div>
      </div>
      <div class="share-actions">
        <button class="share-btn" type="button" @click="emit('cancel')">{{ t('cloudShare.cancel') }}</button>
        <button class="share-btn primary" type="button" @click="confirm">{{ t('cloudShare.share') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.share-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}
.share-dialog {
  width: 460px;
  max-width: calc(100vw - 32px);
  background: var(--ui-panel-bg, #1e1e1e);
  color: var(--ui-fg, #dcdcdc);
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 10px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  overflow: hidden;
}
.share-header {
  padding: 14px 18px;
  border-bottom: 1px solid var(--ui-border, #3a3a3a);
}
.share-title {
  font-size: 15px;
  font-weight: 600;
}
.share-subtitle {
  font-size: 12px;
  opacity: 0.65;
  margin-top: 2px;
}
.share-body {
  padding: 14px 18px;
}
.share-session {
  display: flex;
  gap: 8px;
  align-items: baseline;
  margin-bottom: 12px;
  font-size: 13px;
}
.share-session-label {
  opacity: 0.6;
}
.share-session-name {
  font-weight: 600;
}
.share-policies {
  border: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.share-policies legend {
  font-size: 12px;
  opacity: 0.75;
  margin-bottom: 6px;
}
.share-policy {
  display: flex;
  gap: 10px;
  align-items: flex-start;
  padding: 9px 10px;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 8px;
  cursor: pointer;
}
.share-policy.active {
  border-color: var(--ui-accent, #4d9fff);
  background: rgba(77, 159, 255, 0.08);
}
.share-policy input {
  margin-top: 3px;
}
.share-policy-text {
  display: flex;
  flex-direction: column;
}
.share-policy-name {
  font-size: 13px;
  font-weight: 600;
}
.share-policy-hint {
  font-size: 12px;
  opacity: 0.65;
}
.share-duration {
  margin-top: 12px;
  display: flex;
  gap: 8px;
  align-items: center;
  font-size: 13px;
}
.share-duration select {
  background: var(--ui-input-bg, #16161a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 5px 8px;
}
.share-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--ui-border, #3a3a3a);
}
.share-btn {
  background: var(--ui-input-bg, #26262b);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 7px 14px;
  font-size: 13px;
  cursor: pointer;
}
.share-btn.primary {
  background: var(--ui-accent, #2f6fd0);
  border-color: transparent;
  color: #fff;
}
</style>
