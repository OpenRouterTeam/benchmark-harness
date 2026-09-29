import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";

import { assertLeft, assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import {
  BenchmarkRunConfigSchema,
  InjectedBenchmarkRunConfigSchema,
  isInjectedBenchmarkConfig,
  isModelBenchmarkConfig,
  isSearchBenchmarkConfig,
  KeplerBenchmarkRunConfigSchema,
} from "./benchmark-config";

describe("benchmark config", () => {
  it("loads with frozen globals in a workflow sandbox", () => {
    const result = spawnSync(
      process.execPath,
      ["-e", 'Object.freeze(Error); await import("./benchmark-config.ts");'],
      { cwd: import.meta.dirname, encoding: "utf8" }
    );
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
  });

  it("keeps Kepler configs precise while parsing them through the Kepler schema", () => {
    const result = parseSchema(KeplerBenchmarkRunConfigSchema, {
      benchmarkId: "search_hle",
      model: "openai/gpt-5.4",
      reasoningEffort: "high",
    });

    assertRight(result);
    expect(result.right.benchmarkId).toBe("search_hle");
    expect(result.right.lane).toEqual({
      webSearch: "server-tool",
      engine: "auto",
    });
    expect(isSearchBenchmarkConfig(result.right)).toBe(true);
    expect(isModelBenchmarkConfig(result.right)).toBe(true);
  });

  it("does not let malformed Kepler configs fall through to the injected variant", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "gpqa_diamond",
      model: 42,
    });

    assertLeft(result);
  });

  it("requires reasoningEffort in model benchmark configs", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "gpqa_diamond",
      model: "openai/gpt-5",
    });

    assertLeft(result);
  });

  it("accepts the explicit auto reasoning effort", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "gpqa_diamond",
      model: "openrouter/jev",
      reasoningEffort: "auto",
    });

    assertRight(result);
    expect(result.right.reasoningEffort).toBe("auto");
  });

  it("rejects switchyardAlgorithm on a non-switchyard model", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "gpqa_diamond",
      model: "openai/gpt-5",
      reasoningEffort: "high",
      switchyardAlgorithm: "stage",
    });

    assertLeft(result);
    expect(result.left.issues.map((issue) => issue.path)).toEqual([
      ["switchyardAlgorithm"],
    ]);
  });

  it("accepts switchyardAlgorithm on switchyard variants and omitted algorithms elsewhere", () => {
    for (const model of ["nvidia/switchyard", "nvidia/switchyard:online"]) {
      assertRight(
        parseSchema(BenchmarkRunConfigSchema, {
          benchmarkId: "search_hle",
          model,
          reasoningEffort: "high",
          switchyardAlgorithm: "stage",
        })
      );
    }
    assertRight(
      parseSchema(BenchmarkRunConfigSchema, {
        benchmarkId: "injected_benchmark",
        model: "openai/gpt-5",
        reasoningEffort: "high",
      })
    );
  });

  it("parses injected benchmark configs with opaque options", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "injected_benchmark",
      model: "injected/model",
      reasoningEffort: "high",
      options: {
        subsets: ["all"],
        customFlag: true,
      },
    });

    assertRight(result);
    expect(result.right).toEqual({
      benchmarkId: "injected_benchmark",
      model: "injected/model",
      reasoningEffort: "high",
      options: {
        subsets: ["all"],
        customFlag: true,
      },
    });
    expect(isInjectedBenchmarkConfig(result.right)).toBe(true);
    expect(isModelBenchmarkConfig(result.right)).toBe(true);
    expect(isSearchBenchmarkConfig(result.right)).toBe(false);
  });

  it("defaults injected benchmark options to an empty object", () => {
    const result = parseSchema(InjectedBenchmarkRunConfigSchema, {
      benchmarkId: "injected_benchmark",
      model: "injected/model",
      reasoningEffort: "high",
    });

    assertRight(result);
    expect(result.right.options).toEqual({});
  });

  it("rejects an injected config that reuses a Kepler benchmark id", () => {
    const result = parseSchema(InjectedBenchmarkRunConfigSchema, {
      benchmarkId: "gpqa_diamond",
      model: "injected/model",
      reasoningEffort: "high",
      options: { subsets: ["all"] },
    });

    assertLeft(result);
  });
});
