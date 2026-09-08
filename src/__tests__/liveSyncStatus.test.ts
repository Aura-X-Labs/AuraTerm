import { describe, expect, it } from "vitest";
import { assistStatusPill, cloudStatusPill, relayStatusPill } from "../liveSyncStatus";
import type { AssistStatus } from "../assist";
import type { CloudBridgeStatus } from "../cloudBridge";
import type { RelayPeerView } from "../liveRelay";
import { setLanguage } from "../i18n";

setLanguage("en");
const bridge: CloudBridgeStatus = { enrolled: true, connected: true, reconnecting: false, standby: false, shares: [] };
const peer: RelayPeerView = { connectionId: "relay-1", label: "Laptop", fingerprint: "sas", shareLabel: "Shell", state: "controller", joinedAt: 1, canControl: true, controlRequested: false };

describe("Live Sync status regressions", () => {
  it("does not label a Relay controller as a Console viewer", () => {
    const state = { ...bridge, shares: [{ localSessionId: "local", cloudSessionId: "cloud", label: "Shell", protocol: "local" as const, txPolicy: "read_write" as const, txAllowed: true, viewerCount: 1, controllerAttached: false, consoleViewerCount: 0, consoleControllerCount: 0 }] };
    expect(cloudStatusPill(state, true)?.kind).toBe("online");
  });
  it("shows Console disabled even if another feature holds the bridge online", () => {
    expect(cloudStatusPill(bridge, false)?.text).toBe("Console · Off");
  });
  it("keeps Relay control visible alongside pending admission and control requests", () => {
    const pill = relayStatusPill({ enabled: true, peers: [peer, { ...peer, connectionId: "pending", state: "pending" }, { ...peer, connectionId: "asking", state: "viewer", controlRequested: true }] });
    expect(pill?.kind).toBe("control");
    expect(pill?.text).toContain("control 1");
    expect(pill?.text).toContain("viewing 1");
    expect(pill?.text).toContain("pending 2");
  });
});

describe("Live Sync status boundaries", () => {
  it.each([
    [{ ...bridge, enrolled: false }, true, null],
    [bridge, true, "online"],
    [{ ...bridge, connected: false, standby: true }, true, "standby"],
    [{ ...bridge, connected: false, reconnecting: true }, true, "reconnecting"],
    [{ ...bridge, connected: false }, true, "offline"],
  ] as const)("distinguishes enrollment and enabled link states (%#)", (state, enabled, kind) => {
    expect(cloudStatusPill(state, enabled)?.kind ?? null).toBe(kind);
  });

  it("keeps remaining Console connections visible after auto-share is switched off", () => {
    const share = { localSessionId: "local", cloudSessionId: "cloud", label: "Shell", protocol: "local" as const, txPolicy: "read_write" as const, txAllowed: true, viewerCount: 2, controllerAttached: true, consoleViewerCount: 1, consoleControllerCount: 1 };
    const pill = cloudStatusPill({ ...bridge, shares: [share] }, false);
    expect(pill?.kind).toBe("control");
    expect(pill?.text).toContain("Console");
    expect(pill?.text).toContain("control 1");
    expect(pill?.text).toContain("viewing 1");
    expect(pill?.text).toContain("auto-share off");
  });

  it("does not count pending admission as an attached Relay viewer", () => {
    const pill = relayStatusPill({ enabled: true, peers: [{ ...peer, state: "pending", controlRequested: true }] });
    expect(pill?.kind).toBe("assist");
    expect(pill?.text).toBe("Relay · pending 1");
    expect(relayStatusPill({ enabled: true, peers: [] })).toBeNull();
  });

  it("preserves Share control and both types of pending requests", () => {
    const guest = { connectionId: "control", role: "controller" as const, client: "desktop", displayName: "Guest", fingerprint: "sas", controlRequested: false };
    const assist: AssistStatus = { assistId: "assist", code: "code", link: "link", localSessionId: "local", protocol: "local", label: "Shell", policy: { control: "on_request", approvalRequired: true, singleUse: false, maxGuests: 5 }, followActiveTab: false, joinExpiresAt: 10, joinOpen: true, createdAt: 1, expiresAt: 100, failedAttempts: 0, fence: 1, locked: false, guests: [guest, { ...guest, connectionId: "pending", role: "pending" }, { ...guest, connectionId: "asking", role: "viewer", controlRequested: true }] };
    expect(assistStatusPill(assist)).toEqual({ kind: "control", text: "Share · control 1 · viewing 1 · pending 2" });
    expect(assistStatusPill({ ...assist, guests: [] })?.text).toContain("waiting");
    expect(assistStatusPill(null)).toBeNull();
  });

  it("localizes all three features consistently", () => {
    setLanguage("zh-CN");
    try {
      expect(cloudStatusPill(bridge, false)?.text).toBe("Console · 已关闭");
      expect(relayStatusPill({ enabled: true, peers: [peer] })?.text).toBe("Relay · 控制连接 1");
    } finally {
      setLanguage("en");
    }
  });
});
