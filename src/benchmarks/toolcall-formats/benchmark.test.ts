import { describe, expect, it } from "bun:test";
import assert from "node:assert/strict";

import { toReadonlyArray } from "effect/Chunk";
import type { Effect } from "effect/Effect";
import { gen, provide, runPromise, succeed } from "effect/Effect";
import { mergeAll } from "effect/Layer";
import { runCollect } from "effect/Stream";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../test/helpers/noop-progress-layer";
import type {
  ModelError,
  ModelMessage,
  ModelOutput,
  Score,
} from "../../harness/core";
import { initialTaskState, MessageRole, ScoreValue } from "../../harness/core";
import { Dataset } from "../../harness/dataset";
import type { GenerateConfig, ModelService } from "../../harness/model";
import { TOOLCALL_FORMATS_META } from "../benchmark-meta";
import {
  makeToolCallFormatsDatasetLayer,
  TOOLCALL_FORMATS_BENCHMARK,
  TOOLCALL_FORMATS_SAMPLES,
  toolCallFormatsSolver,
} from "./benchmark";
import { TOOLCALL_FORMAT_CASES } from "./cases";
import { toolCallFormatsScorer } from "./scorer";

const READ_SLICE_STRICT_ID = "toolcall_formats-read_slice-anyof_null_strict";
const READ_WRONG_TYPE_REQUESTED_STRICT_ID =
  "toolcall_formats-read_wrong_type_requested-anyof_null_strict";
const READ_OMIT_REQUESTED_STRICT_ID =
  "toolcall_formats-read_omit_requested-anyof_null_strict";

interface Recorded {
  messages?: readonly ModelMessage[];
  config?: GenerateConfig;
}

interface SolvedRun {
  readonly recorded: Recorded;
  readonly score: Score;
}

function readCallModel(args: string, recorded: Recorded): ModelService {
  return {
    generate: (
      messages: readonly ModelMessage[],
      config: GenerateConfig
    ): Effect<ModelOutput, ModelError> => {
      recorded.messages = messages;
      recorded.config = config;
      return succeed({
        completion: "",
        message: {
          role: MessageRole.Assistant,
          content: "",
          toolCalls: [
            {
              id: "call_1",
              type: "function",
              function: { name: "read", arguments: args },
            },
          ],
        },
      });
    },
  };
}

async function solveRead(toolCall: {
  readonly arguments: string;
  readonly sampleId?: string;
}): Promise<SolvedRun> {
  const sample = TOOLCALL_FORMATS_SAMPLES.find(
    (candidate) => candidate.id === (toolCall.sampleId ?? READ_SLICE_STRICT_ID)
  );
  assert(sample !== undefined);
  const recorded: Recorded = {};
  const solver = toolCallFormatsSolver(
    readCallModel(toolCall.arguments, recorded),
    {
      endpointId: "endpoint-1",
      inference: { reasoningEffort: "high", providerOnly: ["provider-a"] },
    }
  );
  const score = await runPromise(
    gen(function* () {
      const state = yield* solver(initialTaskState(sample));
      return yield* toolCallFormatsScorer(state, sample.target);
    }).pipe(provide(mergeAll(noopProgressLayer, noopCheckpointLayer)))
  );
  return { recorded, score };
}

describe("makeToolCallFormatsDatasetLayer", () => {
  it("serves every case and honours range slicing", async () => {
    const { size, slice } = await runPromise(
      gen(function* () {
        const dataset = yield* Dataset;
        const total = yield* dataset.size;
        const chunk = yield* runCollect(dataset.stream({ start: 5, end: 10 }));
        return { size: total, slice: toReadonlyArray(chunk) };
      }).pipe(provide(makeToolCallFormatsDatasetLayer()))
    );

    expect(size).toBe(135);
    expect(size).toBe(TOOLCALL_FORMAT_CASES.length);
    expect(slice.map((sample) => sample.id)).toEqual(
      TOOLCALL_FORMAT_CASES.slice(5, 10).map((entry) => entry.id)
    );
  });
});

describe("TOOLCALL_FORMATS_BENCHMARK", () => {
  it("exposes a temperature-0 definition that mirrors its metadata", () => {
    expect(TOOLCALL_FORMATS_BENCHMARK.id).toBe(TOOLCALL_FORMATS_META.id);
    expect(TOOLCALL_FORMATS_BENCHMARK.temperature).toBe(0);
    expect(TOOLCALL_FORMATS_BENCHMARK.defaultEpochs).toBe(1);
  });
});

describe("toolCallFormatsSolver", () => {
  it("sends the case tools with provider pinning at temperature 0", async () => {
    const { recorded, score } = await solveRead({
      arguments: '{"path":"src/server.ts","offset":40,"limit":41}',
    });

    expect(recorded.messages).toMatchObject([
      { role: MessageRole.System },
      { role: MessageRole.User, content: "Read lines 40-80 of src/server.ts" },
    ]);
    expect(recorded.config).toMatchObject({
      temperature: 0,
      reasoningEffort: "high",
      providerOnly: ["provider-a"],
      endpointId: "endpoint-1",
      tools: [
        {
          type: "function",
          function: {
            name: "read",
            strict: true,
            parameters: {
              required: ["path", "offset", "limit"],
              properties: {
                offset: { anyOf: [{ type: "number" }, { type: "null" }] },
              },
            },
          },
        },
        { type: "function", function: { name: "bash", strict: true } },
      ],
    });
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("sends strict: false explicitly on non-strict variants", async () => {
    const { recorded } = await solveRead({
      arguments: '{"path":"src/server.ts","offset":40,"limit":41}',
      sampleId: "toolcall_formats-read_slice-omitted",
    });

    expect(recorded.config?.tools?.[0]?.function.strict).toBe(false);
  });

  it("scores stringified numbers from the response as a schema violation", async () => {
    const { score } = await solveRead({
      arguments: '{"limit":"41","offset":"40","path":"src/server.ts"}',
    });

    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toStartWith("schema_violation");
  });

  it("accepts explicit nulls when the prompt asks to leave nullable required keys out", async () => {
    const { score } = await solveRead({
      arguments: '{"path":"src/b.ts","offset":null,"limit":null}',
      sampleId: READ_OMIT_REQUESTED_STRICT_ID,
    });

    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("accepts any schema-valid value for keys the prompt asks to leave out", async () => {
    const { score } = await solveRead({
      arguments: '{"path":"src/b.ts","offset":1,"limit":100}',
      sampleId: READ_OMIT_REQUESTED_STRICT_ID,
    });

    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("scores omitting a nullable required key on request as a schema violation", async () => {
    const { score } = await solveRead({
      arguments: '{"path":"src/b.ts"}',
      sampleId: READ_OMIT_REQUESTED_STRICT_ID,
    });

    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toStartWith("schema_violation");
  });

  it("scores a requested string in a number slot as a schema violation", async () => {
    const { score } = await solveRead({
      arguments: '{"path":"src/b.ts","offset":null,"limit":"50"}',
      sampleId: READ_WRONG_TYPE_REQUESTED_STRICT_ID,
    });

    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toStartWith("schema_violation");
  });
});
