import { describe, expect, it } from "bun:test";

import type { Effect } from "effect/Effect";
import { either, provide, runPromise, succeed } from "effect/Effect";
import { mergeAll, succeed as layerSucceed } from "effect/Layer";

import type { ModelMessage } from "../../harness/core";
import { initialTaskState, MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../harness/progress";
import { Either } from "../../internal/either";
import type { DecisionsCall, DecisionsClientService } from "./client";
import { LLM_SYSTEM_PROMPT } from "./prompt";
import type { DecisionOutcome, DecisionTaskSpec } from "./schema";
import {
  DECISION_OUTCOME_METADATA_KEY,
  DECISION_SPEC_METADATA_KEY,
  DecisionArm,
  DecisionTaskId,
  DecisionVariant,
} from "./schema";
import {
  buildDecisionsRequest,
  decisionSolver,
  LLM_ARM_TEMPERATURE,
  llmSolver,
  specFromState,
} from "./solver";

const SPEC: DecisionTaskSpec = {
  task: DecisionTaskId.BoolQ,
  variant: DecisionVariant.Base,
  language: "en",
  questionId: "answer",
  state: { passage: "Water boils at 100C at sea level." },
  stateText: "Water boils at 100C at sea level.",
  question: {
    type: "noul",
    instructions: "Does water boil at 100C at sea level?",
    criteria: { true: "yes", false: "no" },
  },
  labels: ["true", "false"],
  gold: "true",
  goldKnowable: true,
};

function stateWith(spec: DecisionTaskSpec | undefined) {
  return initialTaskState({
    id: "decisions-boolq-en-base-0",
    input: SPEC.stateText,
    target: { text: SPEC.gold },
    metadata: spec === undefined ? {} : { [DECISION_SPEC_METADATA_KEY]: spec },
  });
}

const HARNESS_SERVICES = mergeAll(
  layerSucceed(ProgressReporter, NOOP_PROGRESS_REPORTER),
  layerSucceed(CheckpointStore, NOOP_CHECKPOINT_STORE)
);

function run<A, E>(
  effect: Effect<A, E, ProgressReporter | CheckpointStore>
): Promise<A> {
  return runPromise(effect.pipe(provide(HARNESS_SERVICES)));
}

function outcomeOf(metadata: Readonly<Record<string, unknown>> | undefined) {
  return metadata?.[DECISION_OUTCOME_METADATA_KEY] as DecisionOutcome;
}

describe("specFromState", () => {
  it("rejects samples without a valid spec", () => {
    const result = specFromState(stateWith(undefined));
    expect(Either.isLeft(result)).toBe(true);
    expect(Either.isLeft(result) ? result.left : undefined).toBeInstanceOf(
      SolverError
    );
  });
  it("parses a valid spec", () => {
    const result = specFromState(stateWith(SPEC));
    expect(Either.isRight(result) ? result.right.gold : undefined).toBe("true");
  });
});

describe("buildDecisionsRequest", () => {
  it("keys the single question by the spec question id and omits provider when unset", () => {
    const request = buildDecisionsRequest(SPEC, {
      model: "typesafe/jev-latest",
    });
    expect(request).toEqual({
      model: "typesafe/jev-latest",
      state: SPEC.state,
      questions: { answer: SPEC.question },
    });
    expect(request).not.toHaveProperty("provider");
  });
  it("forwards provider routing fields", () => {
    const request = buildDecisionsRequest(SPEC, {
      model: "typesafe/jev-latest",
      providerOnly: ["typesafe"],
      providerIgnore: ["other"],
      allowFallbacks: false,
    });
    expect(request.provider).toEqual({
      only: ["typesafe"],
      ignore: ["other"],
      allow_fallbacks: false,
    });
  });
});

describe("decisionSolver", () => {
  it("sends the typed request with endpoint pinning and stores outcome, transcript, usage, and raw response", async () => {
    const calls: DecisionsCall[] = [];
    const client: DecisionsClientService = {
      send: (call) => {
        calls.push(call);
        return succeed({
          response: {
            id: "gen-1",
            model: "typesafe/jev-1.13.0",
            provider: "typesafe",
            answers: { answer: { type: "noul", noul: 0.91 } },
            usage: { input_tokens: 40, output_tokens: 0, cost: 0.0000017 },
          },
          latencyMs: 55,
          cacheHit: false,
          identifiers: { generationId: "gen-1" },
        });
      },
    };
    const solve = decisionSolver(client, {
      model: "typesafe/jev-latest",
      endpointId: "ep-9",
      providerOnly: ["typesafe"],
    });
    const state = await run(solve(stateWith(SPEC)));
    expect(calls).toHaveLength(1);
    expect(calls[0]?.endpointId).toBe("ep-9");
    expect(calls[0]?.request.model).toBe("typesafe/jev-latest");
    expect(calls[0]?.request.provider).toEqual({ only: ["typesafe"] });
    expect(state.completed).toBe(true);
    expect(state.requestBody).toEqual(calls[0]?.request);
    expect(state.output?.completion).toBe('{"type":"noul","noul":0.91}');
    expect(state.output?.usage).toEqual({
      inputTokens: 40,
      outputTokens: 0,
      totalTokens: 40,
      totalCost: 0.0000017,
    });
    expect(state.output?.generationTimeMs).toBe(55);
    expect(state.output?.rawResponse).toMatchObject({ id: "gen-1" });
    expect(state.messages.map((message) => message.role)).toEqual([
      MessageRole.User,
      MessageRole.Assistant,
    ]);
    const outcome = outcomeOf(state.sample.metadata);
    expect(outcome.arm).toBe(DecisionArm.Decision);
    expect(outcome.model).toBe("typesafe/jev-1.13.0");
    expect(outcome.argmax).toBe("true");
    expect(outcome.distribution.true).toBeCloseTo(0.91, 12);
    expect(outcome.latencyMs).toBe(55);
    expect(outcome.cost).toBeCloseTo(0.0000017, 12);
    expect(outcome.violations).toEqual([]);
    expect(state.sample.metadata?.[DECISION_SPEC_METADATA_KEY]).toEqual(SPEC);
  });
  it("fails with SolverError when the sample has no spec", async () => {
    const client: DecisionsClientService = {
      send: () => {
        throw new Error("must not be called");
      },
    };
    const result = await run(
      either(decisionSolver(client, { model: "m" })(stateWith(undefined)))
    );
    expect(Either.isLeft(result) ? result.left : undefined).toBeInstanceOf(
      SolverError
    );
  });
});

describe("llmSolver", () => {
  it("renders the closed-set prompt, pins temperature without maxTokens, and stores the parsed outcome", async () => {
    const captured: {
      messages: readonly ModelMessage[];
      config: GenerateConfig;
    }[] = [];
    const model: ModelService = {
      generate: (messages, config) => {
        captured.push({ messages, config });
        return succeed({
          completion: 'Sure. {"answer": "true", "confidence": 0.8}',
          message: {
            role: MessageRole.Assistant,
            content: '{"answer": "true", "confidence": 0.8}',
          },
          usage: {
            inputTokens: 120,
            outputTokens: 14,
            totalTokens: 134,
            totalCost: 0.0003,
          },
          generationTimeMs: 900,
        });
      },
    };
    const solve = llmSolver(model, {
      model: "openai/gpt-5-mini",
      endpointId: "ep-llm",
      inference: {
        reasoningEffort: "low",
        providerOnly: ["openai"],
        timeoutMs: 30_000,
      },
    });
    const state = await run(solve(stateWith(SPEC)));
    expect(captured).toHaveLength(1);
    const [call] = captured;
    expect(call?.config.temperature).toBe(LLM_ARM_TEMPERATURE);
    expect(call?.config).not.toHaveProperty("maxTokens");
    expect(call?.config.endpointId).toBe("ep-llm");
    expect(call?.config.reasoningEffort).toBe("low");
    expect(call?.config.providerOnly).toEqual(["openai"]);
    expect(call?.messages[0]).toEqual({
      role: MessageRole.System,
      content: LLM_SYSTEM_PROMPT,
    });
    expect(call?.messages[1]?.role).toBe(MessageRole.User);
    expect(call?.messages[1]?.content).toContain(SPEC.question.instructions);
    expect(call?.messages[1]?.content).toContain("Water boils");

    expect(state.completed).toBe(true);
    expect(state.messages).toHaveLength(3);
    expect(state.requestBody).toEqual({
      model: "openai/gpt-5-mini",
      messages: call?.messages,
    });
    const outcome = outcomeOf(state.sample.metadata);
    expect(outcome.arm).toBe(DecisionArm.Llm);
    expect(outcome.model).toBe("openai/gpt-5-mini");
    expect(outcome.argmax).toBe("true");
    expect(outcome.confidence).toBeCloseTo(0.8, 12);
    expect(outcome.distribution.true).toBeCloseTo(0.8, 12);
    expect(outcome.distribution.false).toBeCloseTo(0.2, 12);
    expect(outcome.latencyMs).toBe(900);
    expect(outcome.inputTokens).toBe(120);
    expect(outcome.outputTokens).toBe(14);
    expect(outcome.cost).toBeCloseTo(0.0003, 12);
    expect(outcome.violations).toEqual([]);
  });
  it("records a violation instead of failing when the completion is not a closed-set answer", async () => {
    const model: ModelService = {
      generate: () =>
        succeed({
          completion: "It depends.",
          message: { role: MessageRole.Assistant, content: "It depends." },
        }),
    };
    const state = await run(
      llmSolver(model, {
        model: "m",
        inference: { reasoningEffort: "low" },
      })(stateWith(SPEC))
    );
    const outcome = outcomeOf(state.sample.metadata);
    expect(outcome.argmax).toBeNull();
    expect(outcome.distribution).toEqual({});
    expect(outcome.violations.length).toBeGreaterThan(0);
    expect(outcome.cost).toBeNull();
    expect(outcome.latencyMs).toBeGreaterThanOrEqual(0);
  });
});
