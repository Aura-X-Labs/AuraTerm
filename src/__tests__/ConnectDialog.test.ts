import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ConnectDialog from "../ConnectDialog.vue";
import type { ConnectionTestReport } from "../connectionTest";
import { i18n, setLanguage } from "../i18n";
import type { ConnectionProtocol } from "../types";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

const TEST_COMMANDS = new Set(["ssh_test_connection", "telnet_test_connection", "serial_test_connection"]);

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function report(overrides: Partial<ConnectionTestReport> = {}): ConnectionTestReport {
  return {
    status: "success",
    code: "ok",
    detail: null,
    elapsedMs: 412,
    failedHop: null,
    hostKeys: [],
    authMethods: [],
    serverParams: null,
    signature: null,
    ...overrides,
  };
}

/** Pending backend probes, oldest first. */
let pending: Deferred<ConnectionTestReport>[] = [];
/** What `list_serial_ports` answers with. */
let serialPorts: Array<{ portName: string; portType: string }> = [];

function commandsInvoked(): string[] {
  return tauri.invoke.mock.calls.map((call) => call[0] as string);
}

function testCalls() {
  return tauri.invoke.mock.calls.filter((call) => TEST_COMMANDS.has(call[0] as string));
}

function mountDialog(initialProtocol: ConnectionProtocol = "ssh") {
  return mount(ConnectDialog, {
    props: { initialProtocol },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
}

type Dialog = VueWrapper<InstanceType<typeof ConnectDialog>>;

const testButton = (wrapper: Dialog) => wrapper.get("button.btn-test");
const resultRegion = (wrapper: Dialog) => wrapper.get("#connect-test-result");
const hostInput = (wrapper: Dialog) => wrapper.get("input[autofocus]");
const userInput = (wrapper: Dialog) => wrapper.get(".two-column-grid .form-group:nth-child(2) input");

async function fillSsh(wrapper: Dialog, host = "10.0.0.5", user = "ops") {
  await hostInput(wrapper).setValue(host);
  await userInput(wrapper).setValue(user);
}

let warn: ReturnType<typeof vi.spyOn>;
let error: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  setLanguage("en");
  pending = [];
  serialPorts = [];
  tauri.invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case "get_connections":
        return [];
      case "list_serial_ports":
        return serialPorts;
      case "ssh_test_connection":
      case "telnet_test_connection":
      case "serial_test_connection": {
        const probe = deferred<ConnectionTestReport>();
        pending.push(probe);
        return probe.promise;
      }
      default:
        throw new Error(`unexpected invoke: ${command}`);
    }
  });
  warn = vi.spyOn(console, "warn");
  error = vi.spyOn(console, "error");
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ConnectDialog test button", () => {
  it("is a plain button, disabled until the SSH form is complete", async () => {
    const wrapper = mountDialog();
    await flushPromises();

    const button = testButton(wrapper);
    expect(button.attributes("type")).toBe("button");
    expect(button.text()).toBe("Test");
    expect(button.attributes("title")).toBe("Try the connection without opening a session or saving anything");
    expect(button.attributes("aria-describedby")).toBe("connect-test-result");
    expect(button.element.hasAttribute("disabled")).toBe(true);

    await hostInput(wrapper).setValue("10.0.0.5");
    expect(button.element.hasAttribute("disabled")).toBe(true);

    await userInput(wrapper).setValue("ops");
    expect(button.element.hasAttribute("disabled")).toBe(false);

    // Whitespace alone does not count.
    await hostInput(wrapper).setValue("   ");
    expect(button.element.hasAttribute("disabled")).toBe(true);
    wrapper.unmount();
  });

  it("stays enabled with an empty password, and sends null for it", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(false);

    await testButton(wrapper).trigger("click");
    expect(testCalls()).toEqual([["ssh_test_connection", {
      host: "10.0.0.5",
      port: 22,
      user: "ops",
      password: null,
      privateKey: null,
      passphrase: null,
      authType: "password",
      jumpHosts: [],
    }]]);
    pending[0].resolve(report({ status: "warning", code: "passwordMissing" }));
    await flushPromises();
    wrapper.unmount();
  });

  it("is disabled in key mode until a private key is chosen", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await wrapper.get(".auth-type-group select").setValue("key");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);
    wrapper.unmount();
  });

  it("is disabled while a jump host is missing its host or user", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    const addJump = wrapper.findAll(".ssh-advanced-heading button")[0];
    await addJump.trigger("click");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);

    const card = wrapper.get(".ssh-advanced-card");
    const [jumpHost, , jumpUser] = card.findAll(".ssh-card-grid input");
    await jumpHost.setValue(" bastion ");
    await jumpUser.setValue("jump");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(false);

    await testButton(wrapper).trigger("click");
    const args = testCalls()[0][1] as { jumpHosts: Array<Record<string, unknown>> };
    // Trimmed exactly as Connect trims them.
    expect(args.jumpHosts).toHaveLength(1);
    expect(args.jumpHosts[0]).toMatchObject({ host: "bastion", user: "jump", port: 22, authType: "agent" });
    pending[0].resolve(report());
    await flushPromises();
    wrapper.unmount();
  });

  it("runs one probe at a time and says it is running", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await wrapper.get("input[type='password']").setValue("hunter2");

    await testButton(wrapper).trigger("click");
    const button = testButton(wrapper);
    expect(button.text()).toBe("Testing…");
    expect(button.element.hasAttribute("disabled")).toBe(true);
    expect(button.attributes("aria-busy")).toBe("true");
    expect(resultRegion(wrapper).text()).toBe(
      "Testing the connection… nothing is saved and no session is opened.",
    );

    // A second click (or a synthetic one on the disabled button) starts nothing new.
    await button.trigger("click");
    (button.element as HTMLButtonElement).click();
    await flushPromises();
    expect(testCalls()).toHaveLength(1);
    expect(testCalls()[0][1]).toMatchObject({ password: "hunter2" });

    pending[0].resolve(report());
    await flushPromises();
    expect(testButton(wrapper).text()).toBe("Test");
    expect(testButton(wrapper).attributes("aria-busy")).toBe("false");
    wrapper.unmount();
  });

  it("shows the localized result and never connects, saves or opens anything", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await wrapper.get("input[type='password']").setValue("hunter2");

    await testButton(wrapper).trigger("click");
    pending[0].resolve(report({
      hostKeys: [{ label: "10.0.0.5:22", status: "new", fingerprint: "SHA256:abc" }],
    }));
    await flushPromises();

    const region = resultRegion(wrapper);
    expect(region.attributes("role")).toBe("status");
    expect(region.attributes("aria-live")).toBe("polite");
    const title = region.get(".connect-test-title");
    expect(title.text()).toBe("Connected and authenticated. No session was opened. (412 ms)");
    expect(title.classes()).toContain("success");
    const lines = region.findAll(".connect-test-line");
    expect(lines).toHaveLength(1);
    expect(lines[0].classes()).toContain("warning");
    expect(lines[0].text()).toContain("SHA256:abc");

    expect(wrapper.emitted("connect")).toBeUndefined();
    expect(wrapper.emitted("cancel")).toBeUndefined();
    expect(new Set(commandsInvoked())).toEqual(new Set(["get_connections", "ssh_test_connection"]));
    wrapper.unmount();
  });

  it("colours a failure as an error", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report({ status: "failure", code: "refused", detail: "10.0.0.5:22: refused" }));
    await flushPromises();

    const title = resultRegion(wrapper).get(".connect-test-title");
    expect(title.classes()).toContain("error");
    expect(title.text()).toContain("Connection refused");
    expect(resultRegion(wrapper).get(".connect-test-line").classes()).toContain("muted");
    wrapper.unmount();
  });

  it("shows where a jump host chain stopped, in the result's tone, and what it never reached", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report({
      status: "warning",
      code: "passwordMissing",
      failedHop: { index: 1, label: "jump@bastion:22" },
      untestedHops: ["ops@10.0.0.5:22"],
      hostKeys: [{ label: "bastion:22", status: "trusted", fingerprint: "SHA256:a" }],
    }));
    await flushPromises();

    const region = resultRegion(wrapper);
    expect(region.get(".connect-test-title").classes()).toContain("warning");
    const lines = region.findAll(".connect-test-line");
    expect(lines.map((line) => line.text())).toEqual([
      "At jump host 1 (jump@bastion:22).",
      "Not reached, so not tested: ops@10.0.0.5:22.",
      "Host key for bastion:22 is trusted.",
    ]);
    // The jump host line is a warning here, not a hard-coded error.
    expect(lines[0].classes()).toContain("warning");
    expect(lines[0].classes()).not.toContain("error");
    expect(lines[1].classes()).toContain("muted");
    expect(lines[2].classes()).toContain("success");
    wrapper.unmount();
  });

  it("shows a rejected invoke as a failure instead of leaving the button spinning", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    pending[0].reject("Unsupported SSH auth type: bogus");
    await flushPromises();

    expect(testButton(wrapper).text()).toBe("Test");
    expect(resultRegion(wrapper).get(".connect-test-title").text()).toContain("The test failed.");
    expect(resultRegion(wrapper).text()).toContain("Unsupported SSH auth type: bogus");
    wrapper.unmount();
  });

  it("follows the UI language", async () => {
    setLanguage("zh-CN");
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    expect(testButton(wrapper).text()).toBe("测试");
    await testButton(wrapper).trigger("click");
    expect(testButton(wrapper).text()).toBe("测试中…");
    pending[0].resolve(report({ status: "failure", code: "authFailed" }));
    await flushPromises();
    expect(resultRegion(wrapper).get(".connect-test-title").text()).toBe(
      "认证失败：服务器拒绝了这些凭据。（耗时 412 ms）",
    );

    // A result already on screen is re-translated, like the rest of the dialog.
    setLanguage("en");
    await flushPromises();
    expect(resultRegion(wrapper).get(".connect-test-title").text()).toBe(
      "Authentication failed: the server rejected these credentials. (412 ms)",
    );
    wrapper.unmount();
  });

  it("clears the result when a connection field changes", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);

    await hostInput(wrapper).setValue("10.0.0.6");
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    expect(resultRegion(wrapper).text()).toBe("");
    // The CSS hides the (always mounted) live region with `:empty`.
    expect(resultRegion(wrapper).element.matches(":empty")).toBe(true);
    wrapper.unmount();
  });

  it("keeps an empty live region mounted before any test", async () => {
    const wrapper = mountDialog();
    await flushPromises();
    const region = resultRegion(wrapper);
    expect(region.attributes("role")).toBe("status");
    expect(region.element.matches(":empty")).toBe(true);
    wrapper.unmount();
  });

  it.each([
    ["port", (wrapper: Dialog) => wrapper.get(".two-column-grid input[type='number']").setValue("2222")],
    ["password", (wrapper: Dialog) => wrapper.get("input[type='password']").setValue("x")],
    ["auth type", (wrapper: Dialog) => wrapper.get(".auth-type-group select").setValue("agent")],
    ["protocol", (wrapper: Dialog) => wrapper.get("input[value='telnet']").setValue()],
    ["jump host list", (wrapper: Dialog) => wrapper.findAll(".ssh-advanced-heading button")[0].trigger("click")],
  ])("clears the result when the %s changes", async (_field, change) => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);

    await change(wrapper);
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    wrapper.unmount();
  });

  it("clears the result when a jump host field is edited in place", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await wrapper.findAll(".ssh-advanced-heading button")[0].trigger("click");
    const [jumpHost, , jumpUser] = wrapper.get(".ssh-advanced-card").findAll(".ssh-card-grid input");
    await jumpHost.setValue("bastion");
    await jumpUser.setValue("jump");
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);

    await jumpHost.setValue("bastion-2");
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    wrapper.unmount();
  });

  it("keeps the result when only a save or log option changes", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report());
    await flushPromises();

    await wrapper.get("input.save-connection-name").setValue("my prod box");
    await wrapper.get(".ssh-checkbox input[type='checkbox']").setValue(true);
    await wrapper.get(".save-connection-label input[type='checkbox']").setValue(false);
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);
    wrapper.unmount();
  });

  it("drops a result that arrives after the form changed, and frees the button", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");

    await hostInput(wrapper).setValue("10.0.0.6");
    // Still running: only one probe at a time.
    expect(testButton(wrapper).text()).toBe("Testing…");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);

    pending[0].resolve(report({ status: "failure", code: "refused" }));
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    expect(testButton(wrapper).text()).toBe("Test");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(false);

    // A fresh run tests the new value.
    await testButton(wrapper).trigger("click");
    expect(testCalls()[1][1]).toMatchObject({ host: "10.0.0.6" });
    pending[1].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);
    wrapper.unmount();
  });

  it("survives being closed mid-test without errors or warnings", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    await wrapper.get("button.btn-cancel").trigger("click");
    expect(wrapper.emitted("cancel")).toHaveLength(1);

    wrapper.unmount();
    pending[0].resolve(report());
    await flushPromises();
    // Also a late rejection.
    const second = mountDialog();
    await fillSsh(second);
    await testButton(second).trigger("click");
    second.unmount();
    pending[1].reject(new Error("backend went away"));
    await flushPromises();

    expect(warn).not.toHaveBeenCalled();
    expect(error).not.toHaveBeenCalled();
  });

  it("submitting the form connects rather than testing", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await wrapper.get("form").trigger("submit");

    const connects = wrapper.emitted("connect");
    expect(connects).toHaveLength(1);
    expect(connects![0][0]).toMatchObject({ protocol: "ssh", sshConfig: { host: "10.0.0.5", user: "ops" } });
    expect(testCalls()).toHaveLength(0);
    wrapper.unmount();
  });

  it("connecting while a test runs discards the test's result", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper);
    await testButton(wrapper).trigger("click");
    await wrapper.get("form").trigger("submit");
    expect(wrapper.emitted("connect")).toHaveLength(1);

    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    wrapper.unmount();
  });

  it("tests the connection Connect would open, field for field", async () => {
    const wrapper = mountDialog();
    await fillSsh(wrapper, "  10.0.0.5  ", "ops");
    await wrapper.get(".two-column-grid input[type='number']").setValue("2222");
    await wrapper.get("input[type='password']").setValue("hunter2");

    await testButton(wrapper).trigger("click");
    await wrapper.get("form").trigger("submit");
    const connected = wrapper.emitted("connect")![0][0] as { sshConfig: Record<string, unknown> };
    const tested = testCalls()[0][1] as Record<string, unknown>;
    for (const field of ["host", "port", "user", "password"]) {
      expect(tested[field], field).toEqual(connected.sshConfig[field]);
    }
    // The host is not trimmed for either: known_hosts scopes use it verbatim.
    expect(tested.host).toBe("  10.0.0.5  ");
    pending[0].resolve(report());
    await flushPromises();
    wrapper.unmount();
  });

  it("tests telnet with the telnet command", async () => {
    const wrapper = mountDialog("telnet");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);
    await hostInput(wrapper).setValue("switch-1");
    await wrapper.get("input[type='number']").setValue("2323");

    await testButton(wrapper).trigger("click");
    expect(testCalls()).toEqual([["telnet_test_connection", { host: "switch-1", port: 2323 }]]);
    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).get(".connect-test-title").text()).toContain(
      "The port is reachable. Telnet has no login step that can be tested.",
    );
    wrapper.unmount();
  });

  it("tests RFC 2217 in adopt mode whatever the checkbox says", async () => {
    const wrapper = mountDialog("rfc2217");
    await flushPromises();
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);

    await wrapper.get(".serial-adopt-toggle input[type='checkbox']").setValue(false);
    const [hostField, portField] = wrapper.findAll(".serial-settings-grid input:not([type='checkbox'])");
    await hostField.setValue(" 10.0.0.9 ");
    await portField.setValue("4001");

    await testButton(wrapper).trigger("click");
    expect(testCalls()).toHaveLength(1);
    const [command, args] = testCalls()[0];
    expect(command).toBe("serial_test_connection");
    expect(args).toMatchObject({ transport: "rfc2217", host: "10.0.0.9", netPort: 4001, portName: "10.0.0.9:4001" });
    expect(args).not.toHaveProperty("adoptServerParams");
    expect(args).not.toHaveProperty("autoReconnect");
    pending[0].resolve(report({ serverParams: { baudRate: 9600 } }));
    await flushPromises();
    expect(resultRegion(wrapper).text()).toContain("Server port settings: 9600");
    wrapper.unmount();
  });

  it("is disabled on the local serial page until a port is picked", async () => {
    const wrapper = mountDialog("serial");
    await flushPromises();
    expect(commandsInvoked()).toContain("list_serial_ports");
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);
    wrapper.unmount();
  });

  it("tests a local serial port with the local transport and the form's line settings", async () => {
    serialPorts = [{ portName: "COM7", portType: "UsbPort" }];
    const wrapper = mountDialog("serial");
    await flushPromises();
    // The first enumerated port is picked for the user.
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(false);

    await wrapper.get(".serial-settings-grid input[type='number']").setValue("115200");
    const paritySelect = wrapper.findAll(".serial-settings-grid select")
      .find((select) => select.find("option[value='even']").exists());
    await paritySelect!.setValue("even");

    await testButton(wrapper).trigger("click");
    expect(testCalls()).toEqual([["serial_test_connection", {
      portName: "COM7",
      baudRate: 115200,
      dataBits: 8,
      stopBits: 1,
      parity: "even",
      flowControl: "none",
      transport: "local",
      host: null,
      netPort: null,
    }]]);
    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).get(".connect-test-title").text()).toContain(
      "The port opened with these settings and was closed again.",
    );
    // Testing never remembers the port settings or saves anything.
    expect(new Set(commandsInvoked())).toEqual(
      new Set(["get_connections", "list_serial_ports", "serial_test_connection"]),
    );
    expect(wrapper.emitted("connect")).toBeUndefined();

    // A line setting is part of what gets opened, so changing it clears the result.
    await wrapper.get(".serial-settings-grid input[type='number']").setValue("9600");
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    wrapper.unmount();
  });

  it("tests raw TCP with the raw-tcp transport and a trimmed host", async () => {
    const wrapper = mountDialog("raw-tcp");
    await flushPromises();
    expect(testButton(wrapper).element.hasAttribute("disabled")).toBe(true);

    const [hostField, portField] = wrapper.findAll(".serial-settings-grid input:not([type='checkbox'])");
    await hostField.setValue(" 10.0.0.9 ");
    await portField.setValue("4001");

    await testButton(wrapper).trigger("click");
    expect(testCalls()).toHaveLength(1);
    const [command, args] = testCalls()[0];
    expect(command).toBe("serial_test_connection");
    expect(args).toMatchObject({ transport: "raw-tcp", host: "10.0.0.9", netPort: 4001, portName: "10.0.0.9:4001" });
    expect(args).not.toHaveProperty("autoReconnect");
    pending[0].resolve(report({ elapsedMs: 7 }));
    await flushPromises();
    expect(resultRegion(wrapper).get(".connect-test-title").text()).toBe("The port is reachable. (7 ms)");
    wrapper.unmount();
  });

  it("keeps an RFC 2217 result when only the adopt or reconnect checkbox changes", async () => {
    const wrapper = mountDialog("rfc2217");
    await flushPromises();
    const [hostField] = wrapper.findAll(".serial-settings-grid input:not([type='checkbox'])");
    await hostField.setValue("10.0.0.9");
    await testButton(wrapper).trigger("click");
    pending[0].resolve(report());
    await flushPromises();
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);

    // The test always runs in adopt mode and never reconnects, so neither
    // option changes what it would report.
    for (const toggle of wrapper.findAll(".serial-adopt-toggle input[type='checkbox']")) {
      await toggle.setValue(!(toggle.element as HTMLInputElement).checked);
    }
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(true);

    // The endpoint does.
    await hostField.setValue("10.0.0.10");
    expect(resultRegion(wrapper).find(".connect-test-title").exists()).toBe(false);
    wrapper.unmount();
  });
});
