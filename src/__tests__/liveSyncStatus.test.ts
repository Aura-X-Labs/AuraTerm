import { describe, expect, it } from "vitest";
import {
  consoleStatus,
  liveSyncLabel,
  liveSyncTooltip,
  relayStatus,
  sessionShareLabel,
  shareStatus,
  syncStatus,
  type SyncRuntime,
} from "../liveSyncStatus";
import type { AssistStatus } from "../assist";
import type { CloudBridgeStatus } from "../cloudBridge";
import type { RelayPeerView } from "../liveRelay";
import type { SyncConfigView } from "../cloudSync";
import { setLanguage } from "../i18n";

setLanguage("en");
const bridge: CloudBridgeStatus = { enrolled: true, connected: true, reconnecting: false, standby: false, shares: [] };
const share = {
  localSessionId: "local", cloudSessionId: "cloud", label: "Shell", protocol: "local" as const,
  txPolicy: "read_write" as const, txAllowed: true, viewerCount: 1, controllerAttached: false,
  consoleViewerCount: 0, consoleControllerCount: 0,
};
const peer: RelayPeerView = {
  connectionId: "relay-1", label: "Laptop", fingerprint: "sas", shareLabel: "Shell",
  state: "controller", joinedAt: 1, canControl: true, controlRequested: false,
};
const guest = {
  connectionId: "control", role: "controller" as const, client: "desktop",
  displayName: "Guest", fingerprint: "sas", controlRequested: false,
};
const assist: AssistStatus = {
  assistId: "assist", code: "code", link: "link", localSessionId: "local", protocol: "local", label: "Shell",
  policy: { control: "on_request", approvalRequired: true, singleUse: false, maxGuests: 5 },
  followActiveTab: false, joinExpiresAt: 10, joinOpen: true, createdAt: 1, expiresAt: 100,
  failedAttempts: 0, fence: 1, locked: false, guests: [],
};
const syncView: SyncConfigView = {
  provider: "auraxlab", includeSettings: true, includeKnownHosts: true, includeCredentials: false,
  autoSync: false, deviceId: "dev", deviceLabel: "Mac", lastSyncAt: null, lastRemoteVersion: null,
  credentialsMode: "masterPassword", masterUnlocked: true, legacyProviderNotice: false,
  auraxlab: { username: "alice", email: "alice@example.com", tokenSet: true },
};
const idle: SyncRuntime = { phase: "idle", message: "", at: null };

describe("Live Sync status regressions (P0)", () => {
  it("does not label a Relay controller as a Console viewer", () => {
    // viewerCount counts every bridge peer; only the console-* fields are Console's.
    const status = consoleStatus({ ...bridge, shares: [share] }, true);
    expect(status.pill).toBeNull();
    expect(status.detail).toBe("Console · On · Connected");
  });
  it("shows Console disabled even if another feature holds the bridge online", () => {
    expect(consoleStatus(bridge, false).detail).toBe("Console · Off");
  });
  it("keeps remaining Console connections visible after auto-share is switched off", () => {
    const busy = { ...share, consoleViewerCount: 1, consoleControllerCount: 1 };
    const status = consoleStatus({ ...bridge, shares: [busy] }, false);
    expect(status.pill?.kind).toBe("control");
    expect(status.detail).toContain("control 1");
    expect(status.detail).toContain("viewing 1");
    expect(status.detail).toContain("auto-share off");
  });
  it("keeps Relay control visible alongside pending admission and control requests", () => {
    const status = relayStatus({ enabled: true, peers: [
      peer,
      { ...peer, connectionId: "pending", state: "pending" },
      { ...peer, connectionId: "asking", state: "viewer", controlRequested: true },
    ] });
    expect(status.pill?.kind).toBe("control");
    expect(status.detail).toContain("control 1");
    expect(status.detail).toContain("viewing 1");
    expect(status.detail).toContain("pending 2");
  });
  it("does not count pending admission as an attached Relay viewer", () => {
    const status = relayStatus({ enabled: true, peers: [{ ...peer, state: "pending", controlRequested: true }] });
    expect(status.pill?.kind).toBe("assist");
    expect(status.detail).toBe("Relay · pending 1");
    expect(relayStatus({ enabled: true, peers: [] }).pill).toBeNull();
  });
  it("preserves Share control and both types of pending requests", () => {
    const running: AssistStatus = { ...assist, guests: [
      guest,
      { ...guest, connectionId: "pending", role: "pending" },
      { ...guest, connectionId: "asking", role: "viewer", controlRequested: true },
    ] };
    expect(shareStatus(running).pill)
      .toEqual({ kind: "control", text: "Share · control 1 · viewing 1 · pending 2", feature: "share" });
    expect(shareStatus(assist).detail).toContain("waiting");
    expect(shareStatus(null).pill).toBeNull();
  });
  it("localizes every feature consistently", () => {
    setLanguage("zh-CN");
    try {
      expect(consoleStatus(bridge, false).detail).toBe("Console · 已关闭");
      expect(relayStatus({ enabled: true, peers: [peer] }).detail).toBe("Relay · 控制连接 1");
      expect(syncStatus(syncView, idle).detail).toBe("Sync · 尚未同步");
    } finally {
      setLanguage("en");
    }
  });
});

describe("Link and mode dimensions stay separate", () => {
  it.each([
    [{ ...bridge, enrolled: false }, true, "unconfigured", "unknown", null],
    [bridge, true, "on", "connected", null],
    [bridge, false, "off", "connected", null],
    [{ ...bridge, connected: false, standby: true }, true, "on", "standby", null],
    [{ ...bridge, connected: false, reconnecting: true }, true, "on", "reconnecting", "reconnecting"],
    [{ ...bridge, connected: false }, true, "on", "offline", "error"],
    [{ ...bridge, connected: false }, false, "off", "offline", null],
  ] as const)("reports mode and link independently (%#)", (state, enabled, mode, link, kind) => {
    const status = consoleStatus(state, enabled);
    expect([status.mode, status.link]).toEqual([mode, link]);
    // Idle-but-healthy folds into the entry; only trouble on a switched-on
    // feature earns its own pill.
    expect(status.pill?.kind ?? null).toBe(kind);
  });
});

describe("The single status-bar label (P3)", () => {
  it("stays a bare entry while every feature is idle", () => {
    const statuses = [syncStatus(syncView, idle), consoleStatus(bridge, true), shareStatus(null), relayStatus({ enabled: true, peers: [] })];
    expect(statuses.filter((status) => status.pill)).toEqual([]);
    expect(liveSyncLabel(statuses)).toEqual({ kind: "offline", text: "Live Sync", feature: null });
    // Nothing is lost: the panel rows (and the tooltip) still carry it all.
    expect(statuses.map((status) => status.detail)).toEqual([
      "Sync · never synced", "Console · On · Connected", "Share · not sharing", "Relay · on, nobody attached",
    ]);
  });

  it("names the most important feature and counts the rest", () => {
    const statuses = [
      syncStatus(syncView, { phase: "error", message: "boom", at: 1 }),
      consoleStatus({ ...bridge, shares: [{ ...share, consoleViewerCount: 2 }] }, true),
      shareStatus({ ...assist, guests: [guest, { ...guest, connectionId: "p", role: "pending" }] }),
      relayStatus({ enabled: true, peers: [peer] }),
    ];
    expect(statuses.filter((status) => status.pill)).toHaveLength(4);
    // Share and Relay both hold control; the tie goes to the one also waiting
    // on an approval. The other three active features become "+3".
    expect(liveSyncLabel(statuses)).toEqual({
      kind: "control", feature: "share", text: "Live Sync · Share control 1 · pending 1 · +3",
    });
  });

  it("drops the name and the viewer counts in a narrow window", () => {
    const statuses = [
      syncStatus(syncView, { phase: "error", message: "boom", at: 1 }),
      consoleStatus({ ...bridge, shares: [{ ...share, consoleViewerCount: 2 }] }, true),
      shareStatus({ ...assist, guests: [guest, { ...guest, connectionId: "p", role: "pending" }] }),
      relayStatus({ enabled: true, peers: [peer] }),
    ];
    const narrow = liveSyncLabel(statuses, false);
    expect(narrow.text).toBe("Live Sync · control 2 · pending 1 · issues 1");
    expect(narrow.text).not.toContain("viewing");
    expect(narrow.kind).toBe("control");
  });

  it("never goes silent when the only news has nothing to count", () => {
    const signIn: SyncRuntime = { phase: "error", message: "401", at: 1, code: "signIn" };
    const statuses = [syncStatus(syncView, signIn), consoleStatus(bridge, true), shareStatus(null), relayStatus({ enabled: true, peers: [] })];
    // A sync waiting for a sign-in has no controllers, no pending requests and
    // is not counted as an error, so the count-based narrow form would render
    // an empty summary.
    expect(liveSyncLabel(statuses, false).text).toBe("Live Sync · Sync sign in again");
    expect(liveSyncLabel(statuses).text).toBe("Live Sync · Sync sign in again");
  });

  it("hands the compressed detail to the tooltip", () => {
    const statuses = [syncStatus(syncView, idle), consoleStatus(bridge, true), shareStatus(assist), relayStatus({ enabled: true, peers: [peer] })];
    expect(liveSyncTooltip(statuses).split("\n")).toEqual([
      "Sync · never synced", "Console · On · Connected", "Share · waiting to join", "Relay · control 1",
    ]);
  });

  it("reports the Relay direction separately", () => {
    const status = relayStatus({ enabled: true, peers: [peer] }, 2);
    expect(status.detail).toBe("Relay · control 1 · reaching out 2");
    // The outbound tabs have their own banner, so they never drive the pill.
    expect(status.pill?.text).toBe(status.detail);
    expect(relayStatus({ enabled: true, peers: [] }, 2).pill).toBeNull();
  });
});

describe("Configuration sync and stale snapshots (P2)", () => {
  it("covers the whole sync lifecycle", () => {
    expect(syncStatus(null, idle).detail).toBe("Sync · status unknown");
    const signedOut = { ...syncView, auraxlab: { ...syncView.auraxlab, tokenSet: false } };
    expect(syncStatus(signedOut, idle).detail).toBe("Sync · not signed in");
    // Signed out is signed out, whatever the last run said.
    expect(syncStatus(signedOut, { phase: "error", message: "x", at: 1 }).pill).toBeNull();
    expect(syncStatus(syncView, { phase: "syncing", message: "", at: 1 }).pill?.kind).toBe("monitor");
    expect(syncStatus(syncView, { phase: "error", message: "no route", at: 1 }))
      .toMatchObject({ link: "error", detail: "Sync · failed: no route", pill: { kind: "error" } });
    // A rejected credential asks for a sign-in: attention, not a failure count.
    const signIn = syncStatus(syncView, { phase: "error", message: "401", at: 1, code: "signIn" });
    expect(signIn).toMatchObject({ link: "error", detail: "Sync · sign in again", pill: { kind: "assist" } });
    const synced = { ...syncView, lastSyncAt: 1_700_000_000_000 };
    expect(syncStatus(synced, { phase: "syncing", message: "", at: 1 }).detail).toBe("Sync · syncing…");
    const done = syncStatus(synced, { phase: "ok", message: "done", at: 1 });
    expect(done.detail).toContain("last succeeded");
    expect(done.pill).toBeNull();
    // Everything but the credentials synced: partial success keeps the result
    // and appends the reason, and it earns an attention pill.
    const partial = syncStatus(synced, { phase: "ok", message: "done", at: 1, credentialsSkipped: "masterLocked" });
    expect(partial.detail).toMatch(/^Sync · last succeeded .* · credentials waiting for the master password$/);
    expect(partial.pill?.kind).toBe("assist");
    expect(syncStatus(synced, { phase: "ok", message: "", at: 1, credentialsSkipped: "mismatch" }).detail)
      .toContain("different master password");
  });

  it("marks a failed refresh instead of dropping the previous result", () => {
    const status = shareStatus({ ...assist, guests: [guest] }, true);
    expect(status.stale).toBe(true);
    // The share is still reported, so nobody reads the failure as "it ended".
    expect(status.detail).toBe("Share · control 1 · status not updated");
    expect(status.pill?.kind).toBe("control");
    // Console keeps its own snapshot the same way, and the summary counts it.
    const consoleStale = consoleStatus(bridge, true, true);
    expect(consoleStale.detail).toBe("Console · On · Connected · status not updated");
    expect(liveSyncLabel([status, consoleStale], false).text).toBe("Live Sync · control 1 · issues 1");
  });
});

describe("The session-scope share chip (P3)", () => {
  const shared = { ...share, txAllowed: true, txPolicy: "read_write" as const };
  it("says what the tab granted without leaking the policy enum", () => {
    expect(sessionShareLabel(null)).toBeNull();
    expect(sessionShareLabel(shared)).toBe("shared · input allowed");
    expect(sessionShareLabel({ ...shared, txAllowed: false })).toBe("shared · view only");
    expect(sessionShareLabel(shared)).not.toContain("read_write");
  });

  it("counts down a temporary grant in whole minutes", () => {
    const now = 1_700_000_000_000;
    const temporary = { ...shared, txPolicy: "temporary" as const, txExpiresAt: now / 1000 + 90 };
    expect(sessionShareLabel(temporary, null, now)).toBe("shared · temporary input · 2 min left");
    expect(sessionShareLabel({ ...temporary, txExpiresAt: now / 1000 - 60 }, null, now))
      .toBe("shared · temporary input · 0 min left");
  });

  it("shows a remote-input burst and lets it expire", () => {
    const now = 1_700_000_000_000;
    expect(sessionShareLabel(shared, { byteCount: 128, at: now - 1_000 }, now))
      .toBe("shared · input allowed · remote input 128 B");
    // Older than the window: the burst is gone, the grant is not.
    expect(sessionShareLabel(shared, { byteCount: 128, at: now - 20_000 }, now))
      .toBe("shared · input allowed");
  });
});
