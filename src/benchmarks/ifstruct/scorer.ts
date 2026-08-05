import { sync } from "effect/Effect";

import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import { IfStructRequirementsSchema } from "./schema";
import { validateResponse } from "./validate";

export const ifStructScorer: ScorerService = (state) =>
  sync(() => {
    const parsed = parseSchema(
      IfStructRequirementsSchema,
      state.sample.metadata
    );
    if (Either.isLeft(parsed)) {
      return {
        value: ScoreValue.Incorrect,
        answer: null,
        explanation: `ifstruct requirements missing/invalid on sample metadata: ${parsed.left.message}`,
      };
    }
    const completion = state.output?.completion ?? "";
    const result = validateResponse(completion, parsed.right);
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
