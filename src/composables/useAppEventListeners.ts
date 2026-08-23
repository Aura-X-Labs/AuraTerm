import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ConnectionProtocol } from "../types";
import type { PaneAxis } from "../usePaneLayout";

interface UseAppEventListenersOptions {
  setWindowFocused: (focused: boolean) => void;
  focusActiveTerminal: () => void;
  syncFullscreenState: () => Promise<void>;
  handleOpenAbout: () => void;
  handleOpenSettings: () => void;
  handleOpenCloudSync: () => void;
  handleOpenAccount: () => void;
  handleSyncNow: () => void;
  handleToggleCloudConsole: () => void;
  handleToggleRemoteSend: () => void;
  handleOpenRemoteAssist: () => void;
  handleNewLocalSessionFromMenu: () => void;
  handleOpenConnectionFromMenu: (protocol: ConnectionProtocol) => void;
  handleCloseActiveTab: () => void;
  handleToggleBookmarks: () => void;
  handleToggleRemoteFileManager: () => void;
  handleToggleTunnelManager: () => void;
  handleToggleCommandPalette: () => void;
  handleSplitPaneFromView: (axis: PaneAxis) => void;
  handleClosePaneFromView: () => void;
  handleIncreaseTerminalFontSize: () => void;
  handleDecreaseTerminalFontSize: () => void;
  handleResetTerminalFontSize: () => void;
}

export function useAppEventListeners({
  setWindowFocused,
  focusActiveTerminal,
  syncFullscreenState,
  handleOpenAbout,
  handleOpenSettings,
  handleOpenCloudSync,
  handleOpenAccount,
  handleSyncNow,
  handleToggleCloudConsole,
  handleToggleRemoteSend,
  handleOpenRemoteAssist,
  handleNewLocalSessionFromMenu,
  handleOpenConnectionFromMenu,
  handleCloseActiveTab,
  handleToggleBookmarks,
  handleToggleRemoteFileManager,
  handleToggleTunnelManager,
  handleToggleCommandPalette,
  handleSplitPaneFromView,
  handleClosePaneFromView,
  handleIncreaseTerminalFontSize,
  handleDecreaseTerminalFontSize,
  handleResetTerminalFontSize,
}: UseAppEventListenersOptions) {
  async function registerAppEventListeners() {
    const cleanupFns: Array<() => void> = [];

    try {
      const windowListeners: UnlistenFn[] = await Promise.all([
        listen("tauri://focus", () => {
          setWindowFocused(true);
          focusActiveTerminal();
        }),
        listen("tauri://blur", () => {
          setWindowFocused(false);
        }),
        listen("tauri://resize", () => {
          void syncFullscreenState();
        }),
      ]);
      cleanupFns.push(...windowListeners);
    } catch (error) {
      console.error("Failed to setup window focus listeners:", error);
    }

    try {
      cleanupFns.push(await listen("show-about", () => {
        handleOpenAbout();
      }));
    } catch (error) {
      console.error("Failed to setup about listener:", error);
    }

    try {
      const menuListeners: UnlistenFn[] = await Promise.all([
        listen("menu-open-settings", () => {
          handleOpenSettings();
        }),
        listen("menu-open-cloud-sync", () => {
          handleOpenCloudSync();
        }),
        listen("menu-open-account", () => {
          handleOpenAccount();
        }),
        listen("menu-sync-now", () => {
          handleSyncNow();
        }),
        listen("menu-toggle-cloud-console", () => {
          handleToggleCloudConsole();
        }),
        listen("menu-toggle-remote-send", () => {
          handleToggleRemoteSend();
        }),
        listen("menu-remote-assist", () => {
          handleOpenRemoteAssist();
        }),
        listen("menu-new-local", () => {
          handleNewLocalSessionFromMenu();
        }),
        listen("menu-new-ssh", () => {
          handleOpenConnectionFromMenu("ssh");
        }),
        listen("menu-new-telnet", () => {
          handleOpenConnectionFromMenu("telnet");
        }),
        listen("menu-new-serial", () => {
          handleOpenConnectionFromMenu("serial");
        }),
        listen("menu-close-tab", () => {
          handleCloseActiveTab();
        }),
        listen("menu-toggle-bookmarks", () => {
          handleToggleBookmarks();
        }),
        listen("menu-toggle-remote-files", () => {
          handleToggleRemoteFileManager();
        }),
        listen("menu-toggle-tunnels", () => {
          handleToggleTunnelManager();
        }),
        listen("menu-open-command-palette", () => {
          handleToggleCommandPalette();
        }),
        listen("menu-split-right", () => {
          handleSplitPaneFromView("vertical");
        }),
        listen("menu-split-down", () => {
          handleSplitPaneFromView("horizontal");
        }),
        listen("menu-close-pane", () => {
          handleClosePaneFromView();
        }),
        listen("menu-increase-font-size", () => {
          handleIncreaseTerminalFontSize();
        }),
        listen("menu-decrease-font-size", () => {
          handleDecreaseTerminalFontSize();
        }),
        listen("menu-reset-font-size", () => {
          handleResetTerminalFontSize();
        }),
      ]);
      cleanupFns.push(...menuListeners);
    } catch (error) {
      console.error("Failed to setup menu listeners:", error);
    }

    return () => {
      for (const cleanup of cleanupFns) {
        cleanup();
      }
    };
  }

  return {
    registerAppEventListeners,
  };
}