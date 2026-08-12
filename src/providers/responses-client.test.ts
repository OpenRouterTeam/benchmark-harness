import { describe, expect, it } from "bun:test";
import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";

import type { StreamEvents } from "@openrouter/sdk/models";
import { OpenRouterError } from "@openrouter/sdk/models/errors/openroutererror";
import { failureOption } from "effect/Cause";
import {
  flatMap,
  gen,
  provide,
  runPromise,
  runPromiseExit,
} from "effect/Effect";
import { getOrThrow } from "effect/Option";

import { assertFailure } from "../../test/helpers/exit-asserts";
import { assertRight } from "../internal/testing";
import { parseSchema, z } from "../internal/zod";
import {
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../runtime/generation-ids";
import type { ModelErrorIdentifiers } from "./request-identifiers";
import {
  consumeStream,
  extractMessageText,
  findOutputItems,
  makeResponsesLayer,
  Responses,
  ResponsesError,
  toModelError,
  usageFromResponses,
} from "./responses-client";

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

async function readStreamFixture(): Promise<string> {
  return readFile(
    new URL(
      "../../test/fixtures/advisor-responses-stream.sse",
      import.meta.url
    ),
    "utf8"
  );
}
describe("extractMessageText", () => {
  it("concatenates output_text from message items", () => {
    const output = [
      {
        type: "reasoning",
        content: [{ type: "reasoning_text", text: "thinking..." }],
      },
      {
        type: "message",
        content: [
          { type: "output_text", text: "Hello " },
          { type: "output_text", text: "world" },
        ],
      },
      { type: "openrouter:fusion", responses: [] },
    ];
    expect(extractMessageText(output)).toBe("Hello world");
  });
  it("ignores non-output_text content parts and non-message items", () => {
    const output = [
      {
        type: "message",
        content: [
          { type: "input_text", text: "ignored" },
          { type: "output_text", text: "kept" },
        ],
      },
    ];
    expect(extractMessageText(output)).toBe("kept");
  });
  it("returns empty string for no message items", () => {
    expect(extractMessageText([{ type: "reasoning" }])).toBe("");
    expect(extractMessageText([])).toBe("");
  });
});
describe("findOutputItems", () => {
  it("returns all items matching the type", () => {
    const output = [
      { type: "openrouter:fusion", status: "completed" },
      { type: "message" },
      { type: "openrouter:fusion", status: "incomplete" },
    ];
    const fusion = findOutputItems(output, "openrouter:fusion");
    expect(fusion).toHaveLength(2);
    expect(fusion[0]!.status).toBe("completed");
  });
  it("returns empty when no match", () => {
    expect(findOutputItems([{ type: "message" }], "openrouter:fusion")).toEqual(
      []
    );
  });
});
describe("toModelError", () => {
  it("passes through explicit status", () => {
    const err = toModelError(
      new ResponsesError({ message: "fail", retryable: true, status: 429 })
    );
    expect(err.message).toBe("fail");
    expect(err.status).toBe(429);
  });
  it("synthesizes status 500 for retryable errors without status", () => {
    const err = toModelError(
      new ResponsesError({ message: "stream err", retryable: true })
    );
    expect(err.status).toBe(500);
  });
  it("omits status for non-retryable errors without status", () => {
    const err = toModelError(
      new ResponsesError({ message: "permanent", retryable: false })
    );
    expect(err.status).toBeUndefined();
  });
});
describe("usageFromResponses", () => {
  it("maps camelCase SDK usage keys", () => {
    expect(
      usageFromResponses({
        inputTokens: 10,
        outputTokens: 5,
        totalTokens: 15,
        cost: 0.01,
        outputTokensDetails: { reasoningTokens: 2 },
      })
    ).toEqual({
      inputTokens: 10,
      outputTokens: 5,
      totalTokens: 15,
      reasoningTokens: 2,
      totalCost: 0.01,
    });
  });
  it("maps snake_case passthrough keys", () => {
    expect(
      usageFromResponses({
        input_tokens: 10,
        output_tokens: 5,
        total_tokens: 15,
        output_tokens_details: { reasoning_tokens: 2 },
      })
    ).toEqual({
      inputTokens: 10,
      outputTokens: 5,
      totalTokens: 15,
      reasoningTokens: 2,
    });
  });
  it("returns undefined for null usage", () => {
    expect(usageFromResponses(null)).toBeUndefined();
  });
  it("maps server tool usage in snake_case and camelCase forms", () => {
    expect(
      usageFromResponses({
        server_tool_use_details: {
          web_search_requests: 2,
          tool_calls_requested: 3,
          tool_calls_executed: 1,
        },
      })
    ).toEqual({
      serverToolUse: {
        webSearchRequests: 2,
        toolCallsRequested: 3,
        toolCallsExecuted: 1,
      },
    });
    expect(
      usageFromResponses({
        serverToolUseDetails: {
          webSearchRequests: 4,
          toolCallsRequested: 5,
          toolCallsExecuted: 6,
        },
      })
    ).toEqual({
      serverToolUse: {
        webSearchRequests: 4,
        toolCallsRequested: 5,
        toolCallsExecuted: 6,
      },
    });
  });
  it("maps server_tool_use_details from the real advisor terminal fixture", async () => {
    const terminal = await readTerminalFixture();
    expect(usageFromResponses(terminal.usage)).toEqual({
      inputTokens: 770,
      outputTokens: 186,
      totalTokens: 956,
      reasoningTokens: 64,
      totalCost: 0.0046999,
      serverToolUse: { toolCallsRequested: 1, toolCallsExecuted: 1 },
    });
  });
});
describe("consumeStream", () => {
  it("forwards every stream event to onEvent, including the terminal one", async () => {
    const events: StreamEvents[] = [
      {
        type: "response.output_item.added",
        outputIndex: 0,
        sequenceNumber: 1,
        item: { type: "reasoning", id: "r-1", summary: [] },
      },
      {
        type: "response.completed",
        sequenceNumber: 2,
        response: {
          id: "resp-1",
          createdAt: 0,
          model: "m",
          object: "response",
          output: [],
          parallelToolCalls: true,
          status: "completed",
          toolChoice: "auto",
          tools: [],
          error: null,
          incompleteDetails: null,
          instructions: null,
          temperature: null,
          topP: null,
          completedAt: null,
          frequencyPenalty: null,
          metadata: null,
          presencePenalty: null,
        },
      },
    ];
    async function* stream(): AsyncGenerator<StreamEvents> {
      yield* events;
    }
    const seen: string[] = [];
    const result = await consumeStream(stream(), (event) =>
      seen.push(event.type)
    );
    expect(seen).toEqual(["response.output_item.added", "response.completed"]);
    expect(result?.id).toBe("resp-1");
  });
  it("includes initial request identifiers in stream errors", async () => {
    async function* stream(): AsyncGenerator<StreamEvents> {
      yield {
        type: "response.failed",
        sequenceNumber: 1,
        response: {
          id: "gen-789",
          createdAt: 0,
          model: "m",
          object: "response",
          output: [],
          parallelToolCalls: true,
          status: "failed",
          toolChoice: "auto",
          tools: [],
          error: { code: "server_error", message: "upstream" },
          incompleteDetails: null,
          instructions: null,
          temperature: null,
          topP: null,
          completedAt: null,
          frequencyPenalty: null,
          metadata: null,
          presencePenalty: null,
        },
      };
    }
    const initialIdentifiers: ModelErrorIdentifiers = {
      cfRay: "ray-123",
      xRequestId: "req-456",
    };
    const error = await consumeStream(
      stream(),
      undefined,
      initialIdentifiers
    ).catch((cause: unknown) => cause);
    assert(error instanceof ResponsesError);
    expect(error.cfRay).toBe("ray-123");
    expect(error.xRequestId).toBe("req-456");
    expect(error.generationId).toBe("gen-789");
    expect(error.message).toContain("generation_id=gen-789");
    expect(error.message.match(/generation_id=gen-789/g)).toHaveLength(1);
    expect(initialIdentifiers.generationId).toBe("gen-789");
  });
  it("includes response headers once when stream iteration raises an SDK error", async () => {
    async function* stream(): AsyncGenerator<StreamEvents> {
      throw new OpenRouterError("upstream", {
        response: new Response("failure", {
          status: 503,
          headers: {
            "cf-ray": "ray-123",
            "x-request-id": "req-456",
          },
        }),
        request: new Request("https://example.test"),
        body: "failure",
      });
    }
    const error = await consumeStream(stream()).catch(
      (cause: unknown) => cause
    );
    assert(error instanceof ResponsesError);
    expect(error.cfRay).toBe("ray-123");
    expect(error.xRequestId).toBe("req-456");
    expect(error.message.match(/cf_ray=ray-123/g)).toHaveLength(1);
    expect(error.message.match(/x_request_id=req-456/g)).toHaveLength(1);
  });
});
describe("makeResponsesLayer", () => {
  it("records generation ids and applies default or explicit prompt caching", async () => {
    const originalFetch = globalThis.fetch;
    const stream = await readStreamFixture();
    const capturedBodies: unknown[] = [];
    let capturedHeaders: Headers | undefined;
    globalThis.fetch = async (input, init) => {
      const request =
        input instanceof Request ? input : new Request(input, init);
      capturedBodies.push(await request.clone().json());
      capturedHeaders = request.headers;
      return new Response(stream, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    };
    try {
      const ids = await runPromise(
        resetGenerationIds.pipe(
          flatMap(() =>
            gen(function* run() {
              const responses = yield* Responses;
              yield* responses.send(
                { model: "m", input: [] },
                { timeoutMs: 1000 }
              );
              yield* responses.send(
                { model: "m", input: [] },
                {
                  timeoutMs: 1000,
                  extraBody: {
                    cache_control: { type: "ephemeral", ttl: "1h" },
                  },
                }
              );
            })
          ),
          flatMap(() => getCollectedGenerationIds),
          provide(
            makeResponsesLayer({
              apiKey: "sk-test",
              baseUrl: "https://example.test",
            })
          )
        )
      );
      expect(ids).toEqual(["gen-1784161874-CXX4U5I6Ej7Z5hTnf0wU"]);
      expect(capturedBodies[0]).toMatchObject({
        cache_control: { type: "ephemeral" },
        stream: true,
      });
      expect(capturedBodies[1]).toMatchObject({
        cache_control: { type: "ephemeral", ttl: "1h" },
        stream: true,
      });
      expect(capturedHeaders?.get("http-referer")).toBe(
        "https://bench-harness.openrouter.ai/"
      );
      expect(capturedHeaders?.get("x-openrouter-title")).toBe(
        "OpenRouter: Bench Harness"
      );
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
  it("preserves response headers on a failed SDK stream", async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async () =>
      new Response('data: {"type":"error","message":"upstream"}\n\n', {
        status: 200,
        headers: {
          "content-type": "text/event-stream",
          "cf-ray": "ray-123",
          "x-request-id": "req-456",
        },
      });
    try {
      const exit = await runPromiseExit(
        gen(function* run() {
          const responses = yield* Responses;
          return yield* responses.send(
            { model: "m", input: [] },
            { timeoutMs: 1000 }
          );
        }).pipe(
          provide(
            makeResponsesLayer({
              apiKey: "sk-test",
              baseUrl: "https://example.test",
            })
          )
        )
      );
      assertFailure(exit);
      const error = getOrThrow(failureOption(exit.cause));
      expect(error.cfRay).toBe("ray-123");
      expect(error.xRequestId).toBe("req-456");
      expect(error.message).toContain("cf_ray=ray-123");
      expect(error.message).toContain("x_request_id=req-456");
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
  it("classifies malformed SSE as a retryable response failure", async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async () =>
      new Response("data: {not-json}\n\n", {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    try {
      const exit = await runPromiseExit(
        gen(function* run() {
          const responses = yield* Responses;
          return yield* responses.send(
            { model: "m", input: [] },
            { timeoutMs: 1000 }
          );
        }).pipe(
          provide(
            makeResponsesLayer({
              apiKey: "sk-test",
              baseUrl: "https://example.test",
            })
          )
        )
      );
      assertFailure(exit);
      const error = getOrThrow(failureOption(exit.cause));
      expect(error.status).toBe(500);
      expect(error.retryable).toBe(true);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
});
