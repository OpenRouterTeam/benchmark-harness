import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";

import type { Score, Target, TaskState } from "./core";

export type ScorerService = (
  state: TaskState,
  target: Target
) => Effect<Score, never>;

export class Scorer extends Tag("@openrouter/bench-harness/scorer")<
  Scorer,
  ScorerService
>() {}
