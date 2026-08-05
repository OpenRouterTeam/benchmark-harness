import { z } from "../internal/zod";

export const RESULT_FORMAT_VERSION = 1 as const;

export const RESULT_WRITER = "openrouter-bench" as const;

export const ScorerTrajectorySchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("verifier_log"), log: z.string() }),
  z.object({
    kind: z.literal("judge_runs"),
    runs: z.array(z.unknown()).readonly(),
  }),
]);

export const BenchmarkResultRowSchema = z.object({
  format_version: z.number(),
  task: z.string(),
  model: z.string(),
  epochs: z.number(),
  temperature: z.number().nullable(),
  benchmark_config: z.string().nullish(),
  created_at: z.string(),
  accuracy: z.number(),
  total_questions: z.number(),
  correct_answers: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  total_tokens: z.number(),
  reasoning_tokens: z.number(),
  total_cost: z.number(),
  generation_time_ms: z.number(),
  epoch_total_questions: z.number().nullish(),
  epoch_correct_answers: z.number().nullish(),
  extra_scores: z.string().nullable(),
  primary_score: z.string().nullish(),
  sample_id: z.string(),
  epoch: z.number(),
  input: z.string().nullable(),
  target: z.string().nullable(),
  score_value: z.string(),
  answer: z.string().nullable(),
  explanation: z.string().nullable(),
  scorer_trajectory: z.string().nullish(),
  response_items: z.string().nullish(),
  generation_ids: z.string().nullish(),
  messages: z.string().nullable(),
  metadata: z.string().nullable(),
});

export type BenchmarkResultRow = z.infer<typeof BenchmarkResultRowSchema>;
