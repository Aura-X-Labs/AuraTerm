import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import PrivateKeyPicker from "../PrivateKeyPicker.vue";
import { i18n, setLanguage } from "../i18n";
import { MAX_PRIVATE_KEY_FILE_BYTES } from "../privateKeyFile";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

const OPENSSH_KEY = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\n";
const OTHER_KEY = "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\n";

type Picker = VueWrapper<InstanceType<typeof PrivateKeyPicker>>;

/** Mounted the way a parent with `v-model` drives it. */
function mountPicker(props: Record<string, unknown> = {}): Picker {
  const wrapper: Picker = mount(PrivateKeyPicker, {
    props: {
      modelValue: "",
      "onUpdate:modelValue": (value: string | undefined) => wrapper.setProps({ modelValue: value }),
      ...props,
    },
    global: { plugins: [i18n] },
  });
  return wrapper;
}

/** What the backend hands back for a pick; `content` is null past the size limit. */
interface PickedFile {
  name: string;
  content: string | null;
}

/** Browse, with the native file dialog answering `picked` (null: cancelled). */
async function pickFile(wrapper: Picker, picked: PickedFile | null) {
  tauri.invoke.mockResolvedValueOnce(picked);
  await wrapper.get("button").trigger("click");
  await flushPromises();
}

const display = (wrapper: Picker) => wrapper.get(".private-key-display");
const buttons = (wrapper: Picker) => wrapper.findAll("button").map((button) => button.text());
const modelUpdates = (wrapper: Picker) => (wrapper.emitted("update:modelValue") ?? []).map((call) => call[0]);

let error: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  setLanguage("en");
  tauri.invoke.mockReset();
  error = vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  error.mockRestore();
});

describe("PrivateKeyPicker", () => {
  it("offers a file picker and nowhere to type or paste a key", () => {
    const wrapper = mountPicker();
    expect(wrapper.find("textarea").exists()).toBe(false);
    expect(wrapper.find("input").exists()).toBe(false);
    expect(display(wrapper).text()).toBe("No private key selected");
    expect(display(wrapper).classes()).toContain("empty");
    expect(buttons(wrapper)).toEqual(["Browse..."]);
  });

  it("opens the native file dialog from Browse, capped at the key size limit", async () => {
    const wrapper = mountPicker();
    await pickFile(wrapper, null);
    expect(tauri.invoke.mock.calls).toEqual([
      ["ssh_pick_private_key_file", { maxBytes: MAX_PRIVATE_KEY_FILE_BYTES }],
    ]);
  });

  it("loads the picked file's content and shows its name", async () => {
    const wrapper = mountPicker();
    await pickFile(wrapper, { name: "id_ed25519", content: OPENSSH_KEY });

    expect(modelUpdates(wrapper)).toEqual([OPENSSH_KEY]);
    expect(display(wrapper).text()).toBe("id_ed25519");
    expect(display(wrapper).classes()).not.toContain("empty");
    expect(buttons(wrapper)).toEqual(["Browse...", "Clear"]);
    expect(wrapper.find(".private-key-error").exists()).toBe(false);
  });

  it("clears the key", async () => {
    const wrapper = mountPicker();
    await pickFile(wrapper, { name: "id_ed25519", content: OPENSSH_KEY });
    await wrapper.findAll("button")[1].trigger("click");

    expect(modelUpdates(wrapper)).toEqual([OPENSSH_KEY, ""]);
    expect(display(wrapper).text()).toBe("No private key selected");
    expect(buttons(wrapper)).toEqual(["Browse..."]);
  });

  it("shows a key that came with the saved bookmark without revealing it", () => {
    const wrapper = mountPicker({ modelValue: OPENSSH_KEY });
    expect(display(wrapper).text()).toBe("Saved private key");
    expect(wrapper.text()).not.toContain("b3BlbnNzaC1rZXk");
    expect(buttons(wrapper)).toEqual(["Browse...", "Clear"]);
  });

  it.each([
    ["a public key", { name: "id_ed25519.pub", content: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB ops@host\n" },
      "That is a public key. Choose the matching private key — usually the same file name without .pub."],
    ["a file that is not a key", { name: "notes.txt", content: "hello" },
      "That file is not a supported private key (OpenSSH, PEM or PuTTY .ppk)."],
    ["an oversized file", { name: "disk.img", content: null },
      "That file is too large to be a private key."],
  ])("refuses %s and keeps the key already in place", async (_what, file, message) => {
    const wrapper = mountPicker();
    await pickFile(wrapper, { name: "id_ed25519", content: OPENSSH_KEY });
    await pickFile(wrapper, file);

    expect(modelUpdates(wrapper)).toEqual([OPENSSH_KEY]);
    expect(display(wrapper).text()).toBe("id_ed25519");
    const alert = wrapper.get(".private-key-error");
    expect(alert.attributes("role")).toBe("alert");
    expect(alert.text()).toBe(message);

    // The next good pick replaces the key and drops the complaint.
    await pickFile(wrapper, { name: "id_rsa", content: OTHER_KEY });
    expect(modelUpdates(wrapper)).toEqual([OPENSSH_KEY, OTHER_KEY]);
    expect(display(wrapper).text()).toBe("id_rsa");
    expect(wrapper.find(".private-key-error").exists()).toBe(false);
  });

  it("reports a file it cannot read", async () => {
    const wrapper = mountPicker();
    tauri.invoke.mockRejectedValueOnce("Failed to open id_ed25519: Permission denied (os error 13)");
    await wrapper.get("button").trigger("click");
    await flushPromises();

    expect(modelUpdates(wrapper)).toEqual([]);
    expect(wrapper.get(".private-key-error").text()).toBe("Unable to read the selected private key file.");
  });

  it("does nothing when the file dialog is cancelled", async () => {
    const wrapper = mountPicker();
    await pickFile(wrapper, null);

    expect(modelUpdates(wrapper)).toEqual([]);
    expect(wrapper.find(".private-key-error").exists()).toBe(false);
  });

  it("leaves generating to the parent, and only offers it when asked to", async () => {
    expect(buttons(mountPicker())).not.toContain("Generate");

    const wrapper = mountPicker({ generatable: true });
    expect(buttons(wrapper)).toEqual(["Browse...", "Generate"]);
    await wrapper.findAll("button")[1].trigger("click");
    expect(wrapper.emitted("generate")).toHaveLength(1);
    expect(modelUpdates(wrapper)).toEqual([]);
  });

  it("shows the source the parent names, such as a generated key", async () => {
    const wrapper = mountPicker({ modelValue: OPENSSH_KEY, source: "Generated Ed25519 (SHA256:abc)" });
    expect(display(wrapper).text()).toBe("Generated Ed25519 (SHA256:abc)");

    await pickFile(wrapper, { name: "id_rsa", content: OTHER_KEY });
    expect(wrapper.emitted("update:source")).toEqual([["id_rsa"]]);
  });

  it("shows the parent's error until a pick of its own goes wrong", async () => {
    const wrapper = mountPicker({ error: "Failed to generate Ed25519 key" });
    expect(wrapper.get(".private-key-error").text()).toBe("Failed to generate Ed25519 key");

    await pickFile(wrapper, { name: "notes.txt", content: "hello" });
    expect(wrapper.get(".private-key-error").text()).toContain("not a supported private key");
  });

  it("follows the UI language, including a complaint already on screen", async () => {
    setLanguage("zh-CN");
    const wrapper = mountPicker();
    expect(display(wrapper).text()).toBe("未选择私钥");
    await pickFile(wrapper, { name: "id_rsa.pub", content: "ssh-rsa AAAAB3NzaC1yc2E ops@host\n" });
    expect(wrapper.get(".private-key-error").text()).toContain("这是公钥文件");

    setLanguage("en");
    await flushPromises();
    expect(wrapper.get(".private-key-error").text()).toContain("That is a public key.");
  });
});
