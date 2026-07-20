<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { open as openExternalUrl } from "@tauri-apps/plugin-shell";
import {
  authorizeCloudBridgeEnrollment,
  beginCloudBridgeEnrollment,
  cloudBridgeStatus,
  connectCloudBridge,
  redeemCloudBridgeEnrollment,
  rotateCloudBridgeCredential,
  unbindCloudBridge,
  type CloudBridgeEnrollment,
  type CloudBridgeStatus,
} from "./cloudBridge";
import {
  auraxlabAccountOverview,
  auraxlabLogout,
  getSyncConfig,
  DEFAULT_AURAXLAB_URL,
  type AccountOverview,
  type SyncConfigView,
} from "./cloudSync";
import AuraxlabAuthForm from "./AuraxlabAuthForm.vue";
import { confirmDialog } from "./nativeDialogs";
import { t } from "./i18n";

const props = defineProps<{ platform: string }>();
const emit = defineEmits<{ close: []; openCloudSync: [] }>();

// One account, two credentials: the sync sign-in (axsync_ scoped credential)
// and the Cloud Console device binding (Ed25519 device identity) stay
// separate secrets with separate lifecycles, but this dialog presents them
// as one center and drives both from a single password entry.

// ── account (AuraXLab sign-in + Cloud Console traffic) ──────────────────────
const syncView = ref<SyncConfigView | null>(null);
const overview = ref<AccountOverview | null>(null);
const overviewError = ref("");
const overviewBusy = ref(false);

const signedIn = computed(() => syncView.value?.auraxlab.tokenSet ?? false);
const accountName = computed(() => syncView.value?.auraxlab.username ?? "");
const accountEmail = computed(() => overview.value?.email ?? "");

// ── device binding (Cloud Console enrollment) ───────────────────────────────
const status = ref<CloudBridgeStatus | null>(null);
const busy = ref(false);
const message = ref("");
const isError = ref(false);

// Shared server: the sign-in form binds this (v-model:server-url); binding a
// device while signed in uses the account's server so the two can't diverge.
const serverUrl = ref(DEFAULT_AURAXLAB_URL);
const deviceLabel = ref("");
const email = ref(""); // fallback only, when the profile email is unknown
const password = ref("");
const bindOnLogin = ref(true);

// Browser-approval fallback state.
const enrollment = ref<CloudBridgeEnrollment | null>(null);
let pollTimer: ReturnType<typeof setTimeout> | null = null;

const bound = computed(() => status.value?.enrolled ?? false);
const accountServer = computed(
  () => syncView.value?.auraxlab.baseUrl || serverUrl.value,
);
const connectionText = computed(() => {
  if (!status.value) return "";
  if (status.value.connected) return t("account.connected");
  if (status.value.standby) return t("account.standby");
  if (status.value.reconnecting) return t("account.reconnecting");
  return t("account.offline");
});

function note(text: string, error = false) {
  message.value = text;
  isError.value = error;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = "B";
  for (const next of units) {
    if (value < 1024) break;
    value /= 1024;
    unit = next;
  }
  return `${value.toFixed(1)} ${unit}`;
}

async function refresh() {
  try {
    status.value = await cloudBridgeStatus();
  } catch {
    /* bridge state is best-effort in this dialog */
  }
}

async function refreshOverview() {
  if (!signedIn.value || overviewBusy.value) return;
  overviewBusy.value = true;
  overviewError.value = "";
  try {
    overview.value = await auraxlabAccountOverview();
  } catch (error) {
    overview.value = null;
    overviewError.value = String(error);
  } finally {
    overviewBusy.value = false;
  }
}

async function loadAccount() {
  try {
    syncView.value = await getSyncConfig();
    // A previously used custom server should drive both flows.
    if (syncView.value?.auraxlab.baseUrl) {
      serverUrl.value = syncView.value.auraxlab.baseUrl;
    }
  } catch {
    /* the account section falls back to the sign-in form */
  }
  await refreshOverview();
}

function onSignedIn(view: SyncConfigView) {
  syncView.value = view;
  overview.value = null;
  void refreshOverview();
}

async function signOut() {
  busy.value = true;
  try {
    syncView.value = await auraxlabLogout();
    overview.value = null;
    note(t(bound.value ? "account.signedOutStillBound" : "account.signedOut"));
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

/** Enroll this installation: begin → authorize (password proof) → redeem →
 * connect. Shared by bind-on-login and the signed-in bind card. */
async function enrollDevice(server: string, emailArg: string, passwordArg: string) {
  await beginCloudBridgeEnrollment(
    server,
    deviceLabel.value.trim() || props.platform,
    props.platform,
  );
  await authorizeCloudBridgeEnrollment(emailArg, passwordArg);
  const outcome = await redeemCloudBridgeEnrollment();
  if (outcome.status !== "ok") {
    throw new Error(t(`account.${outcome.status === "denied" ? "denied" : "expired"}`));
  }
  await connectCloudBridge();
}

/** Chained onto the sign-in form's password entry: one sign-in issues the
 * sync credential and (opt-in) enrolls the device — the password is used for
 * both requests in the same moment and never stored. */
async function handlePostLogin(credentials: { email: string; password: string }) {
  if (!bindOnLogin.value || bound.value) return;
  try {
    await enrollDevice(serverUrl.value, credentials.email, credentials.password);
    note(t("account.bound"));
  } catch (error) {
    note(`${t("account.bindAfterLoginFailed")} ${String(error)}`, true);
  } finally {
    await refresh();
  }
}

/** Bind while already signed in: the account email is prefilled; only the
 * password is asked again (freshness proof — the sync credential itself is
 * deliberately not accepted for binding a new device identity). */
async function bindCurrentAccount() {
  const targetEmail = (accountEmail.value || email.value).trim();
  if (!targetEmail) {
    note(t("account.needCredentials"), true);
    return;
  }
  if (!password.value) {
    note(t("account.passwordRequired"), true);
    return;
  }
  busy.value = true;
  note("");
  try {
    await enrollDevice(accountServer.value, targetEmail, password.value);
    note(t("account.bound"));
  } catch (error) {
    note(String(error), true);
  } finally {
    password.value = "";
    busy.value = false;
    await refresh();
  }
}

async function bindWithBrowser() {
  busy.value = true;
  note("");
  try {
    const server = signedIn.value ? accountServer.value : serverUrl.value;
    enrollment.value = await beginCloudBridgeEnrollment(
      server,
      deviceLabel.value.trim() || props.platform,
      props.platform,
    );
    await openExternalUrl(`${server.replace(/\/+$/, "")}/console`);
    note(t("account.waitingApproval"));
    pollRedeem();
  } catch (error) {
    enrollment.value = null;
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

function stopPolling() {
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
}

function pollRedeem() {
  stopPolling();
  pollTimer = setTimeout(async () => {
    try {
      const outcome = await redeemCloudBridgeEnrollment();
      if (outcome.status === "pending") {
        pollRedeem();
        return;
      }
      enrollment.value = null;
      if (outcome.status === "ok") {
        await connectCloudBridge();
        note(t("account.bound"));
      } else {
        note(t(`account.${outcome.status === "denied" ? "denied" : "expired"}`), true);
      }
    } catch (error) {
      enrollment.value = null;
      note(String(error), true);
    }
    await refresh();
  }, 3000);
}

function cancelBrowserBind() {
  stopPolling();
  enrollment.value = null;
  note("");
}

async function rotate() {
  busy.value = true;
  try {
    await rotateCloudBridgeCredential();
    note(t("account.rotated"));
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
    await refresh();
  }
}

async function unbind() {
  if (!(await confirmDialog(t("account.unbindConfirm")))) return;
  busy.value = true;
  try {
    await unbindCloudBridge();
    note("");
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
    await refresh();
  }
}

async function openConsole() {
  const base = status.value?.baseUrl || serverUrl.value;
  await openExternalUrl(`${base.replace(/\/+$/, "")}/console`);
}

onMounted(() => {
  deviceLabel.value = props.platform;
  void refresh();
  void loadAccount();
});
onBeforeUnmount(stopPolling);
</script>

<template>
  <div class="account-overlay" @click.self="emit('close')">
    <div class="account-dialog" role="dialog" :aria-label="t('account.title')">
      <div class="account-header">
        <div>
          <div class="account-title">{{ t('account.title') }}</div>
          <div class="account-subtitle">{{ t('account.subtitle') }}</div>
        </div>
        <button class="account-close" type="button" :aria-label="t('account.close')" @click="emit('close')">×</button>
      </div>

      <div class="account-body">
        <!-- Access status: both scopes of the one account, at a glance -->
        <section class="account-section">
          <h3>{{ t('account.statusSection') }}</h3>
          <dl class="account-facts">
            <dt>{{ t('account.statusSync') }}</dt>
            <dd>
              <span class="account-badge" :data-on="signedIn">
                {{ signedIn ? t('account.statusSignedIn', { name: accountName }) : t('account.statusNotSignedIn') }}
              </span>
            </dd>
            <dt>{{ t('account.statusConsole') }}</dt>
            <dd class="account-badges">
              <span class="account-badge" :data-on="bound">
                {{ bound ? t('account.statusBound', { label: status?.deviceLabel || '' }) : t('account.statusNotBound') }}
              </span>
              <span v-if="bound" class="account-badge" :data-on="status?.connected">{{ connectionText }}</span>
            </dd>
          </dl>
        </section>

        <!-- My account: sign in / profile + Cloud Console traffic -->
        <section class="account-section">
          <h3>{{ t('account.accountSection') }}</h3>

          <template v-if="!signedIn">
            <p class="account-hint">{{ t('account.signInIntro') }}</p>
            <AuraxlabAuthForm
              v-model:server-url="serverUrl"
              show-server
              :post-login="handlePostLogin"
              @signed-in="onSignedIn"
            >
              <div v-if="!bound" class="account-bind-inline">
                <label class="account-checkbox">
                  <input v-model="bindOnLogin" type="checkbox" />
                  <span>{{ t('account.bindOnLogin') }}</span>
                </label>
                <template v-if="bindOnLogin">
                  <label class="account-inline-label">{{ t('account.deviceLabel') }}</label>
                  <input v-model="deviceLabel" class="account-input" type="text" autocomplete="off" />
                </template>
              </div>
            </AuraxlabAuthForm>
          </template>

          <template v-else>
            <dl class="account-facts">
              <dt>{{ t('account.signedInAs') }}</dt>
              <dd>{{ accountName }}</dd>
              <dt>{{ t('account.email') }}</dt>
              <dd>{{ accountEmail || '—' }}</dd>
              <dt>{{ t('account.server') }}</dt>
              <dd>{{ accountServer }}</dd>
            </dl>

            <h4 class="account-subheading">{{ t('account.trafficSection') }}</h4>
            <dl v-if="overview?.traffic" class="account-facts">
              <dt>{{ t('account.trafficTotal') }}</dt>
              <dd class="account-traffic-total">{{ formatBytes(overview.traffic.bytesTotal) }}</dd>
              <dt>{{ t('account.trafficUp') }}</dt>
              <dd>{{ formatBytes(overview.traffic.bytesUp) }}</dd>
              <dt>{{ t('account.trafficDown') }}</dt>
              <dd>{{ formatBytes(overview.traffic.bytesDown) }}</dd>
              <dt>{{ t('account.trafficSessions') }}</dt>
              <dd>{{ overview.traffic.sessions }}</dd>
            </dl>
            <p v-else class="account-hint">
              {{ overviewBusy ? '…' : (overviewError || t('account.trafficUnavailable')) }}
            </p>
            <p class="account-hint">{{ t('account.trafficHint') }}</p>

            <div class="account-actions">
              <button class="account-btn" type="button" :disabled="overviewBusy" @click="refreshOverview">
                {{ t('account.refresh') }}
              </button>
              <button class="account-btn" type="button" @click="emit('openCloudSync')">
                {{ t('account.cloudSyncEntry') }}
              </button>
              <button class="account-btn danger" type="button" :disabled="busy" @click="signOut">
                {{ t('account.signOut') }}
              </button>
            </div>
          </template>
        </section>

        <!-- Bound: device summary -->
        <section v-if="bound" class="account-section">
          <h3>{{ t('account.deviceSection') }}</h3>
          <dl class="account-facts">
            <dt>{{ t('account.deviceLabel') }}</dt>
            <dd>{{ status?.deviceLabel || '—' }}</dd>
            <dt>{{ t('account.server') }}</dt>
            <dd>{{ status?.baseUrl }}</dd>
            <dt>{{ t('account.deviceId') }}</dt>
            <dd class="account-mono">{{ status?.deviceId }}</dd>
            <dt>{{ t('account.fingerprint') }}</dt>
            <dd class="account-mono account-fingerprint">{{ status?.fingerprint }}</dd>
            <dt>{{ t('account.connection') }}</dt>
            <dd>
              <span class="account-badge" :data-on="status?.connected">{{ connectionText }}</span>
            </dd>
            <dt>{{ t('account.sharedSessions') }}</dt>
            <dd>{{ status?.shares.length ?? 0 }}</dd>
          </dl>
          <div class="account-actions">
            <button class="account-btn" type="button" @click="openConsole">{{ t('account.openConsole') }}</button>
            <button class="account-btn" type="button" :disabled="busy" @click="rotate">{{ t('account.rotate') }}</button>
            <button class="account-btn danger" type="button" :disabled="busy" @click="unbind">{{ t('account.unbind') }}</button>
          </div>
        </section>

        <!-- Unbound: bind to the signed-in account, or via browser approval -->
        <section v-else class="account-section">
          <h3>{{ t('account.deviceSection') }}</h3>

          <template v-if="!enrollment">
            <!-- Signed in: prefill the account, only the password is asked
                 again (freshness proof required by the server). -->
            <template v-if="signedIn">
              <p class="account-hint">{{ t('account.bindIntroSignedIn') }}</p>
              <div class="account-grid">
                <label>{{ t('account.email') }}</label>
                <span v-if="accountEmail" class="account-static">{{ accountEmail }}</span>
                <input v-else v-model="email" class="account-input" type="email" autocomplete="username" />
                <label>{{ t('account.deviceLabel') }}</label>
                <input v-model="deviceLabel" class="account-input" type="text" autocomplete="off" />
                <label>{{ t('account.password') }}</label>
                <input
                  v-model="password"
                  class="account-input"
                  type="password"
                  autocomplete="current-password"
                  @keyup.enter="bindCurrentAccount"
                />
              </div>
              <p class="account-hint">{{ t('account.passwordFreshnessHint') }}</p>
              <div class="account-actions">
                <button class="account-btn primary" type="button" :disabled="busy" @click="bindCurrentAccount">
                  {{ t('account.bindNow') }}
                </button>
                <button class="account-btn" type="button" :disabled="busy" @click="bindWithBrowser">
                  {{ t('account.browserApprove') }}
                </button>
              </div>
            </template>

            <!-- Not signed in: the sign-in form above binds in one step;
                 browser approval stays available as the no-password path. -->
            <template v-else>
              <p class="account-hint">{{ t('account.notBound') }} {{ t('account.signInToBindHint') }}</p>
              <div class="account-grid">
                <label>{{ t('account.deviceLabel') }}</label>
                <input v-model="deviceLabel" class="account-input" type="text" autocomplete="off" />
              </div>
              <div class="account-actions">
                <button class="account-btn" type="button" :disabled="busy" @click="bindWithBrowser">
                  {{ t('account.browserApprove') }}
                </button>
              </div>
            </template>
          </template>

          <template v-else>
            <p class="account-hint">{{ t('account.verifyFingerprint') }}</p>
            <dl class="account-facts">
              <dt>{{ t('account.userCode') }}</dt>
              <dd class="account-mono account-code">{{ enrollment.userCode }}</dd>
              <dt>{{ t('account.fingerprint') }}</dt>
              <dd class="account-mono account-fingerprint">{{ enrollment.fingerprint }}</dd>
            </dl>
            <p class="account-hint">{{ t('account.waitingApproval') }}</p>
            <div class="account-actions">
              <button class="account-btn" type="button" @click="cancelBrowserBind">{{ t('account.cancel') }}</button>
            </div>
          </template>
        </section>

        <p v-if="message" class="account-message" :data-error="isError">{{ message }}</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.account-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}
.account-dialog {
  width: 560px;
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
.account-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--ui-border, #3a3a3a);
}
.account-title {
  font-size: 16px;
  font-weight: 600;
}
.account-subtitle {
  font-size: 12px;
  opacity: 0.65;
}
.account-close {
  background: transparent;
  border: none;
  color: inherit;
  font-size: 22px;
  line-height: 1;
  cursor: pointer;
  opacity: 0.7;
}
.account-close:hover {
  opacity: 1;
}
.account-body {
  padding: 14px 18px;
  overflow-y: auto;
}
.account-section {
  margin-bottom: 18px;
}
.account-section h3 {
  margin: 0 0 8px;
  font-size: 13px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.8;
}
.account-subheading {
  margin: 14px 0 4px;
  font-size: 12px;
  font-weight: 600;
  opacity: 0.75;
}
.account-hint {
  font-size: 12px;
  opacity: 0.7;
  margin: 6px 0;
}
.account-hint a {
  color: var(--ui-accent, #4d9fff);
}
.account-grid {
  display: grid;
  grid-template-columns: 110px 1fr;
  gap: 8px 10px;
  align-items: center;
  margin: 10px 0;
}
.account-grid label {
  font-size: 12px;
  opacity: 0.75;
}
.account-input {
  background: var(--ui-input-bg, #16161a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 7px 9px;
  font-size: 13px;
}
.account-static {
  font-size: 13px;
  overflow-wrap: anywhere;
}
.account-bind-inline {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 8px;
}
.account-checkbox {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 13px;
  cursor: pointer;
}
.account-checkbox input {
  margin: 0;
}
.account-inline-label {
  font-size: 12px;
  opacity: 0.75;
  margin-top: 4px;
}
.account-facts {
  display: grid;
  grid-template-columns: 110px 1fr;
  gap: 6px 10px;
  font-size: 13px;
  margin: 10px 0;
}
.account-facts dt {
  opacity: 0.65;
}
.account-facts dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.account-traffic-total {
  font-weight: 600;
  color: var(--ui-accent, #4d9fff);
}
.account-mono {
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  font-size: 12px;
}
.account-code {
  font-size: 20px;
  letter-spacing: 3px;
  color: var(--ui-accent, #4d9fff);
}
.account-fingerprint {
  color: #e5c07b;
}
.account-badge {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 12px;
  background: #5c2b2e;
}
.account-badge[data-on="true"] {
  background: #1f4d2e;
}
.account-badges {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.account-actions {
  display: flex;
  gap: 8px;
  margin: 12px 0 4px;
  flex-wrap: wrap;
}
.account-btn {
  background: var(--ui-input-bg, #26262b);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 7px 12px;
  font-size: 13px;
  cursor: pointer;
}
.account-btn:hover:not(:disabled) {
  border-color: var(--ui-accent, #4d9fff);
}
.account-btn.primary {
  background: var(--ui-accent, #2f6fd0);
  border-color: transparent;
  color: #fff;
}
.account-btn.danger {
  border-color: #7a3338;
  color: #ff8b8f;
}
.account-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.account-message {
  font-size: 12px;
  margin-top: 10px;
  color: #7ec699;
}
.account-message[data-error="true"] {
  color: #ff8b8f;
}
</style>
