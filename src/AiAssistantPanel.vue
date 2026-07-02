<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { t } from "./i18n";
import {
  aiChatCancel,
  aiChatStart,
  listenAiStream,
  type AiChatMessage,
  type AiStreamEventPayload,
} from "./ai";
import { terminalSystemPrompt } from "./aiPrompts";
import type { AiConfig } from "./settings";
import type { UnlistenFn } from "@tauri-apps/api/event";

const props = defineProps<{
  config: AiConfig;
  hasApiKey: boolean;
  env: { os?: string; shell?: string };
  /** Draft text to seed the composer with (e.g. an "explain this command"
   *  prompt). The user reviews and edits it before sending. */
  draft?: string;
  /** Bumped by the parent each time a new draft should be seeded, so re-seeding
   *  the identical prompt string still triggers. */
  draftKey?: number;
}>();

const emit = defineEmits<{
  close: [];
  insertCommand: [text: string];
  openSettings: [];
}>();

interface ChatEntry {
  role: "user" | "assistant";
  content: string;
  /** Assistant entry still receiving stream deltas. */
  streaming?: boolean;
  error?: boolean;
}

const messages = ref<ChatEntry[]>([]);
const composer = ref("");
const activeRequestId = ref<string | null>(null);
const errorBanner = ref("");
const lastUsage = ref<{ input?: number; output?: number } | null>(null);
const scrollRef = ref<HTMLDivElement | null>(null);
const composerRef = ref<HTMLTextAreaElement | null>(null);

const isStreaming = computed(() => activeRequestId.value !== null);
const canSend = computed(
  () => props.config.enabled && props.hasApiKey && !isStreaming.value && composer.value.trim().length > 0,
);

let unlisten: UnlistenFn | null = null;
void listenAiStream(handleStreamEvent).then((fn) => {
  unlisten = fn;
});

function scrollToBottom() {
  void nextTick(() => {
    const el = scrollRef.value;
    if (el) el.scrollTop = el.scrollHeight;
  });
}

function handleStreamEvent(payload: AiStreamEventPayload) {
  if (payload.requestId !== activeRequestId.value) return;
  const current = messages.value[messages.value.length - 1];
  if (!current || current.role !== "assistant") return;

  switch (payload.kind) {
    case "delta":
      current.content += payload.text ?? "";
      scrollToBottom();
      break;
    case "done":
      current.streaming = false;
      lastUsage.value = { input: payload.inputTokens, output: payload.outputTokens };
      activeRequestId.value = null;
      if (!current.content.trim()) {
        current.content = t("ai.emptyReply");
        current.error = true;
      }
      break;
    case "error":
      current.streaming = false;
      current.error = true;
      current.content = current.content
        ? `${current.content}\n\n[${payload.message ?? t("ai.errorGeneric")}]`
        : `[${payload.message ?? t("ai.errorGeneric")}]`;
      activeRequestId.value = null;
      break;
    case "cancelled":
      current.streaming = false;
      if (!current.content.trim()) {
        current.content = t("ai.cancelled");
        current.error = true;
      }
      activeRequestId.value = null;
      break;
  }
}

async function send() {
  const text = composer.value.trim();
  if (!text || isStreaming.value) return;
  if (!props.config.enabled || !props.hasApiKey) {
    errorBanner.value = t("ai.notConfigured");
    return;
  }
  errorBanner.value = "";
  composer.value = "";

  messages.value.push({ role: "user", content: text });
  const history: AiChatMessage[] = messages.value
    .filter((entry) => !entry.error)
    .map((entry) => ({ role: entry.role, content: entry.content }));
  messages.value.push({ role: "assistant", content: "", streaming: true });
  scrollToBottom();

  try {
    const system = terminalSystemPrompt({ os: props.env.os, shell: props.env.shell });
    activeRequestId.value = await aiChatStart(history, system);
  } catch (error) {
    const current = messages.value[messages.value.length - 1];
    if (current) {
      current.streaming = false;
      current.error = true;
      current.content = `[${String(error)}]`;
    }
    activeRequestId.value = null;
  }
}

function stop() {
  const requestId = activeRequestId.value;
  if (requestId) void aiChatCancel(requestId);
}

function clearConversation() {
  if (isStreaming.value) stop();
  messages.value = [];
  lastUsage.value = null;
  errorBanner.value = "";
}

function onComposerKeydown(event: KeyboardEvent) {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    void send();
  }
}

/** Split assistant text into plain-text and fenced-code segments for rendering
 *  without dangerouslySetInnerHTML — Vue escapes each segment as a text node. */
interface Segment {
  type: "text" | "code";
  content: string;
  lang?: string;
}
function parseSegments(content: string): Segment[] {
  const segments: Segment[] = [];
  const fence = /```([^\n`]*)\n?([\s\S]*?)```/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = fence.exec(content)) !== null) {
    if (match.index > lastIndex) {
      segments.push({ type: "text", content: content.slice(lastIndex, match.index) });
    }
    segments.push({ type: "code", lang: match[1].trim() || undefined, content: match[2].replace(/\n$/, "") });
    lastIndex = fence.lastIndex;
  }
  if (lastIndex < content.length) {
    segments.push({ type: "text", content: content.slice(lastIndex) });
  }
  return segments.length ? segments : [{ type: "text", content }];
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    /* clipboard blocked — non-fatal */
  }
}

function applyDraft(draft: string | undefined) {
  if (!draft) return;
  composer.value = draft;
  void nextTick(() => {
    composerRef.value?.focus();
    const el = composerRef.value;
    if (el) el.setSelectionRange(el.value.length, el.value.length);
  });
}

// A new draft (from an "explain command" action) seeds the composer for review.
// Keyed on draftKey so re-issuing the same prompt string still re-seeds.
watch(() => props.draftKey, () => applyDraft(props.draft), { immediate: true });

onBeforeUnmount(() => {
  if (activeRequestId.value) void aiChatCancel(activeRequestId.value);
  unlisten?.();
});
</script>

<template>
  <aside class="ai-panel">
    <div class="ai-panel-header">
      <div>
        <div class="ai-panel-title">{{ $t('ai.title') }}</div>
        <div class="ai-panel-subtitle">
          {{ config.provider === 'anthropic' ? 'Anthropic' : $t('ai.providerOpenAiShort') }} · {{ config.model || '—' }}
        </div>
      </div>
      <div class="ai-panel-header-actions">
        <button class="ai-icon-btn" type="button" :title="$t('ai.clear')" :disabled="!messages.length" @click="clearConversation">⟲</button>
        <button class="ai-icon-btn" type="button" :title="$t('common.close')" @click="emit('close')">×</button>
      </div>
    </div>

    <div v-if="!config.enabled || !hasApiKey" class="ai-panel-setup">
      <p>{{ $t('ai.notConfigured') }}</p>
      <button class="ai-primary-btn" type="button" @click="emit('openSettings')">{{ $t('ai.openSettings') }}</button>
    </div>

    <template v-else>
      <div ref="scrollRef" class="ai-panel-messages">
        <p v-if="!messages.length" class="ai-panel-empty">{{ $t('ai.emptyState') }}</p>

        <div v-for="(entry, index) in messages" :key="index" class="ai-message" :class="entry.role">
          <div class="ai-message-role">{{ entry.role === 'user' ? $t('ai.roleYou') : $t('ai.roleAssistant') }}</div>
          <div class="ai-message-body" :class="{ error: entry.error }">
            <template v-for="(segment, segmentIndex) in parseSegments(entry.content)" :key="segmentIndex">
              <div v-if="segment.type === 'code'" class="ai-code-block">
                <div class="ai-code-actions">
                  <span class="ai-code-lang">{{ segment.lang || 'sh' }}</span>
                  <button class="ai-code-btn" type="button" @click="copyText(segment.content)">{{ $t('common.copy') }}</button>
                  <button class="ai-code-btn" type="button" @click="emit('insertCommand', segment.content)">{{ $t('ai.insert') }}</button>
                </div>
                <pre><code>{{ segment.content }}</code></pre>
              </div>
              <span v-else class="ai-text-segment">{{ segment.content }}</span>
            </template>
            <span v-if="entry.streaming" class="ai-caret">▋</span>
          </div>
        </div>
      </div>

      <div v-if="lastUsage && (lastUsage.input || lastUsage.output)" class="ai-usage">
        {{ $t('ai.usage', { input: lastUsage.input ?? 0, output: lastUsage.output ?? 0 }) }}
      </div>
      <div v-if="errorBanner" class="ai-error-banner">{{ errorBanner }}</div>

      <div class="ai-composer">
        <textarea
          ref="composerRef"
          v-model="composer"
          class="ai-composer-input"
          :placeholder="$t('ai.composerPlaceholder')"
          rows="3"
          @keydown="onComposerKeydown"
        />
        <div class="ai-composer-actions">
          <span class="ai-composer-hint">{{ $t('ai.sendHint') }}</span>
          <button v-if="isStreaming" class="ai-primary-btn stop" type="button" @click="stop">{{ $t('ai.stop') }}</button>
          <button v-else class="ai-primary-btn" type="button" :disabled="!canSend" @click="send">{{ $t('ai.send') }}</button>
        </div>
      </div>
    </template>
  </aside>
</template>

<style scoped>
.ai-panel {
  width: 420px;
  max-width: 42vw;
  min-width: 340px;
  display: flex;
  flex-direction: column;
  background: var(--app-terminal-gradient);
  border-left: 1px solid var(--app-border);
  color: var(--app-text);
}

.ai-panel-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 16px 12px;
  border-bottom: 1px solid var(--app-border);
}

.ai-panel-title {
  font-size: 14px;
  font-weight: 700;
}

.ai-panel-subtitle {
  margin-top: 4px;
  font-size: 11px;
  color: var(--app-text-muted);
  word-break: break-all;
}

.ai-panel-header-actions {
  display: flex;
  gap: 4px;
}

.ai-icon-btn {
  width: 28px;
  height: 28px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--app-text-muted);
  font-size: 16px;
  cursor: pointer;
}

.ai-icon-btn:hover:not(:disabled) {
  background: var(--app-hover);
  color: var(--app-text);
}

.ai-icon-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.ai-panel-setup {
  padding: 24px 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  align-items: flex-start;
  color: var(--app-text-muted);
  font-size: 13px;
}

.ai-panel-messages {
  flex: 1;
  overflow-y: auto;
  padding: 12px 16px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.ai-panel-empty {
  color: var(--app-text-muted);
  font-size: 13px;
  text-align: center;
  margin-top: 24px;
}

.ai-message {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.ai-message-role {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--app-text-muted);
}

.ai-message-body {
  font-size: 13px;
  line-height: 1.5;
  white-space: normal;
  word-break: break-word;
}

.ai-message.user .ai-message-body {
  padding: 8px 10px;
  border-radius: 8px;
  background: var(--app-hover);
}

.ai-message-body.error {
  color: var(--app-danger, #e06c75);
}

.ai-text-segment {
  white-space: pre-wrap;
}

.ai-code-block {
  margin: 8px 0;
  border: 1px solid var(--app-border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--app-terminal-bg, rgba(0, 0, 0, 0.35));
}

.ai-code-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
  border-bottom: 1px solid var(--app-border);
}

.ai-code-lang {
  flex: 1;
  font-size: 10px;
  color: var(--app-text-muted);
}

.ai-code-btn {
  border: none;
  background: transparent;
  color: var(--app-accent, #61afef);
  font-size: 11px;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
}

.ai-code-btn:hover {
  background: var(--app-hover);
}

.ai-code-block pre {
  margin: 0;
  padding: 8px 10px;
  overflow-x: auto;
  font-size: 12px;
}

.ai-code-block code {
  font-family: var(--app-mono, monospace);
}

.ai-caret {
  animation: ai-blink 1s step-start infinite;
}

@keyframes ai-blink {
  50% { opacity: 0; }
}

.ai-usage {
  padding: 4px 16px;
  font-size: 10px;
  color: var(--app-text-muted);
}

.ai-error-banner {
  margin: 0 16px 8px;
  padding: 6px 10px;
  border-radius: 6px;
  background: color-mix(in srgb, var(--app-danger, #e06c75) 18%, transparent);
  color: var(--app-danger, #e06c75);
  font-size: 12px;
}

.ai-composer {
  border-top: 1px solid var(--app-border);
  padding: 10px 16px 14px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.ai-composer-input {
  width: 100%;
  resize: vertical;
  min-height: 52px;
  padding: 8px 10px;
  border: 1px solid var(--app-border);
  border-radius: 8px;
  background: var(--app-input-bg, rgba(0, 0, 0, 0.2));
  color: var(--app-text);
  font-family: inherit;
  font-size: 13px;
  box-sizing: border-box;
}

.ai-composer-input:focus {
  outline: none;
  border-color: var(--app-accent, #61afef);
}

.ai-composer-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.ai-composer-hint {
  font-size: 11px;
  color: var(--app-text-muted);
}

.ai-primary-btn {
  border: none;
  border-radius: 6px;
  padding: 6px 14px;
  background: var(--app-accent, #61afef);
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.ai-primary-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.ai-primary-btn.stop {
  background: var(--app-danger, #e06c75);
}
</style>
