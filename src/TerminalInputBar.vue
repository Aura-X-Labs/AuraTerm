<script setup lang="ts">
import { ref } from "vue";
import type { QuickButton } from "./settings";

const props = defineProps<{
  quickButtons: QuickButton[];
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
const textareaH = ref(DEFAULT_TEXTAREA_H);
const textareaRef = ref<HTMLTextAreaElement | null>(null);
let dragStartY = 0;
let dragStartH = 0;

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

function handleKeyDown(event: KeyboardEvent) {
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
  editButtons.value = structuredClone(props.quickButtons);
  showEditor.value = true;
}

function saveEditor() {
  emit("buttonsChange", editButtons.value.filter((button) => button.label.trim() || button.command.trim()));
  showEditor.value = false;
}

function addButton() {
  editButtons.value = [...editButtons.value, { id: crypto.randomUUID(), label: "", command: "" }];
}

function updateButton(id: string, field: "label" | "command", value: string) {
  editButtons.value = editButtons.value.map((button) => (
    button.id === id ? { ...button, [field]: value } : button
  ));
}

function deleteButton(id: string) {
  editButtons.value = editButtons.value.filter((button) => button.id !== id);
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
        placeholder="Type here…  Ctrl+Enter (⌘+Enter on macOS) to send"
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
          <p v-if="editButtons.length === 0" class="quick-btn-editor-empty">
            No buttons yet — click <strong>+ Add</strong> to create one.
          </p>

          <div v-for="(button, index) in editButtons" :key="button.id" class="quick-btn-editor-row">
            <div class="quick-btn-editor-order">
              <button type="button" :disabled="index === 0" title="Move up" @click="moveButton(button.id, -1)">▲</button>
              <button
                type="button"
                :disabled="index === editButtons.length - 1"
                title="Move down"
                @click="moveButton(button.id, 1)"
              >
                ▼
              </button>
            </div>
            <input
              type="text"
              class="quick-btn-editor-input quick-btn-editor-input--label"
              placeholder="Label"
              :value="button.label"
              @input="updateButton(button.id, 'label', inputValue($event))"
            >
            <input
              type="text"
              class="quick-btn-editor-input quick-btn-editor-input--command"
              placeholder="Command  (e.g.  ls -la)"
              :value="button.command"
              @input="updateButton(button.id, 'command', inputValue($event))"
            >
            <button type="button" class="quick-btn-editor-delete" title="Delete" @click="deleteButton(button.id)">×</button>
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