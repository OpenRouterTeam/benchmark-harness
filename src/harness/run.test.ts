import { afterEach, describe, expect, it, spyOn } from "bun:test";

import { fromIterable } from "effect/Chunk";
import {
  flatMap as effectFlatMap,
  logInfo,
  runPromise,
  succeed as effectSucceed,
  fail as effectFail,
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
import { runHarnessPromise } from "../internal/effect-logger";
import { recordGenerationId } from "../runtime/generation-ids";
import type { ResponseItem, Sample } from "./core";
import { MessageRole, ModelError, ScoreValue } from "./core";
import { Dataset } from "./dataset";
import type { ModelService } from "./model";
import { Model } from "./model";
import type { CheckpointStore, ProgressReporter } from "./progress";
import { runBenchmark } from "./run";
import { Scorer } from "./scorer";
import type { SolverService } from "./solver";
import { systemMessage, chain, generate, Solver } from "./solver";

const infoSpies: {
  mockRestore: () => void;
}[] = [];
afterEach(() => {
  for (const info of infoSpies.splice(0)) {
    info.mockRestore();
  }
});

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

function fakeModel(answerBySampleHint: (input: string) => string): {
  service: ModelService;
  layer: Layer<Model>;
} {
  const service: ModelService = {
    generate: (messages) => {
      const userMsg =
        messages.find((m) => m.role === MessageRole.User)?.content ?? "";
      const completion = answerBySampleHint(userMsg);
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
  return { service, layer: layerSucceed(Model, Model.of(service)) };
}

const SAMPLES: readonly Sample[] = [
  { id: "s-correct", input: "Q1 target B", target: { text: "B" } },
  { id: "s-wrong", input: "Q2 target B", target: { text: "B" } },
];

function makeLayers(
  model: {
    service: ModelService;
    layer: Layer<Model>;
  },
  solverService: ReturnType<typeof chain>
): Layer<Dataset | Solver | Scorer | ProgressReporter | CheckpointStore> {
  return mergeAll(
    fakeDatasetLayer(SAMPLES),
    layerSucceed(Solver, Solver.of(solverService)),
    layerSucceed(Scorer, Scorer.of(mcqScorer)),
    model.layer,
    noopProgressLayer,
    noopCheckpointLayer
  );
}
describe("runBenchmark", () => {
  it("scopes run and sample annotations plus a sample duration span", async () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    infoSpies.push(info);
    const solver: SolverService = (state) =>
      logInfo("stage-2a-annotation-probe").pipe(
        effectFlatMap(() =>
          effectSucceed({
            ...state,
            output: {
              completion: "Answer: B",
              message: { role: MessageRole.Assistant, content: "Answer: B" },
            },
            messages: [
              ...state.messages,
              { role: MessageRole.Assistant, content: "Answer: B" },
            ],
            completed: true,
          })
        )
      );
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES.slice(0, 1)),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      noopProgressLayer,
      noopCheckpointLayer
    );
    await runHarnessPromise(
      runBenchmark({
        epochs: 1,
        maxConcurrency: 1,
        logAnnotations: {
          benchmark: "annotation-probe",
        },
      }).pipe(provide(layers))
    );
    const record = info.mock.calls.find(
      (call) => call[0] === "stage-2a-annotation-probe"
    );
    expect(record?.[1]).toMatchObject({
      benchmark: "annotation-probe",
      sample_id: "s-correct",
      epoch: 0,
      spans: { sample: expect.any(Number) },
    });
  });
  it("streams, scores, and aggregates with the real solver/scorer", async () => {
    const model = fakeModel((input) =>
      input.includes("Q1") ? "Answer: B" : "Answer: A"
    );
    const solver = chain(
      systemMessage("You are a helpful assistant."),
      generate(model.service, { temperature: 0.5 })
    );
    const result = await runPromise(
      runBenchmark({ epochs: 3, maxConcurrency: 4 }).pipe(
        provide(makeLayers(model, solver))
      )
    );
    expect(result.sampleScores.length).toBe(6);
    expect(result.metrics.totalQuestions).toBe(2);
    expect(result.metrics.accuracy).toBeCloseTo(0.5, 5);
    expect(result.metrics.correctAnswers).toBe(1);
    expect(result.usage.outputTokens).toBe(30);
    expect(result.usage.generationTimeMs).toBe(600);
    expect(result.sampleScores[0]?.generationIds).toEqual(["fake-Q1 target B"]);
  });
  it("captures per-sample message trajectories", async () => {
    const model = fakeModel(() => "Answer: B");
    const solver = chain(
      systemMessage("You are a helpful assistant."),
      generate(model.service, { temperature: 0.5 })
    );
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES.slice(0, 1)),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      model.layer,
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 1 }).pipe(provide(layers))
    );
    const score = result.sampleScores[0];
    expect(score?.messages).toBeDefined();
    expect(score?.messages).toHaveLength(3);
    expect(score?.messages?.[0]?.role).toBe(MessageRole.System);
    expect(score?.messages?.[1]?.role).toBe(MessageRole.User);
    expect(score?.messages?.[2]?.role).toBe(MessageRole.Assistant);
    expect(score?.messages?.[2]?.content).toBe("Answer: B");
  });
  it("carries response items from TaskState into per-sample scores", async () => {
    const responseItems: readonly ResponseItem[] = [
      { role: "user", content: "Q1 target B" },
      { type: "web_search_call", id: "search-1", status: "completed" },
      {
        type: "message",
        content: [{ type: "output_text", text: "Answer: B" }],
      },
    ];
    const solver: SolverService = (state) =>
      effectSucceed({
        ...state,
        responseItems,
        messages: [
          ...state.messages,
          { role: MessageRole.Assistant, content: "Answer: B" },
        ],
        output: {
          completion: "Answer: B",
          message: { role: MessageRole.Assistant, content: "Answer: B" },
        },
        completed: true,
      });
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES.slice(0, 1)),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 1 }).pipe(provide(layers))
    );
    expect(result.sampleScores[0]?.responseItems).toEqual(responseItems);
  });
  it("carries the request body from TaskState into per-sample scores", async () => {
    const requestBody = {
      model: "model",
      provider: { order: ["openai"], allowFallbacks: false },
    } as const;
    const solver: SolverService = (state) =>
      effectSucceed({
        ...state,
        requestBody,
        output: {
          completion: "Answer: B",
          message: { role: MessageRole.Assistant, content: "Answer: B" },
        },
        completed: true,
      });
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES.slice(0, 1)),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 1 }).pipe(provide(layers))
    );
    expect(result.sampleScores[0]?.requestBody).toEqual(requestBody);
  });
  it("respects a sample range (chunk slice)", async () => {
    const model = fakeModel(() => "Answer: B");
    const solver = generate(model.service, { temperature: 0 });
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      model.layer,
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({
        epochs: 1,
        maxConcurrency: 2,
        range: { start: 0, end: 1 },
      }).pipe(provide(layers))
    );
    expect(result.metrics.totalQuestions).toBe(1);
  });
  it("scores a sample as Skipped (excluded from accuracy) when the model exhausts 429 retries", async () => {
    const service: ModelService = {
      generate: (messages) => {
        const userMsg =
          messages.find((m) => m.role === MessageRole.User)?.content ?? "";
        if (userMsg.includes("Q1")) {
          return effectFail(
            new ModelError({
              message: "OpenRouter HTTP 429: rate-limited",
              status: 429,
            })
          );
        }
        return effectSucceed({
          completion: "Answer: B",
          message: { role: MessageRole.Assistant, content: "Answer: B" },
          generationTimeMs: 100,
        });
      },
    };
    const model = { service, layer: layerSucceed(Model, Model.of(service)) };
    const solver = generate(model.service, { temperature: 0 });
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      model.layer,
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(provide(layers))
    );
    const skipped = result.sampleScores.find((s) => s.sampleId === "s-correct");
    expect(skipped?.score.value).toBe(ScoreValue.Skipped);
    expect(result.metrics.totalQuestions).toBe(1);
    expect(result.metrics.skippedQuestions).toBe(1);
    expect(result.metrics.accuracy).toBe(1);
  });
  it("scores a sample as Incorrect (counted against accuracy) on a non-retryable model error", async () => {
    const service: ModelService = {
      generate: (messages) => {
        const userMsg =
          messages.find((m) => m.role === MessageRole.User)?.content ?? "";
        if (userMsg.includes("Q1")) {
          return effectFail(
            new ModelError({
              message: "OpenRouter HTTP 400: content policy",
              status: 400,
            })
          );
        }
        return effectSucceed({
          completion: "Answer: B",
          message: { role: MessageRole.Assistant, content: "Answer: B" },
          generationTimeMs: 100,
        });
      },
    };
    const model = { service, layer: layerSucceed(Model, Model.of(service)) };
    const solver = generate(model.service, { temperature: 0 });
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      model.layer,
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(provide(layers))
    );
    const failed = result.sampleScores.find((s) => s.sampleId === "s-correct");
    expect(failed?.score.value).toBe(ScoreValue.Incorrect);
    expect(result.metrics.totalQuestions).toBe(2);
    expect(result.metrics.skippedQuestions).toBe(0);
    expect(result.metrics.accuracy).toBeCloseTo(0.5, 5);
  });
  it("accumulates generationTimeMs even when the response has no usage object", async () => {
    const model: {
      service: ModelService;
      layer: Layer<Model>;
    } = {
      service: {
        generate: () =>
          effectSucceed({
            completion: "Answer: B",
            message: { role: MessageRole.Assistant, content: "Answer: B" },
            generationTimeMs: 100,
          }),
      },
      layer: layerSucceed(
        Model,
        Model.of({
          generate: () =>
            effectSucceed({
              completion: "Answer: B",
              message: { role: MessageRole.Assistant, content: "Answer: B" },
              generationTimeMs: 100,
            }),
        })
      ),
    };
    const solver = generate(model.service, { temperature: 0 });
    const layers = mergeAll(
      fakeDatasetLayer(SAMPLES),
      layerSucceed(Solver, Solver.of(solver)),
      layerSucceed(Scorer, Scorer.of(mcqScorer)),
      model.layer,
      noopProgressLayer,
      noopCheckpointLayer
    );
    const result = await runPromise(
      runBenchmark({ epochs: 1, maxConcurrency: 2 }).pipe(provide(layers))
    );
    expect(result.usage.generationTimeMs).toBe(200);
    expect(result.usage.outputTokens).toBe(0);
  });
});
