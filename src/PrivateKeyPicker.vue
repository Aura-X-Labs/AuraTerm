<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";
import { MAX_PRIVATE_KEY_FILE_BYTES, privateKeyProblem, type PrivateKeyFileProblem } from "./privateKeyFile";

const props = withDefaults(defineProps<{
  /** Offer a "Generate" button; the parent makes the key on `generate`. */
  generatable?: boolean;
  /** Tighter metrics, for use inside a jump host card. */
  compact?: boolean;
  /** A problem the parent wants shown here, e.g. key generation failing. */
  error?: string;
}>(), {
  generatable: false,
  compact: false,
  error: "",
});

const emit = defineEmits<{
  generate: [];
}>();

/** The key's content. Filled from a local file here, or by the parent (a
 *  generated or previously saved key) — never typed or pasted. */
const privateKey = defineModel<string | undefined>();
/** Where the key came from, for display: the picked file's name, or whatever
 *  the parent sets. Plain local state when the parent does not bind it. */
const source = defineModel<string>("source", { default: "" });

/** What `ssh_pick_private_key_file` hands back; `content` is null for a file
 *  over the size limit, which the backend does not read. */
interface PickedPrivateKeyFile {
  name: string;
  content: string | null;
}

/** Kept as a code rather than its text so it follows a UI language change. */
const problem = ref<PrivateKeyFileProblem | "unreadable" | null>(null);

const hasKey = computed(() => Boolean(privateKey.value?.trim()));

const display = computed(() => {
  if (!hasKey.value) {
    return t("connect.noKeySelected");
  }
  // A key with no known source was loaded from the saved bookmark.
  return source.value || t("connect.keySaved");
});

const message = computed(() => {
  switch (problem.value) {
    case "tooLarge":
      return t("connect.keyTooLarge");
    case "publicKey":
      return t("connect.keyIsPublic");
    case "notAKey":
      return t("connect.keyNotRecognized");
    case "unreadable":
      return t("connect.keyReadFailed");
    default:
      return props.error;
  }
});

/** Opens the native file dialog, which starts in `~/.ssh` — a directory the
 *  webview's own file input cannot reach where the OS hides dot-directories.
 *  A pick that fails leaves the key already in place untouched. */
async function browse() {
  problem.value = null;

  try {
    const file = await invoke<PickedPrivateKeyFile | null>("ssh_pick_private_key_file", {
      maxBytes: MAX_PRIVATE_KEY_FILE_BYTES,
    });
    if (!file) {
      return;
    }
    if (file.content === null) {
      problem.value = "tooLarge";
      return;
    }
    const found = privateKeyProblem(file.content);
    if (found) {
      problem.value = found;
      return;
    }
    privateKey.value = file.content;
    source.value = file.name;
  } catch (error) {
    console.error("Failed to read private key file", error);
    problem.value = "unreadable";
  }
}

function clear() {
  privateKey.value = "";
  source.value = "";
  problem.value = null;
}
</script>

<template>
  <div class="private-key-picker" :class="{ compact: props.compact }">
    <div class="private-key-picker-row">
      <div class="private-key-display" :class="{ empty: !hasKey }" :title="display">{{ display }}</div>
      <button type="button" class="private-key-picker-btn" @click="browse">{{ $t('connect.browse') }}</button>
      <button
        v-if="props.generatable"
        type="button"
        class="private-key-picker-btn"
        @click="emit('generate')"
      >
        {{ $t('connect.generate') }}
      </button>
      <button v-if="hasKey" type="button" class="private-key-picker-btn" @click="clear">{{ $t('connect.clear') }}</button>
    </div>
    <div v-if="message" class="private-key-error" role="alert">{{ message }}</div>
  </div>
</template>

<style scoped>
/* Wraps, so the buttons drop below the name in the narrow bookmark detail rail
   instead of squeezing it to nothing. */
.private-key-picker-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.private-key-display {
  flex: 1 1 140px;
  min-width: 0;
  box-sizing: border-box;
  padding: 8px;
  background: var(--app-input-bg);
  border: 1px solid var(--app-border);
  border-radius: 4px;
  color: var(--app-text);
  font-size: 14px;
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.private-key-display.empty {
  color: var(--app-text-muted);
}

.private-key-picker-btn {
  flex-shrink: 0;
  padding: 0 12px;
  min-height: 32px;
  border: 1px solid var(--app-border);
  border-radius: 4px;
  background: var(--app-input-bg);
  color: var(--app-text-secondary);
  cursor: pointer;
}

.private-key-picker-btn:hover {
  background: var(--app-hover);
}

.private-key-error {
  margin-top: 6px;
  font-size: 12px;
  line-height: 1.4;
  color: var(--app-danger);
}

.compact .private-key-picker-row {
  gap: 6px;
}

.compact .private-key-display {
  padding: 7px;
  font-size: 13px;
}

.compact .private-key-picker-btn {
  min-height: 30px;
  padding: 0 9px;
}
</style>
