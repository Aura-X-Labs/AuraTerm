import { describe, expect, it, vi } from "vitest";
import { ref } from "vue";

import { usePaneLayout, type PaneLayoutTab } from "../usePaneLayout";
import type { SessionConfig } from "../types";

/**
 * Workspace restore in the Mac App Store build, which has no local shell
 * (see src/distribution.ts): a workspace saved by a direct download, or
 * synced from one, must not come back with shells that cannot start.
 */
vi.mock("../distribution", () => ({
  isAppStoreBuild: true,
  localShellAvailable: false,
  defaultFilesDir: "~/Downloads/AuraTerm",
}));

function createLayout(initialTabs: PaneLayoutTab[] = []) {
  return usePaneLayout({
    tabs: ref<PaneLayoutTab[]>(initialTabs),
    isWindowFocused: ref(true),
    terminalContainerRef: ref<Element | null>(null),
  });
}

const sshSession: SessionConfig = {
  protocol: "ssh",
  sshConfig: { host: "target.internal", port: 22, user: "alice", authType: "password" },
};

describe("usePaneLayout — App Store build", () => {
  it("drops local shell tabs from a restored workspace and keeps the rest", () => {
    const saved = createLayout([
      { id: "local-1", title: "Local Shell", session: { protocol: "local", cwd: "/tmp" } },
      { id: "ssh-1", title: "SSH", session: sshSession },
    ]);
    saved.assignTabToFocusedPane("local-1");
    const snapshot = saved.createPersistedWorkspaceState(true);

    const restored = createLayout().restoreWorkspaceState(snapshot);

    expect(restored?.tabs.map((tab) => tab.id)).toEqual(["ssh-1"]);
  });

  it("restores nothing from a workspace of only local shells", () => {
    const saved = createLayout([
      { id: "local-1", title: "Local Shell", session: { protocol: "local" } },
    ]);
    saved.assignTabToFocusedPane("local-1");

    expect(createLayout().restoreWorkspaceState(saved.createPersistedWorkspaceState(true))).toBeNull();
  });
});
