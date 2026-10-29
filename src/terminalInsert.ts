import type { TerminalHandle } from "./types";

type InsertTarget = Pick<TerminalHandle, "pasteText" | "writeInput" | "sendData" | "focus">;

/**
 * Put an AI-suggested command at the prompt. It goes through xterm's paste
 * processing so a shell with bracketed paste on receives a multi-line block
 * as one paste instead of running every line but the last.
 */
export function insertAiCommand(handle: InsertTarget, text: string): void {
  handle.pasteText(text);
  handle.focus();
}

/**
 * Send input-bar or snippet text. Raw mode is for control sequences such as
 * ^C or escape codes, so it must reach the PTY unchanged rather than be
 * wrapped in paste markers; line mode appends Enter via sendData.
 */
export function sendInputText(handle: InsertTarget, text: string, raw: boolean): void {
  if (raw) {
    handle.writeInput(text);
  } else {
    handle.sendData(text);
  }
}
