/**
 * Thin typed IPC layer for the AI assistant backend (`src-tauri/src/ai.rs`).
 * A chat request is started with {@link aiChatStart}; the reply arrives as
 * `ai-stream` events carrying the returned request id.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface AiChatMessage {
  role: "user" | "assistant";
  content: string;
}

export interface AiStreamEventPayload {
  requestId: string;
  kind: "delta" | "done" | "error" | "cancelled";
  text?: string;
  message?: string;
  inputTokens?: number;
  outputTokens?: number;
}

/** Start a streaming chat; resolves to the request id the events will carry. */
export function aiChatStart(messages: AiChatMessage[], system?: string): Promise<string> {
  return invoke<string>("ai_chat_start", { messages, system: system ?? null });
}

export function aiChatCancel(requestId: string): Promise<void> {
  return invoke("ai_chat_cancel", { requestId });
}

/** One-shot, non-streaming completion — resolves to the full assistant text. */
export function aiComplete(messages: AiChatMessage[], system?: string): Promise<string> {
  return invoke<string>("ai_complete", { messages, system: system ?? null });
}

export function aiTestConnection(): Promise<void> {
  return invoke("ai_test_connection");
}

export function aiSetApiKey(apiKey: string): Promise<void> {
  return invoke("ai_set_api_key", { apiKey });
}

export function aiClearApiKey(): Promise<void> {
  return invoke("ai_clear_api_key");
}

export function aiHasApiKey(): Promise<boolean> {
  return invoke<boolean>("ai_has_api_key");
}

/** Subscribe to the global AI stream; the caller filters by request id. */
export function listenAiStream(
  handler: (payload: AiStreamEventPayload) => void,
): Promise<UnlistenFn> {
  return listen<AiStreamEventPayload>("ai-stream", (event) => handler(event.payload));
}
