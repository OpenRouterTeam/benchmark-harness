import { describe, expect, it } from "bun:test";

import type { Effect } from "effect/Effect";
import { provide, runPromise, succeed } from "effect/Effect";
import { merge } from "effect/Layer";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../test/helpers/noop-progress-layer";
import type { ModelMessage, ModelError, ModelOutput } from "../harness/core";
import { initialTaskState, MessageRole } from "../harness/core";
import type { GenerateConfig, ModelService } from "../harness/model";
import { MMLU_PRO_TEMPERATURE, mmluProSolver } from "./mmlu-pro";
describe("mmluProSolver", () => {
  it("uses canonical sampling defaults and sends one user message", async () => {
    const recorded: {
      config?: GenerateConfig;
      messages?: readonly ModelMessage[];
    } = {};
    const model: ModelService = {
      generate: (
        messages: readonly ModelMessage[],
        config: GenerateConfig
      ): Effect<ModelOutput, ModelError> => {
        recorded.messages = messages;
        recorded.config = config;
        return succeed({
          completion: "The answer is (A).",
          message: {
            role: MessageRole.Assistant,
            content: "The answer is (A).",
          },
        });
      },
    };
    const solver = mmluProSolver(model);
    await runPromise(
      solver(
        initialTaskState({
          id: "sample",
          input: "question",
          target: { text: "A" },
        })
      ).pipe(provide(merge(noopProgressLayer, noopCheckpointLayer)))
    );
    expect(recorded.config).toEqual({
      temperature: MMLU_PRO_TEMPERATURE,
    });
    expect(recorded.config?.maxTokens).toBeUndefined();
    expect(recorded.messages).toEqual([
      { role: MessageRole.User, content: "question" },
    ]);
  });
});
