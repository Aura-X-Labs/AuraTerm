import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import LiveConsoleStatus from "../LiveConsoleStatus.vue";
import type { CloudBridgeStatus } from "../cloudBridge";
import { setLanguage } from "../i18n";

setLanguage("en");
const bridge: CloudBridgeStatus = { enrolled: true, connected: true, reconnecting: false, standby: false, shares: [] };
const wrappers: ReturnType<typeof mount>[] = [];
afterEach(() => { wrappers.forEach((wrapper) => wrapper.unmount()); wrappers.length = 0; });
function render(canManage = true) {
  const wrapper = mount(LiveConsoleStatus, { props: { bridge, enabled: true, allowRemoteSend: true, canManage }, global: { stubs: { teleport: true } } });
  wrappers.push(wrapper);
  return wrapper;
}

describe("Console status management", () => {
  it("opens management without changing sharing or remote input", async () => {
    const wrapper = render();
    await wrapper.get(".terminal-status-cloud").trigger("click");
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    expect(wrapper.emitted("refresh")).toHaveLength(1);
    expect(wrapper.emitted("toggleConsole")).toBeUndefined();
    expect(wrapper.emitted("toggleRemoteSend")).toBeUndefined();
    await wrapper.findAll('input[type="checkbox"]')[0]!.setValue(false);
    expect(wrapper.emitted("toggleConsole")).toHaveLength(1);
    expect(wrapper.emitted("toggleRemoteSend")).toBeUndefined();
    await wrapper.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });
  it("lets child windows inspect status with read-only switches", async () => {
    const wrapper = render(false);
    expect(wrapper.get(".terminal-status-cloud").attributes("disabled")).toBeUndefined();
    await wrapper.get(".terminal-status-cloud").trigger("click");
    expect(wrapper.findAll('input:disabled')).toHaveLength(2);
    expect(wrapper.text()).toContain("main window");
    expect(wrapper.emitted("toggleConsole")).toBeUndefined();
  });
});
