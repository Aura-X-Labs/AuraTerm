import { describe, expect, it, beforeEach } from "vitest";
import { ref } from "vue";

import {
  usePaneLayout,
  type PaneLayoutTab,
  type PaneNode,
} from "../usePaneLayout";
import type { SessionConfig } from "../types";

/**
 * Unit tests for the pure tree logic inside usePaneLayout:
 *   - initial state & restore
 *   - splitting (horizontal / vertical)
 *   - tab assignment / reassignment to panes
 *   - pane close + merge (sibling promotion)
 *   - split ratio persistence via workspace round-trip
 *
 * These tests do NOT exercise pointer / DOM behavior — only the reducer-like
 * tree transformations that risk regression during future refactors.
 */

function makeLocalSession(cwd = "/tmp"): SessionConfig {
  return { protocol: "local", cwd };
}

function makeTab(id: string, title = id, session = makeLocalSession()): PaneLayoutTab {
  return { id, title, session };
}

interface Harness {
  layout: ReturnType<typeof usePaneLayout>;
  tabs: ReturnType<typeof ref<PaneLayoutTab[]>>;
}

function createHarness(initialTabs: PaneLayoutTab[] = []): Harness {
  const tabs = ref<PaneLayoutTab[]>(initialTabs);
  const isWindowFocused = ref(true);
  const terminalContainerRef = ref<Element | null>(null);
  const layout = usePaneLayout({
    tabs,
    isWindowFocused,
    terminalContainerRef,
  });
  return { layout, tabs };
}

function collectPaneIds(node: PaneNode, out: string[] = []): string[] {
  if (node.kind === "leaf") {
    out.push(node.paneId);
  } else {
    collectPaneIds(node.first, out);
    collectPaneIds(node.second, out);
  }
  return out;
}

describe("usePaneLayout — initial state", () => {
  it("defaults to a single leaf pane with no active tab", () => {
    const { layout } = createHarness();

    expect(layout.paneLayout.value.kind).toBe("leaf");
    expect(layout.paneLeaves.value).toHaveLength(1);
    expect(layout.activeTabId.value).toBe("");
  });

  it("assigns a tab to the focused pane when activated", () => {
    const tab = makeTab("t1");
    const { layout } = createHarness([tab]);
    layout.assignTabToFocusedPane("t1");

    expect(layout.activeTabId.value).toBe("t1");
    expect(layout.paneLeaves.value[0].tabId).toBe("t1");
  });
});

describe("usePaneLayout — splitting", () => {
  it("handleSplitPane with vertical axis produces a split node with two leaves", () => {
    const tabs = [makeTab("t1"), makeTab("t2")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");

    layout.handleSplitPane("vertical");

    expect(layout.paneLayout.value.kind).toBe("split");
    if (layout.paneLayout.value.kind === "split") {
      expect(layout.paneLayout.value.axis).toBe("vertical");
      expect(layout.paneLayout.value.ratio).toBeCloseTo(0.5);
    }
    expect(layout.paneLeaves.value).toHaveLength(2);

    // The new pane should pick up the second (hidden) tab automatically.
    const tabIds = layout.paneLeaves.value.map((p) => p.tabId);
    expect(tabIds).toContain("t1");
    expect(tabIds).toContain("t2");
  });

  it("handleSplitPane supports nested splits and keeps all leaves reachable", () => {
    const tabs = [makeTab("t1"), makeTab("t2"), makeTab("t3")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");

    layout.handleSplitPane("vertical");
    layout.handleSplitPane("horizontal"); // splits the newly focused pane

    expect(layout.paneLeaves.value).toHaveLength(3);
    const paneIds = collectPaneIds(layout.paneLayout.value);
    expect(new Set(paneIds).size).toBe(3);
  });
});

describe("usePaneLayout — pane close & merge", () => {
  it("handleClosePane promotes the sibling to replace a split node", () => {
    const tabs = [makeTab("t1"), makeTab("t2")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");
    layout.handleSplitPane("vertical");

    expect(layout.paneLeaves.value).toHaveLength(2);
    const focusedPane = layout.focusedPaneId.value;
    layout.handleClosePane(focusedPane);

    // After close, tree collapses back to a single leaf.
    expect(layout.paneLayout.value.kind).toBe("leaf");
    expect(layout.paneLeaves.value).toHaveLength(1);
  });

  it("handleClosePane is a no-op when only one pane remains", () => {
    const tabs = [makeTab("t1")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");

    const before = layout.paneLayout.value;
    layout.handleClosePane(layout.focusedPaneId.value);

    expect(layout.paneLayout.value).toBe(before);
    expect(layout.paneLeaves.value).toHaveLength(1);
  });
});

describe("usePaneLayout — tab reassignment", () => {
  it("selectTab focuses an existing pane when the tab is already visible", () => {
    const tabs = [makeTab("t1"), makeTab("t2")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");
    layout.handleSplitPane("vertical");

    const panesBefore = layout.paneLeaves.value.length;
    layout.selectTab("t1");

    expect(layout.paneLeaves.value.length).toBe(panesBefore);
    expect(layout.activeTabId.value).toBe("t1");
  });

  it("selecting a hidden tab moves it into the focused pane", () => {
    const tabs = [makeTab("t1"), makeTab("t2"), makeTab("t3")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");

    // Initially only t1 is visible; bring t3 into the focused pane.
    layout.selectTab("t3");

    const focusedLeaf = layout.paneLeaves.value.find(
      (p) => p.paneId === layout.focusedPaneId.value,
    );
    expect(focusedLeaf?.tabId).toBe("t3");
    expect(layout.activeTabId.value).toBe("t3");
  });

  it("handleTabRemoved reassigns the pane using history fallback", () => {
    const tabs = [makeTab("t1"), makeTab("t2")];
    const { layout, tabs: tabsRef } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");
    layout.selectTab("t2"); // history now has [t1, t2] for the focused pane

    layout.handleTabRemoved("t2", [tabs[0]]);
    tabsRef.value = [tabs[0]];

    const visibleTabIds = layout.paneLeaves.value.map((p) => p.tabId);
    expect(visibleTabIds).toContain("t1");
    expect(visibleTabIds).not.toContain("t2");
  });
});

describe("usePaneLayout — workspace round-trip", () => {
  it("restoreWorkspaceState → applyRestoredWorkspaceState preserves pane tree", () => {
    const tabs = [makeTab("t1"), makeTab("t2")];
    const { layout } = createHarness(tabs);
    layout.assignTabToFocusedPane("t1");
    layout.handleSplitPane("horizontal");

    const snapshot = layout.createPersistedWorkspaceState(true);
    expect(snapshot).not.toBeNull();

    // Reset harness and round-trip restore
    const { layout: restored } = createHarness();
    const restoredState = restored.restoreWorkspaceState(snapshot);
    expect(restoredState).not.toBeNull();
    if (restoredState) {
      restored.applyRestoredWorkspaceState(restoredState);
    }

    expect(restored.paneLayout.value.kind).toBe("split");
    expect(restored.paneLeaves.value).toHaveLength(2);
    const restoredTabIds = new Set(
      restored.paneLeaves.value.map((p) => p.tabId).filter(Boolean),
    );
    expect(restoredTabIds.has("t1")).toBe(true);
    expect(restoredTabIds.has("t2")).toBe(true);
  });

  it("does not persist SSH credentials in workspace snapshots", () => {
    const sshSession: SessionConfig = {
      protocol: "ssh",
      sshConfig: {
        host: "target.internal", port: 22, user: "alice", authType: "key",
        password: "password-secret", privateKey: "private-key-secret",
        passphrase: "passphrase-secret", savedConnectionId: "saved-1",
        jumpHosts: [{ id: "jump-1", host: "bastion", port: 22, user: "alice", authType: "password", password: "jump-secret" }],
        autoLoginRules: [{ expect: "Password:", response: "expect-secret" }],
        postConnectCommands: ["export TOKEN=secret"],
      },
    };
    const { layout } = createHarness([makeTab("ssh-1", "SSH", sshSession)]);
    const snapshot = JSON.stringify(layout.createPersistedWorkspaceState(true));

    expect(snapshot).toContain("saved-1");
    for (const secret of ["password-secret", "private-key-secret", "passphrase-secret", "jump-secret", "expect-secret", "TOKEN=secret"]) {
      expect(snapshot).not.toContain(secret);
    }
  });

  it("rejects workspace snapshots with the wrong version", () => {
    const { layout } = createHarness();
    expect(layout.restoreWorkspaceState(null)).toBeNull();
    expect(layout.restoreWorkspaceState({ version: 999, tabs: [] })).toBeNull();
    expect(layout.restoreWorkspaceState({ version: 1, tabs: [] })).toBeNull();
  });

  it("applyPaneLayoutFromTabs fills empty panes from the provided tab list", () => {
    const { layout } = createHarness();
    const tabs = [makeTab("a"), makeTab("b")];

    // Persisted layout has 2 panes but neither carries a tabId — should be filled.
    // `applyPaneLayoutFromTabs` wraps the raw tree in `{ root, focusedPaneId }`
    // before handing it to `restorePaneLayoutState`.
    const persistedState = {
      root: {
        kind: "split" as const,
        splitId: "split-0",
        axis: "vertical" as const,
        ratio: 0.5,
        first: { kind: "leaf" as const, paneId: "pane-10", tabId: null },
        second: { kind: "leaf" as const, paneId: "pane-11", tabId: null },
      },
      focusedPaneId: "pane-10",
    };

    layout.applyPaneLayoutFromTabs(tabs, persistedState);

    const visibleTabIds = layout.paneLeaves.value
      .map((p) => p.tabId)
      .filter((id): id is string => Boolean(id));
    expect(visibleTabIds).toEqual(expect.arrayContaining(["a", "b"]));
  });
});

describe("usePaneLayout — split ratio clamp", () => {
  beforeEach(() => {
    // no-op; here to document that tests are independent of outer state.
  });

  it("clamps extreme ratios on restore between MIN and MAX", () => {
    const { layout } = createHarness([makeTab("t1"), makeTab("t2")]);
    const persistedState = {
      root: {
        kind: "split" as const,
        splitId: "split-0",
        axis: "horizontal" as const,
        ratio: 10, // out of range; must be clamped
        first: { kind: "leaf" as const, paneId: "pane-10", tabId: "t1" },
        second: { kind: "leaf" as const, paneId: "pane-11", tabId: "t2" },
      },
      focusedPaneId: "pane-10",
    };

    layout.applyPaneLayoutFromTabs(
      [makeTab("t1"), makeTab("t2")],
      persistedState,
    );

    const root = layout.paneLayout.value;
    expect(root.kind).toBe("split");
    if (root.kind === "split") {
      expect(root.ratio).toBeLessThanOrEqual(0.85);
      expect(root.ratio).toBeGreaterThanOrEqual(0.15);
    }
  });
});
