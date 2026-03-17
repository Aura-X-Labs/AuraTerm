<script setup lang="ts">
import { ref, watch } from "vue";
import type { QuickButton } from "./settings";

const props = defineProps<{
  quickButtons: QuickButton[];
  inputHistory: string[];
}>();

const emit = defineEmits<{
  send: [text: string];
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
let dragStartY = 0;
let dragStartH = 0;

// History navigation state
const historyIndex = ref(-1);
const savedTextBeforeNav = ref("");

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

function doSend(payload: string) {
  if (!payload.trim()) {
    return;
  }
  emit("send", payload.endsWith("\n") ? payload : `${payload}\n`);
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

function handleQuickButton(button: QuickButton) {
  doSend(button.command);
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
  editButtons.value = [...editButtons.value, { id: newId, label: "", command: "" }];
  selectedButtonId.value = newId;
}

function updateButton(id: string, field: "label" | "command", value: string) {
  editButtons.value = editButtons.value.map((button) => (
    button.id === id ? { ...button, [field]: value } : button
  ));
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
      title="Drag to resize, double-click to collapse/expand"
      @mousedown="handleResizeMouseDown"
      @dblclick="handleResizeDblClick"
    >
      <span class="terminal-input-resize-grip" />
    </div>

    <div class="quick-buttons-bar">
      <span v-if="quickButtons.length === 0" class="quick-buttons-hint">
        No quick buttons yet. Click Edit to add.
      </span>
      <button
        v-for="button in quickButtons"
        :key="button.id"
        class="quick-btn"
        type="button"
        :title="button.command"
        @click="handleQuickButton(button)"
      >
        {{ button.label.trim() || button.command.slice(0, 20) }}
      </button>

      <button class="quick-btn quick-btn--edit" type="button" title="Edit quick buttons" @click="openEditor">
        ✎ Edit
      </button>
    </div>

    <div v-if="textareaH > 0" class="terminal-input-row">
      <textarea
        ref="textareaRef"
        class="terminal-input-textarea"
        :style="{ height: `${textareaH}px` }"
        :value="text"
        placeholder="Type here…  Ctrl+Enter to send  ·  PgUp/↑ for history"
        spellcheck="false"
        autocorrect="off"
        autocapitalize="off"
        @input="text = inputValue($event)"
        @keydown="handleKeyDown"
      />
      <button class="terminal-input-send-btn" type="button" title="Send  (Ctrl+Enter)" @click="doSend(text); text = ''">▶</button>
    </div>

    <div v-if="showEditor" class="quick-btn-editor-overlay" @click="closeEditor">
      <div class="quick-btn-editor" @click.stop>
        <div class="quick-btn-editor-header">
          <span>Edit Quick Buttons</span>
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
                {{ button.label.trim() || '(No Label)' }}
              </span>
              <div class="quick-btn-editor-sidebar-actions">
                <button type="button" :disabled="index === 0" title="Move up" @click.stop="moveButton(button.id, -1)">▲</button>
                <button
                  type="button"
                  :disabled="index === editButtons.length - 1"
                  title="Move down"
                  @click.stop="moveButton(button.id, 1)"
                >
                  ▼
                </button>
                <button type="button" class="quick-btn-editor-sidebar-delete" title="Delete" @click.stop="deleteButton(button.id)">×</button>
              </div>
            </div>
            <p v-if="editButtons.length === 0" class="quick-btn-editor-empty">
              No buttons yet.
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
                  <label>Label</label>
                  <input
                    type="text"
                    class="quick-btn-editor-input quick-btn-editor-input--label"
                    placeholder="Display name"
                    :value="button.label"
                    @input="updateButton(button.id, 'label', inputValue($event))"
                  >
                </div>
                <div class="quick-btn-editor-field quick-btn-editor-field--command">
                  <label>Command</label>
                  <textarea
                    class="quick-btn-editor-input quick-btn-editor-input--command"
                    placeholder="Command to send (e.g. ls -la)"
                    :value="button.command"
                    autocapitalize="none"
                    autocorrect="off"
                    spellcheck="false"
                    @input="updateButton(button.id, 'command', inputValue($event))"
                  />
                </div>
              </div>
            </template>
            <div v-else class="quick-btn-editor-content-empty">
              Select a button to edit or click <strong>+ Add</strong> to create one.
            </div>
          </div>
        </div>

        <div class="quick-btn-editor-footer">
          <button type="button" class="quick-btn-editor-add" @click="addButton">+ Add</button>
          <span style="flex: 1" />
          <button type="button" class="quick-btn-editor-cancel" @click="closeEditor">Cancel</button>
          <button type="button" class="quick-btn-editor-save" @click="saveEditor">Save</button>
        </div>
      </div>
    </div>
  </div>
</template>