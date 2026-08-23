<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import {
  getSyncConfig,
  setSyncConfig,
  setSyncPassphrase,
  lockSyncPassphrase,
  cloudSyncPush,
  cloudSyncPull,
  cloudSyncNow,
  cloudSyncTestConnection,
  inputFromView,
  PROVIDER_LABELS,
  type SyncConfigView,
  type SyncProvider,
  type SyncResult,
} from "./cloudSync";
import { confirmDialog } from "./nativeDialogs";
import { t } from "./i18n";

const emit = defineEmits<{ close: []; openAccount: [] }>();

const view = ref<SyncConfigView | null>(null);
const busy = ref(false);
const message = ref("");
const isError = ref(false);

// Passphrase (held only in memory; never persisted or uploaded).
const passphrase = ref("");

// Editable form mirror of the config (non-secret fields).
const provider = ref<SyncProvider>("");
const deviceLabel = ref("");
const includeSettings = ref(true);
const includeKnownHosts = ref(true);
const includeCredentials = ref(false);
const autoSync = ref(false);

// Secret inputs — left blank means "keep what's stored".
const githubToken = ref("");
const githubGistId = ref("");
const giteeToken = ref("");
const giteeGistId = ref("");
const webdavUrl = ref("");
const webdavUsername = ref("");
const webdavPassword = ref("");

// AuraXLab (the official account-based service) is featured first.
const providers: Array<Exclude<SyncProvider, "">> = ["auraxlab", "github", "gitee", "webdav"];

function providerLabel(p: Exclude<SyncProvider, "">): string {
  return PROVIDER_LABELS[p];
}

const passphraseUnlocked = computed(() => view.value?.passphraseUnlocked ?? false);
const auraxlabSignedIn = computed(() => view.value?.auraxlab.tokenSet ?? false);

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
  // Feature the official AuraXLab account by default when nothing is configured.
  provider.value = next.provider || "auraxlab";
  deviceLabel.value = next.deviceLabel;
  includeSettings.value = next.includeSettings;
  includeKnownHosts.value = next.includeKnownHosts;
  includeCredentials.value = next.includeCredentials;
  autoSync.value = next.autoSync;
  githubGistId.value = next.github.gistId;
  giteeGistId.value = next.gitee.gistId;
  webdavUrl.value = next.webdav.url;
  webdavUsername.value = next.webdav.username;
  // Secret inputs intentionally left blank.
  githubToken.value = "";
  giteeToken.value = "";
  webdavPassword.value = "";
}

onMounted(async () => {
  try {
    hydrate(await getSyncConfig());
  } catch (e) {
    flash(String(e), true);
  }
});

/** Empty string -> null (keep stored secret); otherwise send the new value. */
function secret(value: string): string | null {
  return value.trim().length > 0 ? value : null;
}

async function withBusy<T>(fn: () => Promise<T>): Promise<T | undefined> {
  if (busy.value) return undefined;
  busy.value = true;
  try {
    return await fn();
  } catch (e) {
    flash(String(e), true);
    return undefined;
  } finally {
    busy.value = false;
  }
}

async function unlockPassphrase() {
  if (passphrase.value.trim().length === 0) {
    flash(t("cloudSync.enterPassphrase"), true);
    return;
  }
  await withBusy(async () => {
    await setSyncPassphrase(passphrase.value);
    hydrate(await getSyncConfig());
    flash(t("cloudSync.passphraseSet"));
  });
}

async function lockPassphrase() {
  await withBusy(async () => {
    await lockSyncPassphrase();
    passphrase.value = "";
    hydrate(await getSyncConfig());
    flash(t("cloudSync.syncLockedMsg"));
  });
}

async function saveConfig(): Promise<boolean> {
  const base = view.value ? inputFromView(view.value) : null;
  if (!base) return false;
  const next = await withBusy(async () => {
    const updated = await setSyncConfig({
      ...base,
      provider: provider.value,
      deviceLabel: deviceLabel.value,
      includeSettings: includeSettings.value,
      includeKnownHosts: includeKnownHosts.value,
      includeCredentials: includeCredentials.value,
      autoSync: autoSync.value,
      githubToken: secret(githubToken.value),
      githubGistId: githubGistId.value,
      giteeToken: secret(giteeToken.value),
      giteeGistId: giteeGistId.value,
      webdavUrl: webdavUrl.value,
      webdavUsername: webdavUsername.value,
      webdavPassword: secret(webdavPassword.value),
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
  return `${result.message} ${parts.join(" ")}`.trim();
}

async function doSyncNow() {
  if (!(await saveConfig())) return;
  await withBusy(async () => {
    const result = await cloudSyncNow(passphrase.value || null);
    hydrate(await getSyncConfig());
    flash(describeResult(result));
  });
}

async function doPush() {
  if (!(await saveConfig())) return;
  await withBusy(async () => {
    const result = await cloudSyncPush(passphrase.value || null);
    hydrate(await getSyncConfig());
    flash(describeResult(result));
  });
}

async function doPull(replace: boolean) {
  if (replace && !(await confirmDialog(t("cloudSync.confirmReplace")))) {
    return;
  }
  if (!(await saveConfig())) return;
  await withBusy(async () => {
    const result = await cloudSyncPull(replace, passphrase.value || null);
    hydrate(await getSyncConfig());
    flash(describeResult(result));
  });
}

async function testConnection() {
  if (!(await saveConfig())) return;
  await withBusy(async () => {
    flash(await cloudSyncTestConnection());
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
        <!-- Passphrase -->
        <section class="sync-section">
          <h3>{{ $t('cloudSync.step1') }}</h3>
          <p class="sync-hint">
            {{ $t('cloudSync.passphraseHint') }}
          </p>
          <div class="sync-row">
            <span class="sync-badge" :data-on="passphraseUnlocked">
              {{ passphraseUnlocked ? $t('cloudSync.unlocked') : $t('cloudSync.locked') }}
            </span>
            <input
              v-model="passphrase"
              class="sync-input"
              type="password"
              autocomplete="off"
              :placeholder="$t('cloudSync.passphrasePlaceholder')"
              @keyup.enter="unlockPassphrase"
            />
            <button class="sync-btn" type="button" :disabled="busy" @click="unlockPassphrase">{{ $t('cloudSync.set') }}</button>
            <button v-if="passphraseUnlocked" class="sync-btn" type="button" :disabled="busy" @click="lockPassphrase">
              {{ $t('cloudSync.lock') }}
            </button>
          </div>
        </section>

        <!-- Provider -->
        <section class="sync-section">
          <h3>{{ $t('cloudSync.step2') }}</h3>
          <div class="sync-provider-tabs">
            <button
              v-for="p in providers"
              :key="p"
              type="button"
              class="sync-provider-tab"
              :class="{ active: provider === p }"
              @click="provider = p"
            >
              {{ providerLabel(p) }}
            </button>
          </div>

          <div v-if="provider === 'github'" class="sync-fields">
            <label>{{ $t('cloudSync.patLabel') }} <span class="sync-muted">{{ $t('cloudSync.scopeGist') }}</span></label>
            <input v-model="githubToken" class="sync-input" type="password" autocomplete="off"
              :placeholder="view?.github.tokenSet ? $t('cloudSync.storedKeep') : 'ghp_…'" />
            <label>{{ $t('cloudSync.gistIdLabel') }} <span class="sync-muted">{{ $t('cloudSync.autoFilled') }}</span></label>
            <input v-model="githubGistId" class="sync-input" type="text" :placeholder="$t('cloudSync.optional')" />
          </div>

          <div v-else-if="provider === 'gitee'" class="sync-fields">
            <label>{{ $t('cloudSync.privateTokenLabel') }} <span class="sync-muted">{{ $t('cloudSync.scopeGists') }}</span></label>
            <input v-model="giteeToken" class="sync-input" type="password" autocomplete="off"
              :placeholder="view?.gitee.tokenSet ? $t('cloudSync.storedKeep') : 'token'" />
            <label>{{ $t('cloudSync.gistIdLabel') }} <span class="sync-muted">{{ $t('cloudSync.autoFilled') }}</span></label>
            <input v-model="giteeGistId" class="sync-input" type="text" :placeholder="$t('cloudSync.optional')" />
          </div>

          <div v-else-if="provider === 'webdav'" class="sync-fields">
            <label>{{ $t('cloudSync.fileUrl') }}</label>
            <input v-model="webdavUrl" class="sync-input" type="text"
              placeholder="https://dav.example.com/auraterm/auraterm-sync.enc" />
            <label>{{ $t('cloudSync.username') }}</label>
            <input v-model="webdavUsername" class="sync-input" type="text" autocomplete="off" />
            <label>{{ $t('cloudSync.password') }}</label>
            <input v-model="webdavPassword" class="sync-input" type="password" autocomplete="off"
              :placeholder="view?.webdav.passwordSet ? $t('cloudSync.storedKeep') : ''" />
          </div>

          <div v-else-if="provider === 'auraxlab'" class="sync-fields">
            <template v-if="auraxlabSignedIn">
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
          </div>
        </section>

        <!-- What to sync -->
        <section class="sync-section">
          <h3>{{ $t('cloudSync.step3') }}</h3>
          <label class="sync-check"><input type="checkbox" checked disabled /> {{ $t('cloudSync.bookmarksAlways') }}</label>
          <label class="sync-check"><input v-model="includeSettings" type="checkbox" /> {{ $t('cloudSync.settingsItem') }}</label>
          <label class="sync-check"><input v-model="includeKnownHosts" type="checkbox" /> {{ $t('cloudSync.knownHosts') }}</label>
          <label class="sync-check sync-danger">
            <input v-model="includeCredentials" type="checkbox" />
            {{ $t('cloudSync.savedCredentials') }}
          </label>
          <div class="sync-fields">
            <label>{{ $t('cloudSync.deviceLabel') }}</label>
            <input v-model="deviceLabel" class="sync-input" type="text" :placeholder="$t('cloudSync.deviceLabelPlaceholder')" />
          </div>
        </section>

        <div v-if="message" class="sync-message" :class="{ error: isError }">{{ message }}</div>
        <div class="sync-meta">{{ $t('cloudSync.lastSyncLabel') }} {{ lastSyncText }}</div>
      </div>

      <div class="sync-footer">
        <button class="sync-btn" type="button" :disabled="busy" @click="saveConfig">{{ $t('common.save') }}</button>
        <button class="sync-btn" type="button" :disabled="busy" @click="testConnection">{{ $t('cloudSync.test') }}</button>
        <span class="sync-spacer" />
        <button class="sync-btn" type="button" :disabled="busy" @click="doPull(false)">{{ $t('cloudSync.pullMerge') }}</button>
        <button class="sync-btn" type="button" :disabled="busy" @click="doPull(true)">{{ $t('cloudSync.pullReplace') }}</button>
        <button class="sync-btn" type="button" :disabled="busy" @click="doPush">{{ $t('cloudSync.push') }}</button>
        <button class="sync-btn primary" type="button" :disabled="busy" @click="doSyncNow">{{ $t('cloudSync.syncNow') }}</button>
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
.sync-hint {
  font-size: 12px;
  opacity: 0.7;
  margin: 0 0 8px;
  line-height: 1.5;
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
.sync-muted {
  opacity: 0.55;
  font-weight: 400;
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
.sync-provider-tabs {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.sync-provider-tab {
  padding: 6px 12px;
  background: var(--ui-input-bg, #2a2a2a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  font-size: 12px;
  cursor: pointer;
}
.sync-provider-tab.active {
  background: var(--ui-accent, #4a90d9);
  border-color: var(--ui-accent, #4a90d9);
  color: #fff;
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
.sync-badge {
  font-size: 11px;
  padding: 3px 8px;
  border-radius: 999px;
  background: rgba(200, 80, 80, 0.18);
  color: #e06c75;
  border: 1px solid rgba(200, 80, 80, 0.35);
}
.sync-badge[data-on="true"] {
  background: rgba(120, 200, 120, 0.18);
  color: #7fb069;
  border-color: rgba(120, 200, 120, 0.35);
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
