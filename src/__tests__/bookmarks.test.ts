import { describe, expect, it } from "vitest";
import { buildBookmarkTree, flattenBookmarkTree, normalizeBookmarkPath } from "../bookmarks";
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
