import { mapBoth } from "effect/Effect";

import type { ModelMessage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import { definedValues } from "../../internal/guards";
import { buildSummarizePrompt } from "./prompts";
import type { Summarizer } from "./solver";

export function makeModelSummarizer(
  model: ModelService,
  config: GenerateConfig
): Summarizer {
  return (messages: readonly ModelMessage[]) =>
    model
      .generate(
        [
          {
            role: MessageRole.User,
            content: buildSummarizePrompt(messages),
          },
        ],
        config
      )
      .pipe(
        mapBoth({
          onFailure: (e) =>
            new SolverError({
              message: `recovery-bench summary generation failed: ${e.message}`,
            }),
          onSuccess: (output) =>
            definedValues({
              text: output.completion,
              usage: output.usage,
              generationTimeMs: output.generationTimeMs,
            }),
        })
      );
}
