import { beforeEach, describe, expect, it, vi } from "vitest";

import { alertDialog, confirmDialog } from "../nativeDialogs";

const plugin = vi.hoisted(() => ({ confirm: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => plugin);

beforeEach(() => {
  plugin.confirm.mockReset();
  plugin.message.mockReset();
  plugin.message.mockResolvedValue(undefined);
  vi.spyOn(console, "error").mockImplementation(() => {});
});

describe("confirmDialog", () => {
  it("passes the user's answer through", async () => {
    plugin.confirm.mockResolvedValue(true);
    await expect(confirmDialog("Delete it?")).resolves.toBe(true);

    plugin.confirm.mockResolvedValue(false);
    await expect(confirmDialog("Delete it?")).resolves.toBe(false);
  });

  /* A dialog bridge that rejects — e.g. a frontend plugin build calling a
   * command the Rust plugin no longer registers — used to reject out of every
   * caller's un-awaited handler, so destructive actions looked like buttons
   * that simply did nothing. */
  it("reports the failure and declines when the dialog cannot be shown", async () => {
    plugin.confirm.mockRejectedValue(new Error("dialog.confirm not allowed"));

    await expect(confirmDialog("Delete it?")).resolves.toBe(false);
    expect(plugin.message).toHaveBeenCalledTimes(1);
    expect(String(plugin.message.mock.calls[0][0])).toContain("dialog.confirm not allowed");
  });
});

describe("alertDialog", () => {
  it("never throws when the message dialog itself fails", async () => {
    plugin.message.mockRejectedValue(new Error("no bridge"));
    await expect(alertDialog("something went wrong")).resolves.toBeUndefined();
  });
});
