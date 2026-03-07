<script setup lang="ts">
import { ref } from "vue";
import type { AppSettings } from "./settings";

const props = defineProps<{
  initial: AppSettings;
}>();

const emit = defineEmits<{
  save: [settings: AppSettings];
  cancel: [];
}>();

const settings = ref<AppSettings>(structuredClone(props.initial));

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

      <div class="settings-body">
        <section class="settings-section">
          <h3>Terminal</h3>

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
            <span>Shell Path</span>
            <input
              type="text"
              placeholder="Default (uses $SHELL)"
              :value="settings.shellPath ?? ''"
              @input="update('shellPath', inputValue($event) || null)"
            >
          </label>
        </section>

        <section class="settings-section">
          <h3>Keyboard &amp; Mouse</h3>

          <label class="settings-field settings-field--toggle">
            <span>
              <strong>Copy on select</strong>
              <small>选中文本后自动复制到剪贴板；Ctrl+C 有选中时消费按键（不发 ^C 给 PTY）</small>
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
              <small>Ctrl+V 将剪贴板内容粘贴到终端</small>
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
              <small>鼠标中键点击将剪贴板内容粘贴到终端</small>
            </span>
            <input
              type="checkbox"
              class="settings-toggle"
              :checked="settings.middleClickPaste"
              @change="update('middleClickPaste', inputChecked($event))"
            >
          </label>
        </section>

        <section class="settings-section">
          <h3>Theme</h3>

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
        </section>
      </div>

      <div class="settings-footer">
        <button class="settings-btn-cancel" type="button" @click="emit('cancel')">Cancel</button>
        <button class="settings-btn-save" type="button" @click="emit('save', settings)">Save</button>
      </div>
    </div>
  </div>
</template>