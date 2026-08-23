<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { open as openExternalUrl } from "@tauri-apps/plugin-shell";
import {
  accountLogout,
  accountState,
  enableConsole,
  pauseConsole,
  refreshAccount,
  type AuraXLabAccountState,
} from "./account";
import AuraxlabAuthForm from "./AuraxlabAuthForm.vue";
import { t } from "./i18n";

const props = defineProps<{ platform: string }>();
const emit = defineEmits<{ close: []; openCloudSync: [] }>();

const state = ref<AuraXLabAccountState | null>(null);
const loading = ref(true);
const refreshingProfile = ref(false);
const busy = ref(false);
const message = ref("");
const isError = ref(false);
const bindOnLogin = ref(true);
const deviceLabel = ref("");
const recoveryPassword = ref("");
let refreshGeneration = 0;

const signedIn = computed(() => state.value?.signedIn ?? false);
const consoleReady = computed(() => state.value?.consistency === "consistent");
const needsRecovery = computed(() =>
  state.value?.consistency === "sync_only" || state.value?.consistency === "mismatch",
);

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
  const generation = ++refreshGeneration;
  loading.value = true;
  try {
    state.value = await accountState();
  } catch (error) {
    note(String(error), true);
  } finally {
    loading.value = false;
  }

  if (state.value?.signedIn && generation === refreshGeneration) {
    void refreshProfile(generation);
  }
}

async function refreshProfile(generation = refreshGeneration) {
  if (refreshingProfile.value) return;
  refreshingProfile.value = true;
  try {
    const next = await refreshAccount();
    if (generation === refreshGeneration) {
      state.value = next;
    }
  } catch (error) {
    if (generation === refreshGeneration) {
      note(String(error), true);
    }
  } finally {
    if (generation === refreshGeneration) {
      refreshingProfile.value = false;
    }
  }
}

function onSignedIn(next: AuraXLabAccountState) {
  refreshGeneration += 1;
  refreshingProfile.value = false;
  state.value = next;
  note(t("cloudSync.signedIn"));
}

async function recoverConsole() {
  if (!state.value?.email || !recoveryPassword.value) {
    note(t("account.passwordRequired"), true);
    return;
  }
  refreshGeneration += 1;
  refreshingProfile.value = false;
  busy.value = true;
  try {
    state.value = await enableConsole(
      state.value.email,
      recoveryPassword.value,
      deviceLabel.value || props.platform,
      props.platform,
    );
    note(t("account.bound"));
  } catch (error) {
    note(String(error), true);
  } finally {
    recoveryPassword.value = "";
    busy.value = false;
  }
}

async function pause() {
  busy.value = true;
  try {
    state.value = await pauseConsole();
    note(t("account.offline"));
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

async function signOut() {
  refreshGeneration += 1;
  refreshingProfile.value = false;
  busy.value = true;
  try {
    state.value = await accountLogout();
    note(t("account.signedOut"));
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

onMounted(() => {
  deviceLabel.value = props.platform;
  void refresh();
});
</script>

<template>
  <div class="account-overlay" @click.self="emit('close')">
    <div class="account-dialog" role="dialog" :aria-label="t('account.title')">
      <header class="account-header">
        <div>
          <div class="account-title">{{ t('account.title') }}</div>
          <div class="account-subtitle">{{ t('account.subtitle') }}</div>
        </div>
        <button class="account-close" type="button" :aria-label="t('account.close')" @click="emit('close')">×</button>
      </header>

      <main class="account-body">
        <section class="account-section">
          <h3>{{ t('account.statusSection') }}</h3>
          <p v-if="loading" class="account-hint" role="status">{{ t('account.loading') }}</p>
          <div v-else class="account-badges">
            <span class="account-badge" :data-on="signedIn">
              {{ signedIn ? t('account.statusSignedIn', { name: state?.username || state?.email || '' }) : t('account.statusNotSignedIn') }}
            </span>
            <span class="account-badge" :data-on="consoleReady">
              {{ consoleReady ? t('account.statusBound', { label: state?.console.deviceLabel || '' }) : t('account.statusNotBound') }}
            </span>
          </div>
          <p v-if="state?.consistency === 'device_only'" class="account-warning">
            {{ t('account.deviceOnlyMigration') }}
          </p>
          <p v-else-if="state?.consistency === 'mismatch'" class="account-warning">
            {{ t('account.subjectMismatch') }}
          </p>
        </section>

        <section class="account-section">
          <h3>{{ t('account.accountSection') }}</h3>
          <template v-if="loading">
            <p class="account-hint" role="status">{{ t('account.loading') }}</p>
          </template>
          <template v-else-if="!signedIn">
            <p class="account-hint">{{ t('account.signInIntro') }}</p>
            <AuraxlabAuthForm
              :device-label="deviceLabel || platform"
              :platform="platform"
              :enable-console="bindOnLogin"
              @signed-in="onSignedIn"
            >
              <label class="account-checkbox">
                <input v-model="bindOnLogin" type="checkbox" />
                <span>{{ t('account.bindOnLogin') }}</span>
              </label>
              <label v-if="bindOnLogin" class="account-inline-label">{{ t('account.deviceLabel') }}</label>
              <input v-if="bindOnLogin" v-model="deviceLabel" class="account-input" type="text" />
            </AuraxlabAuthForm>
          </template>

          <template v-else>
            <dl class="account-facts">
              <dt>{{ t('account.signedInAs') }}</dt><dd>{{ state?.username }}</dd>
              <dt>{{ t('account.email') }}</dt><dd>{{ state?.email }}</dd>
              <dt>{{ t('account.deviceId') }}</dt><dd class="account-mono">{{ state?.console.deviceId || '—' }}</dd>
              <dt>{{ t('account.connection') }}</dt>
              <dd>{{ state?.console.connected ? t('account.connected') : t('account.offline') }}</dd>
            </dl>

            <div v-if="needsRecovery" class="account-recovery">
              <p class="account-hint">{{ t('account.rebindHint') }}</p>
              <input v-model="recoveryPassword" class="account-input" type="password"
                :placeholder="t('account.password')" @keyup.enter="recoverConsole" />
              <button class="account-btn primary" type="button" :disabled="busy" @click="recoverConsole">
                {{ t('account.bindNow') }}
              </button>
            </div>

            <h4>{{ t('account.trafficSection') }}</h4>
            <p v-if="refreshingProfile" class="account-hint" role="status">{{ t('account.refreshing') }}</p>
            <dl v-else-if="state?.traffic" class="account-facts">
              <dt>{{ t('account.trafficTotal') }}</dt><dd>{{ formatBytes(state.traffic.bytesTotal) }}</dd>
              <dt>{{ t('account.trafficUp') }}</dt><dd>{{ formatBytes(state.traffic.bytesUp) }}</dd>
              <dt>{{ t('account.trafficDown') }}</dt><dd>{{ formatBytes(state.traffic.bytesDown) }}</dd>
              <dt>{{ t('account.trafficSessions') }}</dt><dd>{{ state.traffic.sessions }}</dd>
            </dl>
            <p v-else class="account-hint">{{ t('account.trafficUnavailable') }}</p>

            <div class="account-actions">
              <button class="account-btn" type="button" :disabled="busy || refreshingProfile" @click="refreshProfile()">{{ t('account.refresh') }}</button>
              <button class="account-btn" type="button" @click="emit('openCloudSync')">{{ t('account.cloudSyncEntry') }}</button>
              <button class="account-btn" type="button" @click="openExternalUrl('https://auraxlab.com/console')">{{ t('account.openConsole') }}</button>
              <button v-if="state?.console.connected" class="account-btn" type="button" :disabled="busy" @click="pause">{{ t('account.pauseConsole') }}</button>
              <button class="account-btn danger" type="button" :disabled="busy" @click="signOut">{{ t('account.signOut') }}</button>
            </div>
          </template>
        </section>

        <p v-if="message" class="account-message" :data-error="isError">{{ message }}</p>
      </main>
    </div>
  </div>
</template>

<style scoped>
.account-overlay { position: fixed; inset: 0; z-index: 1100; display: flex; align-items: center; justify-content: center; background: rgba(0,0,0,.48); }
.account-dialog { width: 620px; max-width: calc(100vw - 32px); max-height: calc(100vh - 48px); overflow: hidden; color: var(--ui-fg,#ddd); background: var(--ui-panel-bg,#1e1e1e); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 10px; box-shadow: 0 18px 48px rgba(0,0,0,.55); }
.account-header { display: flex; justify-content: space-between; align-items: center; padding: 14px 18px; border-bottom: 1px solid var(--ui-border,#3a3a3a); }
.account-title { font-size: 16px; font-weight: 600; }
.account-subtitle,.account-hint { font-size: 12px; opacity: .7; line-height: 1.5; }
.account-close { border: 0; background: transparent; color: inherit; font-size: 22px; cursor: pointer; }
.account-body { padding: 14px 18px; overflow-y: auto; max-height: calc(100vh - 110px); }
.account-section { margin-bottom: 18px; }
.account-section h3 { margin: 0 0 10px; font-size: 13px; text-transform: uppercase; opacity: .8; }
.account-section h4 { margin: 16px 0 8px; font-size: 13px; }
.account-badges,.account-actions { display: flex; gap: 8px; flex-wrap: wrap; }
.account-badge { padding: 3px 9px; border-radius: 999px; font-size: 12px; background: #5c2b2e; }
.account-badge[data-on="true"] { background: #1f4d2e; }
.account-facts { display: grid; grid-template-columns: 110px 1fr; gap: 7px 10px; font-size: 13px; }
.account-facts dt { opacity: .65; }.account-facts dd { margin: 0; overflow-wrap: anywhere; }
.account-checkbox { display: flex; align-items: center; gap: 7px; margin-top: 8px; font-size: 13px; }
.account-inline-label { display: block; margin-top: 8px; font-size: 12px; opacity: .75; }
.account-input { box-sizing: border-box; width: 100%; margin-top: 5px; padding: 7px 9px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; }
.account-recovery { padding: 10px; border: 1px solid #8a6a2f; border-radius: 6px; }
.account-warning { color: #e5c07b; font-size: 12px; }
.account-btn { padding: 7px 12px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.account-btn.primary { margin-top: 8px; color: white; background: var(--ui-accent,#2f6fd0); }.account-btn.danger { color: #ff8b8f; border-color: #7a3338; }
.account-btn:disabled { opacity: .5; }.account-mono { font-family: ui-monospace,Consolas,monospace; font-size: 12px; }
.account-message { color: #7ec699; font-size: 12px; }.account-message[data-error="true"] { color: #ff8b8f; }
</style>
