import { describe, expect, it } from "vitest";
import { buildSnippetPayload, snippetApplies, snippetVariables } from "../snippets";
import type { QuickButton } from "../settings";

const base: QuickButton = { id: "1", label: "Restart", command: "systemctl restart {{service}}" };

describe("snippets", () => {
  it("resolves variables and sends a line by default", () => {
    expect(snippetVariables("{{host}} {{ host }} {{port}} ")).toEqual(["host", "port"]);
    expect(buildSnippetPayload(base, { service: "nginx" })).toBe("systemctl restart nginx\n");
  });

  it("decodes control characters in raw snippets", () => {
    expect(buildSnippetPayload({ ...base, command: "^C\\e[A\\x0d", sendMode: "raw" })).toBe("\x03\x1b[A\r");
  });

  it("filters by host and saved connection group", () => {
    const scoped = { ...base, hosts: ["prod-*"], sessionGroups: ["Servers"] };
    expect(snippetApplies(scoped, "prod-api", "Servers")).toBe(true);
    expect(snippetApplies(scoped, "dev-api", "Servers")).toBe(false);
  });
});
