import { describe, expect, it } from "bun:test";

import { fromIterable } from "effect/Chunk";
import {
  fail as effectFail,
  orDie,
  promise as effectPromise,
  provide,
  runPromise,
  succeed as effectSucceed,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { mergeAll, succeed as layerSucceed } from "effect/Layer";
import { fromChunk } from "effect/Stream";

import {
  noopProgressLayer,
  noopCheckpointLayer,
} from "../../test/helpers/noop-progress-layer";
import { mcqScorer } from "../benchmarks/scorers/mcq/scorer";
import type { ModelOutput, Sample } from "./core";
import { MessageRole, ModelError, ScoreValue, SolverError } from "./core";
import { Dataset } from "./dataset";
import type { ModelService } from "./model";
import { Model } from "./model";
import type { CheckpointStore, ProgressReporter } from "./progress";
import { runBenchmark } from "./run";
import type {
  CompletedSampleEntry,
  SampleResultStoreService,
} from "./sample-result-store";
import { SampleResultStore } from "./sample-result-store";
import { Scorer } from "./scorer";
import { generate, Solver } from "./solver";

const SAMPLES: readonly Sample[] = [
  { id: "s-0", input: "Q0 target B", target: { text: "B" } },
  { id: "s-1", input: "Q1 target B", target: { text: "B" } },
  { id: "s-2", input: "Q2 target B", target: { text: "B" } },
];

const MODEL_RESPONSE: ModelOutput = {
  completion: "Answer: B",
  message: { role: MessageRole.Assistant, content: "Answer: B" },
  usage: {
    inputTokens: 10,
    outputTokens: 5,
    totalTokens: 15,
    reasoningTokens: 0,
    totalCost: 0.001,
  },
  generationTimeMs: 100,
};

const ANSWERING_MODEL: ModelService = {
  generate: () => effectSucceed(MODEL_RESPONSE),
};

/** Records every write and replays a seeded set of prior records. */
function recordingStore(seed: readonly CompletedSampleEntry[]): {
  readonly service: SampleResultStoreService;
  readonly writes: CompletedSampleEntry[];
} {
  const writes: CompletedSampleEntry[] = [];
  return {
    writes,
    service: {
      write: async (entry) => {
        writes.push(entry);
      },
      list: async (range) =>
        seed.filter(
          (entry) =>
            (range.start === undefined || entry.sampleIndex >= range.start) &&
            (range.end === undefined || entry.sampleIndex < range.end)
        ),
    },
  };
}

function completedEntry(
  sampleIndex: number,
  epoch: number
): CompletedSampleEntry {
  return {
    sampleIndex,
    epoch,
    sampleScore: {
      sampleId: `s-${sampleIndex}`,
      epoch,
      score: { value: ScoreValue.Correct, answer: "B", explanation: "stored" },
    },
    usage: {
      inputTokens: 10,
      outputTokens: 5,
      totalTokens: 15,
      reasoningTokens: 0,
      totalCost: 0.001,
    },
    generationTimeMs: 100,
  };
}

function makeLayers(
  store: SampleResultStoreService,
  model: ModelService = ANSWERING_MODEL
): Layer<
  | Dataset
  | Solver
  | Scorer
  | Model
  | SampleResultStore
  | ProgressReporter
  | CheckpointStore
> {
  return mergeAll(
    layerSucceed(
      Dataset,
      Dataset.of({
        stream: (opts) =>
          fromChunk(
            fromIterable(
              SAMPLES.slice(opts?.start ?? 0, opts?.end ?? SAMPLES.length)
            )
          ),
        size: effectSucceed(SAMPLES.length),
      })
    ),
    layerSucceed(Solver, Solver.of(generate(model, { temperature: 0 }))),
    layerSucceed(Scorer, Scorer.of(mcqScorer)),
    layerSucceed(Model, Model.of(model)),
    layerSucceed(SampleResultStore, store),
    noopProgressLayer,
    noopCheckpointLayer
  );
}

/** Resolve once `condition` holds, so a test can observe a write mid-run. */
async function waitFor(condition: () => boolean): Promise<void> {
  while (!condition()) {
    // oxlint-disable-next-line eslint/no-await-in-loop -- polling an in-flight run
    await new Promise((resolve) => {
      setTimeout(resolve, 5);
    });
  }
}

describe("runBenchmark sample-result resume", () => {
  it("persists one record per completed (sample, epoch)", async () => {
    const store = recordingStore([]);

    const result = await runPromise(
      runBenchmark({ epochs: 2, maxConcurrency: 2 }).pipe(
        provide(makeLayers(store.service))
      )
    );

    expect(result.sampleScores).toHaveLength(6);
    expect(
      store.writes
        .map((w) => `${w.sampleIndex}:${w.epoch}`)
        .toSorted((a, b) => a.localeCompare(b))
    ).toEqual(["0:0", "0:1", "1:0", "1:1", "2:0", "2:1"]);
    expect(store.writes[0]?.usage?.totalCost).toBe(0.001);
    expect(store.writes[0]?.generationTimeMs).toBe(100);
  });

  it("skips stored (sample, epoch) pairs and reproduces the uninterrupted aggregate", async () => {
    const fresh = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(
        provide(makeLayers(recordingStore([]).service))
      )
    );

    const resumed = recordingStore([
      completedEntry(0, 0),
      completedEntry(1, 0),
    ]);
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(
        provide(makeLayers(resumed.service))
      )
    );

    expect(resumed.writes.map((w) => w.sampleIndex)).toEqual([2]);
    expect(result.metrics).toEqual(fresh.metrics);
    expect(result.usage).toEqual(fresh.usage);
    expect(result.sampleScores.map((s) => s.sampleId).toSorted()).toEqual([
      "s-0",
      "s-1",
      "s-2",
    ]);
  });

  it("keys stored records on the absolute sample index when running a chunk range", async () => {
    const resumed = recordingStore([completedEntry(1, 0)]);

    const result = await runPromise(
      runBenchmark({
        epochs: 1,
        maxConcurrency: 1,
        range: { start: 1, end: 3 },
      }).pipe(provide(makeLayers(resumed.service)))
    );

    expect(resumed.writes.map((w) => w.sampleIndex)).toEqual([2]);
    expect(result.metrics.totalQuestions).toBe(2);
  });

  it("ignores stored records whose epoch is outside the configured epoch count", async () => {
    const resumed = recordingStore([completedEntry(0, 5)]);

    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(
        provide(makeLayers(resumed.service))
      )
    );

    expect(resumed.writes.map((w) => w.sampleIndex)).toEqual([0, 1, 2]);
    expect(result.sampleScores).toHaveLength(3);
  });

  it("records scores synthesized from an exhausted model error as degraded, at the end of the run", async () => {
    const store = recordingStore([]);
    const rateLimitedModel: ModelService = {
      generate: () =>
        effectFail(
          new ModelError({
            message: "OpenRouter HTTP 429: rate-limited",
            status: 429,
          })
        ),
    };

    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 1 }).pipe(
        provide(makeLayers(store.service, rateLimitedModel))
      )
    );

    expect(result.metrics.skippedQuestions).toBe(3);
    expect(store.writes).toHaveLength(3);
    expect(store.writes.every((w) => w.degraded === true)).toBe(true);
  });

  it("writes a degraded record after every non-degraded write of the run", async () => {
    const store = recordingStore([]);
    const flakyFirstSampleModel: ModelService = {
      generate: (messages) => {
        const userMsg =
          messages.find((m) => m.role === MessageRole.User)?.content ?? "";
        if (typeof userMsg === "string" && userMsg.includes("Q0")) {
          return effectFail(
            new ModelError({
              message: "OpenRouter HTTP 429: rate-limited",
              status: 429,
            })
          );
        }
        return effectSucceed(MODEL_RESPONSE);
      },
    };

    await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 1 }).pipe(
        provide(makeLayers(store.service, flakyFirstSampleModel))
      )
    );

    expect(store.writes.map((w) => w.degraded === true)).toEqual([
      false,
      false,
      true,
    ]);
    expect(store.writes.at(-1)?.sampleIndex).toBe(0);
  });

  it("re-runs samples whose stored record is degraded", async () => {
    const degradedSeed: CompletedSampleEntry = {
      ...completedEntry(0, 0),
      degraded: true,
      sampleScore: {
        sampleId: "s-0",
        epoch: 0,
        score: {
          value: ScoreValue.Skipped,
          answer: null,
          explanation: "Model error (skipped)",
        },
      },
    };
    const resumed = recordingStore([degradedSeed, completedEntry(1, 0)]);

    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(
        provide(makeLayers(resumed.service))
      )
    );

    expect(resumed.writes.map((w) => w.sampleIndex).toSorted()).toEqual([0, 2]);
    expect(result.metrics.skippedQuestions).toBe(0);
    expect(result.metrics.totalQuestions).toBe(3);
  });

  it("scores a degraded solver error as Skipped, out of the accuracy denominator", async () => {
    const store = recordingStore([]);
    const layers = mergeAll(
      layerSucceed(
        Dataset,
        Dataset.of({
          stream: () => fromChunk(fromIterable(SAMPLES.slice(0, 1))),
          size: effectSucceed(1),
        })
      ),
      layerSucceed(
        Solver,
        Solver.of(() =>
          effectFail(new SolverError({ message: "sandbox exec failed" }))
        )
      ),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      layerSucceed(Model, Model.of(ANSWERING_MODEL)),
      layerSucceed(SampleResultStore, store.service),
      noopProgressLayer,
      noopCheckpointLayer
    );

    const result = await runPromise(
      runBenchmark({
        epochs: 1,
        maxConcurrency: 1,
        degradeSolverErrors: true,
      }).pipe(provide(layers))
    );

    expect(result.metrics.skippedQuestions).toBe(1);
    expect(result.sampleScores[0]?.score.value).toBe(ScoreValue.Skipped);
    expect(store.writes).toHaveLength(1);
    expect(store.writes[0]?.degraded).toBe(true);
  });

  it("records a finished result without waiting for an earlier slow sample", async () => {
    const store = recordingStore([]);
    let releaseSlowSample = (): void => {};
    const slowSampleStarted = new Promise<void>((resolve) => {
      const markStarted = resolve;
      const gate = new Promise<void>((_resolve) => {
        releaseSlowSample = _resolve;
      });
      const slowModel: ModelService = {
        generate: (messages) => {
          const userMsg =
            messages.find((m) => m.role === MessageRole.User)?.content ?? "";
          if (typeof userMsg !== "string" || !userMsg.includes("Q0")) {
            return effectSucceed(MODEL_RESPONSE);
          }
          return effectPromise(async () => {
            markStarted();
            await gate;
            return MODEL_RESPONSE;
          }).pipe(orDie);
        },
      };
      // oxlint-disable-next-line eslint/no-void -- fire-and-forget: the test observes the run via the store
      void runPromise(
        runBenchmark({ epochs: 1, maxConcurrency: 3 }).pipe(
          provide(makeLayers(store.service, slowModel))
        )
      );
    });

    await slowSampleStarted;
    await waitFor(() => store.writes.length === 2);

    expect(store.writes.map((w) => w.sampleIndex).toSorted()).toEqual([1, 2]);
    releaseSlowSample();
  });

  it("re-evaluates everything when listing stored records fails", async () => {
    const failingStore: SampleResultStoreService = {
      write: async () => {},
      list: async () => {
        throw new Error("gcs unavailable");
      },
    };

    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(
        provide(makeLayers(failingStore))
      )
    );

    expect(result.sampleScores).toHaveLength(3);
  });

  it("retries a failed record write within the sample and completes once it succeeds", async () => {
    let attempts = 0;
    const writes: CompletedSampleEntry[] = [];
    const flakyStore: SampleResultStoreService = {
      write: async (entry) => {
        attempts += 1;
        if (attempts <= 2) {
          throw new Error("gcs unavailable");
        }
        writes.push(entry);
      },
      list: async () => [],
    };

    const result = await runPromise(
      runBenchmark({
        epochs: 1,
        maxConcurrency: 1,
        range: { start: 0, end: 1 },
      }).pipe(provide(makeLayers(flakyStore)))
    );

    expect(attempts).toBe(3);
    expect(writes.map((w) => w.sampleIndex)).toEqual([0]);
    expect(result.metrics.totalQuestions).toBe(1);
  });

  it("fails the run when persisting a record still fails after bounded retries", async () => {
    let attempts = 0;
    const failingStore: SampleResultStoreService = {
      write: async () => {
        attempts += 1;
        throw new Error("gcs unavailable");
      },
      list: async () => [],
    };

    await expect(
      runPromise(
        runBenchmark({
          epochs: 1,
          maxConcurrency: 1,
          range: { start: 0, end: 1 },
        }).pipe(provide(makeLayers(failingStore)))
      )
    ).rejects.toThrow("Failed to persist sample result");
    expect(attempts).toBe(4);
  });
});
