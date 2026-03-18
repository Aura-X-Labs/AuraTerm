<script setup lang="ts">
import { ref, computed } from "vue";
import {
  cloneTerminalTheme,
  getMatchingTerminalThemePreset,
  getTerminalThemePreset,
  normalizeAppSettings,
  TERMINAL_THEME_PRESETS,
  type AppSettings,
  type TerminalTheme,
} from "./settings";

const props = defineProps<{
  initial: AppSettings;
}>();

const emit = defineEmits<{
  save: [settings: AppSettings];
  cancel: [];
}>();

const settings = ref<AppSettings>(normalizeAppSettings(props.initial));

const activeTab = ref<"terminal" | "keyboard" | "theme">("terminal");

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

function resetThemeToDefault() {
  const defaultPreset = getTerminalThemePreset("aura-dark");
  if (!defaultPreset) {
    return;
  }

  update("theme", cloneTerminalTheme(defaultPreset.theme));
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
              <small>Show input bar and quick buttons below the terminal</small>
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
            <span>Preset</span>
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
      </div>

      <div class="settings-footer">
        <button class="settings-btn-cancel" type="button" @click="emit('cancel')">Cancel</button>
        <button class="settings-btn-save" type="button" @click="emit('save', settings)">Save</button>
      </div>
    </div>
  </div>
</template>