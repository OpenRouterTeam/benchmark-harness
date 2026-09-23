import { afterEach, describe, expect, it } from "bun:test";

import { blockNetwork } from "../../test/helpers/block-network";
import { NOOP_PROGRESS_REPORTER } from "../harness/progress";
import { assertRight, assertLeft } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import { runBenchmarkById } from "../runner/run-by-id";
import {
  BenchmarkRunConfigSchema,
  isSearchBenchmarkConfig,
} from "./benchmark-config";
import { getBenchmarkMeta } from "./benchmark-meta";
import { benchmarkIds, getBenchmark } from "./registry";
describe("benchmark registry", () => {
  let restoreNetwork: (() => void) | undefined;
  afterEach(() => {
    restoreNetwork?.();
    restoreNetwork = undefined;
  });

  it("resolves gpqa_diamond with a complete definition", () => {
    const b = getBenchmark("gpqa_diamond");
    expect(b).toBeDefined();
    expect(b?.id).toBe("gpqa_diamond");
    expect(b?.temperature).toBe(0.5);
    expect(b?.defaultEpochs).toBe(10);
    expect(typeof b?.makeLayer).toBe("function");
    expect(typeof b?.makeDatasetLayer).toBe("function");
  });

  it("dispatches runBenchmarkById through the registry entry", async () => {
    restoreNetwork = blockNetwork();
    const result = await runBenchmarkById({
      benchmarkId: "gpqa_diamond",
      apiKey: "unused",
      benchmarkConfig: {
        benchmarkId: "gpqa_diamond",
        model: "test/model",
        reasoningEffort: "high",
      },
      epochs: 1,
      maxConcurrency: 1,
      range: { start: 0, end: 1 },
      datasetRetry: { baseDelayMs: 0 },
      sessionId: "test",
      progressReporter: NOOP_PROGRESS_REPORTER,
    });
    assertLeft(result);
  });

  it("registers search_hle with the default search lane", () => {
    const benchmark = getBenchmark("search_hle");
    expect(benchmark?.id).toBe("search_hle");
    expect(benchmark?.defaultEpochs).toBe(1);
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
    });
    assertRight(config);
    expect(config.right).toEqual({
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
      lane: { webSearch: "server-tool", engine: "auto" },
    });
  });

  it("registers search_dsqa with workflow metadata and the default search lane", () => {
    const benchmark = getBenchmark("search_dsqa");
    expect(benchmark?.id).toBe("search_dsqa");
    expect(benchmark?.defaultEpochs).toBe(1);
    expect(typeof benchmark?.runLevelScores).toBe("function");
    expect(typeof benchmark?.primaryScore).toBe("function");
    const config = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "search_dsqa",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
    });
    assertRight(config);
    expect(config.right).toEqual({
      benchmarkId: "search_dsqa",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
      lane: { webSearch: "server-tool", engine: "auto" },
    });
  });

  it("narrows parsed benchmark configs to the search family", () => {
    expect(
      isSearchBenchmarkConfig({
        benchmarkId: "search_dsqa",
        model: "model",
        reasoningEffort: "high",
        lane: { webSearch: "server-tool", engine: "auto" },
      })
    ).toBe(true);
    expect(
      isSearchBenchmarkConfig({ benchmarkId: "gpqa_diamond", model: "model" })
    ).toBe(false);
  });

  it("registers WideSearch with the default search lane", () => {
    const benchmark = getBenchmark("search_widesearch");
    expect(benchmark?.id).toBe("search_widesearch");
    expect(benchmark?.defaultEpochs).toBe(1);
    expect(typeof benchmark?.runLevelScores).toBe("function");
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "search_widesearch",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
    });
    assertRight(result);
    expect(result.right).toEqual({
      benchmarkId: "search_widesearch",
      model: "openai/gpt-5.4-nano",
      reasoningEffort: "high",
      lane: { webSearch: "server-tool", engine: "auto" },
    });
  });

  it("returns undefined for an unknown benchmark", () => {
    expect(getBenchmark("does_not_exist")).toBeUndefined();
  });

  it("lists registered benchmark ids", () => {
    expect(benchmarkIds()).toContain("gpqa_diamond");
    expect(benchmarkIds()).toContain("draco");
  });

  it("meta mirrors registry id + defaultEpochs for every benchmark", () => {
    for (const id of benchmarkIds()) {
      const b = getBenchmark(id);
      const meta = getBenchmarkMeta(id);
      expect(meta).toBeDefined();
      expect(meta?.id).toBe(b?.id);
      expect(meta?.defaultEpochs).toBe(b?.defaultEpochs);
    }
  });

  it("meta and registry agree on the registered id set", () => {
    const metaIds = benchmarkIds().filter(
      (id) => getBenchmarkMeta(id) !== undefined
    );
    expect(metaIds).toEqual([...benchmarkIds()]);
  });
});
