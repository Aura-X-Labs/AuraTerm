import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import LiveSyncStatusBar from "../LiveSyncStatusBar.vue";
import { consoleStatus, relayStatus, shareStatus, syncStatus, type FeatureStatus } from "../liveSyncStatus";
import type { CloudBridgeStatus } from "../cloudBridge";
import type { AssistStatus } from "../assist";
import type { SyncConfigView } from "../cloudSync";
import { setLanguage } from "../i18n";

setLanguage("en");
const bridge: CloudBridgeStatus = { enrolled: true, connected: true, reconnecting: false, standby: false, shares: [] };
const relay = { enabled: true, peers: [] };
const guest = {
  connectionId: "g1", role: "controller" as const, client: "desktop",
  displayName: "Guest", fingerprint: "sas", controlRequested: false,
};
const assist: AssistStatus = {
  assistId: "a", code: "code", link: "link", localSessionId: "local", protocol: "local", label: "Shell",
  policy: { control: "on_request", approvalRequired: true, singleUse: false, maxGuests: 5 },
  followActiveTab: false, joinExpiresAt: 10, joinOpen: true, createdAt: 1, expiresAt: 100,
  failedAttempts: 0, fence: 1, locked: false, guests: [guest],
};
const sync: SyncConfigView = {
  provider: "auraxlab", includeSettings: true, includeKnownHosts: true, includeCredentials: false,
  autoSync: false, deviceId: "dev", deviceLabel: "Mac", lastSyncAt: null, lastRemoteVersion: null,
  credentialsMode: "masterPassword", masterUnlocked: true, legacyProviderNotice: false,
  auraxlab: { username: "alice", email: "alice@example.com", tokenSet: true },
};

const wrappers: ReturnType<typeof mount>[] = [];
afterEach(() => { wrappers.forEach((wrapper) => wrapper.unmount()); wrappers.length = 0; });

function render(overrides: Partial<Record<string, unknown>> = {}, statuses?: FeatureStatus[]) {
  const props = {
    statuses: statuses ?? [
      syncStatus(sync), consoleStatus(bridge, true), shareStatus(null), relayStatus(relay),
    ],
    bridge, assist: null, relay, sync, syncBusy: false, consoleEnabled: true,
    allowRemoteSend: true, outbound: [], canManage: true, compact: false,
    ...overrides,
  };
  const wrapper = mount(LiveSyncStatusBar, { props, global: { stubs: { teleport: true } } });
  wrappers.push(wrapper);
  return wrapper;
}

const chips = (wrapper: ReturnType<typeof mount>) => wrapper.findAll(".terminal-status-cloud");
const chip = (wrapper: ReturnType<typeof mount>) => wrapper.get(".terminal-status-cloud");

describe("Live Sync cluster", () => {
  it("keeps one entry when nothing is happening and opens the panel without changing anything", async () => {
    const wrapper = render();
    expect(chips(wrapper)).toHaveLength(1);
    expect(chip(wrapper).text()).toContain("Live Sync");
    await chip(wrapper).trigger("click");
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    expect(wrapper.emitted("refresh")).toHaveLength(1);
    expect(wrapper.emitted("toggleConsole")).toBeUndefined();
    // Every feature is still readable in the panel even with no pill of its own.
    const text = wrapper.get('[role="dialog"]').text();
    for (const line of ["Sync · never synced", "Console · On · Connected", "Share · not sharing", "Relay · on"]) {
      expect(text).toContain(line);
    }
  });

  it("stays a single chip that names the active feature and opens its row", async () => {
    const statuses = [syncStatus(sync), consoleStatus(bridge, true), shareStatus(assist), relayStatus(relay)];
    const wrapper = render({ assist }, statuses);
    expect(chips(wrapper)).toHaveLength(1);
    expect(chip(wrapper).text()).toContain("Live Sync · Share control 1");
    // Everything the chip compressed is still reachable without clicking.
    expect(chip(wrapper).attributes("title")).toContain("Console · On · Connected");
    await chip(wrapper).trigger("click");
    expect(wrapper.get(".live-sync-row.focused").text()).toContain("Live Share");
    expect(wrapper.emitted("toggleConsole")).toBeUndefined();
    expect(wrapper.emitted("stopShare")).toBeUndefined();
  });

  it("trades the feature name for counts in a narrow window", () => {
    const statuses = [syncStatus(sync), consoleStatus(bridge, true), shareStatus(assist), relayStatus(relay)];
    const wide = render({ assist }, statuses);
    const narrow = render({ assist, compact: true }, statuses);
    expect(chips(narrow)).toHaveLength(1);
    expect(wide.text()).toContain("Share control 1");
    expect(narrow.text()).toContain("Live Sync · control 1");
    expect(narrow.text()).not.toContain("Share control");
  });

  it("lets a child window read everything while the global switches stay put", async () => {
    const wrapper = render({ canManage: false, assist });
    await chip(wrapper).trigger("click");
    expect(wrapper.findAll("input:disabled")).toHaveLength(2);
    expect(wrapper.text()).toContain("main window");
    // Jumping to the feature's own dialog is not a mutation, so it stays open.
    const manage = wrapper.findAll("button").find((button) => button.text() === "Manage share…")!;
    expect(manage.attributes("disabled")).toBeUndefined();
    await manage.trigger("click");
    expect(wrapper.emitted("manageShare")).toHaveLength(1);
    expect(wrapper.emitted("toggleConsole")).toBeUndefined();
  });

  it("offers signing in and blocks a second run while one is in flight", async () => {
    const signedOut = { ...sync, auraxlab: { ...sync.auraxlab, tokenSet: false } };
    const wrapper = render({ sync: signedOut, syncBusy: true }, [
      syncStatus(signedOut, { phase: "syncing", message: "", at: 1 }),
      consoleStatus(bridge, true), shareStatus(null), relayStatus(relay),
    ]);
    await chip(wrapper).trigger("click");
    expect(wrapper.get(".live-sync-row").text()).toContain("not signed in");
    const buttons = wrapper.findAll(".live-sync-row button");
    expect(buttons.find((button) => button.text() === "Sync now")!.attributes("disabled")).toBeDefined();
    expect(buttons.some((button) => button.text() === "Sign in…")).toBe(true);
    // Signed in, the same button opens the settings instead.
    const signedIn = render({ sync });
    await chip(signedIn).trigger("click");
    expect(signedIn.findAll(".live-sync-row button").some((button) => button.text() === "Sync settings…")).toBe(true);
  });

  it("labels a stale row instead of hiding what it last knew", async () => {
    const wrapper = render({ assist }, [
      syncStatus(sync), consoleStatus(bridge, true), shareStatus(assist, true), relayStatus(relay),
    ]);
    await chip(wrapper).trigger("click");
    const row = wrapper.get(".live-sync-row.stale");
    expect(row.text()).toContain("status not updated");
    expect(row.text()).toContain("control 1");
    expect(row.text()).toContain("The last refresh failed");
  });
});
