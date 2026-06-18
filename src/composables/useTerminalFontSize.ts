import type { ShallowRef } from "vue";
import {
  DEFAULT_SETTINGS,
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  type AppSettings,
} from "../settings";

interface UseTerminalFontSizeOptions {
  /** Latest settings snapshot (the persistence-facing ref). */
  settingsRef: ShallowRef<AppSettings>;
  /** Persist a new settings object without surfacing a dialog. */
  persistSettingsSilently: (settings: AppSettings) => void;
  /** Close any open app/context menus before applying a change. */
  closeOpenMenus: () => void;
}

/**
 * Terminal font-size zoom controls (View menu items + Ctrl/Cmd +/-/0 shortcuts).
 * Clamps to the configured min/max and persists silently.
 */
export function useTerminalFontSize({
  settingsRef,
  persistSettingsSilently,
  closeOpenMenus,
}: UseTerminalFontSizeOptions) {
  function adjustTerminalFontSize(delta: number) {
    closeOpenMenus();
    const nextFontSize = Math.min(
      MAX_TERMINAL_FONT_SIZE,
      Math.max(MIN_TERMINAL_FONT_SIZE, settingsRef.value.fontSize + delta),
    );

    if (nextFontSize === settingsRef.value.fontSize) {
      return;
    }

    persistSettingsSilently({
      ...settingsRef.value,
      fontSize: nextFontSize,
    });
  }

  function handleIncreaseTerminalFontSize() {
    adjustTerminalFontSize(1);
  }

  function handleDecreaseTerminalFontSize() {
    adjustTerminalFontSize(-1);
  }

  function handleResetTerminalFontSize() {
    closeOpenMenus();
    if (settingsRef.value.fontSize === DEFAULT_SETTINGS.fontSize) {
      return;
    }
    persistSettingsSilently({
      ...settingsRef.value,
      fontSize: DEFAULT_SETTINGS.fontSize,
    });
  }

  return {
    handleIncreaseTerminalFontSize,
    handleDecreaseTerminalFontSize,
    handleResetTerminalFontSize,
  };
}
