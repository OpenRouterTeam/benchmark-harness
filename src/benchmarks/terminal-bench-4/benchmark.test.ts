import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { BenchmarkRunConfigSchema } from "../benchmark-config";
import { getBenchmarkMeta, TERMINAL_BENCH_4_META } from "../benchmark-meta";
import { getBenchmark } from "../registry";
import { TERMINAL_BENCH_BENCHMARK } from "../terminal-bench/benchmark";
import { TERMINAL_BENCH_4_BENCHMARK } from "./benchmark";
import { DEFAULT_TERMINAL_BENCH_4_IMAGE_REPO, imageTags } from "./images";
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

  it("parses a run config with the default image repo and agent", () => {
    const result = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "terminal_bench_4",
      model: "anthropic/claude-opus-5",
      reasoningEffort: "high",
      agentReasoningEffort: "high",
    });
    assertRight(result);
    expect(result.right.benchmarkId).toBe("terminal_bench_4");
    expect(
      result.right.benchmarkId === "terminal_bench_4" && result.right.imageRepo
    ).toBe(DEFAULT_TERMINAL_BENCH_4_IMAGE_REPO);
    expect(
      result.right.benchmarkId === "terminal_bench_4" && result.right.agent
    ).toBe("pi");
  });
});

describe("terminal-bench-4 image tags", () => {
  it("derives agent and verifier tags from the repo, task id and pinned commit", () => {
    expect(imageTags("ghcr.io/acme/tb4", "hello-world")).toEqual({
      agent: `ghcr.io/acme/tb4/hello-world:${TERMINAL_BENCH_4_SOURCE_COMMIT.slice(0, 12)}`,
      verifier: `ghcr.io/acme/tb4/hello-world-verifier:${TERMINAL_BENCH_4_SOURCE_COMMIT.slice(0, 12)}`,
    });
  });
});
