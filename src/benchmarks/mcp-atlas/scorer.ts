import type { Effect } from "effect/Effect";
import { forEach, gen, succeed } from "effect/Effect";
import * as Result from "effect/Result";

import type { Score, TaskState, Target } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
import type { ScorerService } from "../../harness/scorer";
import type { SolverService } from "../../harness/solver";
import type { ValueOf } from "../../internal/guards";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { JudgeCallSpec, JudgeConfig } from "../../judge/judge";
import { judgeCall } from "../../judge/judge";
import type { ResponsesService } from "../../providers/responses-client";
import { mergeModelUsages } from "../search/core/usage";
import { MCP_ATLAS_TASK_METADATA_KEY, McpAtlasTaskSchema } from "./dataset";

export const MCP_ATLAS_VERDICT_METADATA_KEY = "mcp_atlas_verdict";

export const CLAIM_OUTCOMES = [
  "fulfilled",
  "partially_fulfilled",
  "not_fulfilled",
] as const;

export type ClaimOutcome = ValueOf<typeof CLAIM_OUTCOMES>;

export const ClaimVerdictSchema = z.object({
  coverage_outcome: z.enum(CLAIM_OUTCOMES),
  justification: z.string(),
  confidence_level: z.number(),
});

export type ClaimVerdict = z.infer<typeof ClaimVerdictSchema>;

export const ClaimCoverageSchema = z.object({
  claim: z.string(),
  outcome: z.enum(CLAIM_OUTCOMES),
  score: z.number(),
  justification: z.string(),
});

export const CoverageVerdictSchema = z.object({
  coverage: z.number(),
  claims: z.array(ClaimCoverageSchema).readonly(),
});

export type CoverageVerdict = z.infer<typeof CoverageVerdictSchema>;

export const MCP_ATLAS_COVERAGE_METRIC_NAMES = [
  "coverage_mean",
  "pass_rate_0_50",
  "pass_rate_0_75",
] as const;

const CLAIM_VERDICT_JSON_SCHEMA = {
  type: "object",
  properties: {
    coverage_outcome: { type: "string", enum: [...CLAIM_OUTCOMES] },
    justification: { type: "string" },
    confidence_level: { type: "number" },
  },
  required: ["coverage_outcome", "justification", "confidence_level"],
  additionalProperties: false,
} as const;

const PARSE_FAILURE_VERDICT: ClaimVerdict = {
  coverage_outcome: "not_fulfilled",
  justification: "judge verdict could not be parsed",
  confidence_level: 0.1,
};

const EMPTY_RESPONSE_JUSTIFICATION = "the model produced no final response";

const JUDGE_CONCURRENCY = 4;

export function claimScore(outcome: ClaimOutcome): number {
  switch (outcome) {
    case "fulfilled": {
      return 1;
    }
    case "partially_fulfilled": {
      return 0.5;
    }
    case "not_fulfilled": {
      return 0;
    }
    default: {
      return outcome satisfies never;
    }
  }
}

export function coverageFromScores(scores: readonly number[]): number {
  if (scores.length === 0) {
    return 0;
  }
  const mean = scores.reduce((sum, score) => sum + score, 0) / scores.length;
  return Math.round(mean * 1000) / 1000;
}

export function claimJudgePrompt(claim: string, response: string): string {
  return `You are evaluating how well a model's response addresses a specific expert-defined claim.

SCORING CRITERIA:
- fulfilled: Claim is completely and accurately addressed. The response covers all key details.
- partially_fulfilled: Claim is partially addressed. The response covers some but not all key details.
- not_fulfilled: Claim is not addressed. The response does not include any key details.

NUMERICAL COMPARISON GUIDELINES:
- For numerical values, use reasonable approximation thresholds:
  * Exact match NOT required for decimals
  * Values within 5% of the claimed number are considered matching
  * For percentages, ±1 percentage points is acceptable
  * Round to appropriate significant figures based on context
- Consider the precision appropriate to the domain:
  * Scientific measurements may need higher precision
  * General statistics/estimates can have looser matching
  * Financial figures should match to reasonable business precision (e.g., millions/billions don't need exact cents)
- If a number is expressed differently but mathematically equivalent (e.g., "0.5" vs "50%" vs "half"), consider it a match

CLAIM TO EVALUATE:
${claim}

MODEL RESPONSE TO ANALYZE:
${response}

INSTRUCTIONS:
1. Determine if the core requirement of the claim is met in the response
2. Check if all key components from the claim appear substantively in the response
   - For numerical values, apply the flexible matching guidelines above
   - Focus on whether the same magnitude and meaning are conveyed
3. Assign the appropriate coverage_outcome
4. Provide specific justification referencing what was/wasn't covered
   - When numbers differ slightly, note if they're within acceptable range
5. Provide a confidence level (0.0-1.0) for your assessment

Be rigorous but fair in your assessment. Focus on whether the response conveys the same information as the claim, not on exact numerical precision unless precision is critical to the claim's meaning.`;
}

export function parseClaimVerdict(
  text: string
): Result.Result<ClaimVerdict, string> {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch (error) {
    return Result.fail(
      `verdict is not JSON: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  const parsed = parseSchema(ClaimVerdictSchema, json);
  return Result.isFailure(parsed)
    ? Result.fail(parsed.failure.message)
    : Result.succeed(parsed.success);
}

export function claimJudgeSpec(
  claim: string,
  response: string
): JudgeCallSpec<ClaimVerdict> {
  return {
    userInput: claimJudgePrompt(claim, response),
    schemaName: "mcp_atlas_claim_coverage",
    jsonSchema: CLAIM_VERDICT_JSON_SCHEMA,
    parseVerdict: parseClaimVerdict,
    parseFailureFallback: PARSE_FAILURE_VERDICT,
  };
}

export function buildCoverageVerdict(
  claims: readonly string[],
  verdicts: readonly ClaimVerdict[]
): CoverageVerdict {
  const scored = claims.map((claim, index) => {
    const verdict = verdicts[index] ?? PARSE_FAILURE_VERDICT;
    return {
      claim,
      outcome: verdict.coverage_outcome,
      score: claimScore(verdict.coverage_outcome),
      justification: verdict.justification,
    };
  });
  return {
    coverage: coverageFromScores(scored.map((item) => item.score)),
    claims: scored,
  };
}

function emptyResponseVerdict(claims: readonly string[]): CoverageVerdict {
  return buildCoverageVerdict(
    claims,
    claims.map(() => ({
      coverage_outcome: "not_fulfilled",
      justification: EMPTY_RESPONSE_JUSTIFICATION,
      confidence_level: 1,
    }))
  );
}

export function mcpAtlasJudgeStage(
  responses: ResponsesService,
  judgeConfig: JudgeConfig
): SolverService {
  return (state) =>
    gen(function* () {
      const task = parseSchema(
        McpAtlasTaskSchema,
        state.sample.metadata?.[MCP_ATLAS_TASK_METADATA_KEY]
      );
      const claims = Result.isSuccess(task) ? task.success.claims : [];
      const response = state.output?.completion ?? "";
      if (response.trim() === "") {
        return withVerdict(state, emptyResponseVerdict(claims), undefined);
      }
      const judged = yield* forEach(
        claims,
        (claim) =>
          judgeCall(responses, judgeConfig, claimJudgeSpec(claim, response)),
        { concurrency: JUDGE_CONCURRENCY }
      );
      const verdict = buildCoverageVerdict(
        claims,
        judged.map((result) => result.verdict)
      );
      const usage = mergeModelUsages([
        state.output?.usage,
        ...judged.map((result) => result.usage),
      ]);
      return withVerdict(state, verdict, usage);
    });
}

function withVerdict(
  state: TaskState,
  verdict: CoverageVerdict,
  usage: ReturnType<typeof mergeModelUsages>
): TaskState {
  return {
    ...state,
    ...definedValues({
      output:
        state.output !== undefined
          ? { ...state.output, ...definedValues({ usage }) }
          : undefined,
    }),
    sample: {
      ...state.sample,
      metadata: {
        ...state.sample.metadata,
        [MCP_ATLAS_VERDICT_METADATA_KEY]: verdict,
      },
    },
  };
}

export function sampleVerdict(state: TaskState): CoverageVerdict | undefined {
  const parsed = parseSchema(
    CoverageVerdictSchema,
    state.sample.metadata?.[MCP_ATLAS_VERDICT_METADATA_KEY]
  );
  return Result.isSuccess(parsed) ? parsed.success : undefined;
}

export function makeMcpAtlasScorer(passThreshold: number): ScorerService {
  return (state: TaskState, _target: Target): Effect<Score, never> => {
    const verdict = sampleVerdict(state);
    const answer = state.output?.completion ?? null;
    if (verdict === undefined) {
      return succeed({
        value: ScoreValue.Incorrect,
        answer,
        explanation: "no claim-coverage verdict recorded",
      });
    }
    const fulfilled = verdict.claims.filter(
      (claim) => claim.outcome === "fulfilled"
    ).length;
    const partial = verdict.claims.filter(
      (claim) => claim.outcome === "partially_fulfilled"
    ).length;
    return succeed({
      value:
        verdict.coverage >= passThreshold
          ? ScoreValue.Correct
          : ScoreValue.Incorrect,
      answer,
      explanation: `claim coverage ${verdict.coverage} (${fulfilled} fulfilled, ${partial} partial of ${verdict.claims.length}; pass at ${passThreshold})`,
      trajectory: { kind: "judge_runs", runs: [verdict] },
    });
  };
}

export function mcpAtlasRunLevelScores(result: RunResult): readonly {
  readonly name: string;
  readonly metrics: Readonly<Record<string, { readonly value: number }>>;
}[] {
  const coverages = result.sampleScores.flatMap((sample) => {
    if (sample.score.value === ScoreValue.Skipped) {
      return [];
    }
    const trajectory = sample.score.trajectory;
    if (trajectory?.kind !== "judge_runs") {
      return [0];
    }
    const parsed = parseSchema(CoverageVerdictSchema, trajectory.runs[0]);
    return [Result.isSuccess(parsed) ? parsed.success.coverage : 0];
  });
  if (coverages.length === 0) {
    return [];
  }
  const rate = (threshold: number): number =>
    coverages.filter((coverage) => coverage >= threshold).length /
    coverages.length;
  return [
    {
      name: "mcp_atlas_coverage",
      metrics: {
        coverage_mean: { value: coverageFromScores(coverages) },
        pass_rate_0_50: { value: rate(0.5) },
        pass_rate_0_75: { value: rate(0.75) },
      },
    },
  ];
}
