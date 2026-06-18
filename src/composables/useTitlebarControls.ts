import { ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface UseTitlebarControlsOptions {
  /** Close any open app/context menus before a window-level action runs. */
  closeOpenMenus: () => void;
}

/**
 * Custom-titlebar window controls (the app runs with `decorations: false`).
 *
 * Owns the `appWindow` handle and all minimize/maximize/close/fullscreen logic
 * so `App.vue` stays a coordinator rather than a home for window plumbing.
 */
export function useTitlebarControls({ closeOpenMenus }: UseTitlebarControlsOptions) {
  const appWindow = getCurrentWindow();
  const isFullscreen = ref(false);

  async function syncFullscreenState() {
    isFullscreen.value = await appWindow.isFullscreen().catch((error) => {
      console.error("isFullscreen failed", error);
      return false;
    });
  }

  function handleTitlebarMouseDown(event: MouseEvent) {
    if (event.button !== 0) {
      return;
    }
    if ((event.target as HTMLElement).closest("[data-no-drag='true']")) {
      return;
    }
    void appWindow.startDragging().catch((error) => {
      console.error("startDragging failed", error);
    });
  }

  async function handleMinimize() {
    await appWindow.minimize().catch((error) => {
      console.error("minimize failed", error);
    });
  }

  async function handleToggleMaximize() {
    const isMaximized = await appWindow.isMaximized().catch((error) => {
      console.error("isMaximized failed", error);
      return false;
    });
    if (isMaximized) {
      await appWindow.unmaximize().catch((error) => {
        console.error("unmaximize failed", error);
      });
      return;
    }
    await appWindow.maximize().catch((error) => {
      console.error("maximize failed", error);
    });
  }

  async function handleClose() {
    await appWindow.close().catch((error) => {
      console.error("close failed", error);
    });
  }

  async function handleExitApp() {
    closeOpenMenus();
    await appWindow.close().catch((error) => {
      console.error("exit failed", error);
    });
  }

  async function handleToggleFullScreen() {
    closeOpenMenus();
    const nextFullscreen = !(await appWindow.isFullscreen().catch((error) => {
      console.error("isFullscreen failed", error);
      return false;
    }));
    await appWindow.setFullscreen(nextFullscreen).catch((error) => {
      console.error("setFullscreen failed", error);
    });
    isFullscreen.value = nextFullscreen;
  }

  function stopDragPropagation(event: MouseEvent) {
    event.stopPropagation();
  }

  return {
    isFullscreen,
    syncFullscreenState,
    handleTitlebarMouseDown,
    handleMinimize,
    handleToggleMaximize,
    handleClose,
    handleExitApp,
    handleToggleFullScreen,
    stopDragPropagation,
  };
}
