import { fail, gen, map } from "effect/Effect";

import type { ModelMessage, ModelOutput, TaskState } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import type { DecisionsClientService } from "./client";
import {
  decisionOutcomeFromResponse,
  llmOutcomeFromCompletion,
} from "./outcome";
import { LLM_SYSTEM_PROMPT, renderLlmPrompt } from "./prompt";
import type {
  DecisionOutcome,
  DecisionsRequest,
  DecisionTaskSpec,
} from "./schema";
import {
  DECISION_OUTCOME_METADATA_KEY,
  DECISION_SPEC_METADATA_KEY,
  DecisionTaskSpecSchema,
} from "./schema";

export const LLM_ARM_TEMPERATURE = 0;

export function specFromState(
  state: TaskState
): Either.Either<DecisionTaskSpec, SolverError> {
  const raw = state.sample.metadata?.[DECISION_SPEC_METADATA_KEY];
  const parsed = parseSchema(DecisionTaskSpecSchema, raw);
  return Either.isLeft(parsed)
    ? Either.left(
        new SolverError({
          message: `Sample ${state.sample.id} is missing a valid decision spec: ${parsed.left.message}`,
        })
      )
    : Either.right(parsed.right);
}

function withOutcome(
  state: TaskState,
  outcome: DecisionOutcome,
  messages: readonly ModelMessage[],
  output: ModelOutput,
  requestBody: Readonly<Record<string, unknown>>
): TaskState {
  return {
    ...state,
    sample: {
      ...state.sample,
      metadata: {
        ...state.sample.metadata,
        [DECISION_OUTCOME_METADATA_KEY]: outcome,
      },
    },
    messages,
    output,
    requestBody,
    completed: true,
  };
}

export interface DecisionSolverOptions {
  readonly model: string;
  readonly endpointId?: string;
  readonly providerOnly?: readonly string[];
  readonly providerIgnore?: readonly string[];
  readonly allowFallbacks?: boolean;
}

export function buildDecisionsRequest(
  spec: DecisionTaskSpec,
  options: DecisionSolverOptions
): DecisionsRequest {
  const provider = definedValues({
    only:
      options.providerOnly === undefined
        ? undefined
        : [...options.providerOnly],
    ignore:
      options.providerIgnore === undefined
        ? undefined
        : [...options.providerIgnore],
    allow_fallbacks: options.allowFallbacks,
  });
  return {
    model: options.model,
    state: spec.state,
    questions: { [spec.questionId]: spec.question },
    ...(Object.keys(provider).length > 0 ? { provider } : {}),
  };
}

export function decisionSolver(
  client: DecisionsClientService,
  options: DecisionSolverOptions
): SolverService {
  return (state) => {
    const spec = specFromState(state);
    if (Either.isLeft(spec)) {
      return fail(spec.left);
    }
    const request = buildDecisionsRequest(spec.right, options);
    return client
      .send(definedValues({ request, endpointId: options.endpointId }))
      .pipe(
        map((result) => {
          const outcome = decisionOutcomeFromResponse({
            spec: spec.right,
            response: result.response,
            latencyMs: result.latencyMs,
          });
          const completion = JSON.stringify(
            result.response.answers[spec.right.questionId] ?? null
          );
          const output: ModelOutput = definedValues({
            completion,
            message: { role: MessageRole.Assistant, content: completion },
            usage: definedValues({
              inputTokens: outcome.inputTokens ?? undefined,
              outputTokens: outcome.outputTokens ?? undefined,
              totalTokens:
                outcome.inputTokens === null || outcome.outputTokens === null
                  ? undefined
                  : outcome.inputTokens + outcome.outputTokens,
              totalCost: outcome.cost ?? undefined,
            }),
            generationTimeMs: result.latencyMs,
            rawResponse: result.response,
          });
          return withOutcome(
            state,
            outcome,
            [...state.messages, output.message],
            output,
            request
          );
        })
      );
  };
}

export interface LlmSolverOptions {
  readonly model: string;
  readonly endpointId?: string;
  readonly inference: Omit<GenerateConfig, "temperature" | "maxTokens">;
}

export function llmSolver(
  model: ModelService,
  options: LlmSolverOptions
): SolverService {
  const config: GenerateConfig = {
    temperature: LLM_ARM_TEMPERATURE,
    ...definedValues(options.inference),
    ...definedValues({ endpointId: options.endpointId }),
  };
  return (state) => {
    const spec = specFromState(state);
    if (Either.isLeft(spec)) {
      return fail(spec.left);
    }
    const messages: readonly ModelMessage[] = [
      { role: MessageRole.System, content: LLM_SYSTEM_PROMPT },
      { role: MessageRole.User, content: renderLlmPrompt(spec.right) },
    ];
    return gen(function* () {
      const startedAt = performance.now();
      const output = yield* model.generate(messages, config);
      const latencyMs =
        output.generationTimeMs ?? Math.round(performance.now() - startedAt);
      const outcome = llmOutcomeFromCompletion({
        spec: spec.right,
        model: options.model,
        completion: output.completion,
        latencyMs,
        cost: output.usage?.totalCost ?? null,
        inputTokens: output.usage?.inputTokens ?? null,
        outputTokens: output.usage?.outputTokens ?? null,
      });
      return withOutcome(
        state,
        outcome,
        [...messages, output.message],
        output,
        { model: options.model, messages }
      );
    });
  };
}
