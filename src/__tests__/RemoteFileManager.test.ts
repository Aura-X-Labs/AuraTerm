import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import RemoteFileManager from "../RemoteFileManager.vue";
import { i18n, setLanguage } from "../i18n";
import type { RemoteTransferProgress, SshConfig } from "../types";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
  dropHandler: null as null | ((event: { payload: unknown }) => void),
  openDialog: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    tauri.listeners.set(name, handler);
    return () => tauri.listeners.delete(name);
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn(async (handler: (event: { payload: unknown }) => void) => {
      tauri.dropHandler = handler;
      return () => { tauri.dropHandler = null; };
    }),
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: tauri.openDialog }));
vi.mock("../nativeDialogs", () => ({ confirmDialog: vi.fn(async () => true) }));
vi.mock("../promptDialog", () => ({ promptText: vi.fn(async () => null) }));

const sshConfig = { host: "example.com", port: 22, user: "alice" } as SshConfig;
const listing = { path: "/home/alice", parent: "/home", entries: [] };

/** `ssh_upload_file` calls stay pending until the test settles them. */
type Pending = { args: Record<string, unknown>; resolve: () => void; reject: (error: unknown) => void };

function mountManager() {
  const uploads: Pending[] = [];
  tauri.invoke.mockImplementation((command: string, args: Record<string, unknown>) => {
    if (command === "ssh_list_remote_dir") return Promise.resolve(listing);
    if (command === "ssh_upload_file") {
      return new Promise<void>((resolve, reject) => uploads.push({ args, resolve, reject }));
    }
    return Promise.resolve();
  });
  const wrapper = mount(RemoteFileManager, {
    props: { sessionId: "tab-1", sshConfig },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
  return { wrapper, uploads };
}

function uploadButton(wrapper: ReturnType<typeof mount>) {
  return wrapper.get(`button[title="Upload"]`);
}

beforeEach(() => {
  setLanguage("en");
  tauri.invoke.mockReset();
  tauri.openDialog.mockReset();
  tauri.listeners.clear();
  tauri.dropHandler = null;
});

describe("RemoteFileManager uploads", () => {
  it("hands the backend local paths to stream, never the file bytes", async () => {
    const { wrapper, uploads } = mountManager();
    await flushPromises();
    tauri.openDialog.mockResolvedValueOnce(["/home/me/big.iso", "C:\\Users\\me\\notes.txt"]);

    await uploadButton(wrapper).trigger("click");
    await flushPromises();

    expect(uploads).toHaveLength(1);
    expect(uploads[0].args).toMatchObject({
      id: "tab-1",
      remoteDir: "/home/alice",
      localPath: "/home/me/big.iso",
      mode: "sftp",
    });
    expect(uploads[0].args).not.toHaveProperty("data");
    expect(typeof uploads[0].args.transferId).toBe("string");

    uploads[0].resolve();
    await flushPromises();
    expect(uploads).toHaveLength(2);
    expect(uploads[1].args.localPath).toBe("C:\\Users\\me\\notes.txt");
    expect(uploads[1].args.transferId).not.toBe(uploads[0].args.transferId);
    uploads[1].resolve();
    await flushPromises();
    wrapper.unmount();
  });

  it("cancels the running upload and the rest of the queue", async () => {
    const { wrapper, uploads } = mountManager();
    await flushPromises();
    tauri.openDialog.mockResolvedValueOnce(["/a/one.bin", "/a/two.bin"]);
    await uploadButton(wrapper).trigger("click");
    await flushPromises();

    const transferId = uploads[0].args.transferId as string;
    const progress: RemoteTransferProgress = {
      id: "tab-1",
      transferId,
      direction: "upload",
      status: "progress",
      mode: "sftp",
      fileName: "one.bin",
      remotePath: "/home/alice/one.bin",
      localPath: "/a/one.bin",
      transferredBytes: 512,
      totalBytes: 1024,
    };
    tauri.listeners.get("ssh-transfer-progress")?.({ payload: progress });
    await flushPromises();

    const cancel = wrapper.get(".remote-file-manager-transfer-cancel");
    await cancel.trigger("click");
    expect(tauri.invoke).toHaveBeenCalledWith("ssh_cancel_transfer", { transferId });

    uploads[0].reject("Transfer cancelled");
    await flushPromises();

    expect(uploads).toHaveLength(1);
    expect(wrapper.get(".remote-file-manager-transfer").classes()).toContain("cancelled");
    expect(wrapper.findAll(".remote-file-manager-queue-item.cancelled")).toHaveLength(2);
    expect(wrapper.find(".remote-file-manager-transfer-cancel").exists()).toBe(false);
    wrapper.unmount();
  });

  it("shows a failed upload as failed", async () => {
    const { wrapper, uploads } = mountManager();
    await flushPromises();
    tauri.openDialog.mockResolvedValueOnce("/a/one.bin");
    await uploadButton(wrapper).trigger("click");
    await flushPromises();

    uploads[0].reject("Failed to upload file over SFTP: Permission denied");
    await flushPromises();

    expect(wrapper.get(".remote-file-manager-transfer").classes()).toContain("failed");
    expect(wrapper.text()).toContain("Permission denied");
    wrapper.unmount();
  });

  it("uploads paths dropped onto the file list", async () => {
    const { wrapper, uploads } = mountManager();
    await flushPromises();
    const list = wrapper.get(".remote-file-manager-list").element as HTMLElement;
    list.getBoundingClientRect = () => ({ left: 0, top: 0, right: 200, bottom: 200, width: 200, height: 200, x: 0, y: 0, toJSON: () => ({}) });

    tauri.dropHandler?.({ payload: { type: "drop", paths: ["/dropped/file.txt"], position: { x: 900, y: 900 } } });
    await flushPromises();
    expect(uploads).toHaveLength(0);

    tauri.dropHandler?.({ payload: { type: "drop", paths: ["/dropped/file.txt"], position: { x: 50, y: 50 } } });
    await flushPromises();
    expect(uploads).toHaveLength(1);
    expect(uploads[0].args.localPath).toBe("/dropped/file.txt");
    uploads[0].resolve();
    await flushPromises();
    wrapper.unmount();
  });
});
