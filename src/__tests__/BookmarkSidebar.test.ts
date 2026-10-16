import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import BookmarkSidebar from "../BookmarkSidebar.vue";
import { i18n, t } from "../i18n";
import type { SavedConnection } from "../types";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("../nativeDialogs", () => ({ confirmDialog: vi.fn(async () => true), alertDialog: vi.fn(async () => undefined) }));

// This jsdom run has no usable localStorage; the store only treats it as a convenience.
Object.defineProperty(window, "localStorage", {
  configurable: true,
  value: { getItem: () => null, setItem: () => undefined, removeItem: () => undefined, clear: () => undefined },
});

function connection(id: string, extra: Partial<SavedConnection> = {}): SavedConnection {
  return { id, name: id, protocol: "ssh", host: "10.0.0.1", port: 22, user: "ops", authType: "key", createdAt: 1, ...extra };
}

let store: SavedConnection[] = [];

beforeEach(() => {
  store = [
    connection("web", { lastUsed: 300 }),
    connection("db", { lastUsed: 200, pinned: true }),
    connection("cache"),
  ];
  tauri.invoke.mockReset();
  tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "get_connections":
        return store.map((item) => ({ ...item }));
      case "get_credential_security_state":
        return { unlocked: true };
      case "reorder_connections": {
        const ids = args?.ids as string[];
        const field = args?.pinned ? "pinOrder" : "order";
        store = store.map((item) => (ids.includes(item.id) ? { ...item, [field]: ids.indexOf(item.id) } : item));
        return null;
      }
      case "set_connection_pinned":
        store = store.map((item) => (item.id === args?.id ? { ...item, pinned: args?.pinned as boolean, pinOrder: undefined } : item));
        return null;
      default:
        throw new Error(`unexpected command ${command}`);
    }
  });
});

async function mountSidebar() {
  const wrapper = mount(BookmarkSidebar, { global: { plugins: [i18n] } });
  await flushPromises();
  return wrapper;
}

function section(wrapper: Awaited<ReturnType<typeof mountSidebar>>, key: "pinned" | "recent") {
  return wrapper.findAll(`.bookmark-group--${key} .bookmark-name`).map((node) => node.text());
}

describe("BookmarkSidebar pinning", () => {
  it("lists pinned bookmarks above recent ones, without repeating them there", async () => {
    const wrapper = await mountSidebar();
    const headers = wrapper.findAll(".bookmark-group > .bookmark-group-header .bookmark-group-name").map((node) => node.text());
    expect(headers).toEqual([t("bookmarks.pinned"), t("bookmarks.recentlyUsed")]);
    expect(section(wrapper, "pinned")).toEqual(["db"]);
    expect(section(wrapper, "recent")).toEqual(["web"]);
  });

  it("pins from the right-click menu", async () => {
    const wrapper = await mountSidebar();
    const cache = wrapper.findAll(".bookmark-list > .bookmark-item").find((row) => row.text().includes("cache"));
    await cache!.trigger("contextmenu");
    const pin = wrapper.find(".bookmark-context-item--pin");
    expect(pin.text()).toContain(t("bookmarks.pin"));
    await pin.trigger("click");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("set_connection_pinned", { id: "cache", pinned: true });
    expect(section(wrapper, "pinned")).toEqual(["cache", "db"]);
  });

  it("unpins, and the bookmark returns to the recent list", async () => {
    const wrapper = await mountSidebar();
    await wrapper.find(".bookmark-group--pinned .bookmark-item").trigger("contextmenu");
    const unpin = wrapper.find(".bookmark-context-item--pin");
    expect(unpin.text()).toContain(t("bookmarks.unpin"));
    await unpin.trigger("click");
    await flushPromises();

    expect(wrapper.find(".bookmark-group--pinned").exists()).toBe(false);
    expect(section(wrapper, "recent")).toEqual(["web", "db"]);
  });

  it("hides the shortcut sections while searching", async () => {
    const wrapper = await mountSidebar();
    await wrapper.find(".bookmark-search-input").setValue("db");
    expect(wrapper.find(".bookmark-group--pinned").exists()).toBe(false);
  });
});

describe("BookmarkSidebar move up/down", () => {
  function treeNames(wrapper: Awaited<ReturnType<typeof mountSidebar>>) {
    return wrapper.findAll(".bookmark-list > .bookmark-item .bookmark-name").map((node) => node.text());
  }

  async function openMenu(wrapper: Awaited<ReturnType<typeof mountSidebar>>, name: string) {
    const row = wrapper.findAll(".bookmark-list > .bookmark-item").find((item) => item.find(".bookmark-name").text() === name);
    await row!.trigger("contextmenu");
  }

  it("moves a bookmark up within its group and keeps the order", async () => {
    const wrapper = await mountSidebar();
    expect(treeNames(wrapper)).toEqual(["cache", "db", "web"]);

    await openMenu(wrapper, "web");
    await wrapper.find(".bookmark-context-item--up").trigger("click");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("reorder_connections", { ids: ["cache", "web", "db"], pinned: false });
    expect(treeNames(wrapper)).toEqual(["cache", "web", "db"]);
  });

  it("disables the move that would leave the group", async () => {
    const wrapper = await mountSidebar();
    await openMenu(wrapper, "cache");
    expect(wrapper.find(".bookmark-context-item--up").attributes("disabled")).toBeDefined();
    expect(wrapper.find(".bookmark-context-item--down").attributes("disabled")).toBeUndefined();
  });

  it("does not offer moving from the recent section", async () => {
    const wrapper = await mountSidebar();
    await wrapper.find(".bookmark-group--recent .bookmark-item").trigger("contextmenu");
    expect(wrapper.find(".bookmark-context-item--up").exists()).toBe(false);
  });

  it("reorders the pinned section without touching the group order", async () => {
    store = store.map((item) => ({ ...item, pinned: true }));
    const wrapper = await mountSidebar();
    expect(section(wrapper, "pinned")).toEqual(["cache", "db", "web"]);

    const web = wrapper.findAll(".bookmark-group--pinned .bookmark-item")[2];
    await web.trigger("contextmenu");
    expect(wrapper.find(".bookmark-context-item--down").attributes("disabled")).toBeDefined();
    await wrapper.find(".bookmark-context-item--up").trigger("click");
    await flushPromises();

    expect(tauri.invoke).toHaveBeenCalledWith("reorder_connections", { ids: ["cache", "web", "db"], pinned: true });
    expect(section(wrapper, "pinned")).toEqual(["cache", "web", "db"]);
    expect(treeNames(wrapper)).toEqual(["cache", "db", "web"]);
  });
});

describe("BookmarkSidebar folding", () => {
  it("folds the pinned section apart from a group named like it", async () => {
    store = [connection("db", { pinned: true }), connection("in-group", { group: t("bookmarks.pinned") })];
    const wrapper = await mountSidebar();
    await wrapper.find(".bookmark-group--pinned .bookmark-group-header").trigger("click");

    expect(wrapper.find(".bookmark-group--pinned").classes()).toContain("collapsed");
    expect(wrapper.find(".bookmark-tree-folder").classes()).not.toContain("collapsed");
    expect(wrapper.findAll(".bookmark-list > .bookmark-item .bookmark-name").map((node) => node.text())).toContain("in-group");
  });
});
