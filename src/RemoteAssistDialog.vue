<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  assistStatus,
  kickAssistGuest,
  respondAssistJoin,
  setAssistFollowActiveTab,
  setAssistRole,
  startAssist,
  stopAssist,
  type AssistControlPolicy,
  type AssistGuest,
  type AssistProtocol,
  type AssistStatus,
  extendAssist,
} from "./assist";
import { t } from "./i18n";

const props = defineProps<{
  /** Tabs the host may share (id, protocol, title). */
  sessions: { id: string; protocol: AssistProtocol; title: string }[];
  activeSessionId: string | null;
  /** Current status, kept fresh by App.vue; null when no session runs. */
  status: AssistStatus | null;
}>();
const emit = defineEmits<{ close: []; changed: [] }>();

const sessionId = ref<string>(props.activeSessionId ?? props.sessions[0]?.id ?? "");
const controlPolicy = ref<AssistControlPolicy>("on_request");
const approvalRequired = ref(false);
const multiGuest = ref(false);
const joinTtlMinutes = ref(10);
const followActiveTab = ref(false);
const busy = ref(false);
const message = ref("");
const isError = ref(false);
const codeHidden = ref(false);
const copied = ref<"code" | "link" | null>(null);
const now = ref(Math.floor(Date.now() / 1000));
let ticker: ReturnType<typeof setInterval> | null = null;

const running = computed(() => props.status !== null);
const selectedSession = computed(() => props.sessions.find((s) => s.id === sessionId.value) ?? null);
const remainingJoin = computed(() => {
  if (!props.status) return 0;
  return Math.max(0, props.status.joinExpiresAt - now.value);
});
const remainingSession = computed(() => {
  if (!props.status) return 0;
  return Math.max(0, props.status.expiresAt - now.value);
});
const extending = ref(false);

async function handleExtend() {
  if (extending.value) return;
  extending.value = true;
  try {
    await extendAssist();
    note(t("assist.extended"));
    emit("changed");
  } catch (error) {
    note(String(error), true);
  } finally {
    extending.value = false;
  }
}
const maskedCode = computed(() => {
  if (!props.status) return "";
  const code = props.status.code;
  return codeHidden.value ? `${code.slice(0, 4)}-••••-••••` : code;
});
const pendingGuests = computed(() => (props.status?.guests ?? []).filter((g) => g.role === "pending"));
const activeGuests = computed(() => (props.status?.guests ?? []).filter((g) => g.role !== "pending"));

function note(text: string, error = false) {
  message.value = text;
  isError.value = error;
}

function formatClock(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function guestLabel(guest: AssistGuest): string {
  const name = guest.displayName || t("assist.anonymousGuest");
  const client = guest.client === "web" ? "Web" : guest.client === "auraterm" ? "AuraTerm" : "";
  return client ? `${name} (${client})` : name;
}

async function handleStart() {
  const session = selectedSession.value;
  if (!session) {
    note(t("assist.noSession"), true);
    return;
  }
  busy.value = true;
  note("");
  try {
    await startAssist({
      localSessionId: session.id,
      protocol: session.protocol,
      label: session.title,
      controlPolicy: controlPolicy.value,
      approvalRequired: approvalRequired.value,
      singleUse: !multiGuest.value,
      maxGuests: multiGuest.value ? 3 : 1,
      joinTtlSeconds: joinTtlMinutes.value * 60,
      followActiveTab: followActiveTab.value,
    });
    codeHidden.value = false;
    emit("changed");
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

async function handleStop() {
  busy.value = true;
  try {
    await stopAssist("host_ended");
    emit("changed");
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

async function copy(kind: "code" | "link") {
  if (!props.status) return;
  try {
    await navigator.clipboard.writeText(kind === "code" ? props.status.code : props.status.link);
    copied.value = kind;
    setTimeout(() => { if (copied.value === kind) copied.value = null; }, 1500);
  } catch {
    note(t("assist.copyFailed"), true);
  }
}

async function act(fn: () => Promise<void>) {
  busy.value = true;
  try {
    await fn();
    emit("changed");
  } catch (error) {
    note(String(error), true);
  } finally {
    busy.value = false;
  }
}

function approve(guest: AssistGuest, decision: "allow_view" | "allow_control" | "deny") {
  void act(() => respondAssistJoin(guest.connectionId, decision));
}

function grant(guest: AssistGuest) {
  void act(() => setAssistRole(guest.connectionId, "controller"));
}

function revoke(guest: AssistGuest) {
  void act(() => setAssistRole(guest.connectionId, "viewer"));
}

function kick(guest: AssistGuest) {
  void act(() => kickAssistGuest(guest.connectionId));
}

function toggleFollow(value: boolean) {
  followActiveTab.value = value;
  if (props.status) {
    void setAssistFollowActiveTab(value).then(() => emit("changed")).catch(() => {});
  }
}

watch(() => props.status, (status) => {
  if (status) {
    followActiveTab.value = status.followActiveTab;
    // Once somebody is in, hide the code by default against shoulder surfing.
    if (status.guests.length > 0 && !codeHidden.value && status.policy.singleUse) {
      codeHidden.value = true;
    }
  }
}, { immediate: true });

onMounted(() => {
  ticker = setInterval(() => { now.value = Math.floor(Date.now() / 1000); }, 1000);
  void assistStatus().then(() => emit("changed")).catch(() => {});
});
onBeforeUnmount(() => {
  if (ticker) clearInterval(ticker);
});
</script>

<template>
  <div class="assist-overlay" @click.self="emit('close')">
    <div class="assist-dialog" role="dialog" :aria-label="t('assist.title')">
      <header class="assist-header">
        <div>
          <div class="assist-title">{{ t('assist.title') }}</div>
          <div class="assist-subtitle">{{ t('assist.subtitle') }}</div>
        </div>
        <button class="assist-close" type="button" :aria-label="t('common.close')" @click="emit('close')">×</button>
      </header>

      <main class="assist-body">
        <!-- ── not running: configure + mint ───────────────────────────── -->
        <template v-if="!running">
          <section class="assist-section">
            <label class="assist-label">{{ t('assist.sessionLabel') }}</label>
            <select v-model="sessionId" class="assist-input">
              <option v-for="s in sessions" :key="s.id" :value="s.id">{{ s.title }} · {{ s.protocol }}</option>
            </select>
            <label class="assist-checkbox">
              <input type="checkbox" :checked="followActiveTab" @change="toggleFollow(($event.target as HTMLInputElement).checked)">
              <span>{{ t('assist.followActiveTab') }}</span>
            </label>
          </section>

          <section class="assist-section">
            <label class="assist-label">{{ t('assist.controlPolicy') }}</label>
            <div class="assist-radios">
              <label><input v-model="controlPolicy" type="radio" value="view_only"> {{ t('assist.policyViewOnly') }}</label>
              <label><input v-model="controlPolicy" type="radio" value="on_request"> {{ t('assist.policyOnRequest') }}</label>
              <label><input v-model="controlPolicy" type="radio" value="auto_grant"> {{ t('assist.policyAutoGrant') }}</label>
            </div>
            <label class="assist-checkbox">
              <input v-model="approvalRequired" type="checkbox">
              <span>{{ t('assist.approvalRequired') }}</span>
            </label>
            <label class="assist-checkbox">
              <input v-model="multiGuest" type="checkbox">
              <span>{{ t('assist.multiGuest') }}</span>
            </label>
            <label class="assist-label">{{ t('assist.joinWindow') }}</label>
            <select v-model.number="joinTtlMinutes" class="assist-input assist-input--short">
              <option :value="5">5 {{ t('assist.minutes') }}</option>
              <option :value="10">10 {{ t('assist.minutes') }}</option>
              <option :value="30">30 {{ t('assist.minutes') }}</option>
              <option :value="60">60 {{ t('assist.minutes') }}</option>
            </select>
          </section>

          <p class="assist-hint">{{ t('assist.securityHint') }}</p>
          <div class="assist-actions">
            <button class="assist-btn primary" type="button" :disabled="busy || !selectedSession" @click="handleStart">
              {{ busy ? t('assist.starting') : t('assist.generate') }}
            </button>
          </div>
        </template>

        <!-- ── running: code + guests ─────────────────────────────────── -->
        <template v-else-if="status">
          <section class="assist-section assist-code-section">
            <div class="assist-code" :class="{ locked: status.locked }" @click="codeHidden = !codeHidden">{{ maskedCode }}</div>
            <div class="assist-code-meta">
              <span v-if="status.locked" class="assist-warning">{{ t('assist.locked') }}</span>
              <span v-else-if="status.joinOpen">{{ t('assist.joinOpenFor', { time: formatClock(remainingJoin) }) }}</span>
              <span v-else>{{ t('assist.joinClosed') }}</span>
              <span class="assist-dim">· {{ t('assist.sessionLabel') }}: {{ status.label }} ({{ status.protocol }})</span>
            </div>
            <div class="assist-code-meta">
              <span :class="{ 'assist-warning': remainingSession < 600 }">{{ t('assist.sessionEndsIn', { time: formatClock(remainingSession) }) }}</span>
              <button class="assist-btn assist-btn--inline" type="button" :disabled="extending || status.locked" @click="handleExtend">{{ extending ? t('assist.extending') : t('assist.extend') }}</button>
            </div>
            <div class="assist-actions">
              <button class="assist-btn" type="button" @click="copy('code')">{{ copied === 'code' ? t('assist.copied') : t('assist.copyCode') }}</button>
              <button class="assist-btn" type="button" @click="copy('link')">{{ copied === 'link' ? t('assist.copied') : t('assist.copyLink') }}</button>
              <button class="assist-btn" type="button" @click="codeHidden = !codeHidden">{{ codeHidden ? t('assist.showCode') : t('assist.hideCode') }}</button>
              <span class="spacer" />
              <button class="assist-btn danger" type="button" :disabled="busy" @click="handleStop">{{ t('assist.stop') }}</button>
            </div>
            <label class="assist-checkbox">
              <input type="checkbox" :checked="followActiveTab" @change="toggleFollow(($event.target as HTMLInputElement).checked)">
              <span>{{ t('assist.followActiveTab') }}</span>
            </label>
          </section>

          <section v-if="pendingGuests.length" class="assist-section">
            <h3>{{ t('assist.pendingGuests') }}</h3>
            <div v-for="guest in pendingGuests" :key="guest.connectionId" class="assist-guest">
              <div class="assist-guest-info">
                <span class="assist-guest-name">{{ guestLabel(guest) }}</span>
                <span class="assist-mono assist-dim">{{ guest.fingerprint }}</span>
              </div>
              <div class="assist-guest-actions">
                <button class="assist-btn" type="button" :disabled="busy" @click="approve(guest, 'allow_view')">{{ t('assist.allowView') }}</button>
                <button v-if="status.policy.control !== 'view_only'" class="assist-btn" type="button" :disabled="busy" @click="approve(guest, 'allow_control')">{{ t('assist.allowControl') }}</button>
                <button class="assist-btn danger" type="button" :disabled="busy" @click="approve(guest, 'deny')">{{ t('assist.deny') }}</button>
              </div>
            </div>
          </section>

          <section class="assist-section">
            <h3>{{ t('assist.guests') }} ({{ activeGuests.length }})</h3>
            <p v-if="!activeGuests.length" class="assist-hint">{{ t('assist.noGuests') }}</p>
            <div v-for="guest in activeGuests" :key="guest.connectionId" class="assist-guest">
              <div class="assist-guest-info">
                <span class="assist-guest-name">{{ guestLabel(guest) }}</span>
                <span class="assist-badge" :data-role="guest.role">
                  {{ guest.role === 'controller' ? t('assist.roleController') : t('assist.roleViewer') }}
                  <template v-if="guest.role === 'controller' && guest.controlExpiresAt"> · {{ formatClock(Math.max(0, guest.controlExpiresAt - now)) }}</template>
                </span>
                <span v-if="guest.controlRequested" class="assist-warning">{{ t('assist.controlRequested') }}</span>
                <span class="assist-mono assist-dim">{{ guest.fingerprint }}</span>
              </div>
              <div class="assist-guest-actions">
                <button v-if="guest.role === 'viewer' && status.policy.control !== 'view_only'" class="assist-btn" type="button" :disabled="busy" @click="grant(guest)">{{ t('assist.grantControl') }}</button>
                <button v-if="guest.role === 'controller'" class="assist-btn" type="button" :disabled="busy" @click="revoke(guest)">{{ t('assist.revokeControl') }}</button>
                <button class="assist-btn danger" type="button" :disabled="busy" @click="kick(guest)">{{ t('assist.kick') }}</button>
              </div>
            </div>
          </section>
          <p class="assist-hint">{{ t('assist.hotkeyHint') }}</p>
        </template>

        <p v-if="message" class="assist-message" :data-error="isError">{{ message }}</p>
      </main>
    </div>
  </div>
</template>

<style scoped>
.assist-overlay { position: fixed; inset: 0; z-index: 1100; display: flex; align-items: center; justify-content: center; background: rgba(0,0,0,.48); }
.assist-dialog { width: 640px; max-width: calc(100vw - 32px); max-height: calc(100vh - 48px); overflow: hidden; color: var(--ui-fg,#ddd); background: var(--ui-panel-bg,#1e1e1e); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 10px; box-shadow: 0 18px 48px rgba(0,0,0,.55); }
.assist-header { display: flex; justify-content: space-between; align-items: center; padding: 14px 18px; border-bottom: 1px solid var(--ui-border,#3a3a3a); }
.assist-title { font-size: 16px; font-weight: 600; }
.assist-subtitle,.assist-hint { font-size: 12px; opacity: .7; line-height: 1.5; }
.assist-close { border: 0; background: transparent; color: inherit; font-size: 22px; cursor: pointer; }
.assist-body { padding: 14px 18px; overflow-y: auto; max-height: calc(100vh - 110px); }
.assist-section { margin-bottom: 16px; }
.assist-section h3 { margin: 0 0 8px; font-size: 13px; text-transform: uppercase; opacity: .8; }
.assist-label { display: block; margin-top: 8px; font-size: 12px; opacity: .75; }
.assist-input { box-sizing: border-box; width: 100%; margin-top: 5px; padding: 7px 9px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; }
.assist-input--short { width: 180px; }
.assist-radios { display: flex; flex-direction: column; gap: 6px; margin-top: 6px; font-size: 13px; }
.assist-checkbox { display: flex; align-items: center; gap: 7px; margin-top: 8px; font-size: 13px; }
.assist-actions { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin-top: 10px; }
.assist-actions .spacer { flex: 1; }
.assist-btn { padding: 7px 12px; color: inherit; background: var(--ui-input-bg,#26262b); border: 1px solid var(--ui-border,#3a3a3a); border-radius: 6px; cursor: pointer; }
.assist-btn.primary { color: white; background: var(--ui-accent,#2f6fd0); }
.assist-btn.danger { color: #ff8b8f; border-color: #7a3338; }
.assist-btn:disabled { opacity: .5; }
.assist-code-section { text-align: center; }
.assist-code { font-family: ui-monospace,Consolas,monospace; font-size: 34px; letter-spacing: .12em; padding: 14px; border: 1px dashed var(--ui-border,#3a3a3a); border-radius: 8px; cursor: pointer; user-select: all; }
.assist-code.locked { color: #ff8b8f; text-decoration: line-through; }
.assist-code-meta { margin-top: 6px; font-size: 12px; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.assist-btn--inline { padding: 2px 8px; font-size: 12px; }
.assist-dim { opacity: .6; }
.assist-mono { font-family: ui-monospace,Consolas,monospace; font-size: 12px; }
.assist-guest { display: flex; justify-content: space-between; align-items: center; gap: 10px; padding: 8px 0; border-top: 1px solid var(--ui-border,#3a3a3a); }
.assist-guest-info { display: flex; flex-direction: column; gap: 3px; font-size: 13px; }
.assist-guest-name { font-weight: 600; }
.assist-guest-actions { display: flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
.assist-badge { display: inline-block; width: fit-content; padding: 2px 8px; border-radius: 999px; font-size: 11px; background: #2c3e50; }
.assist-badge[data-role="controller"] { background: #7a3338; }
.assist-warning { color: #e5c07b; font-size: 12px; }
.assist-message { color: #7ec699; font-size: 12px; }
.assist-message[data-error="true"] { color: #ff8b8f; }
</style>
