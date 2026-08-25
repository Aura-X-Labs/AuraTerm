<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  countActions,
  commandRisks,
  credentialRisks,
  hasRisks,
  importReviewQueue,
  type ImportAction,
  type ImportPlan,
} from "./importPreview";
import { useBookmarkStore } from "./composables/useBookmarkStore";
import { t } from "./i18n";

const store = useBookmarkStore();
const { groupPaths } = store;

const current = computed(() => importReviewQueue[0] ?? null);
const plan = ref<ImportPlan | null>(null);
const group = ref("");
const actions = ref<Map<number, ImportAction>>(new Map());
/** Rows the user changed by hand — kept across a retarget, which otherwise
 *  recomputes every disposition from scratch. */
const overridden = ref<Set<number>>(new Set());
const allowCommands = ref(false);
const allowCredentials = ref(false);
const risksOpen = ref(false);
const busy = ref(false);
const error = ref("");

/** Seed the form from a plan. `keepOverrides` survives a landing-group change. */
function seed(next: ImportPlan, keepOverrides = false) {
  const previous = actions.value;
  const kept = keepOverrides ? overridden.value : new Set<number>();
  plan.value = next;
  group.value = next.group;
  actions.value = new Map(next.entries.map((entry) => [
    entry.index,
    (kept.has(entry.index) ? previous.get(entry.index) : undefined) ?? entry.disposition,
  ]));
  overridden.value = new Set(kept);
  error.value = "";
}

watch(current, (request) => {
  if (!request) {
    return;
  }
  allowCommands.value = false;
  allowCredentials.value = false;
  risksOpen.value = false;
  seed(request.plan);
});

const entries = computed(() => plan.value?.entries ?? []);
const risks = computed(() => plan.value?.risks
  ?? { postConnectCommands: 0, autoLoginResponses: 0, jumpHostCredentials: 0, passwords: 0, privateKeys: 0 });
const showRisks = computed(() => hasRisks(risks.value));
const totals = computed(() => countActions(entries.value, actions.value));
const willWrite = computed(() => totals.value.add + totals.value.update);

/** What the share calls itself, falling back to the format for plain files. */
const sourceLabel = computed(() => {
  const share = plan.value?.share;
  if (share) {
    return share.label?.trim() || share.rootName;
  }
  return plan.value?.format ?? "";
});

function setAction(index: number, action: ImportAction) {
  actions.value = new Map(actions.value).set(index, action);
  overridden.value = new Set(overridden.value).add(index);
}

/** Recompute against a different landing group; conflicts depend on where the
 *  entries would land, so this cannot be done in the frontend alone. */
async function retarget() {
  const target = plan.value;
  if (!target || group.value.trim() === target.group) {
    return;
  }
  busy.value = true;
  try {
    seed(await store.retargetImport(target.planId, group.value.trim() || undefined), true);
  } catch (failure) {
    error.value = String(failure);
  } finally {
    busy.value = false;
  }
}

function finish(accepted: boolean) {
  const request = importReviewQueue.shift();
  if (!request) {
    return;
  }
  request.resolve(accepted
    ? {
      group: group.value.trim(),
      decisions: [...actions.value].map(([index, action]) => ({ index, action })),
      trust: { allowCommands: allowCommands.value, allowCredentials: allowCredentials.value },
    }
    : null);
  plan.value = null;
}

/** Keeping the risky fields is a deliberate act: the list has to be open first. */
function toggleRisks() {
  risksOpen.value = !risksOpen.value;
  if (!risksOpen.value) {
    allowCommands.value = false;
    allowCredentials.value = false;
  }
}
</script>

<template>
  <div
    v-if="current && plan"
    class="ip-overlay"
    @click.self="finish(false)"
    @keydown.esc.stop.prevent="finish(false)"
  >
    <div class="ip-dialog" role="dialog" :aria-label="t('bookmarkImport.title')">
      <header class="ip-header">
        <h2>{{ $t('bookmarkImport.title') }}</h2>
        <div class="ip-source">{{ sourceLabel }}</div>
      </header>

      <p v-if="plan.share?.note" class="ip-note">{{ plan.share.note }}</p>

      <label class="ip-field">
        <span>{{ $t('bookmarkImport.targetGroup') }}</span>
        <input
          v-model="group"
          class="ip-input"
          type="text"
          list="ip-groups"
          spellcheck="false"
          :placeholder="$t('bookmarkImport.targetGroupPlaceholder')"
          :disabled="busy"
          @change="retarget"
          @blur="retarget"
        >
        <datalist id="ip-groups">
          <option v-for="path in groupPaths" :key="path" :value="path" />
        </datalist>
      </label>

      <section v-if="showRisks" class="ip-risks">
        <div class="ip-risks-head">
          <span class="ip-risks-icon">⚠</span>
          <div class="ip-risks-text">
            <strong>{{ $t('bookmarkImport.riskTitle') }}</strong>
            <div class="ip-risks-detail">
              <span v-if="risks.postConnectCommands">{{ $t('bookmarkImport.riskCommands', { count: risks.postConnectCommands }) }}</span>
              <span v-if="risks.autoLoginResponses">{{ $t('bookmarkImport.riskResponses', { count: risks.autoLoginResponses }) }}</span>
              <span v-if="risks.jumpHostCredentials">{{ $t('bookmarkImport.riskJumpHosts', { count: risks.jumpHostCredentials }) }}</span>
              <span v-if="risks.passwords">{{ $t('bookmarkImport.riskPasswords', { count: risks.passwords }) }}</span>
              <span v-if="risks.privateKeys">{{ $t('bookmarkImport.riskKeys', { count: risks.privateKeys }) }}</span>
            </div>
            <div class="ip-risks-why">{{ $t('bookmarkImport.riskStripped') }}</div>
          </div>
          <button class="ip-btn ip-btn--ghost" type="button" @click="toggleRisks">
            {{ risksOpen ? $t('bookmarkImport.riskHide') : $t('bookmarkImport.riskReview') }}
          </button>
        </div>
        <div v-if="risksOpen" class="ip-risks-gate">
          <label v-if="commandRisks(risks)">
            <input v-model="allowCommands" type="checkbox">
            {{ $t('bookmarkImport.keepCommands') }}
          </label>
          <label v-if="credentialRisks(risks)">
            <input v-model="allowCredentials" type="checkbox">
            {{ $t('bookmarkImport.keepCredentials') }}
          </label>
        </div>
      </section>

      <div class="ip-table-wrap">
        <table class="ip-table">
          <thead>
            <tr>
              <th>{{ $t('bookmarkManager.colName') }}</th>
              <th>{{ $t('bookmarkManager.colTarget') }}</th>
              <th>{{ $t('bookmarkManager.colGroup') }}</th>
              <th>{{ $t('bookmarkImport.colAction') }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="entry in entries" :key="entry.index" :class="{ 'ip-row--skip': actions.get(entry.index) === 'skip' }">
              <td>
                <div class="ip-name">{{ entry.name }}</div>
                <div v-if="entry.matchedName" class="ip-match">
                  {{ entry.matchedBy === 'origin'
                    ? $t('bookmarkImport.matchOrigin', { name: entry.matchedName })
                    : $t('bookmarkImport.matchEndpoint', { name: entry.matchedName }) }}
                </div>
              </td>
              <td class="ip-target">{{ entry.target }}</td>
              <td class="ip-group">{{ entry.group || $t('bookmarkEditor.ungrouped') }}</td>
              <td>
                <select
                  class="ip-select"
                  :value="actions.get(entry.index)"
                  @change="setAction(entry.index, ($event.target as HTMLSelectElement).value as ImportAction)"
                >
                  <option value="add">{{ $t('bookmarkImport.actionAdd') }}</option>
                  <option v-if="entry.matchedName" value="update">{{ $t('bookmarkImport.actionUpdate') }}</option>
                  <option value="skip">{{ $t('bookmarkImport.actionSkip') }}</option>
                </select>
              </td>
            </tr>
            <tr v-if="entries.length === 0">
              <td class="ip-empty" colspan="4">{{ $t('bookmarkImport.nothingFound') }}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <p v-for="warning in plan.warnings" :key="warning" class="ip-warning">{{ warning }}</p>
      <p v-if="error" class="ip-warning">{{ error }}</p>

      <footer class="ip-footer">
        <div class="ip-summary">
          {{ $t('bookmarkImport.summary', { add: totals.add, update: totals.update, skip: totals.skip }) }}
        </div>
        <button class="ip-btn" type="button" :disabled="busy" @click="finish(false)">{{ $t('common.cancel') }}</button>
        <button class="ip-btn ip-btn--primary" type="button" :disabled="busy || willWrite === 0" @click="finish(true)">
          {{ $t('bookmarkImport.confirm', { count: willWrite }) }}
        </button>
      </footer>
    </div>
  </div>
</template>

<style>
.ip-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
}
.ip-dialog {
  width: min(760px, 94vw);
  max-height: min(720px, 88vh);
  display: flex;
  flex-direction: column;
  gap: 12px;
  background: var(--ui-panel-bg, #1e1e1e);
  color: var(--ui-fg, #dcdcdc);
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 10px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  padding: 16px 18px;
}
.ip-header {
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.ip-header h2 {
  font-size: 14px;
  font-weight: 600;
  margin: 0;
}
.ip-source {
  font-size: 12px;
  opacity: 0.7;
  overflow-wrap: anywhere;
}
.ip-note {
  margin: 0;
  font-size: 12px;
  opacity: 0.85;
  border-left: 2px solid var(--ui-border, #3a3a3a);
  padding-left: 8px;
  overflow-wrap: anywhere;
}
.ip-field {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
}
.ip-field > span {
  flex: none;
  opacity: 0.8;
}
.ip-input {
  flex: 1;
  min-width: 0;
  background: var(--ui-input-bg, #16161a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 6px 9px;
  font-size: 12px;
}
.ip-input:focus {
  outline: none;
  border-color: var(--ui-accent, #4d9fff);
}
.ip-risks {
  border: 1px solid var(--ui-warning, #d0a13a);
  border-radius: 8px;
  padding: 10px 12px;
  background: rgba(208, 161, 58, 0.08);
}
.ip-risks-head {
  display: flex;
  align-items: flex-start;
  gap: 10px;
}
.ip-risks-icon {
  color: var(--ui-warning, #d0a13a);
}
.ip-risks-text {
  flex: 1;
  min-width: 0;
  font-size: 12px;
}
.ip-risks-detail {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 12px;
  margin-top: 3px;
  opacity: 0.9;
}
.ip-risks-why {
  margin-top: 4px;
  opacity: 0.7;
}
.ip-risks-gate {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 10px;
  padding-top: 8px;
  border-top: 1px solid rgba(208, 161, 58, 0.35);
  font-size: 12px;
}
.ip-risks-gate label {
  display: flex;
  align-items: center;
  gap: 7px;
  cursor: pointer;
}
.ip-table-wrap {
  flex: 1;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 8px;
}
.ip-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}
.ip-table th,
.ip-table td {
  text-align: left;
  padding: 6px 10px;
  border-bottom: 1px solid var(--ui-border, #2e2e2e);
  vertical-align: top;
}
.ip-table th {
  position: sticky;
  top: 0;
  background: var(--ui-panel-bg, #1e1e1e);
  font-weight: 600;
  opacity: 0.75;
}
.ip-table tbody tr:last-child td {
  border-bottom: none;
}
.ip-row--skip {
  opacity: 0.45;
}
.ip-name {
  font-weight: 500;
}
.ip-match,
.ip-group,
.ip-target {
  opacity: 0.75;
  overflow-wrap: anywhere;
}
.ip-match {
  margin-top: 2px;
  font-size: 11px;
}
.ip-empty {
  text-align: center;
  padding: 20px;
  opacity: 0.6;
}
.ip-select {
  background: var(--ui-input-bg, #16161a);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 3px 6px;
  font-size: 12px;
}
.ip-warning {
  margin: 0;
  font-size: 12px;
  color: var(--ui-warning, #d0a13a);
  overflow-wrap: anywhere;
}
.ip-footer {
  display: flex;
  align-items: center;
  gap: 8px;
}
.ip-summary {
  flex: 1;
  font-size: 12px;
  opacity: 0.75;
}
.ip-btn {
  background: var(--ui-input-bg, #26262b);
  color: inherit;
  border: 1px solid var(--ui-border, #3a3a3a);
  border-radius: 6px;
  padding: 6px 12px;
  font-size: 12px;
  cursor: pointer;
}
.ip-btn:hover:not(:disabled) {
  border-color: var(--ui-accent, #4d9fff);
}
.ip-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.ip-btn--primary {
  background: var(--ui-accent, #2f6fd0);
  border-color: transparent;
  color: #fff;
}
.ip-btn--ghost {
  flex: none;
  align-self: flex-start;
}
</style>
