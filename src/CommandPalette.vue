<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { PaletteCommand } from "./types";

const props = defineProps<{
  commands: PaletteCommand[];
}>();

const emit = defineEmits<{
  /** A command was picked; the caller should not steal focus back. */
  close: [];
  /** Escape or a backdrop click — nothing ran, so focus belongs to the terminal again. */
  dismiss: [];
}>();

const query = ref("");
const activeIndex = ref(0);
const inputRef = ref<HTMLInputElement | null>(null);
const listRef = ref<HTMLDivElement | null>(null);

interface ScoredCommand {
  command: PaletteCommand;
  score: number;
}

/**
 * Lightweight fuzzy match: prefers a contiguous substring (earlier = better),
 * falls back to an in-order subsequence. Returns -1 when the query cannot be
 * matched at all.
 */
function fuzzyScore(haystack: string, needle: string): number {
  if (!needle) {
    return 0;
  }
  const text = haystack.toLowerCase();
  const q = needle.toLowerCase();

  const directIndex = text.indexOf(q);
  if (directIndex >= 0) {
    // Contiguous match: 1000 base, minus how deep it starts.
    return 1000 - Math.min(directIndex, 500);
  }

  // Subsequence match.
  let textPos = 0;
  let score = 0;
  let streak = 0;
  for (const char of q) {
    const found = text.indexOf(char, textPos);
    if (found < 0) {
      return -1;
    }
    streak = found === textPos ? streak + 1 : 0;
    score += 10 + streak * 2 - Math.min(found - textPos, 8);
    textPos = found + 1;
  }
  return score;
}

const filtered = computed<PaletteCommand[]>(() => {
  const available = props.commands.filter((command) => command.enabled !== false);
  const q = query.value.trim();
  if (!q) {
    return available;
  }
  const scored: ScoredCommand[] = [];
  for (const command of available) {
    const haystack = `${command.title} ${command.subtitle ?? ""} ${command.group ?? ""} ${command.keywords ?? ""}`;
    const score = fuzzyScore(haystack, q);
    if (score >= 0) {
      scored.push({ command, score });
    }
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.map((item) => item.command);
});

watch(filtered, () => {
  activeIndex.value = 0;
});

function clampIndex(index: number): number {
  const count = filtered.value.length;
  if (count === 0) {
    return 0;
  }
  return (index + count) % count;
}

function move(delta: number) {
  activeIndex.value = clampIndex(activeIndex.value + delta);
  void nextTick(() => {
    const list = listRef.value;
    const node = list?.querySelector<HTMLElement>(`[data-index="${activeIndex.value}"]`);
    node?.scrollIntoView({ block: "nearest" });
  });
}

async function runCommand(command: PaletteCommand | undefined) {
  if (!command) {
    return;
  }
  emit("close");
  try {
    await command.run();
  } catch (error) {
    console.error("Command palette action failed", error);
  }
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "ArrowDown") {
    event.preventDefault();
    move(1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    move(-1);
  } else if (event.key === "Enter") {
    event.preventDefault();
    void runCommand(filtered.value[activeIndex.value]);
  } else if (event.key === "Escape") {
    event.preventDefault();
    emit("dismiss");
  }
}

// Listen on the window rather than the overlay: clicking anywhere in the
// palette that is not the input (the list padding, a gap between rows) moves
// focus to <body>, which sits outside the overlay, so a bubbling listener
// would stop seeing keys.
onMounted(() => {
  void nextTick(() => inputRef.value?.focus());
  window.addEventListener("keydown", handleKeydown);
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", handleKeydown);
});
</script>

<template>
  <div class="palette-overlay" @click.self="emit('dismiss')">
    <div class="palette" role="dialog" :aria-label="$t('palette.ariaLabel')">
      <input
        ref="inputRef"
        v-model="query"
        class="palette-input"
        type="text"
        :placeholder="$t('palette.placeholder')"
        spellcheck="false"
        autocomplete="off"
      />
      <div ref="listRef" class="palette-list">
        <div v-if="filtered.length === 0" class="palette-empty">{{ $t('palette.empty') }}</div>
        <button
          v-for="(command, index) in filtered"
          :key="command.id"
          class="palette-item"
          :class="{ active: index === activeIndex }"
          :data-index="index"
          type="button"
          @click="runCommand(command)"
          @mousemove="activeIndex = index"
        >
          <span class="palette-item-main">
            <span class="palette-item-title">{{ command.title }}</span>
            <span v-if="command.subtitle" class="palette-item-subtitle">{{ command.subtitle }}</span>
          </span>
          <span v-if="command.group" class="palette-item-group">{{ command.group }}</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.palette-overlay {
  position: fixed;
  inset: 0;
  z-index: 70;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 12vh;
  background: rgba(0, 0, 0, 0.4);
}

.palette {
  width: 580px;
  max-width: 92vw;
  background: var(--app-surface-1, var(--app-bg));
  border: 1px solid var(--app-border);
  border-radius: 12px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.palette-input {
  border: none;
  border-bottom: 1px solid var(--app-border);
  background: transparent;
  color: var(--app-text);
  padding: 14px 16px;
  font-size: 15px;
}

.palette-input:focus {
  outline: none;
}

.palette-list {
  max-height: 52vh;
  overflow-y: auto;
  padding: 6px;
}

.palette-empty {
  padding: 18px;
  text-align: center;
  color: var(--app-text-muted);
  font-size: 13px;
}

.palette-item {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  border: none;
  background: transparent;
  color: var(--app-text);
  text-align: left;
  padding: 9px 12px;
  border-radius: 8px;
  cursor: pointer;
}

.palette-item.active {
  background: var(--app-accent);
  color: var(--app-accent-contrast);
}

.palette-item-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.palette-item-title {
  font-size: 13px;
  font-weight: 600;
}

.palette-item-subtitle {
  font-size: 11px;
  color: var(--app-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.palette-item.active .palette-item-subtitle {
  color: var(--app-accent-contrast);
  opacity: 0.8;
}

.palette-item-group {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--app-text-muted);
  flex-shrink: 0;
}

.palette-item.active .palette-item-group {
  color: var(--app-accent-contrast);
  opacity: 0.85;
}
</style>
