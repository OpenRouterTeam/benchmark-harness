import type { Effect } from "effect/Effect";
import { fail, flatMap, gen, mapError, succeed } from "effect/Effect";
import { HttpClient, HttpClientRequest } from "effect/http";
import * as Result from "effect/Result";

import { parseSchema, z } from "../internal/zod";

export const DEFAULT_OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1";

export interface DecisionsTarget {
  readonly apiKey: string;
  readonly baseUrl?: string;
}

export interface DecisionChoiceQuestion {
  readonly type: "choice";
  readonly instructions: string;
  readonly criteria: Readonly<Record<string, string>>;
}

export class DecisionsError extends Error {
  readonly _tag = "DecisionsError";
  override readonly name = "DecisionsError";
}

const DecisionsResponseSchema = z.object({
  answers: z.record(
    z.string(),
    z.object({ probabilities: z.record(z.string(), z.number()) })
  ),
});

export function decisionsUrl(baseUrl?: string): string {
  const trimmed = (baseUrl ?? DEFAULT_OPENROUTER_BASE_URL).replace(/\/+$/, "");
  const base = trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
  return new URL("../alpha/decisions", `${base}/`).toString();
}

export function decisionChoiceProbabilities(
  client: HttpClient.HttpClient,
  target: DecisionsTarget,
  model: string,
  state: Readonly<Record<string, string>>,
  question: DecisionChoiceQuestion
): Effect<Readonly<Record<string, number>>, DecisionsError> {
  const request = HttpClientRequest.post(decisionsUrl(target.baseUrl)).pipe(
    HttpClientRequest.setHeaders({
      Authorization: `Bearer ${target.apiKey}`,
      "Content-Type": "application/json",
    }),
    HttpClientRequest.bodyJsonUnsafe({
      model,
      state,
      questions: { level: question },
    })
  );
  return client.execute(request).pipe(
    mapError(
      (error) =>
        new DecisionsError(`decisions request failed: ${error.message}`)
    ),
    flatMap((response) =>
      gen(function* () {
        if (response.status < 200 || response.status >= 300) {
          const text = yield* response.text.pipe(mapError(readError));
          return yield* fail(
            new DecisionsError(
              `decisions HTTP ${response.status}: ${text.slice(0, 300)}`
            )
          );
        }
        const body: unknown = yield* response.json.pipe(mapError(readError));
        const parsed = parseSchema(DecisionsResponseSchema, body);
        if (Result.isFailure(parsed)) {
          return yield* fail(
            new DecisionsError("decisions response had no answers")
          );
        }
        const level = parsed.success.answers["level"];
        return level === undefined
          ? yield* fail(
              new DecisionsError("decisions response had no level answer")
            )
          : yield* succeed(level.probabilities);
      })
    )
  );
}

function readError(error: { readonly message: string }): DecisionsError {
  return new DecisionsError(`decisions response unreadable: ${error.message}`);
}
