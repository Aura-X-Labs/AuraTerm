<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import logo from "./logo.png";
import "./AboutDialog.css";

const emit = defineEmits<{
  close: [];
}>();

const version = ref("unknown");
const buildTime = ref("");

onMounted(async () => {
  try {
    const info = await invoke<{ version: string; build_time: string }>("get_version_info");
    version.value = info.version;
    buildTime.value = info.build_time;
  } catch (error) {
    console.error("Failed to get version info", error);
  }
});

function openExternal(url: string) {
  window.open(url, "_blank");
}
</script>

<template>
  <div class="about-overlay" @click="emit('close')">
    <div class="about-dialog" @click.stop>
      <div class="about-header">
        <h2>About AuraTerm</h2>
        <button class="about-close-btn" type="button" @click="emit('close')">×</button>
      </div>

      <div class="about-body">
        <div class="about-logo">
          <img :src="logo" alt="AuraTerm Logo" class="about-logo-img" />
        </div>

        <div class="about-content">
          <h3>AuraTerm</h3>
          <p class="about-version">Version {{ version }}</p>
          <p v-if="buildTime" class="about-build-time">Built: {{ buildTime }}</p>

<p class="about-description">
  A powerful terminal emulator supporting SSH, Telnet, Serial and local shell sessions.
</p>

<div class="about-info">
            <p><strong>Built with:</strong> Tauri + Vue + TypeScript</p>
            <p><strong>License:</strong> MIT</p>
          </div>

          <div class="about-links">
            <button class="about-link-btn" type="button" @click="openExternal('https://github.com/Aura-X-Labs/AuraTerm')">
              GitHub Repository
            </button>
            <button class="about-link-btn" type="button" @click="openExternal('https://github.com/Aura-X-Labs/AuraTerm/issues')">
              Report Issues
            </button>
          </div>
        </div>
      </div>

      <div class="about-footer">
        <button class="about-ok-btn" type="button" @click="emit('close')">OK</button>
      </div>
    </div>
  </div>
</template>