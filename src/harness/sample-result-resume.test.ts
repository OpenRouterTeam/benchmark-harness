import { describe, expect, it, spyOn } from "bun:test";

import { fromIterable } from "effect/Chunk";
import {
  fail as effectFail,
  flatMap as effectFlatMap,
  runPromise,
  succeed as effectSucceed,
  provide,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { mergeAll, succeed as layerSucceed } from "effect/Layer";
import { fromChunk } from "effect/Stream";

import {
  noopProgressLayer,
  noopCheckpointLayer,
} from "../../test/helpers/noop-progress-layer";
import { mcqScorer } from "../benchmarks/scorers/mcq/scorer";
import { recordGenerationId } from "../runtime/generation-ids";
import type { Sample } from "./core";
import { MessageRole, ModelError, ScoreValue } from "./core";
import { Dataset } from "./dataset";
import type { ModelService } from "./model";
import type { RunResult } from "./run";
import { runBenchmark } from "./run";
import type { SampleResultStoreService } from "./sample-result-store";
import {
  PersistedSampleOutcomeSchema,
  SampleResultStore,
} from "./sample-result-store";
import { Scorer } from "./scorer";
import type { ScorerService } from "./scorer";
import { systemMessage, chain, generate, Solver } from "./solver";

const SAMPLES: readonly Sample[] = [
  { id: "s1", input: "Q1 target B", target: { text: "B" } },
  { id: "s2", input: "Q2 target B", target: { text: "B" } },
  { id: "s3", input: "Q3 target B", target: { text: "B" } },
];
const EPOCHS = 2;
const ALL_KEYS = ["s1/0", "s1/1", "s2/0", "s2/1", "s3/0", "s3/1"];

function fakeDatasetLayer(samples: readonly Sample[]): Layer<Dataset> {
  return layerSucceed(
    Dataset,
    Dataset.of({
      stream: (opts) => {
        const start = opts?.start ?? 0;
        const end = opts?.end ?? samples.length;
        return fromChunk(fromIterable(samples.slice(start, end)));
      },
      size: effectSucceed(samples.length),
    })
  );
}

function fakeModel(): ModelService {
  return {
    generate: (messages) => {
      const userMsg =
        messages.find((m) => m.role === MessageRole.User)?.content ?? "";
      const completion = userMsg.includes("Q2") ? "Answer: A" : "Answer: B";
      return recordGenerationId(`fake-${userMsg}`).pipe(
        effectFlatMap(() =>
          effectSucceed({
            completion,
            message: { role: MessageRole.Assistant, content: completion },
            usage: {
              inputTokens: 10,
              outputTokens: 5,
              totalTokens: 15,
              totalCost: 0.001,
            },
            generationTimeMs: 100,
          })
        )
      );
    },
  };
}

interface Counters {
  solver: number;
  scorer: number;
}

interface InMemoryStore extends SampleResultStoreService {
  readonly map: Map<string, string>;
  readonly writtenKeys: string[];
}

function makeInMemoryStore(seed?: Map<string, string>): InMemoryStore {
  const map = new Map(seed ?? []);
  const writtenKeys: string[] = [];
  return {
    map,
    writtenKeys,
    read: async (key) => {
      const raw = map.get(key);
      return raw === undefined ? null : JSON.parse(raw);
    },
    write: async (key, data) => {
      writtenKeys.push(key);
      map.set(key, JSON.stringify(data));
    },
  };
}

function throwingStore(): SampleResultStoreService {
  return {
    read: async () => {
      throw new Error("read exploded");
    },
    write: async () => {
      throw new Error("write exploded");
    },
  };
}

async function runWithStore(
  store: SampleResultStoreService,
  overrides?: {
    readonly model?: ModelService;
    readonly scorer?: ScorerService;
  }
): Promise<{ result: RunResult; counters: Counters }> {
  const counters: Counters = { solver: 0, scorer: 0 };
  const model = overrides?.model ?? fakeModel();
  const innerSolver = chain(
    systemMessage("You are a helpful assistant."),
    generate(model, { temperature: 0.5 })
  );
  const countingSolver = Solver.of((state) => {
    counters.solver += 1;
    return innerSolver(state);
  });
  const baseScorer = overrides?.scorer ?? mcqScorer;
  const countingScorer: ScorerService = (state, target) => {
    counters.scorer += 1;
    return baseScorer(state, target);
  };
  const layers = mergeAll(
    fakeDatasetLayer(SAMPLES),
    layerSucceed(Solver, countingSolver),
    layerSucceed(Scorer, Scorer.of(countingScorer)),
    noopProgressLayer,
    noopCheckpointLayer,
    layerSucceed(SampleResultStore, store)
  );
  const result = await runPromise(
    runBenchmark({ epochs: EPOCHS, maxConcurrency: 2 }).pipe(provide(layers))
  );
  return { result, counters };
}

function summarize(result: RunResult): { key: string; value: string }[] {
  return result.sampleScores
    .map((s) => ({ key: `${s.sampleId}/${s.epoch}`, value: s.score.value }))
    .sort((a, b) => a.key.localeCompare(b.key));
}

let run1Store: InMemoryStore;
let run1Summary: { key: string; value: string }[];
let run1Metrics: { accuracy: number; total: number; correct: number };

describe("sample-result resume", () => {
  it("T1: fresh run evaluates all 6 and persists all 6 keys", async () => {
    run1Store = makeInMemoryStore();
    const { result, counters } = await runWithStore(run1Store);
    expect(result.sampleScores.length).toBe(6);
    expect(counters.solver).toBe(6);
    expect(counters.scorer).toBe(6);
    expect([...run1Store.map.keys()].sort()).toEqual(ALL_KEYS);
    for (const raw of run1Store.map.values()) {
      const parsed = PersistedSampleOutcomeSchema.safeParse(JSON.parse(raw));
      expect(parsed.success).toBe(true);
    }
    run1Summary = summarize(result);
    run1Metrics = {
      accuracy: result.metrics.accuracy,
      total: result.metrics.totalQuestions,
      correct: result.metrics.correctAnswers,
    };
    expect(run1Metrics.total).toBe(3);
    expect(run1Metrics.accuracy).toBeCloseTo(2 / 3, 5);
  });

  it("T2: full resume — solver/scorer never invoked, identical outcome", async () => {
    const store = makeInMemoryStore(run1Store.map);
    const { result, counters } = await runWithStore(store);
    expect(counters.solver).toBe(0);
    expect(counters.scorer).toBe(0);
    expect(summarize(result)).toEqual(run1Summary);
    expect(result.metrics.accuracy).toBeCloseTo(run1Metrics.accuracy, 5);
    expect(result.metrics.totalQuestions).toBe(run1Metrics.total);
    expect(result.metrics.correctAnswers).toBe(run1Metrics.correct);
    expect(store.writtenKeys).toEqual([]);
  });

  it("T3: partial resume — only the 4 missing keys are evaluated", async () => {
    const seeded = new Map(
      [...run1Store.map.entries()].filter(([k]) => ["s1/0", "s2/1"].includes(k))
    );
    const store = makeInMemoryStore(seeded);
    const { result, counters } = await runWithStore(store);
    expect(counters.solver).toBe(4);
    expect(counters.scorer).toBe(4);
    expect([...store.writtenKeys].sort()).toEqual([
      "s1/1",
      "s2/0",
      "s3/0",
      "s3/1",
    ]);
    expect(summarize(result)).toEqual(run1Summary);
    expect(result.metrics.accuracy).toBeCloseTo(run1Metrics.accuracy, 5);
  });

  it("T4: store whose read/write throw — run completes, all evaluated, failures logged", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    try {
      const { result, counters } = await runWithStore(throwingStore());
      expect(counters.solver).toBe(6);
      expect(counters.scorer).toBe(6);
      expect(result.sampleScores.length).toBe(6);
      expect(summarize(result)).toEqual(run1Summary);
      expect(result.metrics.accuracy).toBeCloseTo(run1Metrics.accuracy, 5);
      const warned = warn.mock.calls.map((call) =>
        call.map((arg) => JSON.stringify(arg)).join(" ")
      );
      expect(
        warned.some((line) =>
          line.includes("Failed to read persisted sample outcome")
        )
      ).toBe(true);
      expect(
        warned.some((line) => line.includes("Failed to persist sample outcome"))
      ).toBe(true);
      expect(warned.some((line) => line.includes("read exploded"))).toBe(true);
      expect(warned.some((line) => line.includes("write exploded"))).toBe(true);
    } finally {
      warn.mockRestore();
    }
  });

  it("T5: degraded error outcomes are not persisted and are re-evaluated on resume", async () => {
    const failingModel: ModelService = {
      generate: (messages) => {
        const userMsg =
          messages.find((m) => m.role === MessageRole.User)?.content ?? "";
        if (userMsg.includes("Q1")) {
          return effectFail(
            new ModelError({ message: "OpenRouter HTTP 429", status: 429 })
          );
        }
        return effectSucceed({
          completion: "Answer: B",
          message: { role: MessageRole.Assistant, content: "Answer: B" },
          generationTimeMs: 100,
        });
      },
    };
    const store = makeInMemoryStore();
    const run1 = await runWithStore(store, { model: failingModel });
    const skippedRun1 = run1.result.sampleScores.filter(
      (s) => s.sampleId === "s1"
    );
    expect(skippedRun1.every((s) => s.score.value === ScoreValue.Skipped)).toBe(
      true
    );
    expect([...store.map.keys()].sort()).toEqual([
      "s2/0",
      "s2/1",
      "s3/0",
      "s3/1",
    ]);

    const run2 = await runWithStore(store);
    expect(run2.counters.solver).toBe(2);
    expect(run2.counters.scorer).toBe(2);
    const s1Run2 = run2.result.sampleScores.filter((s) => s.sampleId === "s1");
    expect(s1Run2.every((s) => s.score.value === ScoreValue.Correct)).toBe(
      true
    );
    expect([...store.map.keys()].sort()).toEqual(ALL_KEYS);
  });

  it("T7: unparseable persisted record is logged and re-evaluated", async () => {
    const seeded = new Map(run1Store.map);
    const corruptRaw = seeded.get("s1/0");
    if (corruptRaw === undefined) {
      throw new Error("expected s1/0 in run1 store");
    }
    const corrupt: unknown = JSON.parse(corruptRaw);
    if (typeof corrupt !== "object" || corrupt === null) {
      throw new Error("expected persisted record to be an object");
    }
    seeded.set(
      "s1/0",
      JSON.stringify({ ...corrupt, usage: { inputTokens: 10.5 } })
    );
    const store = makeInMemoryStore(seeded);
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    try {
      const { result, counters } = await runWithStore(store);
      expect(counters.solver).toBe(1);
      expect(counters.scorer).toBe(1);
      expect(summarize(result)).toEqual(run1Summary);
      expect(store.writtenKeys).toEqual(["s1/0"]);
      const warned = warn.mock.calls.map((call) =>
        call.map((arg) => JSON.stringify(arg)).join(" ")
      );
      expect(
        warned.some(
          (line) =>
            line.includes("Persisted sample outcome failed validation") &&
            line.includes("s1/0") &&
            line.includes("inputTokens")
        )
      ).toBe(true);
    } finally {
      warn.mockRestore();
    }
  });

  it("T6: scorer trajectory survives persistence and resume", async () => {
    const trajectoryScorer: ScorerService = (state, target) =>
      mcqScorer(state, target).pipe(
        effectFlatMap((score) =>
          effectSucceed({
            ...score,
            trajectory: {
              kind: "verifier_log" as const,
              log: "pytest: 12 passed",
            },
          })
        )
      );
    const store = makeInMemoryStore();
    await runWithStore(store, { scorer: trajectoryScorer });

    const resumed = await runWithStore(store);
    expect(resumed.counters.solver).toBe(0);
    expect(
      resumed.result.sampleScores.every(
        (s) =>
          s.score.trajectory?.kind === "verifier_log" &&
          s.score.trajectory.log === "pytest: 12 passed"
      )
    ).toBe(true);
  });
});
