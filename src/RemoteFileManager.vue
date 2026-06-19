<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RemoteDirectoryListing, RemoteFileEntry, RemoteTransferMode, RemoteTransferProgress, SshConfig } from "./types";
import "./RemoteFileManager.css";

const props = defineProps<{
  sessionId: string;
  sshConfig: SshConfig;
}>();

const emit = defineEmits<{
  close: [];
}>();

const entries = ref<RemoteFileEntry[]>([]);
const currentPath = ref("");
const parentPath = ref<string | null>(null);
const selectedPath = ref<string | null>(null);
const transferMode = ref<RemoteTransferMode>("sftp");
// Resume assumes the existing file is a prefix of the selected file, so keep it opt-in.
const resumeTransfers = ref(false);
const downloadDirectory = ref("~/AuraTerm/downloads");
const loading = ref(false);
const busy = ref(false);
const errorMessage = ref("");
const statusMessage = ref("");
const uploadInputRef = ref<HTMLInputElement | null>(null);
const transfer = ref<RemoteTransferProgress | null>(null);
type UploadQueueItem = { id: string; name: string; status: "waiting" | "active" | "completed" | "failed" };
const uploadQueue = ref<UploadQueueItem[]>([]);
const draggingFiles = ref(false);
const editorPath = ref<string | null>(null);
const editorName = ref("");
const editorContent = ref("");
const editorOriginal = ref("");
const editorLoading = ref(false);
const editorSaving = ref(false);
const cleanupFns: UnlistenFn[] = [];

const selectedEntry = computed(() => (
  entries.value.find((entry: RemoteFileEntry) => entry.path === selectedPath.value) ?? null
));
const editorDirty = computed(() => editorContent.value !== editorOriginal.value);

const breadcrumbs = computed(() => {
  if (!currentPath.value) {
    return [] as Array<{ label: string; path: string }>;
  }

  if (currentPath.value === "/") {
    return [{ label: "/", path: "/" }];
  }

  const parts = currentPath.value.split("/").filter(Boolean);
  const segments: Array<{ label: string; path: string }> = [{ label: "/", path: "/" }];
  let accumulated = "";
  for (const part of parts) {
    accumulated += `/${part}`;
    segments.push({ label: part, path: accumulated });
  }
  return segments;
});

const transferPercent = computed(() => {
  if (!transfer.value?.totalBytes || transfer.value.totalBytes <= 0) {
    return null;
  }

  const ratio = transfer.value.transferredBytes / transfer.value.totalBytes;
  return Math.max(0, Math.min(100, Math.round(ratio * 100)));
});

const transferBarWidth = computed(() => {
  if (!transfer.value) {
    return 0;
  }
  if (transfer.value.status === "completed") {
    return 100;
  }
  if (transferPercent.value !== null) {
    return Math.max(6, transferPercent.value);
  }
  return 12;
});

const transferSummary = computed(() => {
  if (!transfer.value) {
    return "";
  }

  switch (transfer.value.status) {
    case "completed":
      return transfer.value.direction === "upload" ? "Upload Completed" : "Download Completed";
    case "failed":
      return transfer.value.direction === "upload" ? "Upload Failed" : "Download Failed";
    default:
      return transfer.value.direction === "upload" ? "Uploading" : "Downloading";
  }
});

const transferSourceLabel = computed(() => {
  if (!transfer.value) {
    return "";
  }
  return transfer.value.direction === "upload" ? "Local File" : "Remote File";
});

const transferSourcePath = computed(() => {
  if (!transfer.value) {
    return "";
  }
  if (transfer.value.direction === "upload") {
    return transfer.value.localPath || transfer.value.fileName;
  }
  return transfer.value.remotePath;
});

const transferDestinationLabel = computed(() => {
  if (!transfer.value) {
    return "";
  }
  return transfer.value.direction === "upload" ? "Remote Destination" : "Local Destination";
});

const transferDestinationPath = computed(() => {
  if (!transfer.value) {
    return "";
  }
  if (transfer.value.direction === "upload") {
    return transfer.value.remotePath;
  }
  return transfer.value.localPath || downloadDirectory.value;
});

const transferProgressLabel = computed(() => {
  if (!transfer.value) {
    return "";
  }
  if (transferPercent.value !== null) {
    return `${transferPercent.value}%`;
  }
  return formatSize(transfer.value.transferredBytes);
});

const transferBytesLabel = computed(() => {
  if (!transfer.value) {
    return "";
  }
  if (transfer.value.totalBytes === 0) {
    return "0 B / 0 B";
  }
  if (!transfer.value.totalBytes || transfer.value.totalBytes < 0) {
    return formatSize(transfer.value.transferredBytes);
  }
  return `${formatSize(transfer.value.transferredBytes)} / ${formatSize(transfer.value.totalBytes)}`;
});

function formatSize(size: number) {
  if (!Number.isFinite(size) || size < 1024) {
    return `${size || 0} B`;
  }

  const units = ["KB", "MB", "GB", "TB"];
  let value = size;
  let unitIndex = -1;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function formatDate(timestamp?: number | null) {
  if (!timestamp) {
    return "-";
  }
  return new Date(timestamp * 1000).toLocaleString();
}

function describeError(error: unknown) {
  const message = String(error);
  if (message.toLowerCase().includes("not ready")) {
    return "SSH session is still being established, please refresh later.";
  }
  return message;
}

function setError(error: unknown) {
  errorMessage.value = describeError(error);
  statusMessage.value = "";
}

function setStatus(message: string) {
  statusMessage.value = message;
  errorMessage.value = "";
}

function joinRemotePath(base: string, name: string) {
  const cleanName = name.replace(/^\/+|\/+$/g, "");
  if (!base) {
    return cleanName;
  }
  if (base === "/") {
    return `/${cleanName}`;
  }
  return `${base.replace(/\/+$/, "")}/${cleanName}`;
}

function markTransferFailed(error: unknown) {
  if (!transfer.value) {
    return;
  }

  transfer.value = {
    ...transfer.value,
    status: "failed",
    message: describeError(error),
  };
}

function resetTransferState() {
  transfer.value = null;
}

async function loadDirectory(path?: string | null) {
  loading.value = true;
  errorMessage.value = "";

  try {
    const listing = await invoke<RemoteDirectoryListing>("ssh_list_remote_dir", {
      id: props.sessionId,
      path: path ?? null,
    });
    currentPath.value = listing.path;
    parentPath.value = listing.parent ?? null;
    entries.value = listing.entries;
    if (!entries.value.some((entry: RemoteFileEntry) => entry.path === selectedPath.value)) {
      selectedPath.value = null;
    }
  } catch (error) {
    entries.value = [];
    setError(error);
  } finally {
    loading.value = false;
  }
}

function selectEntry(entry: RemoteFileEntry) {
  selectedPath.value = selectedPath.value === entry.path ? null : entry.path;
}

function openEntry(entry: RemoteFileEntry) {
  if (busy.value) {
    return;
  }
  if (entry.isDir) {
    void loadDirectory(entry.path);
  } else {
    void openRemoteEditor(entry);
  }
}

function triggerUpload() {
  uploadInputRef.value?.click();
}

async function handleUploadChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files ?? []);
  await uploadFiles(files);
  input.value = "";
}

async function uploadFiles(files: File[]) {
  if (files.length === 0) {
    return;
  }

  const queued: UploadQueueItem[] = files.map((file) => ({
    id: crypto.randomUUID(),
    name: file.name,
    status: "waiting",
  }));
  uploadQueue.value = [...uploadQueue.value.filter((item) => item.status === "active"), ...queued];
  statusMessage.value = "";
  errorMessage.value = "";
  busy.value = true;
  try {
    for (let index = 0; index < files.length; index += 1) {
      const file = files[index];
      const queueItem = queued[index];
      uploadQueue.value = uploadQueue.value.map((item) => (
        item.id === queueItem.id ? { ...item, status: "active" } : item
      ));
      transfer.value = {
        id: props.sessionId,
        direction: "upload",
        status: "started",
        mode: transferMode.value,
        fileName: file.name,
        remotePath: joinRemotePath(currentPath.value || ".", file.name),
        localPath: file.name,
        transferredBytes: 0,
        totalBytes: file.size,
        message: null,
      };

      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      await invoke("ssh_upload_file", {
        id: props.sessionId,
        remoteDir: currentPath.value || ".",
        fileName: file.name,
        data: bytes,
        mode: transferMode.value,
        resume: transferMode.value === "sftp" && resumeTransfers.value,
      });
      uploadQueue.value = uploadQueue.value.map((item) => (
        item.id === queueItem.id ? { ...item, status: "completed" } : item
      ));
    }
    setStatus(`Uploaded ${files.length} file(s) to ${currentPath.value || "."}`);
    await loadDirectory(currentPath.value);
  } catch (error) {
    uploadQueue.value = uploadQueue.value.map((item) => (
      item.status === "active" ? { ...item, status: "failed" } : item
    ));
    markTransferFailed(error);
    setError(error);
  } finally {
    busy.value = false;
  }
}

function handleFileDrop(event: DragEvent) {
  draggingFiles.value = false;
  void uploadFiles(Array.from(event.dataTransfer?.files ?? []));
}

async function openRemoteEditor(entry: RemoteFileEntry) {
  if (entry.size > 2 * 1024 * 1024) {
    setError("Remote quick edit supports files up to 2 MiB.");
    return;
  }
  editorLoading.value = true;
  errorMessage.value = "";
  editorPath.value = entry.path;
  editorName.value = entry.name;
  try {
    const content = await invoke<string>("ssh_read_remote_text_file", {
      id: props.sessionId,
      path: entry.path,
    });
    editorContent.value = content;
    editorOriginal.value = content;
  } catch (error) {
    editorPath.value = null;
    editorName.value = "";
    setError(error);
  } finally {
    editorLoading.value = false;
  }
}

async function saveRemoteEditor() {
  if (!editorPath.value || editorSaving.value) return;
  editorSaving.value = true;
  try {
    await invoke("ssh_write_remote_text_file", {
      id: props.sessionId,
      path: editorPath.value,
      content: editorContent.value,
    });
    editorOriginal.value = editorContent.value;
    setStatus(`Saved ${editorName.value}`);
    await loadDirectory(currentPath.value);
  } catch (error) {
    setError(error);
  } finally {
    editorSaving.value = false;
  }
}

function closeRemoteEditor() {
  if (editorDirty.value && !window.confirm("Discard unsaved remote file changes?")) return;
  editorPath.value = null;
  editorName.value = "";
  editorContent.value = "";
  editorOriginal.value = "";
}

function closeManager() {
  if (editorDirty.value && !window.confirm("Close Remote Files and discard unsaved changes?")) return;
  emit("close");
}

function handleEditorKeydown(event: KeyboardEvent) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void saveRemoteEditor();
  }
}

async function downloadSelected() {
  if (!selectedEntry.value || selectedEntry.value.isDir) {
    return;
  }

  statusMessage.value = "";
  errorMessage.value = "";
  busy.value = true;
  try {
    transfer.value = {
      id: props.sessionId,
      direction: "download",
      status: "started",
      mode: transferMode.value,
      fileName: selectedEntry.value.name,
      remotePath: selectedEntry.value.path,
      localPath: downloadDirectory.value,
      transferredBytes: 0,
      totalBytes: selectedEntry.value.size,
      message: null,
    };

    const localPath = await invoke<string>("ssh_download_file", {
      id: props.sessionId,
      remotePath: selectedEntry.value.path,
      localDir: downloadDirectory.value || null,
      expectedSize: selectedEntry.value.size,
      mode: transferMode.value,
      resume: transferMode.value === "sftp" && resumeTransfers.value,
    });
    setStatus(`Downloaded to ${localPath}`);
  } catch (error) {
    markTransferFailed(error);
    setError(error);
  } finally {
    busy.value = false;
  }
}

async function deleteSelected() {
  if (!selectedEntry.value) {
    return;
  }

  const label = selectedEntry.value.isDir ? "folder" : "file";
  if (!window.confirm(`Are you sure you want to delete the ${label} "${selectedEntry.value.name}"?`)) {
    return;
  }

  busy.value = true;
  try {
    await invoke("ssh_remove_remote_entry", {
      id: props.sessionId,
      path: selectedEntry.value.path,
      isDir: selectedEntry.value.isDir,
    });
    setStatus(`Deleted ${selectedEntry.value.name}`);
    selectedPath.value = null;
    await loadDirectory(currentPath.value);
  } catch (error) {
    setError(error);
  } finally {
    busy.value = false;
  }
}

async function createFolder() {
  const name = window.prompt("New Folder Name", "new-folder");
  if (!name) {
    return;
  }

  busy.value = true;
  try {
    await invoke("ssh_create_remote_dir", {
      id: props.sessionId,
      parentPath: currentPath.value || ".",
      name,
    });
    setStatus(`Created folder ${name}`);
    await loadDirectory(currentPath.value);
  } catch (error) {
    setError(error);
  } finally {
    busy.value = false;
  }
}

watch(() => props.sessionId, () => {
  selectedPath.value = null;
  currentPath.value = "";
  parentPath.value = null;
  entries.value = [];
  errorMessage.value = "";
  statusMessage.value = "";
  resetTransferState();
  editorPath.value = null;
  editorContent.value = "";
  editorOriginal.value = "";
  void loadDirectory();
});

onMounted(async () => {
  try {
    cleanupFns.push(await listen<RemoteTransferProgress>("ssh-transfer-progress", (event: { payload: RemoteTransferProgress }) => {
      if (event.payload.id !== props.sessionId) {
        return;
      }
      transfer.value = {
        ...event.payload,
        message: null,
      };
    }));
  } catch (error) {
    console.error("Failed to setup SSH transfer progress listener:", error);
  }

  void loadDirectory();
});

onBeforeUnmount(() => {
  while (cleanupFns.length > 0) {
    const cleanup = cleanupFns.pop();
    cleanup?.();
  }
});
</script>

<template>
  <aside class="remote-file-manager">
    <div class="remote-file-manager-header">
      <div>
        <div class="remote-file-manager-title">Remote Files</div>
        <div class="remote-file-manager-subtitle">{{ sshConfig.user }}@{{ sshConfig.host }}:{{ sshConfig.port }}</div>
      </div>
      <button class="remote-file-manager-close" type="button" @click="closeManager">×</button>
    </div>

    <div class="remote-file-manager-toolbar">
      <div class="remote-file-manager-mode-switch">
        <button
          type="button"
          class="remote-file-manager-mode-btn"
          :class="{ active: transferMode === 'sftp' }"
          :disabled="busy"
          @click="transferMode = 'sftp'"
        >SFTP</button>
        <button
          type="button"
          class="remote-file-manager-mode-btn"
          :class="{ active: transferMode === 'scp' }"
          :disabled="busy"
          @click="transferMode = 'scp'"
        >SCP</button>
      </div>

      <div class="remote-file-manager-actions">
        <button type="button" title="Go Up" :disabled="loading || busy || !parentPath" @click="void loadDirectory(parentPath)">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m18 15-6-6-6 6"/></svg>
        </button>
        <button type="button" title="Download" :disabled="loading || busy || !selectedEntry || selectedEntry.isDir" @click="void downloadSelected()">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/></svg>
        </button>
        <button type="button" title="Upload" :disabled="loading || busy" @click="triggerUpload">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" x2="12" y1="3" y2="15"/></svg>
        </button>
        <button type="button" title="Refresh" :disabled="loading || busy" @click="void loadDirectory(currentPath)">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M3 21v-5h5"/></svg>
        </button>
        <button type="button" title="New Folder" :disabled="loading || busy" @click="void createFolder()">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/><line x1="12" x2="12" y1="10" y2="16"/><line x1="9" x2="15" y1="13" y2="13"/></svg>
        </button>
        <button type="button" class="danger" title="Delete" :disabled="loading || busy || !selectedEntry" @click="void deleteSelected()">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/></svg>
        </button>
      </div>

      <label class="remote-file-manager-resume">
        <input v-model="resumeTransfers" type="checkbox" :disabled="transferMode !== 'sftp' || busy">
        Resume partial SFTP transfers
      </label>

      <input ref="uploadInputRef" type="file" multiple hidden @change="handleUploadChange">
    </div>

    <div v-if="editorPath" class="remote-file-editor-toolbar">
      <button type="button" :disabled="editorSaving" @click="closeRemoteEditor">Back</button>
      <strong :title="editorPath">{{ editorName }}</strong>
      <span v-if="editorDirty">Modified</span>
      <button type="button" :disabled="editorSaving || !editorDirty" @click="void saveRemoteEditor()">
        {{ editorSaving ? 'Saving...' : 'Save' }}
      </button>
    </div>

    <div v-else class="remote-file-manager-pathbar">
      <button
        v-for="segment in breadcrumbs"
        :key="segment.path"
        type="button"
        class="remote-file-manager-path-segment"
        @click="void loadDirectory(segment.path)"
      >
        {{ segment.label }}
      </button>
    </div>

    <div
      v-if="!editorPath"
      class="remote-file-manager-list"
      :class="{ 'dragging-files': draggingFiles }"
      @dragenter.prevent="draggingFiles = true"
      @dragover.prevent="draggingFiles = true"
      @dragleave.prevent="draggingFiles = false"
      @drop.prevent="handleFileDrop"
    >
      <div v-if="draggingFiles" class="remote-file-manager-drop-hint">Drop files to upload to {{ currentPath }}</div>
      <div class="remote-file-manager-list-head">
        <span>Name</span>
        <span>Size</span>
        <span>Modified</span>
      </div>

      <div v-if="loading" class="remote-file-manager-empty">Loading remote directory...</div>
      <div v-else-if="entries.length === 0" class="remote-file-manager-empty">No files in this directory.</div>

      <button
        v-for="entry in entries"
        :key="entry.path"
        type="button"
        class="remote-file-manager-row"
        :class="{ selected: selectedPath === entry.path }"
        @click="selectEntry(entry)"
        @dblclick="openEntry(entry)"
      >
        <span class="remote-file-manager-name">
          <span class="remote-file-manager-icon">{{ entry.isDir ? "📁" : entry.kind === "symlink" ? "🔗" : "📄" }}</span>
          <span class="remote-file-manager-name-text">{{ entry.name }}</span>
          <span class="remote-file-manager-permissions">{{ entry.permissions }}</span>
        </span>
        <span class="remote-file-manager-meta">{{ entry.isDir ? "-" : formatSize(entry.size) }}</span>
        <span class="remote-file-manager-meta">{{ formatDate(entry.modifiedAt) }}</span>
      </button>
    </div>

    <div v-else class="remote-file-editor">
      <div v-if="editorLoading" class="remote-file-manager-empty">Loading remote text...</div>
      <textarea
        v-else
        v-model="editorContent"
        class="remote-file-editor-textarea"
        :aria-label="`Edit ${editorName}`"
        autocapitalize="none"
        autocorrect="off"
        spellcheck="false"
        @keydown="handleEditorKeydown"
      />
    </div>

    <div class="remote-file-manager-footer">
      <div v-if="transfer" class="remote-file-manager-transfer" :class="transfer.status">
        <div class="remote-file-manager-transfer-header">
          <div>
            <div class="remote-file-manager-transfer-title">{{ transferSummary }}</div>
            <div class="remote-file-manager-transfer-subtitle">{{ transfer.mode.toUpperCase() }} · {{ transfer.fileName }}</div>
          </div>
          <div class="remote-file-manager-transfer-percent">{{ transferProgressLabel }}</div>
        </div>

        <div class="remote-file-manager-progress-track">
          <div class="remote-file-manager-progress-fill" :style="{ width: `${transferBarWidth}%` }"></div>
        </div>

        <div class="remote-file-manager-transfer-bytes">{{ transferBytesLabel }}</div>

        <div class="remote-file-manager-transfer-paths">
          <div class="remote-file-manager-transfer-path">
            <span>{{ transferSourceLabel }}</span>
            <strong>{{ transferSourcePath }}</strong>
          </div>
          <div class="remote-file-manager-transfer-path">
            <span>{{ transferDestinationLabel }}</span>
            <strong>{{ transferDestinationPath }}</strong>
          </div>
        </div>

        <div v-if="transfer.message" class="remote-file-manager-transfer-note">{{ transfer.message }}</div>
      </div>

      <div v-if="uploadQueue.length > 1" class="remote-file-manager-queue">
        <div class="remote-file-manager-queue-title">Upload queue</div>
        <div v-for="item in uploadQueue" :key="item.id" class="remote-file-manager-queue-item" :class="item.status">
          <span>{{ item.name }}</span><span>{{ item.status }}</span>
        </div>
      </div>

      <label class="remote-file-manager-download-dir">
        <span>Download to</span>
        <input
          v-model="downloadDirectory"
          type="text"
          placeholder="~/AuraTerm/downloads"
          autocapitalize="none"
          autocorrect="off"
          spellcheck="false"
        >
      </label>

      <div v-if="statusMessage" class="remote-file-manager-status success">{{ statusMessage }}</div>
      <div v-if="errorMessage" class="remote-file-manager-status error">{{ errorMessage }}</div>
    </div>
  </aside>
</template>
