import { describe, expect, it } from "vitest";
import {
  buildExplainPrompt,
  buildOptimizePrompt,
  buildSummarizePrompt,
  trimOutput,
  MAX_OUTPUT_CHARS,
} from "../aiContext";

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

describe("buildOptimizePrompt", () => {
  it("includes command, environment, and fenced output", () => {
    const prompt = buildOptimizePrompt(
      {
        command: "ls | grep foo",
        output: "foo.txt\nfoobar.txt",
        exitCode: 0,
        shell: "/bin/zsh",
        os: "macOS",
      },
      "en",
    );

    expect(prompt).toContain("Suggest a better way");
    expect(prompt).toContain("ls | grep foo");
    expect(prompt).toContain("Exit code: 0");
    expect(prompt).toContain("foobar.txt");
    expect(prompt).toContain("OS: macOS");
    expect(prompt).toContain("Shell/session: /bin/zsh");
    expect(prompt).toContain("Respond in English.");
  });

  it("marks empty output and follows the zh-CN locale", () => {
    const prompt = buildOptimizePrompt({ command: "true", output: "  " }, "zh-CN");
    expect(prompt).toContain("Output: (empty)");
    expect(prompt).not.toContain("Exit code:");
    expect(prompt).toContain("请用简体中文回答。");
  });

  it("trims oversized output inside the prompt", () => {
    const prompt = buildOptimizePrompt(
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

describe("buildSummarizePrompt", () => {
  it("includes environment hints and fenced output", () => {
    const prompt = buildSummarizePrompt(
      {
        output: "PASS test_a\nFAIL test_b — assertion error",
        shell: "ssh root@web-01",
        os: "linux",
      },
      "en",
    );

    expect(prompt).toContain("Summarize the following terminal output");
    expect(prompt).toContain("FAIL test_b");
    expect(prompt).toContain("OS: linux");
    expect(prompt).toContain("Shell/session: ssh root@web-01");
    expect(prompt).toContain("Respond in English.");
  });

  it("marks empty output and follows the zh-CN locale", () => {
    const prompt = buildSummarizePrompt({ output: "   " }, "zh-CN");
    expect(prompt).toContain("Output: (empty)");
    expect(prompt).toContain("请用简体中文回答。");
  });

  it("trims oversized output inside the prompt", () => {
    const prompt = buildSummarizePrompt(
      { output: Array.from({ length: 5000 }, (_, i) => `row ${i}`).join("\n") },
      "en",
    );
    expect(prompt).toMatch(/… \[\d+ lines truncated\] …/);
    expect(prompt.length).toBeLessThan(MAX_OUTPUT_CHARS + 1000);
  });
});
