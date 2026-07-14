import { ref, watch, type Ref } from "vue";

export type AppMenuId = "file" | "view" | "tools" | "cloud" | "help";
export type FileSubmenuId = "new-session" | "preferences";

export interface TabContextMenuState {
  x: number;
  y: number;
  tabId: string;
}

interface UseAppMenusOptions {
  /** Menu-bar root — outside-click on the menu bar keeps the menu open. */
  menuBarRef: Ref<HTMLDivElement | null>;
  /** Layout dropdown anchor — positions the dropdown and scopes outside-click. */
  layoutMenuRef: Ref<HTMLDivElement | null>;
  /** Tab context-menu root — scopes its outside-click hit testing. */
  tabContextMenuRef: Ref<HTMLDivElement | null>;
}

/**
 * App menu-bar / dropdown / context-menu open-close state.
 *
 * Centralizes every "which menu is open" flag plus the outside-click and
 * Escape-to-close watchers that used to live inline in `App.vue`. Action
 * handlers (what each menu item *does*) stay in `App.vue`. The DOM refs the
 * watchers hit-test against are owned by the component (they are bound via
 * `ref="…"` in its template) and passed in here.
 */
export function useAppMenus({ menuBarRef, layoutMenuRef, tabContextMenuRef }: UseAppMenusOptions) {
  const openMenuId = ref<AppMenuId | null>(null);
  const openFileSubmenuId = ref<FileSubmenuId | null>(null);
  const tabContextMenu = ref<TabContextMenuState | null>(null);
  const showLayoutMenu = ref(false);
  const showNewTabMenu = ref(false);
  const layoutMenuPos = ref({ top: 0, right: 0 });

  function closeOpenMenus() {
    openMenuId.value = null;
    openFileSubmenuId.value = null;
    tabContextMenu.value = null;
    showLayoutMenu.value = false;
  }

  function toggleLayoutMenu() {
    if (showLayoutMenu.value) {
      showLayoutMenu.value = false;
      return;
    }
    // 计算按钮的 viewport 坐标，用于 fixed 定位
    const container = layoutMenuRef.value;
    if (container) {
      const rect = container.getBoundingClientRect();
      layoutMenuPos.value = {
        top: rect.bottom + 4,
        right: window.innerWidth - rect.right,
      };
    }
    showLayoutMenu.value = true;
  }

  function toggleMenu(menuId: AppMenuId) {
    const nextMenuId = openMenuId.value === menuId ? null : menuId;
    openMenuId.value = nextMenuId;
    if (nextMenuId !== "file") {
      openFileSubmenuId.value = null;
    }
  }

  function toggleFileSubmenu(submenuId: FileSubmenuId) {
    openFileSubmenuId.value = openFileSubmenuId.value === submenuId ? null : submenuId;
  }

  function handleOpenNewTabMenu() {
    closeOpenMenus();
    showNewTabMenu.value = true;
  }

  watch(openMenuId, (menuId, _previous, onCleanup) => {
    if (menuId !== "file") {
      openFileSubmenuId.value = null;
    }

    if (!menuId) {
      return;
    }

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        return;
      }
      if (menuBarRef.value?.contains(target)) {
        return;
      }
      openMenuId.value = null;
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        openMenuId.value = null;
      }
    };

    document.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    onCleanup(() => {
      document.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    });
  });

  watch(tabContextMenu, (value, _previous, onCleanup) => {
    if (!value) {
      return;
    }

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        return;
      }
      if (tabContextMenuRef.value?.contains(target)) {
        return;
      }
      tabContextMenu.value = null;
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        tabContextMenu.value = null;
      }
    };

    document.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    onCleanup(() => {
      document.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    });
  });

  watch(showLayoutMenu, (value, _previous, onCleanup) => {
    if (!value) {
      return;
    }

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) {
        return;
      }
      if (layoutMenuRef.value?.contains(target)) {
        return;
      }
      showLayoutMenu.value = false;
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        showLayoutMenu.value = false;
      }
    };

    document.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    onCleanup(() => {
      document.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    });
  });

  return {
    openMenuId,
    openFileSubmenuId,
    tabContextMenu,
    showLayoutMenu,
    showNewTabMenu,
    layoutMenuPos,
    closeOpenMenus,
    toggleLayoutMenu,
    toggleMenu,
    toggleFileSubmenu,
    handleOpenNewTabMenu,
  };
}
