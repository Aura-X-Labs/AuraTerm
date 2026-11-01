import { describe, expect, it, vi } from "vitest";

import { insertAiCommand, sendInputText } from "../terminalInsert";

function fakeHandle() {
  return {
    pasteText: vi.fn(),
    writeInput: vi.fn(),
    sendData: vi.fn(),
    focus: vi.fn(),
  };
}

describe("terminal text routing", () => {
  it("inserts AI commands through paste processing, never as raw input", () => {
    const handle = fakeHandle();
    insertAiCommand(handle, "rm -rf build\nmake\n");
    expect(handle.pasteText).toHaveBeenCalledWith("rm -rf build\nmake\n");
    expect(handle.writeInput).not.toHaveBeenCalled();
    expect(handle.focus).toHaveBeenCalled();
  });

  it("sends raw input-bar text unchanged so control characters act", () => {
    const handle = fakeHandle();
    sendInputText(handle, "\x03", true);
    expect(handle.writeInput).toHaveBeenCalledWith("\x03");
    expect(handle.pasteText).not.toHaveBeenCalled();
  });

  it("hands raw input-bar text to the broadcast fan-out, which xterm's onData no longer reaches", () => {
    const handle = fakeHandle();
    const broadcast = vi.fn();
    sendInputText(handle, "\x03", true, broadcast);
    expect(broadcast).toHaveBeenCalledExactlyOnceWith("\x03");
  });

  it("sends line-mode input-bar text through sendData", () => {
    const handle = fakeHandle();
    sendInputText(handle, "ls\n", false);
    expect(handle.sendData).toHaveBeenCalledWith("ls\n");
    expect(handle.writeInput).not.toHaveBeenCalled();
  });
});
