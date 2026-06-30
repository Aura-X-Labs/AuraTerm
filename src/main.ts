import { createApp } from "vue";
import App from "./App.vue";
import { invoke } from "@tauri-apps/api/core";
import { i18n, setLanguage } from "./i18n";

// Best-guess locale from the system before settings load; App.vue re-applies
// the persisted preference once `get_settings` resolves.
setLanguage("system");

// Apply global styles
const style = document.createElement("style");
style.textContent = `
  html, body, #root {
    margin: 0;
    padding: 0;
    width: 100%;
    height: 100%;
    overflow: hidden;
    background-color: var(--app-bg, #1e1e1e);
    color: var(--app-text, #ffffff);
    font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
  }
`;
document.head.appendChild(style);

// Get startup directory from command line args
let startupDir: string | null = null;
invoke<string | null>("get_startup_dir").then((dir) => {
  startupDir = dir;
}).catch((error) => {
  console.error("Failed to get startup dir:", error);
});

// Expose startup directory globally for access in components
declare global {
  interface Window {
    getStartupDir: () => string | null;
  }
}
window.getStartupDir = () => startupDir;

createApp(App).use(i18n).mount("#root");