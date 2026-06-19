import { describe, expect, it } from "vitest";
import { OutputRuleEngine, ruleAppliesToHost, stripTerminalSequences } from "../outputRules";
import type { OutputRule } from "../settings";

function rule(overrides: Partial<OutputRule> = {}): OutputRule {
  return {
    id: "error",
    name: "Errors",
    enabled: true,
    pattern: "ERROR (\\d+)",
    isRegex: true,
    caseSensitive: false,
    scope: "global",
    hosts: [],
    foreground: "#ff0000",
    bell: false,
    notify: true,
    autoResponse: "ack $1",
    cooldownMs: 1000,
    ...overrides,
  };
}

describe("OutputRuleEngine", () => {
  it("highlights matches while preserving existing terminal sequences", () => {
    const result = new OutputRuleEngine().process("\x1b[1mERROR 42\x1b[0m", [rule()]);
    expect(result.rendered).toContain("\x1b[38;2;255;0;0mERROR 42\x1b[0m");
    expect(stripTerminalSequences(result.rendered)).toBe("ERROR 42");
  });

  it("matches triggers across output chunk boundaries and honors cooldown", () => {
    const engine = new OutputRuleEngine();
    expect(engine.process("ERR", [rule()], undefined, 100).matches).toHaveLength(0);
    const match = engine.process("OR 7", [rule()], undefined, 200).matches[0];
    expect(match.response).toBe("ack 7");
    expect(engine.process(" ERROR 8", [rule()], undefined, 500).matches).toHaveLength(0);
    expect(engine.process(" ERROR 9", [rule()], undefined, 1300).matches).toHaveLength(1);
  });

  it("ignores invalid expressions and filters host-scoped rules", () => {
    expect(new OutputRuleEngine().process("anything", [rule({ pattern: "[" })]).rendered).toBe("anything");
    const scoped = rule({ scope: "hosts", hosts: ["prod-*.example.com"] });
    expect(ruleAppliesToHost(scoped, "prod-01.example.com")).toBe(true);
    expect(ruleAppliesToHost(scoped, "dev.example.com")).toBe(false);
  });
});
