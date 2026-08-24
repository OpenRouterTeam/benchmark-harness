import type { Mock } from "bun:test";
import { afterEach, describe, expect, it, spyOn } from "bun:test";
import assert from "node:assert";

import { FetchHttpClient } from "@effect/platform";
import { failureOption } from "effect/Cause";
import {
  flatMap,
  gen,
  provide,
  runPromise,
  runPromiseExit,
} from "effect/Effect";
import type { Exit, Failure } from "effect/Exit";
import { isFailure, isSuccess } from "effect/Exit";
import { provide as layerProvide } from "effect/Layer";
import { getOrUndefined } from "effect/Option";

import { assertFailure, assertSuccess } from "../../test/helpers/exit-asserts";
import type { CapturedRequest } from "../../test/helpers/fetch-sequence";
import { installFetchSequence } from "../../test/helpers/fetch-sequence";
import type { ModelError, ModelOutput } from "../harness/core";
import { MessageRole } from "../harness/core";
import { Model } from "../harness/model";
import { ProviderSort } from "../internal/enums";
import { isRecord } from "../internal/guards";
import {
  getCollectedGenerationIdEntries,
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../runtime/generation-ids";
import { setCurrentEpoch, withRunAttempt } from "../runtime/response-cache";
import { makeOpenRouterModelLayer } from "./openrouter-model";

const CHAT_RESULT = {
  id: "1",
  object: "chat.completion",
  created: 0,
  model: "m",
  system_fingerprint: "",
  choices: [
    {
      index: 0,
      message: { role: "assistant", content: "Answer: A" },
      finish_reason: "stop",
    },
  ],
  usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
};

const CHAT_RESULT_JSON = JSON.stringify(CHAT_RESULT);

const CHAT_RESULT_WITH_RAW_USAGE = {
  ...CHAT_RESULT,
  choices: [
    {
      ...CHAT_RESULT.choices[0],
      logprobs: null,
    },
  ],
  usage: {
    ...CHAT_RESULT.usage,
    cost: 0.25,
    completion_tokens_details: { reasoning_tokens: 3 },
  },
};

const REASONING_CHAT_RESULT = {
  ...CHAT_RESULT,
  model: "openai/gpt-4o",
};

function installFetchCapture(captured: {
  value: CapturedRequest | undefined;
}): () => void {
  const original = globalThis.fetch;
  const stub: typeof fetch = async (input, init) => {
    const req = input instanceof Request ? input : new Request(input, init);
    const { url } = req;
    const rawBody = await req.clone().text();
    const body: Record<string, unknown> = parseJsonObject(rawBody);
    const headers: Record<string, string> = {};
    for (const [k, v] of req.headers.entries()) {
      headers[k.toLowerCase()] = v;
    }
    captured.value = { url, body, headers, signal: req.signal };
    return new Response(JSON.stringify(CHAT_RESULT), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  globalThis.fetch = stub;
  return () => {
    globalThis.fetch = original;
  };
}

interface CapturedHolder {
  value: CapturedRequest | undefined;
}

function newHolder(): CapturedHolder {
  return { value: undefined };
}

function parseJsonObject(raw: string): Record<string, unknown> {
  if (raw.length === 0) {
    return {};
  }
  const parsed: unknown = JSON.parse(raw);
  if (!isRecord(parsed)) {
    return {};
  }
  return parsed;
}

const MESSAGES = [{ role: MessageRole.User, content: "q" }] as const;

function modelErrorFrom(
  exit: Failure<ModelOutput, ModelError>
): ModelError | undefined {
  return getOrUndefined(failureOption(exit.cause));
}
describe("openrouter-model request parity", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });
  it("sends provider.sort on unpinned runs", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, { sort: ProviderSort.Price });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["provider"]).toEqual({ sort: "price" });
    expect(captured.value?.body["provider"]).not.toHaveProperty("ignore");
  });
  it("sends provider.only with fallbacks disabled on pinned runs", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {
          providerOnly: ["google-vertex"],
          providerIgnore: ["azure"],
          allowFallbacks: false,
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["provider"]).toEqual({
      only: ["google-vertex"],
      ignore: ["azure"],
      allow_fallbacks: false,
    });
  });
  it("merges sort with provider.only on pinned runs", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {
          sort: ProviderSort.Price,
          providerOnly: ["google-vertex"],
          providerIgnore: ["azure"],
          allowFallbacks: false,
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["provider"]).toEqual({
      sort: "price",
      only: ["google-vertex"],
      ignore: ["azure"],
      allow_fallbacks: false,
    });
  });
  it("sends provider.ignore without other provider preferences", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {
          providerIgnore: ["azure"],
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["provider"]).toEqual({
      ignore: ["azure"],
    });
  });
  it("records the chat completion generation id", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    const ids = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() =>
          gen(function* run() {
            const model = yield* Model;
            yield* model.generate(MESSAGES, {});
          })
        ),
        flatMap(() => getCollectedGenerationIds),
        provide(layer.pipe(layerProvide(FetchHttpClient.layer)))
      )
    );
    expect(ids).toEqual(["1"]);
  });
  it("records the cache source id from the response header on cache hits", async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response(CHAT_RESULT_JSON, {
        status: 200,
        headers: {
          "content-type": "application/json",
          "x-openrouter-cache-status": "HIT",
          "x-openrouter-cache-source-id": "gen-source",
        },
      })) as typeof fetch;
    try {
      const layer = makeOpenRouterModelLayer({
        model: "openai/gpt-4o",
        apiKey: "sk-test",
      });
      const entries = await runPromise(
        resetGenerationIds.pipe(
          flatMap(() =>
            gen(function* run() {
              const model = yield* Model;
              yield* model.generate(MESSAGES, {});
            })
          ),
          flatMap(() => getCollectedGenerationIdEntries),
          provide(layer.pipe(layerProvide(FetchHttpClient.layer)))
        )
      );
      expect(entries).toEqual([
        {
          id: "gen-source",
          isCacheHit: true,
          countsTowardUsage: true,
          isResolvedSource: true,
        },
      ]);
    } finally {
      globalThis.fetch = original;
    }
  });
  it("records the dummy id when a cache hit has no source id header", async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response(CHAT_RESULT_JSON, {
        status: 200,
        headers: {
          "content-type": "application/json",
          "x-openrouter-cache-status": "HIT",
        },
      })) as typeof fetch;
    try {
      const layer = makeOpenRouterModelLayer({
        model: "openai/gpt-4o",
        apiKey: "sk-test",
      });
      const entries = await runPromise(
        resetGenerationIds.pipe(
          flatMap(() =>
            gen(function* run() {
              const model = yield* Model;
              yield* model.generate(MESSAGES, {});
            })
          ),
          flatMap(() => getCollectedGenerationIdEntries),
          provide(layer.pipe(layerProvide(FetchHttpClient.layer)))
        )
      );
      expect(entries).toEqual([
        {
          id: "1",
          isCacheHit: true,
          countsTowardUsage: true,
          isResolvedSource: false,
        },
      ]);
    } finally {
      globalThis.fetch = original;
    }
  });
  it("suppresses sort when endpointId is set (pinning overrides sorting)", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {
          sort: ProviderSort.Price,
          endpointId: "ep-1",
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["provider"]).toBeUndefined();
    expect(captured.value?.headers["x-or-endpoint-id"]).toBe("ep-1");
  });
  it("serializes a video_url content part by url", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "google/gemini-2.5-flash",
      apiKey: "sk-test",
    });
    const messages = [
      {
        role: MessageRole.User,
        content: "",
        contentParts: [
          {
            type: "video_url" as const,
            videoUrl: { url: "https://cdn.seldon.global/v/clip.mp4" },
          },
          { type: "text" as const, text: "describe the video" },
        ],
      },
    ];
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(messages, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    const sentMessages = captured.value?.body["messages"] as unknown[];
    expect(sentMessages).toBeDefined();
    expect(sentMessages[0]).toEqual({
      role: "user",
      content: [
        {
          type: "video_url",
          video_url: { url: "https://cdn.seldon.global/v/clip.mp4" },
        },
        { type: "text", text: "describe the video" },
      ],
    });
  });
  it("sends reasoning_effort, maxTokens, temperature", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {
          temperature: 0,
          maxTokens: 128,
          reasoningEffort: "high",
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["temperature"]).toBe(0);
    expect(captured.value?.body["max_tokens"]).toBe(128);
    expect(captured.value?.body["reasoning_effort"]).toBe("high");
  });
  it("sends the Cloudflare version override header", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, { cloudflareVersion: "ver-9" });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(
      captured.value?.headers["cloudflare-workers-version-overrides"]
    ).toBe("ver-9");
  });
  it("sends x-session-id header when sessionId is configured", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      sessionId: "bench-run-abc123",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.headers["x-session-id"]).toBe("bench-run-abc123");
  });
  it("omits x-session-id header when sessionId is not configured", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.headers["x-session-id"]).toBeUndefined();
  });
  it("sends app attribution headers on every call", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.headers["http-referer"]).toBe(
      "https://bench-harness.openrouter.ai/"
    );
    expect(captured.value?.headers["x-openrouter-title"]).toBe(
      "OpenRouter: Bench Harness"
    );
  });
  it("always sends top-level cache_control", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["cache_control"]).toEqual({
      type: "ephemeral",
    });
  });
  it("omits provider, reasoning_effort, and headers when unset", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, { temperature: 0.5 });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["provider"]).toBeUndefined();
    expect(captured.value?.body["reasoning_effort"]).toBeUndefined();
    expect(captured.value?.headers["x-or-endpoint-id"]).toBeUndefined();
    expect(
      captured.value?.headers["cloudflare-workers-version-overrides"]
    ).toBeUndefined();
  });
  it("preserves the raw non-streaming response alongside the mapped output", async () => {
    const captured: CapturedRequest[] = [];
    restore = installFetchSequence([CHAT_RESULT_WITH_RAW_USAGE], captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(exit.value.rawResponse).toEqual(CHAT_RESULT_WITH_RAW_USAGE);
    expect(exit.value.completion).toBe("Answer: A");
    expect(exit.value.message).toEqual({
      role: MessageRole.Assistant,
      content: "Answer: A",
      model: "m",
    });
    expect(exit.value.usage).toEqual({
      inputTokens: 1,
      outputTokens: 1,
      totalTokens: 2,
      reasoningTokens: 3,
      totalCost: 0.25,
    });
  });
});
describe("openrouter-model response caching", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });
  it("always sends the response-cache header", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.headers["x-openrouter-cache"]).toBe("true");
  });
  it("sends a session- and epoch-scoped cache salt header", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      sessionId: "wf-123",
    });
    await runPromiseExit(
      gen(function* run() {
        yield* setCurrentEpoch(1);
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.headers["x-openrouter-cache-salt"]).toBe(
      "wf-123:epoch-1"
    );
    expect(captured.value?.body["cache_salt"]).toBeUndefined();
  });
  it("varies the cache salt header across epochs", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      sessionId: "wf-123",
    });
    const salts: unknown[] = [];
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* setCurrentEpoch(0);
        yield* model.generate(MESSAGES, {});
        salts.push(captured.value?.headers["x-openrouter-cache-salt"]);
        yield* setCurrentEpoch(1);
        yield* model.generate(MESSAGES, {});
        salts.push(captured.value?.headers["x-openrouter-cache-salt"]);
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(salts).toEqual(["wf-123:epoch-0", "wf-123:epoch-1"]);
  });
  it("appends the retry attempt to the cache salt header on in-process retries", async () => {
    const salts: unknown[] = [];
    const original = globalThis.fetch;
    let callCount = 0;
    const stub: typeof fetch = async (input, init) => {
      const req = input instanceof Request ? input : new Request(input, init);
      salts.push(req.headers.get("x-openrouter-cache-salt"));
      callCount += 1;
      if (callCount === 1) {
        return new Response(JSON.stringify({ error: { message: "slow" } }), {
          status: 429,
          headers: { "content-type": "application/json", "retry-after": "0" },
        });
      }
      return new Response(CHAT_RESULT_JSON, {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    };
    globalThis.fetch = stub;
    restore = () => {
      globalThis.fetch = original;
    };
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      sessionId: "wf-123",
      retry: { maxRetries: 2, baseDelayMs: 1 },
    });
    await runPromiseExit(
      gen(function* run() {
        yield* setCurrentEpoch(1);
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(salts).toEqual(["wf-123:epoch-1", "wf-123:epoch-1:attempt-1"]);
  });
  it("omits the cache salt header when session id and epoch are unset", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.headers["x-openrouter-cache-salt"]).toBeUndefined();
    expect(captured.value?.body["cache_salt"]).toBeUndefined();
  });
});
describe("openrouter-model auto-router plugin", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });
  it("sends pin_model on the auto-beta-router plugin", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openrouter/auto-beta",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, { pinModel: true });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["plugins"]).toEqual([
      { id: "auto-beta-router", pin_model: true },
    ]);
  });
  it("sends pin_model on the auto-router plugin for openrouter/auto", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openrouter/auto",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, { pinModel: true });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["plugins"]).toEqual([
      { id: "auto-router", pin_model: true },
    ]);
  });
  for (const [model, pluginId] of [
    ["openrouter/auto", "auto-router"],
    ["openrouter/auto-beta", "auto-beta-router"],
  ] as const) {
    it(`sends cost_tier on the ${pluginId} plugin`, async () => {
      const captured = newHolder();
      restore = installFetchCapture(captured);
      const layer = makeOpenRouterModelLayer({ model, apiKey: "sk-test" });
      await runPromiseExit(
        gen(function* run() {
          const modelService = yield* Model;
          yield* modelService.generate(MESSAGES, { costTier: "xhigh" });
        }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
      );
      expect(captured.value?.body["plugins"]).toEqual([
        { id: pluginId, cost_tier: "xhigh" },
      ]);
    });
  }
  it("merges pin_model and cost_quality_tradeoff into one plugin entry", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openrouter/auto-beta",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {
          pinModel: true,
          costQualityTradeoff: 8,
        });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["plugins"]).toEqual([
      { id: "auto-beta-router", cost_quality_tradeoff: 8, pin_model: true },
    ]);
  });
  it("omits plugins when no auto-router option is set", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openrouter/auto-beta",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["plugins"]).toBeUndefined();
  });
  it("omits plugins for non-auto models even when pinModel is set", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(MESSAGES, { pinModel: true });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["plugins"]).toBeUndefined();
  });
  it("echoes the served model on the returned assistant message", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openrouter/auto-beta",
      apiKey: "sk-test",
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, { pinModel: true });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(exit.value.message.model).toBe("m");
  });
  it("re-emits model on assistant messages sent back in history", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openrouter/auto-beta",
      apiKey: "sk-test",
    });
    const history = [
      { role: MessageRole.User, content: "q" },
      { role: MessageRole.Assistant, content: "a", model: "openai/gpt-4o" },
    ] as const;
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(history, { pinModel: true });
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    const sentMessages = captured.value?.body["messages"];
    const assistant = Array.isArray(sentMessages)
      ? sentMessages.find((m) => isRecord(m) && m["role"] === "assistant")
      : undefined;
    expect(
      assistant && isRecord(assistant) ? assistant["model"] : undefined
    ).toBe("openai/gpt-4o");
  });
  it("keeps history without reasoning_details in the pre-replay wire shape", async () => {
    const captured = newHolder();
    restore = installFetchCapture(captured);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    const history = [
      { role: MessageRole.User, content: "q" },
      { role: MessageRole.Assistant, content: "a" },
    ] as const;
    await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate(history, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(captured.value?.body["messages"]).toEqual([
      { role: "user", content: "q" },
      { role: "assistant", content: "a" },
    ]);
  });
  it("replays opaque reasoning_details with and without tool calls", async () => {
    const captured: CapturedRequest[] = [];
    const reasoningDetails = [
      { type: "summary", summary: "opaque" },
      { type: "future_variant", future_payload: { step: 1 } },
    ];
    const textReasoningDetails = [{ type: "summary", summary: "text" }];
    restore = installFetchSequence(
      [
        {
          ...REASONING_CHAT_RESULT,
          choices: [
            {
              ...CHAT_RESULT.choices[0],
              message: {
                role: "assistant",
                content: "tool response",
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
          ...REASONING_CHAT_RESULT,
          choices: [
            {
              ...CHAT_RESULT.choices[0],
              message: {
                role: "assistant",
                content: "text response",
                reasoning_details: textReasoningDetails,
              },
            },
          ],
        },
        REASONING_CHAT_RESULT,
      ],
      captured
    );
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    const first = await runPromise(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    const text = await runPromise(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate([MESSAGES[0], first.message], {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    await runPromise(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate([MESSAGES[0], first.message, text.message], {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    const secondMessages = captured[1]?.body["messages"];
    assert(Array.isArray(secondMessages));
    const assistant = secondMessages.find(
      (message) => isRecord(message) && message["role"] === "assistant"
    );
    assert(isRecord(assistant));
    expect(assistant).toMatchObject({
      content: "tool response",
      reasoning_details: reasoningDetails,
      tool_calls: [
        {
          id: "call_1",
          type: "function",
          function: { name: "lookup", arguments: "{}" },
        },
      ],
    });
    const thirdMessages = captured[2]?.body["messages"];
    assert(Array.isArray(thirdMessages));
    const textAssistant = thirdMessages.find(
      (message) => isRecord(message) && message["content"] === "text response"
    );
    assert(isRecord(textAssistant));
    expect(textAssistant).toMatchObject({
      content: "text response",
      reasoning_details: textReasoningDetails,
    });
    expect(text.message.reasoningDetails).toEqual(textReasoningDetails);
  });
  it("omits reasoning_details when the provider returns none", async () => {
    const captured: CapturedRequest[] = [];
    restore = installFetchSequence(
      [REASONING_CHAT_RESULT, REASONING_CHAT_RESULT],
      captured
    );
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
    });
    const first = await runPromise(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    await runPromise(
      gen(function* run() {
        const model = yield* Model;
        yield* model.generate([MESSAGES[0], first.message], {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    const secondMessages = captured[1]?.body["messages"];
    assert(Array.isArray(secondMessages));
    const assistant = secondMessages.find(
      (message) => isRecord(message) && message["role"] === "assistant"
    );
    assert(isRecord(assistant));
    expect(assistant["reasoning_details"]).toBeUndefined();
  });
});
describe("openrouter-model transient fetch retry", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });
  function installFlakyFetch(
    failTimes: number,
    failure: "network" | "5xx" | "429",
    fetchCalls: {
      count: number;
    }
  ): () => void {
    const original = globalThis.fetch;
    const stub: typeof fetch = async () => {
      fetchCalls.count += 1;
      if (fetchCalls.count <= failTimes) {
        if (failure === "network") {
          throw new TypeError("terminated");
        }
        const status = failure === "5xx" ? 503 : 429;
        const headers: Record<string, string> =
          failure === "429" ? { "retry-after": "0" } : {};
        return new Response(`http ${status}`, { status, headers });
      }
      return new Response(CHAT_RESULT_JSON, {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    };
    globalThis.fetch = stub;
    return () => {
      globalThis.fetch = original;
    };
  }
  it("retries a transient network error (terminated) then succeeds", async () => {
    const fetchCalls = { count: 0 };
    restore = installFlakyFetch(2, "network", fetchCalls);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 5 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(isSuccess(exit)).toBe(true);
    expect(fetchCalls.count).toBe(3);
  });
  it("retries a 5xx then succeeds", async () => {
    const fetchCalls = { count: 0 };
    restore = installFlakyFetch(1, "5xx", fetchCalls);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 5 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(isSuccess(exit)).toBe(true);
    expect(fetchCalls.count).toBe(2);
  });
  it("does not retry a non-retryable 4xx (400)", async () => {
    const fetchCalls = { count: 0 };
    const original = globalThis.fetch;
    globalThis.fetch = async () => {
      fetchCalls.count += 1;
      return new Response("bad request", { status: 400 });
    };
    restore = () => {
      globalThis.fetch = original;
    };
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries: 5 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    expect(isFailure(exit)).toBe(true);
    expect(fetchCalls.count).toBe(1);
  });
  it("generationTimeMs excludes retry backoff delays", async () => {
    const fetchCalls = { count: 0 };
    restore = installFlakyFetch(2, "5xx", fetchCalls);
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      retry: { baseDelayMs: 200, maxRetries: 5 },
    });
    const exit = await runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
    assertSuccess(exit);
    expect(exit.value.generationTimeMs).toBeLessThan(600);
  });
});
describe("openrouter-model 2xx error envelope", () => {
  let restore: (() => void) | undefined;
  const warnSpies: {
    mockRestore: () => void;
  }[] = [];
  afterEach(() => {
    restore?.();
    restore = undefined;
    for (const warn of warnSpies.splice(0)) {
      warn.mockRestore();
    }
  });
  function silenceWarnings(): Mock<(...args: unknown[]) => void> {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    return warn;
  }
  function installBodyFetch(
    body: string,
    bodyTimes: number,
    fetchCalls: {
      count: number;
    }
  ): () => void {
    const original = globalThis.fetch;
    globalThis.fetch = async () => {
      fetchCalls.count += 1;
      const payload = fetchCalls.count <= bodyTimes ? body : CHAT_RESULT_JSON;
      return new Response(payload, {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    };
    return () => {
      globalThis.fetch = original;
    };
  }
  function generateWithRetries(
    maxRetries: number
  ): Promise<Exit<ModelOutput, ModelError>> {
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      retry: { baseDelayMs: 0, maxRetries },
    });
    return runPromiseExit(
      gen(function* run() {
        const model = yield* Model;
        return yield* model.generate(MESSAGES, {});
      }).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
  }
  function warnContextFor(
    warn: Mock<(...args: unknown[]) => void>
  ): Record<string, unknown> {
    const call = warn.mock.calls.find(
      (args) => args[0] === "OpenRouter 2xx response did not yield a completion"
    );
    const context = call?.[1];
    assert(isRecord(context), "expected an unusable-body log record");
    return context;
  }
  it("retries a 200 error envelope with code 429 and then succeeds", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({
        error: { message: "Provider returned error", code: 429 },
      }),
      1,
      fetchCalls
    );
    const exit = await generateWithRetries(5);
    assertSuccess(exit);
    expect(fetchCalls.count).toBe(2);
  });
  it("surfaces status 429 from a persistent 200 error envelope", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({
        error: { message: "Provider returned error", code: 429 },
      }),
      Number.POSITIVE_INFINITY,
      fetchCalls
    );
    const exit = await generateWithRetries(1);
    assertFailure(exit);
    expect(modelErrorFrom(exit)?.status).toBe(429);
    expect(fetchCalls.count).toBe(2);
  });
  it("surfaces status 429 from an error envelope whose code is a numeric string", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({
        error: { message: "Provider returned error", code: "429" },
      }),
      Number.POSITIVE_INFINITY,
      fetchCalls
    );
    const exit = await generateWithRetries(1);
    assertFailure(exit);
    expect(modelErrorFrom(exit)?.status).toBe(429);
    expect(fetchCalls.count).toBe(2);
  });
  it("does not retry a 200 error envelope carrying a non-retryable code", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({ error: { message: "bad request", code: 400 } }),
      Number.POSITIVE_INFINITY,
      fetchCalls
    );
    const exit = await generateWithRetries(5);
    assertFailure(exit);
    expect(modelErrorFrom(exit)?.status).toBe(400);
    expect(fetchCalls.count).toBe(1);
  });
  it("retries an envelope whose code is not an HTTP status", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({ error: { message: "provider overloaded", code: 1000 } }),
      1,
      fetchCalls
    );
    const exit = await generateWithRetries(5);
    assertSuccess(exit);
    expect(fetchCalls.count).toBe(2);
  });
  it("decodes a 200 body that carries both choices and an error", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({ ...CHAT_RESULT, error: { code: 429 } }),
      1,
      fetchCalls
    );
    const exit = await generateWithRetries(5);
    assertSuccess(exit);
    expect(fetchCalls.count).toBe(1);
  });
  it("decodes a 200 body that omits required unused fields", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    const {
      system_fingerprint: _systemFingerprint,
      created: _created,
      object: _object,
      ...body
    } = CHAT_RESULT;
    restore = installBodyFetch(JSON.stringify(body), 1, fetchCalls);
    const exit = await generateWithRetries(5);
    assertSuccess(exit);
    expect(exit.value.completion).toBe("Answer: A");
    expect(fetchCalls.count).toBe(1);
  });
  it("treats an envelope with null choices as an error envelope", async () => {
    silenceWarnings();
    const fetchCalls = { count: 0 };
    restore = installBodyFetch(
      JSON.stringify({
        choices: null,
        error: { message: "bad request", code: 400 },
      }),
      Number.POSITIVE_INFINITY,
      fetchCalls
    );
    const exit = await generateWithRetries(5);
    assertFailure(exit);
    expect(modelErrorFrom(exit)?.status).toBe(400);
    expect(fetchCalls.count).toBe(1);
  });
  it("retries an unparseable 200 body and logs it truncated to 2000 characters", async () => {
    const warn = silenceWarnings();
    const fetchCalls = { count: 0 };
    const body = `not json ${"x".repeat(3000)}`;
    restore = installBodyFetch(body, 1, fetchCalls);
    const exit = await generateWithRetries(5);
    assertSuccess(exit);
    expect(fetchCalls.count).toBe(2);
    expect(warnContextFor(warn)["raw_body"]).toBe(`${body.slice(0, 1997)}...`);
  });
  it("retries a 200 body that fails schema validation and logs the raw body", async () => {
    const warn = silenceWarnings();
    const fetchCalls = { count: 0 };
    const body = JSON.stringify({ unexpected: true });
    restore = installBodyFetch(body, 1, fetchCalls);
    const exit = await generateWithRetries(5);
    assertSuccess(exit);
    expect(fetchCalls.count).toBe(2);
    expect(warnContextFor(warn)["raw_body"]).toBe(body);
  });
});

describe("openrouter-model response cache miss logging", () => {
  let restore: (() => void) | undefined;
  let warn: Mock<(...args: unknown[]) => void> | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
    warn?.mockRestore();
    warn = undefined;
  });
  function installCacheStatusFetch(cacheStatus: string): () => void {
    const original = globalThis.fetch;
    globalThis.fetch = async () =>
      new Response(CHAT_RESULT_JSON, {
        status: 200,
        headers: {
          "content-type": "application/json",
          "x-openrouter-cache-status": cacheStatus,
        },
      });
    return () => {
      globalThis.fetch = original;
    };
  }
  function generateOnRunAttempt(
    runAttempt: number
  ): Promise<Exit<ModelOutput, ModelError>> {
    const layer = makeOpenRouterModelLayer({
      model: "openai/gpt-4o",
      apiKey: "sk-test",
      sessionId: "wf-123",
    });
    return runPromiseExit(
      withRunAttempt(
        runAttempt,
        gen(function* run() {
          const model = yield* Model;
          return yield* model.generate(MESSAGES, {});
        })
      ).pipe(provide(layer.pipe(layerProvide(FetchHttpClient.layer))))
    );
  }
  it("warns when a retried run misses the response cache", async () => {
    warn = spyOn(console, "warn").mockImplementation(() => {});
    restore = installCacheStatusFetch("MISS");
    const exit = await generateOnRunAttempt(2);
    assertSuccess(exit);
    expect(warn).toHaveBeenCalledTimes(1);
    const [message, context] = warn.mock.calls[0] ?? [];
    expect(message).toBe("Expected response cache hit on run retry but missed");
    expect(context).toMatchObject({
      run_attempt: 2,
      cache_salt: "wf-123",
      cache_status: "MISS",
      model: "openai/gpt-4o",
    });
  });
  it("stays silent when a retried run hits the response cache", async () => {
    warn = spyOn(console, "warn").mockImplementation(() => {});
    restore = installCacheStatusFetch("HIT");
    const exit = await generateOnRunAttempt(2);
    assertSuccess(exit);
    expect(warn).not.toHaveBeenCalled();
  });
  it("stays silent on the first run attempt", async () => {
    warn = spyOn(console, "warn").mockImplementation(() => {});
    restore = installCacheStatusFetch("MISS");
    const exit = await generateOnRunAttempt(1);
    assertSuccess(exit);
    expect(warn).not.toHaveBeenCalled();
  });
});
