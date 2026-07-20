<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { QuickButton } from "./settings";
import { buildSnippetPayload, snippetApplies, snippetVariables } from "./snippets";
import { t } from "./i18n";
import { promptText } from "./promptDialog";
import { aiComplete } from "./ai";
import { commandGenerationSystemPrompt, extractCommand } from "./aiPrompts";

const props = defineProps<{
  quickButtons: QuickButton[];
  inputHistory: string[];
  activeHost?: string;
  sessionGroup?: string;
  /** Whether AI command generation is available (enabled + API key present). */
  aiAvailable?: boolean;
  /** Environment hints for the command-generation prompt. */
  aiEnv?: { os?: string; shell?: string };
}>();

const emit = defineEmits<{
  send: [text: string, raw?: boolean];
  buttonsChange: [buttons: QuickButton[]];
  resize: [];
}>();

const SNAP_COLLAPSE_PX = 28;
const DEFAULT_TEXTAREA_H = 90;

const text = ref("");
const showEditor = ref(false);
const editButtons = ref<QuickButton[]>([]);
const selectedButtonId = ref<string | null>(null);
const textareaH = ref(DEFAULT_TEXTAREA_H);
const textareaRef = ref<HTMLTextAreaElement | null>(null);
const selectedToolbar = ref("Default");
let dragStartY = 0;
let dragStartH = 0;

// History navigation state
const historyIndex = ref(-1);
const savedTextBeforeNav = ref("");

// AI command-generation state. When `aiMode` is on, the textarea content is a
// natural-language request; Enter generates a command (it does not send).
const aiMode = ref(false);
const aiGenerating = ref(false);
const aiError = ref("");

function toggleAiMode() {
  if (!props.aiAvailable) return;
  aiMode.value = !aiMode.value;
  aiError.value = "";
  if (aiMode.value) {
    void nextTick(() => textareaRef.value?.focus());
  }
}

async function generateCommand() {
  const request = text.value.trim();
  if (!request || aiGenerating.value || !props.aiAvailable) return;
  aiGenerating.value = true;
  aiError.value = "";
  try {
    const system = commandGenerationSystemPrompt(props.aiEnv ?? {});
    const reply = await aiComplete([{ role: "user", content: request }], system);
    const command = extractCommand(reply);
    if (!command) {
      aiError.value = t("inputBar.ai.noCommand");
      return;
    }
    // Drop the generated command into the input for the user to review and run.
    aiMode.value = false;
    text.value = command;
    void nextTick(() => {
      const el = textareaRef.value;
      if (el) {
        el.focus();
        el.selectionStart = el.selectionEnd = text.value.length;
      }
    });
  } catch (error) {
    aiError.value = String(error);
  } finally {
    aiGenerating.value = false;
  }
}

watch(text, () => {
  // Reset history index when user types manually (not during navigation)
  if (historyIndex.value >= 0) {
    historyIndex.value = -1;
    savedTextBeforeNav.value = "";
  }
});

function inputValue(event: Event) {
  return (event.target as HTMLInputElement | HTMLTextAreaElement).value;
}

function handleResizeMouseDown(event: MouseEvent) {
  event.preventDefault();
  dragStartY = event.clientY;
  dragStartH = textareaH.value;

  const handleMove = (moveEvent: globalThis.MouseEvent) => {
    const delta = dragStartY - moveEvent.clientY;
    const nextHeight = Math.max(0, dragStartH + delta);
    textareaH.value = nextHeight < SNAP_COLLAPSE_PX ? 0 : nextHeight;
  };

  const handleUp = () => {
    document.removeEventListener("mousemove", handleMove);
    document.removeEventListener("mouseup", handleUp);
    emit("resize");
  };

  document.addEventListener("mousemove", handleMove);
  document.addEventListener("mouseup", handleUp);
}

function handleResizeDblClick() {
  textareaH.value = textareaH.value === 0 ? DEFAULT_TEXTAREA_H : 0;
  emit("resize");
}

const availableToolbars = computed(() => {
  const names = props.quickButtons
    .filter((button) => snippetApplies(button, props.activeHost, props.sessionGroup))
    .map((button) => button.toolbar || "Default");
  return [...new Set(names)];
});

const visibleButtons = computed(() => props.quickButtons.filter((button) => (
  snippetApplies(button, props.activeHost, props.sessionGroup)
  && (button.toolbar || "Default") === selectedToolbar.value
)));

watch(availableToolbars, (toolbars) => {
  if (!toolbars.includes(selectedToolbar.value)) {
    selectedToolbar.value = toolbars[0] || "Default";
  }
}, { immediate: true });

function doSend(payload: string, raw = false) {
  if (raw ? payload.length === 0 : !payload.trim()) {
    return;
  }
  emit("send", raw ? payload : (payload.endsWith("\n") ? payload : `${payload}\n`), raw);
}

function navigateHistory(direction: 1 | -1) {
  if (direction === 1) { // Older
    if (props.inputHistory.length === 0) return;
    if (historyIndex.value < 0) {
      savedTextBeforeNav.value = text.value;
    }
    const nextIndex = Math.min(historyIndex.value + 1, props.inputHistory.length - 1);
    if (nextIndex !== historyIndex.value) {
      historyIndex.value = nextIndex;
      text.value = props.inputHistory[nextIndex] ?? "";
    }
  } else { // Newer
    if (historyIndex.value < 0) return;
    const nextIndex = historyIndex.value - 1;
    if (nextIndex < 0) {
      historyIndex.value = -1;
      text.value = savedTextBeforeNav.value;
      savedTextBeforeNav.value = "";
    } else {
      historyIndex.value = nextIndex;
      text.value = props.inputHistory[nextIndex] ?? "";
    }
  }

  // Move cursor to end
  setTimeout(() => {
    if (textareaRef.value) {
      textareaRef.value.selectionStart = textareaRef.value.selectionEnd = text.value.length;
    }
  }, 0);
}

function handleKeyDown(event: KeyboardEvent) {
  // Ctrl/Cmd+K: toggle AI command-generation mode.
  if ((event.ctrlKey || event.metaKey) && (event.key === "k" || event.key === "K")) {
    event.preventDefault();
    toggleAiMode();
    return;
  }

  // While in AI mode, Enter generates a command (does not send); Escape exits.
  if (aiMode.value) {
    if (event.key === "Escape") {
      event.preventDefault();
      aiMode.value = false;
      aiError.value = "";
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void generateCommand();
      return;
    }
  }

  // PageUp: navigate to older history (index increases)
  if (event.key === "PageUp") {
    event.preventDefault();
    navigateHistory(1);
    return;
  }

  // PageDown: navigate to newer history (index decreases)
  if (event.key === "PageDown") {
    event.preventDefault();
    navigateHistory(-1);
    return;
  }

  // ArrowUp: navigate older history if cursor is on the first line
  if (event.key === "ArrowUp") {
    const textarea = textareaRef.value;
    if (textarea) {
      const before = text.value.substring(0, textarea.selectionStart ?? 0);
      if (!before.includes("\n")) {
        event.preventDefault();
        navigateHistory(1);
      }
    }
    return;
  }

  // ArrowDown: navigate newer history if cursor is on the last line
  if (event.key === "ArrowDown") {
    const textarea = textareaRef.value;
    if (textarea) {
      const after = text.value.substring(textarea.selectionStart ?? text.value.length);
      if (!after.includes("\n") && historyIndex.value >= 0) {
        event.preventDefault();
        navigateHistory(-1);
      }
    }
    return;
  }

  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    doSend(text.value);
    text.value = "";
  }
}

async function handleQuickButton(button: QuickButton) {
  const values: Record<string, string> = {};
  for (const variable of snippetVariables(button.command)) {
    const value = await promptText(t("inputBar.valuePrompt", { variable: `{{${variable}}}` }));
    if (value === null) return;
    values[variable] = value;
  }
  const payload = buildSnippetPayload(button, values);
  doSend(payload, button.sendMode === "raw");
  textareaRef.value?.focus();
}

function openEditor() {
  try {
    editButtons.value = structuredClone(props.quickButtons);
  } catch (error) {
    // Fallback to compatible deep copy approach
    editButtons.value = props.quickButtons.map(btn => ({ ...btn }));
  }
  if (editButtons.value.length > 0) {
    selectedButtonId.value = editButtons.value[0].id;
  } else {
    selectedButtonId.value = null;
  }
  showEditor.value = true;
}

function saveEditor() {
  emit("buttonsChange", editButtons.value.filter((button) => button.label.trim() || button.command.trim()));
  showEditor.value = false;
}

function addButton() {
  const newId = crypto.randomUUID();
  editButtons.value = [...editButtons.value, {
    id: newId,
    label: "",
    command: "",
    toolbar: selectedToolbar.value || "Default",
    group: "General",
    hosts: [],
    sessionGroups: [],
    sendMode: "line",
  }];
  selectedButtonId.value = newId;
}

function updateButton<K extends keyof QuickButton>(id: string, field: K, value: QuickButton[K]) {
  editButtons.value = editButtons.value.map((button) => (
    button.id === id ? { ...button, [field]: value } : button
  ));
}

function parseScopes(value: string) {
  return value.split(",").map((item) => item.trim()).filter(Boolean);
}

function deleteButton(id: string) {
  const index = editButtons.value.findIndex(b => b.id === id);
  editButtons.value = editButtons.value.filter((button) => button.id !== id);
  if (selectedButtonId.value === id) {
    if (editButtons.value.length > 0) {
      const nextIndex = Math.min(index, editButtons.value.length - 1);
      selectedButtonId.value = editButtons.value[nextIndex].id;
    } else {
      selectedButtonId.value = null;
    }
  }
}

function moveButton(id: string, direction: -1 | 1) {
  const index = editButtons.value.findIndex((button) => button.id === id);
  if (index < 0) {
    return;
  }
  const nextIndex = index + direction;
  if (nextIndex < 0 || nextIndex >= editButtons.value.length) {
    return;
  }
  const nextButtons = [...editButtons.value];
  [nextButtons[index], nextButtons[nextIndex]] = [nextButtons[nextIndex], nextButtons[index]];
  editButtons.value = nextButtons;
}

function closeEditor() {
  showEditor.value = false;
}
</script>

<template>
  <div class="terminal-input-bar">
    <div
      class="terminal-input-resize-handle"
      :class="{ collapsed: textareaH === 0 }"
      :title="$t('inputBar.resizeHandle')"
      @mousedown="handleResizeMouseDown"
      @dblclick="handleResizeDblClick"
    >
      <span class="terminal-input-resize-grip" />
    </div>

    <div class="quick-buttons-bar">
      <select
        v-if="availableToolbars.length > 1"
        v-model="selectedToolbar"
        class="quick-toolbar-select"
        :title="$t('inputBar.snippetToolbar')"
      >
        <option v-for="toolbar in availableToolbars" :key="toolbar" :value="toolbar">{{ toolbar }}</option>
      </select>
      <span v-if="visibleButtons.length === 0" class="quick-buttons-hint">
        {{ $t('inputBar.noSnippets') }}
      </span>
      <template v-for="(button, index) in visibleButtons" :key="button.id">
        <span
          v-if="index === 0 || (visibleButtons[index - 1]?.group || 'General') !== (button.group || 'General')"
          class="quick-button-group-label"
        >{{ button.group || 'General' }}</span>
        <button
          class="quick-btn"
          type="button"
          :title="`${button.command}${button.sendMode === 'raw' ? ' (raw)' : ''}`"
          @click="handleQuickButton(button)"
        >
          {{ button.label.trim() || button.command.slice(0, 20) }}
        </button>
      </template>

      <button class="quick-btn quick-btn--edit" type="button" :title="$t('inputBar.editQuickButtons')" @click="openEditor">
        {{ $t('inputBar.edit') }}
      </button>
    </div>

    <div v-if="textareaH > 0" class="terminal-input-row" :class="{ 'ai-mode': aiMode }">
      <textarea
        ref="textareaRef"
        class="terminal-input-textarea"
        :style="{ height: `${textareaH}px` }"
        :value="text"
        :placeholder="aiMode ? $t('inputBar.ai.placeholder') : $t('inputBar.placeholder')"
        spellcheck="false"
        autocorrect="off"
        autocapitalize="off"
        @input="text = inputValue($event)"
        @keydown="handleKeyDown"
      />
      <div class="terminal-input-actions">
        <button
          v-if="aiAvailable"
          class="terminal-input-ai-btn"
          :class="{ active: aiMode }"
          type="button"
          :title="$t('inputBar.ai.toggleHint')"
          @click="toggleAiMode"
        >AI</button>
        <button
          v-if="aiMode"
          class="terminal-input-send-btn"
          type="button"
          :disabled="aiGenerating || !text.trim()"
          :title="$t('inputBar.ai.generate')"
          @click="generateCommand"
        >{{ aiGenerating ? '…' : '✨' }}</button>
        <button
          v-else
          class="terminal-input-send-btn"
          type="button"
          :title="$t('inputBar.send')"
          @click="doSend(text); text = ''"
        >▶</button>
      </div>
    </div>
    <div v-if="aiMode && textareaH > 0" class="terminal-input-ai-status">
      <span v-if="aiError" class="terminal-input-ai-error">{{ aiError }}</span>
      <span v-else class="terminal-input-ai-hint">{{ aiGenerating ? $t('inputBar.ai.generating') : $t('inputBar.ai.hint') }}</span>
    </div>

    <div v-if="showEditor" class="quick-btn-editor-overlay" @click="closeEditor">
      <div class="quick-btn-editor" @click.stop>
        <div class="quick-btn-editor-header">
          <span>{{ $t('inputBar.snippetLibrary') }}</span>
          <button type="button" class="quick-btn-editor-close" @click="closeEditor">×</button>
        </div>

        <div class="quick-btn-editor-body">
          <div class="quick-btn-editor-sidebar">
            <div
              v-for="(button, index) in editButtons"
              :key="button.id"
              class="quick-btn-editor-sidebar-item"
              :class="{ active: selectedButtonId === button.id }"
              @click="selectedButtonId = button.id"
            >
              <span class="quick-btn-editor-sidebar-label">
                {{ button.label.trim() || $t('inputBar.noLabel') }}
              </span>
              <div class="quick-btn-editor-sidebar-actions">
                <button type="button" :disabled="index === 0" :title="$t('inputBar.moveUp')" @click.stop="moveButton(button.id, -1)">▲</button>
                <button
                  type="button"
                  :disabled="index === editButtons.length - 1"
                  :title="$t('inputBar.moveDown')"
                  @click.stop="moveButton(button.id, 1)"
                >
                  ▼
                </button>
                <button type="button" class="quick-btn-editor-sidebar-delete" :title="$t('common.delete')" @click.stop="deleteButton(button.id)">×</button>
              </div>
            </div>
            <p v-if="editButtons.length === 0" class="quick-btn-editor-empty">
              {{ $t('inputBar.noButtons') }}
            </p>
          </div>

          <div class="quick-btn-editor-content">
            <template v-if="selectedButtonId">
              <div
                v-for="button in editButtons.filter(b => b.id === selectedButtonId)"
                :key="button.id"
                class="quick-btn-editor-detail"
              >
                <div class="quick-btn-editor-field">
                  <label>{{ $t('inputBar.label') }}</label>
                  <input
                    type="text"
                    class="quick-btn-editor-input quick-btn-editor-input--label"
                    :placeholder="$t('inputBar.displayName')"
                    :value="button.label"
                    @input="updateButton(button.id, 'label', inputValue($event))"
                  >
                </div>
                <div class="quick-btn-editor-field quick-btn-editor-field--command">
                  <label>{{ $t('inputBar.commandHint') }}</label>
                  <textarea
                    class="quick-btn-editor-input quick-btn-editor-input--command"
                    :placeholder="$t('inputBar.commandPlaceholder')"
                    :value="button.command"
                    autocapitalize="none"
                    autocorrect="off"
                    spellcheck="false"
                    @input="updateButton(button.id, 'command', inputValue($event))"
                  />
                </div>
                <div class="quick-btn-editor-fields-grid">
                  <div class="quick-btn-editor-field">
                    <label>{{ $t('inputBar.toolbar') }}</label>
                    <input class="quick-btn-editor-input" type="text" :value="button.toolbar || 'Default'" @input="updateButton(button.id, 'toolbar', inputValue($event))">
                  </div>
                  <div class="quick-btn-editor-field">
                    <label>{{ $t('inputBar.group') }}</label>
                    <input class="quick-btn-editor-input" type="text" :value="button.group || 'General'" @input="updateButton(button.id, 'group', inputValue($event))">
                  </div>
                  <div class="quick-btn-editor-field">
                    <label>{{ $t('inputBar.sshHosts') }}</label>
                    <input class="quick-btn-editor-input" type="text" :value="(button.hosts || []).join(', ')" :placeholder="$t('inputBar.allHosts')" @input="updateButton(button.id, 'hosts', parseScopes(inputValue($event)))">
                  </div>
                  <div class="quick-btn-editor-field">
                    <label>{{ $t('inputBar.connectionGroups') }}</label>
                    <input class="quick-btn-editor-input" type="text" :value="(button.sessionGroups || []).join(', ')" :placeholder="$t('inputBar.allGroups')" @input="updateButton(button.id, 'sessionGroups', parseScopes(inputValue($event)))">
                  </div>
                  <div class="quick-btn-editor-field">
                    <label>{{ $t('inputBar.sendMode') }}</label>
                    <select class="quick-btn-editor-input" :value="button.sendMode || 'line'" @change="updateButton(button.id, 'sendMode', inputValue($event) === 'raw' ? 'raw' : 'line')">
                      <option value="line">{{ $t('inputBar.sendModeLine') }}</option>
                      <option value="raw">{{ $t('inputBar.sendModeRaw') }}</option>
                    </select>
                  </div>
                </div>
              </div>
            </template>
            <div v-else class="quick-btn-editor-content-empty">
              {{ $t('inputBar.selectButtonPre') }}<strong>{{ $t('inputBar.addLabel') }}</strong>{{ $t('inputBar.selectButtonPost') }}
            </div>
          </div>
        </div>

        <div class="quick-btn-editor-footer">
          <button type="button" class="quick-btn-editor-add" @click="addButton">{{ $t('inputBar.addLabel') }}</button>
          <span style="flex: 1" />
          <button type="button" class="quick-btn-editor-cancel" @click="closeEditor">{{ $t('common.cancel') }}</button>
          <button type="button" class="quick-btn-editor-save" @click="saveEditor">{{ $t('common.save') }}</button>
        </div>
      </div>
    </div>
  </div>
</template>
