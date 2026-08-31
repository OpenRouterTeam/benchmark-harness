import { describe, expect, it } from "bun:test";
import { readFile } from "node:fs/promises";

import { FetchHttpClient } from "@effect/platform";
import {
  either,
  gen,
  provide,
  runPromise,
  succeed as effectSucceed,
} from "effect/Effect";
import { provide as layerProvide, succeed } from "effect/Layer";
import { empty } from "effect/Stream";

import { ScoreValue } from "../../../harness/core";
import { Dataset } from "../../../harness/dataset";
import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../../harness/progress";
import { Solver } from "../../../harness/solver";
import { ProviderSort, WebSearchEngine } from "../../../internal/enums";
import type { BenchmarkRunInput } from "../../types";
import {
  makeSearchBenchmarkLayer,
  searchSolverOptionsFromConfig,
} from "./benchmark";
import { buildSearchRequestBody } from "./request";
describe("searchSolverOptionsFromConfig", () => {
  const baseConfig = {
    benchmarkId: "search_hle",
    model: "model",
    lane: { webSearch: "server-tool", engine: "auto" },
  } as const;

  it("projects every shared search inference option", () => {
    const config = {
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4-nano",
      endpointId: "endpoint",
      lane: {
        webSearch: "server-tool",
        engine: WebSearchEngine.Exa,
        maxAgentTurns: 3,
      },
      temperature: 0.2,
      maxTokens: 123,
      reasoningEffort: "high",
      costTier: "xhigh",
      costQualityTradeoff: 4,
      timeoutMs: 456,
      sort: ProviderSort.Latency,
      providerOrder: ["openai", "azure"],
      providerOnly: ["openai", "azure"],
      providerIgnore: ["bedrock"],
      allowFallbacks: false,
      cloudflareVersion: "worker-version",
    } as const;
    const options = searchSolverOptionsFromConfig({
      config,
      instructions: "instructions",
      temperature: 0,
      retry: {
        maxRetries: 2,
        baseDelayMs: 3,
      },
    });
    expect(options).toEqual({
      model: "openai/gpt-5.4-nano",
      instructions: "instructions",
      lane: { webSearch: "server-tool", engine: "exa", maxAgentTurns: 3 },
      maxOutputTokens: 123,
      temperature: 0.2,
      reasoningEffort: "high",
      costTier: "xhigh",
      costQualityTradeoff: 4,
      timeoutMs: 456,
      endpointId: "endpoint",
      sort: "latency",
      providerOrder: ["openai", "azure"],
      providerOnly: ["openai", "azure"],
      providerIgnore: ["bedrock"],
      allowFallbacks: false,
      versionOverride: "worker-version",
      retry: { maxRetries: 2, baseDelayMs: 3 },
    });
    expect(
      buildSearchRequestBody({ ...options, problem: "Q?" }).temperature
    ).toBe(0.2);
  });
  it("uses the benchmark-declared temperature when the config omits an override", () => {
    const options = searchSolverOptionsFromConfig({
      config: baseConfig,
      instructions: "instructions",
      temperature: 0,
      maxOutputTokens: 999,
    });
    expect(options.maxOutputTokens).toBe(999);
    expect(options.temperature).toBe(0);
    expect(
      buildSearchRequestBody({ ...options, problem: "Q?" }).temperature
    ).toBe(0);
  });

  it("clamps the default output tokens to the supplied ceiling", () => {
    const options = searchSolverOptionsFromConfig({
      config: baseConfig,
      instructions: "instructions",
      temperature: 0,
      maxOutputTokensCeiling: 32768,
    });

    expect(options.maxOutputTokens).toBe(32768);
  });

  it("clamps configured output tokens to the supplied ceiling", () => {
    const options = searchSolverOptionsFromConfig({
      config: { ...baseConfig, maxTokens: 64000 },
      instructions: "instructions",
      temperature: 0,
      maxOutputTokensCeiling: 16000,
    });

    expect(options.maxOutputTokens).toBe(16000);
  });

  it("preserves configured output tokens below the supplied ceiling", () => {
    const options = searchSolverOptionsFromConfig({
      config: { ...baseConfig, maxTokens: 8000 },
      instructions: "instructions",
      temperature: 0,
      maxOutputTokensCeiling: 16000,
    });

    expect(options.maxOutputTokens).toBe(8000);
  });

  it("preserves output tokens when no ceiling is supplied", () => {
    const options = searchSolverOptionsFromConfig({
      config: baseConfig,
      instructions: "instructions",
      temperature: 0,
      maxOutputTokens: 999,
    });

    expect(options.maxOutputTokens).toBe(999);
  });
});

describe("makeSearchBenchmarkLayer", () => {
  it("forwards trace headers to inference requests", async () => {
    const stream = await readFile(
      new URL(
        "../../../../test/fixtures/advisor-responses-stream.sse",
        import.meta.url
      ),
      "utf8"
    );
    const originalFetch = globalThis.fetch;
    let capturedHeaders: Headers | undefined;
    globalThis.fetch = async (fetchInput, init) => {
      const request =
        fetchInput instanceof Request
          ? fetchInput
          : new Request(fetchInput, init);
      capturedHeaders = request.headers;
      return new Response(stream, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    };
    try {
      const input: BenchmarkRunInput = {
        apiKey: "sk-test",
        baseUrl: "https://example.test",
        sessionId: "session-1",
        benchmarkConfig: {
          benchmarkId: "search_hle",
          model: "openai/gpt-5",
          lane: { webSearch: "server-tool", engine: "auto" },
        },
        traceHeaders: {
          traceparent:
            "00-11111111111111111111111111111111-2222222222222222-01",
          "x-or-traceparent":
            "00-11111111111111111111111111111111-2222222222222222-01",
          "x-benchmark-trace": "bench-key",
          authorization: "Bearer attacker",
        },
      };
      const layer = makeSearchBenchmarkLayer(input, {
        benchmarkId: "search_hle",
        instructions: "instructions",
        temperature: 0,
        makeDatasetLayer: () =>
          succeed(Dataset, {
            stream: () => empty,
            size: effectSucceed(0),
          }),
        makeSolver: (responses) => (state) =>
          gen(function* () {
            yield* either(
              responses.send({ model: "m", input: [] }, { timeoutMs: 1000 })
            );
            return { ...state, completed: true };
          }),
        scorer: () =>
          effectSucceed({
            value: ScoreValue.Correct,
            answer: "",
            explanation: "",
          }),
      });
      await runPromise(
        gen(function* () {
          const solver = yield* Solver;
          yield* solver({
            sample: { id: "s1", input: "Q?", target: { text: "A" } },
            messages: [],
            completed: false,
          });
        }).pipe(
          provide(layer.pipe(layerProvide(FetchHttpClient.layer))),
          provide(succeed(ProgressReporter, NOOP_PROGRESS_REPORTER)),
          provide(succeed(CheckpointStore, NOOP_CHECKPOINT_STORE))
        )
      );
      expect(capturedHeaders?.get("traceparent")).toBe(
        "00-11111111111111111111111111111111-2222222222222222-01"
      );
      expect(capturedHeaders?.get("x-or-traceparent")).toBe(
        "00-11111111111111111111111111111111-2222222222222222-01"
      );
      expect(capturedHeaders?.get("x-benchmark-trace")).toBe("bench-key");
      expect(capturedHeaders?.get("authorization")).toBe("Bearer sk-test");
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
});
