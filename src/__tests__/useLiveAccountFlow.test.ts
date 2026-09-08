import { describe, expect, it, vi } from "vitest";
import { useLiveAccountFlow } from "../composables/useLiveAccountFlow";

describe("Live account return flow", () => {
  it.each(["share", "relay", "sync"] as const)("returns to %s after success or cancellation without enabling Console", async (target) => {
    const resume = vi.fn();
    const refresh = vi.fn().mockResolvedValue(false);
    const flow = useLiveAccountFlow({ refresh, resume });
    flow.open(target);
    await flow.close();
    expect(resume).toHaveBeenCalledExactlyOnceWith(target);
    expect(flow.returnTarget.value).toBeNull();
    expect(flow.showAccount.value).toBe(false);
    refresh.mockResolvedValue(true);
    flow.open(target);
    await flow.close(true);
    expect(resume.mock.calls).toEqual([[target], [target]]);
  });

  it("enables Console once after a successful login and fresh device verification", async () => {
    const resume = vi.fn();
    const flow = useLiveAccountFlow({ refresh: vi.fn().mockResolvedValue(true), resume });
    flow.open("console");
    await flow.close(true);
    await flow.close(true);
    expect(resume).toHaveBeenCalledExactlyOnceWith("console");
  });

  it("does not enable Console after cancellation even if an in-flight login bound the device", async () => {
    const resume = vi.fn();
    const flow = useLiveAccountFlow({ refresh: vi.fn().mockResolvedValue(true), resume });
    flow.open("console");
    await flow.close();
    flow.open();
    await flow.close(true);
    expect(resume).not.toHaveBeenCalled();
  });

  it("drops Console intent after a failed or unready refresh", async () => {
    const resume = vi.fn();
    const refresh = vi.fn().mockRejectedValueOnce(new Error("IPC failed")).mockResolvedValueOnce(false).mockResolvedValue(true);
    const flow = useLiveAccountFlow({ refresh, resume });
    flow.open("console");
    await flow.close(true);
    flow.open("console");
    await flow.close(true);
    flow.open();
    await flow.close(true);
    expect(resume).not.toHaveBeenCalled();
  });

  it("ignores completion from a previous trip after another account action opens", async () => {
    let resolve!: (ready: boolean) => void;
    const refresh = () => new Promise<boolean>((done) => { resolve = done; });
    const resume = vi.fn();
    const flow = useLiveAccountFlow({ refresh, resume });
    flow.open("console");
    const closing = flow.close(true);
    flow.open("relay");
    resolve(true);
    await closing;
    expect(resume).not.toHaveBeenCalled();
    expect(flow.showAccount.value).toBe(true);
    expect(flow.returnTarget.value).toBe("relay");
  });

  it("still returns to a retained form when account refresh fails", async () => {
    const resume = vi.fn();
    const flow = useLiveAccountFlow({ refresh: vi.fn().mockRejectedValue(new Error("offline")), resume });
    flow.open("share");
    await flow.close();
    expect(resume).toHaveBeenCalledExactlyOnceWith("share");
  });
});
