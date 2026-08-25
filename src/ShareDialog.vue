<script setup lang="ts">
import { computed, ref } from "vue";
import {
  createBookmarkShare,
  SHARE_TTL_CHOICES,
  formatShareTime,
  type ShareTicket,
} from "./bookmarkShare";
import { t } from "./i18n";

const props = defineProps<{
  /** The group being shared, as an absolute path. */
  root: string;
  /** `settings.bookmarkGroups`, so empty subfolders travel with the bundle. */
  bookmarkGroups: readonly string[];
}>();

const emit = defineEmits<{ close: [] }>();

const label = ref(props.root);
const note = ref("");
const ttlHours = ref<number>(168);
const maxRedeems = ref<number>(10);
const busy = ref(false);
const error = ref("");
const ticket = ref<ShareTicket | null>(null);
const copied = ref("");

const canCreate = computed(() => !busy.value && !ticket.value);

async function create() {
  busy.value = true;
  error.value = "";
  try {
    ticket.value = await createBookmarkShare(props.root, props.bookmarkGroups, {
      label: label.value.trim() || props.root,
      note: note.value.trim() || undefined,
      ttlHours: ttlHours.value,
      maxRedeems: maxRedeems.value,
    });
  } catch (failure) {
    error.value = String(failure);
  } finally {
    busy.value = false;
  }
}

async function copy(what: "code" | "link") {
  const value = what === "code" ? ticket.value?.code : ticket.value?.link;
  if (!value) {
    return;
  }
  try {
    await navigator.clipboard.writeText(value);
    copied.value = what;
    window.setTimeout(() => { copied.value = ""; }, 1600);
  } catch (failure) {
    error.value = String(failure);
  }
}
</script>

<template>
  <div class="sd-overlay" @click.self="emit('close')" @keydown.esc.stop.prevent="emit('close')">
    <div class="sd-dialog" role="dialog" :aria-label="t('bookmarkShare.title')">
      <header class="sd-header">
        <h2>{{ $t('bookmarkShare.title') }}</h2>
        <div class="sd-group">{{ root }}</div>
      </header>

      <template v-if="!ticket">
        <p class="sd-hint">{{ $t('bookmarkShare.intro') }}</p>

        <label class="sd-field">
          <span>{{ $t('bookmarkShare.label') }}</span>
          <input v-model="label" class="sd-input" type="text" spellcheck="false" :disabled="busy">
        </label>
        <label class="sd-field">
          <span>{{ $t('bookmarkShare.note') }}</span>
          <input
            v-model="note"
            class="sd-input"
            type="text"
            :placeholder="$t('bookmarkShare.notePlaceholder')"
            :disabled="busy"
          >
        </label>
        <div class="sd-row">
          <label class="sd-field">
            <span>{{ $t('bookmarkShare.expiry') }}</span>
            <select v-model.number="ttlHours" class="sd-input" :disabled="busy">
              <option v-for="hours in SHARE_TTL_CHOICES" :key="hours" :value="hours">
                {{ $t(`bookmarkShare.ttl${hours}`) }}
              </option>
            </select>
          </label>
          <label class="sd-field">
            <span>{{ $t('bookmarkShare.maxRedeems') }}</span>
            <select v-model.number="maxRedeems" class="sd-input" :disabled="busy">
              <option :value="1">{{ $t('bookmarkShare.redeemsOnce') }}</option>
              <option :value="5">5</option>
              <option :value="10">10</option>
              <option :value="100">100</option>
            </select>
          </label>
        </div>

        <p class="sd-privacy">{{ $t('bookmarkShare.privacy') }}</p>
      </template>

      <template v-else>
        <p class="sd-hint">{{ $t('bookmarkShare.readyHint') }}</p>
        <div class="sd-code">{{ ticket.code }}</div>
        <div class="sd-actions">
          <button class="sd-btn" type="button" @click="copy('code')">
            {{ copied === 'code' ? $t('bookmarkShare.copied') : $t('bookmarkShare.copyCode') }}
          </button>
          <button class="sd-btn" type="button" @click="copy('link')">
            {{ copied === 'link' ? $t('bookmarkShare.copied') : $t('bookmarkShare.copyLink') }}
          </button>
        </div>
        <div class="sd-link">{{ ticket.link }}</div>
        <dl class="sd-meta">
          <div>
            <dt>{{ $t('bookmarkShare.expiresAt') }}</dt>
            <dd>{{ formatShareTime(ticket.expiresAt) || '—' }}</dd>
          </div>
          <div>
            <dt>{{ $t('bookmarkShare.maxRedeems') }}</dt>
            <dd>{{ ticket.maxRedeems }}</dd>
          </div>
        </dl>
        <p class="sd-warning">{{ $t('bookmarkShare.onceOnly') }}</p>
      </template>

      <p v-if="error" class="sd-error">{{ error }}</p>

      <footer class="sd-footer">
        <button class="sd-btn" type="button" @click="emit('close')">
          {{ ticket ? $t('common.close') : $t('common.cancel') }}
        </button>
        <button v-if="canCreate" class="sd-btn sd-btn--primary" type="button" :disabled="busy" @click="create">
          {{ busy ? $t('bookmarkShare.creating') : $t('bookmarkShare.create') }}
        </button>
      </footer>
    </div>
  </div>
</template>

<style>
.sd-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
}
.sd-dialog {
  width: min(520px, 94vw);
  display: flex;
  flex-direction: column;
  gap: 10px;
  background: var(--ui-panel-bg, #1e1e1e);
  color: var(--ui-fg, #dcdcdc);
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 10px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  padding: 16px 18px;
}
.sd-header {
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.sd-header h2 {
  font-size: 14px;
  font-weight: 600;
  margin: 0;
}
.sd-group {
  font-size: 12px;
  opacity: 0.7;
  overflow-wrap: anywhere;
}
.sd-hint,
.sd-privacy,
.sd-warning,
.sd-error {
  margin: 0;
  font-size: 12px;
  overflow-wrap: anywhere;
}
.sd-hint {
  opacity: 0.8;
}
.sd-privacy {
  opacity: 0.65;
  border-left: 2px solid var(--ui-border, #3a3a3a);
  padding-left: 8px;
}
.sd-warning {
  color: var(--ui-warning, #d0a13a);
}
.sd-error {
  color: var(--ui-danger, #e06c6c);
}
.sd-row {
  display: flex;
  gap: 10px;
}
.sd-row .sd-field {
  flex: 1;
}
.sd-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
}
.sd-field > span {
  opacity: 0.8;
}
.sd-input {
  background: var(--ui-input-bg, #16161a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 6px 9px;
  font-size: 12px;
}
.sd-input:focus {
  outline: none;
  border-color: var(--ui-accent, #4d9fff);
}
.sd-code {
  font-family: var(--terminal-font, ui-monospace, monospace);
  font-size: 22px;
  letter-spacing: 0.08em;
  text-align: center;
  padding: 12px 8px;
  border: 1px dashed var(--ui-border, #3a3a3a);
  border-radius: 8px;
  user-select: all;
  overflow-wrap: anywhere;
}
.sd-link {
  font-size: 11px;
  opacity: 0.6;
  text-align: center;
  user-select: all;
  overflow-wrap: anywhere;
}
.sd-actions {
  display: flex;
  gap: 8px;
  justify-content: center;
}
.sd-meta {
  display: flex;
  gap: 24px;
  justify-content: center;
  margin: 0;
  font-size: 12px;
}
.sd-meta dt {
  opacity: 0.6;
}
.sd-meta dd {
  margin: 2px 0 0;
}
.sd-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 2px;
}
.sd-btn {
  background: var(--ui-input-bg, #26262b);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 6px 12px;
  font-size: 12px;
  cursor: pointer;
}
.sd-btn:hover:not(:disabled) {
  border-color: var(--ui-accent, #4d9fff);
}
.sd-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.sd-btn--primary {
  background: var(--ui-accent, #2f6fd0);
  border-color: transparent;
  color: #fff;
}
</style>
