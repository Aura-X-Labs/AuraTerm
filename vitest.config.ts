import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "jsdom",
    include: ["src/**/__tests__/**/*.test.ts", "src/**/*.test.ts"],
    globals: false,
    restoreMocks: true,
    clearMocks: true,
    // The TS source pulls in Tauri/xterm modules via other files; we only
    // unit-test pure logic modules, so no setup file is required.
  },
});
