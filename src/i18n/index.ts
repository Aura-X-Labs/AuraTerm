import { reactive, type App } from "vue";
import en from "./locales/en";
import zhCN from "./locales/zh-CN";

/** Shape every locale must satisfy, derived from the English catalog. */
export type Messages = typeof en;

/** A concrete, translatable locale. */
export type Locale = "en" | "zh-CN";

/**
 * The persisted language preference. `"system"` resolves to a concrete
 * {@link Locale} at apply time based on the OS/browser language.
 */
export type AppLanguage = "system" | Locale;

/**
 * Options for the language picker, in display order. Concrete locales are shown
 * by their endonym (untranslated by convention); `"system"` is rendered via
 * `t("language.system")` so it follows the current UI language.
 */
export const LANGUAGE_OPTIONS: Array<{ value: AppLanguage; nativeLabel: string }> = [
  { value: "system", nativeLabel: "" },
  { value: "en", nativeLabel: "English" },
  { value: "zh-CN", nativeLabel: "简体中文" },
];

const messages: Record<Locale, Messages> = {
  en,
  "zh-CN": zhCN,
};

// Reactive so every `t()` call made during render re-runs when the locale
// changes — that is what makes language switching update the UI live.
const state = reactive<{ locale: Locale }>({ locale: "en" });

/** Best-effort mapping from the OS/browser language to a supported locale. */
export function detectSystemLocale(): Locale {
  const candidates = [
    typeof navigator !== "undefined" ? navigator.language : "",
    ...(typeof navigator !== "undefined" && Array.isArray(navigator.languages)
      ? navigator.languages
      : []),
  ];
  for (const candidate of candidates) {
    const lower = (candidate || "").toLowerCase();
    if (lower.startsWith("zh")) return "zh-CN";
    if (lower.startsWith("en")) return "en";
  }
  return "en";
}

/** Resolve a stored preference to the concrete locale to render. */
export function resolveLocale(language: AppLanguage): Locale {
  return language === "system" ? detectSystemLocale() : language;
}

/** The locale currently being rendered. */
export function currentLocale(): Locale {
  return state.locale;
}

/** Apply a language preference, updating every reactive `t()` consumer. */
export function setLanguage(language: AppLanguage): Locale {
  const locale = resolveLocale(language);
  state.locale = locale;
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
  }
  return locale;
}

function lookup(locale: Locale, key: string): string | undefined {
  let node: unknown = messages[locale];
  for (const part of key.split(".")) {
    if (node == null || typeof node !== "object") return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
}

/**
 * Translate a dotted `key` (e.g. `"menu.file"`), interpolating `{name}`
 * placeholders from `params`. Falls back to English, then to the raw key, so a
 * missing translation degrades gracefully instead of throwing.
 */
export function t(key: string, params?: Record<string, string | number>): string {
  let result = lookup(state.locale, key);
  if (result === undefined && state.locale !== "en") {
    result = lookup("en", key);
  }
  if (result === undefined) return key;
  if (params) {
    result = result.replace(/\{(\w+)\}/g, (match, name: string) =>
      params[name] !== undefined ? String(params[name]) : match,
    );
  }
  return result;
}

/** Composition-API accessor for use inside `<script setup>`. */
export function useI18n() {
  return { t, setLanguage, resolveLocale, currentLocale };
}

/** Vue plugin exposing `$t` to every template. */
export const i18n = {
  install(app: App) {
    app.config.globalProperties.$t = t;
  },
};

declare module "vue" {
  interface ComponentCustomProperties {
    $t: typeof t;
  }
}
