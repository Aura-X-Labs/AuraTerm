import { reactive } from "vue";

export interface PromptRequest {
  message: string;
  defaultValue: string;
  resolve: (value: string | null) => void;
}

/** FIFO of open prompt requests; PromptDialogHost renders the head. */
export const promptQueue = reactive<PromptRequest[]>([]);

/** In-app replacement for window.prompt (a silent no-op in the macOS
 * WebView): resolves with the entered text, or null on cancel. */
export function promptText(message: string, defaultValue = ""): Promise<string | null> {
  return new Promise((resolve) => {
    promptQueue.push({ message, defaultValue, resolve });
  });
}
