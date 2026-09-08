/**
 * Automatic configuration sync (design docs/plans/sync-passphrase-removal-design.md §7.3).
 *
 * With no passphrase left to type, a sync can run unattended. The schedule:
 *
 * - **startup** — one two-way sync shortly after the app is up and signed in;
 * - **local change** — bookmarks or synced settings changed: push after a
 *   debounce, so a burst of edits becomes one upload;
 * - **periodic** — a two-way sync at a fixed interval to pick up other devices.
 *
 * Credential changes never trigger a run on their own (the master password
 * may be locked); they ride along with the next scheduled or manual sync.
 *
 * Pure scheduling: the caller supplies `enabled()` (signed in + switched on)
 * and `run()` (the actual sync, which must never throw into here). Timers use
 * the global `setTimeout`, so tests drive it with fake timers.
 */

export const BOOKMARKS_CHANGED_EVENT = "auraterm:bookmarks-changed";

export interface AutoSyncOptions {
  enabled: () => boolean;
  run: (reason: AutoSyncReason) => Promise<void> | void;
  startupDelayMs?: number;
  changeDebounceMs?: number;
  intervalMs?: number;
}

export type AutoSyncReason = "startup" | "change" | "interval";

export interface AutoSync {
  /** Begin the schedule (idempotent). */
  start(): void;
  /** Cancel every pending run. */
  stop(): void;
  /** Local bookmarks / settings changed. */
  notifyChange(): void;
  /** `enabled()` flipped: start or stop accordingly. */
  refresh(): void;
  /** For tests and diagnostics. */
  readonly pending: { startup: boolean; change: boolean; interval: boolean };
}

export const AUTO_SYNC_DEFAULTS = {
  startupDelayMs: 10_000,
  changeDebounceMs: 30_000,
  intervalMs: 30 * 60_000,
} as const;

export function createAutoSync(options: AutoSyncOptions): AutoSync {
  const startupDelayMs = options.startupDelayMs ?? AUTO_SYNC_DEFAULTS.startupDelayMs;
  const changeDebounceMs = options.changeDebounceMs ?? AUTO_SYNC_DEFAULTS.changeDebounceMs;
  const intervalMs = options.intervalMs ?? AUTO_SYNC_DEFAULTS.intervalMs;

  let startupTimer: ReturnType<typeof setTimeout> | null = null;
  let changeTimer: ReturnType<typeof setTimeout> | null = null;
  let intervalTimer: ReturnType<typeof setInterval> | null = null;
  let running = false;
  let started = false;
  let rerun: AutoSyncReason | null = null;

  async function fire(reason: AutoSyncReason) {
    if (!options.enabled()) return;
    if (running) {
      // Coalesce: one more run after the current one, not one per trigger.
      rerun = reason;
      return;
    }
    running = true;
    try {
      await options.run(reason);
    } catch {
      // The runner reports its own failures; the schedule keeps going.
    } finally {
      running = false;
    }
    if (rerun) {
      const next = rerun;
      rerun = null;
      void fire(next);
    }
  }

  function clearTimers() {
    if (startupTimer) clearTimeout(startupTimer);
    if (changeTimer) clearTimeout(changeTimer);
    if (intervalTimer) clearInterval(intervalTimer);
    startupTimer = changeTimer = null;
    intervalTimer = null;
  }

  function start() {
    if (started) return;
    started = true;
    if (!options.enabled()) return;
    startupTimer = setTimeout(() => {
      startupTimer = null;
      void fire("startup");
    }, startupDelayMs);
    intervalTimer = setInterval(() => { void fire("interval"); }, intervalMs);
  }

  function stop() {
    started = false;
    clearTimers();
  }

  function notifyChange() {
    if (!started || !options.enabled()) return;
    if (changeTimer) clearTimeout(changeTimer);
    changeTimer = setTimeout(() => {
      changeTimer = null;
      void fire("change");
    }, changeDebounceMs);
  }

  function refresh() {
    const wanted = options.enabled();
    const active = intervalTimer !== null;
    if (wanted && !active) {
      started = false;
      start();
    } else if (!wanted && active) {
      clearTimers();
      // Stay "started" so a later refresh() can resume without a new start().
    }
  }

  return {
    start,
    stop,
    notifyChange,
    refresh,
    get pending() {
      return { startup: startupTimer !== null, change: changeTimer !== null, interval: intervalTimer !== null };
    },
  };
}

/** Tell the auto-sync schedule (and anyone else listening) that bookmarks changed. */
export function notifyBookmarksChanged(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(BOOKMARKS_CHANGED_EVENT));
}
