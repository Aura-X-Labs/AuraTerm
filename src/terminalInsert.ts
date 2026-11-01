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
 *
 * Raw text skips xterm's onData, which is what fans typed input out to the
 * other panes, so it is handed to `broadcast` here instead.
 */
export function sendInputText(
  handle: InsertTarget,
  text: string,
  raw: boolean,
  broadcast: (data: string) => void = () => {},
): void {
  if (raw) {
    handle.writeInput(text);
    broadcast(text);
  } else {
    handle.sendData(text);
  }
}
