import { describe, expect, it } from "vitest";

import {
  DEFAULT_SETTINGS,
  TERMINAL_THEME_PRESETS,
  deriveUiTheme,
  getTerminalThemeAppearance,
  resolveUiThemeAppearance,
  type TerminalTheme,
} from "../settings";

/**
 * Unit tests for deriveUiTheme and its appearance-detection helpers.
 *
 * These helpers produce the CSS variable map consumed by every dialog,
 * titlebar, and panel in the app. A regression here silently breaks
 * theming across the whole UI, so we assert:
 *   - appearance detection (light vs dark via relative luminance)
 *   - correct source selection under each uiThemeMode
 *   - required CSS variables are populated with well-formed values
 *   - variables differ between light and dark modes (sanity check)
 */

const LIGHT_PRESET = TERMINAL_THEME_PRESETS.find((p) => p.id === "paper-light")!;
const DARK_PRESET = TERMINAL_THEME_PRESETS.find((p) => p.id === "aura-dark")!;

const REQUIRED_VARIABLES = [
  "--app-bg",
  "--app-text",
  "--app-text-secondary",
  "--app-text-muted",
  "--app-accent",
  "--app-accent-contrast",
  "--app-border",
  "--app-success",
  "--app-warning",
  "--app-danger",
  "--app-panel-bg",
  "--app-selection",
  "--app-dialog-bg",
  // Legacy-compat aliases still consumed by older .vue files.
  "--bg-dialog",
  "--fg-dialog",
  "--accent",
];

function isColorValue(value: string) {
  return /^(rgb|rgba|#|linear-gradient|radial-gradient)/.test(value);
}

describe("getTerminalThemeAppearance", () => {
  it("classifies a bright background as light", () => {
    expect(getTerminalThemeAppearance(LIGHT_PRESET.theme)).toBe("light");
  });

  it("classifies a dim background as dark", () => {
    expect(getTerminalThemeAppearance(DARK_PRESET.theme)).toBe("dark");
  });

  it("defaults to dark when parsing fails for nonsense values", () => {
    const bogus: TerminalTheme = {
      ...DARK_PRESET.theme,
      background: "not-a-color",
    };
    // Falls back to DEFAULT_THEME.background which is very dark.
    expect(getTerminalThemeAppearance(bogus)).toBe("dark");
  });
});

describe("resolveUiThemeAppearance", () => {
  it("returns the fixed appearance when uiThemeMode is explicitly set", () => {
    expect(resolveUiThemeAppearance(DARK_PRESET.theme, "light")).toBe("light");
    expect(resolveUiThemeAppearance(LIGHT_PRESET.theme, "dark")).toBe("dark");
  });

  it("follows terminal appearance when uiThemeMode is 'follow-terminal'", () => {
    expect(resolveUiThemeAppearance(LIGHT_PRESET.theme, "follow-terminal")).toBe(
      "light",
    );
    expect(resolveUiThemeAppearance(DARK_PRESET.theme, "follow-terminal")).toBe(
      "dark",
    );
  });
});

describe("deriveUiTheme", () => {
  it("produces every required CSS variable with a color-like value", () => {
    const result = deriveUiTheme(DARK_PRESET.theme, "follow-terminal");

    for (const name of REQUIRED_VARIABLES) {
      expect(result.variables[name], `missing ${name}`).toBeDefined();
      expect(
        isColorValue(result.variables[name]),
        `${name} = ${result.variables[name]} is not color-like`,
      ).toBe(true);
    }
  });

  it("reports matching appearance on the returned theme", () => {
    const dark = deriveUiTheme(DARK_PRESET.theme, "follow-terminal");
    expect(dark.appearance).toBe("dark");

    const light = deriveUiTheme(LIGHT_PRESET.theme, "follow-terminal");
    expect(light.appearance).toBe("light");
  });

  it("overrides the source theme when uiThemeMode is 'light' or 'dark'", () => {
    // Even when given a dark terminal theme, forcing uiThemeMode=light
    // must yield a light-appearance CSS variable set.
    const forcedLight = deriveUiTheme(DARK_PRESET.theme, "light");
    expect(forcedLight.appearance).toBe("light");

    const forcedDark = deriveUiTheme(LIGHT_PRESET.theme, "dark");
    expect(forcedDark.appearance).toBe("dark");

    // Accent-contrast should flip with appearance.
    expect(forcedLight.variables["--app-accent-contrast"]).not.toBe(
      forcedDark.variables["--app-accent-contrast"],
    );
  });

  it("yields different background between light and dark modes", () => {
    const light = deriveUiTheme(LIGHT_PRESET.theme, "light");
    const dark = deriveUiTheme(DARK_PRESET.theme, "dark");

    expect(light.variables["--app-bg"]).not.toBe(dark.variables["--app-bg"]);
    expect(light.variables["--app-text"]).not.toBe(dark.variables["--app-text"]);
  });

  it("uses the default uiThemeMode when argument is omitted", () => {
    const explicit = deriveUiTheme(DARK_PRESET.theme, DEFAULT_SETTINGS.uiThemeMode);
    const implicit = deriveUiTheme(DARK_PRESET.theme);
    expect(implicit).toEqual(explicit);
  });

  it("is deterministic for the same input", () => {
    const a = deriveUiTheme(DARK_PRESET.theme, "follow-terminal");
    const b = deriveUiTheme(DARK_PRESET.theme, "follow-terminal");
    expect(a).toEqual(b);
  });

  it("handles invalid color strings without throwing", () => {
    const malformed: TerminalTheme = {
      ...DARK_PRESET.theme,
      background: "####",
      foreground: "oklch(bogus)",
      blue: "rgb(not,a,color)",
    };

    expect(() => deriveUiTheme(malformed, "follow-terminal")).not.toThrow();
    const derived = deriveUiTheme(malformed, "follow-terminal");
    expect(isColorValue(derived.variables["--app-bg"])).toBe(true);
  });
});
