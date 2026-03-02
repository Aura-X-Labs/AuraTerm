import { useState } from "react";
import type { AppSettings } from "./settings";

interface SettingsDialogProps {
  initial: AppSettings;
  onSave: (settings: AppSettings) => void;
  onCancel: () => void;
}

export function SettingsDialog({ initial, onSave, onCancel }: SettingsDialogProps) {
  const [settings, setSettings] = useState<AppSettings>(structuredClone(initial));

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) =>
    setSettings((prev) => ({ ...prev, [key]: value }));

  const updateTheme = <K extends keyof AppSettings["theme"]>(
    key: K,
    value: AppSettings["theme"][K],
  ) => setSettings((prev) => ({ ...prev, theme: { ...prev.theme, [key]: value } }));

  return (
    <div className="settings-overlay" onClick={onCancel}>
      <div className="settings-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="settings-header">
          <h2>Settings</h2>
          <button className="settings-close-btn" onClick={onCancel} type="button">
            ×
          </button>
        </div>

        <div className="settings-body">
          {/* ── Terminal ── */}
          <section className="settings-section">
            <h3>Terminal</h3>

            <label className="settings-field">
              <span>Font Size</span>
              <input
                type="number"
                min={8}
                max={72}
                value={settings.fontSize}
                onChange={(e) => update("fontSize", Number(e.target.value))}
              />
            </label>

            <label className="settings-field">
              <span>Font Family</span>
              <input
                type="text"
                value={settings.fontFamily}
                onChange={(e) => update("fontFamily", e.target.value)}
              />
            </label>

            <label className="settings-field">
              <span>Scrollback Lines</span>
              <input
                type="number"
                min={100}
                max={100000}
                step={100}
                value={settings.scrollback}
                onChange={(e) => update("scrollback", Number(e.target.value))}
              />
            </label>

            <label className="settings-field">
              <span>Shell Path</span>
              <input
                type="text"
                placeholder="Default (uses $SHELL)"
                value={settings.shellPath ?? ""}
                onChange={(e) => update("shellPath", e.target.value || null)}
              />
            </label>
          </section>

          {/* ── Theme ── */}
          <section className="settings-section">
            <h3>Theme</h3>

            <label className="settings-field">
              <span>Background</span>
              <div className="settings-color-row">
                <input
                  type="color"
                  value={settings.theme.background}
                  onChange={(e) => updateTheme("background", e.target.value)}
                />
                <input
                  type="text"
                  value={settings.theme.background}
                  onChange={(e) => updateTheme("background", e.target.value)}
                />
              </div>
            </label>

            <label className="settings-field">
              <span>Foreground</span>
              <div className="settings-color-row">
                <input
                  type="color"
                  value={settings.theme.foreground}
                  onChange={(e) => updateTheme("foreground", e.target.value)}
                />
                <input
                  type="text"
                  value={settings.theme.foreground}
                  onChange={(e) => updateTheme("foreground", e.target.value)}
                />
              </div>
            </label>

            <label className="settings-field">
              <span>Cursor</span>
              <div className="settings-color-row">
                <input
                  type="color"
                  value={settings.theme.cursor}
                  onChange={(e) => updateTheme("cursor", e.target.value)}
                />
                <input
                  type="text"
                  value={settings.theme.cursor}
                  onChange={(e) => updateTheme("cursor", e.target.value)}
                />
              </div>
            </label>
          </section>
        </div>

        <div className="settings-footer">
          <button className="settings-btn-cancel" onClick={onCancel} type="button">
            Cancel
          </button>
          <button className="settings-btn-save" onClick={() => onSave(settings)} type="button">
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
