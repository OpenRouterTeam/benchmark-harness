import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import { gen, reduce, succeed } from "effect/Effect";

import type { ModelError, SolverError, TaskState } from "./core";
import { MessageRole } from "./core";
import type { GenerateConfig, ModelService } from "./model";
import type { ProgressReporter } from "./progress";
import { CheckpointStore } from "./progress";

export type SolverService = (
  state: TaskState
) => Effect<
  TaskState,
  ModelError | SolverError,
  ProgressReporter | CheckpointStore
>;

export class Solver extends Tag("@openrouter/bench-harness/solver")<
  Solver,
  SolverService
>() {}

export function chain(...solvers: readonly SolverService[]): SolverService {
  return (initial) =>
    reduce(solvers, initial, (state, solver) =>
      state.completed ? succeed(state) : solver(state)
    );
}

export function systemMessage(content: string): SolverService {
  return (state) =>
    succeed({
      ...state,
      messages: [{ role: MessageRole.System, content }, ...state.messages],
    });
}

export function generate(
  model: ModelService,
  config: GenerateConfig
): SolverService {
  return (state) =>
    gen(function* () {
      const output = yield* model.generate(state.messages, config);
      return {
        ...state,
        output,
        messages: [...state.messages, output.message],
        completed: true,
      };
    });
}
