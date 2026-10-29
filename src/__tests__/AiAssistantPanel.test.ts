import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import AiAssistantPanel from "../AiAssistantPanel.vue";
import { i18n } from "../i18n";
import { DEFAULT_AI_CONFIG } from "../settings";
import type { AiStreamEventPayload } from "../ai";

const ai = vi.hoisted(() => {
  const state = {
    handlers: [] as Array<(payload: AiStreamEventPayload) => void>,
    unlistened: 0,
    /** Resolves the pending listenAiStream call when set. */
    releaseListen: null as null | (() => void),
    holdListen: false,
  };
  return {
    state,
    aiChatStart: vi.fn(async (..._args: unknown[]) => "unused"),
    aiChatCancel: vi.fn(async (_requestId: string) => undefined),
    listenAiStream: vi.fn((handler: (payload: AiStreamEventPayload) => void) => {
      state.handlers.push(handler);
      const unlisten = () => {
        state.unlistened += 1;
        state.handlers = state.handlers.filter((candidate) => candidate !== handler);
      };
      if (!state.holdListen) return Promise.resolve(unlisten);
      return new Promise<() => void>((resolve) => {
        state.releaseListen = () => resolve(unlisten);
      });
    }),
  };
});
vi.mock("../ai", () => ai);

function emitStream(payload: AiStreamEventPayload) {
  for (const handler of ai.state.handlers) handler(payload);
}

function mountPanel() {
  return mount(AiAssistantPanel, {
    props: {
      config: { ...DEFAULT_AI_CONFIG, enabled: true },
      hasApiKey: true,
      env: {},
    },
    global: { plugins: [i18n] },
  });
}

async function sendMessage(wrapper: ReturnType<typeof mountPanel>, text: string) {
  await wrapper.find("textarea").setValue(text);
  await wrapper.find("textarea").trigger("keydown", { key: "Enter" });
  await flushPromises();
}

describe("AiAssistantPanel", () => {
  beforeEach(() => {
    ai.state.handlers = [];
    ai.state.unlistened = 0;
    ai.state.holdListen = false;
    ai.state.releaseListen = null;
    ai.aiChatStart.mockReset();
    ai.aiChatCancel.mockReset();
    ai.aiChatCancel.mockResolvedValue(undefined);
  });

  it("does not drop an error that arrives before the start call returns", async () => {
    // The backend can fail (e.g. a refused connection) and emit the error
    // event before the invoke that started the request has resolved.
    ai.aiChatStart.mockImplementation(async (...args: unknown[]) => {
      const requestId = args[2] as string;
      emitStream({ requestId, kind: "error", message: "connection refused" });
      return requestId;
    });
    const wrapper = mountPanel();
    await flushPromises();

    await sendMessage(wrapper, "hello");

    expect(wrapper.find(".ai-caret").exists()).toBe(false);
    expect(wrapper.text()).toContain("connection refused");
    // The panel is ready for the next message rather than stuck generating.
    await wrapper.find("textarea").setValue("again");
    expect(wrapper.find(".ai-message.assistant .ai-message-body.error").exists()).toBe(true);
    wrapper.unmount();
  });

  it("releases the stream listener even when closed before it was registered", async () => {
    ai.state.holdListen = true;
    const wrapper = mountPanel();
    wrapper.unmount();
    ai.state.releaseListen?.();
    await flushPromises();
    expect(ai.state.unlistened).toBe(1);
    expect(ai.state.handlers).toHaveLength(0);
  });

  it("does not leave a failed cancel as an unhandled rejection", async () => {
    let resolveStart: (id: string) => void = () => {};
    ai.aiChatStart.mockImplementation((...args: unknown[]) => new Promise<string>((resolve) => {
      resolveStart = () => resolve(args[2] as string);
    }));
    ai.aiChatCancel.mockRejectedValue(new Error("backend gone"));
    const logError = vi.spyOn(console, "error").mockImplementation(() => {});
    const wrapper = mountPanel();
    await flushPromises();
    await sendMessage(wrapper, "hello");
    resolveStart("");
    await flushPromises();

    wrapper.unmount();
    await flushPromises();
    expect(ai.aiChatCancel).toHaveBeenCalledTimes(1);
    expect(logError).toHaveBeenCalled();
    logError.mockRestore();
  });
});
