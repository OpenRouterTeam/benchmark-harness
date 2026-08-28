import { afterEach, describe, expect, it } from "bun:test";
import { readFile } from "node:fs/promises";

import { FetchHttpClient } from "@effect/platform";
import type { ResponsesRequest } from "@openrouter/sdk/models";
import { failureOption } from "effect/Cause";
import { flatMap, gen, provide, runPromiseExit, succeed } from "effect/Effect";
import { provide as layerProvide } from "effect/Layer";
import { getOrThrow } from "effect/Option";

import { assertFailure, assertSuccess } from "../../test/helpers/exit-asserts";
import { ProviderSort } from "../internal/enums";
import { isRecord } from "../internal/guards";
import { assertRight } from "../internal/testing";
import { parseSchema, z } from "../internal/zod";
import {
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../runtime/generation-ids";
import type {
  ResponsesSendOptions,
  ResponsesService,
} from "./responses-client";
import { usageFromResponses } from "./responses-client";
import {
  generate,
  ResponsesModel,
  makeResponsesModelLayer,
  responsesMessage,
} from "./responses-model";

interface CapturedRequest {
  readonly url: string;
  readonly body: Record<string, unknown>;
  readonly headers: Record<string, string>;
}

function parseJsonObject(raw: string): Record<string, unknown> {
  const parsed: unknown = JSON.parse(raw);
  return isRecord(parsed) ? parsed : {};
}

function installFetchStub(
  responseBody: string,
  status: number,
  captured: {
    value: CapturedRequest | undefined;
    responseHeaders?: Record<string, string>;
  }
): () => void {
  const original = globalThis.fetch;
  globalThis.fetch = async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    const headers: Record<string, string> = {};
    for (const [key, value] of request.headers.entries()) {
      headers[key.toLowerCase()] = value;
    }
    captured.value = {
      url: request.url,
      body: parseJsonObject(await request.clone().text()),
      headers,
    };
    return new Response(responseBody, {
      status,
      headers: {
        "content-type": "text/event-stream",
        ...captured.responseHeaders,
        ...(status === 429 && { "retry-after": "0" }),
      },
    });
  };
  return () => {
    globalThis.fetch = original;
  };
}

const TerminalFixtureSchema = z.object({
  output: z.array(z.record(z.string(), z.unknown())),
  usage: z.record(z.string(), z.unknown()),
});

async function readTerminalFixture(): Promise<
  z.infer<typeof TerminalFixtureSchema>
> {
  const raw = await readFile(
    new URL(
      "../../test/fixtures/advisor-responses-terminal.json",
      import.meta.url
    ),
    "utf8"
  );
  const result = parseSchema(TerminalFixtureSchema, JSON.parse(raw));
  assertRight(result);
  return result.right;
}

function readStreamFixture(): Promise<string> {
  return readFile(
    new URL(
      "../../test/fixtures/advisor-responses-stream.sse",
      import.meta.url
    ),
    "utf8"
  );
}

const FUNCTION_CALL_OUTPUT = {
  type: "function_call",
  id: "fc_1",
  call_id: "call_1",
  status: "completed",
  name: "bash",
  arguments: '{"command":"pwd"}',
};

function withFunctionCallOutput(stream: string): string {
  return stream
    .split("\n")
    .map((line) => {
      if (!line.startsWith("data: ") || line === "data: [DONE]") {
        return line;
      }
      const event: unknown = JSON.parse(line.slice(6));
      if (!isRecord(event) || event["type"] !== "response.completed") {
        return line;
      }
      const response = event["response"];
      if (!isRecord(response) || !Array.isArray(response["output"])) {
        return line;
      }
      return `data: ${JSON.stringify({
        ...event,
        response: {
          ...response,
          output: [...response["output"], FUNCTION_CALL_OUTPUT],
        },
      })}`;
    })
    .join("\n");
}

describe("responses-model", () => {
  it("constructs explicit Responses message items", () => {
    expect(responsesMessage("user", "solve this")).toEqual({
      type: "message",
      role: "user",
      content: "solve this",
    });
  });

  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });
  it("builds a Responses request and parses output items, calls, text, and usage", async () => {
    const terminal = await readTerminalFixture();
    const stream = withFunctionCallOutput(await readStreamFixture());
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub(stream, 200, captured);
    const input = [
      { role: "user", content: "solve this" },
      { type: "function_call_output", call_id: "call_0", output: "ok" },
    ];
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      baseUrl: "https://example.test",
      sessionId: "session-1",
    });
    const exit = await runPromiseExit(
      resetGenerationIds.pipe(
        flatMap(() =>
          gen(function* run() {
            const model = yield* ResponsesModel;
            const turn = yield* model.generate(input, {
              instructions: "Use bash.",
              tools: [
                {
                  type: "function",
                  name: "bash",
                  description: "Run bash.",
                  parameters: { type: "object" },
                },
              ],
              reasoningEffort: "high",
              temperature: 0,
              maxTokens: 256,
              sort: ProviderSort.Price,
              cloudflareVersion: "ver-1",
              extraBody: { custom_field: "value" },
            });
            const generationIds = yield* getCollectedGenerationIds;
            return { turn, generationIds };
          })
        ),
        provide(layer.pipe(layerProvide(FetchHttpClient.layer)))
      )
    );
    assertSuccess(exit);
    expect(captured.value?.url).toBe("https://example.test/api/v1/responses");
    expect(captured.value?.body).toMatchObject({
      model: "openai/gpt-5",
      input,
      stream: true,
      store: false,
      cache_control: { type: "ephemeral" },
      include: ["reasoning.encrypted_content"],
      instructions: "Use bash.",
      tools: [
        {
          type: "function",
          name: "bash",
          description: "Run bash.",
          parameters: { type: "object" },
        },
      ],
      reasoning: { effort: "high" },
      temperature: 0,
      max_output_tokens: 256,
      provider: { sort: "price" },
      custom_field: "value",
    });
    expect(exit.value.turn.outputItems).toEqual([
      ...terminal.output,
      FUNCTION_CALL_OUTPUT,
    ]);
    expect(exit.value.turn.functionCalls).toEqual([
      { callId: "call_1", name: "bash", arguments: '{"command":"pwd"}' },
    ]);
    expect(exit.value.turn.text).toBe("4");
    expect(exit.value.turn.usage).toEqual(usageFromResponses(terminal.usage));
    expect(exit.value.generationIds).toEqual([
      "gen-1784161874-CXX4U5I6Ej7Z5hTnf0wU",
    ]);
    expect(captured.value?.headers["http-referer"]).toBe(
      "https://bench-harness.openrouter.ai/"
    );
    expect(captured.value?.headers["x-openrouter-title"]).toBe(
      "OpenRouter: Bench Harness"
    );
    expect(
      captured.value?.headers["cloudflare-workers-version-overrides"]
    ).toBe("ver-1");
    expect(captured.value?.headers["x-session-id"]).toBe("session-1");
  });
  it("preserves legacy checkpoint items and maps SDK function calls", async () => {
    let sentBody: ResponsesRequest | undefined;
    let sentOptions: ResponsesSendOptions | undefined;
    const responses: ResponsesService = {
      send: (body, options) => {
        sentBody = body;
        sentOptions = options;
        return succeed({
          id: "resp-1",
          model: "openai/gpt-5",
          status: "completed",
          output: [
            {
              type: "function_call",
              callId: "call-1",
              name: "bash",
              arguments: '{"command":"pwd"}',
            },
            {
              type: "openrouter:fusion",
              failedModels: [{ model: "model-a", statusCode: 503 }],
              failureReason: "all_panels_failed",
              futureSdkField: "preserved",
              output: { statusCode: 200 },
            },
          ],
          usage: null,
          text: "",
          generationId: "resp-1",
          provider: "OpenAI",
          generationTimeMs: 1,
        });
      },
    };
    const exit = await runPromiseExit(
      generate(
        {
          model: "openai/gpt-5",
          input: [
            { type: "function_call_output", call_id: "call-0", output: "ok" },
            {
              type: "reasoning",
              id: "reasoning-1",
              summary: [],
              encrypted_content: "opaque",
            },
          ],
          genConfig: {},
        },
        responses
      )
    );
    assertSuccess(exit);
    expect(sentBody?.input).toEqual([
      {
        type: "function_call_output",
        callId: "call-0",
        output: "ok",
      },
      {
        type: "reasoning",
        id: "reasoning-1",
        summary: [],
        encryptedContent: "opaque",
      },
    ]);
    expect(sentOptions?.extraBody).toBeUndefined();
    expect(exit.value.outputItems).toEqual([
      {
        type: "function_call",
        call_id: "call-1",
        name: "bash",
        arguments: '{"command":"pwd"}',
      },
      {
        type: "openrouter:fusion",
        failed_models: [{ model: "model-a", status_code: 503 }],
        failure_reason: "all_panels_failed",
        future_sdk_field: "preserved",
        output: { statusCode: 200 },
      },
    ]);
    expect(exit.value.functionCalls).toEqual([
      {
        callId: "call-1",
        name: "bash",
        arguments: '{"command":"pwd"}',
      },
    ]);
  });
  it("sends provider sort only when endpoint pinning is absent", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub(await readStreamFixture(), 200, captured);
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 0 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate([], {
          sort: ProviderSort.Price,
          endpointId: "endpoint-1",
          reasoningEffort: "high",
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(captured.value?.body["provider"]).toBeUndefined();
    expect(captured.value?.headers["x-or-endpoint-id"]).toBe("endpoint-1");
  });
  it("sends provider.only with fallbacks disabled on pinned runs", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub(await readStreamFixture(), 200, captured);
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 0 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate([], {
          providerOnly: ["google-vertex"],
          providerIgnore: ["azure"],
          allowFallbacks: false,
          reasoningEffort: "high",
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(captured.value?.body["provider"]).toEqual({
      only: ["google-vertex"],
      ignore: ["azure"],
      allow_fallbacks: false,
    });
  });
  for (const [model, pluginId] of [
    ["openrouter/auto", "auto-router"],
    ["openrouter/auto-beta", "auto-beta-router"],
  ] as const) {
    it(`sends cost_tier on the ${pluginId} plugin`, async () => {
      const captured: {
        value: CapturedRequest | undefined;
      } = { value: undefined };
      restore = installFetchStub(await readStreamFixture(), 200, captured);
      const layer = makeResponsesModelLayer({ model, apiKey: "sk-test" });
      const exit = await runPromiseExit(
        gen(function* run() {
          const modelService = yield* ResponsesModel;
          return yield* modelService.generate([], {
            costTier: "high",
            reasoningEffort: "high",
          });
        }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
      );
      assertSuccess(exit);
      expect(captured.value?.body["plugins"]).toEqual([
        { id: pluginId, cost_tier: "high" },
      ]);
    });
  }
  it("sends cost_tier alongside the deprecated numeric tradeoff", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub(await readStreamFixture(), 200, captured);
    const layer = makeResponsesModelLayer({
      model: "openrouter/auto",
      apiKey: "sk-test",
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const modelService = yield* ResponsesModel;
        return yield* modelService.generate([], {
          costTier: "medium",
          costQualityTradeoff: 8,
          reasoningEffort: "high",
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(captured.value?.body["plugins"]).toEqual([
      { id: "auto-router", cost_tier: "medium", cost_quality_tradeoff: 8 },
    ]);
  });
  it("omits plugins when no auto-router option is set", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub(await readStreamFixture(), 200, captured);
    const layer = makeResponsesModelLayer({
      model: "openrouter/auto",
      apiKey: "sk-test",
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const modelService = yield* ResponsesModel;
        return yield* modelService.generate([], { reasoningEffort: "high" });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(captured.value?.body["plugins"]).toBeUndefined();
  });
  it("forwards streamed events and fails retryably on a failed event", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    const events: Record<string, unknown>[] = [];
    restore = installFetchStub(
      `data: ${JSON.stringify({ type: "response.failed", response: { error: { message: "upstream" } } })}\n\n`,
      200,
      captured
    );
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 0 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate(
          [],
          {},
          { onStreamEvent: (event) => events.push(event) }
        );
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertFailure(exit);
    expect(events).toEqual([
      { type: "response.failed", response: { error: { message: "upstream" } } },
    ]);
    const error = getOrThrow(failureOption(exit.cause));
    expect(error.status).toBe(500);
  });
  it("preserves response identifiers on a failed stream", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub(
      `data: ${JSON.stringify({
        type: "response.failed",
        response: { id: "gen-789", error: { message: "upstream" } },
      })}\n\n`,
      200,
      {
        ...captured,
        responseHeaders: { "cf-ray": "ray-123", "x-request-id": "req-456" },
      }
    );
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 0 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate([], { reasoningEffort: "high" });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertFailure(exit);
    const error = getOrThrow(failureOption(exit.cause));
    expect(error.cfRay).toBe("ray-123");
    expect(error.xRequestId).toBe("req-456");
    expect(error.generationId).toBe("gen-789");
    expect(error.message).toContain("cf_ray=ray-123");
    expect(error.message).toContain("x_request_id=req-456");
    expect(error.message).toContain("generation_id=gen-789");
  });
  it("preserves response headers when streaming times out", async () => {
    const original = globalThis.fetch;
    const streamGate = new Promise<void>(() => {});
    globalThis.fetch = async () =>
      new Response(
        new ReadableStream({
          start: async (controller) => {
            await streamGate;
            controller.enqueue(new TextEncoder().encode("data: [DONE]\n\n"));
            controller.close();
          },
        }),
        {
          status: 200,
          headers: {
            "content-type": "text/event-stream",
            "cf-ray": "ray-timeout",
          },
        }
      );
    restore = () => {
      globalThis.fetch = original;
    };
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 0 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate([], {
          timeoutMs: 50,
          reasoningEffort: "high",
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertFailure(exit);
    const error = getOrThrow(failureOption(exit.cause));
    expect(error.status).toBe(408);
    expect(error.cfRay).toBe("ray-timeout");
    expect(error.message).toContain("cf_ray=ray-timeout");
  });
  it("preserves the real advisor stream output and reports streamed item events", async () => {
    const terminal = await readTerminalFixture();
    const stream = await readStreamFixture();
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    const events: Record<string, unknown>[] = [];
    restore = installFetchStub(stream, 200, captured);
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 0 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate(
          [],
          { reasoningEffort: "high" },
          { onStreamEvent: (event) => events.push(event) }
        );
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(exit.value.outputItems).toEqual(terminal.output);
    expect(exit.value.text).toBe("4");
    expect(exit.value.usage).toEqual(usageFromResponses(terminal.usage));
    expect(
      events.filter((event) => event["type"] === "response.output_item.added")
    ).not.toHaveLength(0);
    expect(captured.value?.body["stream"]).toBe(true);
  });
  it("maps a non-success response to ModelError without retrying a 400", async () => {
    const captured: {
      value: CapturedRequest | undefined;
    } = { value: undefined };
    restore = installFetchStub("bad request", 400, captured);
    const layer = makeResponsesModelLayer({
      model: "openai/gpt-5",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 3 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* ResponsesModel;
        return yield* model.generate([], { reasoningEffort: "high" });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertFailure(exit);
    const error = getOrThrow(failureOption(exit.cause));
    expect(error._tag).toBe("ModelError");
    expect(error.status).toBe(400);
    expect(error.message).toBe("OpenRouter HTTP 400: bad request");
  });
});
