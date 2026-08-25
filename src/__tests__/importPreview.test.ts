import { describe, expect, it } from "vitest";
import {
  commandRisks,
  countActions,
  credentialRisks,
  hasRisks,
  type ImportAction,
  type ImportPlanEntry,
  type ImportRisks,
} from "../importPreview";

const noRisks: ImportRisks = {
  postConnectCommands: 0,
  autoLoginResponses: 0,
  jumpHostCredentials: 0,
  passwords: 0,
  privateKeys: 0,
};

function entry(index: number, disposition: ImportAction): ImportPlanEntry {
  return { index, name: `host-${index}`, protocol: "ssh", target: "ops@10.0.0.1:22", disposition };
}

describe("import preview", () => {
  it("counts what the plan would do, letting per-row overrides win", () => {
    const entries = [entry(0, "add"), entry(1, "update"), entry(2, "skip")];
    expect(countActions(entries, new Map())).toEqual({ add: 1, update: 1, skip: 1 });

    // The user kept the defaults for rows 0 and 2 but rejected the update.
    const overrides = new Map<number, ImportAction>([[1, "skip"]]);
    expect(countActions(entries, overrides)).toEqual({ add: 1, update: 0, skip: 2 });
  });

  it("separates what runs on connect from what authenticates", () => {
    const risks: ImportRisks = {
      ...noRisks,
      postConnectCommands: 3,
      autoLoginResponses: 2,
      passwords: 1,
    };
    // The trust gate has two switches; each one needs its own tally.
    expect(commandRisks(risks)).toBe(5);
    expect(credentialRisks(risks)).toBe(1);
    expect(hasRisks(risks)).toBe(true);
  });

  it("stays quiet for a file that carries nothing dangerous", () => {
    expect(hasRisks(noRisks)).toBe(false);
    expect(commandRisks(noRisks)).toBe(0);
    expect(credentialRisks(noRisks)).toBe(0);
  });
});
