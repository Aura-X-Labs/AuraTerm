<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SETTINGS, type AppSettings, type SerialHistoryItem } from "./settings";
import { buildDefaultLogPath } from "./logging";
import { isReconnectEnabled, type ConnectResult, type ConnectionProtocol, type ReconnectType, type SerialConfig } from "./types";
import "./ConnectDialog.css";

interface SerialPortInfo {
  portName: string;
  portType: string;
  manufacturer?: string | null;
  serialNumber?: string | null;
  vid?: number | null;
  pid?: number | null;
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
const privateKey = ref("");
const privateKeyFileName = ref("");
const privateKeyError = ref("");
const privateKeyFileInput = ref<HTMLInputElement | null>(null);
const authType = ref<"password" | "key">("password");
const saveConnection = ref(true);
const connectionName = ref("");
const connectionGroup = ref("");
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
      && (authType.value !== "key" || privateKey.value.trim()),
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
  if (privateKeyFileInput.value) {
    privateKeyFileInput.value.value = "";
  }
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
      autoReconnect: isReconnectEnabled(reconnectType.value),
      reconnectType: reconnectType.value,
    } : undefined,
    telnetConfig: isTelnet.value ? { host: host.value, port: parseInt(telnetPort.value, 10) || 23 } : undefined,
    serialConfig,
    saveAs: saveConnection.value ? (connectionName.value.trim() || defaultName.value) : undefined,
    saveGroup: saveConnection.value && connectionGroup.value.trim() ? connectionGroup.value.trim() : undefined,
    logPath: enableLog.value ? (logFilePath.value.trim() || defaultLogPath.value) : undefined,
  });
}

</script>

<template>
  <div class="dialog-overlay">
    <div class="dialog-content dialog-content--wide">
      <h2 class="dialog-title">New Session</h2>

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
          Serial
        </label>
      </div>

      <form @submit="handleSubmit">
        <div v-if="isSsh || isTelnet" class="form-group">
          <label>Host:</label>
          <input
            v-model="host"
            type="text"
            placeholder="e.g. 192.168.1.100"
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
              <label>Port:</label>
              <input v-model="port" type="number" required>
            </div>
            <div class="form-group">
              <label>User:</label>
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
          <label>Port:</label>
          <input v-model="telnetPort" type="number" required>
        </div>

        <template v-else>
          <div class="serial-settings-grid serial-settings-grid--compact">
            <div class="form-group serial-settings-grid-span-2">
              <label>Preset:</label>
              <select :value="selectedSerialPresetId" @change="handleSerialPresetChange">
                <option value="custom">Custom</option>
                <optgroup v-if="recentPresetOptions.length > 0" label="Recent">
                  <option v-for="preset in recentPresetOptions" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
                </optgroup>
                <optgroup label="Common">
                  <option v-for="preset in BUILTIN_SERIAL_PRESETS" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
                </optgroup>
              </select>
            </div>

            <div class="form-group serial-settings-grid-span-2">
              <label>Serial Port:</label>
              <div class="serial-port-row">
                <select v-model="serialPortName" class="serial-port-select" required>
                  <option value="" disabled>Select a serial port...</option>
                  <option v-for="portInfo in serialPorts" :key="portInfo.portName" :value="portInfo.portName">
                    {{ portInfo.portName }}{{ portInfo.manufacturer ? ` - ${portInfo.manufacturer}` : '' }} ({{ portInfo.portType }})
                  </option>
                </select>
                <button type="button" class="serial-refresh-btn" :disabled="loadingSerialPorts" @click="loadSerialPorts">
                  {{ loadingSerialPorts ? '...' : '↻' }}
                </button>
              </div>
              <div v-if="serialError" class="form-hint error">Serial port enumeration failed: {{ serialError }}</div>
              <div v-else-if="serialPorts.length > 0" class="form-hint">
                Found {{ serialPorts.length }} device(s)
              </div>
              <div v-else class="form-hint">No serial ports found. Click refresh to scan again.</div>
            </div>

            <div class="form-group">
              <label>Baud Rate:</label>
              <input v-model="serialBaudRate" type="number" min="1" required>
            </div>
            <div class="form-group">
              <label>Data Bits:</label>
              <select v-model="serialDataBits">
                <option value="5">5</option>
                <option value="6">6</option>
                <option value="7">7</option>
                <option value="8">8</option>
              </select>
            </div>
            <div class="form-group">
              <label>Stop Bits:</label>
              <select v-model="serialStopBits">
                <option value="1">1</option>
                <option value="2">2</option>
              </select>
            </div>
            <div class="form-group">
              <label>Parity:</label>
              <select v-model="serialParity">
                <option value="none">None</option>
                <option value="odd">Odd</option>
                <option value="even">Even</option>
              </select>
            </div>
            <div class="form-group serial-settings-grid-span-2">
              <label>Flow Control:</label>
              <select v-model="serialFlowControl">
                <option value="none">None</option>
                <option value="hardware">Hardware</option>
                <option value="software">Software</option>
              </select>
            </div>
          </div>
        </template>

        <template v-if="isSsh">
          <div class="form-group auth-type-group">
            <label>Auth Type:</label>
            <select v-model="authType">
              <option value="password">Password</option>
              <option value="key">Private Key</option>
            </select>
          </div>

          <div v-if="authType === 'password'" class="form-group">
            <label>Password:</label>
            <input v-model="password" type="password">
          </div>

          <div v-else class="form-group">
            <label>Private Key:</label>
            <input
              ref="privateKeyFileInput"
              type="file"
              class="private-key-file-input"
              @change="handlePrivateKeyFileChange"
            >
            <div class="private-key-picker-row">
              <input
                :value="privateKeyFileName || 'No private key selected'"
                type="text"
                class="private-key-display"
                readonly
              >
              <button type="button" class="private-key-picker-btn" @click="triggerPrivateKeyPicker">Browse...</button>
              <button
                v-if="privateKeyFileName"
                type="button"
                class="private-key-clear-btn"
                @click="clearPrivateKeySelection"
              >
                Clear
              </button>
            </div>
            <div v-if="privateKeyError" class="form-hint error">{{ privateKeyError }}</div>
            <input v-model="password" type="password" placeholder="Key Passphrase (optional)" style="margin-top: 8px">
          </div>
        </template>

        <div class="form-group save-connection-group">
          <label class="save-connection-label">
            <input v-model="saveConnection" type="checkbox">
            <span>Save this connection</span>
          </label>
          <div v-if="saveConnection" class="two-column-grid">
            <input v-model="connectionName" type="text" class="save-connection-name" :placeholder="defaultName || 'Connection name (optional)'">
            <input v-model="connectionGroup" type="text" class="save-connection-name" placeholder="Group (optional)">
          </div>
        </div>

        <div class="form-group save-connection-group">
          <label class="save-connection-label">
            <input v-model="enableLog" type="checkbox">
            <span>Save session log</span>
          </label>
          <input
            v-if="enableLog"
            v-model="logFilePath"
            type="text"
            class="save-connection-name"
            :placeholder="defaultLogPath"
          >
        </div>

        <template v-if="isSsh">
          <div class="form-group save-connection-group">
            <div class="form-group" style="margin-bottom: 4px">
              <label>Reconnect Mode:</label>
              <select v-model="reconnectType">
                <option value="manual">Manual (press r to reconnect after disconnect)</option>
                <option value="simple">Simple (auto reconnect, no session persistence)</option>
                <option value="tmux">tmux (auto reconnect with session persistence)</option>
                <option value="screen">screen (auto reconnect with session persistence)</option>
              </select>
            </div>
            <div class="form-hint" style="margin-top: 2px;">
              <template v-if="reconnectType === 'manual'">
                Does not reconnect automatically. After a disconnect, press <strong>r</strong> or <strong>R</strong> in the terminal to reconnect.
              </template>
              <template v-else-if="reconnectType === 'simple'">
                Reconnects automatically after a disconnect. No server-side tools required. Running tasks will be interrupted on disconnect.
              </template>
              <template v-else-if="reconnectType === 'tmux'">
                Uses <strong>tmux</strong> to keep your session alive. Mouse scroll is enabled automatically. Requires <code>tmux</code> installed on the remote host.
              </template>
              <template v-else>
                Uses <strong>screen</strong> to keep your session alive. Requires <code>screen</code> installed on the remote host.
                AuraTerm only manages sessions whose names start with <strong>at-</strong>.
              </template>
            </div>
          </div>
        </template>

        <div class="dialog-actions">
          <button type="button" class="btn-cancel" @click="emit('cancel')">Cancel</button>
          <button type="submit" class="btn-connect" :disabled="!canConnect">Connect</button>
        </div>
      </form>
    </div>
  </div>
</template>