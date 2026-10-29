import { sync } from "effect/Effect";
import * as Result from "effect/Result";

import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { parseSchema } from "../../internal/zod";
import { IfStructRequirementsSchema } from "./schema";
import { validateResponse } from "./validate";

export const ifStructScorer: ScorerService = (state) =>
  sync(() => {
    const parsed = parseSchema(
      IfStructRequirementsSchema,
      state.sample.metadata
    );
    if (Result.isFailure(parsed)) {
      return {
        value: ScoreValue.Incorrect,
        answer: null,
        explanation: `ifstruct requirements missing/invalid on sample metadata: ${parsed.failure.message}`,
      };
    }
    const completion = state.output?.completion ?? "";
    const result = validateResponse(completion, parsed.success);
    const ratio = result.details.schemaMatchRatio ?? 0;
    if (result.passed) {
      return {
        value: ScoreValue.Correct,
        answer: completion,
        explanation: `Passed all ifstruct checks (schema match ${(ratio * 100).toFixed(0)}%)`,
      };
    }
    return {
      value: ScoreValue.Incorrect,
      answer: completion,
      explanation: `Failed ifstruct checks (schema match ${(ratio * 100).toFixed(0)}%): ${result.errors.join("; ")}`,
    };
  });
