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

          {/* ── Keyboard & Mouse ── */}
          <section className="settings-section">
            <h3>Keyboard &amp; Mouse</h3>

            <label className="settings-field settings-field--toggle">
              <span>
                <strong>Copy on select</strong>
                <small>选中文本后自动复制到剪贴板；Ctrl+C 有选中时消费按键（不发 ^C 给 PTY）</small>
              </span>
              <input
                type="checkbox"
                className="settings-toggle"
                checked={settings.ctrlCCopy}
                onChange={(e) => update("ctrlCCopy", e.target.checked)}
              />
            </label>

            <label className="settings-field settings-field--toggle">
              <span>
                <strong>Ctrl+V</strong> Paste from clipboard
                <small>Ctrl+V 将剪贴板内容粘贴到终端</small>
              </span>
              <input
                type="checkbox"
                className="settings-toggle"
                checked={settings.ctrlVPaste}
                onChange={(e) => update("ctrlVPaste", e.target.checked)}
              />
            </label>

            <label className="settings-field settings-field--toggle">
              <span>
                <strong>Middle-click</strong> Paste
                <small>鼠标中键点击将剪贴板内容粘贴到终端</small>
              </span>
              <input
                type="checkbox"
                className="settings-toggle"
                checked={settings.middleClickPaste}
                onChange={(e) => update("middleClickPaste", e.target.checked)}
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
