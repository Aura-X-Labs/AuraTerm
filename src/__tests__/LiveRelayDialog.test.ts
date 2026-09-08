import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import LiveRelayDialog from "../LiveRelayDialog.vue";

const mocks = vi.hoisted(() => ({ relayListDevices: vi.fn() }));
vi.mock("../liveRelay", async (importOriginal) => ({
  ...await importOriginal<typeof import("../liveRelay")>(), ...mocks,
}));

describe("LiveRelayDialog login return", () => {
  it("loads devices when a retained dialog becomes enrolled after login", async () => {
    mocks.relayListDevices.mockResolvedValue([]);
    const wrapper = mount(LiveRelayDialog, { props: { enrolled: false, status: { enabled: false, peers: [] } } });
    expect(mocks.relayListDevices).not.toHaveBeenCalled();
    await wrapper.setProps({ enrolled: true });
    await flushPromises();
    expect(mocks.relayListDevices).toHaveBeenCalledOnce();
    expect(wrapper.find(".relay-error").exists()).toBe(false);
    wrapper.unmount();
  });

  it("discards a pending device lookup if association is lost", async () => {
    let resolve!: (devices: unknown[]) => void;
    mocks.relayListDevices.mockReturnValue(new Promise((done) => { resolve = done; }));
    const wrapper = mount(LiveRelayDialog, { props: { enrolled: true, status: { enabled: false, peers: [] } } });
    await wrapper.setProps({ enrolled: false });
    resolve([{ device_id: "stale", label: "Stale device" }]);
    await flushPromises();
    mocks.relayListDevices.mockResolvedValueOnce([]);
    await wrapper.setProps({ enrolled: true });
    await flushPromises();
    expect(wrapper.text()).not.toContain("Stale device");
    wrapper.unmount();
  });
});
