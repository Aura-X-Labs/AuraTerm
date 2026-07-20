<script setup lang="ts">
import { ref } from "vue";
import {
  auraxlabLogin,
  auraxlabRegister,
  auraxlabRequestEmailCode,
  auraxlabVerifyEmailCode,
  validateRegistration,
  DEFAULT_AURAXLAB_URL,
  SYNC_EMAIL_RE,
  SYNC_MIN_PASSWORD_LENGTH,
  type SyncConfigView,
} from "./cloudSync";
import { t } from "./i18n";

/**
 * AuraXLab sign-in / sign-up form (login + verify-first registration).
 * Shared by the Cloud Sync dialog and the account center; on success the
 * backend selects the AuraXLab provider and the updated config view is
 * emitted so the host dialog can re-hydrate its own state.
 *
 * Hosts can chain an extra step onto the same password entry via `postLogin`
 * (the account center uses it to enroll the device for Cloud Console — one
 * sign-in, two credentials). The callback runs after a successful login and
 * must handle its own errors; the password is never stored and is cleared
 * as soon as the chained step finishes.
 */
const props = defineProps<{
  /** Show an editable server URL field above the login form. */
  showServer?: boolean;
  /** Extra step run with the fresh credentials after a successful login. */
  postLogin?: (credentials: { email: string; password: string }) => Promise<void>;
}>();

const emit = defineEmits<{ signedIn: [view: SyncConfigView] }>();

/** Server the form talks to; hosts may bind it (`v-model:server-url`). */
const serverUrl = defineModel<string>("serverUrl", { default: DEFAULT_AURAXLAB_URL });

const email = ref("");
const password = ref("");
const username = ref(""); // registration only
const mode = ref<"login" | "register">("login");
// Sign-up is verify-first: email -> code -> account details.
const regStep = ref<"email" | "code" | "details">("email");
const code = ref("");

const busy = ref(false);
const message = ref("");
const isError = ref(false);

function flash(text: string, error = false) {
  message.value = text;
  isError.value = error;
}

function startRegister() {
  mode.value = "register";
  regStep.value = "email";
  code.value = "";
}

function startLogin() {
  mode.value = "login";
  regStep.value = "email";
  code.value = "";
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

async function signIn() {
  if (!email.value.trim() || !password.value) {
    flash(t("cloudSync.enterEmailPassword"), true);
    return;
  }
  await withBusy(async () => {
    const updated = await auraxlabLogin(serverUrl.value, email.value, password.value);
    const freshPassword = password.value;
    password.value = "";
    flash(t("cloudSync.signedIn"));
    emit("signedIn", updated);
    // Chain the host's extra step (e.g. device binding) onto this same
    // password entry, then drop the secret.
    if (props.postLogin) {
      await props.postLogin({ email: email.value.trim(), password: freshPassword });
    }
  });
}

// Sign-up step 1: email -> request a verification code.
async function sendCode() {
  if (!SYNC_EMAIL_RE.test(email.value.trim())) {
    flash(t("cloudSync.validEmailRequired"), true);
    return;
  }
  await withBusy(async () => {
    const msg = await auraxlabRequestEmailCode(serverUrl.value, email.value.trim());
    code.value = "";
    regStep.value = "code";
    flash(msg);
  });
}

// Sign-up step 2: verify the emailed code.
async function verifyCode() {
  if (!code.value.trim()) {
    flash(t("cloudSync.enterCode"), true);
    return;
  }
  await withBusy(async () => {
    const msg = await auraxlabVerifyEmailCode(
      serverUrl.value,
      email.value.trim(),
      code.value.trim(),
    );
    regStep.value = "details";
    flash(msg);
  });
}

// Sign-up step 3: create the account (email already verified).
async function register() {
  // Validate locally with the same rules the server enforces (immediate
  // feedback; the server still re-checks and owns duplicate detection).
  const validationError = validateRegistration(email.value, username.value, password.value);
  if (validationError) {
    flash(validationError, true);
    return;
  }
  await withBusy(async () => {
    const msg = await auraxlabRegister(serverUrl.value, email.value, username.value, password.value);
    password.value = "";
    startLogin();
    flash(msg);
  });
}
</script>

<template>
  <div class="ax-auth">
    <!-- Sign in -->
    <template v-if="mode === 'login'">
      <p class="ax-hint">{{ t('cloudSync.loginHint') }}</p>
      <template v-if="props.showServer">
        <label>{{ t('account.serverUrl') }}</label>
        <input v-model="serverUrl" class="ax-input" type="url" autocomplete="off" spellcheck="false" />
      </template>
      <label>{{ t('cloudSync.email') }}</label>
      <input v-model="email" class="ax-input" type="email" autocomplete="off" placeholder="you@example.com" />
      <label>{{ t('cloudSync.password') }}</label>
      <input v-model="password" class="ax-input" type="password" autocomplete="off" @keyup.enter="signIn" />
      <!-- Host-provided extras (e.g. the account center's bind-on-login option). -->
      <slot />
      <div class="ax-row">
        <button class="ax-btn primary" type="button" :disabled="busy" @click="signIn">{{ t('cloudSync.signIn') }}</button>
        <button class="ax-btn" type="button" :disabled="busy" @click="startRegister">{{ t('cloudSync.needAccount') }}</button>
      </div>
    </template>

    <!-- Sign up — step 1: email -->
    <template v-else-if="regStep === 'email'">
      <p class="ax-hint">{{ t('cloudSync.registerHint') }}</p>
      <label>{{ t('cloudSync.email') }}</label>
      <input v-model="email" class="ax-input" type="email" autocomplete="off" placeholder="you@example.com" @keyup.enter="sendCode" />
      <div class="ax-row">
        <button class="ax-btn primary" type="button" :disabled="busy" @click="sendCode">{{ t('cloudSync.sendCode') }}</button>
        <button class="ax-btn" type="button" :disabled="busy" @click="startLogin">{{ t('cloudSync.haveAccount') }}</button>
      </div>
    </template>

    <!-- Sign up — step 2: verify code -->
    <template v-else-if="regStep === 'code'">
      <p class="ax-hint">{{ t('cloudSync.codeHint', { email }) }}</p>
      <label>{{ t('cloudSync.verificationCode') }}</label>
      <input v-model="code" class="ax-input" type="text" inputmode="numeric" maxlength="6"
        autocomplete="one-time-code" placeholder="••••••" @keyup.enter="verifyCode" />
      <div class="ax-row">
        <button class="ax-btn primary" type="button" :disabled="busy" @click="verifyCode">{{ t('cloudSync.verify') }}</button>
        <button class="ax-btn" type="button" :disabled="busy" @click="sendCode">{{ t('cloudSync.resend') }}</button>
        <button class="ax-btn" type="button" :disabled="busy" @click="regStep = 'email'">{{ t('cloudSync.changeEmail') }}</button>
      </div>
    </template>

    <!-- Sign up — step 3: account details -->
    <template v-else>
      <p class="ax-hint">{{ t('cloudSync.verifiedHint', { email }) }}</p>
      <label>{{ t('cloudSync.username') }}</label>
      <input v-model="username" class="ax-input" type="text" autocomplete="off" />
      <label>{{ t('cloudSync.password') }} <span class="ax-muted">{{ t('cloudSync.passwordMinChars', { n: SYNC_MIN_PASSWORD_LENGTH }) }}</span></label>
      <input v-model="password" class="ax-input" type="password" autocomplete="new-password" @keyup.enter="register" />
      <div class="ax-row">
        <button class="ax-btn primary" type="button" :disabled="busy" @click="register">{{ t('cloudSync.createAccount') }}</button>
        <button class="ax-btn" type="button" :disabled="busy" @click="regStep = 'code'">{{ t('cloudSync.back') }}</button>
      </div>
    </template>

    <div v-if="message" class="ax-message" :class="{ error: isError }">{{ message }}</div>
  </div>
</template>

<style scoped>
.ax-auth {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.ax-auth label {
  font-size: 12px;
  opacity: 0.8;
  margin-top: 6px;
}
.ax-hint {
  font-size: 12px;
  opacity: 0.7;
  margin: 0 0 8px;
  line-height: 1.5;
}
.ax-muted {
  opacity: 0.55;
  font-weight: 400;
}
.ax-input {
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
.ax-input:focus {
  outline: none;
  border-color: var(--ui-accent, #4a90d9);
}
.ax-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 10px;
}
.ax-btn {
  padding: 7px 14px;
  background: var(--ui-input-bg, #2a2a2a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
}
.ax-btn:hover:not(:disabled) {
  border-color: var(--ui-accent, #4a90d9);
}
.ax-btn.primary {
  background: var(--ui-accent, #4a90d9);
  border-color: var(--ui-accent, #4a90d9);
  color: #fff;
}
.ax-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.ax-message {
  font-size: 12px;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(120, 200, 120, 0.12);
  border: 1px solid rgba(120, 200, 120, 0.3);
  margin-top: 8px;
  word-break: break-word;
}
.ax-message.error {
  background: rgba(200, 80, 80, 0.12);
  border-color: rgba(200, 80, 80, 0.3);
  color: #e06c75;
}
</style>
