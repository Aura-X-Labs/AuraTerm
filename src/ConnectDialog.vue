<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SerialHistoryItem } from "./settings";
import type { ConnectResult, ConnectionProtocol, SerialConfig } from "./types";
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
}>(), {
  initialProtocol: "ssh",
  lastSerialConfig: null,
  recentSerialConfigs: () => [],
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

const safeDefaultName = computed(() => defaultName.value.replace(/[^a-zA-Z0-9\-_@.]/g, "_") || "session");
const defaultLogPath = computed(() => `~/AuraTerm/logs/${safeDefaultName.value}`);
const canConnect = computed(() => {
  if (isSsh.value) {
    return Boolean(host.value.trim() && user.value.trim());
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

function handleSubmit(event: Event) {
  event.preventDefault();
  if ((isSsh.value || isTelnet.value) && !host.value.trim()) {
    return;
  }
  if (isSsh.value && !user.value.trim()) {
    return;
  }
  if (isSerial.value && !serialPortName.value.trim()) {
    return;
  }

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
      password: authType.value === "password" ? password.value : undefined,
      privateKey: authType.value === "key" ? privateKey.value : undefined,
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
                <input
                  v-model="serialPortName"
                  type="text"
                  placeholder="e.g. /dev/cu.usbserial-1410"
                  autofocus
                  list="serial-port-options"
                  required
                >
                <button type="button" class="serial-refresh-btn" :disabled="loadingSerialPorts" @click="loadSerialPorts">
                  {{ loadingSerialPorts ? '...' : '↻' }}
                </button>
              </div>
              <datalist id="serial-port-options">
                <option v-for="portInfo in serialPorts" :key="portInfo.portName" :value="portInfo.portName">
                  {{ portInfo.manufacturer ? `${portInfo.manufacturer} (${portInfo.portType})` : portInfo.portType }}
                </option>
              </datalist>
              <div v-if="serialError" class="form-hint error">串口枚举失败：{{ serialError }}</div>
              <div v-else-if="serialPorts.length > 0" class="form-hint">
                已发现 {{ serialPorts.length }} 个设备：{{ serialPorts.map((item) => item.portName).join(', ') }}
              </div>
              <div v-else class="form-hint">未发现串口设备，可手动输入设备路径后连接。</div>
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
            <label>Private Key (PEM):</label>
            <textarea v-model="privateKey" placeholder="-----BEGIN RSA PRIVATE KEY-----..." rows="4" />
            <input v-model="password" type="password" placeholder="Key Passphrase (optional)" style="margin-top: 8px">
          </div>
        </template>

        <div class="form-group save-connection-group">
          <label class="save-connection-label">
            <input v-model="saveConnection" type="checkbox">
            <span>保存此连接</span>
          </label>
          <div v-if="saveConnection" class="two-column-grid">
            <input v-model="connectionName" type="text" class="save-connection-name" :placeholder="defaultName || '连接名称（可选）'">
            <input v-model="connectionGroup" type="text" class="save-connection-name" placeholder="分组（可选）">
          </div>
        </div>

        <div class="form-group save-connection-group">
          <label class="save-connection-label">
            <input v-model="enableLog" type="checkbox">
            <span>保存会话日志</span>
          </label>
          <input
            v-if="enableLog"
            v-model="logFilePath"
            type="text"
            class="save-connection-name"
            :placeholder="`${defaultLogPath}_YYYYMMDD_HHmmss.log`"
          >
        </div>

        <div class="dialog-actions">
          <button type="button" class="btn-cancel" @click="emit('cancel')">Cancel</button>
          <button type="submit" class="btn-connect" :disabled="!canConnect">Connect</button>
        </div>
      </form>
    </div>
  </div>
</template>