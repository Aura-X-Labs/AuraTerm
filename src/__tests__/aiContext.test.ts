import { describe, expect, it } from "vitest";
import { buildExplainPrompt, trimOutput, MAX_OUTPUT_CHARS } from "../aiContext";

describe("trimOutput", () => {
  it("returns short output unchanged", () => {
    const output = "line1\nline2\nline3";
    expect(trimOutput(output)).toBe(output);
  });

  it("keeps head and tail of oversized output with a truncation marker", () => {
    const lines = Array.from({ length: 2000 }, (_, i) => `line-${i} ${"x".repeat(20)}`);
    const output = lines.join("\n");
    const trimmed = trimOutput(output, 2000);

    expect(trimmed.length).toBeLessThan(output.length);
    expect(trimmed).toContain("line-0 ");
    expect(trimmed).toContain(`line-${lines.length - 1} `);
    expect(trimmed).toMatch(/… \[\d+ lines truncated\] …/);
    // Middle content is gone
    expect(trimmed).not.toContain("line-1000 ");
  });

  it("reports the number of omitted lines accurately", () => {
    const lines = Array.from({ length: 100 }, (_, i) => `l${String(i).padStart(3, "0")}`);
    const output = lines.join("\n");
    const trimmed = trimOutput(output, 200);

    const match = trimmed.match(/… \[(\d+) lines truncated\] …/);
    expect(match).not.toBeNull();
    const omitted = Number(match![1]);
    const kept = trimmed.split("\n").length - 1; // minus the marker line
    expect(kept + omitted).toBe(lines.length);
  });

  it("falls back to a character slice for one huge line", () => {
    const output = "x".repeat(50_000);
    const trimmed = trimOutput(output, 1000);
    expect(trimmed.length).toBeLessThan(2000);
    expect(trimmed).toContain("… [output truncated] …");
    expect(trimmed.startsWith("x")).toBe(true);
    expect(trimmed.endsWith("x")).toBe(true);
  });

  it("respects the default cap", () => {
    const output = "y".repeat(MAX_OUTPUT_CHARS * 3);
    expect(trimOutput(output).length).toBeLessThan(MAX_OUTPUT_CHARS + 100);
  });
});

describe("buildExplainPrompt", () => {
  it("includes command, exit code, and fenced output", () => {
    const prompt = buildExplainPrompt(
      {
        command: "ls -la /missing",
        output: "ls: /missing: No such file or directory",
        exitCode: 1,
        shell: "/bin/zsh",
        os: "macOS",
      },
      "en",
    );

    expect(prompt).toContain("failed");
    expect(prompt).toContain("ls -la /missing");
    expect(prompt).toContain("Exit code: 1");
    expect(prompt).toContain("No such file or directory");
    expect(prompt).toContain("OS: macOS");
    expect(prompt).toContain("Shell/session: /bin/zsh");
    expect(prompt).toContain("Respond in English.");
  });

  it("uses the neutral phrasing for successful commands", () => {
    const prompt = buildExplainPrompt(
      { command: "uname -a", output: "Darwin host 25.5.0", exitCode: 0 },
      "en",
    );
    expect(prompt).toContain("Explain the following terminal command");
    expect(prompt).not.toContain("failed");
  });

  it("marks empty output explicitly and follows the zh-CN locale", () => {
    const prompt = buildExplainPrompt(
      { command: "true", output: "   " },
      "zh-CN",
    );
    expect(prompt).toContain("Output: (empty)");
    expect(prompt).toContain("请用简体中文回答。");
  });

  it("trims oversized output inside the prompt", () => {
    const prompt = buildExplainPrompt(
      {
        command: "cat big.log",
        output: Array.from({ length: 5000 }, (_, i) => `entry ${i}`).join("\n"),
      },
      "en",
    );
    expect(prompt).toMatch(/… \[\d+ lines truncated\] …/);
    expect(prompt.length).toBeLessThan(MAX_OUTPUT_CHARS + 1000);
  });
});
