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
import { t } from "./i18n";

const props = defineProps<{ platform: string }>();
const emit = defineEmits<{ close: []; openCloudSync: [] }>();

// ── account (AuraXLab sign-in + Cloud Console traffic) ──────────────────────
const syncView = ref<SyncConfigView | null>(null);
const overview = ref<AccountOverview | null>(null);
const overviewError = ref("");
const overviewBusy = ref(false);

const signedIn = computed(() => syncView.value?.auraxlab.tokenSet ?? false);
const accountName = computed(() => syncView.value?.auraxlab.username ?? "");

// ── device binding (Cloud Console enrollment) ───────────────────────────────
const status = ref<CloudBridgeStatus | null>(null);
const busy = ref(false);
const message = ref("");
const isError = ref(false);

const serverUrl = ref(DEFAULT_AURAXLAB_URL);
const deviceLabel = ref("");
const email = ref("");
const password = ref("");

// Browser-approval fallback state.
const enrollment = ref<CloudBridgeEnrollment | null>(null);
let pollTimer: ReturnType<typeof setTimeout> | null = null;

const bound = computed(() => status.value?.enrolled ?? false);
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

async function bindWithPassword() {
  if (!email.value.trim() || !password.value) {
    note(t("account.needCredentials"), true);
    return;
  }
  busy.value = true;
  note("");
  try {
    await beginCloudBridgeEnrollment(
      serverUrl.value,
      deviceLabel.value.trim() || props.platform,
      props.platform,
    );
    await authorizeCloudBridgeEnrollment(email.value.trim(), password.value);
    const outcome = await redeemCloudBridgeEnrollment();
    if (outcome.status !== "ok") {
      throw new Error(t(`account.${outcome.status === "denied" ? "denied" : "expired"}`));
    }
    await connectCloudBridge();
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
    enrollment.value = await beginCloudBridgeEnrollment(
      serverUrl.value,
      deviceLabel.value.trim() || props.platform,
      props.platform,
    );
    await openExternalUrl(`${serverUrl.value.replace(/\/+$/, "")}/console`);
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
  if (!window.confirm(t("account.unbindConfirm"))) return;
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
        <!-- My account: sign in / profile + Cloud Console traffic -->
        <section class="account-section">
          <h3>{{ t('account.accountSection') }}</h3>

          <template v-if="!signedIn">
            <p class="account-hint">{{ t('account.signInIntro') }}</p>
            <AuraxlabAuthForm @signed-in="onSignedIn" />
          </template>

          <template v-else>
            <dl class="account-facts">
              <dt>{{ t('account.signedInAs') }}</dt>
              <dd>{{ accountName }}</dd>
              <dt>{{ t('account.server') }}</dt>
              <dd>{{ syncView?.auraxlab.baseUrl || DEFAULT_AURAXLAB_URL }}</dd>
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

        <!-- Unbound: sign in & bind -->
        <section v-else class="account-section">
          <h3>{{ t('account.deviceSection') }}</h3>
          <p class="account-hint">{{ t('account.notBound') }} {{ t('account.bindIntro') }}</p>

          <template v-if="!enrollment">
            <div class="account-grid">
              <label>{{ t('account.serverUrl') }}</label>
              <input v-model="serverUrl" class="account-input" type="url" autocomplete="off" spellcheck="false" />
              <label>{{ t('account.deviceLabel') }}</label>
              <input v-model="deviceLabel" class="account-input" type="text" autocomplete="off" />
              <label>{{ t('account.email') }}</label>
              <input v-model="email" class="account-input" type="email" autocomplete="username" />
              <label>{{ t('account.password') }}</label>
              <input
                v-model="password"
                class="account-input"
                type="password"
                autocomplete="current-password"
                @keyup.enter="bindWithPassword"
              />
            </div>
            <div class="account-actions">
              <button class="account-btn primary" type="button" :disabled="busy" @click="bindWithPassword">
                {{ t('account.signInAndBind') }}
              </button>
              <button class="account-btn" type="button" :disabled="busy" @click="bindWithBrowser">
                {{ t('account.browserApprove') }}
              </button>
            </div>
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
