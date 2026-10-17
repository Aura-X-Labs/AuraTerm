import { flushPromises, shallowMount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import AccountDialog from "../AccountDialog.vue";
import { setLanguage } from "../i18n";
import AuraxlabAuthForm from "../AuraxlabAuthForm.vue";
import type { AuraXLabAccountState } from "../account";

const accountMocks = vi.hoisted(() => ({
  accountState: vi.fn(),
  refreshAccount: vi.fn(),
  accountLogout: vi.fn(),
  deleteAccount: vi.fn(),
  enableConsole: vi.fn(),
  pauseConsole: vi.fn(),
}));

vi.mock("../account", async (importOriginal) => {
  const original = await importOriginal<typeof import("../account")>();
  return { ...original, ...accountMocks };
});

vi.mock("@tauri-apps/plugin-shell", () => ({
  open: vi.fn(),
}));

const dialogMocks = vi.hoisted(() => ({
  confirmDialog: vi.fn(),
}));

vi.mock("../nativeDialogs", () => dialogMocks);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

const signedInState: AuraXLabAccountState = {
  signedIn: true,
  accountSubject: "acc_test",
  email: "bill@example.com",
  username: "bill",
  confirmed: true,
  syncCredentialSet: true,
  consistency: "sync_only",
  console: {
    enrolled: false,
    connected: false,
    deviceId: null,
    deviceLabel: null,
  },
  traffic: null,
};

describe("AccountDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setLanguage("zh-CN");
  });

  it("does not render the sign-in form while the saved account is loading", async () => {
    const localState = deferred<AuraXLabAccountState>();
    const remoteState = deferred<AuraXLabAccountState>();
    accountMocks.accountState.mockReturnValue(localState.promise);
    accountMocks.refreshAccount.mockReturnValue(remoteState.promise);

    const wrapper = shallowMount(AccountDialog, {
      props: { platform: "windows" },
    });

    expect(wrapper.find("auraxlab-auth-form-stub").exists()).toBe(false);

    localState.resolve(signedInState);
    await flushPromises();

    expect(wrapper.text()).toContain("bill");
    expect(wrapper.find("auraxlab-auth-form-stub").exists()).toBe(false);
    expect(accountMocks.refreshAccount).toHaveBeenCalledOnce();

    wrapper.unmount();
  });
});


const readyState: AuraXLabAccountState = {
  ...signedInState, consistency: "consistent",
  console: { enrolled: true, connected: false, deviceId: "device", deviceLabel: "Windows" },
};
const signedOutState: AuraXLabAccountState = {
  ...signedInState, signedIn: false, consistency: "signed_out", syncCredentialSet: false,
};

describe("AccountDialog Live return", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setLanguage("zh-CN");
    accountMocks.accountState.mockResolvedValue(signedOutState);
    accountMocks.refreshAccount.mockResolvedValue(readyState);
  });

  it("returns only after both sign-in and device association succeed", async () => {
    const wrapper = shallowMount(AccountDialog, { props: { platform: "windows", returnOnReady: true } });
    await flushPromises();
    wrapper.findComponent(AuraxlabAuthForm).vm.$emit("signed-in", signedInState);
    await flushPromises();
    expect(wrapper.emitted("ready")).toBeUndefined();
    accountMocks.enableConsole.mockResolvedValue(readyState);
    await wrapper.find('.account-recovery input').setValue("password");
    await wrapper.find('.account-recovery button').trigger("click");
    await flushPromises();
    expect(wrapper.emitted("ready")).toHaveLength(1);
    wrapper.unmount();
  });

  it.each([true, false])("auto-returns after complete login only when a Live action requested it (%s)", async (returnOnReady) => {
    const wrapper = shallowMount(AccountDialog, { props: { platform: "windows", returnOnReady } });
    await flushPromises();
    wrapper.findComponent(AuraxlabAuthForm).vm.$emit("signed-in", readyState);
    await flushPromises();
    expect(wrapper.emitted("ready")?.length ?? 0).toBe(returnOnReady ? 1 : 0);
    wrapper.unmount();
  });

  it("returns to sync after login without requiring device association", async () => {
    const wrapper = shallowMount(AccountDialog, {
      props: { platform: "windows", returnOnReady: true, requireDevice: false },
    });
    await flushPromises();
    wrapper.findComponent(AuraxlabAuthForm).vm.$emit("signed-in", signedInState);
    await flushPromises();
    expect(wrapper.emitted("ready")).toHaveLength(1);
    wrapper.unmount();
  });

  it("uses the same standby and reconnecting status as the status bar", async () => {
    accountMocks.accountState.mockResolvedValue(readyState);
    const bridge = { enrolled: true, connected: false, reconnecting: false, standby: true, deviceId: "device", shares: [] };
    const wrapper = shallowMount(AccountDialog, { props: { platform: "windows", bridgeStatus: bridge } });
    await flushPromises();
    expect(wrapper.text()).toContain("在线（空闲）");
    expect(wrapper.text()).not.toContain("离线");
    await wrapper.setProps({ bridgeStatus: { ...bridge, standby: false, reconnecting: true } });
    expect(wrapper.text()).toContain("重连中");
    wrapper.unmount();
  });
});

describe("AccountDialog account deletion", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setLanguage("zh-CN");
    accountMocks.accountState.mockResolvedValue(readyState);
    accountMocks.refreshAccount.mockResolvedValue(readyState);
  });

  async function openDeletePanel() {
    const wrapper = shallowMount(AccountDialog, { props: { platform: "darwin" } });
    await flushPromises();
    const toggle = wrapper.findAll("button").find((button) => button.text() === "删除账户…");
    expect(toggle).toBeDefined();
    await toggle!.trigger("click");
    return wrapper;
  }

  function deleteButton(wrapper: Awaited<ReturnType<typeof openDeletePanel>>) {
    return wrapper.find(".account-delete button.danger");
  }

  it("confirms natively, sends the password, and shows the signed-out state", async () => {
    dialogMocks.confirmDialog.mockResolvedValue(true);
    accountMocks.deleteAccount.mockResolvedValue(signedOutState);
    const wrapper = await openDeletePanel();

    expect(deleteButton(wrapper).attributes("disabled")).toBeDefined();
    await wrapper.find(".account-delete input").setValue("hunter22");
    await deleteButton(wrapper).trigger("click");
    await flushPromises();

    expect(dialogMocks.confirmDialog).toHaveBeenCalledWith(expect.stringContaining("bill"), "warning");
    expect(accountMocks.deleteAccount).toHaveBeenCalledWith("hunter22");
    expect(wrapper.text()).toContain("账户已删除，此设备已退出登录。");
    expect(wrapper.find(".account-delete").exists()).toBe(false);
    expect(wrapper.find("auraxlab-auth-form-stub").exists()).toBe(true);
    expect(wrapper.emitted("changed")).toHaveLength(1);
    wrapper.unmount();
  });

  it("sends nothing when the confirmation is declined", async () => {
    dialogMocks.confirmDialog.mockResolvedValue(false);
    const wrapper = await openDeletePanel();
    await wrapper.find(".account-delete input").setValue("hunter22");
    await deleteButton(wrapper).trigger("click");
    await flushPromises();

    expect(dialogMocks.confirmDialog).toHaveBeenCalledOnce();
    expect(accountMocks.deleteAccount).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("bill");
    wrapper.unmount();
  });

  it("keeps the account and clears the password when the server refuses", async () => {
    dialogMocks.confirmDialog.mockResolvedValue(true);
    accountMocks.deleteAccount.mockRejectedValue("Account deletion failed: Incorrect password");
    const wrapper = await openDeletePanel();
    await wrapper.find(".account-delete input").setValue("wrong");
    await deleteButton(wrapper).trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Incorrect password");
    expect(wrapper.find(".account-delete").exists()).toBe(true);
    expect((wrapper.find(".account-delete input").element as HTMLInputElement).value).toBe("");
    expect(wrapper.emitted("changed")).toBeUndefined();
    wrapper.unmount();
  });
});
