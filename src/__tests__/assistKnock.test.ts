import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { respondAssistKnock, type AssistKnock } from "../assist";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
const knock: AssistKnock = { connectionId: "guest", displayName: "Guest", client: "desktop", fingerprint: "sas", kind: "control" };

describe("Share approval completion", () => {
  it.each(["allow_view", "deny"] as const)("clears a control request when the decision is %s", async (decision) => {
    await respondAssistKnock(knock, decision);
    expect(invoke).toHaveBeenCalledWith("assist_set_role", { connectionId: "guest", role: "viewer", durationSeconds: null });
  });
  it("still grants requested control and denies admission via the correct host commands", async () => {
    await respondAssistKnock(knock, "allow_control");
    expect(invoke).toHaveBeenCalledWith("assist_set_role", { connectionId: "guest", role: "controller", durationSeconds: null });
    await respondAssistKnock({ ...knock, kind: "join" }, "deny");
    expect(invoke).toHaveBeenCalledWith("assist_respond_join", { connectionId: "guest", decision: "deny" });
  });
});
