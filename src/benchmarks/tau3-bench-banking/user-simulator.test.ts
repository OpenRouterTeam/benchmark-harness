import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import assert from "node:assert/strict";

import { FetchHttpClient, HttpClient } from "@effect/platform";
import type { Effect } from "effect/Effect";
import { flatMap, provide, runPromise } from "effect/Effect";

import type { CapturedRequest } from "../../../test/helpers/fetch-sequence";
import { installFetchSequence } from "../../../test/helpers/fetch-sequence";
import { isRecord } from "../../internal/guards";
import { setCurrentEpoch } from "../../runtime/response-cache";
import { UserSimulator } from "./user-simulator";

const TEST_API_KEY = "test-key-123";

const TEST_MODEL = "openai/gpt-4o-mini";

const TEST_SESSION = "test-session-1";

function createTestConfig() {
  return {
    apiKey: TEST_API_KEY,
    model: TEST_MODEL,
    sessionId: TEST_SESSION,
  };
}

let originalFetch: typeof global.fetch;
beforeEach(() => {
  originalFetch = global.fetch;
  global.fetch = async () =>
    new Response(
      JSON.stringify({
        choices: [{ message: { content: "Default response" } }],
      }),
      { status: 200 }
    );
});
afterEach(() => {
  global.fetch = originalFetch;
});

function runSim<A>(
  effect: Effect<A, unknown, HttpClient.HttpClient>
): Promise<A> {
  return runPromise(provide(effect, FetchHttpClient.layer));
}
describe("UserSimulator", () => {
  describe("initialization", () => {
    it("normalizes trailing-slash and already-suffixed base URLs to the same endpoint", async () => {
      const requestedUrls: string[] = [];
      global.fetch = async (input, init) => {
        requestedUrls.push(new Request(input, init).url);
        return new Response(
          JSON.stringify({ choices: [{ message: { content: "ok" } }] }),
          {
            status: 200,
          }
        );
      };
      for (const baseUrl of [
        "https://api.example.com/api/v1/",
        "https://api.example.com",
      ]) {
        const sim = new UserSimulator({ ...createTestConfig(), baseUrl });
        sim.reset("Scenario", "Hi.");
        await runSim(sim.generateInitial());
      }
      expect(requestedUrls).toEqual([
        "https://api.example.com/api/v1/chat/completions",
        "https://api.example.com/api/v1/chat/completions",
      ]);
    });
  });
  describe("reset", () => {
    it("seeds the scenario system prompt and first agent message", async () => {
      const scenario = "Help me reset my password.";
      const firstAgentMessage = "Hi, I need help with my account.";
      let requestBody: {
        messages?: {
          role: string;
          content: string;
        }[];
      } = {};
      global.fetch = async (input, init) => {
        requestBody = await new Request(input, init).json();
        return new Response(
          JSON.stringify({ choices: [{ message: { content: "ok" } }] }),
          {
            status: 200,
          }
        );
      };
      const sim = new UserSimulator(createTestConfig());
      sim.reset(scenario, firstAgentMessage);
      await runSim(sim.generateInitial());
      const [systemMessage, userMessage] = requestBody.messages ?? [];
      assert(systemMessage);
      assert(userMessage);
      expect(systemMessage.role).toBe("system");
      expect(systemMessage.content).toContain(scenario);
      expect(userMessage.role).toBe("user");
      expect(userMessage.content).toBe(firstAgentMessage);
    });
  });
  describe("generateInitial", () => {
    it("returns text turn with content", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me with my account", "Hi there.");
      let callCount = 0;
      global.fetch = async () => {
        callCount++;
        return new Response(
          JSON.stringify({
            choices: [
              {
                message: {
                  content: "Hello! How can I help you today?",
                },
              },
            ],
          }),
          { status: 200 }
        );
      };
      const result = await runSim(sim.generateInitial());
      expect(result.kind).toBe("text");
      if (result.kind === "text") {
        expect(result.content).toBe("Hello! How can I help you today?");
      }
      expect(callCount).toBe(1);
    });
    it("sends auth headers", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help", "Hi");
      let authorization: string | null = null;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        authorization = request.headers.get("Authorization");
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "Hi" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(sim.generateInitial());
      expect(authorization).toBe(`Bearer ${TEST_API_KEY}`);
    });
    it("sends the response-cache headers and an epoch-scoped cache_salt", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help", "Hi");
      let cacheHeader: string | null = null;
      let ttlHeader: string | null = null;
      let capturedBody: unknown;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        cacheHeader = request.headers.get("x-openrouter-cache");
        ttlHeader = request.headers.get("x-openrouter-cache-ttl");
        capturedBody = await request.clone().json();
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "Hi" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(
        setCurrentEpoch(2).pipe(flatMap(() => sim.generateInitial()))
      );
      expect(cacheHeader).toBe("true");
      expect(ttlHeader).toBe("7200");
      assert(isRecord(capturedBody));
      expect(capturedBody["cache_salt"]).toBe(`${TEST_SESSION}:epoch-2`);
    });
  });
  describe("step", () => {
    it("appends agent message and generates response", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      global.fetch = async () =>
        new Response(
          JSON.stringify({
            choices: [{ message: { content: "Will do" } }],
          }),
          { status: 200 }
        );
      const result = await runSim(sim.step("How can I help?"));
      expect(result.kind).toBe("text");
      if (result.kind === "text") {
        expect(result.content).toBe("Will do");
      }
    });
    it("does not mutate history when the effect is constructed but never run", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      const discarded = sim.step("SHOULD NOT APPEAR");
      expect(discarded).toBeDefined();
      let capturedMessages: unknown;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const body: unknown = await request.json();
        assert(isRecord(body));
        capturedMessages = body["messages"];
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "Hello" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(sim.generateInitial());
      assert(Array.isArray(capturedMessages));
      expect(capturedMessages).toHaveLength(2);
      expect(JSON.stringify(capturedMessages)).not.toContain(
        "SHOULD NOT APPEAR"
      );
    });
    it("keeps prior user-simulator text in the next request history", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      let callCount = 0;
      global.fetch = async (input, init) => {
        callCount++;
        const request = new Request(input, init);
        const requestBody: unknown = await request.json();
        assert(isRecord(requestBody));
        const messages = requestBody["messages"];
        assert(Array.isArray(messages));
        if (callCount === 2) {
          expect(messages).toContainEqual({
            role: "assistant",
            content: "First user turn",
          });
        }
        return Response.json({
          choices: [
            {
              message: {
                content:
                  callCount === 1 ? "First user turn" : "Second user turn",
              },
            },
          ],
        });
      };
      await runSim(sim.generateInitial());
      await runSim(sim.step("Agent reply"));
      expect(callCount).toBe(2);
    });
    it("keeps non-reasoning history in the pre-replay wire shape", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      const requests: CapturedRequest[] = [];
      const restore = installFetchSequence(
        [
          {
            model: TEST_MODEL,
            choices: [{ message: { content: "First user turn" } }],
          },
          { choices: [{ message: { content: "Second user turn" } }] },
        ],
        requests
      );
      try {
        await runSim(sim.generateInitial());
        await runSim(sim.step("Agent reply"));
        const secondRequest = requests[1];
        assert(secondRequest);
        const messages = secondRequest.body["messages"];
        assert(Array.isArray(messages));
        const assistant = messages.find(
          (message) =>
            isRecord(message) && message["content"] === "First user turn"
        );
        assert(isRecord(assistant));
        expect(assistant).toEqual({
          role: "assistant",
          content: "First user turn",
        });
      } finally {
        restore();
      }
    });
  });
  describe("tool-call handling", () => {
    it("replays opaque reasoning_details across tool and text turns", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      const reasoningDetails = [
        { type: "summary", summary: "opaque" },
        { type: "future_variant", future_payload: { step: 1 } },
      ];
      const requests: CapturedRequest[] = [];
      const restore = installFetchSequence(
        [
          {
            model: TEST_MODEL,
            choices: [
              {
                message: {
                  content: "",
                  reasoning_details: reasoningDetails,
                  tool_calls: [
                    {
                      id: "call_1",
                      type: "function",
                      function: { name: "lookup", arguments: "{}" },
                    },
                  ],
                },
              },
            ],
          },
          {
            model: "openai/gpt-4o",
            choices: [
              {
                message: {
                  content: "Done",
                  reasoning_details: reasoningDetails,
                },
              },
            ],
          },
          {
            choices: [{ message: { content: "Done" } }],
          },
        ],
        requests
      );
      try {
        const first = await runSim(sim.generateInitial());
        expect(first.kind).toBe("toolCalls");
        sim.addToolResult("call_1", "tool result");
        await runSim(sim.continueAfterTools());
        const secondRequest = requests[1];
        assert(secondRequest);
        const secondMessages = secondRequest.body["messages"];
        assert(Array.isArray(secondMessages));
        const assistant = secondMessages.find(
          (message) => isRecord(message) && message["role"] === "assistant"
        );
        assert(isRecord(assistant));
        expect(assistant).toMatchObject({
          reasoning_details: reasoningDetails,
          tool_calls: [
            {
              id: "call_1",
              type: "function",
              function: { name: "lookup", arguments: "{}" },
            },
          ],
        });
        const third = await runSim(sim.step("Agent follow-up"));
        expect(third.kind).toBe("text");
        const thirdRequest = requests[2];
        assert(thirdRequest);
        const thirdMessages = thirdRequest.body["messages"];
        assert(Array.isArray(thirdMessages));
        const textAssistant = thirdMessages.find(
          (message) => isRecord(message) && message["content"] === "Done"
        );
        assert(isRecord(textAssistant));
        expect(textAssistant["reasoning_details"]).toEqual(reasoningDetails);
      } finally {
        restore();
      }
    });
    it("returns tool calls when response includes them", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      global.fetch = async () =>
        new Response(
          JSON.stringify({
            choices: [
              {
                message: {
                  content: null,
                  tool_calls: [
                    {
                      id: "call_123",
                      type: "function",
                      function: {
                        name: "check_balance",
                        arguments: '{"account_id": "acc_1"}',
                      },
                    },
                  ],
                },
              },
            ],
          }),
          { status: 200 }
        );
      const result = await runSim(sim.generateInitial());
      expect(result.kind).toBe("toolCalls");
      if (result.kind === "toolCalls") {
        expect(result.calls).toHaveLength(1);
        expect(result.calls[0]?.name).toBe("check_balance");
        expect(result.calls[0]?.id).toBe("call_123");
      }
    });
    it("addToolResult appends result message", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      sim.addToolResult("call_123", "Balance: $1000");
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const bodyJson: unknown = await request.json();
        assert(isRecord(bodyJson));
        const messages = bodyJson["messages"];
        assert(Array.isArray(messages));
        expect(messages.length).toBeGreaterThanOrEqual(3);
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "Got it" } }],
          }),
          { status: 200 }
        );
      };
      const result = await runSim(sim.step("Here is the balance"));
      expect(result.kind).toBe("text");
    });
    it("continues after tool results without duplicating the agent message", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help me", "Hi");
      sim.addToolResult("call_123", "Balance: $1000");
      let capturedMessages: unknown[] | undefined;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const bodyJson: unknown = await request.json();
        assert(isRecord(bodyJson));
        const messages = bodyJson["messages"];
        assert(Array.isArray(messages));
        capturedMessages = messages;
        return new Response(
          JSON.stringify({ choices: [{ message: { content: "Got it" } }] }),
          {
            status: 200,
          }
        );
      };
      await runSim(sim.continueAfterTools());
      expect(capturedMessages).toHaveLength(3);
      expect(capturedMessages?.at(-1)).toEqual({
        role: "tool",
        content: "Balance: $1000",
        tool_call_id: "call_123",
      });
    });
  });
  describe("setAvailableTools", () => {
    it("includes tools in request when set", async () => {
      const sim = new UserSimulator(createTestConfig());
      const toolDef = {
        type: "function" as const,
        function: {
          name: "test_tool",
          description: "A test tool",
          parameters: { type: "object" as const, properties: {} },
        },
      };
      sim.setAvailableTools([toolDef]);
      sim.reset("Help", "Hi");
      let hasTools = false;
      let systemPrompt = "";
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const bodyJson: unknown = await request.json();
        assert(isRecord(bodyJson));
        const tools = bodyJson["tools"];
        hasTools = Array.isArray(tools) && tools.length > 0;
        const messages = bodyJson["messages"];
        if (Array.isArray(messages) && isRecord(messages[0])) {
          systemPrompt =
            typeof messages[0]["content"] === "string"
              ? messages[0]["content"]
              : "";
        }
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "OK" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(sim.generateInitial());
      expect(hasTools).toBe(true);
      expect(systemPrompt).toContain(
        "Make a tool call to perform an action requested by the agent."
      );
    });
  });
  describe("error handling", () => {
    it("throws on HTTP error", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help", "Hi");
      global.fetch = async () =>
        new Response(JSON.stringify({ error: "Server error" }), {
          status: 500,
        });
      await expect(runSim(sim.generateInitial())).rejects.toThrow("HTTP 500");
    });
    it("propagates parse errors without retrying another model", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help", "Hi");
      let callCount = 0;
      global.fetch = async () => {
        callCount++;
        return new Response(JSON.stringify({}), { status: 200 });
      };
      await expect(runSim(sim.generateInitial())).rejects.toThrow(
        "response parse error"
      );
      expect(callCount).toBe(1);
    });
  });
  describe("temperature and model", () => {
    it("sends temperature: 0", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help", "Hi");
      let capturedTemp: unknown;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const bodyJson: unknown = await request.json();
        assert(isRecord(bodyJson));
        capturedTemp = bodyJson["temperature"];
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "OK" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(sim.generateInitial());
      expect(capturedTemp).toBe(0);
    });
    it("sends configured model", async () => {
      const sim = new UserSimulator(createTestConfig());
      sim.reset("Help", "Hi");
      let capturedModel: unknown;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const bodyJson: unknown = await request.json();
        assert(isRecord(bodyJson));
        capturedModel = bodyJson["model"];
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "OK" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(sim.generateInitial());
      expect(capturedModel).toBe(TEST_MODEL);
    });
    it("sends configured user reasoning effort", async () => {
      const sim = new UserSimulator({
        ...createTestConfig(),
        userReasoningEffort: "medium",
      });
      sim.reset("Help", "Hi");
      let capturedReasoningEffort: unknown;
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const bodyJson: unknown = await request.json();
        assert(isRecord(bodyJson));
        capturedReasoningEffort = bodyJson["reasoning_effort"];
        return new Response(
          JSON.stringify({
            choices: [{ message: { content: "OK" } }],
          }),
          { status: 200 }
        );
      };
      await runSim(sim.generateInitial());
      expect(capturedReasoningEffort).toBe("medium");
    });
  });
});
