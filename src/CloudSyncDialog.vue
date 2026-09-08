<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import {
  getSyncConfig,
  setSyncConfig,
  acknowledgeLegacyProviderNotice,
  cloudSyncPush,
  cloudSyncPull,
  cloudSyncNow,
  cloudSyncTestConnection,
  cloudSyncMigrateLegacy,
  classifySyncError,
  inputFromView,
  type SyncConfigView,
  type SyncResult,
} from "./cloudSync";
import { confirmDialog } from "./nativeDialogs";
import { t } from "./i18n";

const props = defineProps<{
  /** Opened because a sync found a passphrase-era vault: show the migration step. */
  legacyVault?: boolean;
}>();
const emit = defineEmits<{ close: []; openAccount: []; synced: [result: SyncResult] }>();

const view = ref<SyncConfigView | null>(null);
const busy = ref(false);
const message = ref("");
const isError = ref(false);

// Editable form mirror of the config.
const deviceLabel = ref("");
const includeSettings = ref(true);
const includeKnownHosts = ref(true);
const includeCredentials = ref(false);
const autoSync = ref(false);

// One-time migration of a vault uploaded by a passphrase-era build.
const migrationNeeded = ref(props.legacyVault ?? false);
const legacyPassphrase = ref("");

const signedIn = computed(() => view.value?.auraxlab.tokenSet ?? false);
const credentialsAvailable = computed(() => view.value?.credentialsMode === "masterPassword");
const masterUnlocked = computed(() => view.value?.masterUnlocked ?? false);

const lastSyncText = computed(() => {
  const ts = view.value?.lastSyncAt;
  if (!ts) return t("cloudSync.never");
  return new Date(ts).toLocaleString();
});

function flash(text: string, error = false) {
  message.value = text;
  isError.value = error;
}

function hydrate(next: SyncConfigView) {
  view.value = next;
  deviceLabel.value = next.deviceLabel;
  includeSettings.value = next.includeSettings;
  includeKnownHosts.value = next.includeKnownHosts;
  includeCredentials.value = next.includeCredentials;
  autoSync.value = next.autoSync;
}

onMounted(async () => {
  try {
    hydrate(await getSyncConfig());
  } catch (e) {
    flash(String(e), true);
  }
});

async function withBusy<T>(fn: () => Promise<T>): Promise<T | undefined> {
  if (busy.value) return undefined;
  busy.value = true;
  try {
    return await fn();
  } catch (e) {
    reportError(e);
    return undefined;
  } finally {
    busy.value = false;
  }
}

/** A legacy vault turns the error into the migration step; everything else is shown as-is. */
function reportError(error: unknown) {
  const kind = classifySyncError(error);
  if (kind === "legacyVault") {
    migrationNeeded.value = true;
    flash(t("cloudSync.migrationNeeded"), true);
    return;
  }
  flash(String(error), true);
}

async function saveConfig(): Promise<boolean> {
  const base = view.value ? inputFromView(view.value) : null;
  if (!base) return false;
  const next = await withBusy(async () => {
    const updated = await setSyncConfig({
      ...base,
      deviceLabel: deviceLabel.value,
      includeSettings: includeSettings.value,
      includeKnownHosts: includeKnownHosts.value,
      includeCredentials: includeCredentials.value,
      autoSync: autoSync.value,
    });
    hydrate(updated);
    flash(t("cloudSync.settingsSaved"));
    return updated;
  });
  return next !== undefined;
}

function describeResult(result: SyncResult): string {
  const parts: string[] = [];
  if (result.pulled) {
    parts.push(`pulled (+${result.bookmarksAdded} bookmarks`);
    if (result.knownHostsAdded) parts.push(`+${result.knownHostsAdded} known-hosts`);
    if (result.credentialsSynced) parts.push(`${result.credentialsSynced} creds`);
    if (result.settingsApplied) parts.push("settings");
    parts.push(")");
  }
  if (result.pushed) parts.push(`pushed (${result.bookmarksTotal} bookmarks)`);
  const summary = `${result.message} ${parts.join(" ")}`.trim();
  return result.credentialsSkipped
    ? `${summary} — ${t(`cloudSync.skipped.${result.credentialsSkipped}`)}`
    : summary;
}

async function runAction(action: () => Promise<SyncResult>) {
  if (!(await saveConfig())) return;
  await withBusy(async () => {
    const result = await action();
    hydrate(await getSyncConfig());
    flash(describeResult(result), Boolean(result.credentialsSkipped));
    migrationNeeded.value = false;
    emit("synced", result);
  });
}

const doSyncNow = () => runAction(cloudSyncNow);
const doPush = () => runAction(cloudSyncPush);

async function doPull(replace: boolean) {
  if (replace && !(await confirmDialog(t("cloudSync.confirmReplace")))) {
    return;
  }
  await runAction(() => cloudSyncPull(replace));
}

async function testConnection() {
  if (!(await saveConfig())) return;
  await withBusy(async () => {
    flash(await cloudSyncTestConnection());
  });
}

async function migrate(overwrite: boolean) {
  if (overwrite) {
    if (!(await confirmDialog(t("cloudSync.confirmMigrateOverwrite")))) return;
  } else if (legacyPassphrase.value.trim().length === 0) {
    flash(t("cloudSync.enterLegacyPassphrase"), true);
    return;
  }
  await runAction(() => cloudSyncMigrateLegacy(overwrite ? null : legacyPassphrase.value, overwrite));
  legacyPassphrase.value = "";
}

async function dismissNotice() {
  await withBusy(async () => {
    await acknowledgeLegacyProviderNotice();
    hydrate(await getSyncConfig());
  });
}
</script>

<template>
  <div class="sync-overlay" @click.self="emit('close')">
    <div class="sync-dialog" role="dialog" :aria-label="$t('cloudSync.title')">
      <div class="sync-header">
        <div>
          <div class="sync-title">{{ $t('cloudSync.title') }}</div>
          <div class="sync-subtitle">{{ $t('cloudSync.subtitle') }}</div>
        </div>
        <button class="sync-close" type="button" :aria-label="$t('common.close')" @click="emit('close')">×</button>
      </div>

      <div class="sync-body">
        <!-- Removed providers (one-time notice) -->
        <section v-if="view?.legacyProviderNotice" class="sync-section sync-notice" data-testid="legacy-provider-notice">
          <p class="sync-hint">{{ $t('cloudSync.legacyProviderNotice') }}</p>
          <button class="sync-btn" type="button" :disabled="busy" @click="dismissNotice">{{ $t('cloudSync.dismiss') }}</button>
        </section>

        <!-- Account -->
        <section class="sync-section">
          <h3>{{ $t('cloudSync.accountSection') }}</h3>
          <template v-if="signedIn">
            <p class="sync-hint">
              {{ $t('cloudSync.signedInAs', { username: view?.auraxlab.username ?? '' }) }}
            </p>
          </template>
          <template v-else>
            <p class="sync-hint">{{ $t('cloudSync.signInInAccount') }}</p>
            <button class="sync-btn primary" type="button" @click="emit('openAccount')">
              {{ $t('cloudSync.openAccount') }}
            </button>
          </template>
          <p class="sync-hint">{{ $t('cloudSync.howItWorks') }}</p>
        </section>

        <!-- One-time migration of a passphrase-era vault -->
        <section v-if="migrationNeeded && signedIn" class="sync-section sync-migration" data-testid="legacy-migration">
          <h3>{{ $t('cloudSync.migrationTitle') }}</h3>
          <p class="sync-hint">{{ $t('cloudSync.migrationHint') }}</p>
          <div class="sync-row">
            <input
              v-model="legacyPassphrase"
              class="sync-input"
              type="password"
              autocomplete="off"
              :placeholder="$t('cloudSync.migrationPassphrase')"
              @keyup.enter="migrate(false)"
            />
            <button class="sync-btn primary" type="button" :disabled="busy" @click="migrate(false)">{{ $t('cloudSync.migrate') }}</button>
            <button class="sync-btn" type="button" :disabled="busy" @click="migrate(true)">{{ $t('cloudSync.migrateOverwrite') }}</button>
          </div>
        </section>

        <!-- What to sync -->
        <section class="sync-section">
          <h3>{{ $t('cloudSync.contentSection') }}</h3>
          <label class="sync-check"><input type="checkbox" checked disabled /> {{ $t('cloudSync.bookmarksAlways') }}</label>
          <label class="sync-check"><input v-model="includeSettings" type="checkbox" /> {{ $t('cloudSync.settingsItem') }}</label>
          <label class="sync-check"><input v-model="includeKnownHosts" type="checkbox" /> {{ $t('cloudSync.knownHosts') }}</label>
          <label class="sync-check sync-danger" :class="{ disabled: !credentialsAvailable }">
            <input v-model="includeCredentials" type="checkbox" :disabled="!credentialsAvailable" />
            {{ $t('cloudSync.savedCredentials') }}
          </label>
          <p v-if="!credentialsAvailable" class="sync-hint sync-indent">{{ $t('cloudSync.credentialsNeedMaster') }}</p>
          <template v-else-if="includeCredentials">
            <p v-if="!masterUnlocked" class="sync-hint sync-indent sync-warn">{{ $t('cloudSync.credentialsMasterLocked') }}</p>
            <p class="sync-hint sync-indent">{{ $t('cloudSync.credentialsSameMaster') }}</p>
          </template>
        </section>

        <!-- This device -->
        <section class="sync-section">
          <h3>{{ $t('cloudSync.deviceSection') }}</h3>
          <div class="sync-fields">
            <label>{{ $t('cloudSync.deviceLabel') }}</label>
            <input v-model="deviceLabel" class="sync-input" type="text" :placeholder="$t('cloudSync.deviceLabelPlaceholder')" />
          </div>
          <label class="sync-check"><input v-model="autoSync" type="checkbox" /> {{ $t('cloudSync.autoSync') }}</label>
        </section>

        <div v-if="message" class="sync-message" :class="{ error: isError }">{{ message }}</div>
        <div class="sync-meta">{{ $t('cloudSync.lastSyncLabel') }} {{ lastSyncText }}</div>
      </div>

      <div class="sync-footer">
        <button class="sync-btn" type="button" :disabled="busy" @click="saveConfig">{{ $t('common.save') }}</button>
        <button class="sync-btn" type="button" :disabled="busy || !signedIn" @click="testConnection">{{ $t('cloudSync.test') }}</button>
        <span class="sync-spacer" />
        <button class="sync-btn" type="button" :disabled="busy || !signedIn" @click="doPull(false)">{{ $t('cloudSync.pullMerge') }}</button>
        <button class="sync-btn" type="button" :disabled="busy || !signedIn" @click="doPull(true)">{{ $t('cloudSync.pullReplace') }}</button>
        <button class="sync-btn" type="button" :disabled="busy || !signedIn" @click="doPush">{{ $t('cloudSync.push') }}</button>
        <button class="sync-btn primary" type="button" :disabled="busy || !signedIn" @click="doSyncNow">{{ $t('cloudSync.syncNow') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.sync-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}
.sync-dialog {
  width: 640px;
  max-width: calc(100vw - 32px);
  max-height: calc(100vh - 48px);
  display: flex;
  flex-direction: column;
  background: var(--ui-panel-bg, #1e1e1e);
  color: var(--ui-fg, #dcdcdc);
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 10px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  overflow: hidden;
}
.sync-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--ui-border, #3a3a3a);
}
.sync-title {
  font-size: 16px;
  font-weight: 600;
}
.sync-subtitle {
  font-size: 12px;
  opacity: 0.65;
}
.sync-close {
  background: transparent;
  border: none;
  color: inherit;
  font-size: 22px;
  line-height: 1;
  cursor: pointer;
  opacity: 0.7;
}
.sync-close:hover {
  opacity: 1;
}
.sync-body {
  padding: 14px 18px;
  overflow-y: auto;
}
.sync-section {
  margin-bottom: 18px;
}
.sync-section h3 {
  font-size: 13px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.8;
  margin: 0 0 8px;
}
.sync-notice,
.sync-migration {
  padding: 10px 12px;
  border-radius: 8px;
  border: 1px solid rgba(224, 164, 88, 0.45);
  background: rgba(224, 164, 88, 0.1);
}
.sync-hint {
  font-size: 12px;
  opacity: 0.7;
  margin: 0 0 8px;
  line-height: 1.5;
}
.sync-hint.sync-indent {
  margin-left: 24px;
}
.sync-hint.sync-warn {
  color: var(--ui-warn, #e0a458);
  opacity: 0.9;
}
.sync-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.sync-fields {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 8px;
}
.sync-fields label {
  font-size: 12px;
  opacity: 0.8;
  margin-top: 6px;
}
.sync-input {
  flex: 1 1 auto;
  min-width: 0;
  padding: 7px 10px;
  background: var(--ui-input-bg, #2a2a2a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  font-size: 13px;
  font-family: inherit;
}
.sync-input:focus {
  outline: none;
  border-color: var(--ui-accent, #4a90d9);
}
.sync-check {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  padding: 4px 0;
}
.sync-check.sync-danger {
  color: var(--ui-warn, #e0a458);
}
.sync-check.disabled {
  opacity: 0.5;
}
.sync-message {
  font-size: 12px;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(120, 200, 120, 0.12);
  border: 1px solid rgba(120, 200, 120, 0.3);
  margin-top: 6px;
  word-break: break-word;
}
.sync-message.error {
  background: rgba(200, 80, 80, 0.12);
  border-color: rgba(200, 80, 80, 0.3);
  color: #e06c75;
}
.sync-meta {
  font-size: 11px;
  opacity: 0.55;
  margin-top: 8px;
}
.sync-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--ui-border, #3a3a3a);
  flex-wrap: wrap;
}
.sync-spacer {
  flex: 1 1 auto;
}
.sync-btn {
  padding: 7px 14px;
  background: var(--ui-input-bg, #2a2a2a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
}
.sync-btn:hover:not(:disabled) {
  border-color: var(--ui-accent, #4a90d9);
}
.sync-btn.primary {
  background: var(--ui-accent, #4a90d9);
  border-color: var(--ui-accent, #4a90d9);
  color: #fff;
}
.sync-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>
