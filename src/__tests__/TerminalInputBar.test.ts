import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import TerminalInputBar from "../TerminalInputBar.vue";
import { i18n, t } from "../i18n";
import type { QuickButton } from "../settings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../promptDialog", () => ({ promptText: vi.fn(async () => "") }));
vi.mock("../ai", () => ({ aiComplete: vi.fn(async () => "") }));

function button(id: string, group: string | undefined, toolbar = "Default"): QuickButton {
  return { id, label: id, command: id, toolbar, group, hosts: [], sessionGroups: [], sendMode: "line" };
}

function mountBar(quickButtons: QuickButton[]) {
  return mount(TerminalInputBar, {
    props: { quickButtons, inputHistory: [] },
    global: { plugins: [i18n] },
  });
}

function visibleLabels(wrapper: ReturnType<typeof mountBar>) {
  return wrapper.findAll(".quick-btn:not(.quick-btn--edit)").map((node) => node.text());
}

describe("TerminalInputBar groups", () => {
  it("hides the group picker while the toolbar holds a single group", () => {
    const wrapper = mountBar([button("a", "General"), button("b", undefined)]);
    expect(wrapper.find(".quick-group-select").exists()).toBe(false);
    expect(visibleLabels(wrapper)).toEqual(["a", "b"]);
  });

  it("narrows the bar to the picked group and offers every group of the toolbar", async () => {
    const wrapper = mountBar([button("a", "Docker"), button("b", "Git"), button("c", "Docker")]);
    const picker = wrapper.find<HTMLSelectElement>(".quick-group-select");
    expect(picker.findAll("option").map((option) => option.text())).toEqual([
      t("inputBar.allGroups"),
      "Docker",
      "Git",
    ]);

    await picker.setValue("Git");
    expect(visibleLabels(wrapper)).toEqual(["b"]);
    // One group on screen: its name would only repeat what the picker says.
    expect(wrapper.find(".quick-button-group-label").exists()).toBe(false);

    await picker.setValue("");
    expect(visibleLabels(wrapper)).toEqual(["a", "b", "c"]);
    expect(wrapper.findAll(".quick-button-group-label").map((node) => node.text())).toEqual(["Docker", "Git", "Docker"]);
  });

  it("falls back to all groups when the picked group leaves the toolbar", async () => {
    const wrapper = mountBar([button("a", "Docker"), button("b", "Git")]);
    await wrapper.find(".quick-group-select").setValue("Git");
    expect(visibleLabels(wrapper)).toEqual(["b"]);

    await wrapper.setProps({ quickButtons: [button("a", "Docker"), button("c", "Kube")] });
    expect(visibleLabels(wrapper)).toEqual(["a", "c"]);
  });

  it("offers the existing groups in the editor and lets a new one be typed", async () => {
    const wrapper = mountBar([button("a", "Docker"), button("b", "Git")]);
    await wrapper.find(".quick-btn--edit").trigger("click");

    const groupSelect = wrapper.find<HTMLSelectElement>(".quick-btn-editor-fields-grid select");
    expect(groupSelect.findAll("option").map((option) => option.text())).toEqual([
      "Docker",
      "Git",
      t("connect.groupNew"),
    ]);
    expect(groupSelect.element.value).toBe("Docker");

    await groupSelect.setValue("Git");
    await groupSelect.setValue("__auraterm_new_group__");
    const input = wrapper.find<HTMLInputElement>(".quick-btn-editor-input--group-name");
    await input.setValue("Kube");

    await wrapper.find(".quick-btn-editor-save").trigger("click");
    const saved = wrapper.emitted("buttonsChange")?.[0]?.[0] as QuickButton[];
    expect(saved.find((item) => item.id === "a")?.group).toBe("Kube");
    expect(saved.find((item) => item.id === "b")?.group).toBe("Git");
  });

  function sidebarLabels(wrapper: ReturnType<typeof mountBar>) {
    return wrapper.findAll(".quick-btn-editor-sidebar-label").map((node) => node.text());
  }

  it("narrows the editor's list to one group and keeps the detail pane on a listed button", async () => {
    const wrapper = mountBar([button("a", "Docker"), button("b", "Git"), button("c", "Docker", "Ops")]);
    await wrapper.find(".quick-btn--edit").trigger("click");
    expect(sidebarLabels(wrapper)).toEqual(["a", "b", "c"]);

    const groupFilter = wrapper.find<HTMLSelectElement>(".quick-btn-editor-filter--group");
    expect(groupFilter.findAll("option").map((option) => option.text())).toEqual([t("inputBar.allGroups"), "Docker", "Git"]);
    await groupFilter.setValue("Git");
    expect(sidebarLabels(wrapper)).toEqual(["b"]);
    expect(wrapper.find(".quick-btn-editor-sidebar-item.active").text()).toContain("b");

    // Narrowing to a toolbar drops groups it does not hold and resets the group pick.
    await wrapper.find(".quick-btn-editor-filter--toolbar").setValue("Ops");
    expect(groupFilter.findAll("option").map((option) => option.text())).toEqual([t("inputBar.allGroups"), "Docker"]);
    expect(sidebarLabels(wrapper)).toEqual(["c"]);
  });

  it("reorders within the filtered list without disturbing the buttons in between", async () => {
    const wrapper = mountBar([button("a", "Docker"), button("b", "Git"), button("c", "Docker")]);
    await wrapper.find(".quick-btn--edit").trigger("click");
    await wrapper.find(".quick-btn-editor-filter--group").setValue("Docker");
    expect(sidebarLabels(wrapper)).toEqual(["a", "c"]);

    const moveUp = wrapper.findAll(".quick-btn-editor-sidebar-actions button")[3];
    await moveUp.trigger("click");
    expect(sidebarLabels(wrapper)).toEqual(["c", "a"]);

    await wrapper.find(".quick-btn-editor-save").trigger("click");
    const saved = wrapper.emitted("buttonsChange")?.[0]?.[0] as QuickButton[];
    expect(saved.map((item) => item.id)).toEqual(["c", "b", "a"]);
  });

  it("adds a new button into the group and toolbar the list is narrowed to", async () => {
    const wrapper = mountBar([button("a", "Docker"), button("b", "Git", "Ops")]);
    await wrapper.find(".quick-btn--edit").trigger("click");
    await wrapper.find(".quick-btn-editor-filter--toolbar").setValue("Ops");
    await wrapper.find(".quick-btn-editor-filter--group").setValue("Git");

    await wrapper.find(".quick-btn-editor-add").trigger("click");
    expect(wrapper.findAll(".quick-btn-editor-sidebar-item")).toHaveLength(2);
    await wrapper.find(".quick-btn-editor-input--label").setValue("new");

    await wrapper.find(".quick-btn-editor-save").trigger("click");
    const saved = wrapper.emitted("buttonsChange")?.[0]?.[0] as QuickButton[];
    expect(saved[saved.length - 1]).toMatchObject({ label: "new", toolbar: "Ops", group: "Git" });
  });
});
