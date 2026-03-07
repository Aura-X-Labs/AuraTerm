<script setup lang="ts">
import "./AboutDialog.css";

const emit = defineEmits<{
  close: [];
}>();

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
        <div class="about-logo">🖥️</div>

        <div class="about-content">
          <h3>AuraTerm</h3>
          <p class="about-version">Version 0.1.0</p>

          <p class="about-description">
            A modern terminal application built with Tauri and Vue, featuring SSH, Telnet, and Serial connections.
          </p>

          <div class="about-features">
            <h4>Features</h4>
            <ul>
              <li>SSH, Telnet, and Serial connections</li>
              <li>Multi-tab support</li>
              <li>Bookmark management</li>
              <li>Customizable settings</li>
              <li>Light and dark theme support</li>
            </ul>
          </div>

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