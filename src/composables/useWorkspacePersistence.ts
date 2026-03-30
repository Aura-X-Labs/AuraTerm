import { invoke } from "@tauri-apps/api/core";
import type { Ref } from "vue";
import { normalizeAppSettings, type AppSettings } from "../settings";

interface UseWorkspacePersistenceOptions {
  settings: Ref<AppSettings>;
  settingsRef: Ref<AppSettings>;
  hasLoadedSettings: Ref<boolean>;
  createPersistedPaneLayoutState: () => unknown;
  createPersistedWorkspaceState: (restoreEnabled: boolean) => unknown;
}

export function useWorkspacePersistence({
  settings,
  settingsRef,
  hasLoadedSettings,
  createPersistedPaneLayoutState,
  createPersistedWorkspaceState,
}: UseWorkspacePersistenceOptions) {
  let persistWorkspaceStateTimer: number | null = null;

  function prepareSettingsForSave(baseSettings: AppSettings, restoreEnabled = baseSettings.restoreTabsOnStartup) {
    return normalizeAppSettings({
      ...baseSettings,
      paneLayout: createPersistedPaneLayoutState(),
      workspaceState: createPersistedWorkspaceState(restoreEnabled),
    });
  }

  function clearScheduledWorkspacePersistence() {
    if (persistWorkspaceStateTimer !== null) {
      window.clearTimeout(persistWorkspaceStateTimer);
      persistWorkspaceStateTimer = null;
    }
  }

  function persistSettingsSilently(newSettings: AppSettings) {
    const normalizedSettings = prepareSettingsForSave(newSettings, newSettings.restoreTabsOnStartup);
    settingsRef.value = normalizedSettings;
    settings.value = normalizedSettings;
    void invoke("save_settings", { settings: normalizedSettings }).catch((error) => {
      console.error("save_settings failed", error);
    });
  }

  function scheduleWorkspaceStatePersistence() {
    if (!hasLoadedSettings.value) {
      return;
    }

    clearScheduledWorkspacePersistence();
    persistWorkspaceStateTimer = window.setTimeout(() => {
      persistWorkspaceStateTimer = null;
      const nextSettings = prepareSettingsForSave(settingsRef.value);
      if (
        JSON.stringify(settingsRef.value.paneLayout) === JSON.stringify(nextSettings.paneLayout)
        && JSON.stringify(settingsRef.value.workspaceState) === JSON.stringify(nextSettings.workspaceState)
      ) {
        return;
      }

      persistSettingsSilently(nextSettings);
    }, 240);
  }

  return {
    prepareSettingsForSave,
    persistSettingsSilently,
    scheduleWorkspaceStatePersistence,
    clearScheduledWorkspacePersistence,
  };
}