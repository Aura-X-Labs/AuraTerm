export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
}

export interface QuickButton {
  id: string;
  /** 显示名称，为空时展示 command 的前 20 个字符 */
  label: string;
  /** 点击后发送到终端的命令，会自动补 \n */
  command: string;
}

export interface AppSettings {
  fontSize: number;
  fontFamily: string;
  scrollback: number;
  shellPath: string | null;
  theme: TerminalTheme;
  /** 选中即复制：选中文本后自动写入剪贴板；同时 Ctrl+C 在有选中时消费按键（不发 ^C 给 PTY） */
  ctrlCCopy: boolean;
  /** Ctrl+V：粘贴剪贴板内容到终端 */
  ctrlVPaste: boolean;
  /** 鼠标中键：粘贴剪贴板内容到终端 */
  middleClickPaste: boolean;
  /** 终端下方快捷按钮列表 */
  quickButtons: QuickButton[];
}

export const DEFAULT_SETTINGS: AppSettings = {
  fontSize: 14,
  fontFamily: 'Consolas, "Courier New", monospace',
  scrollback: 1000,
  shellPath: null,
  theme: {
    background: "#000000",
    foreground: "#ffffff",
    cursor: "#ffffff",
  },
  ctrlCCopy: true,
  ctrlVPaste: true,
  middleClickPaste: true,
  quickButtons: [],
};
