import { flushPromises, mount, type DOMWrapper, type VueWrapper } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import BookmarkEditor from "../BookmarkEditor.vue";
import { i18n, setLanguage } from "../i18n";
import type { SavedConnection } from "../types";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

const SAVED_KEY = "-----BEGIN OPENSSH PRIVATE KEY-----\nc2F2ZWQ\n-----END OPENSSH PRIVATE KEY-----\n";
const PICKED_KEY = "-----BEGIN OPENSSH PRIVATE KEY-----\ncGlja2Vk\n-----END OPENSSH PRIVATE KEY-----\n";
const GENERATED_KEY = {
  privateKey: "-----BEGIN OPENSSH PRIVATE KEY-----\nZ2VuZXJhdGVk\n-----END OPENSSH PRIVATE KEY-----\n",
  publicKey: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGenerated ops@10.0.0.5",
  fingerprint: "SHA256:generated",
};

function connection(overrides: Partial<SavedConnection> = {}): SavedConnection {
  return {
    id: "web-1",
    name: "web-1",
    protocol: "ssh",
    host: "10.0.0.5",
    port: 22,
    user: "ops",
    authType: "key",
    createdAt: 1,
    ...overrides,
  };
}

type Editor = VueWrapper<InstanceType<typeof BookmarkEditor>>;

function mountEditor(saved: SavedConnection): Editor {
  return mount(BookmarkEditor, {
    props: { connection: saved },
    global: { plugins: [i18n] },
  });
}

/** What the browser does once the user confirms the native file dialog. */
async function pickKeyFile(
  scope: { get(selector: string): Omit<DOMWrapper<Element>, "exists"> },
  content: string,
  name: string,
) {
  const input = scope.get("input[type='file']");
  Object.defineProperty(input.element, "files", { value: [new File([content], name)], configurable: true });
  await input.trigger("change");
  await flushPromises();
}

async function save(wrapper: Editor): Promise<SavedConnection> {
  await wrapper.get(".bookmark-editor-btn.primary").trigger("click");
  const saves = wrapper.emitted("save");
  expect(saves).toHaveLength(1);
  return saves![0][0] as SavedConnection;
}

beforeEach(() => {
  setLanguage("en");
  tauri.invoke.mockImplementation(async (command: string) => {
    if (command === "ssh_generate_key_pair") {
      return GENERATED_KEY;
    }
    throw new Error(`unexpected invoke: ${command}`);
  });
});

describe("BookmarkEditor private keys", () => {
  it("shows a saved key as saved, without revealing it or offering a box to paste into", async () => {
    const wrapper = mountEditor(connection({ privateKey: SAVED_KEY }));

    expect(wrapper.get(".private-key-display").text()).toBe("Saved private key");
    expect(wrapper.html()).not.toContain("c2F2ZWQ");
    // Only the post-connect commands box is left as a textarea.
    expect(wrapper.findAll("textarea")).toHaveLength(1);

    // Saving untouched keeps the key.
    expect((await save(wrapper)).privateKey).toBe(SAVED_KEY);
  });

  it("replaces the key with one picked from a local file", async () => {
    const wrapper = mountEditor(connection({ privateKey: SAVED_KEY }));
    await pickKeyFile(wrapper, PICKED_KEY, "id_ed25519");

    expect(wrapper.get(".private-key-display").text()).toBe("id_ed25519");
    expect((await save(wrapper)).privateKey).toBe(PICKED_KEY);
  });

  it("keeps the saved key when the picked file is not a private key", async () => {
    const wrapper = mountEditor(connection({ privateKey: SAVED_KEY }));
    await pickKeyFile(wrapper, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB ops@host", "id_ed25519.pub");

    expect(wrapper.get(".private-key-error").text()).toContain("That is a public key.");
    expect(wrapper.get(".private-key-display").text()).toBe("Saved private key");
    expect((await save(wrapper)).privateKey).toBe(SAVED_KEY);
  });

  it("clears the key", async () => {
    const wrapper = mountEditor(connection({ privateKey: SAVED_KEY }));
    const clear = wrapper.findAll(".private-key-picker-btn").find((button) => button.text() === "Clear");
    await clear!.trigger("click");

    expect(wrapper.get(".private-key-display").text()).toBe("No private key selected");
    expect((await save(wrapper)).privateKey).toBe("");
  });

  it("generates a key, and shows its public half only while that key is in the draft", async () => {
    const wrapper = mountEditor(connection());
    expect(wrapper.get(".private-key-display").text()).toBe("No private key selected");

    const generate = wrapper.findAll(".private-key-picker-btn").find((button) => button.text() === "Generate");
    await generate!.trigger("click");
    await flushPromises();
    expect(tauri.invoke).toHaveBeenCalledWith("ssh_generate_key_pair", { passphrase: null, comment: "ops@10.0.0.5" });
    expect(wrapper.get(".private-key-display").text()).toBe("Generated Ed25519 (SHA256:generated)");
    expect((wrapper.get(".generated-key-row textarea").element as HTMLTextAreaElement).value)
      .toBe(GENERATED_KEY.publicKey);

    await pickKeyFile(wrapper, PICKED_KEY, "id_ed25519");
    expect(wrapper.find(".generated-key-row").exists()).toBe(false);
    expect((await save(wrapper)).privateKey).toBe(PICKED_KEY);
  });

  it("shows a failed generation beside the picker, until the key changes", async () => {
    tauri.invoke.mockRejectedValue("Failed to generate Ed25519 key: rng");
    const wrapper = mountEditor(connection());
    const generate = wrapper.findAll(".private-key-picker-btn").find((button) => button.text() === "Generate");
    await generate!.trigger("click");
    await flushPromises();
    expect(wrapper.get(".private-key-error").text()).toBe("Failed to generate Ed25519 key: rng");

    await pickKeyFile(wrapper, PICKED_KEY, "id_ed25519");
    expect(wrapper.find(".private-key-error").exists()).toBe(false);
  });

  it("takes a jump host's key from a local file too", async () => {
    const wrapper = mountEditor(connection({
      authType: "agent",
      jumpHosts: [{ id: "jump-1", host: "bastion", port: 22, user: "jump", authType: "key", privateKey: SAVED_KEY }],
    }));
    const card = wrapper.get(".bookmark-advanced-card");
    expect(card.find("textarea").exists()).toBe(false);
    expect(card.get(".private-key-display").text()).toBe("Saved private key");

    await pickKeyFile(card, PICKED_KEY, "bastion_key");
    expect(card.get(".private-key-display").text()).toBe("bastion_key");

    const saved = await save(wrapper);
    expect(saved.jumpHosts).toEqual([
      { id: "jump-1", host: "bastion", port: 22, user: "jump", authType: "key", privateKey: PICKED_KEY },
    ]);
    // The caller's bookmark is not edited in place.
    expect(wrapper.props("connection").jumpHosts![0].privateKey).toBe(SAVED_KEY);
  });
});
