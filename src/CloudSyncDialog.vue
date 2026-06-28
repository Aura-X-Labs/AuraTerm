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
  auraxlabLogin,
  auraxlabRegister,
  auraxlabRequestEmailCode,
  auraxlabVerifyEmailCode,
  auraxlabLogout,
  inputFromView,
  validateRegistration,
  PROVIDER_LABELS,
  DEFAULT_AURAXLAB_URL,
  SYNC_EMAIL_RE,
  SYNC_MIN_PASSWORD_LENGTH,
  type SyncConfigView,
  type SyncProvider,
  type SyncResult,
} from "./cloudSync";

const emit = defineEmits<{ close: [] }>();

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

// AuraXLab account flow (the server is always the official AuraXLab server).
const axEmail = ref("");
const axPassword = ref("");
const axUsername = ref(""); // registration only
const axMode = ref<"login" | "register">("login");
// Sign-up is verify-first: email -> code -> account details.
const axRegStep = ref<"email" | "code" | "details">("email");
const axCode = ref("");

function startRegister() {
  axMode.value = "register";
  axRegStep.value = "email";
  axCode.value = "";
}

function startLogin() {
  axMode.value = "login";
  axRegStep.value = "email";
  axCode.value = "";
}

// AuraXLab (the official account-based service) is featured first.
const providers: Array<Exclude<SyncProvider, "">> = ["auraxlab", "github", "gitee", "webdav"];

function providerLabel(p: Exclude<SyncProvider, "">): string {
  return PROVIDER_LABELS[p];
}

const passphraseUnlocked = computed(() => view.value?.passphraseUnlocked ?? false);
const auraxlabSignedIn = computed(() => view.value?.auraxlab.tokenSet ?? false);

const lastSyncText = computed(() => {
  const ts = view.value?.lastSyncAt;
  if (!ts) return "Never";
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
  axEmail.value = next.auraxlab.username;
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
    flash("Enter a sync passphrase first.", true);
    return;
  }
  await withBusy(async () => {
    await setSyncPassphrase(passphrase.value);
    hydrate(await getSyncConfig());
    flash("Sync passphrase set for this session.");
  });
}

async function lockPassphrase() {
  await withBusy(async () => {
    await lockSyncPassphrase();
    passphrase.value = "";
    hydrate(await getSyncConfig());
    flash("Sync locked.");
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
    flash("Sync settings saved.");
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
  if (replace && !window.confirm("Replace ALL local bookmarks with the cloud copy? This cannot be undone.")) {
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

async function signIn() {
  if (!axEmail.value.trim() || !axPassword.value) {
    flash("Enter your email and password.", true);
    return;
  }
  await withBusy(async () => {
    const updated = await auraxlabLogin(DEFAULT_AURAXLAB_URL, axEmail.value, axPassword.value);
    hydrate(updated);
    axPassword.value = "";
    flash("Signed in to AuraXLab.");
  });
}

// Sign-up step 1: email -> request a verification code.
async function sendCode() {
  if (!SYNC_EMAIL_RE.test(axEmail.value.trim())) {
    flash("A valid email address is required", true);
    return;
  }
  await withBusy(async () => {
    const msg = await auraxlabRequestEmailCode(DEFAULT_AURAXLAB_URL, axEmail.value.trim());
    axCode.value = "";
    axRegStep.value = "code";
    flash(msg);
  });
}

// Sign-up step 2: verify the emailed code.
async function verifyCode() {
  if (!axCode.value.trim()) {
    flash("Enter the verification code from your email.", true);
    return;
  }
  await withBusy(async () => {
    const msg = await auraxlabVerifyEmailCode(
      DEFAULT_AURAXLAB_URL,
      axEmail.value.trim(),
      axCode.value.trim(),
    );
    axRegStep.value = "details";
    flash(msg);
  });
}

// Sign-up step 3: create the account (email already verified).
async function register() {
  // Validate locally with the same rules the server enforces (immediate
  // feedback; the server still re-checks and owns duplicate detection).
  const validationError = validateRegistration(axEmail.value, axUsername.value, axPassword.value);
  if (validationError) {
    flash(validationError, true);
    return;
  }
  await withBusy(async () => {
    const msg = await auraxlabRegister(DEFAULT_AURAXLAB_URL, axEmail.value, axUsername.value, axPassword.value);
    axPassword.value = "";
    startLogin();
    flash(msg);
  });
}

async function signOut() {
  await withBusy(async () => {
    hydrate(await auraxlabLogout());
    flash("Signed out.");
  });
}
</script>

<template>
  <div class="sync-overlay" @click.self="emit('close')">
    <div class="sync-dialog" role="dialog" aria-label="Cloud Sync">
      <div class="sync-header">
        <div>
          <div class="sync-title">Cloud Sync</div>
          <div class="sync-subtitle">End-to-end encrypted · self-hosted providers</div>
        </div>
        <button class="sync-close" type="button" aria-label="Close" @click="emit('close')">×</button>
      </div>

      <div class="sync-body">
        <!-- Passphrase -->
        <section class="sync-section">
          <h3>1 · Sync passphrase</h3>
          <p class="sync-hint">
            Your data is encrypted with this passphrase <b>before</b> upload. It is never sent to the
            provider — keep it safe; if you lose it, the synced data cannot be recovered.
          </p>
          <div class="sync-row">
            <span class="sync-badge" :data-on="passphraseUnlocked">
              {{ passphraseUnlocked ? "Unlocked" : "Locked" }}
            </span>
            <input
              v-model="passphrase"
              class="sync-input"
              type="password"
              autocomplete="off"
              placeholder="Sync passphrase"
              @keyup.enter="unlockPassphrase"
            />
            <button class="sync-btn" type="button" :disabled="busy" @click="unlockPassphrase">Set</button>
            <button v-if="passphraseUnlocked" class="sync-btn" type="button" :disabled="busy" @click="lockPassphrase">
              Lock
            </button>
          </div>
        </section>

        <!-- Provider -->
        <section class="sync-section">
          <h3>2 · Storage provider</h3>
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
            <label>Personal access token <span class="sync-muted">(scope: gist)</span></label>
            <input v-model="githubToken" class="sync-input" type="password" autocomplete="off"
              :placeholder="view?.github.tokenSet ? '•••••• (stored — leave blank to keep)' : 'ghp_…'" />
            <label>Gist ID <span class="sync-muted">(auto-filled after first push)</span></label>
            <input v-model="githubGistId" class="sync-input" type="text" placeholder="(optional)" />
          </div>

          <div v-else-if="provider === 'gitee'" class="sync-fields">
            <label>Private token <span class="sync-muted">(scope: gists)</span></label>
            <input v-model="giteeToken" class="sync-input" type="password" autocomplete="off"
              :placeholder="view?.gitee.tokenSet ? '•••••• (stored — leave blank to keep)' : 'token'" />
            <label>Gist ID <span class="sync-muted">(auto-filled after first push)</span></label>
            <input v-model="giteeGistId" class="sync-input" type="text" placeholder="(optional)" />
          </div>

          <div v-else-if="provider === 'webdav'" class="sync-fields">
            <label>File URL</label>
            <input v-model="webdavUrl" class="sync-input" type="text"
              placeholder="https://dav.example.com/auraterm/auraterm-sync.enc" />
            <label>Username</label>
            <input v-model="webdavUsername" class="sync-input" type="text" autocomplete="off" />
            <label>Password</label>
            <input v-model="webdavPassword" class="sync-input" type="password" autocomplete="off"
              :placeholder="view?.webdav.passwordSet ? '•••••• (stored — leave blank to keep)' : ''" />
          </div>

          <div v-else-if="provider === 'auraxlab'" class="sync-fields">
            <template v-if="auraxlabSignedIn">
              <p class="sync-hint">
                Signed in as <b>{{ view?.auraxlab.username }}</b> on the official AuraXLab sync service.
              </p>
              <button class="sync-btn" type="button" :disabled="busy" @click="signOut">Sign out</button>
            </template>
            <!-- Sign in -->
            <template v-else-if="axMode === 'login'">
              <p class="sync-hint">Sign in to your account on the official AuraXLab sync service.</p>
              <label>Email</label>
              <input v-model="axEmail" class="sync-input" type="email" autocomplete="off" placeholder="you@example.com" />
              <label>Password</label>
              <input v-model="axPassword" class="sync-input" type="password" autocomplete="off" @keyup.enter="signIn" />
              <div class="sync-row">
                <button class="sync-btn primary" type="button" :disabled="busy" @click="signIn">Sign in</button>
                <button class="sync-btn" type="button" :disabled="busy" @click="startRegister">Need an account?</button>
              </div>
            </template>

            <!-- Sign up — step 1: email -->
            <template v-else-if="axRegStep === 'email'">
              <p class="sync-hint">Create an account on the official AuraXLab sync service. We'll email you a verification code first.</p>
              <label>Email</label>
              <input v-model="axEmail" class="sync-input" type="email" autocomplete="off" placeholder="you@example.com" @keyup.enter="sendCode" />
              <div class="sync-row">
                <button class="sync-btn primary" type="button" :disabled="busy" @click="sendCode">Send code</button>
                <button class="sync-btn" type="button" :disabled="busy" @click="startLogin">Have an account?</button>
              </div>
            </template>

            <!-- Sign up — step 2: verify code -->
            <template v-else-if="axRegStep === 'code'">
              <p class="sync-hint">We emailed a 6-digit code to <b>{{ axEmail }}</b>. Enter it to verify your email.</p>
              <label>Verification code</label>
              <input v-model="axCode" class="sync-input" type="text" inputmode="numeric" maxlength="6"
                autocomplete="one-time-code" placeholder="••••••" @keyup.enter="verifyCode" />
              <div class="sync-row">
                <button class="sync-btn primary" type="button" :disabled="busy" @click="verifyCode">Verify</button>
                <button class="sync-btn" type="button" :disabled="busy" @click="sendCode">Resend</button>
                <button class="sync-btn" type="button" :disabled="busy" @click="axRegStep = 'email'">Change email</button>
              </div>
            </template>

            <!-- Sign up — step 3: account details -->
            <template v-else>
              <p class="sync-hint"><b>{{ axEmail }}</b> verified ✓ — choose a username and password.</p>
              <label>Username</label>
              <input v-model="axUsername" class="sync-input" type="text" autocomplete="off" />
              <label>Password <span class="sync-muted">(at least {{ SYNC_MIN_PASSWORD_LENGTH }} characters)</span></label>
              <input v-model="axPassword" class="sync-input" type="password" autocomplete="new-password" @keyup.enter="register" />
              <div class="sync-row">
                <button class="sync-btn primary" type="button" :disabled="busy" @click="register">Create account</button>
                <button class="sync-btn" type="button" :disabled="busy" @click="axRegStep = 'code'">Back</button>
              </div>
            </template>
          </div>
        </section>

        <!-- What to sync -->
        <section class="sync-section">
          <h3>3 · What to sync</h3>
          <label class="sync-check"><input type="checkbox" checked disabled /> Bookmarks (always)</label>
          <label class="sync-check"><input v-model="includeSettings" type="checkbox" /> Settings (theme, fonts, quick buttons, rules)</label>
          <label class="sync-check"><input v-model="includeKnownHosts" type="checkbox" /> SSH known-hosts</label>
          <label class="sync-check sync-danger">
            <input v-model="includeCredentials" type="checkbox" />
            Saved credentials (passwords / keys) — needs the master password unlocked
          </label>
          <div class="sync-fields">
            <label>This device's label</label>
            <input v-model="deviceLabel" class="sync-input" type="text" placeholder="e.g. work-laptop" />
          </div>
        </section>

        <div v-if="message" class="sync-message" :class="{ error: isError }">{{ message }}</div>
        <div class="sync-meta">Last sync: {{ lastSyncText }}</div>
      </div>

      <div class="sync-footer">
        <button class="sync-btn" type="button" :disabled="busy" @click="saveConfig">Save</button>
        <button class="sync-btn" type="button" :disabled="busy" @click="testConnection">Test</button>
        <span class="sync-spacer" />
        <button class="sync-btn" type="button" :disabled="busy" @click="doPull(false)">Pull (merge)</button>
        <button class="sync-btn" type="button" :disabled="busy" @click="doPull(true)">Pull (replace)</button>
        <button class="sync-btn" type="button" :disabled="busy" @click="doPush">Push</button>
        <button class="sync-btn primary" type="button" :disabled="busy" @click="doSyncNow">Sync now</button>
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
