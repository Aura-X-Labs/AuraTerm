<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { ref, computed, watch } from "vue";
import { LANGUAGE_OPTIONS, t } from "./i18n";
import {
  cloneTerminalTheme,
  DEFAULT_AI_MODELS,
  getMatchingTerminalThemePreset,
  getTerminalThemePreset,
  normalizeAppSettings,
  TERMINAL_THEME_PRESETS,
  type AiConfig,
  type AiProvider,
  type AppSettings,
  type OutputRule,
  type RendererMode,
  type TerminalTheme,
  type UiThemeMode,
} from "./settings";
import { aiClearApiKey, aiHasApiKey, aiSetApiKey, aiTestConnection } from "./ai";

const props = defineProps<{
  initial: AppSettings;
}>();

const emit = defineEmits<{
  save: [settings: AppSettings];
  cancel: [];
}>();

const settings = ref<AppSettings>(normalizeAppSettings(props.initial));

const activeTab = ref<"terminal" | "keyboard" | "theme" | "automation" | "ai" | "security">("terminal");

type TrustedSshHostKeyEntry = {
  host: string;
  port: number;
  fingerprint: string;
  fingerprintSummary: string;
};

type ShellPresetValue = "auto" | "git-bash" | "powershell" | "cmd" | "custom";
type ThemeColorKey = keyof TerminalTheme;

// Shell preset options (computed so labels follow the active locale).
const shellPresets = computed<Array<{ value: ShellPresetValue; label: string }>>(() => [
  { value: "auto", label: t("settings.shellPresets.auto") },
  { value: "git-bash", label: t("settings.shellPresets.gitBash") },
  { value: "powershell", label: t("settings.shellPresets.powershell") },
  { value: "cmd", label: t("settings.shellPresets.cmd") },
  { value: "custom", label: t("settings.shellPresets.custom") },
]);

const themePresets = TERMINAL_THEME_PRESETS;
const uiThemeModeOptions = computed<Array<{ value: UiThemeMode; label: string }>>(() => [
  { value: "follow-terminal", label: t("settings.uiThemeModes.followTerminal") },
  { value: "light", label: t("settings.uiThemeModes.light") },
  { value: "dark", label: t("settings.uiThemeModes.dark") },
]);
const rendererModeOptions = computed<Array<{ value: RendererMode; label: string }>>(() => [
  { value: "auto", label: t("settings.renderers.auto") },
  { value: "webgl", label: t("settings.renderers.webgl") },
  { value: "dom", label: t("settings.renderers.dom") },
]);
const basicThemeFields = computed<Array<{ key: ThemeColorKey; label: string }>>(() => [
  { key: "background", label: t("settings.colors.background") },
  { key: "foreground", label: t("settings.colors.foreground") },
  { key: "cursor", label: t("settings.colors.cursor") },
  { key: "selectionBackground", label: t("settings.colors.selection") },
]);
const ansiThemeFields = computed<Array<{ key: ThemeColorKey; label: string }>>(() => [
  { key: "black", label: t("settings.colors.black") },
  { key: "red", label: t("settings.colors.red") },
  { key: "green", label: t("settings.colors.green") },
  { key: "yellow", label: t("settings.colors.yellow") },
  { key: "blue", label: t("settings.colors.blue") },
  { key: "magenta", label: t("settings.colors.magenta") },
  { key: "cyan", label: t("settings.colors.cyan") },
  { key: "white", label: t("settings.colors.white") },
]);
const brightAnsiThemeFields = computed<Array<{ key: ThemeColorKey; label: string }>>(() => [
  { key: "brightBlack", label: t("settings.colors.brightBlack") },
  { key: "brightRed", label: t("settings.colors.brightRed") },
  { key: "brightGreen", label: t("settings.colors.brightGreen") },
  { key: "brightYellow", label: t("settings.colors.brightYellow") },
  { key: "brightBlue", label: t("settings.colors.brightBlue") },
  { key: "brightMagenta", label: t("settings.colors.brightMagenta") },
  { key: "brightCyan", label: t("settings.colors.brightCyan") },
  { key: "brightWhite", label: t("settings.colors.brightWhite") },
]);

// Determine which preset is selected based on shellPath
const selectedShellPreset = computed<ShellPresetValue>({
  get: () => {
    const path = settings.value.shellPath;
    if (!path) return "auto";
    if (path.includes("Git\\bin\\bash.exe") || path.includes("Git/bin/bash.exe")) return "git-bash";
    if (path.toLowerCase().includes("powershell")) return "powershell";
    if (path.toLowerCase().includes("cmd.exe")) return "cmd";
    return "custom";
  },
  set: (value: ShellPresetValue) => {
    switch (value) {
      case "auto":
        settings.value = { ...settings.value, shellPath: null };
        break;
      case "git-bash":
        settings.value = { ...settings.value, shellPath: "C:\\Program Files\\Git\\bin\\bash.exe" };
        break;
      case "powershell":
        settings.value = { ...settings.value, shellPath: "powershell.exe" };
        break;
      case "cmd":
        settings.value = { ...settings.value, shellPath: "cmd.exe" };
        break;
      case "custom":
        // Keep current custom path or set empty
        if (!settings.value.shellPath || 
            settings.value.shellPath.includes("Git") || 
            settings.value.shellPath.toLowerCase().includes("powershell") ||
            settings.value.shellPath.toLowerCase().includes("cmd.exe")) {
          settings.value = { ...settings.value, shellPath: "" };
        }
        break;
    }
  },
});

const showCustomPath = computed(() => selectedShellPreset.value === "custom");
const selectedThemePresetId = computed(() => getMatchingTerminalThemePreset(settings.value.theme)?.id ?? "custom");
const selectedThemePresetDescription = computed(() => {
  const matchedPreset = getMatchingTerminalThemePreset(settings.value.theme);
  return matchedPreset?.description ?? t("settings.customPaletteDesc");
});
const selectedUiThemeModeDescription = computed(() => {
  switch (settings.value.uiThemeMode) {
    case "light":
      return t("settings.uiThemeModeDesc.light");
    case "dark":
      return t("settings.uiThemeModeDesc.dark");
    default:
      return t("settings.uiThemeModeDesc.follow");
  }
});

const selectedRendererModeDescription = computed(() => {
  switch (settings.value.rendererMode) {
    case "dom":
      return t("settings.rendererDesc.dom");
    case "webgl":
      return t("settings.rendererDesc.webgl");
    default:
      return t("settings.rendererDesc.auto");
  }
});

function update<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
  settings.value = { ...settings.value, [key]: value };
}

// ---- AI assistant ----------------------------------------------------------
const aiProviderOptions = computed<Array<{ value: AiProvider; label: string }>>(() => [
  { value: "anthropic", label: t("settings.ai.providerAnthropic") },
  { value: "openai-compatible", label: t("settings.ai.providerOpenAi") },
]);
// The key is write-only from the UI: we only ever learn whether one is stored.
const aiHasKey = ref(false);
const aiKeyInput = ref("");
const aiKeyBusy = ref(false);
const aiTesting = ref(false);
const aiTestResult = ref<{ ok: boolean; message: string } | null>(null);

void aiHasApiKey().then((has) => { aiHasKey.value = has; }).catch(() => {});

function updateAi<K extends keyof AiConfig>(key: K, value: AiConfig[K]) {
  update("aiConfig", { ...settings.value.aiConfig, [key]: value });
}

function onAiProviderChange(provider: AiProvider) {
  // Swapping providers with an empty/default model resets to that provider's
  // default so the field is never left pointing at a foreign model id.
  const current = settings.value.aiConfig;
  const model = !current.model.trim() || current.model === DEFAULT_AI_MODELS[current.provider]
    ? DEFAULT_AI_MODELS[provider]
    : current.model;
  update("aiConfig", { ...current, provider, model });
}

async function saveAiKey() {
  const key = aiKeyInput.value.trim();
  if (!key) return;
  aiKeyBusy.value = true;
  aiTestResult.value = null;
  try {
    await aiSetApiKey(key);
    aiKeyInput.value = "";
    aiHasKey.value = true;
  } catch (error) {
    aiTestResult.value = { ok: false, message: String(error) };
  } finally {
    aiKeyBusy.value = false;
  }
}

async function clearAiKey() {
  aiKeyBusy.value = true;
  try {
    await aiClearApiKey();
    aiHasKey.value = false;
    aiTestResult.value = null;
  } catch (error) {
    aiTestResult.value = { ok: false, message: String(error) };
  } finally {
    aiKeyBusy.value = false;
  }
}

// Why the test button is disabled, so the user isn't left clicking a dead
// button with no feedback. Empty string = enabled.
const aiTestBlockedReason = computed(() => {
  if (!settings.value.aiConfig.enabled) return t("settings.ai.testNeedsEnable");
  if (!aiHasKey.value) return t("settings.ai.testNeedsKey");
  return "";
});

async function testAiConnection() {
  aiTesting.value = true;
  aiTestResult.value = null;
  try {
    // Persist current settings first (without closing the dialog) so the
    // backend probe reads the config as shown.
    await invoke("save_settings", { settings: normalizeAppSettings(settings.value) });
    await aiTestConnection();
    aiTestResult.value = { ok: true, message: t("settings.ai.testOk") };
  } catch (error) {
    aiTestResult.value = { ok: false, message: String(error) };
  } finally {
    aiTesting.value = false;
  }
}

function updateTheme<K extends keyof AppSettings["theme"]>(key: K, value: AppSettings["theme"][K]) {
  settings.value = {
    ...settings.value,
    theme: {
      ...settings.value.theme,
      [key]: value,
    },
  };
}

function inputValue(event: Event) {
  return (event.target as HTMLInputElement).value;
}

function inputChecked(event: Event) {
  return (event.target as HTMLInputElement).checked;
}

function addOutputRule() {
  const next: OutputRule = {
    id: crypto.randomUUID(),
    name: "New rule",
    enabled: true,
    pattern: "ERROR",
    isRegex: false,
    caseSensitive: false,
    scope: "global",
    hosts: [],
    foreground: "#ff6b6b",
    bell: false,
    notify: false,
    cooldownMs: 1000,
  };
  update("outputRules", [...settings.value.outputRules, next]);
}

function updateOutputRule<K extends keyof OutputRule>(id: string, key: K, value: OutputRule[K]) {
  update("outputRules", settings.value.outputRules.map((rule) => (
    rule.id === id ? { ...rule, [key]: value } : rule
  )));
}

function removeOutputRule(id: string) {
  update("outputRules", settings.value.outputRules.filter((rule) => rule.id !== id));
}

function parseHostPatterns(value: string) {
  return value.split(",").map((host) => host.trim()).filter(Boolean);
}

function outputRulePatternError(rule: OutputRule) {
  if (!rule.pattern.trim()) return "Pattern is required.";
  if (!rule.isRegex) return "";
  try {
    new RegExp(rule.pattern);
    return "";
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

async function enableDesktopNotifications() {
  if ("Notification" in window) {
    await Notification.requestPermission();
  }
}

function handleShellPresetChange(event: Event) {
  selectedShellPreset.value = inputValue(event) as ShellPresetValue;
}

function handleThemePresetChange(event: Event) {
  const presetId = inputValue(event);
  if (presetId === "custom") {
    return;
  }

  const preset = getTerminalThemePreset(presetId);
  if (!preset) {
    return;
  }

  update("theme", cloneTerminalTheme(preset.theme));
}

function handleUiThemeModeChange(event: Event) {
  update("uiThemeMode", inputValue(event) as UiThemeMode);
}

function handleRendererModeChange(event: Event) {
  update("rendererMode", inputValue(event) as RendererMode);
}

function resetThemeToDefault() {
  const defaultPreset = getTerminalThemePreset("aura-dark");
  if (!defaultPreset) {
    return;
  }

  update("theme", cloneTerminalTheme(defaultPreset.theme));
}

const trustedHostKeys = ref<TrustedSshHostKeyEntry[]>([]);
const trustedHostKeysLoading = ref(false);
const trustedHostKeysError = ref("");
const deletingHostKeyScope = ref<string | null>(null);
const resettingHostKeys = ref(false);

function trustedHostScope(entry: TrustedSshHostKeyEntry) {
  return `${entry.host}:${entry.port}`;
}

async function refreshTrustedHostKeys() {
  trustedHostKeysLoading.value = true;
  trustedHostKeysError.value = "";
  try {
    const entries = await invoke<TrustedSshHostKeyEntry[]>("ssh_list_known_hosts");
    trustedHostKeys.value = entries;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    trustedHostKeysError.value = `Failed to load trusted host keys: ${message}`;
  } finally {
    trustedHostKeysLoading.value = false;
  }
}

async function removeTrustedHostKey(entry: TrustedSshHostKeyEntry) {
  const scope = trustedHostScope(entry);
  deletingHostKeyScope.value = scope;
  trustedHostKeysError.value = "";
  try {
    await invoke("ssh_delete_known_host", {
      host: entry.host,
      port: entry.port,
    });
    await refreshTrustedHostKeys();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    trustedHostKeysError.value = `Failed to remove trusted host key: ${message}`;
  } finally {
    deletingHostKeyScope.value = null;
  }
}

async function resetTrustedHostKeys() {
  const confirmed = window.confirm(t("settings.security.confirmResetHostKeys"));
  if (!confirmed) {
    return;
  }

  resettingHostKeys.value = true;
  trustedHostKeysError.value = "";
  try {
    await invoke("ssh_reset_known_hosts");
    await refreshTrustedHostKeys();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    trustedHostKeysError.value = `Failed to reset trusted host keys: ${message}`;
  } finally {
    resettingHostKeys.value = false;
  }
}

watch(
  activeTab,
  (tab) => {
    if (tab === "security") {
      void refreshTrustedHostKeys();
      void refreshCredentialState();
    }
  },
  { immediate: false },
);

// ---------- Master password / credential backup ----------

const exportPassword = ref("");
const exportPasswordConfirm = ref("");
const exportLoading = ref(false);
const exportError = ref("");
const exportSuccess = ref("");

const importPassword = ref("");
const importLoading = ref(false);
const importError = ref("");
const importSuccess = ref("");
const importFileInput = ref<HTMLInputElement | null>(null);
const pendingImportContent = ref<string>("");
const pendingImportFileName = ref<string>("");

const lockingMasterPassword = ref(false);

function resetExportForm() {
  exportPassword.value = "";
  exportPasswordConfirm.value = "";
  exportError.value = "";
}

function resetImportForm() {
  importPassword.value = "";
  importError.value = "";
  pendingImportContent.value = "";
  pendingImportFileName.value = "";
  if (importFileInput.value) {
    importFileInput.value.value = "";
  }
}

async function exportCredentials() {
  exportError.value = "";
  exportSuccess.value = "";

  if (exportPassword.value.length < 8) {
    exportError.value = "Backup password must be at least 8 characters.";
    return;
  }
  if (exportPassword.value !== exportPasswordConfirm.value) {
    exportError.value = "Backup passwords do not match.";
    return;
  }

  exportLoading.value = true;
  try {
    const backupContent = await invoke<string>("export_connections", {
      password: exportPassword.value,
    });

    // 触发浏览器下载
    const blob = new Blob([backupContent], { type: "application/octet-stream" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
    link.href = url;
    link.download = `auraterm-credentials-${timestamp}.aurabak`;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
    URL.revokeObjectURL(url);

    exportSuccess.value = "Backup downloaded successfully.";
    resetExportForm();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    exportError.value = `Export failed: ${message}`;
  } finally {
    exportLoading.value = false;
  }
}

function onImportFileSelected(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) {
    pendingImportContent.value = "";
    pendingImportFileName.value = "";
    return;
  }

  importError.value = "";
  importSuccess.value = "";
  pendingImportFileName.value = file.name;

  const reader = new FileReader();
  reader.onload = () => {
    pendingImportContent.value = String(reader.result ?? "");
  };
  reader.onerror = () => {
    importError.value = "Failed to read selected file.";
    pendingImportContent.value = "";
  };
  reader.readAsText(file);
}

async function importCredentials() {
  importError.value = "";
  importSuccess.value = "";

  if (!pendingImportContent.value) {
    importError.value = "Please select a backup file first.";
    return;
  }
  if (!importPassword.value) {
    importError.value = "Backup password is required.";
    return;
  }

  const confirmed = window.confirm(
    "Importing will overwrite existing connection credentials with the same IDs. Continue?",
  );
  if (!confirmed) {
    return;
  }

  importLoading.value = true;
  try {
    await invoke("import_connections", {
      encryptedData: pendingImportContent.value,
      password: importPassword.value,
    });
    importSuccess.value = "Credentials imported successfully.";
    resetImportForm();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    importError.value = `Import failed: ${message}`;
  } finally {
    importLoading.value = false;
  }
}

async function lockMasterPasswordNow() {
  const confirmed = window.confirm(
    "Lock the master password now? You'll need to re-enter it before accessing saved credentials.",
  );
  if (!confirmed) {
    return;
  }
  lockingMasterPassword.value = true;
  try {
    await invoke("lock_master_password");
    window.alert(t("settings.security.masterPasswordLocked"));
    window.location.reload();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    window.alert(t("settings.security.lockFailed", { error: message }));
  } finally {
    lockingMasterPassword.value = false;
  }
}

// ---------- Master password mode (optional password / keychain remember) ----------

interface CredentialSecurityState {
  passwordEnabled: boolean;
  unlocked: boolean;
  rememberEnabled: boolean;
  rememberAvailable: boolean;
}

const credentialState = ref<CredentialSecurityState>({
  passwordEnabled: false,
  unlocked: false,
  rememberEnabled: false,
  rememberAvailable: false,
});
const mpBusy = ref(false);
const mpError = ref("");
const mpSuccess = ref("");
const enableNewPassword = ref("");
const enableConfirm = ref("");
const enableRemember = ref(false);
const changeCurrent = ref("");
const changeNew = ref("");
const changeConfirm = ref("");
const disableCurrent = ref("");

function errMsg(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

async function refreshCredentialState() {
  try {
    credentialState.value = await invoke<CredentialSecurityState>("get_credential_security_state");
  } catch (error) {
    console.error("Failed to load credential security state", error);
  }
}

async function enableMasterPassword() {
  mpError.value = "";
  mpSuccess.value = "";
  if (enableNewPassword.value.length < 8) {
    mpError.value = "Password must be at least 8 characters.";
    return;
  }
  if (enableNewPassword.value !== enableConfirm.value) {
    mpError.value = "Passwords do not match.";
    return;
  }
  mpBusy.value = true;
  try {
    await invoke("set_master_password", {
      password: enableNewPassword.value,
      remember: enableRemember.value && credentialState.value.rememberAvailable,
    });
    enableNewPassword.value = "";
    enableConfirm.value = "";
    enableRemember.value = false;
    mpSuccess.value = "Master password enabled.";
    await refreshCredentialState();
  } catch (error) {
    mpError.value = `Failed: ${errMsg(error)}`;
  } finally {
    mpBusy.value = false;
  }
}

async function changeMasterPassword() {
  mpError.value = "";
  mpSuccess.value = "";
  if (changeNew.value.length < 8) {
    mpError.value = "New password must be at least 8 characters.";
    return;
  }
  if (changeNew.value !== changeConfirm.value) {
    mpError.value = "New passwords do not match.";
    return;
  }
  mpBusy.value = true;
  try {
    await invoke("change_master_password", {
      currentPassword: changeCurrent.value,
      newPassword: changeNew.value,
      remember: credentialState.value.rememberEnabled && credentialState.value.rememberAvailable,
    });
    changeCurrent.value = "";
    changeNew.value = "";
    changeConfirm.value = "";
    mpSuccess.value = "Master password changed.";
    await refreshCredentialState();
  } catch (error) {
    mpError.value = `Failed: ${errMsg(error)}`;
  } finally {
    mpBusy.value = false;
  }
}

async function disableMasterPassword() {
  mpError.value = "";
  mpSuccess.value = "";
  const confirmed = window.confirm(
    "Remove the master password? Credentials will be re-encrypted with a device-local key and you'll no longer be prompted on startup.",
  );
  if (!confirmed) {
    return;
  }
  mpBusy.value = true;
  try {
    await invoke("disable_master_password", { currentPassword: disableCurrent.value });
    disableCurrent.value = "";
    mpSuccess.value = "Master password removed. Credentials are now protected by a device-local key.";
    await refreshCredentialState();
  } catch (error) {
    mpError.value = `Failed: ${errMsg(error)}`;
  } finally {
    mpBusy.value = false;
  }
}

async function toggleRememberMasterPassword(enabled: boolean) {
  mpError.value = "";
  mpSuccess.value = "";
  mpBusy.value = true;
  try {
    await invoke("set_remember_master_password", { enabled });
  } catch (error) {
    mpError.value = `Failed: ${errMsg(error)}`;
  } finally {
    await refreshCredentialState();
    mpBusy.value = false;
  }
}
</script>

<template>
  <div class="settings-overlay" @click="emit('cancel')">
    <div class="settings-dialog" @click.stop>
      <div class="settings-header">
        <h2>{{ $t('settings.title') }}</h2>
        <button class="settings-close-btn" type="button" @click="emit('cancel')">×</button>
      </div>

      <div class="settings-tabs">
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'terminal' }]"
          type="button"
          @click="activeTab = 'terminal'"
        >{{ $t('settings.tabs.terminal') }}</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'keyboard' }]"
          type="button"
          @click="activeTab = 'keyboard'"
        >{{ $t('settings.tabs.keyboard') }}</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'theme' }]"
          type="button"
          @click="activeTab = 'theme'"
        >{{ $t('settings.tabs.theme') }}</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'automation' }]"
          type="button"
          @click="activeTab = 'automation'"
        >{{ $t('settings.tabs.rules') }}</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'ai' }]"
          type="button"
          @click="activeTab = 'ai'"
        >{{ $t('settings.tabs.ai') }}</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'security' }]"
          type="button"
          @click="activeTab = 'security'"
        >{{ $t('settings.tabs.security') }}</button>
      </div>

      <div class="settings-body">
        <!-- Terminal Tab -->
        <div v-show="activeTab === 'terminal'" class="settings-section">
          <label class="settings-field settings-field--stacked">
            <span>{{ $t('language.label') }}</span>
            <select :value="settings.language" @change="update('language', inputValue($event) as AppSettings['language'])">
              <option v-for="option in LANGUAGE_OPTIONS" :key="option.value" :value="option.value">
                {{ option.value === 'system' ? $t('language.system') : option.nativeLabel }}
              </option>
            </select>
          </label>
          <div class="settings-field-full-hint">{{ $t('language.hint') }}</div>

          <label class="settings-field">
            <span>{{ $t('settings.fontSize') }}</span>
            <input
              type="number"
              min="8"
              max="72"
              :value="settings.fontSize"
              @input="update('fontSize', Number(inputValue($event)))"
            >
          </label>

          <label class="settings-field">
            <span>{{ $t('settings.fontFamily') }}</span>
            <input
              type="text"
              :value="settings.fontFamily"
              @input="update('fontFamily', inputValue($event))"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
          </label>

          <label class="settings-field">
            <span>{{ $t('settings.scrollbackLines') }}</span>
            <input
              type="number"
              min="100"
              max="100000"
              step="100"
              :value="settings.scrollback"
              @input="update('scrollback', Number(inputValue($event)))"
            >
          </label>

          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.terminalRenderer') }}</span>
            <select :value="settings.rendererMode" @change="handleRendererModeChange">
              <option v-for="option in rendererModeOptions" :key="option.value" :value="option.value">
                {{ option.label }}
              </option>
            </select>
          </label>
          <div class="settings-field-full-hint">{{ selectedRendererModeDescription }}</div>

          <label class="settings-field">
            <span>{{ $t('settings.shell') }}</span>
            <select :value="selectedShellPreset" @change="handleShellPresetChange">
              <option v-for="preset in shellPresets" :key="preset.value" :value="preset.value">
                {{ preset.label }}
              </option>
            </select>
          </label>

          <label v-if="showCustomPath" class="settings-field">
            <span>{{ $t('settings.customShellPath') }}</span>
            <input
              type="text"
              placeholder="e.g., C:\Program Files\Git\bin\bash.exe"
              :value="settings.shellPath ?? ''"
              @input="update('shellPath', inputValue($event) || null)"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
          </label>

          <label class="settings-field">
            <span>{{ $t('settings.defaultLogSavePath') }}</span>
            <input
              type="text"
              placeholder="e.g., ~/AuraTerm/logs"
              :value="settings.logSavePath"
              @input="update('logSavePath', inputValue($event))"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
          </label>

          <label class="settings-field">
            <span>{{ $t('settings.defaultLogFilenameTemplate') }}</span>
            <input
              type="text"
              placeholder="e.g., {timestamp}_{session}"
              :value="settings.logFileNameTemplate"
              @input="update('logFileNameTemplate', inputValue($event))"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
          </label>
          <div class="settings-field-full-hint">{{ $t('settings.logPlaceholders') }}</div>



          <label class="settings-field settings-field--toggle">
            <span>
              <strong>{{ $t('settings.showInputBar') }}</strong>
              <small>{{ $t('settings.showInputBarHint') }}</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.showInputBar"
              @change="update('showInputBar', inputChecked($event))"
            >
          </label>

          <label class="settings-field settings-field--full">
            <span>{{ $t('settings.zmodemDownloadDir') }}</span>
            <input
              type="text"
              :value="settings.zmodemDownloadPath"
              placeholder="~/AuraTerm/downloads"
              @input="update('zmodemDownloadPath', inputValue($event))"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
            <small>{{ $t('settings.zmodemDownloadHint') }}</small>
          </label>

          <label class="settings-field settings-field--toggle">
            <span>
              <strong>{{ $t('settings.openSftpAfterSsh') }}</strong>
              <small>{{ $t('settings.openSftpAfterSshHint') }}</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.autoOpenSftp"
              @change="update('autoOpenSftp', inputChecked($event))"
            >
          </label>

          <label class="settings-field settings-field--toggle">
            <span>
              <strong>{{ $t('settings.restoreTabs') }}</strong>
              <small>{{ $t('settings.restoreTabsHint') }}</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.restoreTabsOnStartup"
              @change="update('restoreTabsOnStartup', inputChecked($event))"
            >
          </label>
        </div>

        <!-- Keyboard & Mouse Tab -->
        <div v-show="activeTab === 'keyboard'" class="settings-section">
          <label class="settings-field settings-field--toggle">
            <span>
              <strong>{{ $t('settings.copyOnSelect') }}</strong>
              <small>{{ $t('settings.copyOnSelectHint') }}</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.ctrlCCopy"
              @change="update('ctrlCCopy', inputChecked($event))"
            >
          </label>

          <label class="settings-field settings-field--toggle">
            <span>
              <strong>{{ $t('settings.ctrlVPaste') }}</strong>
              <small>{{ $t('settings.ctrlVPasteHint') }}</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.ctrlVPaste"
              @change="update('ctrlVPaste', inputChecked($event))"
            >
          </label>

          <label class="settings-field settings-field--toggle">
            <span>
              <strong>{{ $t('settings.middleClickPaste') }}</strong>
              <small>{{ $t('settings.middleClickPasteHint') }}</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.middleClickPaste"
              @change="update('middleClickPaste', inputChecked($event))"
            >
          </label>
        </div>

        <!-- Theme Tab -->
        <div v-show="activeTab === 'theme'" class="settings-section">
          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.uiStyle') }}</span>
            <div class="settings-theme-preset-row">
              <select :value="settings.uiThemeMode" @change="handleUiThemeModeChange">
                <option v-for="option in uiThemeModeOptions" :key="option.value" :value="option.value">
                  {{ option.label }}
                </option>
              </select>
            </div>
          </label>

          <div class="settings-field-full-hint">{{ selectedUiThemeModeDescription }}</div>

          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.terminalPreset') }}</span>
            <div class="settings-theme-preset-row">
              <select :value="selectedThemePresetId" @change="handleThemePresetChange">
                <option v-for="preset in themePresets" :key="preset.id" :value="preset.id">
                  {{ preset.label }}
                </option>
                <option value="custom">{{ $t('settings.custom') }}</option>
              </select>
              <button class="settings-btn-secondary" type="button" @click="resetThemeToDefault">{{ $t('common.reset') }}</button>
            </div>
          </label>

          <div class="settings-field-full-hint">{{ selectedThemePresetDescription }}</div>

          <div
            class="settings-theme-preview"
            :style="{
              background: settings.theme.background,
              color: settings.theme.foreground,
              borderColor: settings.theme.brightBlack,
            }"
          >
            <div class="settings-theme-preview-title-row">
              <strong>{{ $t('settings.preview') }}</strong>
              <span>{{ selectedThemePresetId === 'custom' ? $t('settings.customPalette') : $t('settings.presetPalette') }}</span>
            </div>
            <div class="settings-theme-preview-line">
              <span :style="{ color: settings.theme.green }">user@auraterm</span>
              <span :style="{ color: settings.theme.foreground }">:</span>
              <span :style="{ color: settings.theme.blue }">~/workspace</span>
              <span :style="{ color: settings.theme.foreground }">$ git status</span>
            </div>
            <div class="settings-theme-preview-line">
              <span :style="{ color: settings.theme.red }">error:</span>
              <span :style="{ color: settings.theme.yellow }">modified file</span>
              <span :style="{ color: settings.theme.cyan }">src/TerminalComponent.vue</span>
            </div>
            <div class="settings-theme-swatches">
              <span class="settings-theme-swatch" :style="{ background: settings.theme.red }" :title="$t('settings.colors.red')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.green }" :title="$t('settings.colors.green')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.yellow }" :title="$t('settings.colors.yellow')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.blue }" :title="$t('settings.colors.blue')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.magenta }" :title="$t('settings.colors.magenta')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.cyan }" :title="$t('settings.colors.cyan')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.brightRed }" :title="$t('settings.colors.brightRed')"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.brightBlue }" :title="$t('settings.colors.brightBlue')"></span>
            </div>
          </div>

          <div class="settings-theme-subsection">
            <div class="settings-theme-subtitle">{{ $t('settings.coreColors') }}</div>
            <div class="settings-theme-grid">
              <label v-for="field in basicThemeFields" :key="field.key" class="settings-theme-grid-item">
                <span>{{ field.label }}</span>
                <div class="settings-color-row">
                  <input type="color" :value="settings.theme[field.key]" @input="updateTheme(field.key, inputValue($event))">
                  <input
                    type="text"
                    :value="settings.theme[field.key]"
                    @input="updateTheme(field.key, inputValue($event))"
                    autocapitalize="none"
                    autocorrect="off"
                    spellcheck="false"
                  >
                </div>
              </label>
            </div>
          </div>

          <div class="settings-theme-subsection">
            <div class="settings-theme-subtitle">{{ $t('settings.ansiColors') }}</div>
            <div class="settings-theme-grid">
              <label v-for="field in ansiThemeFields" :key="field.key" class="settings-theme-grid-item">
                <span>{{ field.label }}</span>
                <div class="settings-color-row">
                  <input type="color" :value="settings.theme[field.key]" @input="updateTheme(field.key, inputValue($event))">
                  <input
                    type="text"
                    :value="settings.theme[field.key]"
                    @input="updateTheme(field.key, inputValue($event))"
                    autocapitalize="none"
                    autocorrect="off"
                    spellcheck="false"
                  >
                </div>
              </label>
            </div>
          </div>

          <div class="settings-theme-subsection">
            <div class="settings-theme-subtitle">{{ $t('settings.brightAnsiColors') }}</div>
            <div class="settings-theme-grid">
              <label v-for="field in brightAnsiThemeFields" :key="field.key" class="settings-theme-grid-item">
                <span>{{ field.label }}</span>
                <div class="settings-color-row">
                  <input type="color" :value="settings.theme[field.key]" @input="updateTheme(field.key, inputValue($event))">
                  <input
                    type="text"
                    :value="settings.theme[field.key]"
                    @input="updateTheme(field.key, inputValue($event))"
                    autocapitalize="none"
                    autocorrect="off"
                    spellcheck="false"
                  >
                </div>
              </label>
            </div>
          </div>
        </div>

        <!-- Shared highlight / trigger rules -->
        <div v-show="activeTab === 'automation'" class="settings-section settings-rules-section">
          <div class="settings-rules-header">
            <div>
              <strong>{{ $t('settings.rules.title') }}</strong>
              <small>{{ $t('settings.rules.subtitle') }}</small>
            </div>
            <div class="settings-rules-header-actions">
              <button class="settings-btn-secondary" type="button" @click="void enableDesktopNotifications()">{{ $t('settings.rules.enableNotifications') }}</button>
              <button class="settings-btn-primary" type="button" @click="addOutputRule">{{ $t('settings.rules.addRule') }}</button>
            </div>
          </div>

          <div v-if="settings.outputRules.length === 0" class="settings-field-full-hint">
            {{ $t('settings.rules.empty') }}
          </div>

          <div v-for="rule in settings.outputRules" :key="rule.id" class="settings-rule-card">
            <div class="settings-rule-card-title">
              <label class="settings-rule-enabled">
                <input type="checkbox" :checked="rule.enabled" @change="updateOutputRule(rule.id, 'enabled', inputChecked($event))">
                <input type="text" :value="rule.name" :placeholder="$t('settings.rules.ruleName')" @input="updateOutputRule(rule.id, 'name', inputValue($event))">
              </label>
              <button class="settings-btn-danger" type="button" @click="removeOutputRule(rule.id)">{{ $t('common.delete') }}</button>
            </div>
            <div class="settings-rule-grid">
              <label class="settings-field settings-field--stacked settings-rule-pattern">
                <span>{{ $t('settings.rules.pattern') }}</span>
                <input type="text" :value="rule.pattern" autocapitalize="none" autocorrect="off" spellcheck="false" @input="updateOutputRule(rule.id, 'pattern', inputValue($event))">
                <small v-if="outputRulePatternError(rule)" class="settings-rule-error">{{ outputRulePatternError(rule) }}</small>
              </label>
              <label class="settings-field settings-field--stacked">
                <span>{{ $t('settings.rules.scope') }}</span>
                <select :value="rule.scope" @change="updateOutputRule(rule.id, 'scope', inputValue($event) === 'hosts' ? 'hosts' : 'global')">
                  <option value="global">{{ $t('settings.rules.scopeAll') }}</option>
                  <option value="hosts">{{ $t('settings.rules.scopeHosts') }}</option>
                </select>
              </label>
              <label v-if="rule.scope === 'hosts'" class="settings-field settings-field--stacked settings-rule-hosts">
                <span>{{ $t('settings.rules.hostPatterns') }}</span>
                <input type="text" :value="rule.hosts.join(', ')" placeholder="prod-*, router.example.com" @input="updateOutputRule(rule.id, 'hosts', parseHostPatterns(inputValue($event)))">
              </label>
            </div>
            <div class="settings-rule-options">
              <label><input type="checkbox" :checked="rule.isRegex" @change="updateOutputRule(rule.id, 'isRegex', inputChecked($event))"> {{ $t('settings.rules.regex') }}</label>
              <label><input type="checkbox" :checked="rule.caseSensitive" @change="updateOutputRule(rule.id, 'caseSensitive', inputChecked($event))"> {{ $t('settings.rules.caseSensitive') }}</label>
              <label><input type="checkbox" :checked="Boolean(rule.foreground || rule.background)" @change="updateOutputRule(rule.id, 'foreground', inputChecked($event) ? (rule.foreground || '#ff6b6b') : undefined); !inputChecked($event) && updateOutputRule(rule.id, 'background', undefined)"> {{ $t('settings.rules.highlight') }}</label>
              <label><input type="checkbox" :checked="rule.bell" @change="updateOutputRule(rule.id, 'bell', inputChecked($event))"> {{ $t('settings.rules.bell') }}</label>
              <label><input type="checkbox" :checked="rule.notify" @change="updateOutputRule(rule.id, 'notify', inputChecked($event))"> {{ $t('settings.rules.notification') }}</label>
            </div>
            <div class="settings-rule-grid">
              <label class="settings-field settings-field--stacked">
                <span>{{ $t('settings.rules.textColor') }}</span>
                <input type="color" :disabled="!rule.foreground && !rule.background" :value="rule.foreground || '#ff6b6b'" @input="updateOutputRule(rule.id, 'foreground', inputValue($event))">
              </label>
              <label class="settings-field settings-field--stacked">
                <span>{{ $t('settings.rules.backgroundOptional') }}</span>
                <input type="text" :value="rule.background || ''" placeholder="#3b2020" @input="updateOutputRule(rule.id, 'background', inputValue($event) || undefined)">
              </label>
              <label class="settings-field settings-field--stacked settings-rule-response">
                <span>{{ $t('settings.rules.autoResponse') }}</span>
                <input type="text" :value="rule.autoResponse || ''" placeholder="ack $1\n" @input="updateOutputRule(rule.id, 'autoResponse', inputValue($event) || undefined)">
              </label>
              <label class="settings-field settings-field--stacked">
                <span>{{ $t('settings.rules.cooldown') }}</span>
                <input type="number" min="0" :value="rule.cooldownMs" @input="updateOutputRule(rule.id, 'cooldownMs', Math.max(0, Number(inputValue($event)) || 0))">
              </label>
            </div>
          </div>
        </div>

        <!-- Security Tab -->
        <!-- AI Tab -->
        <div v-show="activeTab === 'ai'" class="settings-section">
          <label class="settings-field">
            <span>{{ $t('settings.ai.enable') }}</span>
            <input
              type="checkbox"
              :checked="settings.aiConfig.enabled"
              @change="updateAi('enabled', inputChecked($event))"
            />
          </label>
          <div class="settings-field-full-hint">{{ $t('settings.ai.enableHint') }}</div>

          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.ai.provider') }}</span>
            <select
              :value="settings.aiConfig.provider"
              @change="onAiProviderChange(inputValue($event) as AiProvider)"
            >
              <option v-for="option in aiProviderOptions" :key="option.value" :value="option.value">
                {{ option.label }}
              </option>
            </select>
          </label>

          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.ai.baseUrl') }}</span>
            <input
              type="text"
              :value="settings.aiConfig.baseUrl ?? ''"
              :placeholder="settings.aiConfig.provider === 'anthropic' ? 'https://api.anthropic.com' : 'https://api.deepseek.com/v1'"
              @input="updateAi('baseUrl', inputValue($event).trim() || null)"
            />
          </label>
          <div class="settings-field-full-hint">
            {{ settings.aiConfig.provider === 'anthropic' ? $t('settings.ai.baseUrlHintAnthropic') : $t('settings.ai.baseUrlHintOpenAi') }}
          </div>

          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.ai.model') }}</span>
            <input
              type="text"
              :value="settings.aiConfig.model"
              :placeholder="DEFAULT_AI_MODELS[settings.aiConfig.provider] || 'deepseek-chat'"
              @input="updateAi('model', inputValue($event))"
            />
          </label>

          <label class="settings-field settings-field--stacked">
            <span>{{ $t('settings.ai.maxTokens') }}</span>
            <input
              type="number"
              min="1"
              :value="settings.aiConfig.maxTokens ?? ''"
              placeholder="4096"
              @input="updateAi('maxTokens', inputValue($event) ? Math.max(1, Number(inputValue($event))) : null)"
            />
          </label>

          <div class="settings-security-header" style="margin-top: 24px;">
            <div>
              <div class="settings-security-title">{{ $t('settings.ai.apiKey') }}</div>
              <div class="settings-security-subtitle">
                {{ aiHasKey ? $t('settings.ai.apiKeySet') : $t('settings.ai.apiKeyNone') }}
              </div>
            </div>
          </div>
          <div class="settings-field-full-hint">{{ $t('settings.ai.apiKeyHint') }}</div>
          <div class="settings-ai-key-row">
            <input
              v-model="aiKeyInput"
              type="password"
              autocomplete="off"
              :placeholder="$t('settings.ai.apiKeyPlaceholder')"
            />
            <button
              class="settings-btn-secondary"
              type="button"
              :disabled="aiKeyBusy || !aiKeyInput.trim()"
              @click="saveAiKey"
            >{{ $t('common.save') }}</button>
            <button
              v-if="aiHasKey"
              class="settings-btn-secondary settings-btn-danger"
              type="button"
              :disabled="aiKeyBusy"
              @click="clearAiKey"
            >{{ $t('common.delete') }}</button>
          </div>

          <div class="settings-ai-test-row">
            <button
              class="settings-btn-secondary"
              type="button"
              :disabled="aiTesting || !aiHasKey || !settings.aiConfig.enabled"
              @click="testAiConnection"
            >{{ aiTesting ? $t('settings.ai.testing') : $t('settings.ai.test') }}</button>
            <span
              v-if="aiTestResult"
              class="settings-ai-test-result"
              :class="{ ok: aiTestResult.ok, error: !aiTestResult.ok }"
            >{{ aiTestResult.message }}</span>
            <span
              v-else-if="aiTestBlockedReason"
              class="settings-ai-test-result hint"
            >{{ aiTestBlockedReason }}</span>
          </div>
        </div>

        <div v-show="activeTab === 'security'" class="settings-section">
          <!-- Master Password -->
          <div class="settings-security-header">
            <div>
              <div class="settings-security-title">{{ $t('settings.security.masterPassword') }}</div>
              <div class="settings-security-subtitle">
                <template v-if="credentialState.passwordEnabled">
                  {{ $t('settings.security.masterPasswordSetDesc') }}
                </template>
                <template v-else>
                  {{ $t('settings.security.masterPasswordNoneDesc') }}
                </template>
              </div>
            </div>
          </div>

          <div v-if="mpError" class="settings-security-error">{{ mpError }}</div>
          <div v-if="mpSuccess" class="settings-security-success">{{ mpSuccess }}</div>

          <!-- No password: offer to enable one -->
          <div v-if="!credentialState.passwordEnabled" class="settings-backup-card" style="margin-top: 12px;">
            <div class="settings-backup-card-title">{{ $t('settings.security.setMasterPassword') }}</div>
            <div class="settings-backup-card-desc">
              {{ $t('settings.security.setMasterPasswordDesc') }}
            </div>
            <div class="settings-form-row">
              <label>{{ $t('settings.security.newPassword') }}</label>
              <input v-model="enableNewPassword" type="password" autocomplete="new-password" :disabled="mpBusy" />
            </div>
            <div class="settings-form-row">
              <label>{{ $t('settings.security.confirmPassword') }}</label>
              <input v-model="enableConfirm" type="password" autocomplete="new-password" :disabled="mpBusy" />
            </div>
            <label v-if="credentialState.rememberAvailable" class="settings-remember-row">
              <input v-model="enableRemember" type="checkbox" :disabled="mpBusy" />
              <span>{{ $t('settings.security.rememberDevice') }}</span>
            </label>
            <button class="settings-btn-primary" type="button" :disabled="mpBusy" @click="enableMasterPassword">
              {{ mpBusy ? $t('settings.security.working') : $t('settings.security.enableMasterPassword') }}
            </button>
          </div>

          <!-- Password set: remember toggle + change + remove -->
          <template v-else>
            <label v-if="credentialState.rememberAvailable" class="settings-remember-row" style="margin: 12px 0 16px;">
              <input
                type="checkbox"
                :checked="credentialState.rememberEnabled"
                :disabled="mpBusy"
                @change="toggleRememberMasterPassword(($event.target as HTMLInputElement).checked)"
              />
              <span>{{ $t('settings.security.rememberDevice') }}</span>
            </label>
            <div v-else class="settings-backup-card-desc" style="margin: 12px 0 16px;">
              {{ $t('settings.security.rememberUnavailable') }}
            </div>

            <div class="settings-backup-grid">
              <div class="settings-backup-card">
                <div class="settings-backup-card-title">{{ $t('settings.security.changePassword') }}</div>
                <div class="settings-form-row">
                  <label>{{ $t('settings.security.currentPassword') }}</label>
                  <input v-model="changeCurrent" type="password" autocomplete="off" :disabled="mpBusy" />
                </div>
                <div class="settings-form-row">
                  <label>{{ $t('settings.security.newPassword') }}</label>
                  <input v-model="changeNew" type="password" autocomplete="new-password" :disabled="mpBusy" />
                </div>
                <div class="settings-form-row">
                  <label>{{ $t('settings.security.confirmNewPassword') }}</label>
                  <input v-model="changeConfirm" type="password" autocomplete="new-password" :disabled="mpBusy" />
                </div>
                <button class="settings-btn-primary" type="button" :disabled="mpBusy" @click="changeMasterPassword">
                  {{ mpBusy ? $t('settings.security.working') : $t('settings.security.changePassword') }}
                </button>
              </div>

              <div class="settings-backup-card">
                <div class="settings-backup-card-title">{{ $t('settings.security.removePassword') }}</div>
                <div class="settings-backup-card-desc">
                  {{ $t('settings.security.removePasswordDesc') }}
                </div>
                <div class="settings-form-row">
                  <label>{{ $t('settings.security.currentPassword') }}</label>
                  <input v-model="disableCurrent" type="password" autocomplete="off" :disabled="mpBusy" />
                </div>
                <button class="settings-btn-secondary settings-btn-danger" type="button" :disabled="mpBusy" @click="disableMasterPassword">
                  {{ mpBusy ? $t('settings.security.working') : $t('settings.security.removeMasterPassword') }}
                </button>
              </div>
            </div>
          </template>

          <div class="settings-security-header" style="margin-top: 32px;">
            <div>
              <div class="settings-security-title">{{ $t('settings.security.trustedHosts') }}</div>
              <div class="settings-security-subtitle">
                {{ $t('settings.security.trustedHostsDesc') }}
              </div>
            </div>
            <div class="settings-security-actions">
              <button class="settings-btn-secondary" type="button" @click="refreshTrustedHostKeys" :disabled="trustedHostKeysLoading">
                {{ $t('settings.security.refresh') }}
              </button>
              <button
                class="settings-btn-secondary settings-btn-danger"
                type="button"
                @click="resetTrustedHostKeys"
                :disabled="resettingHostKeys || trustedHostKeysLoading || trustedHostKeys.length === 0"
              >
                {{ resettingHostKeys ? $t('settings.security.resetting') : $t('settings.security.resetAll') }}
              </button>
            </div>
          </div>

          <div v-if="trustedHostKeysError" class="settings-security-error">{{ trustedHostKeysError }}</div>

          <div v-if="trustedHostKeysLoading" class="settings-security-loading">{{ $t('settings.security.loadingHostKeys') }}</div>

          <div v-else-if="trustedHostKeys.length === 0" class="settings-security-empty">
            {{ $t('settings.security.noHostKeys') }}
          </div>

          <div v-else class="settings-security-list">
            <div v-for="entry in trustedHostKeys" :key="trustedHostScope(entry)" class="settings-security-item">
              <div class="settings-security-item-main">
                <div class="settings-security-host">{{ entry.host }}:{{ entry.port }}</div>
                <div class="settings-security-fingerprint" :title="entry.fingerprint">
                  {{ entry.fingerprintSummary }}
                </div>
                <div class="settings-security-fingerprint-full">{{ entry.fingerprint }}</div>
              </div>
              <button
                class="settings-btn-secondary settings-btn-danger"
                type="button"
                @click="removeTrustedHostKey(entry)"
                :disabled="deletingHostKeyScope === trustedHostScope(entry) || resettingHostKeys"
              >
                {{ deletingHostKeyScope === trustedHostScope(entry) ? $t('settings.security.removing') : $t('common.delete') }}
              </button>
            </div>
          </div>

          <!-- Encrypted Credential Backup -->
          <div class="settings-security-header" style="margin-top: 32px;">
            <div>
              <div class="settings-security-title">{{ $t('settings.security.credentialBackup') }}</div>
              <div class="settings-security-subtitle">
                {{ $t('settings.security.credentialBackupDesc') }}
              </div>
            </div>
          </div>

          <div class="settings-backup-grid">
            <!-- Export -->
            <div class="settings-backup-card">
              <div class="settings-backup-card-title">{{ $t('settings.security.exportBackup') }}</div>
              <div class="settings-backup-card-desc">
                {{ $t('settings.security.exportBackupDesc') }}
              </div>
              <div class="settings-form-row">
                <label>{{ $t('settings.security.backupPasswordMin') }}</label>
                <input
                  v-model="exportPassword"
                  type="password"
                  autocomplete="new-password"
                  :disabled="exportLoading"
                />
              </div>
              <div class="settings-form-row">
                <label>{{ $t('settings.security.confirmPassword') }}</label>
                <input
                  v-model="exportPasswordConfirm"
                  type="password"
                  autocomplete="new-password"
                  :disabled="exportLoading"
                />
              </div>
              <div v-if="exportError" class="settings-security-error">{{ exportError }}</div>
              <div v-if="exportSuccess" class="settings-security-success">{{ exportSuccess }}</div>
              <button
                class="settings-btn-primary"
                type="button"
                :disabled="exportLoading"
                @click="exportCredentials"
              >
                {{ exportLoading ? $t('settings.security.exporting') : $t('settings.security.exportDownload') }}
              </button>
            </div>

            <!-- Import -->
            <div class="settings-backup-card">
              <div class="settings-backup-card-title">{{ $t('settings.security.importBackup') }}</div>
              <div class="settings-backup-card-desc">
                {{ $t('settings.security.importBackupDesc') }}
              </div>
              <div class="settings-form-row">
                <label>{{ $t('settings.security.backupFile') }}</label>
                <input
                  ref="importFileInput"
                  type="file"
                  accept=".aurabak,.txt,application/octet-stream"
                  :disabled="importLoading"
                  @change="onImportFileSelected"
                />
                <small v-if="pendingImportFileName">{{ $t('settings.security.selectedFile', { name: pendingImportFileName }) }}</small>
              </div>
              <div class="settings-form-row">
                <label>{{ $t('settings.security.backupPassword') }}</label>
                <input
                  v-model="importPassword"
                  type="password"
                  autocomplete="off"
                  :disabled="importLoading"
                />
              </div>
              <div v-if="importError" class="settings-security-error">{{ importError }}</div>
              <div v-if="importSuccess" class="settings-security-success">{{ importSuccess }}</div>
              <button
                class="settings-btn-primary"
                type="button"
                :disabled="importLoading || !pendingImportContent"
                @click="importCredentials"
              >
                {{ importLoading ? $t('settings.security.importing') : $t('settings.security.import') }}
              </button>
            </div>
          </div>

          <!-- Lock master password (only relevant when one is set) -->
          <div v-if="credentialState.passwordEnabled" class="settings-security-header" style="margin-top: 32px;">
            <div>
              <div class="settings-security-title">{{ $t('settings.security.lockMasterPassword') }}</div>
              <div class="settings-security-subtitle">
                {{ $t('settings.security.lockMasterPasswordDesc') }}
              </div>
            </div>
            <div class="settings-security-actions">
              <button
                class="settings-btn-secondary settings-btn-danger"
                type="button"
                :disabled="lockingMasterPassword"
                @click="lockMasterPasswordNow"
              >
                {{ lockingMasterPassword ? $t('settings.security.locking') : $t('settings.security.lockNow') }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-footer">
        <button class="settings-btn-cancel" type="button" @click="emit('cancel')">{{ $t('common.cancel') }}</button>
        <button class="settings-btn-save" type="button" @click="emit('save', settings)">{{ $t('common.save') }}</button>
      </div>
    </div>
  </div>
</template>
