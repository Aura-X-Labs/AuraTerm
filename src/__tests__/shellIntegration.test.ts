import { describe, expect, it } from "vitest";
import { ShellIntegration, type ShellTerminalLike } from "../shellIntegration";

function harness() {
  let osc: (data: string) => boolean | Promise<boolean> = () => false;
  let line = 0;
  const scrolled: number[] = [];
  const terminal: ShellTerminalLike = {
    parser: { registerOscHandler: (_id, handler) => { osc = handler; return { dispose() {} }; } },
    registerMarker: () => ({ line: line++ }),
    registerDecoration: () => ({ onRender() {}, dispose() {} }),
    scrollToLine: (target) => scrolled.push(target),
  };
  const integration = new ShellIntegration(terminal);
  return { integration, osc, scrolled };
}

describe("ShellIntegration", () => {
  it("records OSC 133 commands and exit codes", () => {
    const { integration, osc } = harness();
    osc("B");
    integration.handleInput("echo hello\r");
    osc("C");
    osc("D;7");
    expect(integration.lastCommand()).toMatchObject({ command: "echo hello", exitCode: 7, source: "osc133" });
  });

  it("navigates command markers", () => {
    const { integration, osc, scrolled } = harness();
    osc("B"); integration.handleInput("one"); osc("C"); osc("D;0");
    osc("B"); integration.handleInput("two"); osc("C"); osc("D;0");
    expect(integration.previous()?.command).toBe("two");
    expect(integration.previous()?.command).toBe("one");
    expect(integration.next()?.command).toBe("two");
    expect(scrolled).toEqual([2, 0, 2]);
  });
});
