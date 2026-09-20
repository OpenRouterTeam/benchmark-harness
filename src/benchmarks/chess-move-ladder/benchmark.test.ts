import { describe, expect, it } from "bun:test";

import type { Effect } from "effect/Effect";
import { provide, runSync, succeed } from "effect/Effect";
import { mergeAll, succeed as layerSucceed } from "effect/Layer";

import type { ModelMessage, TaskState } from "../../harness/core";
import { MessageRole, ScoreValue } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../harness/progress";
import type { SolverService } from "../../harness/solver";
import { assertRight } from "../../internal/testing";
import { parseSystemOneRequest } from "../../providers/systemone-model";
import { chessRecordToSample, chessSolver } from "./benchmark";
import { PUZZLE_RECORD } from "./fixtures";
import type { ChessCondition } from "./schema";
import { resolveModelInterface } from "./schema";
import { makeChessScorer } from "./scorer";

const CONDITION: ChessCondition = {
  criteriaLevel: "heuristics",
  heuristicsVariant: "correct",
  boardFormat: "fen",
};

interface Captured {
  messages: readonly ModelMessage[];
  config: GenerateConfig | undefined;
}

function fakeModel(completion: string, captured: Captured): ModelService {
  return {
    generate: (messages, config) => {
      captured.messages = messages;
      captured.config = config;
      return succeed({
        completion,
        message: { role: MessageRole.Assistant, content: completion },
      });
    },
  };
}

const HARNESS_CONTEXT = mergeAll(
  layerSucceed(ProgressReporter, NOOP_PROGRESS_REPORTER),
  layerSucceed(CheckpointStore, NOOP_CHECKPOINT_STORE)
);

function runSolver(solver: SolverService, state: TaskState): TaskState {
  const effect: Effect<TaskState, unknown> = solver(state).pipe(
    provide(HARNESS_CONTEXT)
  );
  return runSync(effect);
}

function initialState(): TaskState {
  return {
    sample: chessRecordToSample(PUZZLE_RECORD, 4),
    messages: [],
    completed: false,
  };
}

describe("chessSolver", () => {
  it("sends a chat prompt with fixed temperature and configured overrides", () => {
    const captured: Captured = { messages: [], config: undefined };
    const solver = chessSolver(fakeModel("Answer: B", captured), {
      condition: CONDITION,
      modelInterface: "chat",
      inference: { reasoningEffort: "low", timeoutMs: 1000 },
    });
    const state = runSolver(solver, initialState());
    expect(captured.messages).toHaveLength(1);
    expect(captured.messages[0]?.role).toBe(MessageRole.User);
    expect(captured.messages[0]?.content).toContain("FEN:");
    expect(captured.config?.temperature).toBe(0);
    expect(captured.config?.reasoningEffort).toBe("low");
    expect(captured.config?.timeoutMs).toBe(1000);
    expect(captured.config).not.toHaveProperty("maxTokens");
    expect(state.output?.completion).toBe("Answer: B");
    expect(state.sample.metadata?.["condition"]).toEqual(CONDITION);
    expect(state.sample.metadata?.["modelInterface"]).toBe("chat");
  });
  it("sends a typed System One request for the systemone interface", () => {
    const captured: Captured = { messages: [], config: undefined };
    const solver = chessSolver(fakeModel("{}", captured), {
      condition: CONDITION,
      modelInterface: "systemone",
      inference: { reasoningEffort: "low" },
    });
    runSolver(solver, initialState());
    const request = parseSystemOneRequest(captured.messages);
    assertRight(request);
    expect(request.right.questions["best_move"]?.type).toBe("choice");
    expect(request.right.state).toHaveProperty("heuristics");
  });
  it("scores the solver output end to end", () => {
    const captured: Captured = { messages: [], config: undefined };
    const sample = chessRecordToSample(PUZZLE_RECORD, 4);
    const solver = chessSolver(
      fakeModel(`Answer: ${sample.target.text}`, captured),
      {
        condition: CONDITION,
        modelInterface: "chat",
        inference: { reasoningEffort: "low" },
      }
    );
    const state = runSolver(solver, initialState());
    const score = runSync(makeChessScorer("chat")(state, sample.target));
    expect(score.value).toBe(ScoreValue.Correct);
  });
});

describe("resolveModelInterface", () => {
  it("defaults Jev models to systemone and others to chat", () => {
    expect(resolveModelInterface("typesafe/jev-1.13", undefined)).toBe(
      "systemone"
    );
    expect(resolveModelInterface("jev-latest", undefined)).toBe("systemone");
    expect(resolveModelInterface("openai/gpt-5", undefined)).toBe("chat");
  });
  it("honours an explicit override", () => {
    expect(resolveModelInterface("typesafe/jev-1.13", "chat")).toBe("chat");
  });
});
