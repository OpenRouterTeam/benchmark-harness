import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { BenchmarkRunConfigSchema } from "../benchmark-config";
import { getBenchmarkMeta, TERMINAL_BENCH_4_META } from "../benchmark-meta";
import { getBenchmark } from "../registry";
import { TERMINAL_BENCH_BENCHMARK } from "../terminal-bench/benchmark";
import { TERMINAL_BENCH_4_BENCHMARK } from "./benchmark";
import { buildImageMap, TERMINAL_BENCH_4_IMAGES, taskImages } from "./images";
import { TERMINAL_BENCH_4_SOURCE_COMMIT } from "./tasks-source";

describe("terminal-bench-4 registry wiring", () => {
  it("registers a benchmark distinct from terminal-bench 2.1", () => {
    expect(TERMINAL_BENCH_4_BENCHMARK.id).toBe("terminal_bench_4");
    expect(TERMINAL_BENCH_BENCHMARK.id).toBe("terminal_bench");
    expect(getBenchmark("terminal_bench_4")).toBe(TERMINAL_BENCH_4_BENCHMARK);
    expect(getBenchmark("terminal_bench")).toBe(TERMINAL_BENCH_BENCHMARK);
  });

  it("agrees with its metadata on id, epochs and temperature", () => {
    expect(TERMINAL_BENCH_4_META.id).toBe(TERMINAL_BENCH_4_BENCHMARK.id);
    expect(TERMINAL_BENCH_4_BENCHMARK.defaultEpochs).toBe(
      TERMINAL_BENCH_4_META.defaultEpochs
    );
    expect(getBenchmarkMeta(TERMINAL_BENCH_4_META.id)).toBe(
      TERMINAL_BENCH_4_META
    );
    expect(TERMINAL_BENCH_4_BENCHMARK.temperature).toBe(0);
    expect(TERMINAL_BENCH_4_BENCHMARK.degradeSolverErrors).toBe(true);
  });

  it("parses a run config with the default agent", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench_4",
      model: "anthropic/claude-opus-5",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    expect(result.right.benchmarkId).toBe("terminal_bench_4");
    expect(
      result.right.benchmarkId === "terminal_bench_4" && result.right.agent
    ).toBe("pi");
  });
});

describe("terminal-bench-4 image map", () => {
  const raw = {
    sourceCommit: TERMINAL_BENCH_4_SOURCE_COMMIT,
    images: {
      "hello-world": { agent: "im-abc123", verifier: "im-def456" },
    },
  };

  it("exposes Modal image ids per task and undefined for unknown tasks", () => {
    const map = buildImageMap(raw);
    expect(taskImages(map, "hello-world")).toEqual({
      agent: "im-abc123",
      verifier: "im-def456",
    });
    expect(taskImages(map, "missing")).toBeUndefined();
  });

  it("rejects a map built from a different source commit", () => {
    expect(() =>
      buildImageMap({ ...raw, sourceCommit: "0".repeat(40) })
    ).toThrow(/pinned to/);
  });

  it("rejects ids that are not Modal image ids", () => {
    expect(() =>
      buildImageMap({
        ...raw,
        images: { x: { agent: "ghcr.io/x:y", verifier: "im-1" } },
      })
    ).toThrow(/invalid/);
  });

  it("ships a committed map for the pinned commit", () => {
    expect(TERMINAL_BENCH_4_IMAGES.sourceCommit).toBe(
      TERMINAL_BENCH_4_SOURCE_COMMIT
    );
    expect(TERMINAL_BENCH_4_IMAGES.images.size).toBeGreaterThan(0);
  });
});
