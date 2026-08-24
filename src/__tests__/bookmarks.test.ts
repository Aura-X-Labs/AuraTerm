import { describe, expect, it } from "vitest";
import {
  buildBookmarkTree,
  collectGroupPaths,
  connectionTarget,
  filterConnections,
  flattenBookmarkTree,
  matchesScope,
  normalizeBookmarkPath,
  renameGroupPath,
  rewriteGroupPath,
  rewriteGroupPaths,
  sortConnections,
} from "../bookmarks";
import type { SavedConnection } from "../types";

function connection(id: string, group?: string): SavedConnection {
  return {
    id, name: id, group, protocol: "ssh", host: id, port: 22, user: "ops",
    authType: "agent", createdAt: 1,
  };
}

describe("bookmark tree", () => {
  it("normalizes slash and backslash folder paths", () => {
    expect(normalizeBookmarkPath(" Production\\EU / Web ")).toEqual(["Production", "EU", "Web"]);
  });

  it("collects group paths with their parent folders, de-duplicated and sorted", () => {
    expect(collectGroupPaths([
      connection("web", "Production/EU/Web"),
      connection("api", "Production/EU"),
      connection("dev", "dev"),
      connection("loose"),
      connection("blank", "   "),
    ])).toEqual(["dev", "Production", "Production/EU", "Production/EU/Web"]);
  });

  it("builds arbitrary nested folders and honors collapse state", () => {
    const tree = buildBookmarkTree([
      connection("api", "Production/EU"),
      connection("web", "Production/EU/Web"),
      connection("loose"),
    ]);
    expect(tree.folders[0].count).toBe(2);
    expect(flattenBookmarkTree(tree, new Set()).map((row) => row.key)).toContain("folder:Production/EU/Web");
    expect(flattenBookmarkTree(tree, new Set(["Production"])).map((row) => row.key)).toEqual([
      "folder:Production",
      "connection:loose",
    ]);
  });
});

describe("bookmark list helpers", () => {
  const list = [
    { ...connection("web", "Production/EU"), name: "web", user: "ops", host: "10.0.0.1", lastUsed: 300 },
    { ...connection("db", "Production"), name: "db", user: "dba", host: "10.0.0.2", lastUsed: 100 },
    { ...connection("loose"), name: "loose", user: "root", host: "192.168.1.9" },
  ];

  it("renders a target per protocol", () => {
    expect(connectionTarget(list[0])).toBe("ops@10.0.0.1:22");
    expect(connectionTarget({ ...list[0], protocol: "telnet" })).toBe("10.0.0.1:22");
    expect(connectionTarget({ ...list[0], protocol: "serial", portName: "/dev/ttyS0", baudRate: 115200 }))
      .toBe("/dev/ttyS0 @ 115200");
  });

  it("scopes a group to itself plus its subfolders", () => {
    expect(list.filter((item) => matchesScope(item, { kind: "group", path: "Production" })).map((item) => item.name))
      .toEqual(["web", "db"]);
    expect(list.filter((item) => matchesScope(item, { kind: "group", path: "Production/EU" })).map((item) => item.name))
      .toEqual(["web"]);
    expect(list.filter((item) => matchesScope(item, { kind: "ungrouped" })).map((item) => item.name)).toEqual(["loose"]);
    expect(list.filter((item) => matchesScope(item, { kind: "recent" })).map((item) => item.name)).toEqual(["web", "db"]);
  });

  it("filters across name, host, user and group", () => {
    expect(filterConnections(list, "dba").map((item) => item.name)).toEqual(["db"]);
    expect(filterConnections(list, "production/eu").map((item) => item.name)).toEqual(["web"]);
    expect(filterConnections(list, "  ").map((item) => item.name)).toEqual(["web", "db", "loose"]);
  });

  it("sorts never-used bookmarks as oldest", () => {
    expect(sortConnections(list, "lastUsed", "desc").map((item) => item.name)).toEqual(["web", "db", "loose"]);
    expect(sortConnections(list, "lastUsed", "asc").map((item) => item.name)).toEqual(["loose", "db", "web"]);
    expect(sortConnections(list, "name", "asc").map((item) => item.name)).toEqual(["db", "loose", "web"]);
  });

  it("renames a group path and carries its subfolders", () => {
    const moved = renameGroupPath(list, "Production", "Prod/2026");
    expect(moved.map((item) => [item.name, item.group])).toEqual([["web", "Prod/2026/EU"], ["db", "Prod/2026"]]);
    expect(renameGroupPath(list, "Production", "Production")).toEqual([]);
  });

  it("rewrites a single path and dissolves a group into its parent", () => {
    expect(rewriteGroupPath("Production/EU", "Production", "Prod")).toBe("Prod/EU");
    expect(rewriteGroupPath("Production", "Production", "")).toBe("");
    expect(rewriteGroupPath("Production/EU", "Production", "")).toBe("EU");
    expect(rewriteGroupPath("Staging", "Production", "Prod")).toBeNull();
  });

  it("keeps the explicit group list in step with a rename", () => {
    expect(rewriteGroupPaths(["Production", "Production/EU", "Lab"], "Production", "Prod"))
      .toEqual(["Lab", "Prod", "Prod/EU"]);
    // A dissolved group drops out of the list; its children are promoted.
    expect(rewriteGroupPaths(["Lab", "Lab/Bench"], "Lab", "")).toEqual(["Bench"]);
  });
});
