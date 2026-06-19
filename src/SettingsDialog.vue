<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { ref, computed, watch } from "vue";
import {
  cloneTerminalTheme,
  getMatchingTerminalThemePreset,
  getTerminalThemePreset,
  normalizeAppSettings,
  TERMINAL_THEME_PRESETS,
  type AppSettings,
  type OutputRule,
  type TerminalTheme,
  type UiThemeMode,
} from "./settings";

const props = defineProps<{
  initial: AppSettings;
}>();

const emit = defineEmits<{
  save: [settings: AppSettings];
  cancel: [];
}>();

const settings = ref<AppSettings>(normalizeAppSettings(props.initial));

const activeTab = ref<"terminal" | "keyboard" | "theme" | "automation" | "security">("terminal");

type TrustedSshHostKeyEntry = {
  host: string;
  port: number;
  fingerprint: string;
  fingerprintSummary: string;
};

type ShellPresetValue = "auto" | "git-bash" | "powershell" | "cmd" | "custom";
type ThemeColorKey = keyof TerminalTheme;

// Shell preset options
const shellPresets: Array<{ value: ShellPresetValue; label: string }> = [
  { value: "auto", label: "Auto Detect (Git Bash → CMD)" },
  { value: "git-bash", label: "Git Bash" },
  { value: "powershell", label: "PowerShell" },
  { value: "cmd", label: "Command Prompt (cmd.exe)" },
  { value: "custom", label: "Custom Path" },
];

const themePresets = TERMINAL_THEME_PRESETS;
const uiThemeModeOptions: Array<{ value: UiThemeMode; label: string }> = [
  { value: "follow-terminal", label: "Follow Terminal Theme" },
  { value: "light", label: "Light UI" },
  { value: "dark", label: "Dark UI" },
];
const basicThemeFields: Array<{ key: ThemeColorKey; label: string }> = [
  { key: "background", label: "Background" },
  { key: "foreground", label: "Foreground" },
  { key: "cursor", label: "Cursor" },
  { key: "selectionBackground", label: "Selection" },
];
const ansiThemeFields: Array<{ key: ThemeColorKey; label: string }> = [
  { key: "black", label: "Black" },
  { key: "red", label: "Red" },
  { key: "green", label: "Green" },
  { key: "yellow", label: "Yellow" },
  { key: "blue", label: "Blue" },
  { key: "magenta", label: "Magenta" },
  { key: "cyan", label: "Cyan" },
  { key: "white", label: "White" },
];
const brightAnsiThemeFields: Array<{ key: ThemeColorKey; label: string }> = [
  { key: "brightBlack", label: "Bright Black" },
  { key: "brightRed", label: "Bright Red" },
  { key: "brightGreen", label: "Bright Green" },
  { key: "brightYellow", label: "Bright Yellow" },
  { key: "brightBlue", label: "Bright Blue" },
  { key: "brightMagenta", label: "Bright Magenta" },
  { key: "brightCyan", label: "Bright Cyan" },
  { key: "brightWhite", label: "Bright White" },
];

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
  return matchedPreset?.description ?? "Custom palette based on the colors below.";
});
const selectedUiThemeModeDescription = computed(() => {
  switch (settings.value.uiThemeMode) {
    case "light":
      return "Use a dedicated light office UI, independent from the terminal preset.";
    case "dark":
      return "Use a dedicated dark UI, independent from the terminal preset.";
    default:
      return "Let the app UI follow the terminal theme appearance and accent direction.";
  }
});

function update<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
  settings.value = { ...settings.value, [key]: value };
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
  const confirmed = window.confirm("Remove all trusted SSH host fingerprints?");
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
    window.alert("Master password locked. The application will now reload.");
    window.location.reload();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    window.alert(`Failed to lock master password: ${message}`);
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
        <h2>Settings</h2>
        <button class="settings-close-btn" type="button" @click="emit('cancel')">×</button>
      </div>

      <div class="settings-tabs">
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'terminal' }]"
          type="button"
          @click="activeTab = 'terminal'"
        >Terminal</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'keyboard' }]"
          type="button"
          @click="activeTab = 'keyboard'"
        >Keyboard & Mouse</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'theme' }]"
          type="button"
          @click="activeTab = 'theme'"
        >Theme</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'automation' }]"
          type="button"
          @click="activeTab = 'automation'"
        >Rules</button>
        <button
          :class="['settings-tab', { 'settings-tab--active': activeTab === 'security' }]"
          type="button"
          @click="activeTab = 'security'"
        >Security</button>
      </div>

      <div class="settings-body">
        <!-- Terminal Tab -->
        <div v-show="activeTab === 'terminal'" class="settings-section">
          <label class="settings-field">
            <span>Font Size</span>
            <input
              type="number"
              min="8"
              max="72"
              :value="settings.fontSize"
              @input="update('fontSize', Number(inputValue($event)))"
            >
          </label>

          <label class="settings-field">
            <span>Font Family</span>
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
            <span>Scrollback Lines</span>
            <input
              type="number"
              min="100"
              max="100000"
              step="100"
              :value="settings.scrollback"
              @input="update('scrollback', Number(inputValue($event)))"
            >
          </label>

          <label class="settings-field">
            <span>Shell</span>
            <select :value="selectedShellPreset" @change="handleShellPresetChange">
              <option v-for="preset in shellPresets" :key="preset.value" :value="preset.value">
                {{ preset.label }}
              </option>
            </select>
          </label>

          <label v-if="showCustomPath" class="settings-field">
            <span>Custom Shell Path</span>
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
            <span>Default Log Save Path</span>
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
            <span>Default Log Filename Template</span>
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
          <div class="settings-field-full-hint">Available placeholders: {timestamp}, {datetime}, {date}, {time}, {yyyy}, {MM}, {dd}, {HH}, {mm}, {ss}, {unix}, {session}, {protocol}, {host}, {user}, {port}, {serialPort}, {baudRate}</div>



          <label class="settings-field settings-field--toggle">
            <span>
              <strong>Show Input Bar</strong>
              <small>Show input bar and snippet toolbars below the terminal</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.showInputBar"
              @change="update('showInputBar', inputChecked($event))"
            >
          </label>

          <label class="settings-field settings-field--toggle">
            <span>
              <strong>Open SFTP after SSH connects</strong>
              <small>Automatically reveal Remote Files for the active SSH session</small>
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
              <strong>Restore Session Tabs On Startup</strong>
              <small>Restore all open tabs and pane layout after restarting AuraTerm</small>
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
              <strong>Copy on select</strong>
              <small>Auto-copy selected text to clipboard; Ctrl+C consumes key when selection exists (no ^C to PTY)</small>
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
              <strong>Ctrl+V</strong> Paste from clipboard
              <small>Ctrl+V pastes clipboard content to terminal</small>
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
              <strong>Middle-click</strong> Paste
              <small>Middle-click pastes clipboard content to terminal</small>
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
            <span>UI Style</span>
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
            <span>Terminal Preset</span>
            <div class="settings-theme-preset-row">
              <select :value="selectedThemePresetId" @change="handleThemePresetChange">
                <option v-for="preset in themePresets" :key="preset.id" :value="preset.id">
                  {{ preset.label }}
                </option>
                <option value="custom">Custom</option>
              </select>
              <button class="settings-btn-secondary" type="button" @click="resetThemeToDefault">Reset</button>
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
              <strong>Preview</strong>
              <span>{{ selectedThemePresetId === 'custom' ? 'Custom palette' : 'Preset palette' }}</span>
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
              <span class="settings-theme-swatch" :style="{ background: settings.theme.red }" title="Red"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.green }" title="Green"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.yellow }" title="Yellow"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.blue }" title="Blue"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.magenta }" title="Magenta"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.cyan }" title="Cyan"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.brightRed }" title="Bright Red"></span>
              <span class="settings-theme-swatch" :style="{ background: settings.theme.brightBlue }" title="Bright Blue"></span>
            </div>
          </div>

          <div class="settings-theme-subsection">
            <div class="settings-theme-subtitle">Core colors</div>
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
            <div class="settings-theme-subtitle">ANSI colors</div>
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
            <div class="settings-theme-subtitle">Bright ANSI colors</div>
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
              <strong>Output Rules</strong>
              <small>One matcher powers highlighting, alerts, and automatic responses.</small>
            </div>
            <div class="settings-rules-header-actions">
              <button class="settings-btn-secondary" type="button" @click="void enableDesktopNotifications()">Enable notifications</button>
              <button class="settings-btn-primary" type="button" @click="addOutputRule">+ Add rule</button>
            </div>
          </div>

          <div v-if="settings.outputRules.length === 0" class="settings-field-full-hint">
            No output rules yet. Add one to highlight keywords or react to terminal output.
          </div>

          <div v-for="rule in settings.outputRules" :key="rule.id" class="settings-rule-card">
            <div class="settings-rule-card-title">
              <label class="settings-rule-enabled">
                <input type="checkbox" :checked="rule.enabled" @change="updateOutputRule(rule.id, 'enabled', inputChecked($event))">
                <input type="text" :value="rule.name" placeholder="Rule name" @input="updateOutputRule(rule.id, 'name', inputValue($event))">
              </label>
              <button class="settings-btn-danger" type="button" @click="removeOutputRule(rule.id)">Delete</button>
            </div>
            <div class="settings-rule-grid">
              <label class="settings-field settings-field--stacked settings-rule-pattern">
                <span>Pattern</span>
                <input type="text" :value="rule.pattern" autocapitalize="none" autocorrect="off" spellcheck="false" @input="updateOutputRule(rule.id, 'pattern', inputValue($event))">
                <small v-if="outputRulePatternError(rule)" class="settings-rule-error">{{ outputRulePatternError(rule) }}</small>
              </label>
              <label class="settings-field settings-field--stacked">
                <span>Scope</span>
                <select :value="rule.scope" @change="updateOutputRule(rule.id, 'scope', inputValue($event) === 'hosts' ? 'hosts' : 'global')">
                  <option value="global">All sessions</option>
                  <option value="hosts">SSH hosts</option>
                </select>
              </label>
              <label v-if="rule.scope === 'hosts'" class="settings-field settings-field--stacked settings-rule-hosts">
                <span>Host patterns (comma separated)</span>
                <input type="text" :value="rule.hosts.join(', ')" placeholder="prod-*, router.example.com" @input="updateOutputRule(rule.id, 'hosts', parseHostPatterns(inputValue($event)))">
              </label>
            </div>
            <div class="settings-rule-options">
              <label><input type="checkbox" :checked="rule.isRegex" @change="updateOutputRule(rule.id, 'isRegex', inputChecked($event))"> Regular expression</label>
              <label><input type="checkbox" :checked="rule.caseSensitive" @change="updateOutputRule(rule.id, 'caseSensitive', inputChecked($event))"> Case sensitive</label>
              <label><input type="checkbox" :checked="Boolean(rule.foreground || rule.background)" @change="updateOutputRule(rule.id, 'foreground', inputChecked($event) ? (rule.foreground || '#ff6b6b') : undefined); !inputChecked($event) && updateOutputRule(rule.id, 'background', undefined)"> Highlight</label>
              <label><input type="checkbox" :checked="rule.bell" @change="updateOutputRule(rule.id, 'bell', inputChecked($event))"> Bell</label>
              <label><input type="checkbox" :checked="rule.notify" @change="updateOutputRule(rule.id, 'notify', inputChecked($event))"> Notification</label>
            </div>
            <div class="settings-rule-grid">
              <label class="settings-field settings-field--stacked">
                <span>Text color</span>
                <input type="color" :disabled="!rule.foreground && !rule.background" :value="rule.foreground || '#ff6b6b'" @input="updateOutputRule(rule.id, 'foreground', inputValue($event))">
              </label>
              <label class="settings-field settings-field--stacked">
                <span>Background (optional)</span>
                <input type="text" :value="rule.background || ''" placeholder="#3b2020" @input="updateOutputRule(rule.id, 'background', inputValue($event) || undefined)">
              </label>
              <label class="settings-field settings-field--stacked settings-rule-response">
                <span>Automatic response (optional, $1 uses capture group)</span>
                <input type="text" :value="rule.autoResponse || ''" placeholder="ack $1\n" @input="updateOutputRule(rule.id, 'autoResponse', inputValue($event) || undefined)">
              </label>
              <label class="settings-field settings-field--stacked">
                <span>Cooldown (ms)</span>
                <input type="number" min="0" :value="rule.cooldownMs" @input="updateOutputRule(rule.id, 'cooldownMs', Math.max(0, Number(inputValue($event)) || 0))">
              </label>
            </div>
          </div>
        </div>

        <!-- Security Tab -->
        <div v-show="activeTab === 'security'" class="settings-section">
          <!-- Master Password -->
          <div class="settings-security-header">
            <div>
              <div class="settings-security-title">Master Password</div>
              <div class="settings-security-subtitle">
                <template v-if="credentialState.passwordEnabled">
                  A master password is set; saved credentials are encrypted with it.
                </template>
                <template v-else>
                  No master password — credentials are encrypted with a device-local key and the
                  app never prompts on startup. Anyone able to sign in as you can read them, so keep
                  an encrypted backup (below).
                </template>
              </div>
            </div>
          </div>

          <div v-if="mpError" class="settings-security-error">{{ mpError }}</div>
          <div v-if="mpSuccess" class="settings-security-success">{{ mpSuccess }}</div>

          <!-- No password: offer to enable one -->
          <div v-if="!credentialState.passwordEnabled" class="settings-backup-card" style="margin-top: 12px;">
            <div class="settings-backup-card-title">Set a master password</div>
            <div class="settings-backup-card-desc">
              You'll enter it on launch (or have it remembered on this device).
            </div>
            <div class="settings-form-row">
              <label>New password (min 8 chars)</label>
              <input v-model="enableNewPassword" type="password" autocomplete="new-password" :disabled="mpBusy" />
            </div>
            <div class="settings-form-row">
              <label>Confirm password</label>
              <input v-model="enableConfirm" type="password" autocomplete="new-password" :disabled="mpBusy" />
            </div>
            <label v-if="credentialState.rememberAvailable" class="settings-remember-row">
              <input v-model="enableRemember" type="checkbox" :disabled="mpBusy" />
              <span>Remember on this device (auto-unlock via system keychain)</span>
            </label>
            <button class="settings-btn-primary" type="button" :disabled="mpBusy" @click="enableMasterPassword">
              {{ mpBusy ? 'Working...' : 'Enable Master Password' }}
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
              <span>Remember on this device (auto-unlock via system keychain)</span>
            </label>
            <div v-else class="settings-backup-card-desc" style="margin: 12px 0 16px;">
              Auto-unlock via the system keychain is available on macOS and Windows only.
            </div>

            <div class="settings-backup-grid">
              <div class="settings-backup-card">
                <div class="settings-backup-card-title">Change Password</div>
                <div class="settings-form-row">
                  <label>Current password</label>
                  <input v-model="changeCurrent" type="password" autocomplete="off" :disabled="mpBusy" />
                </div>
                <div class="settings-form-row">
                  <label>New password (min 8 chars)</label>
                  <input v-model="changeNew" type="password" autocomplete="new-password" :disabled="mpBusy" />
                </div>
                <div class="settings-form-row">
                  <label>Confirm new password</label>
                  <input v-model="changeConfirm" type="password" autocomplete="new-password" :disabled="mpBusy" />
                </div>
                <button class="settings-btn-primary" type="button" :disabled="mpBusy" @click="changeMasterPassword">
                  {{ mpBusy ? 'Working...' : 'Change Password' }}
                </button>
              </div>

              <div class="settings-backup-card">
                <div class="settings-backup-card-title">Remove Password</div>
                <div class="settings-backup-card-desc">
                  Switch back to a device-local key (no prompt on startup).
                </div>
                <div class="settings-form-row">
                  <label>Current password</label>
                  <input v-model="disableCurrent" type="password" autocomplete="off" :disabled="mpBusy" />
                </div>
                <button class="settings-btn-secondary settings-btn-danger" type="button" :disabled="mpBusy" @click="disableMasterPassword">
                  {{ mpBusy ? 'Working...' : 'Remove Master Password' }}
                </button>
              </div>
            </div>
          </template>

          <div class="settings-security-header" style="margin-top: 32px;">
            <div>
              <div class="settings-security-title">Trusted SSH Host Fingerprints</div>
              <div class="settings-security-subtitle">
                Review and manage fingerprints saved for host verification.
              </div>
            </div>
            <div class="settings-security-actions">
              <button class="settings-btn-secondary" type="button" @click="refreshTrustedHostKeys" :disabled="trustedHostKeysLoading">
                Refresh
              </button>
              <button
                class="settings-btn-secondary settings-btn-danger"
                type="button"
                @click="resetTrustedHostKeys"
                :disabled="resettingHostKeys || trustedHostKeysLoading || trustedHostKeys.length === 0"
              >
                {{ resettingHostKeys ? 'Resetting...' : 'Reset All' }}
              </button>
            </div>
          </div>

          <div v-if="trustedHostKeysError" class="settings-security-error">{{ trustedHostKeysError }}</div>

          <div v-if="trustedHostKeysLoading" class="settings-security-loading">Loading trusted host keys...</div>

          <div v-else-if="trustedHostKeys.length === 0" class="settings-security-empty">
            No trusted SSH host fingerprints found.
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
                {{ deletingHostKeyScope === trustedHostScope(entry) ? 'Removing...' : 'Delete' }}
              </button>
            </div>
          </div>

          <!-- Encrypted Credential Backup -->
          <div class="settings-security-header" style="margin-top: 32px;">
            <div>
              <div class="settings-security-title">Encrypted Credential Backup</div>
              <div class="settings-security-subtitle">
                Export saved connection passwords/SSH keys to an encrypted backup, or import a previous backup.
                The backup uses its own password (independent of your master password).
              </div>
            </div>
          </div>

          <div class="settings-backup-grid">
            <!-- Export -->
            <div class="settings-backup-card">
              <div class="settings-backup-card-title">Export Backup</div>
              <div class="settings-backup-card-desc">
                Choose a strong password to encrypt the exported file.
              </div>
              <div class="settings-form-row">
                <label>Backup password (min 8 chars)</label>
                <input
                  v-model="exportPassword"
                  type="password"
                  autocomplete="new-password"
                  :disabled="exportLoading"
                />
              </div>
              <div class="settings-form-row">
                <label>Confirm password</label>
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
                {{ exportLoading ? 'Exporting...' : 'Export & Download' }}
              </button>
            </div>

            <!-- Import -->
            <div class="settings-backup-card">
              <div class="settings-backup-card-title">Import Backup</div>
              <div class="settings-backup-card-desc">
                Existing connections with the same ID will be overwritten.
              </div>
              <div class="settings-form-row">
                <label>Backup file</label>
                <input
                  ref="importFileInput"
                  type="file"
                  accept=".aurabak,.txt,application/octet-stream"
                  :disabled="importLoading"
                  @change="onImportFileSelected"
                />
                <small v-if="pendingImportFileName">Selected: {{ pendingImportFileName }}</small>
              </div>
              <div class="settings-form-row">
                <label>Backup password</label>
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
                {{ importLoading ? 'Importing...' : 'Import' }}
              </button>
            </div>
          </div>

          <!-- Lock master password (only relevant when one is set) -->
          <div v-if="credentialState.passwordEnabled" class="settings-security-header" style="margin-top: 32px;">
            <div>
              <div class="settings-security-title">Lock Master Password</div>
              <div class="settings-security-subtitle">
                Clear the cached master password from memory. The app will reload and prompt
                you to enter it again before accessing saved credentials.
              </div>
            </div>
            <div class="settings-security-actions">
              <button
                class="settings-btn-secondary settings-btn-danger"
                type="button"
                :disabled="lockingMasterPassword"
                @click="lockMasterPasswordNow"
              >
                {{ lockingMasterPassword ? 'Locking...' : 'Lock Now' }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-footer">
        <button class="settings-btn-cancel" type="button" @click="emit('cancel')">Cancel</button>
        <button class="settings-btn-save" type="button" @click="emit('save', settings)">Save</button>
      </div>
    </div>
  </div>
</template>
