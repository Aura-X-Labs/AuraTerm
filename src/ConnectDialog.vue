<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SETTINGS, type AppSettings, type SerialHistoryItem } from "./settings";
import { buildDefaultLogPath } from "./logging";
import { collectGroupPaths } from "./bookmarks";
import { isReconnectEnabled, type AutoLoginRule, type ConnectResult, type ConnectionProtocol, type JumpHostConfig, type ReconnectType, type SavedConnection, type SerialConfig, type SshAuthType } from "./types";
import "./ConnectDialog.css";

interface SerialPortInfo {
  portName: string;
  portType: string;
  manufacturer?: string | null;
  serialNumber?: string | null;
  vid?: number | null;
  pid?: number | null;
}

interface GeneratedSshKeyPair {
  privateKey: string;
  publicKey: string;
  fingerprint: string;
}

interface SerialPresetOption {
  id: string;
  name: string;
  portName?: string;
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  stopBits: 1 | 2;
  parity: "none" | "odd" | "even";
  flowControl: "none" | "hardware" | "software";
}

const props = withDefaults(defineProps<{
  initialProtocol?: ConnectionProtocol;
  lastSerialConfig?: SerialHistoryItem | null;
  recentSerialConfigs?: SerialHistoryItem[];
  settings?: AppSettings;
}>(), {
  initialProtocol: "ssh",
  lastSerialConfig: null,
  recentSerialConfigs: () => [],
  settings: () => DEFAULT_SETTINGS,
});

const emit = defineEmits<{
  connect: [result: ConnectResult];
  cancel: [];
}>();

const BUILTIN_SERIAL_PRESETS: SerialPresetOption[] = [
  { id: "builtin-115200-8n1", name: "115200 · 8N1", baudRate: 115200, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-9600-8n1", name: "9600 · 8N1", baudRate: 9600, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-57600-8n1", name: "57600 · 8N1", baudRate: 57600, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-38400-8n1", name: "38400 · 8N1", baudRate: 38400, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-9600-7e1", name: "9600 · 7E1", baudRate: 9600, dataBits: 7, stopBits: 1, parity: "even", flowControl: "none" },
];

function applySerialOption(option: Pick<SerialPresetOption, "portName" | "baudRate" | "dataBits" | "stopBits" | "parity" | "flowControl">) {
  if (option.portName) {
    serialPortName.value = option.portName;
  }
  serialBaudRate.value = String(option.baudRate);
  serialDataBits.value = String(option.dataBits) as "5" | "6" | "7" | "8";
  serialStopBits.value = String(option.stopBits) as "1" | "2";
  serialParity.value = option.parity;
  serialFlowControl.value = option.flowControl;
}

const protocol = ref<ConnectionProtocol>(props.initialProtocol);
const host = ref("");
const port = ref("22");
const user = ref("");
const password = ref("");
const passphrase = ref("");
const privateKey = ref("");
const privateKeyFileName = ref("");
const privateKeyError = ref("");
const generatedPublicKey = ref("");
const privateKeyFileInput = ref<HTMLInputElement | null>(null);
const authType = ref<SshAuthType>("password");
const agentForwarding = ref(false);
const jumpHosts = ref<JumpHostConfig[]>([]);
const autoLoginRules = ref<AutoLoginRule[]>([]);
const postConnectCommands = ref("");
const saveConnection = ref(true);
const connectionName = ref("");
const groupSelection = ref("");
const customGroup = ref("");
const existingGroups = ref<string[]>([]);
const customGroupInput = ref<HTMLInputElement | null>(null);
const telnetPort = ref("23");
const serialPortName = ref(props.lastSerialConfig?.portName ?? "");
const serialBaudRate = ref(String(props.lastSerialConfig?.baudRate ?? 9600));
const serialDataBits = ref<"5" | "6" | "7" | "8">(String(props.lastSerialConfig?.dataBits ?? 8) as "5" | "6" | "7" | "8");
const serialStopBits = ref<"1" | "2">(String(props.lastSerialConfig?.stopBits ?? 1) as "1" | "2");
const serialParity = ref<"none" | "odd" | "even">(props.lastSerialConfig?.parity ?? "none");
const serialFlowControl = ref<"none" | "hardware" | "software">(props.lastSerialConfig?.flowControl ?? "none");
const selectedSerialPresetId = ref("custom");
const serialPorts = ref<SerialPortInfo[]>([]);
const loadingSerialPorts = ref(false);
const serialError = ref("");
const enableLog = ref(true);
const logFilePath = ref("");
const reconnectType = ref<ReconnectType>("manual");

const isSsh = computed(() => protocol.value === "ssh");
const isTelnet = computed(() => protocol.value === "telnet");
const isSerial = computed(() => protocol.value === "serial");

const recentPresetOptions = computed<SerialPresetOption[]>(() => {
  const seen = new Set<string>();
  return props.recentSerialConfigs
    .filter((item) => {
      const key = `${item.portName}|${item.baudRate}|${item.dataBits}|${item.stopBits}|${item.parity}|${item.flowControl}`;
      if (seen.has(key)) {
        return false;
      }
      seen.add(key);
      return true;
    })
    .map((item) => ({
      id: item.id,
      name: item.name,
      portName: item.portName,
      baudRate: item.baudRate,
      dataBits: item.dataBits,
      stopBits: item.stopBits,
      parity: item.parity,
      flowControl: item.flowControl,
    }));
});

const serialPresetOptions = computed(() => [...recentPresetOptions.value, ...BUILTIN_SERIAL_PRESETS]);

watch(() => props.initialProtocol, (initialProtocol) => {
  protocol.value = initialProtocol;
}, { immediate: true });

watch(() => props.lastSerialConfig, (lastSerialConfig) => {
  if (lastSerialConfig) {
    applySerialOption(lastSerialConfig);
  }
}, { immediate: true });

watch(
  [serialBaudRate, serialDataBits, serialStopBits, serialParity, serialFlowControl, serialPortName, serialPresetOptions],
  () => {
    const matched = serialPresetOptions.value.find((option) => (
      (option.portName === undefined || option.portName === serialPortName.value)
      && option.baudRate === (parseInt(serialBaudRate.value, 10) || 9600)
      && option.dataBits === (parseInt(serialDataBits.value, 10) as 5 | 6 | 7 | 8)
      && option.stopBits === (parseInt(serialStopBits.value, 10) as 1 | 2)
      && option.parity === serialParity.value
      && option.flowControl === serialFlowControl.value
    ));
    selectedSerialPresetId.value = matched?.id ?? "custom";
  },
  { immediate: true },
);

/** Sentinel `<select>` value that reveals the free-text field for a brand-new group. */
const NEW_GROUP_OPTION = "__auraterm_new_group__";

/** True when the group name comes from the text field instead of the dropdown. */
const useCustomGroup = computed(() => existingGroups.value.length === 0 || groupSelection.value === NEW_GROUP_OPTION);
const connectionGroup = computed(() => (useCustomGroup.value ? customGroup.value : groupSelection.value).trim());

/** Offer the groups already in use by saved connections in the group dropdown. */
async function loadConnectionGroups() {
  try {
    existingGroups.value = collectGroupPaths(await invoke<SavedConnection[]>("get_connections"));
  } catch (error) {
    console.error("Failed to load connection groups", error);
    existingGroups.value = [];
  }
}

onMounted(() => {
  void loadConnectionGroups();
});

function handleGroupSelectionChange() {
  if (groupSelection.value === NEW_GROUP_OPTION) {
    void nextTick(() => customGroupInput.value?.focus());
  }
}

async function loadSerialPorts() {
  loadingSerialPorts.value = true;
  serialError.value = "";
  try {
    const ports = await invoke<SerialPortInfo[]>("list_serial_ports");
    serialPorts.value = ports;
    if (!serialPortName.value && ports.length > 0) {
      serialPortName.value = ports[0].portName;
    }
  } catch (error) {
    console.error("Failed to enumerate serial ports", error);
    serialError.value = String(error);
    serialPorts.value = [];
  } finally {
    loadingSerialPorts.value = false;
  }
}

watch(isSerial, (value) => {
  if (value) {
    void loadSerialPorts();
  }
}, { immediate: true });

const defaultName = computed(() => {
  if (isSsh.value) {
    return user.value && host.value ? `${user.value}@${host.value}` : host.value;
  }
  if (isTelnet.value) {
    return host.value ? `telnet://${host.value}:${telnetPort.value}` : "";
  }
  return serialPortName.value ? `serial://${serialPortName.value}@${serialBaudRate.value}` : "";
});

const defaultLogPath = computed(() => {
  return buildDefaultLogPath(props.settings, {
    protocol: protocol.value,
    host: host.value,
    user: user.value,
    port: isSsh.value ? (parseInt(port.value, 10) || 22) : isTelnet.value ? (parseInt(telnetPort.value, 10) || 23) : undefined,
    serialPort: serialPortName.value,
    baudRate: parseInt(serialBaudRate.value, 10) || 9600,
    session: defaultName.value,
  });
});

const canConnect = computed(() => {
  if (isSsh.value) {
    return Boolean(
      host.value.trim()
      && user.value.trim()
      && (authType.value !== "key" || privateKey.value.trim())
      && jumpHosts.value.every((jump) => jump.host.trim() && jump.user.trim() && (jump.authType !== "key" || jump.privateKey?.trim()))
    );
  }
  if (isTelnet.value) {
    return Boolean(host.value.trim());
  }
  return Boolean(serialPortName.value.trim());
});

function handleSerialPresetChange(event: Event) {
  const presetId = (event.target as HTMLSelectElement).value;
  selectedSerialPresetId.value = presetId;
  if (presetId === "custom") {
    return;
  }
  const preset = serialPresetOptions.value.find((item) => item.id === presetId);
  if (preset) {
    applySerialOption(preset);
  }
}

function addJumpHost() {
  jumpHosts.value.push({
    id: crypto.randomUUID(), host: "", port: 22, user: "", authType: "agent",
  });
}

function removeJumpHost(index: number) {
  jumpHosts.value.splice(index, 1);
}

function addAutoLoginRule() {
  autoLoginRules.value.push({ expect: "", response: "", timeoutSecs: 30 });
}

function removeAutoLoginRule(index: number) {
  autoLoginRules.value.splice(index, 1);
}

function triggerPrivateKeyPicker() {
  privateKeyError.value = "";
  privateKeyFileInput.value?.click();
}

async function handlePrivateKeyFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const [file] = input.files ?? [];

  if (!file) {
    return;
  }

  try {
    privateKey.value = await file.text();
    privateKeyFileName.value = file.name;
    privateKeyError.value = "";
  } catch (error) {
    console.error("Failed to read private key file", error);
    privateKey.value = "";
    privateKeyFileName.value = "";
    privateKeyError.value = "Unable to read the selected private key file.";
  } finally {
    input.value = "";
  }
}

function clearPrivateKeySelection() {
  privateKey.value = "";
  privateKeyFileName.value = "";
  privateKeyError.value = "";
  generatedPublicKey.value = "";
  if (privateKeyFileInput.value) {
    privateKeyFileInput.value.value = "";
  }
}

async function generatePrivateKey() {
  privateKeyError.value = "";
  try {
    const generated = await invoke<GeneratedSshKeyPair>("ssh_generate_key_pair", {
      passphrase: passphrase.value || null,
      comment: user.value && host.value ? `${user.value}@${host.value}` : "AuraTerm",
    });
    privateKey.value = generated.privateKey;
    privateKeyFileName.value = `Generated Ed25519 (${generated.fingerprint})`;
    generatedPublicKey.value = generated.publicKey;
  } catch (error) {
    privateKeyError.value = String(error);
  }
}

async function copyGeneratedPublicKey() {
  await navigator.clipboard.writeText(generatedPublicKey.value);
}

function handleSubmit(event: Event) {
  event.preventDefault();
  if ((isSsh.value || isTelnet.value) && !host.value.trim()) {
    return;
  }
  if (isSsh.value && !user.value.trim()) {
    return;
  }
  if (isSsh.value && authType.value === "key" && !privateKey.value.trim()) {
    return;
  }
  if (isSerial.value && !serialPortName.value.trim()) {
    return;
  }

  const sshSecret = password.value !== "" ? password.value : undefined;
  const customConnectionName = connectionName.value.trim();

  const serialConfig: SerialConfig | undefined = isSerial.value ? {
    portName: serialPortName.value.trim(),
    baudRate: parseInt(serialBaudRate.value, 10) || 9600,
    dataBits: parseInt(serialDataBits.value, 10) as 5 | 6 | 7 | 8,
    stopBits: parseInt(serialStopBits.value, 10) as 1 | 2,
    parity: serialParity.value,
    flowControl: serialFlowControl.value,
  } : undefined;

  emit("connect", {
    protocol: protocol.value,
    sshConfig: isSsh.value ? {
      host: host.value,
      port: parseInt(port.value, 10) || 22,
      user: user.value,
      password: sshSecret,
      privateKey: authType.value === "key" ? privateKey.value : undefined,
      passphrase: authType.value === "key" && passphrase.value ? passphrase.value : undefined,
      authType: authType.value,
      agentForwarding: agentForwarding.value,
      jumpHosts: jumpHosts.value.map((jump) => ({ ...jump, host: jump.host.trim(), user: jump.user.trim() })),
      autoLoginRules: autoLoginRules.value.filter((rule) => rule.expect.trim()).map((rule) => ({ ...rule, expect: rule.expect.trim() })),
      postConnectCommands: postConnectCommands.value.split(/\r?\n/).map((command) => command.trim()).filter(Boolean),
      autoReconnect: isReconnectEnabled(reconnectType.value),
      reconnectType: reconnectType.value,
    } : undefined,
    telnetConfig: isTelnet.value ? { host: host.value, port: parseInt(telnetPort.value, 10) || 23 } : undefined,
    serialConfig,
    saveAs: saveConnection.value ? (customConnectionName || defaultName.value) : undefined,
    saveGroup: saveConnection.value && connectionGroup.value.trim() ? connectionGroup.value.trim() : undefined,
    logPath: enableLog.value ? (logFilePath.value.trim() || defaultLogPath.value) : undefined,
  });
}

</script>

<template>
  <div class="dialog-overlay">
    <div class="dialog-content dialog-content--wide">
      <h2 class="dialog-title">{{ $t('connect.title') }}</h2>

      <div class="protocol-selector">
        <label :class="{ active: isSsh }">
          <input v-model="protocol" type="radio" name="protocol" value="ssh">
          SSH
        </label>
        <label :class="{ active: isTelnet }">
          <input v-model="protocol" type="radio" name="protocol" value="telnet">
          Telnet
        </label>
        <label :class="{ active: isSerial }">
          <input v-model="protocol" type="radio" name="protocol" value="serial">
          {{ $t('menu.serial') }}
        </label>
      </div>

      <form @submit="handleSubmit">
        <div v-if="isSsh || isTelnet" class="form-group">
          <label>{{ $t('connect.host') }}</label>
          <input
            v-model="host"
            type="text"
            :placeholder="$t('connect.hostPlaceholder')"
            autofocus
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            required
          >
        </div>

        <template v-if="isSsh">
          <div class="two-column-grid">
            <div class="form-group">
              <label>{{ $t('connect.port') }}</label>
              <input v-model="port" type="number" required>
            </div>
            <div class="form-group">
              <label>{{ $t('connect.user') }}</label>
              <input
                v-model="user"
                type="text"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
                required
              >
            </div>
          </div>
        </template>

        <div v-else-if="isTelnet" class="form-group">
          <label>{{ $t('connect.port') }}</label>
          <input v-model="telnetPort" type="number" required>
        </div>

        <template v-else>
          <div class="serial-settings-grid serial-settings-grid--compact">
            <div class="form-group serial-settings-grid-span-2">
              <label>{{ $t('connect.preset') }}</label>
              <select :value="selectedSerialPresetId" @change="handleSerialPresetChange">
                <option value="custom">{{ $t('connect.custom') }}</option>
                <optgroup v-if="recentPresetOptions.length > 0" :label="$t('connect.recent')">
                  <option v-for="preset in recentPresetOptions" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
                </optgroup>
                <optgroup label="Common">
                  <option v-for="preset in BUILTIN_SERIAL_PRESETS" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
                </optgroup>
              </select>
            </div>

            <div class="form-group serial-settings-grid-span-2">
              <label>{{ $t('connect.serialPort') }}</label>
              <div class="serial-port-row">
                <select v-model="serialPortName" class="serial-port-select" required>
                  <option value="" disabled>{{ $t('connect.selectSerialPort') }}</option>
                  <option v-for="portInfo in serialPorts" :key="portInfo.portName" :value="portInfo.portName">
                    {{ portInfo.portName }}{{ portInfo.manufacturer ? ` - ${portInfo.manufacturer}` : '' }} ({{ portInfo.portType }})
                  </option>
                </select>
                <button type="button" class="serial-refresh-btn" :disabled="loadingSerialPorts" @click="loadSerialPorts">
                  {{ loadingSerialPorts ? '...' : '↻' }}
                </button>
              </div>
              <div v-if="serialError" class="form-hint error">{{ $t('connect.serialEnumFailed') }} {{ serialError }}</div>
              <div v-else-if="serialPorts.length > 0" class="form-hint">
                {{ $t('connect.foundDevices', { count: serialPorts.length }) }}
              </div>
              <div v-else class="form-hint">{{ $t('connect.noSerialPorts') }}</div>
            </div>

            <div class="form-group">
              <label>{{ $t('connect.baudRate') }}</label>
              <input v-model="serialBaudRate" type="number" min="1" required>
            </div>
            <div class="form-group">
              <label>{{ $t('connect.dataBits') }}</label>
              <select v-model="serialDataBits">
                <option value="5">5</option>
                <option value="6">6</option>
                <option value="7">7</option>
                <option value="8">8</option>
              </select>
            </div>
            <div class="form-group">
              <label>{{ $t('connect.stopBits') }}</label>
              <select v-model="serialStopBits">
                <option value="1">1</option>
                <option value="2">2</option>
              </select>
            </div>
            <div class="form-group">
              <label>{{ $t('connect.parity') }}</label>
              <select v-model="serialParity">
                <option value="none">{{ $t('connect.none') }}</option>
                <option value="odd">{{ $t('connect.odd') }}</option>
                <option value="even">{{ $t('connect.even') }}</option>
              </select>
            </div>
            <div class="form-group serial-settings-grid-span-2">
              <label>{{ $t('connect.flowControl') }}</label>
              <select v-model="serialFlowControl">
                <option value="none">{{ $t('connect.none') }}</option>
                <option value="hardware">{{ $t('connect.hardware') }}</option>
                <option value="software">{{ $t('connect.software') }}</option>
              </select>
            </div>
          </div>
        </template>

        <template v-if="isSsh">
          <div class="form-group auth-type-group">
            <label>{{ $t('connect.authType') }}</label>
            <select v-model="authType">
              <option value="password">{{ $t('connect.authPassword') }}</option>
              <option value="key">{{ $t('connect.authKey') }}</option>
              <option value="agent">{{ $t('connect.authAgent') }}</option>
              <option value="none">{{ $t('connect.authKeyboard') }}</option>
            </select>
          </div>

          <div v-if="authType === 'password'" class="form-group">
            <label>{{ $t('connect.password') }}</label>
            <input v-model="password" type="password">
          </div>

          <div v-else-if="authType === 'key'" class="form-group">
            <label>{{ $t('connect.privateKey') }}</label>
            <input
              ref="privateKeyFileInput"
              type="file"
              class="private-key-file-input"
              @change="handlePrivateKeyFileChange"
            >
            <div class="private-key-picker-row">
              <input
                :value="privateKeyFileName || $t('connect.noKeySelected')"
                type="text"
                class="private-key-display"
                readonly
              >
              <button type="button" class="private-key-picker-btn" @click="triggerPrivateKeyPicker">{{ $t('connect.browse') }}</button>
              <button type="button" class="private-key-picker-btn" @click="generatePrivateKey">{{ $t('connect.generate') }}</button>
              <button
                v-if="privateKeyFileName"
                type="button"
                class="private-key-clear-btn"
                @click="clearPrivateKeySelection"
              >
                {{ $t('connect.clear') }}
              </button>
            </div>
            <div v-if="privateKeyError" class="form-hint error">{{ privateKeyError }}</div>
            <input v-model="passphrase" type="password" :placeholder="$t('connect.keyPassphrase')" style="margin-top: 8px">
            <div v-if="generatedPublicKey" class="generated-public-key">
              <textarea :value="generatedPublicKey" rows="2" readonly />
              <button type="button" class="private-key-picker-btn" @click="copyGeneratedPublicKey">{{ $t('connect.copyPublicKey') }}</button>
            </div>
          </div>

          <details class="ssh-advanced">
            <summary>{{ $t('connect.advancedSummary') }}</summary>

            <label class="ssh-checkbox">
              <input v-model="agentForwarding" type="checkbox">
              {{ $t('connect.forwardAgent') }}
            </label>

            <div class="ssh-advanced-heading">
              <strong>{{ $t('connect.jumpHosts') }}</strong>
              <button type="button" @click="addJumpHost">{{ $t('connect.addJumpHost') }}</button>
            </div>
            <div v-for="(jump, index) in jumpHosts" :key="jump.id" class="ssh-advanced-card">
              <div class="ssh-card-grid">
                <input v-model="jump.host" type="text" :placeholder="$t('connect.jumpHostPlaceholder')">
                <input v-model.number="jump.port" type="number" min="1" max="65535" :placeholder="$t('connect.portPlaceholder')">
                <input v-model="jump.user" type="text" :placeholder="$t('connect.userPlaceholder')">
                <select v-model="jump.authType">
                  <option value="password">{{ $t('connect.authPassword') }}</option>
                  <option value="key">{{ $t('connect.authKey') }}</option>
                  <option value="agent">{{ $t('connect.authAgent') }}</option>
                  <option value="none">{{ $t('connect.authKeyboard') }}</option>
                </select>
              </div>
              <input v-if="jump.authType === 'password'" v-model="jump.password" type="password" :placeholder="$t('connect.jumpPassword')">
              <template v-else-if="jump.authType === 'key'">
                <textarea v-model="jump.privateKey" rows="3" :placeholder="$t('connect.privateKeyPlaceholder')" />
                <input v-model="jump.passphrase" type="password" :placeholder="$t('connect.keyPassphrase')">
              </template>
              <button type="button" class="ssh-remove-btn" @click="removeJumpHost(index)">{{ $t('connect.remove') }}</button>
            </div>

            <div class="ssh-advanced-heading">
              <strong>{{ $t('connect.expectRules') }}</strong>
              <button type="button" @click="addAutoLoginRule">{{ $t('connect.addRule') }}</button>
            </div>
            <div v-for="(rule, index) in autoLoginRules" :key="index" class="ssh-advanced-card">
              <div class="ssh-card-grid ssh-card-grid--automation">
                <input v-model="rule.expect" type="text" :placeholder="$t('connect.waitForText')">
                <input v-model="rule.response" type="password" :placeholder="$t('connect.sendResponse')">
                <input v-model.number="rule.timeoutSecs" type="number" min="1" max="300" :title="$t('connect.timeoutSeconds')">
              </div>
              <label class="ssh-checkbox"><input v-model="rule.caseSensitive" type="checkbox"> {{ $t('connect.caseSensitive') }}</label>
              <button type="button" class="ssh-remove-btn" @click="removeAutoLoginRule(index)">{{ $t('connect.remove') }}</button>
            </div>

            <div class="form-group">
              <label>{{ $t('connect.commandsAfterLogin') }}</label>
              <textarea v-model="postConnectCommands" rows="3" :placeholder="$t('connect.postConnectPlaceholder')" />
            </div>
          </details>
        </template>

        <div class="form-group save-connection-group">
          <label class="save-connection-label">
            <input v-model="saveConnection" type="checkbox">
            <span>{{ $t('connect.saveConnection') }}</span>
          </label>
          <div v-if="saveConnection" class="two-column-grid">
            <input
              v-model="connectionName"
              type="text"
              class="save-connection-name"
              :placeholder="defaultName || $t('connect.connectionNamePlaceholder')"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
            >
            <div class="save-connection-group-field">
              <select
                v-if="existingGroups.length > 0"
                v-model="groupSelection"
                class="save-connection-name"
                @change="handleGroupSelectionChange"
              >
                <option value="">{{ $t('connect.groupUngrouped') }}</option>
                <optgroup :label="$t('connect.groupExisting')">
                  <option v-for="group in existingGroups" :key="group" :value="group">{{ group }}</option>
                </optgroup>
                <option :value="NEW_GROUP_OPTION">{{ $t('connect.groupNew') }}</option>
              </select>
              <input
                v-if="useCustomGroup"
                ref="customGroupInput"
                v-model="customGroup"
                type="text"
                class="save-connection-name"
                :placeholder="$t('connect.groupPlaceholder')"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
              >
            </div>
          </div>
        </div>

        <div class="form-group save-connection-group">
          <label class="save-connection-label">
            <input v-model="enableLog" type="checkbox">
            <span>{{ $t('connect.saveLog') }}</span>
          </label>
          <input
            v-if="enableLog"
            v-model="logFilePath"
            type="text"
            class="save-connection-name"
            :placeholder="defaultLogPath"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
          >
        </div>

        <template v-if="isSsh">
          <div class="form-group save-connection-group">
            <div class="form-group" style="margin-bottom: 4px">
              <label>{{ $t('connect.reconnectMode') }}</label>
              <select v-model="reconnectType">
                <option value="manual">{{ $t('connect.reconnectManual') }}</option>
                <option value="simple">{{ $t('connect.reconnectSimple') }}</option>
                <option value="tmux">{{ $t('connect.reconnectTmux') }}</option>
                <option value="screen">{{ $t('connect.reconnectScreen') }}</option>
              </select>
            </div>
            <div class="form-hint" style="margin-top: 2px;">
              <template v-if="reconnectType === 'manual'">
                {{ $t('connect.reconnectManualHint') }}
              </template>
              <template v-else-if="reconnectType === 'simple'">
                {{ $t('connect.reconnectSimpleHint') }}
              </template>
              <template v-else-if="reconnectType === 'tmux'">
                {{ $t('connect.reconnectTmuxHint') }}
              </template>
              <template v-else>
                {{ $t('connect.reconnectScreenHint') }}
              </template>
            </div>
          </div>
        </template>

        <div class="dialog-actions">
          <button type="button" class="btn-cancel" @click="emit('cancel')">{{ $t('common.cancel') }}</button>
          <button type="submit" class="btn-connect" :disabled="!canConnect">{{ $t('connect.connect') }}</button>
        </div>
      </form>
    </div>
  </div>
</template>
