import { describe, expect, it } from "vitest";
import { commandGenerationSystemPrompt, extractCommand } from "../aiPrompts";

describe("extractCommand", () => {
  it("returns a plain command unchanged", () => {
    expect(extractCommand("ls -la")).toBe("ls -la");
  });

  it("strips a wrapping code fence with a language tag", () => {
    expect(extractCommand("```bash\ngit status\n```")).toBe("git status");
  });

  it("strips a bare code fence", () => {
    expect(extractCommand("```\ndf -h /\n```")).toBe("df -h /");
  });

  it("keeps only the first non-empty line when the model appends a note", () => {
    expect(extractCommand("tar -czf out.tgz dir/\n\n# creates a gzip archive")).toBe(
      "tar -czf out.tgz dir/",
    );
  });

  it("maps the UNSUPPORTED sentinel to an empty string", () => {
    expect(extractCommand("UNSUPPORTED")).toBe("");
    expect(extractCommand("  UNSUPPORTED  ")).toBe("");
  });

  it("returns empty for blank replies", () => {
    expect(extractCommand("")).toBe("");
    expect(extractCommand("   \n  ")).toBe("");
  });

  it("preserves a one-line pipeline", () => {
    expect(extractCommand("ps aux | grep node | awk '{print $2}'")).toBe(
      "ps aux | grep node | awk '{print $2}'",
    );
  });
});

describe("commandGenerationSystemPrompt", () => {
  it("includes environment hints and the single-command constraint", () => {
    const prompt = commandGenerationSystemPrompt({ os: "linux", shell: "/bin/bash" });
    expect(prompt).toContain("Operating system: linux.");
    expect(prompt).toContain("Shell/session: /bin/bash.");
    expect(prompt).toContain("Return ONLY the command");
    expect(prompt).toContain("UNSUPPORTED");
  });

  it("omits environment lines when unknown", () => {
    const prompt = commandGenerationSystemPrompt({});
    expect(prompt).not.toContain("Operating system:");
    expect(prompt).not.toContain("Shell/session:");
  });
});
