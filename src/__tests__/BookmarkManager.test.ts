import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import BookmarkManager from "../BookmarkManager.vue";
import { i18n } from "../i18n";
import type { SavedConnection } from "../types";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
const dialogs = vi.hoisted(() => ({
  confirmDialog: vi.fn(async () => true),
  alertDialog: vi.fn(async () => undefined),
}));
const prompt = vi.hoisted(() => ({ promptText: vi.fn(async () => "" as string | null) }));
const transfer = vi.hoisted(() => ({ downloadText: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("../nativeDialogs", () => dialogs);
vi.mock("../promptDialog", () => prompt);
vi.mock("../bookmarkTransfer", async (importOriginal) => ({
  ...await importOriginal<typeof import("../bookmarkTransfer")>(),
  downloadText: transfer.downloadText,
}));

function connection(id: string, group: string | undefined, lastUsed?: number): SavedConnection {
  return {
    id,
    name: id,
    group,
    protocol: "ssh",
    host: `10.0.0.${id.length}`,
    port: 22,
    user: "ops",
    authType: "key",
    createdAt: 1,
    lastUsed,
  };
}

let store: SavedConnection[] = [];

function mountManager(bookmarkGroups: string[] = []) {
  return mount(BookmarkManager, {
    props: { bookmarkGroups },
    global: { plugins: [i18n] },
  });
}

function menuItem(wrapper: ReturnType<typeof mountManager>, selector: string, index: number) {
  return wrapper.findAll(selector)[index];
}

beforeEach(() => {
  store = [
    connection("web-1", "Production/EU", 300),
    connection("web-2", "Production/EU", 200),
    connection("bastion", "Production", 100),
    connection("scratch", undefined),
  ];
  dialogs.confirmDialog.mockResolvedValue(true);
  prompt.promptText.mockResolvedValue("");
  tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "get_connections":
        return store.map((item) => ({ ...item }));
      case "get_credential_security_state":
        return { unlocked: true };
      case "delete_connection":
        store = store.filter((item) => item.id !== args?.id);
        return null;
      case "delete_connections": {
        const ids = new Set(args?.ids as string[]);
        const before = store.length;
        store = store.filter((item) => !ids.has(item.id));
        return before - store.length;
      }
      case "move_connections": {
        const ids = new Set(args?.ids as string[]);
        const group = (args?.group as string | null) ?? undefined;
        let moved = 0;
        store = store.map((item) => {
          if (!ids.has(item.id) || item.group === group) return item;
          moved += 1;
          return { ...item, group };
        });
        return moved;
      }
      case "rename_group":
        return 2;
      case "duplicate_connection":
        return "copy-id";
      case "export_bookmarks":
        return "{}";
      case "save_connection":
        return (args?.connection as SavedConnection).id;
      case "touch_connection":
        return null;
      default:
        throw new Error(`unexpected command ${command}`);
    }
  });
});

describe("BookmarkManager", () => {
  it("lists every bookmark, most recently used first", async () => {
    const wrapper = mountManager();
    await flushPromises();

    const names = wrapper.findAll(".bm-row .bm-name").map((cell) => cell.text().trim());
    expect(names).toEqual(["web-1", "web-2", "bastion", "scratch"]);
  });

  it("narrows the list to the selected group, subfolders included", async () => {
    const wrapper = mountManager();
    await flushPromises();

    const production = wrapper.findAll(".bm-tree-item--folder").find((item) => item.text().startsWith("Production"));
    await production!.trigger("click");

    expect(wrapper.findAll(".bm-row")).toHaveLength(3);

    const ungrouped = wrapper.findAll(".bm-tree-item")[2];
    await ungrouped.trigger("click");
    expect(wrapper.findAll(".bm-row .bm-name").map((cell) => cell.text().trim())).toEqual(["scratch"]);
  });

  it("shows the batch bar once rows are checked and deletes them in one call", async () => {
    const wrapper = mountManager();
    await flushPromises();

    expect(wrapper.find(".bm-batch").exists()).toBe(false);

    await wrapper.findAll(".bm-row")[0].find(".bm-check").trigger("click");
    await wrapper.findAll(".bm-row")[1].find(".bm-check").trigger("click");

    const batch = wrapper.find(".bm-batch");
    expect(batch.exists()).toBe(true);
    expect(batch.find(".bm-batch-count").text()).toContain("2");

    await batch.find(".bm-btn--danger").trigger("click");
    await flushPromises();

    expect(dialogs.confirmDialog).toHaveBeenCalledTimes(1);
    expect(tauri.invoke).toHaveBeenCalledWith("delete_connections", { ids: ["web-1", "web-2"] });
    expect(wrapper.findAll(".bm-row")).toHaveLength(2);
    expect(wrapper.find(".bm-batch").exists()).toBe(false);
  });

  it("connects on double click after recording the use", async () => {
    const wrapper = mountManager();
    await flushPromises();

    await wrapper.findAll(".bm-row")[0].trigger("dblclick");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("touch_connection", expect.objectContaining({ id: "web-1" }));
    expect(wrapper.emitted("connect")?.[0]?.[0]).toMatchObject({ id: "web-1" });
    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("keeps the detail column on a row that is still visible", async () => {
    const wrapper = mountManager();
    await flushPromises();
    expect(wrapper.find(".bm-detail-name").text()).toBe("web-1");

    const ungrouped = wrapper.findAll(".bm-tree-item")[2];
    await ungrouped.trigger("click");
    await flushPromises();

    expect(wrapper.find(".bm-detail-name").text()).toBe("scratch");
  });

  it("moves a dragged bookmark into the group it is dropped on", async () => {
    const wrapper = mountManager();
    await flushPromises();

    const row = wrapper.findAll(".bm-row")[3]; // scratch, ungrouped
    await row.trigger("dragstart");

    const production = wrapper.findAll(".bm-tree-item--folder").find((item) => item.text().startsWith("Production"));
    await production!.trigger("dragover");
    await production!.trigger("drop");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("move_connections", { ids: ["scratch"], group: "Production" });
  });

  it("renames a group and carries the explicit group list with it", async () => {
    const wrapper = mountManager(["Production/Empty"]);
    await flushPromises();

    const production = wrapper.findAll(".bm-tree-item--folder").find((item) => item.text().startsWith("Production"));
    await production!.trigger("contextmenu");
    prompt.promptText.mockResolvedValue("Prod");
    await menuItem(wrapper, ".bm-context-menu .bm-menu-item", 0).trigger("click");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("rename_group", { from: "Production", to: "Prod" });
    expect(wrapper.emitted("updateGroups")?.[0]?.[0]).toEqual(["Prod/Empty"]);
  });

  it("remembers a newly created group so an empty folder survives", async () => {
    const wrapper = mountManager(["Lab"]);
    await flushPromises();

    prompt.promptText.mockResolvedValue("Staging");
    await wrapper.find(".bm-tree-new").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("updateGroups")?.[0]?.[0]).toEqual(["Lab", "Staging"]);
  });

  it("acts on the right-clicked row through its context menu", async () => {
    const wrapper = mountManager();
    await flushPromises();

    await wrapper.findAll(".bm-row")[2].trigger("contextmenu"); // bastion
    const items = wrapper.findAll(".bm-context-menu .bm-menu-item");
    expect(items).toHaveLength(4);

    await items[3].trigger("click"); // delete
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("delete_connections", { ids: ["bastion"] });
    expect(wrapper.find(".bm-context-menu").exists()).toBe(false);
  });

  it("refuses to save while credentials are locked, instead of erasing them", async () => {
    tauri.invoke.mockImplementation(async (command: string) => {
      if (command === "get_connections") return store.map((item) => ({ ...item }));
      if (command === "get_credential_security_state") return { unlocked: false };
      throw new Error(`unexpected command ${command}`);
    });

    const wrapper = mountManager();
    await flushPromises();
    expect(wrapper.find(".bm-banner").exists()).toBe(true);

    await wrapper.find(".bookmark-editor-footer .primary").trigger("click");
    await flushPromises();

    expect(tauri.invoke).not.toHaveBeenCalledWith("save_connection", expect.anything());
    expect(dialogs.alertDialog).toHaveBeenCalledTimes(1);
  });

  it("exports the checked bookmarks without secrets", async () => {
    const wrapper = mountManager();
    await flushPromises();

    await wrapper.findAll(".bm-row")[0].find(".bm-check").trigger("click");
    const exportButton = wrapper.findAll(".bm-batch .bm-btn").find((button) => button.text() === "Export");
    await exportButton!.trigger("click");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("export_bookmarks", { ids: ["web-1"], includeSecrets: false });
    expect(transfer.downloadText).toHaveBeenCalledWith(expect.stringContaining("auraterm-bookmarks-selection"), "{}");
  });
});
