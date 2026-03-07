<script setup lang="ts">
import { ref, computed } from "vue";
import { normalizeAppSettings, type AppSettings } from "./settings";

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

// Shell preset options
const shellPresets: Array<{ value: ShellPresetValue; label: string }> = [
  { value: "auto", label: "Auto Detect (Git Bash → CMD)" },
  { value: "git-bash", label: "Git Bash" },
  { value: "powershell", label: "PowerShell" },
  { value: "cmd", label: "Command Prompt (cmd.exe)" },
  { value: "custom", label: "Custom Path" },
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
            <input type="text" :value="settings.fontFamily" @input="update('fontFamily', inputValue($event))">
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
            <select :value="selectedShellPreset" @change="selectedShellPreset = inputValue($event) as ShellPresetValue">
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
            >
          </label>

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
          <label class="settings-field">
            <span>Background</span>
            <div class="settings-color-row">
              <input type="color" :value="settings.theme.background" @input="updateTheme('background', inputValue($event))">
              <input type="text" :value="settings.theme.background" @input="updateTheme('background', inputValue($event))">
            </div>
          </label>

          <label class="settings-field">
            <span>Foreground</span>
            <div class="settings-color-row">
              <input type="color" :value="settings.theme.foreground" @input="updateTheme('foreground', inputValue($event))">
              <input type="text" :value="settings.theme.foreground" @input="updateTheme('foreground', inputValue($event))">
            </div>
          </label>

          <label class="settings-field">
            <span>Cursor</span>
            <div class="settings-color-row">
              <input type="color" :value="settings.theme.cursor" @input="updateTheme('cursor', inputValue($event))">
              <input type="text" :value="settings.theme.cursor" @input="updateTheme('cursor', inputValue($event))">
            </div>
          </label>
        </div>
      </div>

      <div class="settings-footer">
        <button class="settings-btn-cancel" type="button" @click="emit('cancel')">Cancel</button>
        <button class="settings-btn-save" type="button" @click="emit('save', settings)">Save</button>
      </div>
    </div>
  </div>
</template>