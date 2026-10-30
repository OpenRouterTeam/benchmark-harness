import type { Effect } from "effect/Effect";
import { flatMap, gen, mapError } from "effect/Effect";
import { HttpClient, HttpClientRequest } from "effect/http";
import * as Result from "effect/Result";

import { ModelError } from "../harness/core";
import { parseSchema, z } from "../internal/zod";

export const TYPESAFE_API_KEY_ENV = "TYPESAFE_AI_API_KEY";

export const TYPESAFE_SYSTEMONE_URL = "https://api.typesafe.ai/v1/systemone";

export const TypesafeNoulQuestionSchema = z.object({
  type: z.literal("noul"),
  instructions: z.string(),
});

export const TypesafeScoreQuestionSchema = z.object({
  type: z.literal("score"),
  instructions: z.string(),
  criteria: z.array(z.string()).min(2),
});

export const TypesafeQuestionSchema = z.union([
  TypesafeNoulQuestionSchema,
  TypesafeScoreQuestionSchema,
]);

export type TypesafeQuestion = z.infer<typeof TypesafeQuestionSchema>;

const NoulAnswerSchema = z.object({
  type: z.literal("noul"),
  noul: z.number(),
});

const ScoreAnswerSchema = z.object({
  type: z.literal("score"),
  score: z.number(),
  confidence: z.number().optional(),
});

export const TypesafeAnswerSchema = z.union([
  NoulAnswerSchema,
  ScoreAnswerSchema,
]);

export type TypesafeAnswer = z.infer<typeof TypesafeAnswerSchema>;

export const TypesafeResponseSchema = z.object({
  model: z.string(),
  answers: z.record(z.string(), TypesafeAnswerSchema),
  usage: z
    .object({ input_tokens: z.number(), output_tokens: z.number() })
    .optional(),
});

export type TypesafeResponse = z.infer<typeof TypesafeResponseSchema>;

export interface TypesafeJudgeConfig {
  readonly model: string;
  readonly apiKey: string;
  readonly url?: string;
}

export interface TypesafeJudgeService {
  readonly ask: (
    state: unknown,
    questions: Readonly<Record<string, TypesafeQuestion>>
  ) => Effect<TypesafeResponse, ModelError>;
}

export function makeTypesafeJudge(
  client: HttpClient.HttpClient,
  config: TypesafeJudgeConfig
): TypesafeJudgeService {
  const url = config.url ?? TYPESAFE_SYSTEMONE_URL;
  const fail = (message: string): ModelError =>
    new ModelError({ message: `TypeSafe judge: ${message}` });
  return {
    ask: (state, questions) =>
      client
        .execute(
          HttpClientRequest.post(url).pipe(
            HttpClientRequest.setHeaders({
              Authorization: `Bearer ${config.apiKey}`,
              "Content-Type": "application/json",
              "TypeSafe-Danger-Zone-Long-Context": "true",
            }),
            HttpClientRequest.bodyJsonUnsafe({
              model: config.model,
              state,
              questions,
            })
          )
        )
        .pipe(
          mapError((error) => fail(`request failed: ${error.message}`)),
          flatMap((response) =>
            gen(function* () {
              if (response.status < 200 || response.status >= 300) {
                const text = yield* response.text.pipe(
                  mapError((error) =>
                    fail(`response unreadable: ${error.message}`)
                  )
                );
                return yield* fail(`HTTP ${response.status}: ${text}`);
              }
              const body: unknown = yield* response.json.pipe(
                mapError((error) =>
                  fail(`response unreadable: ${error.message}`)
                )
              );
              const parsed = parseSchema(TypesafeResponseSchema, body);
              if (Result.isFailure(parsed)) {
                return yield* fail(
                  `unexpected response body: ${parsed.failure.message}`
                );
              }
              return parsed.success;
            })
          )
        ),
  };
}

export function noulOf(
  answers: Readonly<Record<string, TypesafeAnswer>>,
  name: string
): number | undefined {
  const answer = answers[name];
  return answer?.type === "noul" ? answer.noul : undefined;
}

export function scoreOf(
  answers: Readonly<Record<string, TypesafeAnswer>>,
  name: string
): number | undefined {
  const answer = answers[name];
  return answer?.type === "score" ? answer.score : undefined;
}
