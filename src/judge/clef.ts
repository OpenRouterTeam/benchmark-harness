import type { Effect } from "effect/Effect";
import { fail, flatMap, gen, mapError, succeed } from "effect/Effect";
import { HttpClient, HttpClientRequest } from "effect/http";
import * as Result from "effect/Result";

import { parseSchema, z } from "../internal/zod";

export const CLEF_MODEL = "@cf/cloudflare/clef-flash";

export interface ClefCredentials {
  readonly accountId: string;
  readonly apiToken: string;
}

export interface ClefChoiceQuestion {
  readonly type: "choice";
  readonly instructions: string;
  readonly criteria: Readonly<Record<string, string>>;
}

export class ClefError extends Error {
  readonly _tag = "ClefError";
  override readonly name = "ClefError";
}

const ClefResponseSchema = z.object({
  result: z.object({
    answers: z.record(
      z.string(),
      z.object({ probabilities: z.record(z.string(), z.number()) })
    ),
  }),
});

export function clefCredentialsFromEnv(
  env: Readonly<Record<string, string | undefined>>
): ClefCredentials | null {
  const accountId = env["CLOUDFLARE_ACCOUNT_ID"];
  const apiToken = env["CLOUDFLARE_API_TOKEN"];
  if (!accountId || !apiToken) {
    return null;
  }
  return { accountId, apiToken };
}

export function clefChoiceProbabilities(
  client: HttpClient.HttpClient,
  credentials: ClefCredentials,
  state: Readonly<Record<string, string>>,
  question: ClefChoiceQuestion
): Effect<Readonly<Record<string, number>>, ClefError> {
  const url = `https://api.cloudflare.com/client/v4/accounts/${credentials.accountId}/ai/run/${CLEF_MODEL}`;
  const request = HttpClientRequest.post(url).pipe(
    HttpClientRequest.setHeaders({
      Authorization: `Bearer ${credentials.apiToken}`,
      "Content-Type": "application/json",
    }),
    HttpClientRequest.bodyJsonUnsafe({
      state,
      questions: { level: question },
    })
  );
  return client.execute(request).pipe(
    mapError((error) => new ClefError(`Clef request failed: ${error.message}`)),
    flatMap((response) =>
      gen(function* () {
        if (response.status < 200 || response.status >= 300) {
          const text = yield* response.text.pipe(mapError(readError));
          return yield* fail(
            new ClefError(`Clef HTTP ${response.status}: ${text.slice(0, 300)}`)
          );
        }
        const body: unknown = yield* response.json.pipe(mapError(readError));
        const parsed = parseSchema(ClefResponseSchema, body);
        if (Result.isFailure(parsed)) {
          return yield* fail(new ClefError("Clef response had no answers"));
        }
        const level = parsed.success.result.answers["level"];
        return level === undefined
          ? yield* fail(new ClefError("Clef response had no level answer"))
          : yield* succeed(level.probabilities);
      })
    )
  );
}

function readError(error: { readonly message: string }): ClefError {
  return new ClefError(`Clef response unreadable: ${error.message}`);
}
