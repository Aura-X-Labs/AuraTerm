import { flushPromises, shallowMount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import AccountDialog from "../AccountDialog.vue";
import type { AuraXLabAccountState } from "../account";

const accountMocks = vi.hoisted(() => ({
  accountState: vi.fn(),
  refreshAccount: vi.fn(),
  accountLogout: vi.fn(),
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
