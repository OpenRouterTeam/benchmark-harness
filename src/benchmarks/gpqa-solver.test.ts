import { describe, expect, it } from "bun:test";

import type { Effect } from "effect/Effect";
import { succeed, provide, runPromiseExit } from "effect/Effect";
import { isSuccess } from "effect/Exit";
import type { Layer } from "effect/Layer";
import { merge, succeed as layerSucceed } from "effect/Layer";

import {
  noopProgressLayer,
  noopCheckpointLayer,
} from "../../test/helpers/noop-progress-layer";
import type { ChatMessage, ModelError, ModelOutput } from "../harness/core";
import { initialTaskState, MessageRole } from "../harness/core";
import type { GenerateConfig, ModelService } from "../harness/model";
import { Model } from "../harness/model";
import { ProviderSort } from "../internal/enums";
import { gpqaSolver, GPQA_TEMPERATURE } from "./gpqa";

function recordingModel(record: { config: GenerateConfig | undefined }): {
  layer: Layer<Model>;
  service: ModelService;
} {
  const service: ModelService = {
    generate: (
      _messages: readonly ChatMessage[],
      config: GenerateConfig
    ): Effect<ModelOutput, ModelError> => {
      record.config = config;
      const output: ModelOutput = {
        completion: "Answer: A",
        message: { role: MessageRole.Assistant, content: "Answer: A" },
        generationTimeMs: 1,
      };
      return succeed(output);
    },
  };
  return { layer: layerSucceed(Model, Model.of(service)), service };
}

const SAMPLE = { id: "gpqa_diamond-0", input: "q", target: { text: "A" } };

async function runSolver(
  opts?: Parameters<typeof gpqaSolver>[1]
): Promise<GenerateConfig | undefined> {
  const record: {
    config: GenerateConfig | undefined;
  } = { config: undefined };
  const { layer, service } = recordingModel(record);
  const state = initialTaskState(SAMPLE);
  const exit = await runPromiseExit(
    gpqaSolver(
      service,
      opts
    )(state).pipe(
      provide(merge(merge(layer, noopProgressLayer), noopCheckpointLayer))
    )
  );
  if (!isSuccess(exit)) {
    throw new Error("solver failed");
  }
  return record.config;
}
describe("gpqaSolver inference overrides (openbench parity)", () => {
  it("uses the gpqa temperature default when no override is given", async () => {
    const config = await runSolver();
    expect(config?.temperature).toBe(GPQA_TEMPERATURE);
    expect(config?.endpointId).toBeUndefined();
    expect(config?.maxTokens).toBeUndefined();
  });
  it("forwards endpointId and falls back to the default temperature", async () => {
    const config = await runSolver({ endpointId: "ep-1" });
    expect(config?.temperature).toBe(GPQA_TEMPERATURE);
    expect(config?.endpointId).toBe("ep-1");
  });
  it("forwards all inference knobs", async () => {
    const config = await runSolver({
      endpointId: "ep-1",
      inference: {
        maxTokens: 100,
        reasoningEffort: "low",
        timeoutMs: 5000,
        sort: ProviderSort.Price,
        providerOnly: ["google-vertex"],
        allowFallbacks: false,
        cloudflareVersion: "ver-1",
      },
    });
    expect(config).toEqual({
      temperature: GPQA_TEMPERATURE,
      maxTokens: 100,
      reasoningEffort: "low",
      timeoutMs: 5000,
      sort: ProviderSort.Price,
      providerOnly: ["google-vertex"],
      allowFallbacks: false,
      cloudflareVersion: "ver-1",
      endpointId: "ep-1",
    });
  });
  it("drops undefined override fields (does not clobber defaults with undefined)", async () => {
    const config = await runSolver({
      inference: { maxTokens: 42 },
    });
    expect(config?.temperature).toBe(GPQA_TEMPERATURE);
    expect(config?.maxTokens).toBe(42);
    expect(config?.reasoningEffort).toBeUndefined();
  });
});
