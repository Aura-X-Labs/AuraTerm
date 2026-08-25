<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import {
  forgetBookmarkSubscription,
  formatShareTime,
  listBookmarkShares,
  listBookmarkSubscriptions,
  refreshBookmarkSubscription,
  revokeBookmarkShare,
  type ShareRecord,
  type ShareSubscription,
} from "./bookmarkShare";
import { useBookmarkStore } from "./composables/useBookmarkStore";
import { t } from "./i18n";
import { confirmDialog } from "./nativeDialogs";

const emit = defineEmits<{
  close: [];
  /** Groups an imported update created; the manager folds them into settings. */
  createdGroups: [groups: string[]];
}>();

const store = useBookmarkStore();

const tab = ref<"mine" | "subscribed">("mine");
const shares = ref<ShareRecord[]>([]);
const subscriptions = ref<ShareSubscription[]>([]);
const busy = ref(false);
const error = ref("");
const status = ref("");

/** Shares the account still serves, newest first (the server orders them). */
const outbound = computed(() => shares.value);

function stateLabel(share: ShareRecord): string {
  if (share.state === "revoked") {
    return t("bookmarkShare.stateRevoked");
  }
  if (share.redeemable) {
    return t("bookmarkShare.stateActive");
  }
  // The server does not say which of the two it is; the numbers do.
  return share.redeemCount >= share.maxRedeems
    ? t("bookmarkShare.stateExhausted")
    : t("bookmarkShare.stateExpired");
}

async function load() {
  busy.value = true;
  error.value = "";
  try {
    subscriptions.value = await listBookmarkSubscriptions();
  } catch (failure) {
    error.value = String(failure);
  }
  try {
    shares.value = await listBookmarkShares();
  } catch (failure) {
    // Listing your own shares needs an account; subscriptions do not, so a
    // signed-out user still gets a working half of this dialog.
    error.value = t("bookmarkShare.listFailed", { error: String(failure) });
  } finally {
    busy.value = false;
  }
}

async function revoke(share: ShareRecord) {
  const label = share.label?.trim() || share.routeCode;
  if (!await confirmDialog(t("bookmarkShare.confirmRevoke", { label }), "warning")) {
    return;
  }
  busy.value = true;
  try {
    await revokeBookmarkShare(share.routeCode);
    status.value = t("bookmarkShare.revoked", { label });
    await load();
  } catch (failure) {
    error.value = t("bookmarkShare.revokeFailed", { error: String(failure) });
  } finally {
    busy.value = false;
  }
}

/** Re-fetch a subscribed share and run it through the usual import review;
 *  entries already held default to "update", so nothing doubles up. */
async function checkForUpdates(subscription: ShareSubscription) {
  busy.value = true;
  error.value = "";
  try {
    const bundle = await refreshBookmarkSubscription(subscription.bundleId);
    const imported = await store.importWithPreview("auraterm", bundle);
    if (!imported) {
      status.value = "";
      return;
    }
    emit("createdGroups", imported.result.createdGroups);
    status.value = t("bookmarks.importResult", {
      imported: imported.result.imported,
      updated: imported.result.updated,
      skipped: imported.result.skipped,
    });
    subscriptions.value = await listBookmarkSubscriptions();
  } catch (failure) {
    error.value = String(failure);
  } finally {
    busy.value = false;
  }
}

async function forget(subscription: ShareSubscription) {
  busy.value = true;
  try {
    await forgetBookmarkSubscription(subscription.bundleId);
    subscriptions.value = await listBookmarkSubscriptions();
  } catch (failure) {
    error.value = String(failure);
  } finally {
    busy.value = false;
  }
}

onMounted(load);
</script>

<template>
  <div class="sl-overlay" @click.self="emit('close')" @keydown.esc.stop.prevent="emit('close')">
    <div class="sl-dialog" role="dialog" :aria-label="t('bookmarkShare.myShares')">
      <header class="sl-header">
        <button class="sl-tab" :class="{ active: tab === 'mine' }" type="button" @click="tab = 'mine'">
          {{ $t('bookmarkShare.myShares') }}
        </button>
        <button class="sl-tab" :class="{ active: tab === 'subscribed' }" type="button" @click="tab = 'subscribed'">
          {{ $t('bookmarkShare.subscriptions') }}
        </button>
      </header>

      <div class="sl-body">
        <template v-if="tab === 'mine'">
          <p v-if="!busy && outbound.length === 0" class="sl-empty">{{ $t('bookmarkShare.noShares') }}</p>
          <div v-for="share in outbound" :key="share.routeCode" class="sl-row">
            <div class="sl-main">
              <div class="sl-name">{{ share.label?.trim() || $t('bookmarkShare.unnamed') }}</div>
              <div class="sl-meta">
                <span class="sl-route">{{ share.routeCode }}</span>
                <span>{{ stateLabel(share) }}</span>
                <span>{{ $t('bookmarkShare.redeemedCount', { count: share.redeemCount, max: share.maxRedeems }) }}</span>
                <span v-if="share.expiresAt">{{ $t('bookmarkShare.expiresAt') }} {{ formatShareTime(share.expiresAt) }}</span>
              </div>
            </div>
            <button
              v-if="share.state !== 'revoked'"
              class="sl-btn sl-btn--danger"
              type="button"
              :disabled="busy"
              @click="revoke(share)"
            >{{ $t('bookmarkShare.revoke') }}</button>
          </div>
        </template>

        <template v-else>
          <p v-if="!busy && subscriptions.length === 0" class="sl-empty">{{ $t('bookmarkShare.noSubscriptions') }}</p>
          <div v-for="subscription in subscriptions" :key="subscription.bundleId" class="sl-row">
            <div class="sl-main">
              <div class="sl-name">{{ subscription.label || $t('bookmarkShare.unnamed') }}</div>
              <div class="sl-meta">
                <span class="sl-route">{{ subscription.route }}</span>
                <span>{{ $t('bookmarkShare.importedAt') }} {{ formatShareTime(new Date(subscription.importedAt).toISOString()) }}</span>
                <span v-if="subscription.lastCheckedAt">
                  {{ $t('bookmarkShare.lastChecked') }}
                  {{ formatShareTime(new Date(subscription.lastCheckedAt).toISOString()) }}
                </span>
              </div>
            </div>
            <button class="sl-btn" type="button" :disabled="busy" @click="checkForUpdates(subscription)">
              {{ $t('bookmarkShare.checkUpdates') }}
            </button>
            <button class="sl-btn sl-btn--ghost" type="button" :disabled="busy" @click="forget(subscription)">
              {{ $t('bookmarkShare.forget') }}
            </button>
          </div>
        </template>
      </div>

      <p v-if="status" class="sl-status">{{ status }}</p>
      <p v-if="error" class="sl-error">{{ error }}</p>

      <footer class="sl-footer">
        <button class="sl-btn" type="button" :disabled="busy" @click="load">{{ $t('bookmarkShare.refresh') }}</button>
        <button class="sl-btn" type="button" @click="emit('close')">{{ $t('common.close') }}</button>
      </footer>
    </div>
  </div>
</template>

<style>
.sl-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
}
.sl-dialog {
  width: min(640px, 94vw);
  max-height: min(640px, 86vh);
  display: flex;
  flex-direction: column;
  gap: 10px;
  background: var(--ui-panel-bg, #1e1e1e);
  color: var(--ui-fg, #dcdcdc);
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 10px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  padding: 14px 16px;
}
.sl-header {
  display: flex;
  gap: 6px;
}
.sl-tab {
  background: transparent;
  color: inherit;
  border: none;
  border-bottom: 2px solid transparent;
  padding: 6px 10px;
  font-size: 13px;
  cursor: pointer;
  opacity: 0.65;
}
.sl-tab.active {
  opacity: 1;
  border-bottom-color: var(--ui-accent, #4d9fff);
}
.sl-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 8px;
}
.sl-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--ui-border, #2e2e2e);
}
.sl-row:last-child {
  border-bottom: none;
}
.sl-main {
  flex: 1;
  min-width: 0;
}
.sl-name {
  font-size: 13px;
  overflow-wrap: anywhere;
}
.sl-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 3px 12px;
  margin-top: 3px;
  font-size: 11px;
  opacity: 0.65;
}
.sl-route {
  font-family: var(--terminal-font, ui-monospace, monospace);
  letter-spacing: 0.06em;
}
.sl-empty {
  margin: 0;
  padding: 24px;
  text-align: center;
  font-size: 12px;
  opacity: 0.6;
}
.sl-status,
.sl-error {
  margin: 0;
  font-size: 12px;
  overflow-wrap: anywhere;
}
.sl-status {
  opacity: 0.75;
}
.sl-error {
  color: var(--ui-danger, #e06c6c);
}
.sl-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.sl-btn {
  flex: none;
  background: var(--ui-input-bg, #26262b);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 5px 10px;
  font-size: 12px;
  cursor: pointer;
}
.sl-btn:hover:not(:disabled) {
  border-color: var(--ui-accent, #4d9fff);
}
.sl-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.sl-btn--danger:hover:not(:disabled) {
  border-color: var(--ui-danger, #e06c6c);
}
.sl-btn--ghost {
  opacity: 0.75;
}
</style>
