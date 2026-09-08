import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BOOKMARKS_CHANGED_EVENT, createAutoSync, notifyBookmarksChanged, type AutoSyncReason } from "../composables/useAutoSync";

describe("auto sync schedule (design §7.3)", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  function harness(enabled = true) {
    const runs: AutoSyncReason[] = [];
    let on = enabled;
    const auto = createAutoSync({
      enabled: () => on,
      run: (reason) => { runs.push(reason); },
      startupDelayMs: 100,
      changeDebounceMs: 50,
      intervalMs: 1000,
    });
    return { auto, runs, setEnabled: (value: boolean) => { on = value; } };
  }

  it("runs once after startup, then on the interval", async () => {
    const { auto, runs } = harness();
    auto.start();
    expect(auto.pending).toEqual({ startup: true, change: false, interval: true });
    await vi.advanceTimersByTimeAsync(99);
    expect(runs).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(runs).toEqual(["startup"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(runs).toEqual(["startup", "interval"]);
    auto.stop();
    await vi.advanceTimersByTimeAsync(5000);
    expect(runs).toEqual(["startup", "interval"]);
  });

  it("debounces a burst of local changes into one push", async () => {
    const { auto, runs } = harness();
    auto.start();
    await vi.advanceTimersByTimeAsync(100);
    auto.notifyChange();
    await vi.advanceTimersByTimeAsync(30);
    auto.notifyChange();
    await vi.advanceTimersByTimeAsync(30);
    expect(runs).toEqual(["startup"]);
    await vi.advanceTimersByTimeAsync(20);
    expect(runs).toEqual(["startup", "change"]);
  });

  it("does nothing while disabled and resumes on refresh", async () => {
    const { auto, runs, setEnabled } = harness(false);
    auto.start();
    auto.notifyChange();
    await vi.advanceTimersByTimeAsync(2000);
    expect(runs).toEqual([]);
    expect(auto.pending.interval).toBe(false);

    setEnabled(true);
    auto.refresh();
    await vi.advanceTimersByTimeAsync(100);
    expect(runs).toEqual(["startup"]);

    setEnabled(false);
    auto.refresh();
    auto.notifyChange();
    await vi.advanceTimersByTimeAsync(2000);
    expect(runs).toEqual(["startup"]);
  });

  it("never overlaps runs; a trigger during a run queues exactly one more", async () => {
    const order: string[] = [];
    let release: (() => void) | null = null;
    const auto = createAutoSync({
      enabled: () => true,
      run: (reason) => new Promise<void>((resolve) => {
        order.push(`start:${reason}`);
        release = () => { order.push(`end:${reason}`); resolve(); };
      }),
      startupDelayMs: 10, changeDebounceMs: 10, intervalMs: 10_000,
    });
    auto.start();
    await vi.advanceTimersByTimeAsync(10);
    expect(order).toEqual(["start:startup"]);
    auto.notifyChange();
    await vi.advanceTimersByTimeAsync(10);
    auto.notifyChange();
    await vi.advanceTimersByTimeAsync(10);
    // Still inside the first run: nothing else started.
    expect(order).toEqual(["start:startup"]);
    release!();
    await vi.advanceTimersByTimeAsync(0);
    expect(order).toEqual(["start:startup", "end:startup", "start:change"]);
    release!();
    await vi.advanceTimersByTimeAsync(0);
    expect(order).toEqual(["start:startup", "end:startup", "start:change", "end:change"]);
    auto.stop();
  });

  it("keeps the schedule alive when a run throws", async () => {
    let calls = 0;
    const auto = createAutoSync({
      enabled: () => true,
      run: () => { calls += 1; throw new Error("boom"); },
      startupDelayMs: 10, changeDebounceMs: 10, intervalMs: 100,
    });
    auto.start();
    await vi.advanceTimersByTimeAsync(10);
    await vi.advanceTimersByTimeAsync(100);
    expect(calls).toBe(2);
    auto.stop();
  });

  it("broadcasts bookmark changes as a window event", () => {
    const seen = vi.fn();
    window.addEventListener(BOOKMARKS_CHANGED_EVENT, seen);
    notifyBookmarksChanged();
    expect(seen).toHaveBeenCalledTimes(1);
    window.removeEventListener(BOOKMARKS_CHANGED_EVENT, seen);
  });
});
