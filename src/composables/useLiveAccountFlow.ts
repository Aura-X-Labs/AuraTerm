import { ref } from "vue";

export type LiveAccountTarget = "console" | "share" | "relay" | "sync";

/** One trip through the account dialog. Never keep an enable intent after
 * cancellation, failed refresh, or navigation to another account operation. */
export function useLiveAccountFlow(options: {
  refresh: () => Promise<boolean>;
  resume: (target: LiveAccountTarget) => void;
}) {
  const showAccount = ref(false);
  const returnTarget = ref<LiveAccountTarget | null>(null);
  let generation = 0;

  function open(target: LiveAccountTarget | null = null) {
    generation += 1;
    returnTarget.value = target;
    showAccount.value = true;
  }

  function abandon() {
    generation += 1;
    returnTarget.value = null;
    showAccount.value = false;
  }

  async function close(completed = false) {
    const target = returnTarget.value;
    abandon();
    const version = generation;
    // Return to retained forms even if login was cancelled or refresh fails.
    if (target && target !== "console") options.resume(target);
    try {
      const ready = await options.refresh();
      if (version === generation && completed && ready && target === "console") options.resume(target);
    } catch {
      // A failed read must never enable sharing from a stale enrolled flag.
    }
  }

  return { showAccount, returnTarget, open, close, abandon };
}
