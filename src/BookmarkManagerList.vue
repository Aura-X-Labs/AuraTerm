<script setup lang="ts">
import { connectionTarget, type BookmarkSortKey, type SortDirection } from "./bookmarks";
import { currentLocale } from "./i18n";
import { normalizeReconnectType, type SavedConnection } from "./types";

const props = defineProps<{
  rows: SavedConnection[];
  selectedId: string | null;
  checkedIds: Set<string>;
  sortKey: BookmarkSortKey;
  sortDirection: SortDirection;
}>();

const emit = defineEmits<{
  /** Plain click: make this row the detail subject. */
  select: [id: string];
  /** Checkbox or ⌘/Ctrl-click: add or remove from the batch selection. */
  toggle: [id: string];
  /** Shift-click: extend the batch selection to this row. */
  range: [id: string];
  /** Double-click or Enter: connect. */
  open: [connection: SavedConnection];
  sort: [key: BookmarkSortKey];
  toggleAll: [];
  /** Drag a row (or the whole checked selection) onto a group in the tree. */
  rowDragStart: [event: DragEvent, connection: SavedConnection];
  rowDragEnd: [];
  rowContext: [event: MouseEvent, connection: SavedConnection];
}>();

const COLUMNS: Array<{ key: BookmarkSortKey; labelKey: string }> = [
  { key: "name", labelKey: "bookmarkManager.colName" },
  { key: "protocol", labelKey: "bookmarkManager.colProtocol" },
  { key: "target", labelKey: "bookmarkManager.colTarget" },
  { key: "auth", labelKey: "bookmarkManager.colAuth" },
  { key: "group", labelKey: "bookmarkManager.colGroup" },
  { key: "lastUsed", labelKey: "bookmarkManager.colLastUsed" },
];

const AUTH_LABEL_KEYS: Record<string, string> = {
  password: "connect.authPassword",
  key: "connect.authKey",
  agent: "connect.authAgent",
  none: "connect.authKeyboard",
};

/** Largest-unit relative time ("3 days ago"), localized like the rest of the UI. */
const RELATIVE_UNITS: Array<[Intl.RelativeTimeFormatUnit, number]> = [
  ["year", 365 * 24 * 3600_000],
  ["month", 30 * 24 * 3600_000],
  ["day", 24 * 3600_000],
  ["hour", 3600_000],
  ["minute", 60_000],
];

function formatLastUsed(timestamp?: number) {
  if (!timestamp) {
    return "—";
  }
  const delta = timestamp - Date.now();
  const formatter = new Intl.RelativeTimeFormat(currentLocale(), { numeric: "auto" });
  for (const [unit, span] of RELATIVE_UNITS) {
    if (Math.abs(delta) >= span) {
      return formatter.format(Math.round(delta / span), unit);
    }
  }
  return formatter.format(0, "minute");
}

function protocolLabel(connection: SavedConnection) {
  if (connection.protocol === "serial") return "Serial";
  if (connection.protocol === "rfc2217") return "RFC 2217";
  if (connection.protocol === "raw-tcp") return "Raw TCP";
  if (connection.protocol === "telnet") return "Telnet";
  return "SSH";
}

function authLabelKey(connection: SavedConnection) {
  if (connection.protocol !== "ssh" && connection.protocol !== undefined) {
    return null;
  }
  return AUTH_LABEL_KEYS[connection.authType] ?? null;
}

/** Password auth is the one worth flagging while scanning a long list. */
function isWeakAuth(connection: SavedConnection) {
  return (connection.protocol ?? "ssh") === "ssh" && connection.authType === "password";
}

function persistenceBadge(connection: SavedConnection): "tmux" | "screen" | null {
  if ((connection.protocol ?? "ssh") !== "ssh") {
    return null;
  }
  const type = normalizeReconnectType(connection);
  return type === "tmux" || type === "screen" ? type : null;
}

function handleRowClick(event: MouseEvent, connection: SavedConnection) {
  if (event.shiftKey) {
    emit("range", connection.id);
    return;
  }
  if (event.metaKey || event.ctrlKey) {
    emit("toggle", connection.id);
    return;
  }
  emit("select", connection.id);
}

function handleRowKeydown(event: KeyboardEvent, connection: SavedConnection, index: number) {
  if (event.key === "Enter") {
    event.preventDefault();
    emit("open", connection);
    return;
  }
  if (event.key === " ") {
    event.preventDefault();
    emit("toggle", connection.id);
    return;
  }
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
    return;
  }
  event.preventDefault();
  const next = props.rows[index + (event.key === "ArrowDown" ? 1 : -1)];
  if (!next) {
    return;
  }
  emit("select", next.id);
  const row = (event.currentTarget as HTMLElement).parentElement?.children[index + (event.key === "ArrowDown" ? 1 : -1)];
  (row as HTMLElement | undefined)?.focus();
}

const allChecked = () => props.rows.length > 0 && props.rows.every((row) => props.checkedIds.has(row.id));
</script>

<template>
  <table class="bm-table">
    <thead>
      <tr>
        <th class="bm-col-check">
          <span
            class="bm-check"
            :class="{ checked: allChecked() }"
            role="checkbox"
            :aria-checked="allChecked()"
            tabindex="0"
            :title="$t('bookmarkManager.selectAll')"
            @click="emit('toggleAll')"
            @keydown.enter.prevent="emit('toggleAll')"
            @keydown.space.prevent="emit('toggleAll')"
          >✓</span>
        </th>
        <th
          v-for="column in COLUMNS"
          :key="column.key"
          :class="{ sorted: props.sortKey === column.key }"
          @click="emit('sort', column.key)"
        >
          {{ $t(column.labelKey) }}
          <span v-if="props.sortKey === column.key" class="bm-sort-arrow">{{ props.sortDirection === 'asc' ? '↑' : '↓' }}</span>
        </th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="(connection, index) in props.rows"
        :key="connection.id"
        class="bm-row"
        tabindex="0"
        draggable="true"
        :aria-selected="connection.id === props.selectedId"
        :data-checked="props.checkedIds.has(connection.id)"
        @click="handleRowClick($event, connection)"
        @dblclick="emit('open', connection)"
        @keydown="handleRowKeydown($event, connection, index)"
        @contextmenu="emit('rowContext', $event, connection)"
        @dragstart="emit('rowDragStart', $event, connection)"
        @dragend="emit('rowDragEnd')"
      >
        <td class="bm-col-check">
          <span
            class="bm-check"
            :class="{ checked: props.checkedIds.has(connection.id) }"
            @click.stop="emit('toggle', connection.id)"
          >✓</span>
        </td>
        <td>
          <span class="bm-name">
            {{ connection.name }}
            <span
              v-if="persistenceBadge(connection)"
              class="bm-chip bm-chip--persist"
              :title="$t(persistenceBadge(connection) === 'tmux' ? 'bookmarks.reconnectTmux' : 'bookmarks.reconnectScreen')"
            >{{ persistenceBadge(connection) === 'tmux' ? 'T' : 'S' }}</span>
          </span>
        </td>
        <td>
          <span class="bm-chip" :class="`bm-chip--${connection.protocol ?? 'ssh'}`">{{ protocolLabel(connection) }}</span>
        </td>
        <td class="bm-mono">{{ connectionTarget(connection) }}</td>
        <td>
          <span v-if="authLabelKey(connection)" class="bm-auth" :class="{ weak: isWeakAuth(connection) }">
            {{ $t(authLabelKey(connection)!) }}
          </span>
          <span v-else class="bm-dim">—</span>
        </td>
        <td class="bm-dim">{{ connection.group || $t('bookmarkEditor.ungrouped') }}</td>
        <td class="bm-when">{{ formatLastUsed(connection.lastUsed) }}</td>
      </tr>
    </tbody>
  </table>
</template>
